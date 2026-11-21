//! TIR → Cranelift IR 的代码生成。
//!
//! 形态决策（MVP）：
//! - 变量一律 SSA（`Variable` + `def_var`/`use_var`），循环变量由前端自动插 phi；
//!   无取地址运算，不需要栈槽。
//! - 每个函数一张帧；参数在 entry 块定址为块参数后 `def_var`。
//! - 整数运算按回绕处理（不加溢出检查）；除法/取模由 Cranelift 的 trap 兜底。
//! - `print` 按实参静态类型分派到运行时打印函数（`mio_print_i32` 等，由 `emit`
//!   生成的 `runtime.c` 提供），不存在运行时探测。
//! - 入口函数固定为 `main`：链接层面 crt 需要 `int main(void)`，故 `main` 的
//!   生成签名强制为 `() -> i32`，缺省返回 0。
//!
//! 指令选择上的两个优化（TL20261121T210405）：
//! - 分支语境（if/while/for/三元）的条件若是比较表达式，`icmp`/`fcmp` 的 b1
//!   结果直传 `brif`，省去 `select` 转 i32；
//! - 整数字面量作操作数时用立即数变体（`iadd_imm_s` / `imul_imm_s` / `icmp_imm_*`）。
//!
//! 借用形态：`builder` 作为独立参数与各函数并列传递（不与 `Ctx` 混装），
//! 因为 `finalize` 消耗 builder 的所有权，混装会导致借用无法释放。

use cranelift_codegen::ir::condcodes::FloatCC;
use cranelift_codegen::ir::condcodes::IntCC;
use cranelift_codegen::ir::types::{F32, F64, I8, I16, I32, I64};
use cranelift_codegen::ir::{AbiParam, Block, BlockArg, InstBuilder, Signature, Type, Value};
use cranelift_frontend::{FunctionBuilder, FunctionBuilderContext, Variable};
use cranelift_module::{Linkage, Module};

use crate::builtin;
use crate::lower::{BinOp, Block as TIRBlock, Expr, Func, Program, Stmt};
use crate::types::MachineType;

/// 变量名 → SSA 变量句柄（已 `declare_var`）。
type Vars = Vec<(String, Variable)>;

/// 循环栈：`(continue 目标, break 目标)`，`break`/`continue` 翻译时查表。
type LoopStack = Vec<(Block, Block)>;

/// 翻译上下文：一帧内共享的可变借用集合（builder 除外，见文件头说明）。
struct Ctx<'a> {
    vars: &'a mut Vars,
    loops: &'a mut LoopStack,
    program: &'a Program,
    module: &'a mut cranelift_object::ObjectModule,
}

/// 代码生成入口：整个程序 → 一个 `ObjectModule`（供 `emit` 产出目标文件）。
///
/// # Errors
/// 遇到无法映射到 Cranelift 指令的节点即返回 `Err`。
pub fn codegen(program: &Program) -> Result<cranelift_object::ObjectModule, String> {
    let mut module = crate::emit::new_module()?;
    for func in &program.functions {
        build_function(func, program, &mut module)?;
    }
    Ok(module)
}

/// 机器类型 → Cranelift 标量类型。
fn clif_type(ty: MachineType) -> Type {
    match ty {
        // Cranelift 的整数类型是 sign-agnostic 的：无符号性由指令
        //（udiv/urem/unsigned IntCC）表达，故 U* 与同名 I* 共用类型。
        MachineType::I8 | MachineType::U8 => I8,
        MachineType::I16 | MachineType::U16 => I16,
        MachineType::I32 | MachineType::U32 | MachineType::Bool => I32,
        MachineType::I64 | MachineType::U64 => I64,
        MachineType::F32 => F32,
        MachineType::F64 => F64,
        // void 只出现在返回位置，不建值。
        MachineType::Void => I8,
    }
}

/// 构造一个函数的 Cranelift 签名。`main` 特判为 `() -> i32`（crt 入口约定）。
fn signature(func: &Func) -> Signature {
    let is_main = func.name == "main";
    let mut sig = Signature::new(cranelift_codegen::isa::CallConv::SystemV);
    for p in &func.params {
        sig.params.push(AbiParam::new(clif_type(p.ty)));
    }
    if is_main {
        sig.returns.push(AbiParam::new(I32));
    } else if func.ret != MachineType::Void {
        sig.returns.push(AbiParam::new(clif_type(func.ret)));
    }
    sig
}

/// 单个函数的代码生成。
fn build_function(
    func: &Func,
    program: &Program,
    module: &mut cranelift_object::ObjectModule,
) -> Result<(), String> {
    let is_main = func.name == "main";
    let sig = signature(func);
    let func_id = module
        .declare_function(&func.name, Linkage::Export, &sig)
        .map_err(|error| format!("声明函数 {} 失败：{error}", func.name))?;

    let mut ctx = cranelift_codegen::Context::new();
    ctx.func.signature = sig;

    let mut builder_ctx = FunctionBuilderContext::new();
    let mut builder = FunctionBuilder::new(&mut ctx.func, &mut builder_ctx);

    let entry = builder.create_block();
    builder.append_block_params_for_function_params(entry);
    builder.switch_to_block(entry);
    builder.seal_block(entry);

    // 参数先定址为 SSA 变量，保证「参数也是变量」的统一形态。
    let mut vars: Vars = Vec::new();
    for (index, param) in func.params.iter().enumerate() {
        let var = builder.declare_var(clif_type(param.ty));
        builder.def_var(var, builder.block_params(entry)[index]);
        vars.push((param.name.clone(), var));
    }

    let mut loops: LoopStack = Vec::new();
    let mut cx = Ctx {
        vars: &mut vars,
        loops: &mut loops,
        program,
        module,
    };

    let mut terminated = false;
    for stmt in &func.body {
        if terminated {
            // return 之后的语句不可达；类型已在 check 阶段校验过，不再翻译。
            break;
        }
        terminated = translate_stmt(stmt, &mut builder, &mut cx, is_main)?;
    }

    // 函数体没有显式 return 时，落到帧尾返回零值（void 函数直接返回）。
    if !terminated {
        emit_tail_return(&mut builder, func.ret, is_main);
    }

    builder.finalize(module.isa().frontend_config());
    module
        .define_function(func_id, &mut ctx)
        .map_err(|error| format!("定义函数 {} 失败：{error}", func.name))?;
    Ok(())
}

/// 函数尾的默认返回。
fn emit_tail_return(builder: &mut FunctionBuilder, ret: MachineType, is_main: bool) {
    if is_main || ret == MachineType::Void {
        // main 的生成签名是 `() -> i32`，返回 0；void 函数无返回值。
        if is_main {
            let zero = builder.ins().iconst(I32, 0);
            builder.ins().return_(&[zero]);
        } else {
            builder.ins().return_(&[]);
        }
    } else {
        let zero = builder.ins().iconst(clif_type(ret), 0);
        builder.ins().return_(&[zero]);
    }
}

/// 翻译一条语句；返回「本块是否已终止」（`return` 之后不可达）。
fn translate_stmt(
    stmt: &Stmt,
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
    is_main: bool,
) -> Result<bool, String> {
    match stmt {
        Stmt::Let { name, ty, init } => {
            if cx.vars.iter().any(|(n, _)| n == name) {
                return Err(format!("变量 {name} 重复声明"));
            }
            let value = translate_expr(init, builder, cx)?;
            let var = builder.declare_var(clif_type(*ty));
            builder.def_var(var, value);
            cx.vars.push((name.clone(), var));
            Ok(false)
        }
        Stmt::Expr(e) => {
            translate_expr(e, builder, cx)?;
            Ok(false)
        }
        Stmt::If { cond, then, els } => {
            let condition = translate_cond(cond, builder, cx)?;
            let then_block = builder.create_block();
            let else_block = builder.create_block();
            let join_block = builder.create_block();
            builder
                .ins()
                .brif(condition, then_block, &[], else_block, &[]);

            builder.switch_to_block(then_block);
            let then_terminated = translate_block(then, builder, cx, is_main)?;
            if !then_terminated {
                builder.ins().jump(join_block, &[]);
            }

            builder.switch_to_block(else_block);
            let else_terminated = match els {
                Some(els) => translate_block(els, builder, cx, is_main)?,
                None => false,
            };
            if !else_terminated {
                builder.ins().jump(join_block, &[]);
            }

            builder.switch_to_block(join_block);
            builder.seal_block(then_block);
            builder.seal_block(else_block);
            builder.seal_block(join_block);
            // 两侧都以 return/trap 结束时，join 块不可达，仍合法（无指令无前驱）。
            Ok(then_terminated && else_terminated)
        }
        Stmt::While { cond, body } => {
            let header = builder.create_block();
            let body_block = builder.create_block();
            let exit = builder.create_block();
            builder.ins().jump(header, &[]);
            builder.switch_to_block(header);

            let condition = translate_cond(cond, builder, cx)?;
            builder.ins().brif(condition, body_block, &[], exit, &[]);

            builder.switch_to_block(body_block);
            cx.loops.push((header, exit));
            let body_terminated = translate_block(body, builder, cx, is_main)?;
            cx.loops.pop();
            if !body_terminated {
                builder.ins().jump(header, &[]);
            }

            builder.switch_to_block(exit);
            builder.seal_block(header);
            builder.seal_block(body_block);
            builder.seal_block(exit);
            Ok(false)
        }
        Stmt::For {
            var,
            ty,
            init,
            cond,
            step,
            body,
        } => {
            let init_value = translate_expr(init, builder, cx)?;
            if cx.vars.iter().any(|(n, _)| n == var) {
                return Err(format!("循环变量 {var} 重复声明"));
            }
            let loop_var = builder.declare_var(clif_type(*ty));
            builder.def_var(loop_var, init_value);
            cx.vars.push((var.clone(), loop_var));

            let header = builder.create_block();
            let body_block = builder.create_block();
            let step_block = builder.create_block();
            let join = builder.create_block();
            builder.ins().jump(header, &[]);
            builder.switch_to_block(header);

            let condition = translate_cond(cond, builder, cx)?;
            builder.ins().brif(condition, body_block, &[], join, &[]);

            builder.switch_to_block(body_block);
            cx.loops.push((step_block, join));
            let body_terminated = translate_block(body, builder, cx, is_main)?;
            cx.loops.pop();
            if !body_terminated {
                builder.ins().jump(step_block, &[]);
            }

            builder.switch_to_block(step_block);
            let next = translate_expr(step, builder, cx)?;
            builder.def_var(loop_var, next);
            builder.ins().jump(header, &[]);

            builder.switch_to_block(join);
            builder.seal_block(header);
            builder.seal_block(body_block);
            builder.seal_block(step_block);
            builder.seal_block(join);
            cx.vars.pop();
            Ok(false)
        }
        Stmt::Return(e) => {
            match e {
                Some(e) => {
                    let value = translate_expr(e, builder, cx)?;
                    if is_main {
                        // main 的生成签名返回 i32 退出码；用户 main 标注 void，忽略返回值。
                        let zero = builder.ins().iconst(I32, 0);
                        builder.ins().return_(&[zero]);
                    } else {
                        builder.ins().return_(&[value]);
                    }
                }
                None => {
                    if is_main {
                        let zero = builder.ins().iconst(I32, 0);
                        builder.ins().return_(&[zero]);
                    } else {
                        builder.ins().return_(&[]);
                    }
                }
            }
            // return 之后补一个终止块，保证后续语句（若有）有处可放。
            let after = builder.create_block();
            builder.switch_to_block(after);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            builder.seal_block(after);
            Ok(true)
        }
        Stmt::Break => {
            let Some((_, exit)) = cx.loops.last() else {
                return Err("break 出现在循环外".to_string());
            };
            builder.ins().jump(*exit, &[]);
            let after = builder.create_block();
            builder.switch_to_block(after);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            builder.seal_block(after);
            Ok(true)
        }
        Stmt::Continue => {
            let Some((header, _)) = cx.loops.last() else {
                return Err("continue 出现在循环外".to_string());
            };
            builder.ins().jump(*header, &[]);
            let after = builder.create_block();
            builder.switch_to_block(after);
            builder
                .ins()
                .trap(cranelift_codegen::ir::TrapCode::unwrap_user(1));
            builder.seal_block(after);
            Ok(true)
        }
    }
}

/// 翻译一条语句序列；返回「末块是否已终止」。
fn translate_block(
    body: &TIRBlock,
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
    is_main: bool,
) -> Result<bool, String> {
    let mut terminated = false;
    for stmt in body {
        if terminated {
            break;
        }
        terminated = translate_stmt(stmt, builder, cx, is_main)?;
    }
    Ok(terminated)
}

/// 翻译单个表达式，返回对应的 Cranelift 值。
fn translate_expr(
    expr: &Expr,
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
) -> Result<Value, String> {
    match expr {
        Expr::Int { value, ty } => Ok(builder.ins().iconst(clif_type(*ty), *value)),
        Expr::Float { value, ty } => Ok(match ty {
            MachineType::F32 => builder.ins().f32const(*value as f32),
            _ => builder.ins().f64const(*value),
        }),
        Expr::Bool(v) => Ok(builder.ins().iconst(I32, i64::from(*v))),
        Expr::Var(name, ty) => {
            let var = cx
                .vars
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| format!("变量 {name} 未声明"))?;
            let value = builder.use_var(var);
            // use_var 的结果类型与声明一致；此处不额外转换。
            let _ = ty;
            Ok(value)
        }
        Expr::UnaryNeg(inner) => {
            let value = translate_expr(inner, builder, cx)?;
            let ty = builder.func.dfg.value_type(value);
            Ok(if ty.is_float() {
                builder.ins().fneg(value)
            } else {
                builder.ins().ineg(value)
            })
        }
        Expr::Not(inner) => {
            let value = translate_expr(inner, builder, cx)?;
            let one = builder.ins().iconst(I32, 1);
            Ok(builder.ins().bxor(value, one))
        }
        Expr::Binary { op, lhs, rhs, ty } => translate_binary(*op, lhs, rhs, *ty, builder, cx),
        Expr::Cond {
            cond,
            then,
            els,
            ty,
        } => {
            let condition = translate_cond(cond, builder, cx)?;
            let then_block = builder.create_block();
            let else_block = builder.create_block();
            let join = builder.create_block();
            builder.append_block_param(join, clif_type(*ty));
            builder
                .ins()
                .brif(condition, then_block, &[], else_block, &[]);

            builder.switch_to_block(then_block);
            let then_value = translate_expr(then, builder, cx)?;
            builder.ins().jump(join, &[BlockArg::Value(then_value)]);

            builder.switch_to_block(else_block);
            let else_value = translate_expr(els, builder, cx)?;
            builder.ins().jump(join, &[BlockArg::Value(else_value)]);

            builder.switch_to_block(join);
            builder.seal_block(then_block);
            builder.seal_block(else_block);
            builder.seal_block(join);
            Ok(builder.block_params(join)[0])
        }
        Expr::Cast { value, from, to } => {
            let inner = translate_expr(value, builder, cx)?;
            Ok(match (from, to) {
                // 同符号整数扩宽
                (_, _) if from.is_signed() && to.is_signed() && to.size() > from.size() => {
                    builder.ins().sextend(clif_type(*to), inner)
                }
                (_, _) if from.is_unsigned() && to.is_unsigned() && to.size() > from.size() => {
                    builder.ins().uextend(clif_type(*to), inner)
                }
                (MachineType::F32, MachineType::F64) => builder.ins().fpromote(F64, inner),
                _ => return Err(format!("不可生成的 cast：{from:?} → {to:?}")),
            })
        }
        Expr::Assign { name, ty, value } => {
            let new_value = translate_expr(value, builder, cx)?;
            let var = cx
                .vars
                .iter()
                .find(|(n, _)| n == name)
                .map(|(_, v)| *v)
                .ok_or_else(|| format!("变量 {name} 未声明"))?;
            builder.def_var(var, new_value);
            let _ = ty;
            Ok(new_value)
        }
        Expr::Call { name, args, ty } => {
            let value = translate_call(name, args, builder, cx)?;
            // void 调用的结果值是占位 i8，不参与后续消费；此处按 TIR 类型对齐。
            let _ = ty;
            Ok(value)
        }
    }
}

/// 条件语境的翻译：比较表达式直传 b1 给分支指令，省去 select 转换。
///
/// 其他表达式（变量、逻辑运算）仍走值层（i32 0/1），`brif` 对两者都接受。
fn translate_cond(
    expr: &Expr,
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
) -> Result<Value, String> {
    if let Expr::Binary { op, lhs, rhs, ty } = expr
        && matches!(
            op,
            BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge
        )
    {
        let left = translate_expr(lhs, builder, cx)?;
        let right = translate_expr(rhs, builder, cx)?;
        return Ok(compare_value(*op, left, right, *ty, builder));
    }
    translate_expr(expr, builder, cx)
}

/// 二元运算翻译（表达式层）：先尝试立即数路径，否则走通用值层。
fn translate_binary(
    op: BinOp,
    lhs_expr: &Expr,
    rhs_expr: &Expr,
    ty: MachineType,
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
) -> Result<Value, String> {
    // 立即数路径：整数字面量在右侧（全部整数运算），或在左侧且运算可交换。
    if !ty.is_float() {
        if let Some(imm) = int_literal(rhs_expr) {
            let left = translate_expr(lhs_expr, builder, cx)?;
            return Ok(int_imm_op(builder, op, left, imm, ty));
        }
        if matches!(op, BinOp::Add | BinOp::Mul | BinOp::Eq | BinOp::Ne)
            && let Some(imm) = int_literal(lhs_expr)
        {
            let right = translate_expr(rhs_expr, builder, cx)?;
            return Ok(int_imm_op(builder, op, right, imm, ty));
        }
    }

    let left = translate_expr(lhs_expr, builder, cx)?;
    let right = translate_expr(rhs_expr, builder, cx)?;
    value_binary(op, left, right, ty, builder)
}

/// 整数字面量（立即数路径用；浮点无立即数变体，不检测）。
fn int_literal(e: &Expr) -> Option<i64> {
    match e {
        Expr::Int { value, ty } if ty.is_integer() => Some(*value),
        _ => None,
    }
}

/// 立即数路径的整数运算。比较运算仍转 i32(0/1)，与值层保持一致。
fn int_imm_op(
    builder: &mut FunctionBuilder,
    op: BinOp,
    left: Value,
    imm: i64,
    ty: MachineType,
) -> Value {
    let signed = ty.is_signed();
    match op {
        BinOp::Add => builder.ins().iadd_imm_s(left, imm),
        // 减常量 = 加负常量（位模式等价，回绕语义一致）。
        BinOp::Sub => builder.ins().iadd_imm_s(left, imm.wrapping_neg()),
        BinOp::Mul => builder.ins().imul_imm_s(left, imm),
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let cmp = if signed {
                builder.ins().icmp_imm_s(int_cc(op, signed), left, imm)
            } else {
                builder.ins().icmp_imm_u(int_cc(op, signed), left, imm)
            };
            let one = builder.ins().iconst(I32, 1);
            let zero = builder.ins().iconst(I32, 0);
            builder.ins().select(cmp, one, zero)
        }
        // 逻辑/除法/取余无立即数变体（或语义不等价），调用方不会走到这里。
        BinOp::And | BinOp::Or | BinOp::Div | BinOp::Rem => {
            unreachable!("立即数路径不覆盖逻辑/除法/取余")
        }
    }
}

/// 值层二元运算（通用路径）。
fn value_binary(
    op: BinOp,
    left: Value,
    right: Value,
    ty: MachineType,
    builder: &mut FunctionBuilder,
) -> Result<Value, String> {
    let is_float = ty.is_float();
    let signed = ty.is_signed();
    let result = match op {
        BinOp::Add => arith(
            builder,
            left,
            right,
            is_float,
            |b, l, r| b.ins().iadd(l, r),
            |b, l, r| b.ins().fadd(l, r),
        ),
        BinOp::Sub => arith(
            builder,
            left,
            right,
            is_float,
            |b, l, r| b.ins().isub(l, r),
            |b, l, r| b.ins().fsub(l, r),
        ),
        BinOp::Mul => arith(
            builder,
            left,
            right,
            is_float,
            |b, l, r| b.ins().imul(l, r),
            |b, l, r| b.ins().fmul(l, r),
        ),
        BinOp::Div => {
            if is_float {
                builder.ins().fdiv(left, right)
            } else if signed {
                builder.ins().sdiv(left, right)
            } else {
                builder.ins().udiv(left, right)
            }
        }
        BinOp::Rem => {
            if is_float {
                return Err("浮点取余本阶段不支持".to_string());
            }
            if signed {
                builder.ins().srem(left, right)
            } else {
                builder.ins().urem(left, right)
            }
        }
        BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
            let cmp = compare_value(op, left, right, ty, builder);
            // 比较结果是 b1，统一转成 i32(0/1)，与 Bool 的静态表示一致。
            let one = builder.ins().iconst(I32, 1);
            let zero = builder.ins().iconst(I32, 0);
            builder.ins().select(cmp, one, zero)
        }
        BinOp::And | BinOp::Or => {
            // 逻辑运算两侧已保证为 boolean(0/1)；按位与/或即结果。
            // 注：MVP 不做短路（表达式无副作用，语义等价）。
            if op == BinOp::And {
                builder.ins().band(left, right)
            } else {
                builder.ins().bor(left, right)
            }
        }
    };
    Ok(result)
}

/// 比较运算：返回 b1（分支语境直传；值层再经 select 转 i32）。
fn compare_value(
    op: BinOp,
    left: Value,
    right: Value,
    ty: MachineType,
    builder: &mut FunctionBuilder,
) -> Value {
    if ty.is_float() {
        let cc = match op {
            BinOp::Eq => FloatCC::Equal,
            BinOp::Ne => FloatCC::NotEqual,
            BinOp::Lt => FloatCC::LessThan,
            BinOp::Le => FloatCC::LessThanOrEqual,
            BinOp::Gt => FloatCC::GreaterThan,
            BinOp::Ge => FloatCC::GreaterThanOrEqual,
            _ => unreachable!("比较运算已穷举"),
        };
        builder.ins().fcmp(cc, left, right)
    } else {
        builder.ins().icmp(int_cc(op, ty.is_signed()), left, right)
    }
}

/// 整数比较的条件码（按符号性分派）。
fn int_cc(op: BinOp, signed: bool) -> IntCC {
    match (op, signed) {
        (BinOp::Eq, _) => IntCC::Equal,
        (BinOp::Ne, _) => IntCC::NotEqual,
        (BinOp::Lt, true) => IntCC::SignedLessThan,
        (BinOp::Lt, false) => IntCC::UnsignedLessThan,
        (BinOp::Le, true) => IntCC::SignedLessThanOrEqual,
        (BinOp::Le, false) => IntCC::UnsignedLessThanOrEqual,
        (BinOp::Gt, true) => IntCC::SignedGreaterThan,
        (BinOp::Gt, false) => IntCC::UnsignedGreaterThan,
        (BinOp::Ge, true) => IntCC::SignedGreaterThanOrEqual,
        (BinOp::Ge, false) => IntCC::UnsignedGreaterThanOrEqual,
        _ => unreachable!("比较运算已穷举"),
    }
}

/// 算术运算的统一入口：整数走 iadd 族，浮点走 fadd 族。
fn arith(
    builder: &mut FunctionBuilder,
    left: Value,
    right: Value,
    is_float: bool,
    int_op: impl Fn(&mut FunctionBuilder, Value, Value) -> Value,
    float_op: impl Fn(&mut FunctionBuilder, Value, Value) -> Value,
) -> Value {
    if is_float {
        float_op(builder, left, right)
    } else {
        int_op(builder, left, right)
    }
}

/// 函数调用：`print` 映射到运行时打印函数，其余查程序内的函数表。
fn translate_call(
    name: &str,
    args: &[Expr],
    builder: &mut FunctionBuilder,
    cx: &mut Ctx,
) -> Result<Value, String> {
    let mut values = Vec::with_capacity(args.len());
    for arg in args {
        values.push(translate_expr(arg, builder, cx)?);
    }

    if name == builtin::PRINT {
        if args.len() != 1 {
            return Err("print 只接受 1 个参数".to_string());
        }
        let ty = expr_static_type(&args[0]).ok_or("print 的实参类型无法确定")?;
        let Some(symbol) = builtin::print_runtime(type_name(ty)) else {
            return Err(format!("print 不支持类型 {ty:?}"));
        };
        return emit_runtime_call(cx, builder, symbol, &[clif_type(ty)], &values);
    }

    // 用户函数：同模块内直接 call。
    let callee = cx
        .program
        .functions
        .iter()
        .find(|f| f.name == name)
        .ok_or_else(|| format!("函数 {name} 未声明"))?;
    let callee_id = cx
        .module
        .declare_function(&callee.name, Linkage::Local, &signature(callee))
        .map_err(|error| format!("声明函数 {name} 失败：{error}"))?;
    let fref = cx.module.declare_func_in_func(callee_id, builder.func);
    let sig = signature(callee);
    let call = builder.ins().call(fref, &values);
    let results = builder.inst_results(call);
    Ok(match sig.returns.is_empty() {
        true => builder.ins().iconst(I8, 0),
        false => results
            .first()
            .copied()
            .unwrap_or_else(|| builder.ins().iconst(I8, 0)),
    })
}

/// 调用一个运行时导入函数（`runtime.c` 提供），返回第一个结果（无结果时给 i8 零值）。
fn emit_runtime_call(
    cx: &mut Ctx,
    builder: &mut FunctionBuilder,
    symbol: &str,
    param_types: &[Type],
    args: &[Value],
) -> Result<Value, String> {
    let mut sig = Signature::new(cranelift_codegen::isa::CallConv::SystemV);
    for ty in param_types {
        sig.params.push(AbiParam::new(*ty));
    }
    let func_id = cx
        .module
        .declare_function(symbol, Linkage::Import, &sig)
        .map_err(|error| format!("声明运行时函数 {symbol} 失败：{error}"))?;
    let fref = cx.module.declare_func_in_func(func_id, builder.func);
    let call = builder.ins().call(fref, args);
    let results = builder.inst_results(call);
    Ok(results
        .first()
        .copied()
        .unwrap_or_else(|| builder.ins().iconst(I8, 0)))
}

/// 表达式的静态类型（仅当类型可直接从节点读出时）。
fn expr_static_type(e: &Expr) -> Option<MachineType> {
    match e {
        Expr::Var(_, ty) => Some(*ty),
        Expr::Int { ty, .. } => Some(*ty),
        Expr::Float { ty, .. } => Some(*ty),
        Expr::Binary { ty, .. } => Some(*ty),
        Expr::Cond { ty, .. } => Some(*ty),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Assign { ty, .. } => Some(*ty),
        Expr::Bool(_) => Some(MachineType::Bool),
        Expr::UnaryNeg(inner) => expr_static_type(inner),
        Expr::Not(inner) => expr_static_type(inner),
        Expr::Call { ty, .. } => Some(*ty),
    }
}

/// 机器类型 → 运行时打印函数符号名的类型键（与 `builtin::print_runtime` 配套）。
fn type_name(ty: MachineType) -> &'static str {
    match ty {
        MachineType::I8 => "i8",
        MachineType::I16 => "i16",
        MachineType::I32 => "i32",
        MachineType::I64 => "i64",
        MachineType::U8 => "u8",
        MachineType::U16 => "u16",
        MachineType::U32 => "u32",
        MachineType::U64 => "u64",
        MachineType::F32 => "f32",
        MachineType::F64 => "f64",
        MachineType::Bool => "bool",
        MachineType::Void => "void",
    }
}
