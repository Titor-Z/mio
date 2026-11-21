//! 类型检查：无隐式转换、`as` 白名单、函数签名匹配。
//!
//! MVP 阶段的最小实现：逐节点验证 TIR，不做跨函数推断。
//!
//! 错误定位：TIR 不带 span（明确记录的临时偏离），check 期错误统一定位到
//! **函数声明行**（`Func.line`，由 lower 阶段记录），消息前缀保留函数名。

use std::collections::HashMap;

use crate::diag::{BuildError, Stage};
use crate::lower::{BinOp, Expr, Func, Program, Stmt};
use crate::types::MachineType;

/// 函数签名表：函数名 → (参数类型, 返回类型)。
type Signatures<'a> = HashMap<&'a str, (&'a [crate::lower::Param], MachineType)>;

/// 构造一个定位到函数声明行的 Check 期错误。
fn err_at(func: &Func, path: &str, message: String) -> BuildError {
    BuildError {
        stage: Stage::Check,
        path: String::from(path),
        line: func.line,
        column: 1,
        message,
    }
}

/// 类型检查入口。返回 `Ok(())` 或第一条错误。
///
/// # Errors
/// 任一违反静态强类型规则即返回 `Err`，错误定位到函数声明行。
pub fn check(program: &Program) -> Result<(), BuildError> {
    let mut sigs = HashMap::new();
    for f in &program.functions {
        if sigs
            .insert(f.name.as_str(), (f.params.as_slice(), f.ret))
            .is_some()
        {
            return Err(err_at(
                f,
                &program.path,
                format!("函数 {} 重复声明", f.name),
            ));
        }
    }
    for f in &program.functions {
        check_func(f, &sigs, &program.path)?;
    }
    Ok(())
}

fn check_func(f: &Func, sigs: &Signatures, path: &str) -> Result<(), BuildError> {
    // 记录本函数内声明的变量类型，供表达式检查用。
    let mut vars: Vec<(String, MachineType)> = Vec::new();
    for p in &f.params {
        vars.push((p.name.clone(), p.ty));
    }
    check_block(&f.body, &mut vars, f.ret, f, sigs, path)
}

fn check_block(
    body: &[Stmt],
    vars: &mut Vec<(String, MachineType)>,
    ret: MachineType,
    func: &Func,
    sigs: &Signatures,
    path: &str,
) -> Result<(), BuildError> {
    for s in body {
        check_stmt(s, vars, ret, func, sigs, path)?;
    }
    Ok(())
}

fn check_stmt(
    s: &Stmt,
    vars: &mut Vec<(String, MachineType)>,
    ret: MachineType,
    func: &Func,
    sigs: &Signatures,
    path: &str,
) -> Result<(), BuildError> {
    match s {
        Stmt::Let { name, ty, init } => {
            let t = check_expr(init, vars, func, sigs, path)?;
            if t != *ty {
                return Err(err_at(
                    func,
                    path,
                    format!("初始化 {name} 的类型 {t:?} 与标注 {ty:?} 不一致（无隐式转换）"),
                ));
            }
            vars.push((name.clone(), *ty));
            Ok(())
        }
        Stmt::Expr(e) => {
            check_expr(e, vars, func, sigs, path)?;
            Ok(())
        }
        Stmt::If { cond, then, els } => {
            let t = check_expr(cond, vars, func, sigs, path)?;
            if t != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("if 条件必须为 boolean，得到 {t:?}"),
                ));
            }
            check_block(then, vars, ret, func, sigs, path)?;
            if let Some(e) = els {
                check_block(e, vars, ret, func, sigs, path)?;
            }
            Ok(())
        }
        Stmt::While { cond, body } => {
            let t = check_expr(cond, vars, func, sigs, path)?;
            if t != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("while 条件必须为 boolean，得到 {t:?}"),
                ));
            }
            check_block(body, vars, ret, func, sigs, path)
        }
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => {
            let t = check_expr(init, vars, func, sigs, path)?;
            if t != *ty {
                return Err(err_at(
                    func,
                    path,
                    format!("for 变量 {var} 初始化类型 {t:?} 与标注 {ty:?} 不一致"),
                ));
            }
            vars.push((var.clone(), *ty));
            let c = check_expr(cond, vars, func, sigs, path)?;
            if c != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("for 条件必须为 boolean，得到 {c:?}"),
                ));
            }
            let s = check_expr(step, vars, func, sigs, path)?;
            if s != *ty {
                return Err(err_at(
                    func,
                    path,
                    format!("for 步进类型 {s:?} 与变量类型 {ty:?} 不一致"),
                ));
            }
            check_block(body, vars, ret, func, sigs, path)
        }
        Stmt::Return(e) => {
            let t = match e {
                Some(e) => check_expr(e, vars, func, sigs, path)?,
                None => MachineType::Void,
            };
            if t != ret {
                return Err(err_at(
                    func,
                    path,
                    format!("函数 {} 返回 {t:?}，与标注 {ret:?} 不一致", func.name),
                ));
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn check_expr(
    e: &Expr,
    vars: &[(String, MachineType)],
    func: &Func,
    sigs: &Signatures,
    path: &str,
) -> Result<MachineType, BuildError> {
    match e {
        Expr::Int { ty, .. } => Ok(*ty),
        Expr::Float { ty, .. } => Ok(*ty),
        Expr::Bool(_) => Ok(MachineType::Bool),
        Expr::Var(name, ty) => {
            if vars.iter().any(|(n, _)| n == name) {
                Ok(*ty)
            } else {
                Err(err_at(
                    func,
                    path,
                    format!("函数 {} 中变量 {name} 未声明", func.name),
                ))
            }
        }
        Expr::UnaryNeg(inner) => {
            let t = check_expr(inner, vars, func, sigs, path)?;
            if !t.is_integer() && !t.is_float() {
                return Err(err_at(
                    func,
                    path,
                    format!("一元负号只允许数值类型，得到 {t:?}"),
                ));
            }
            Ok(t)
        }
        Expr::Not(inner) => {
            let t = check_expr(inner, vars, func, sigs, path)?;
            if t != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("逻辑非只允许 boolean，得到 {t:?}"),
                ));
            }
            Ok(t)
        }
        Expr::Binary { op, lhs, rhs, ty } => {
            let lt = check_expr(lhs, vars, func, sigs, path)?;
            let rt = check_expr(rhs, vars, func, sigs, path)?;
            if lt != rt {
                return Err(err_at(
                    func,
                    path,
                    format!("二元运算两侧类型不一致：{lt:?} 与 {rt:?}（无隐式转换）"),
                ));
            }
            let logic = matches!(op, BinOp::And | BinOp::Or);
            if logic && lt != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("逻辑运算只允许 boolean，得到 {lt:?}"),
                ));
            }
            if !logic && !lt.is_integer() && !lt.is_float() {
                return Err(err_at(
                    func,
                    path,
                    format!("算术/比较运算只允许数值类型，得到 {lt:?}"),
                ));
            }
            // 与降低阶段推断的类型交叉验证
            if *ty != lt {
                return Err(err_at(
                    func,
                    path,
                    format!("运算类型推断不一致：降低得 {ty:?}，检查得 {lt:?}"),
                ));
            }
            Ok(
                if matches!(
                    op,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
                ) {
                    MachineType::Bool
                } else {
                    lt
                },
            )
        }
        Expr::Call { name, args, ty } => {
            if name == "print" {
                if args.len() != 1 {
                    return Err(err_at(func, path, String::from("print 只接受 1 个参数")));
                }
                let t = check_expr(&args[0], vars, func, sigs, path)?;
                if t == MachineType::Void || t == MachineType::I8 || t == MachineType::U8 {
                    return Err(err_at(func, path, format!("print 不支持类型 {t:?}")));
                }
                if *ty != MachineType::Void {
                    return Err(err_at(
                        func,
                        path,
                        format!("print 的返回类型推断不一致：{ty:?} 与 Void"),
                    ));
                }
                return Ok(MachineType::Void);
            }
            // 用户函数：签名必须已声明，参数个数与类型逐一核对。
            let Some((params, ret)) = sigs.get(name.as_str()).copied() else {
                return Err(err_at(func, path, format!("函数 {name} 未声明")));
            };
            if args.len() != params.len() {
                return Err(err_at(
                    func,
                    path,
                    format!(
                        "函数 {name} 期望 {} 个参数，得到 {} 个",
                        params.len(),
                        args.len()
                    ),
                ));
            }
            for (index, arg) in args.iter().enumerate() {
                let t = check_expr(arg, vars, func, sigs, path)?;
                if t != params[index].ty {
                    return Err(err_at(
                        func,
                        path,
                        format!(
                            "函数 {name} 的参数 {} 类型 {t:?} 与标注 {:?} 不一致",
                            params[index].name, params[index].ty
                        ),
                    ));
                }
            }
            if *ty != ret {
                return Err(err_at(
                    func,
                    path,
                    format!("函数调用 {name} 的返回类型推断不一致：降低得 {ty:?}，签名得 {ret:?}"),
                ));
            }
            Ok(ret)
        }
        Expr::Cond {
            cond,
            then,
            els,
            ty,
        } => {
            let ct = check_expr(cond, vars, func, sigs, path)?;
            if ct != MachineType::Bool {
                return Err(err_at(
                    func,
                    path,
                    format!("条件表达式必须为 boolean，得到 {ct:?}"),
                ));
            }
            let tt = check_expr(then, vars, func, sigs, path)?;
            let et = check_expr(els, vars, func, sigs, path)?;
            if tt != et {
                return Err(err_at(
                    func,
                    path,
                    format!("条件表达式两侧类型不一致：{tt:?} 与 {et:?}"),
                ));
            }
            if *ty != tt {
                return Err(err_at(
                    func,
                    path,
                    format!("条件表达式类型推断不一致：{ty:?} 与 {tt:?}"),
                ));
            }
            Ok(tt)
        }
        // `as` 白名单：同符号整数扩宽、f32→f64。其余（缩窄、跨符号、数值↔浮点）一律拒绝。
        Expr::Cast { value, from, to } => {
            let t = check_expr(value, vars, func, sigs, path)?;
            if t != *from {
                return Err(err_at(
                    func,
                    path,
                    format!("`as` 源类型推断不一致：降低得 {from:?}，检查得 {t:?}"),
                ));
            }
            if !is_widening_cast(*from, *to) {
                return Err(err_at(
                    func,
                    path,
                    format!(
                        "`as` 只允许可证明安全的扩宽（同符号整数扩宽、f32→f64），{from:?} → {to:?} 不在白名单"
                    ),
                ));
            }
            Ok(*to)
        }
        Expr::Assign { name, ty, value } => {
            let t = check_expr(value, vars, func, sigs, path)?;
            if t != *ty {
                return Err(err_at(
                    func,
                    path,
                    format!("赋值 {name} 的类型 {t:?} 与标注 {ty:?} 不一致（无隐式转换）"),
                ));
            }
            if !vars.iter().any(|(n, _)| n == name) {
                return Err(err_at(
                    func,
                    path,
                    format!("函数 {} 中变量 {name} 未声明", func.name),
                ));
            }
            Ok(*ty)
        }
    }
}

/// `as` 白名单判定：同符号整数扩宽，或 f32 → f64。
fn is_widening_cast(from: MachineType, to: MachineType) -> bool {
    match (from, to) {
        (MachineType::F32, MachineType::F64) => true,
        (f, t) if f.is_signed() && t.is_signed() => t.size() > f.size(),
        (f, t) if f.is_unsigned() && t.is_unsigned() => t.size() > f.size(),
        _ => false,
    }
}
