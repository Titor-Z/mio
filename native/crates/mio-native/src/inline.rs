//! TIR 层小函数内联（结构层优化，TL20261121T210405）。
//!
//! 为什么需要：Cranelift speed 档不做函数内联（它的优化 pass 是机器指令级的），
//! 而小函数调用的固定开销（prologue/epilogue + 参数搬运 + call/ret）在热点循环里
//! 可占 3 倍时间（实测：1e8 次 `add` 调用 0.20s → 内联后 0.07s）。
//!
//! MVP 的保守边界（每一条都有明确理由）：
//! - 只内联**叶子小函数**：体 ≤ 8 条语句、体内无任何 `Call`（同时排除递归与
//!   他调，保证副本自包含）。
//! - **允许控制流**（if/while/for/提前 return），但**拒绝「循环体内的 return」**：
//!   return 转换为「结果变量赋值 + break」时，break 会错误绑定到内层循环，
//!   语义无法保持。该形态的函数保留调用不动。
//! - 只处理三种调用语境：顶层语句、`let` 初始化式、赋值 rhs。嵌套表达式语境
//!   （如 `print(add(1, 2))`）保留调用——表达式无法携带语句序列，语句化提升
//!   留待后续。
//! - `main` 不内联（它是入口）；内联后仍被引用的函数定义按「剩余调用点」保留。
//!
//! 展开形态（以非 void 函数为例）：
//! ```text
//! let f#{id}.p0 = arg0;        // 参数承接（实参表达式复制）
//! ...                          // 函数体副本（变量重命名，return 已转换）
//! let f.ret#{id} = <零值>;     // 结果变量（落空路径语义与原 fallback 一致）
//! while (true) {               // 只执行一次的包裹块
//!     <副本语句，return e → { f.ret#{id} = e; break; }>
//!     break;                   // 兜底：所有路径已 return 时不可达
//! }
//! ```
//! void 函数不生成结果变量，`return;` 只转换为 `break`。

use std::collections::HashMap;

use crate::lower::{Expr, Func, Program, Stmt};
use crate::types::MachineType;

/// 内联入口：返回内联后的程序（类型正确性不变——实参类型与形参一致已由
/// check 阶段保证，副本只是复制）。
#[must_use]
pub fn inline(mut program: Program) -> Program {
    // 路径随程序贯穿各阶段（诊断定位用），先取出再重建。
    let path = std::mem::take(&mut program.path);
    // 第一遍：收集可内联函数的定义（克隆出来，避免与 functions 的消费遍历冲突）。
    let defs: HashMap<String, Func> = program
        .functions
        .iter()
        .filter(|f| is_inlinable(f))
        .map(|f| (f.name.clone(), f.clone()))
        .collect();
    if defs.is_empty() {
        return program;
    }

    // 第二遍：逐函数重写调用点。
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
                line: func.line,
                body,
            });
        }
        // 可内联函数的定义在内联后移除（其调用点已被展开）。
    }

    // 收尾：扫描剩余程序里仍被引用的可内联函数（嵌套语境未展开的调用点），
    // 把它们的定义保留回去——「移除定义 + 残留调用」会产生难懂的「函数未声明」。
    let remaining_calls = collect_call_names(&functions);
    for (name, func) in &defs {
        if name != "main" && remaining_calls.contains(name) {
            functions.push(func.clone());
        }
    }

    Program { functions, path }
}

/// 可内联判定：叶子小函数，允许控制流，拒绝循环体内的 return。
fn is_inlinable(f: &Func) -> bool {
    if f.name == "main" || f.body.len() > 8 {
        return false;
    }
    !block_has_call(&f.body) && !has_return_in_loop(&f.body, false)
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

/// 块内「循环语境中的 return」是否存在（该形态无法内联：return 转换出的 break
/// 会错误绑定到内层循环）。
fn has_return_in_loop(body: &[Stmt], in_loop: bool) -> bool {
    body.iter().any(|stmt| match stmt {
        Stmt::Return(_) => in_loop,
        Stmt::If { then, els, .. } => {
            has_return_in_loop(then, in_loop)
                || els
                    .as_deref()
                    .is_some_and(|b| has_return_in_loop(b, in_loop))
        }
        Stmt::While { body, .. } | Stmt::For { body, .. } => has_return_in_loop(body, true),
        Stmt::Expr(_) | Stmt::Let { .. } | Stmt::Break | Stmt::Continue => false,
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
        // let x = f(args) → 参数承接 + 包裹副本 + 结果变量，原 Let 的 init 换引用。
        Stmt::Let { name, ty, init } => {
            if let Expr::Call {
                name: callee, args, ..
            } = &init
                && let Some(expanded) = try_inline_call(callee, args, defs, counter)
            {
                out.extend(expanded);
                out.push(Stmt::Let {
                    name,
                    ty,
                    init: Expr::Var(expanded_result_name(callee, *counter), ty),
                });
                return;
            }
            out.push(Stmt::Let { name, ty, init });
        }
        // 语句级调用两种形态：裸调用 `f(args);` 与赋值 rhs `x = f(args);`。
        // 用 match 一次分派（同一表达式上不做连续 if-let 判断）。
        Stmt::Expr(e) => match &e {
            // 裸调用：prelude 已含结果变量声明，本语境无人读取它，作为 SSA 死值由后端消除。
            Expr::Call {
                name: callee, args, ..
            } => match try_inline_call(callee, args, defs, counter) {
                Some(expanded) => out.extend(expanded),
                None => out.push(Stmt::Expr(e)),
            },
            // 赋值 rhs 是调用：展开后把 rhs 换成对结果变量的引用。
            Expr::Assign { name, ty, value }
                if let Expr::Call {
                    name: callee, args, ..
                } = value.as_ref() =>
            {
                match try_inline_call(callee, args, defs, counter) {
                    Some(expanded) => {
                        out.extend(expanded);
                        out.push(Stmt::Expr(Expr::Assign {
                            name: name.clone(),
                            ty: *ty,
                            value: Box::new(Expr::Var(expanded_result_name(callee, *counter), *ty)),
                        }));
                    }
                    None => out.push(Stmt::Expr(e)),
                }
            }
            _ => out.push(Stmt::Expr(e)),
        },
        // 嵌套块语境：递归重写（if/while/for 的体）。
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
        other => out.push(other),
    }
}

/// 内联展开的一个调用点：参数承接 + 结果变量 + 包裹循环（副本）。
fn try_inline_call(
    callee: &str,
    args: &[Expr],
    defs: &HashMap<String, Func>,
    counter: &mut u64,
) -> Option<Vec<Stmt>> {
    let func = defs.get(callee)?;
    if func.params.len() != args.len() {
        return None;
    }

    *counter += 1;
    let call_id = *counter;
    let mangle = |name: &str| format!("{name}#{call_id}");
    let result_name = expanded_result_name(callee, call_id);

    let mut prelude = Vec::new();

    // 参数承接：每个参数变为以实参初始化的 Let（实参表达式复制一份）。
    for (param, arg) in func.params.iter().zip(args.iter()) {
        prelude.push(Stmt::Let {
            name: mangle(&param.name),
            ty: param.ty,
            init: arg.clone(),
        });
    }

    // 结果变量（非 void）：零值占位，落空路径语义与原 codegen fallback 一致。
    if func.ret != MachineType::Void {
        prelude.push(Stmt::Let {
            name: result_name.clone(),
            ty: func.ret,
            init: zero_value(func.ret),
        });
    }

    // 函数体副本：先重命名（防捕获），再把 return 转换为「赋值 + break」。
    let mut body = Vec::new();
    for stmt in &func.body {
        let renamed = rename_stmt(stmt, &mangle);
        for expanded in transform_returns(renamed, &result_name, func.ret) {
            body.push(expanded);
        }
    }
    // 兜底 break：所有路径已 return 时不可达；落空路径靠它正常退出包裹循环。
    body.push(Stmt::Break);

    prelude.push(Stmt::While {
        cond: Expr::Bool(true),
        body,
    });

    Some(prelude)
}

/// 内联结果变量的名字（调用点替换为对它的引用）。
fn expanded_result_name(callee: &str, call_id: u64) -> String {
    format!("{callee}.ret#{call_id}")
}

/// 把语句内的 `return e` 转换为「结果变量赋值 + break」，`return;` 只转 break。
/// 递归处理 if/while/for 的嵌套块（循环体内的 return 已被 is_inlinable 拒绝，
/// 此处递归仅为防御性一致）。
fn transform_returns(stmt: Stmt, result_var: &str, ret: MachineType) -> Vec<Stmt> {
    match stmt {
        // TIR 的赋值是表达式（Expr::Assign），包进 Stmt::Expr 作为语句。
        Stmt::Return(Some(e)) if ret != MachineType::Void => vec![
            Stmt::Expr(Expr::Assign {
                name: String::from(result_var),
                ty: ret,
                value: Box::new(e),
            }),
            Stmt::Break,
        ],
        Stmt::Return(_) => vec![Stmt::Break],
        Stmt::If { cond, then, els } => vec![Stmt::If {
            cond,
            then: then
                .into_iter()
                .flat_map(|s| transform_returns(s, result_var, ret))
                .collect(),
            els: els.map(|b| {
                b.into_iter()
                    .flat_map(|s| transform_returns(s, result_var, ret))
                    .collect()
            }),
        }],
        Stmt::While { cond, body } => vec![Stmt::While {
            cond,
            body: body
                .into_iter()
                .flat_map(|s| transform_returns(s, result_var, ret))
                .collect(),
        }],
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => vec![Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body: body
                .into_iter()
                .flat_map(|s| transform_returns(s, result_var, ret))
                .collect(),
        }],
        other => vec![other],
    }
}

/// 各类型的零值（结果变量占位用）。
fn zero_value(ty: MachineType) -> Expr {
    match ty {
        MachineType::F32 | MachineType::F64 => Expr::Float { value: 0.0, ty },
        MachineType::Bool => Expr::Bool(false),
        MachineType::Void => unreachable!("void 函数不生成结果变量"),
        _ => Expr::Int { value: 0, ty },
    }
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

/// 收集程序里剩余的全部函数调用名字（含嵌套块与实参）。
/// 收集程序里剩余的全部函数调用名字（含嵌套块与实参）。
/// 用途：内联移除定义后，若仍有未被展开的调用点（嵌套表达式语境），据此把定义放回。
fn collect_call_names(functions: &[Func]) -> std::collections::HashSet<String> {
    let mut names = std::collections::HashSet::new();
    for func in functions {
        for stmt in &func.body {
            collect_calls_in_block(stmt, &mut names);
        }
    }
    names
}

/// 收集单条语句（递归进 if/while/for 的体）里出现的调用名字。
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

/// 收集表达式内（递归进实参）出现的调用名字。
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lower::{BinOp, Param};

    /// 构造 `fn inc(a: i32) -> i32 { return a + 1; }`。
    fn helper(name: &str) -> Func {
        Func {
            name: String::from(name),
            params: vec![Param {
                name: String::from("a"),
                ty: MachineType::I32,
            }],
            ret: MachineType::I32,
            line: 1,
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

    /// 构造 `fn abs(x: i64) -> i64 { if (x < 0) { return -x; } return x; }`。
    fn abs_helper() -> Func {
        Func {
            name: String::from("abs"),
            params: vec![Param {
                name: String::from("x"),
                ty: MachineType::I64,
            }],
            ret: MachineType::I64,
            line: 1,
            body: vec![
                Stmt::If {
                    cond: Expr::Binary {
                        op: BinOp::Lt,
                        lhs: Box::new(Expr::Var(String::from("x"), MachineType::I64)),
                        rhs: Box::new(Expr::Int {
                            value: 0,
                            ty: MachineType::I64,
                        }),
                        ty: MachineType::I64,
                    },
                    then: vec![Stmt::Return(Some(Expr::UnaryNeg(Box::new(Expr::Var(
                        String::from("x"),
                        MachineType::I64,
                    )))))],
                    els: None,
                },
                Stmt::Return(Some(Expr::Var(String::from("x"), MachineType::I64))),
            ],
        }
    }

    fn main_with(body: Vec<Stmt>) -> Func {
        Func {
            name: String::from("main"),
            params: vec![],
            ret: MachineType::Void,
            line: 1,
            body,
        }
    }

    #[test]
    fn inline_expands_let_call_and_renames_locals() {
        let program = Program {
            path: String::new(),
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
        // 展开后：参数承接 Let、结果 Let、包裹 While、原 Let（init 换 Var 引用）。
        let body = &out.functions[0].body;
        assert_eq!(body.len(), 4);
        assert!(matches!(&body[0], Stmt::Let { name, .. } if name == "a#1"));
        assert!(matches!(&body[1], Stmt::Let { name, .. } if name == "inc.ret#1"));
        assert!(matches!(&body[2], Stmt::While { .. }));
        match &body[3] {
            Stmt::Let { init, .. } => {
                assert!(matches!(init, Expr::Var(name, _) if name == "inc.ret#1"));
            }
            other => panic!("末语句应为 Let，得到 {other:?}"),
        }
    }

    #[test]
    fn control_flow_function_is_inlined_with_wrapper_loop() {
        let program = Program {
            path: String::new(),
            functions: vec![
                abs_helper(),
                main_with(vec![Stmt::Let {
                    name: String::from("a"),
                    ty: MachineType::I64,
                    init: Expr::Call {
                        name: String::from("abs"),
                        args: vec![Expr::Int {
                            value: -3,
                            ty: MachineType::I64,
                        }],
                        ty: MachineType::I64,
                    },
                }]),
            ],
        };

        let out = inline(program);
        assert_eq!(out.functions.len(), 1, "abs 应被内联移除");
        let body = &out.functions[0].body;
        // 参数承接 + 结果变量 + 包裹 While + 原 Let。
        assert_eq!(body.len(), 4);
        // 包裹循环体内的 return 已转换为「赋值表达式语句 + break」。
        match &body[2] {
            Stmt::While { body, .. } => {
                assert!(body.iter().any(|s| matches!(
                    s,
                    Stmt::Expr(Expr::Assign { name, .. }) if name == "abs.ret#1"
                )));
                assert!(body.iter().any(|s| matches!(s, Stmt::Break)));
            }
            other => panic!("第 3 条应为包裹 While，得到 {other:?}"),
        }
    }

    #[test]
    fn recursive_function_is_not_inlined() {
        // fib 体内有 Call，不可内联。
        let program = Program {
            path: String::new(),
            functions: vec![
                Func {
                    name: String::from("fib"),
                    params: vec![Param {
                        name: String::from("n"),
                        ty: MachineType::I64,
                    }],
                    ret: MachineType::I64,
                    line: 1,
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
    fn return_inside_loop_is_not_inlined() {
        // 循环体内的 return 无法保持语义（break 会绑定到内层循环），拒绝内联。
        let program = Program {
            path: String::new(),
            functions: vec![
                Func {
                    name: String::from("search"),
                    params: vec![Param {
                        name: String::from("n"),
                        ty: MachineType::I64,
                    }],
                    ret: MachineType::I64,
                    line: 1,
                    body: vec![
                        Stmt::While {
                            cond: Expr::Bool(true),
                            body: vec![Stmt::Return(Some(Expr::Var(
                                String::from("n"),
                                MachineType::I64,
                            )))],
                        },
                        Stmt::Return(Some(Expr::Int {
                            value: 0,
                            ty: MachineType::I64,
                        })),
                    ],
                },
                main_with(vec![]),
            ],
        };
        let out = inline(program);
        assert_eq!(out.functions.len(), 2, "循环内 return 的函数不应被内联");
        assert!(!is_inlinable(&out.functions[0]));
    }

    #[test]
    fn nested_block_contexts_are_rewritten() {
        // if 体内的调用也应展开。
        let program = Program {
            path: String::new(),
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
                assert_eq!(then.len(), 4, "if 体内的调用应展开为 4 条语句");
            }
            other => panic!("首语句应为 If，得到 {other:?}"),
        }
    }
}
