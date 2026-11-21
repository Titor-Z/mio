//! AST → TIR 降低。
//!
//! TIR（Typed IR）是带类型标注的 AST + 局部变量槽位表，不展平、不做 SSA。
//! 这是明确记录的临时偏离：不建独立 IR，触发条件为出现第二个后端（wasm）。
//!
//! 类型传播规则（MVP）：
//! - 变量声明、参数、函数返回值必须有显式类型标注；
//! - 字面量类型取自期望类型（声明 / 形参 / 返回 / 运算符另一侧）；
//! - 二元运算两侧类型必须一致；条件表达式两侧类型必须一致。
//!
//! 多文件：入口模块的函数名无前缀，被引模块为 `模块前缀::函数名`；
//! Call 表达式按「导入绑定 → 本模块原名 → 模块前缀」顺序解析全局名。
//! 错误统一为 [`BuildError`]：本阶段（Lower）的每个错误都带 OXC `Span`，
//! 可精确到行列（由 `Ctx::err` 换算）。

use std::collections::HashMap;

use crate::diag::{BuildError, Stage};
use crate::repr::{self, Representation};
use crate::resolve::ModuleSource;
use crate::types::MachineType;
use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use oxc_span::{GetSpan, SourceType, Span};

/// 降低后的程序：函数列表。入口固定为 `main`。
pub struct Program {
    pub functions: Vec<Func>,
    /// 入口源码路径（诊断渲染用，随程序贯穿各阶段）。
    pub path: String,
}

#[derive(Debug, Clone)]
pub struct Func {
    pub name: String,
    pub params: Vec<Param>,
    pub ret: MachineType,
    pub body: Block,
    /// 函数声明所在行（1 起始）：check 阶段的错误定位到该行。
    pub line: u32,
    /// 函数所属模块路径（check 期错误的位置渲染用）。
    pub module_path: String,
}

#[derive(Debug, Clone)]
pub struct Param {
    pub name: String,
    pub ty: MachineType,
}

pub type Block = Vec<Stmt>;

#[derive(Debug, Clone)]
pub enum Stmt {
    Let {
        name: String,
        ty: MachineType,
        init: Expr,
    },
    Expr(Expr),
    If {
        cond: Expr,
        then: Block,
        els: Option<Block>,
    },
    While {
        cond: Expr,
        body: Block,
    },
    /// `for (let i: T = a; c; s)` 原样保留，由 codegen 处理三段式。
    For {
        var: String,
        ty: MachineType,
        init: Expr,
        cond: Expr,
        step: Expr,
        body: Block,
    },
    Return(Option<Expr>),
    Break,
    Continue,
}

#[derive(Debug, Clone)]
pub enum Expr {
    /// 整数字面量及其静态类型（无期望时默认 i32）。
    Int {
        value: i64,
        ty: MachineType,
    },
    /// 浮点字面量及其静态类型（无期望时默认 f64）。
    Float {
        value: f64,
        ty: MachineType,
    },
    Bool(bool),
    Var(String, MachineType),
    UnaryNeg(Box<Expr>),
    Not(Box<Expr>),
    Binary {
        op: BinOp,
        lhs: Box<Expr>,
        rhs: Box<Expr>,
        ty: MachineType,
    },
    Call {
        name: String,
        args: Vec<Expr>,
        ty: MachineType,
    },
    Cond {
        cond: Box<Expr>,
        then: Box<Expr>,
        els: Box<Expr>,
        ty: MachineType,
    },
    /// `as` 扩宽断言（白名单在 check 阶段校验）。
    Cast {
        value: Box<Expr>,
        from: MachineType,
        to: MachineType,
    },

    /// 赋值表达式 `name = value`，返回赋值后的值。
    Assign {
        name: String,
        ty: MachineType,
        value: Box<Expr>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinOp {
    Add,
    Sub,
    Mul,
    Div,
    Rem,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    And,
    Or,
}

/// 降低期上下文：源码与路径用于错误定位；导入绑定与模块前缀用于
/// Call 表达式的全局名解析（多文件降低期）。
struct Ctx<'a> {
    src: &'a str,
    path: &'a str,
    sigs: &'a HashMap<String, (Vec<Param>, MachineType)>,
    /// 本模块的导入绑定：本地名 → 全局函数名。
    bindings: &'a HashMap<String, String>,
    /// 本模块的前缀（入口模块为空串）。
    module_prefix: String,
}

impl Ctx<'_> {
    /// 由 AST 节点的 span 构造带行列的 Lower 期错误。
    fn err(&self, span: Span, message: impl Into<String>) -> BuildError {
        BuildError::at_offset(
            Stage::Lower,
            self.path,
            self.src,
            span.start as usize,
            message.into(),
        )
    }
}

/// 多文件降低入口：模块图（已由 `resolve::expand` 展开）→ TIR。
///
/// 单文件编译也走本函数（入口模块无 import，expand 返回单模块表）。
///
/// 两遍降低：先 parse 全部模块收集签名（全局名），再逐模块降低函数体。
/// 每个模块各自 parse（oxc 的 AST 生命周期绑定局部 allocator，无法跨函数持有）。
///
/// # Errors
/// 同 [`lower`]，另加「导入名与本地函数冲突」。
pub fn lower_multi(modules: &[ModuleSource]) -> Result<Program, BuildError> {
    // 第一遍：签名收集（全局名 = 入口原名 / 被引 `前缀::原名`）。
    let mut sigs: HashMap<String, (Vec<Param>, MachineType)> = HashMap::new();
    for m in modules {
        let allocator = Allocator::default();
        let source_type = SourceType::default().with_typescript(true);
        let ret = Parser::new(&allocator, &m.src, source_type).parse();
        if !ret.diagnostics.is_empty() {
            return Err(BuildError::at_offset(
                Stage::Lower,
                &m.path,
                &m.src,
                ret.diagnostics[0]
                    .labels
                    .first()
                    .map_or(0, |l| l.span().start as usize),
                format!("语法错误：{}", ret.diagnostics[0]),
            ));
        }
        // 第一遍只需要错误定位（签名收集不查表），用空绑定构造轻量 ctx。
        let empty_sigs: HashMap<String, (Vec<Param>, MachineType)> = HashMap::new();
        let empty_bindings: HashMap<String, String> = HashMap::new();
        let ctx = Ctx {
            src: &m.src,
            path: &m.path,
            sigs: &empty_sigs,
            bindings: &empty_bindings,
            module_prefix: String::new(),
        };
        for stmt in &ret.program.body {
            let f = match stmt {
                Statement::FunctionDeclaration(f) => f,
                // import 的形态校验在 resolve 期已完成，这里跳过。
                Statement::ImportDeclaration(_) => continue,
                _ => {
                    return Err(ctx.err(stmt.span(), "顶层只允许函数声明与 import"));
                }
            };
            let name =
                f.id.as_ref()
                    .map(|id| id.name.as_str().to_string())
                    .ok_or_else(|| ctx.err(f.span(), "函数必须有名字"))?;
            let global = global_name(m, &name);
            if sigs.insert(global, signature_of(&ctx, f)?).is_some() {
                return Err(ctx.err(f.span(), format!("函数 {name} 重复声明")));
            }
        }
    }

    // 入口模块的导入名不得与本地函数同名（TS 语义即冲突）。
    let entry = modules.iter().find(|m| m.is_entry).expect("模块图必含入口");
    for name in local_function_names(entry) {
        if entry.bindings.contains_key(&name) {
            return Err(BuildError::new(
                Stage::Lower,
                &entry.path,
                format!("导入名 {name} 与本模块函数同名（不支持遮蔽）"),
            ));
        }
    }

    // 第二遍：逐模块降低函数体。
    let mut funcs = Vec::new();
    for m in modules {
        let allocator = Allocator::default();
        let source_type = SourceType::default().with_typescript(true);
        let ret = Parser::new(&allocator, &m.src, source_type).parse();
        if !ret.diagnostics.is_empty() {
            return Err(BuildError::at_offset(
                Stage::Lower,
                &m.path,
                &m.src,
                ret.diagnostics[0]
                    .labels
                    .first()
                    .map_or(0, |l| l.span().start as usize),
                format!("语法错误：{}", ret.diagnostics[0]),
            ));
        }
        let ctx = Ctx {
            src: &m.src,
            path: &m.path,
            sigs: &sigs,
            bindings: &m.bindings,
            module_prefix: m.prefix.clone(),
        };
        for stmt in &ret.program.body {
            if let Statement::FunctionDeclaration(f) = stmt {
                let name =
                    f.id.as_ref()
                        .map(|id| id.name.as_str().to_string())
                        .ok_or_else(|| ctx.err(f.span(), "函数必须有名字"))?;
                let global = global_name(m, &name);
                funcs.push(lower_function(&ctx, f, &global)?);
            }
        }
    }

    if funcs.iter().all(|f| f.name != "main") {
        return Err(BuildError::new(
            Stage::Lower,
            &entry.path,
            String::from("缺少入口函数 main"),
        ));
    }
    Ok(Program {
        functions: funcs,
        path: String::from(&entry.path),
    })
}

/// 模块内函数的原名表（入口冲突校验用）。
fn local_function_names(m: &ModuleSource) -> Vec<String> {
    let allocator = Allocator::default();
    let source_type = SourceType::default().with_typescript(true);
    let ret = Parser::new(&allocator, &m.src, source_type).parse();
    ret.program
        .body
        .iter()
        .filter_map(|stmt| match stmt {
            Statement::FunctionDeclaration(f) => {
                f.id.as_ref().map(|id| id.name.as_str().to_string())
            }
            _ => None,
        })
        .collect()
}

/// 函数的全局名：入口模块用原名，被引模块加 `前缀::`。
fn global_name(m: &ModuleSource, local: &str) -> String {
    if m.is_entry {
        String::from(local)
    } else {
        format!("{}::{}", m.prefix, local)
    }
}

/// 从 AST 函数声明提取签名（参数名+类型、返回类型）。
fn signature_of(ctx: &Ctx, f: &Function) -> Result<(Vec<Param>, MachineType), BuildError> {
    let name =
        f.id.as_ref()
            .map(|id| id.name.as_str().to_string())
            .ok_or_else(|| ctx.err(f.span(), "函数必须有名字"))?;
    let ret = match &f.return_type {
        Some(ann) => type_from_ts(ctx, &ann.type_annotation)?,
        None => return Err(ctx.err(f.span(), format!("函数 {name} 必须有返回类型标注"))),
    };
    let mut params = Vec::new();
    for p in &f.params.items {
        let BindingPattern::BindingIdentifier(id) = &p.pattern else {
            return Err(ctx.err(p.pattern.span(), "参数必须为简单标识符"));
        };
        let ty = p
            .type_annotation
            .as_ref()
            .map(|ann| type_from_ts(ctx, &ann.type_annotation))
            .transpose()?
            .ok_or_else(|| ctx.err(p.span(), format!("参数 {} 必须有类型标注", id.name)))?;
        params.push(Param {
            name: id.name.as_str().to_string(),
            ty,
        });
    }
    Ok((params, ret))
}

fn lower_function(ctx: &Ctx, f: &Function, global: &str) -> Result<Func, BuildError> {
    let name =
        f.id.as_ref()
            .map(|id| id.name.as_str().to_string())
            .ok_or_else(|| ctx.err(f.span(), "函数必须有名字"))?;
    let ret = match &f.return_type {
        Some(ann) => type_from_ts(ctx, &ann.type_annotation)?,
        None => return Err(ctx.err(f.span(), format!("函数 {name} 必须有返回类型标注"))),
    };
    let mut params = Vec::new();
    for p in &f.params.items {
        let BindingPattern::BindingIdentifier(id) = &p.pattern else {
            return Err(ctx.err(p.pattern.span(), "参数必须为简单标识符"));
        };
        let ty = p
            .type_annotation
            .as_ref()
            .map(|ann| type_from_ts(ctx, &ann.type_annotation))
            .transpose()?
            .ok_or_else(|| ctx.err(p.span(), format!("参数 {} 必须有类型标注", id.name)))?;
        params.push(Param {
            name: id.name.as_str().to_string(),
            ty,
        });
    }
    let body = match &f.body {
        Some(b) => {
            // 参数先进入作用域，函数体内的变量引用才能解析。
            let mut scope = Scope::default();
            for p in &params {
                scope.vars.insert(p.name.clone(), p.ty);
            }
            lower_statements_with(ctx, &b.statements, &mut scope)?
        }
        None => return Err(ctx.err(f.span(), format!("函数 {name} 必须有函数体"))),
    };
    Ok(Func {
        name: String::from(global),
        params,
        ret,
        line: line_of(ctx.src, f.span().start as usize),
        module_path: String::from(ctx.path),
        body,
    })
}

/// 从 AST 类型标注提取机器类型。
fn type_from_ts(ctx: &Ctx, ty: &TSType) -> Result<MachineType, BuildError> {
    let name = match ty {
        TSType::TSNumberKeyword(_) => "number",
        TSType::TSBooleanKeyword(_) => "boolean",
        TSType::TSVoidKeyword(_) => "void",
        TSType::TSTypeReference(r) => match &r.type_name {
            TSTypeName::IdentifierReference(id) => id.name.as_str(),
            _ => return Err(ctx.err(ty.span(), "类型引用必须是简单标识符")),
        },
        other => {
            return Err(ctx.err(other.span(), format!("本阶段不支持该类型标注：{other:?}")));
        }
    };
    match repr::decide(name) {
        Representation::Pod(mt) => Ok(mt),
        Representation::Unsupported => Err(ctx.err(
            ty.span(),
            format!("类型 `{name}` 本阶段不支持（无确定内存表示）"),
        )),
    }
}

/// 语句序列降低，继承调用方准备好的作用域（参数等）。
fn lower_statements_with(
    ctx: &Ctx,
    stmts: &[Statement],
    scope: &mut Scope,
) -> Result<Block, BuildError> {
    let mut out = Vec::new();
    for s in stmts {
        lower_stmt(ctx, s, scope, &mut out)?;
    }
    Ok(out)
}

#[derive(Default)]
struct Scope {
    vars: HashMap<String, MachineType>,
}

fn lower_stmt(
    ctx: &Ctx,
    s: &Statement,
    scope: &mut Scope,
    out: &mut Block,
) -> Result<(), BuildError> {
    match s {
        Statement::VariableDeclaration(vd) => {
            for d in &vd.declarations {
                let BindingPattern::BindingIdentifier(id) = &d.id else {
                    return Err(ctx.err(d.id.span(), "变量声明必须为简单标识符"));
                };
                let ty = d
                    .type_annotation
                    .as_ref()
                    .map(|ann| type_from_ts(ctx, &ann.type_annotation))
                    .transpose()?
                    .ok_or_else(|| {
                        ctx.err(d.id.span(), format!("变量 {} 必须有类型标注", id.name))
                    })?;
                let init = d.init.as_ref().ok_or_else(|| {
                    ctx.err(
                        d.span(),
                        format!(
                            "变量 {} 必须有初始化式（本阶段不支持未初始化声明）",
                            id.name
                        ),
                    )
                })?;
                let e = lower_expr(ctx, init, scope, Some(ty))?;
                scope.vars.insert(id.name.as_str().to_string(), ty);
                out.push(Stmt::Let {
                    name: id.name.as_str().to_string(),
                    ty,
                    init: e,
                });
            }
            Ok(())
        }
        Statement::ExpressionStatement(es) => {
            let e = lower_expr(ctx, &es.expression, scope, None)?;
            out.push(Stmt::Expr(e));
            Ok(())
        }
        Statement::IfStatement(i) => {
            let cond = lower_expr(ctx, &i.test, scope, Some(MachineType::Bool))?;
            let then = match &i.consequent {
                Statement::BlockStatement(b) => lower_statements_with(ctx, &b.body, scope)?,
                other => {
                    let mut v = Vec::new();
                    lower_stmt(ctx, other, scope, &mut v)?;
                    v
                }
            };
            let els = match &i.alternate {
                // else if：降低为嵌套 If，保持结构扁平
                Some(Statement::IfStatement(_)) => {
                    let alt = i.alternate.as_ref().unwrap();
                    let mut inner = Vec::new();
                    lower_stmt(ctx, alt, scope, &mut inner)?;
                    Some(inner)
                }
                Some(Statement::BlockStatement(b)) => {
                    Some(lower_statements_with(ctx, &b.body, scope)?)
                }
                Some(other) => {
                    return Err(ctx.err(
                        other.span(),
                        format!("else 分支只允许块或 else if，遇到：{other:?}"),
                    ));
                }
                None => None,
            };
            out.push(Stmt::If { cond, then, els });
            Ok(())
        }
        Statement::WhileStatement(w) => {
            let cond = lower_expr(ctx, &w.test, scope, Some(MachineType::Bool))?;
            let body = match &w.body {
                Statement::BlockStatement(b) => lower_statements_with(ctx, &b.body, scope)?,
                other => {
                    let mut v = Vec::new();
                    lower_stmt(ctx, other, scope, &mut v)?;
                    v
                }
            };
            out.push(Stmt::While { cond, body });
            Ok(())
        }
        // do-while desugar 为「死循环 + 条件不满足时 break」：体在条件前执行，语义等价。
        Statement::DoWhileStatement(d) => {
            let body = match &d.body {
                Statement::BlockStatement(b) => lower_statements_with(ctx, &b.body, scope)?,
                other => {
                    let mut v = Vec::new();
                    lower_stmt(ctx, other, scope, &mut v)?;
                    v
                }
            };
            let cond = lower_expr(ctx, &d.test, scope, Some(MachineType::Bool))?;
            let mut desugared = body;
            desugared.push(Stmt::If {
                cond: Expr::Not(Box::new(cond)),
                then: vec![Stmt::Break],
                els: None,
            });
            out.push(Stmt::While {
                cond: Expr::Bool(true),
                body: desugared,
            });
            Ok(())
        }
        Statement::ForStatement(f) => {
            // for (let i: T = a; c; s) —— 三段式原样保留
            let (var, ty, init) = match &f.init {
                Some(ForStatementInit::VariableDeclaration(vd)) => {
                    let d = vd
                        .declarations
                        .first()
                        .ok_or_else(|| ctx.err(vd.span(), "for 初始化不能为空"))?;
                    let BindingPattern::BindingIdentifier(id) = &d.id else {
                        return Err(ctx.err(d.id.span(), "for 初始化变量必须为简单标识符"));
                    };
                    let ty = d
                        .type_annotation
                        .as_ref()
                        .map(|ann| type_from_ts(ctx, &ann.type_annotation))
                        .transpose()?
                        .ok_or_else(|| ctx.err(d.id.span(), "for 变量必须有类型标注"))?;
                    let e = d
                        .init
                        .as_ref()
                        .ok_or_else(|| ctx.err(d.span(), "for 变量必须有初始化式"))?;
                    let e = lower_expr(ctx, e, scope, Some(ty))?;
                    scope.vars.insert(id.name.as_str().to_string(), ty);
                    (id.name.as_str().to_string(), ty, e)
                }
                _ => return Err(ctx.err(f.span(), "for 初始化只支持带类型标注的变量声明")),
            };
            let cond = match &f.test {
                Some(e) => lower_expr(ctx, e, scope, Some(MachineType::Bool))?,
                None => {
                    return Err(ctx.err(f.span(), "for 必须有循环条件（本阶段不支持无限循环）"));
                }
            };
            let step = match &f.update {
                Some(e) => lower_expr(ctx, e, scope, Some(ty))?,
                None => return Err(ctx.err(f.span(), "for 必须有步进表达式")),
            };
            let body = match &f.body {
                Statement::BlockStatement(b) => lower_statements_with(ctx, &b.body, scope)?,
                other => {
                    let mut v = Vec::new();
                    lower_stmt(ctx, other, scope, &mut v)?;
                    v
                }
            };
            out.push(Stmt::For {
                var,
                ty,
                init,
                cond,
                step,
                body,
            });
            Ok(())
        }
        // switch desugar 为 if-else 链：连续无 break/return 结尾的 case 合并为一组
        // （穿透语义）；末尾 break 丢弃（if 块结束即跳出）；非末尾 break 拒绝。
        // oxc 的 SwitchCase 以 `test: Option<Expression>` 区分 case 与 default。
        Statement::SwitchStatement(sw) => {
            let disc = lower_expr(ctx, &sw.discriminant, scope, None)?;
            let disc_ty = expr_type(&disc)
                .ok_or_else(|| ctx.err(sw.span(), "switch 判别式无法确定类型（请加类型标注）"))?;
            let mut groups: Vec<(Vec<Expr>, Block)> = Vec::new();
            let mut conds: Vec<Expr> = Vec::new();
            let mut body: Block = Vec::new();
            let mut terminated = true;
            let mut default_body: Option<Block> = None;
            let case_count = sw.cases.len();
            for (index, clause) in sw.cases.iter().enumerate() {
                match &clause.test {
                    Some(test) => {
                        // case 条件 desugar 为「判别式 == 表达式」（与用户手写比较同构，
                        // ty 标操作数类型——check 期比较运算的结果类型修正为 Bool）。
                        let case_expr = lower_expr(ctx, test, scope, Some(disc_ty))?;
                        let cond = Expr::Binary {
                            op: BinOp::Eq,
                            lhs: Box::new(disc.clone()),
                            rhs: Box::new(case_expr),
                            ty: disc_ty,
                        };
                        if terminated {
                            // 上一组已结束：先结算，再开新组。
                            if !conds.is_empty() {
                                groups
                                    .push((std::mem::take(&mut conds), std::mem::take(&mut body)));
                            }
                            conds.push(cond);
                        } else {
                            // 穿透：与上一组同条件分支。
                            conds.push(cond);
                        }
                        terminated = false;
                        let n = clause.consequent.len();
                        for (i, st) in clause.consequent.iter().enumerate() {
                            if let Statement::BreakStatement(_) = st {
                                if i + 1 != n {
                                    return Err(ctx.err(
                                        st.span(),
                                        "switch 内 break 只支持出现在 case 体末尾",
                                    ));
                                }
                                terminated = true;
                                continue;
                            }
                            lower_stmt(ctx, st, scope, &mut body)?;
                        }
                        // 体以 return 结尾同样终止本组（return 保留在体内）。
                        if matches!(body.last(), Some(Stmt::Return(_))) {
                            terminated = true;
                        }
                    }
                    None => {
                        // default：只允许出现在末尾，其体作为 else 分支。
                        if index + 1 != case_count {
                            return Err(ctx.err(
                                clause.span(),
                                "default 只支持出现在 switch 末尾（本阶段限制）",
                            ));
                        }
                        let mut default_block = Vec::new();
                        for st in &clause.consequent {
                            if let Statement::BreakStatement(_) = st {
                                continue;
                            }
                            lower_stmt(ctx, st, scope, &mut default_block)?;
                        }
                        default_body = Some(default_block);
                    }
                }
            }
            if !conds.is_empty() {
                groups.push((conds, body));
            }
            // 从后往前渲染 if-else 链。
            let mut els: Option<Block> = default_body;
            for (group_conds, group_body) in groups.iter().rev() {
                let mut iter = group_conds.iter();
                let first = iter
                    .next()
                    .ok_or_else(|| ctx.err(sw.span(), "switch 组缺少条件"))?
                    .clone();
                let cond = iter.fold(first, |acc, c| Expr::Binary {
                    op: BinOp::Or,
                    lhs: Box::new(acc),
                    rhs: Box::new(c.clone()),
                    ty: MachineType::Bool,
                });
                let if_stmt = Stmt::If {
                    cond,
                    then: group_body.clone(),
                    els: els.take(),
                };
                els = Some(vec![if_stmt]);
            }
            // 无任何 case（空 switch）：不生成语句。
            if let Some(stmts) = els {
                out.extend(stmts);
            }
            Ok(())
        }
        Statement::ReturnStatement(r) => {
            let e = match &r.argument {
                Some(e) => Some(lower_expr(ctx, e, scope, None)?),
                None => None,
            };
            out.push(Stmt::Return(e));
            Ok(())
        }
        Statement::BreakStatement(_) => {
            out.push(Stmt::Break);
            Ok(())
        }
        Statement::ContinueStatement(_) => {
            out.push(Stmt::Continue);
            Ok(())
        }
        Statement::BlockStatement(b) => {
            let inner = lower_statements_with(ctx, &b.body, scope)?;
            out.extend(inner);
            Ok(())
        }
        other => Err(ctx.err(other.span(), format!("本阶段不支持该语句：{other:?}"))),
    }
}

/// 表达式降低。`expected` 为期望类型（来自声明 / 形参 / 返回 / 运算符另一侧）。
fn lower_expr(
    ctx: &Ctx,
    e: &Expression,
    scope: &Scope,
    expected: Option<MachineType>,
) -> Result<Expr, BuildError> {
    match e {
        Expression::NumericLiteral(n) => {
            let v = n.value;
            match expected {
                // 有期望类型时字面量服从该类型（范围/精度在下方校验）。
                Some(ty) if ty.is_float() => {
                    if ty == MachineType::F32 && (!v.is_finite() || v as f32 as f64 != v) {
                        return Err(ctx.err(n.span(), format!("浮点字面量 {v} 无法精确表示为 f32")));
                    }
                    Ok(Expr::Float { value: v, ty })
                }
                Some(ty) if ty.is_integer() => {
                    // 健全性：浮点形态字面量不得静默截断进整数类型。
                    if v.fract() != 0.0 {
                        return Err(ctx.err(
                            n.span(),
                            format!("浮点字面量 {v} 不能赋给整数类型 {ty:?}（无隐式转换）"),
                        ));
                    }
                    check_int_range(v, ty).map_err(|m| ctx.err(n.span(), m))?;
                    if ty == MachineType::U64 && v > i64::MAX as f64 {
                        return Err(ctx.err(n.span(), "u64 字面量超出 i64 正数范围，本阶段不支持"));
                    }
                    Ok(Expr::Int {
                        value: v as i64,
                        ty,
                    })
                }
                Some(_) => Err(ctx.err(n.span(), format!("字面量 {v} 与期望类型不匹配"))),
                // 无期望时按字面量形态：整数 → i32，浮点 → f64。
                None if v.fract() == 0.0 => {
                    check_int_range(v, MachineType::I32).map_err(|m| ctx.err(n.span(), m))?;
                    Ok(Expr::Int {
                        value: v as i64,
                        ty: MachineType::I32,
                    })
                }
                None => Ok(Expr::Float {
                    value: v,
                    ty: MachineType::F64,
                }),
            }
        }
        Expression::BooleanLiteral(b) => Ok(Expr::Bool(b.value)),
        Expression::Identifier(id) => {
            let ty = scope.vars.get(id.name.as_str()).copied().ok_or_else(|| {
                ctx.err(id.span(), format!("变量 {} 未声明或无类型标注", id.name))
            })?;
            Ok(Expr::Var(id.name.as_str().to_string(), ty))
        }
        Expression::UnaryExpression(u) => {
            let inner = lower_expr(ctx, &u.argument, scope, expected)?;
            match u.operator {
                UnaryOperator::UnaryNegation => Ok(Expr::UnaryNeg(Box::new(inner))),
                UnaryOperator::LogicalNot => Ok(Expr::Not(Box::new(inner))),
                other => Err(ctx.err(u.span(), format!("本阶段不支持一元运算符：{other:?}"))),
            }
        }
        Expression::LogicalExpression(l) => {
            let lhs = lower_expr(ctx, &l.left, scope, Some(MachineType::Bool))?;
            let rhs = lower_expr(ctx, &l.right, scope, Some(MachineType::Bool))?;
            let op = match l.operator {
                LogicalOperator::And => BinOp::And,
                LogicalOperator::Or => BinOp::Or,
                // `??` 是 null 合并，需要可空类型语义，本阶段不支持。
                LogicalOperator::Coalesce => {
                    return Err(ctx.err(l.span(), "本阶段不支持 `??`（null 合并）"));
                }
            };
            Ok(Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                ty: MachineType::Bool,
            })
        }
        Expression::BinaryExpression(b) => {
            // 期望类型的传播规则：Bool 是「结果语境」（比较/逻辑运算的结果），
            // 不向操作数传播；数值期望传播给左侧，右侧服从左侧的类型。
            // 字面量在无标注语境默认 i32/f64——不跨类型隐式提升（与「无隐式转换」一致）。
            let lhs_expected = match expected {
                Some(MachineType::Bool) => None,
                other => other,
            };
            let lhs = lower_expr(ctx, &b.left, scope, lhs_expected)?;
            let rhs = lower_expr(ctx, &b.right, scope, expr_type(&lhs))?;
            let ty = expr_type(&lhs).or(expr_type(&rhs)).ok_or_else(|| {
                ctx.err(b.span(), "二元运算无法确定操作数类型（请为变量加类型标注）")
            })?;
            let op = map_binop(b.operator).map_err(|m| ctx.err(b.span(), m))?;
            Ok(Expr::Binary {
                op,
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
                ty,
            })
        }
        Expression::ConditionalExpression(c) => {
            let cond = lower_expr(ctx, &c.test, scope, Some(MachineType::Bool))?;
            let then = lower_expr(ctx, &c.consequent, scope, expected)?;
            let els = lower_expr(ctx, &c.alternate, scope, expected)?;
            let ty = expr_type(&then).ok_or_else(|| ctx.err(c.span(), "条件表达式无法确定类型"))?;
            Ok(Expr::Cond {
                cond: Box::new(cond),
                then: Box::new(then),
                els: Box::new(els),
                ty,
            })
        }
        Expression::CallExpression(c) => {
            let callee = match &c.callee {
                Expression::Identifier(id) => id.name.as_str().to_string(),
                other => {
                    return Err(
                        ctx.err(other.span(), format!("本阶段只支持直接调用函数：{other:?}"))
                    );
                }
            };
            // 内建 print：固定 void，接受任意数值实参。
            let is_print = callee == "print";
            let global = if is_print {
                String::from("print")
            } else {
                resolve_callee(ctx, &callee, c.span())?
            };
            let mut args = Vec::new();
            for (index, a) in c.arguments.iter().enumerate() {
                match a {
                    Argument::SpreadElement(_) => {
                        return Err(ctx.err(a.span(), "本阶段不支持展开参数"));
                    }
                    other => {
                        // 实参服从形参类型（print 除外：它接受任意数值实参）。
                        let arg_expected = if is_print {
                            None
                        } else {
                            ctx.sigs
                                .get(&global)
                                .and_then(|(params, _)| params.get(index))
                                .map(|p| p.ty)
                        };
                        args.push(lower_expr(ctx, other.to_expression(), scope, arg_expected)?)
                    }
                }
            }
            // 返回类型：print 固定 void；用户函数查签名表（两遍降低保证表已完整）。
            let ty = if is_print {
                MachineType::Void
            } else {
                match ctx.sigs.get(&global) {
                    Some((_, ret)) => *ret,
                    None => return Err(ctx.err(c.span(), format!("函数 {callee} 未声明"))),
                }
            };
            Ok(Expr::Call {
                name: global,
                args,
                ty,
            })
        }
        // 括号在 TIR 层无意义，直接解包递归降低。
        Expression::ParenthesizedExpression(p) => lower_expr(ctx, &p.expression, scope, expected),
        // `as`：仅允许「可证明安全」的扩宽子集，语义校验在 check 阶段；
        // 降低阶段只做「源类型已知 + 目标类型可表示」两件事。
        Expression::TSAsExpression(a) => {
            let value = lower_expr(ctx, &a.expression, scope, None)?;
            let from = expr_type(&value).ok_or_else(|| {
                ctx.err(
                    a.span(),
                    "`as` 的源类型无法确定（请为源表达式加类型标注或使用变量）",
                )
            })?;
            let to = type_from_ts(ctx, &a.type_annotation)?;
            Ok(Expr::Cast {
                value: Box::new(value),
                from,
                to,
            })
        }
        // 赋值是表达式：`i = i + 1` 与 `i++` 都归到这里。
        // 复合赋值（+= 等）在此 desugar 为「读取 + 运算 + 写回」。
        Expression::AssignmentExpression(a) => {
            let Some(SimpleAssignmentTarget::AssignmentTargetIdentifier(id)) =
                a.left.as_simple_assignment_target()
            else {
                return Err(ctx.err(a.span(), "赋值目标必须是简单标识符"));
            };
            let ty = scope
                .vars
                .get(id.name.as_str())
                .copied()
                .ok_or_else(|| ctx.err(id.span(), format!("变量 {} 未声明", id.name)))?;
            let op = match a.operator {
                AssignmentOperator::Assign => None,
                AssignmentOperator::Addition => Some(BinOp::Add),
                AssignmentOperator::Subtraction => Some(BinOp::Sub),
                AssignmentOperator::Multiplication => Some(BinOp::Mul),
                AssignmentOperator::Division => Some(BinOp::Div),
                AssignmentOperator::Remainder => Some(BinOp::Rem),
                _ => {
                    return Err(ctx.err(
                        a.span(),
                        "只支持 += -= *= /= %=（位运算与逻辑赋值本阶段不支持）",
                    ));
                }
            };
            let value = match op {
                None => lower_expr(ctx, &a.right, scope, Some(ty))?,
                Some(op) => {
                    // a op= b desugar 为 a = a op b（a 的类型即目标类型）。
                    let rhs = lower_expr(ctx, &a.right, scope, Some(ty))?;
                    Expr::Binary {
                        op,
                        lhs: Box::new(Expr::Var(id.name.as_str().to_string(), ty)),
                        rhs: Box::new(rhs),
                        ty,
                    }
                }
            };
            Ok(Expr::Assign {
                name: id.name.as_str().to_string(),
                ty,
                value: Box::new(value),
            })
        }
        // `i++` / `++i` / `i--` / `--i` 显式 desugar 为 `i = i ± 1`。
        Expression::UpdateExpression(u) => {
            let SimpleAssignmentTarget::AssignmentTargetIdentifier(id) = &u.argument else {
                return Err(ctx.err(u.span(), "自增/自减的目标必须是简单标识符"));
            };
            let ty = scope
                .vars
                .get(id.name.as_str())
                .copied()
                .ok_or_else(|| ctx.err(id.span(), format!("变量 {} 未声明", id.name)))?;
            let current = Expr::Var(id.name.as_str().to_string(), ty);
            let delta = if ty.is_float() {
                Expr::Float {
                    value: if u.operator == UpdateOperator::Increment {
                        1.0
                    } else {
                        -1.0
                    },
                    ty,
                }
            } else {
                Expr::Int {
                    value: if u.operator == UpdateOperator::Increment {
                        1
                    } else {
                        -1
                    },
                    ty,
                }
            };
            let value = Expr::Binary {
                op: BinOp::Add,
                lhs: Box::new(current),
                rhs: Box::new(delta),
                ty,
            };
            Ok(Expr::Assign {
                name: id.name.as_str().to_string(),
                ty,
                value: Box::new(value),
            })
        }
        other => Err(ctx.err(other.span(), format!("本阶段不支持该表达式：{other:?}"))),
    }
}

/// Call 表达式的被调名解析（多文件）：导入绑定 → 本模块原名 → 模块前缀。
fn resolve_callee(ctx: &Ctx, local: &str, span: Span) -> Result<String, BuildError> {
    if let Some(global) = ctx.bindings.get(local) {
        return Ok(global.clone());
    }
    if ctx.sigs.contains_key(local) {
        return Ok(String::from(local));
    }
    if !ctx.module_prefix.is_empty() {
        let prefixed = format!("{}::{local}", ctx.module_prefix);
        if ctx.sigs.contains_key(&prefixed) {
            return Ok(prefixed);
        }
    }
    Err(ctx.err(span, format!("函数 {local} 未声明")))
}

fn map_binop(op: BinaryOperator) -> Result<BinOp, String> {
    Ok(match op {
        BinaryOperator::Addition => BinOp::Add,
        BinaryOperator::Subtraction => BinOp::Sub,
        BinaryOperator::Multiplication => BinOp::Mul,
        BinaryOperator::Division => BinOp::Div,
        BinaryOperator::Remainder => BinOp::Rem,
        BinaryOperator::Equality | BinaryOperator::StrictEquality => BinOp::Eq,
        BinaryOperator::Inequality | BinaryOperator::StrictInequality => BinOp::Ne,
        BinaryOperator::LessThan => BinOp::Lt,
        BinaryOperator::LessEqualThan => BinOp::Le,
        BinaryOperator::GreaterThan => BinOp::Gt,
        BinaryOperator::GreaterEqualThan => BinOp::Ge,
        other => return Err(format!("本阶段不支持该运算符：{other:?}")),
    })
}

/// 整数字面量范围校验：超出目标类型表示范围即编译期报错（健全性检查）。
fn check_int_range(v: f64, ty: MachineType) -> Result<(), String> {
    let (min, max) = match ty {
        MachineType::I8 => (i8::MIN as f64, i8::MAX as f64),
        MachineType::I16 => (i16::MIN as f64, i16::MAX as f64),
        MachineType::I32 => (i32::MIN as f64, i32::MAX as f64),
        MachineType::I64 => (i64::MIN as f64, i64::MAX as f64),
        MachineType::U8 => (0.0, u8::MAX as f64),
        MachineType::U16 => (0.0, u16::MAX as f64),
        MachineType::U32 => (0.0, u32::MAX as f64),
        MachineType::U64 => (0.0, u64::MAX as f64),
        // 布尔/浮点/void 不接收整数字面量（由调用方保证）。
        _ => return Err(format!("整数字面量不能赋给类型 {ty:?}")),
    };
    if v < min || v > max {
        return Err(format!("整数字面量 {v} 超出类型 {ty:?} 的表示范围"));
    }
    Ok(())
}

/// 表达式类型（仅当类型可直接从节点读出时）。
fn expr_type(e: &Expr) -> Option<MachineType> {
    match e {
        Expr::Var(_, ty) => Some(*ty),
        Expr::Int { ty, .. } => Some(*ty),
        Expr::Float { ty, .. } => Some(*ty),
        Expr::Binary { ty, .. } => Some(*ty),
        Expr::Cond { ty, .. } => Some(*ty),
        Expr::Cast { to, .. } => Some(*to),
        Expr::Assign { ty, .. } => Some(*ty),
        Expr::Bool(_) => Some(MachineType::Bool),
        Expr::UnaryNeg(inner) => expr_type(inner),
        Expr::Not(inner) => expr_type(inner),
        Expr::Call { ty, .. } => Some(*ty),
    }
}

/// 由字节偏移换算 1 起始行号（lower 期错误定位的辅助）。
fn line_of(src: &str, offset: usize) -> u32 {
    let offset = offset.min(src.len());
    let mut line = 1u32;
    for (i, ch) in src.char_indices() {
        if i >= offset {
            break;
        }
        if ch == '\n' {
            line += 1;
        }
    }
    line
}
