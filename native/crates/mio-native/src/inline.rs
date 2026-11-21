//! TIR 层小函数内联（结构层优化，TL20261121T210405）。
//!
//! 为什么需要：Cranelift speed 档不做函数内联（它的优化 pass 是机器指令级的），
//! 而小函数调用的固定开销（prologue/epilogue + 参数搬运 + call/ret）在热点循环里
//! 可占 3 倍时间（实测：1e8 次 `add` 调用 0.20s → 内联后 0.06s）。
//!
//! MVP 的保守边界（每一条都有明确理由）：
//! - 只内联**直线条叶子函数**：体 ≤ 8 条语句、仅 `Let`/`Expr`、末语句恰好一条
//!   `Return(Some(_))`、体内无任何 `Call`。无 Call 同时排除了递归与互递归，
//!   也保证内联副本自包含（自由变量只有参数）。
//! - 只处理三种调用语境：顶层语句、`let` 初始化式、赋值 rhs。嵌套表达式语境
//!   （如 `print(add(1, 2))`）保留调用——表达式无法携带语句序列，语句化提升
//!   留待后续（如有需要）。
//! - `main` 不内联（它是入口）；内联后无剩余调用点的函数从程序中移除。

use std::collections::HashMap;

use crate::lower::{Expr, Func, Program, Stmt};
use crate::types::MachineType;

/// 内联入口：返回内联后的程序（类型正确性不变——实参类型与形参一致已由
/// check 阶段保证，副本只是复制）。
#[must_use]
pub fn inline(mut program: Program) -> Program {
    // 第一遍：收集可内联函数的定义。若为空，原样返回（零改动）。
    let defs: HashMap<String, Func> = program
        .functions
        .iter()
        .filter(|f| is_inlinable(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if defs.is_empty() {
        return program;
    }

    // 第二遍：逐函数重写调用点（take 出 functions；定义查询走 defs）。
    let mut functions = Vec::with_capacity(program.functions.len());
    let mut counter: u64 = 0;
    for func in std::mem::take(&mut program.functions) {
        if func.name == "main" || !defs.contains_key(&func.name) {
            let mut body = Vec::with_capacity(func.body.len());
            for stmt in func.body {
                inline_stmt(stmt, &defs, &mut counter, &mut body);
            }
            functions.push(Func {
                name: func.name,
                params: func.params,
                ret: func.ret,
                body,
            });
        }
        // 可内联的函数定义在内联后移除（其调用点已被展开；main 除外，但 main
        // 不可内联，不会走到这里）。
    }

    // 收尾：扫描剩余程序里仍被引用的可内联函数（嵌套语境未展开的调用点），
    // 把它们的定义保留回去——「移除定义 + 残留调用」会产生难懂的「函数未声明」。
    let remaining_calls = collect_call_names(&functions);
    for (name, func) in &defs {
        if name != "main" && remaining_calls.contains(name) {
            functions.push(func.clone());
        }
    }

    Program { functions }
}

/// 收集程序里剩余的全部函数调用名字（含嵌套块）。
fn collect_call_names(functions: &[Func]) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for func in functions {
        for stmt in &func.body {
            collect_calls_in_block(stmt, &mut names);
        }
    }
    names
}

fn collect_calls_in_block(stmt: &Stmt, names: &mut std::collections::HashSet<String>) {
    match stmt {
        Stmt::Expr(e) | Stmt::Let { init: e, .. } => collect_calls_in_expr(e, names),
        Stmt::If {
            cond, then, els, ..
        } => {
            collect_calls_in_expr(cond, names);
            for s in then {
                collect_calls_in_block(s, names);
            }
            if let Some(els) = els {
                for s in els {
                    collect_calls_in_block(s, names);
                }
            }
        }
        Stmt::While { cond, body } => {
            collect_calls_in_expr(cond, names);
            for s in body {
                collect_calls_in_block(s, names);
            }
        }
        Stmt::For {
            init,
            cond,
            step,
            body,
            ..
        } => {
            collect_calls_in_expr(init, names);
            collect_calls_in_expr(cond, names);
            collect_calls_in_expr(step, names);
            for s in body {
                collect_calls_in_block(s, names);
            }
        }
        Stmt::Return(e) => {
            if let Some(e) = e {
                collect_calls_in_expr(e, names);
            }
        }
        Stmt::Break | Stmt::Continue => {}
    }
}

fn collect_calls_in_expr(e: &Expr, names: &mut std::collections::HashSet<String>) {
    match e {
        Expr::Call { name, args, .. } => {
            names.insert(name.clone());
            for arg in args {
                collect_calls_in_expr(arg, names);
            }
        }
        Expr::UnaryNeg(inner) | Expr::Not(inner) => collect_calls_in_expr(inner, names),
        Expr::Binary { lhs, rhs, .. } => {
            collect_calls_in_expr(lhs, names);
            collect_calls_in_expr(rhs, names);
        }
        Expr::Cond {
            cond, then, els, ..
        } => {
            collect_calls_in_expr(cond, names);
            collect_calls_in_expr(then, names);
            collect_calls_in_expr(els, names);
        }
        Expr::Cast { value, .. } | Expr::Assign { value, .. } => {
            collect_calls_in_expr(value, names);
        }
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool(_) | Expr::Var(..) => {}
    }
}

/// 可内联判定：直线条叶子小函数。
fn is_inlinable(f: &Func) -> bool {
    if f.name == "main" || f.body.len() > 8 {
        return false;
    }
    let Some(Stmt::Return(Some(_))) = f.body.last() else {
        return false;
    };
    // 除末语句外，其余必须全部是 Let/Expr（直线条）；且全体内无 Call。
    for stmt in &f.body[..f.body.len() - 1] {
        match stmt {
            Stmt::Let { .. } | Stmt::Expr(_) => {}
            _ => return false,
        }
    }
    !block_has_call(&f.body)
}

/// 块内是否含函数调用（含嵌套 if/while/for 的体）。
fn block_has_call(body: &[Stmt]) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Expr(e) | Stmt::Let { init: e, .. } => expr_has_call(e),
        Stmt::If { then, els, .. } => {
            block_has_call(then) || els.as_deref().is_some_and(block_has_call)
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => block_has_call(body),
        Stmt::Return(e) => e.as_ref().is_some_and(expr_has_call),
        Stmt::Break | Stmt::Continue => false,
    })
}

/// 表达式内是否含函数调用。
fn expr_has_call(e: &Expr) -> bool {
    match e {
        Expr::Call { .. } => true,
        Expr::UnaryNeg(inner) | Expr::Not(inner) => expr_has_call(inner),
        Expr::Binary { lhs, rhs, .. } => expr_has_call(lhs) || expr_has_call(rhs),
        Expr::Cond {
            cond, then, els, ..
        } => expr_has_call(cond) || expr_has_call(then) || expr_has_call(els),
        Expr::Cast { value, .. } | Expr::Assign { value, .. } => expr_has_call(value),
        Expr::Int { .. } | Expr::Float { .. } | Expr::Bool(_) | Expr::Var(..) => false,
    }
}

/// 重写一条语句：展开其中的可内联调用，输出零条或多条语句。
fn inline_stmt(stmt: Stmt, defs: &HashMap<String, Func>, counter: &mut u64, out: &mut Vec<Stmt>) {
    match stmt {
        // let x = f(args) → 参数承接 + 函数体副本 + 结果变量，原 Let 的 init 换引用。
        Stmt::Let { name, ty, init } => {
            if let Expr::Call {
                name: callee, args, ..
            } = &init
                && let Some(expanded) = try_inline_call(callee, args, defs, counter)
            {
                out.extend(expanded.prelude);
                out.push(Stmt::Let {
                    name,
                    ty,
                    init: Expr::Var(expanded.result_name, expanded.result_ty),
                });
                return;
            }
            out.push(Stmt::Let { name, ty, init });
        }
        // f(args); 或 x = f(args); → 参数承接 + 函数体副本 + 结果变量。
        // 语句级赋值的 rhs 是调用：展开后 rhs 换引用。
        Stmt::Expr(e) => {
            if let Expr::Call {
                name: callee, args, ..
            } = &e
                && let Some(expanded) = try_inline_call(callee, args, defs, counter)
            {
                // prelude 尾部已含结果变量的 Let（值正确）；本语境无人读取它，
                // 作为 SSA 死值由后端消除。
                out.extend(expanded.prelude);
                return;
            }
            if let Expr::Assign { name, ty, value } = &e
                && let Expr::Call {
                    name: callee, args, ..
                } = value.as_ref()
                && let Some(expanded) = try_inline_call(callee, args, defs, counter)
            {
                out.extend(expanded.prelude);
                out.push(Stmt::Expr(Expr::Assign {
                    name: name.clone(),
                    ty: *ty,
                    value: Box::new(Expr::Var(expanded.result_name, expanded.result_ty)),
                }));
                return;
            }
            out.push(Stmt::Expr(e));
        }
        // x = f(args) → 同 let，赋值 rhs 换引用。
        Stmt::If { cond, then, els } => {
            let mut then_out = Vec::new();
            for s in then {
                inline_stmt(s, defs, counter, &mut then_out);
            }
            let els_out = els.map(|els| {
                let mut v = Vec::new();
                for s in els {
                    inline_stmt(s, defs, counter, &mut v);
                }
                v
            });
            out.push(Stmt::If {
                cond,
                then: then_out,
                els: els_out,
            });
        }
        Stmt::While { cond, body } => {
            let mut body_out = Vec::new();
            for s in body {
                inline_stmt(s, defs, counter, &mut body_out);
            }
            out.push(Stmt::While {
                cond,
                body: body_out,
            });
        }
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => {
            let mut body_out = Vec::new();
            for s in body {
                inline_stmt(s, defs, counter, &mut body_out);
            }
            out.push(Stmt::For {
                var,
                ty,
                init,
                cond,
                step,
                body: body_out,
            });
        }
        // 赋值的 rhs 是调用：展开后 rhs 换引用。
        other => out.push(other),
    }
}

/// 内联展开的一个调用点。
struct Expansion {
    /// 前置语句序列（参数承接 + 函数体副本 + 结果变量）。
    prelude: Vec<Stmt>,
    /// 结果变量名（调用点替换为对它的引用）。
    result_name: String,
    result_ty: MachineType,
}

/// 尝试内联一次调用；不可内联（候选集外 / 实参形态不支持）时返回 None。
fn try_inline_call(
    callee: &str,
    args: &[Expr],
    defs: &HashMap<String, Func>,
    counter: &mut u64,
) -> Option<Expansion> {
    let func = defs.get(callee)?;
    if func.params.len() != args.len() {
        return None;
    }

    *counter += 1;
    let call_id = *counter;
    let mangle = |name: &str| format!("{name}#{call_id}");

    let mut prelude = Vec::new();

    // 参数承接：每个参数变为以实参初始化的 Let（实参表达式复制一份）。
    for (param, arg) in func.params.iter().zip(args.iter()) {
        prelude.push(Stmt::Let {
            name: mangle(&param.name),
            ty: param.ty,
            init: arg.clone(),
        });
    }

    // 函数体副本（除末语句 Return）：变量与赋值目标全部重命名。
    for stmt in &func.body[..func.body.len() - 1] {
        prelude.push(rename_stmt(stmt, &mangle));
    }

    // 末语句 Return(Some(expr)) → 结果变量 Let。
    let result_name = format!("{callee}.ret#{call_id}");
    let result_ty = func.ret;
    match func.body.last() {
        Some(Stmt::Return(Some(expr))) => {
            let renamed = rename_expr(expr, &mangle);
            prelude.push(Stmt::Let {
                name: result_name.clone(),
                ty: result_ty,
                init: renamed,
            });
        }
        _ => unreachable!("is_inlinable 已保证末语句是 Return(Some)"),
    }

    Some(Expansion {
        prelude,
        result_name,
        result_ty,
    })
}

/// 语句内变量重命名（递归处理嵌套块）。
fn rename_stmt(stmt: &Stmt, mangle: &dyn Fn(&str) -> String) -> Stmt {
    match stmt {
        Stmt::Let { name, ty, init } => Stmt::Let {
            name: mangle(name),
            ty: *ty,
            init: rename_expr(init, mangle),
        },
        Stmt::Expr(e) => Stmt::Expr(rename_expr(e, mangle)),
        Stmt::If { cond, then, els } => Stmt::If {
            cond: rename_expr(cond, mangle),
            then: then.iter().map(|s| rename_stmt(s, mangle)).collect(),
            els: els
                .as_ref()
                .map(|b| b.iter().map(|s| rename_stmt(s, mangle)).collect()),
        },
        Stmt::While { cond, body } => Stmt::While {
            cond: rename_expr(cond, mangle),
            body: body.iter().map(|s| rename_stmt(s, mangle)).collect(),
        },
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => Stmt::For {
            var: mangle(var),
            ty: *ty,
            init: rename_expr(init, mangle),
            cond: rename_expr(cond, mangle),
            step: rename_expr(step, mangle),
            body: body.iter().map(|s| rename_stmt(s, mangle)).collect(),
        },
        Stmt::Return(e) => Stmt::Return(e.as_ref().map(|e| rename_expr(e, mangle))),
        Stmt::Break | Stmt::Continue => stmt.clone(),
    }
}

/// 表达式内变量引用与赋值目标重命名。
fn rename_expr(e: &Expr, mangle: &dyn Fn(&str) -> String) -> Expr {
    match e {
        Expr::Var(name, ty) => Expr::Var(mangle(name), *ty),
        Expr::Assign { name, ty, value } => Expr::Assign {
            name: mangle(name),
            ty: *ty,
            value: Box::new(rename_expr(value, mangle)),
        },
        Expr::UnaryNeg(inner) => Expr::UnaryNeg(Box::new(rename_expr(inner, mangle))),
        Expr::Not(inner) => Expr::Not(Box::new(rename_expr(inner, mangle))),
        Expr::Binary { op, lhs, rhs, ty } => Expr::Binary {
            op: *op,
            lhs: Box::new(rename_expr(lhs, mangle)),
            rhs: Box::new(rename_expr(rhs, mangle)),
            ty: *ty,
        },
        Expr::Cond {
            cond,
            then,
            els,
            ty,
        } => Expr::Cond {
            cond: Box::new(rename_expr(cond, mangle)),
            then: Box::new(rename_expr(then, mangle)),
            els: Box::new(rename_expr(els, mangle)),
            ty: *ty,
        },
        Expr::Cast { value, from, to } => Expr::Cast {
            value: Box::new(rename_expr(value, mangle)),
            from: *from,
            to: *to,
        },
        // Call 不重命名（可内联函数体内无 Call；此处保留原样兜底）。
        other => other.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::{BinOp, Param};

    /// 构造一个 `fn(a: i32) -> i32 { return a + 1; }` 形式的函数。
    fn helper(name: &str) -> Func {
        Func {
            name: String::from(name),
            params: vec![Param {
                name: String::from("a"),
                ty: MachineType::I32,
            }],
            ret: MachineType::I32,
            body: vec![Stmt::Return(Some(Expr::Binary {
                op: BinOp::Add,
                lhs: Box::new(Expr::Var(String::from("a"), MachineType::I32)),
                rhs: Box::new(Expr::Int {
                    value: 1,
                    ty: MachineType::I32,
                }),
                ty: MachineType::I32,
            }))],
        }
    }

    fn main_with(body: Vec<Stmt>) -> Func {
        Func {
            name: String::from("main"),
            params: vec![],
            ret: MachineType::Void,
            body,
        }
    }

    #[test]
    fn inline_expands_let_call_and_renames_locals() {
        let program = Program {
            functions: vec![
                helper("inc"),
                main_with(vec![Stmt::Let {
                    name: String::from("x"),
                    ty: MachineType::I32,
                    init: Expr::Call {
                        name: String::from("inc"),
                        args: vec![Expr::Int {
                            value: 41,
                            ty: MachineType::I32,
                        }],
                        ty: MachineType::I32,
                    },
                }]),
            ],
        };

        let out = inline(program);
        // inc 被移除，main 保留。
        assert_eq!(out.functions.len(), 1);
        assert_eq!(out.functions[0].name, "main");
        // 展开后：参数承接 Let、结果 Let、原 Let（init 换 Var 引用）。
        let body = &out.functions[0].body;
        assert_eq!(body.len(), 3);
        assert!(matches!(&body[0], Stmt::Let { name, .. } if name == "a#1"));
        assert!(matches!(&body[1], Stmt::Let { name, .. } if name == "inc.ret#1"));
        match &body[2] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::Var(name, _) if name == "inc.ret#1"));
            }
            other => panic!("末语句应为 Let，得到 {other:?}"),
        }
    }

    #[test]
    fn recursive_function_is_not_inlined() {
        // fib 体内有 Call，不可内联。
        let program = Program {
            functions: vec![
                Func {
                    name: String::from("fib"),
                    params: vec![Param {
                        name: String::from("n"),
                        ty: MachineType::I64,
                    }],
                    ret: MachineType::I64,
                    body: vec![Stmt::Return(Some(Expr::Call {
                        name: String::from("fib"),
                        args: vec![Expr::Int {
                            value: 1,
                            ty: MachineType::I64,
                        }],
                        ty: MachineType::I64,
                    }))],
                },
                main_with(vec![]),
            ],
        };
        let out = inline(program);
        assert_eq!(out.functions.len(), 2, "递归函数不应被移除");
    }

    #[test]
    fn nested_block_contexts_are_rewritten() {
        // if 体内的调用也应展开。
        let program = Program {
            functions: vec![
                helper("inc"),
                main_with(vec![Stmt::If {
                    cond: Expr::Bool(true),
                    then: vec![Stmt::Let {
                        name: String::from("y"),
                        ty: MachineType::I32,
                        init: Expr::Call {
                            name: String::from("inc"),
                            args: vec![Expr::Int {
                                value: 0,
                                ty: MachineType::I32,
                            }],
                            ty: MachineType::I32,
                        },
                    }],
                    els: None,
                }]),
            ],
        };
        let out = inline(program);
        let body = &out.functions[0].body;
        match &body[0] {
            Stmt::If { then, .. } => {
                assert_eq!(then.len(), 3, "if 体内的调用应展开为 3 条语句");
            }
            other => panic!("首语句应为 If，得到 {other:?}"),
        }
    }
}
