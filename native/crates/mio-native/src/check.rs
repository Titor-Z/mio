//! 类型检查：无隐式转换、`as` 白名单、函数签名匹配。
//!
//! MVP 阶段的最小实现：逐节点验证 TIR，不做跨函数推断。

use std::collections::HashMap;

use crate::lower::{BinOp, Expr, Func, Program, Stmt};
use crate::types::MachineType;

/// 函数签名表：函数名 → (参数类型, 返回类型)。
type Signatures<'a> = HashMap<&'a str, (&'a [crate::lower::Param], MachineType)>;

/// 类型检查入口。返回 `Ok(())` 或第一条错误。
///
/// # Errors
/// 任一违反静态强类型规则即返回 `Err`，错误信息带函数名与原因。
pub fn check(program: &Program) -> Result<(), String> {
    let mut sigs = HashMap::new();
    for f in &program.functions {
        if sigs
            .insert(f.name.as_str(), (f.params.as_slice(), f.ret))
            .is_some()
        {
            return Err(format!("函数 {} 重复声明", f.name));
        }
    }
    for f in &program.functions {
        check_func(f, &sigs)?;
    }
    Ok(())
}

fn check_func(f: &Func, sigs: &Signatures) -> Result<(), String> {
    // 记录本函数内声明的变量类型，供表达式检查用。
    let mut vars: Vec<(String, MachineType)> = Vec::new();
    for p in &f.params {
        vars.push((p.name.clone(), p.ty));
    }
    check_block(&f.body, &mut vars, f.ret, &f.name, sigs)
}

fn check_block(
    body: &[Stmt],
    vars: &mut Vec<(String, MachineType)>,
    ret: MachineType,
    fn_name: &str,
    sigs: &Signatures,
) -> Result<(), String> {
    for s in body {
        check_stmt(s, vars, ret, fn_name, sigs)?;
    }
    Ok(())
}

fn check_stmt(
    s: &Stmt,
    vars: &mut Vec<(String, MachineType)>,
    ret: MachineType,
    fn_name: &str,
    sigs: &Signatures,
) -> Result<(), String> {
    match s {
        Stmt::Let { name, ty, init } => {
            let t = check_expr(init, vars, fn_name, sigs)?;
            if t != *ty {
                return Err(format!(
                    "初始化 {name} 的类型 {t:?} 与标注 {ty:?} 不一致（无隐式转换）"
                ));
            }
            vars.push((name.clone(), *ty));
            Ok(())
        }
        Stmt::Expr(e) => {
            check_expr(e, vars, fn_name, sigs)?;
            Ok(())
        }
        Stmt::If { cond, then, els } => {
            let t = check_expr(cond, vars, fn_name, sigs)?;
            if t != MachineType::Bool {
                return Err(format!("if 条件必须为 boolean，得到 {t:?}"));
            }
            check_block(then, vars, ret, fn_name, sigs)?;
            if let Some(e) = els {
                check_block(e, vars, ret, fn_name, sigs)?;
            }
            Ok(())
        }
        Stmt::While { cond, body } => {
            let t = check_expr(cond, vars, fn_name, sigs)?;
            if t != MachineType::Bool {
                return Err(format!("while 条件必须为 boolean，得到 {t:?}"));
            }
            check_block(body, vars, ret, fn_name, sigs)
        }
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => {
            let t = check_expr(init, vars, fn_name, sigs)?;
            if t != *ty {
                return Err(format!(
                    "for 变量 {var} 初始化类型 {t:?} 与标注 {ty:?} 不一致"
                ));
            }
            vars.push((var.clone(), *ty));
            let c = check_expr(cond, vars, fn_name, sigs)?;
            if c != MachineType::Bool {
                return Err(format!("for 条件必须为 boolean，得到 {c:?}"));
            }
            let s = check_expr(step, vars, fn_name, sigs)?;
            if s != *ty {
                return Err(format!("for 步进类型 {s:?} 与变量类型 {ty:?} 不一致"));
            }
            check_block(body, vars, ret, fn_name, sigs)
        }
        Stmt::Return(e) => {
            let t = match e {
                Some(e) => check_expr(e, vars, fn_name, sigs)?,
                None => MachineType::Void,
            };
            if t != ret {
                return Err(format!("函数 {fn_name} 返回 {t:?}，与标注 {ret:?} 不一致"));
            }
            Ok(())
        }
        Stmt::Break | Stmt::Continue => Ok(()),
    }
}

fn check_expr(
    e: &Expr,
    vars: &[(String, MachineType)],
    fn_name: &str,
    sigs: &Signatures,
) -> Result<MachineType, String> {
    match e {
        Expr::Int { ty, .. } => Ok(*ty),
        Expr::Float { ty, .. } => Ok(*ty),
        Expr::Bool(_) => Ok(MachineType::Bool),
        Expr::Var(name, ty) => {
            if vars.iter().any(|(n, _)| n == name) {
                Ok(*ty)
            } else {
                Err(format!("函数 {fn_name} 中变量 {name} 未声明"))
            }
        }
        Expr::UnaryNeg(inner) => {
            let t = check_expr(inner, vars, fn_name, sigs)?;
            if !t.is_integer() && !t.is_float() {
                return Err(format!("一元负号只允许数值类型，得到 {t:?}"));
            }
            Ok(t)
        }
        Expr::Not(inner) => {
            let t = check_expr(inner, vars, fn_name, sigs)?;
            if t != MachineType::Bool {
                return Err(format!("逻辑非只允许 boolean，得到 {t:?}"));
            }
            Ok(t)
        }
        Expr::Binary { op, lhs, rhs, ty } => {
            let lt = check_expr(lhs, vars, fn_name, sigs)?;
            let rt = check_expr(rhs, vars, fn_name, sigs)?;
            if lt != rt {
                return Err(format!(
                    "二元运算两侧类型不一致：{lt:?} 与 {rt:?}（无隐式转换）"
                ));
            }
            let logic = matches!(op, BinOp::And | BinOp::Or);
            if logic && lt != MachineType::Bool {
                return Err(format!("逻辑运算只允许 boolean，得到 {lt:?}"));
            }
            if !logic && !lt.is_integer() && !lt.is_float() {
                return Err(format!("算术/比较运算只允许数值类型，得到 {lt:?}"));
            }
            // 与降低阶段推断的类型交叉验证
            if *ty != lt {
                return Err(format!("运算类型推断不一致：降低得 {ty:?}，检查得 {lt:?}"));
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
                    return Err("print 只接受 1 个参数".to_string());
                }
                let t = check_expr(&args[0], vars, fn_name, sigs)?;
                if t == MachineType::Void || t == MachineType::I8 || t == MachineType::U8 {
                    return Err(format!("print 不支持类型 {t:?}"));
                }
                if *ty != MachineType::Void {
                    return Err(format!("print 的返回类型推断不一致：{ty:?} 与 Void"));
                }
                return Ok(MachineType::Void);
            }
            // 用户函数：签名必须已声明，参数个数与类型逐一核对。
            let Some((params, ret)) = sigs.get(name.as_str()).copied() else {
                return Err(format!("函数 {name} 未声明"));
            };
            if args.len() != params.len() {
                return Err(format!(
                    "函数 {name} 期望 {} 个参数，得到 {} 个",
                    params.len(),
                    args.len()
                ));
            }
            for (index, arg) in args.iter().enumerate() {
                let t = check_expr(arg, vars, fn_name, sigs)?;
                if t != params[index].ty {
                    return Err(format!(
                        "函数 {name} 的参数 {} 类型 {t:?} 与标注 {:?} 不一致",
                        params[index].name, params[index].ty
                    ));
                }
            }
            if *ty != ret {
                return Err(format!(
                    "函数调用 {name} 的返回类型推断不一致：降低得 {ty:?}，签名得 {ret:?}"
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
            let ct = check_expr(cond, vars, fn_name, sigs)?;
            if ct != MachineType::Bool {
                return Err(format!("条件表达式必须为 boolean，得到 {ct:?}"));
            }
            let tt = check_expr(then, vars, fn_name, sigs)?;
            let et = check_expr(els, vars, fn_name, sigs)?;
            if tt != et {
                return Err(format!("条件表达式两侧类型不一致：{tt:?} 与 {et:?}"));
            }
            if *ty != tt {
                return Err(format!("条件表达式类型推断不一致：{ty:?} 与 {tt:?}"));
            }
            Ok(tt)
        }
        // `as` 白名单：同符号整数扩宽、f32→f64。其余（缩窄、跨符号、数值↔浮点）一律拒绝。
        Expr::Cast { value, from, to } => {
            let t = check_expr(value, vars, fn_name, sigs)?;
            if t != *from {
                return Err(format!(
                    "`as` 源类型推断不一致：降低得 {from:?}，检查得 {t:?}"
                ));
            }
            if !is_widening_cast(*from, *to) {
                return Err(format!(
                    "`as` 只允许可证明安全的扩宽（同符号整数扩宽、f32→f64），{from:?} → {to:?} 不在白名单"
                ));
            }
            Ok(*to)
        }
        Expr::Assign { name, ty, value } => {
            let t = check_expr(value, vars, fn_name, sigs)?;
            if t != *ty {
                return Err(format!(
                    "赋值 {name} 的类型 {t:?} 与标注 {ty:?} 不一致（无隐式转换）"
                ));
            }
            if !vars.iter().any(|(n, _)| n == name) {
                return Err(format!("函数 {fn_name} 中变量 {name} 未声明"));
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
