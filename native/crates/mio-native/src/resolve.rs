//! 模块解析：把「入口文件 + import 图」展开为单文件视图。
//!
//! 多文件降低的前置步骤。`import { f } from "./m"` 在本层被解析为
//! 模块序列（路径 + 源码 + 本地名 → 全局函数名的绑定表），供 `lower` 的
//! 多文件降低使用。设计边界（与阶段 A 的 Import Maps 语义区分）：
//!
//! - 只支持**具名导入**（`import { a, b } from "./m"`），不支持默认导入、
//!   命名空间导入、`as` 别名、`import type`（明确报错）。
//! - 路径只接受相对路径（`./` / `../`），扩展名 `.ts` 可省略；
//!   裸包名与 `node:` 前缀给出明确报错（本阶段无 node_modules）。
//! - 循环导入在解析期检出并报错（不进入降低）。
//! - `main` 只能在入口文件；被引模块里的 `main` 报错。
//!
//! 生命周期说明：本模块 parse 一次只为提取 import 图与顶层形态校验；
//! 函数体的降低在 `lower` 内对同一源码再次 parse（oxc 的 AST 借用
//! allocator，无法跨函数持有所有权；子集程序文件小，重复解析的代价可忽略）。

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use oxc_allocator::Allocator;
use oxc_ast::ast::*;
use oxc_parser::Parser;
use oxc_span::SourceType;

/// 展开后的一个模块：源码 + 导入绑定 + 来源路径。
#[derive(Debug)]
pub struct ModuleSource {
    /// 源文件路径（诊断展示用）。
    pub path: String,
    /// 源码原文（降低期重新 parse 用）。
    pub src: String,
    /// 导入绑定：本地名 → 全局函数名（被引模块为 `模块前缀::函数名`）。
    pub bindings: HashMap<String, String>,
    /// 是否入口模块（入口的 main 与无前置模块名）。
    pub is_entry: bool,
    /// 全局名前缀（入口为空串；被引模块为相对入口的路径段，`/` 换 `::`）。
    pub prefix: String,
}

/// 从入口文件展开 import 图（深度优先，先被引模块后本模块入表）。
///
/// # Errors
/// 文件读取失败、语法错误、非相对路径、裸包名、`node:` 前缀、循环导入、
/// 重复导入同名函数、`import type`、默认/命名空间导入、`as` 别名、
/// 被引模块声明 `main`，均报错。
pub fn expand(entry_path: &str, entry_src: &str) -> Result<Vec<ModuleSource>, String> {
    // 入口源码由调用方提供（编译入口已读入，避免重复读盘）；被引模块在本函数内读盘。
    let entry_path_buf = PathBuf::from(entry_path);

    let mut modules: Vec<ModuleSource> = Vec::new();
    let mut visiting: HashSet<PathBuf> = HashSet::new();
    let mut visited: HashSet<PathBuf> = HashSet::new();

    load_module(
        &entry_path_buf,
        entry_src,
        true,
        "",
        &mut modules,
        &mut visiting,
        &mut visited,
    )?;

    Ok(modules)
}

/// 递归加载一个模块：先递归其 import（保证降低期签名表完整），再登记本模块。
fn load_module(
    path: &Path,
    src: &str,
    is_entry: bool,
    rel: &str,
    modules: &mut Vec<ModuleSource>,
    visiting: &mut HashSet<PathBuf>,
    visited: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let canonical = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
    if visited.contains(&canonical) {
        return Ok(());
    }
    if !visiting.insert(canonical.clone()) {
        return Err(format!("循环导入：{}", path.display()));
    }

    let allocator = Allocator::default();
    let source_type = SourceType::default().with_typescript(true);
    let ret = Parser::new(&allocator, src, source_type).parse();
    if !ret.diagnostics.is_empty() {
        return Err(format!(
            "{}：语法错误：{}",
            path.display(),
            ret.diagnostics[0]
        ));
    }

    // 全局名前缀：入口为空串；被引模块为相对入口的路径段（/ 换 ::）。
    let prefix = if is_entry {
        String::new()
    } else {
        rel.replace('/', "::")
    };
    let mut bindings = HashMap::new();
    for stmt in &ret.program.body {
        match stmt {
            Statement::ImportDeclaration(import) => {
                // 校验被引模块形态（main 归属）要在加载之后进行——先展开图。
                for (local, global) in import_bindings(import, path, rel)? {
                    if bindings.insert(local.clone(), global).is_some() {
                        return Err(format!("重复导入同名函数：{local}"));
                    }
                }
            }
            Statement::FunctionDeclaration(f) => {
                if !is_entry && f.id.as_ref().is_some_and(|id| id.name.as_str() == "main") {
                    return Err(format!(
                        "{}：main 只能在入口文件声明（被引模块不允许）",
                        path.display()
                    ));
                }
            }
            _ => {
                return Err(format!(
                    "{}：顶层只允许函数声明与 import（遇到：{}）",
                    path.display(),
                    stmt_name(stmt)
                ));
            }
        }
    }

    // 先递归加载被引模块，再登记本模块（降低期签名表自底向上完整）。
    for stmt in &ret.program.body {
        if let Statement::ImportDeclaration(import) = stmt {
            let (resolved, dep_rel) = resolve_source(&import.source, path, rel)?;
            let dep_src = std::fs::read_to_string(&resolved).map_err(|e| {
                format!(
                    "读取 {} 失败：{e}（由 {} 的 import 引入）",
                    resolved.display(),
                    path.display()
                )
            })?;
            load_module(
                &resolved, &dep_src, false, &dep_rel, modules, visiting, visited,
            )?;
        }
    }

    visiting.remove(&canonical);
    visited.insert(canonical);
    modules.push(ModuleSource {
        path: path.to_string_lossy().into_owned(),
        src: String::from(src),
        bindings,
        is_entry,
        prefix,
    });
    Ok(())
}

/// 解析一个 import 的全部具名绑定：本地名 → 全局函数名（带模块前缀）。
fn import_bindings(
    import: &ImportDeclaration,
    referrer: &Path,
    referrer_rel: &str,
) -> Result<Vec<(String, String)>, String> {
    if import.import_kind == ImportOrExportKind::Type {
        return Err(format!(
            "{}：`import type` 不支持（本阶段无类型级导入）",
            referrer.display()
        ));
    }
    let (_, dep_rel) = resolve_source(&import.source, referrer, referrer_rel)?;
    let prefix = dep_rel.replace('/', "::");
    let Some(specifiers) = &import.specifiers else {
        // `import "./m"`（纯副作用）：本阶段无副作用语义，明确拒绝。
        return Err(format!(
            "{}：只支持具名导入 `import {{ f }} from \"./m\"`",
            referrer.display()
        ));
    };
    let mut out = Vec::new();
    for spec in specifiers {
        let ImportDeclarationSpecifier::ImportSpecifier(s) = spec else {
            return Err(format!(
                "{}：只支持具名导入（不支持默认导入与命名空间导入）",
                referrer.display()
            ));
        };
        // 导入名只接受标识符（`import { "a-b" as x }` 的字符串形式不支持）。
        let ModuleExportName::IdentifierName(imported) = &s.imported else {
            return Err(format!(
                "{}：导入名必须是标识符（不支持字符串形式）",
                referrer.display()
            ));
        };
        // 不支持 `as` 别名：imported 与 local 必须同名。
        if imported.name.as_str() != s.local.name.as_str() {
            return Err(format!(
                "{}：不支持 `as` 重命名（imported 与 local 必须同名）",
                referrer.display()
            ));
        }
        out.push((
            s.local.name.as_str().to_string(),
            format!("{prefix}::{}", imported.name.as_str()),
        ));
    }
    Ok(out)
}

/// 把被引源码路径解析为磁盘路径：相对路径（相对引用者目录），`.ts` 可省略。
/// 解析被引路径：返回 (磁盘路径, 相对入口的路径段（去扩展名）)。
fn resolve_source(
    source: &StringLiteral,
    referrer: &Path,
    referrer_rel: &str,
) -> Result<(PathBuf, String), String> {
    // JSStr::as_str 对含 lone surrogate 的串返回 None；导入路径必为 ASCII，None 即非法。
    let Some(raw) = source.value.as_str() else {
        return Err(format!(
            "{}：导入路径包含无法表示的字符",
            referrer.display()
        ));
    };
    if raw.starts_with("node:") {
        return Err(String::from("`node:` 前缀不支持（本阶段无 Node 兼容层）"));
    }
    if !raw.starts_with("./") && !raw.starts_with("../") {
        return Err(format!("只支持相对路径导入（./ 或 ../），遇到：{raw}"));
    }
    let base = referrer.parent().unwrap_or(Path::new("."));
    let mut candidate = base.join(raw);
    if candidate.extension().is_none() {
        candidate = candidate.with_extension("ts");
    }
    if !candidate.exists() {
        return Err(format!("导入的文件不存在：{}", candidate.display()));
    }
    let parent_rel = if referrer_rel.is_empty() {
        String::new()
    } else {
        referrer_rel
            .rsplit_once('/')
            .map_or(String::new(), |(parent, _)| String::from(parent))
    };
    Ok((candidate, join_rel(&parent_rel, raw)))
}

/// 合并相对路径段（`..` 弹栈、`.` 忽略），返回以 `/` 分隔的段串（已去 `.ts`）。
fn join_rel(parent: &str, spec: &str) -> String {
    let spec = spec.trim_end_matches(".ts");
    let mut parts: Vec<String> = if parent.is_empty() {
        Vec::new()
    } else {
        parent.split('/').map(String::from).collect()
    };
    for seg in spec.split('/') {
        match seg {
            "." | "" => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(String::from(other)),
        }
    }
    parts.join("/")
}

/// 顶层语句的人类可读名字（用于「顶层只允许函数声明与 import」的报错）。
fn stmt_name(stmt: &Statement) -> &'static str {
    match stmt {
        Statement::VariableDeclaration(_) => "变量声明",
        Statement::ExpressionStatement(_) => "表达式语句",
        Statement::IfStatement(_) => "if",
        Statement::WhileStatement(_) => "while",
        Statement::ForStatement(_) => "for",
        Statement::ReturnStatement(_) => "return",
        Statement::BreakStatement(_) => "break",
        Statement::ContinueStatement(_) => "continue",
        Statement::BlockStatement(_) => "块语句",
        _ => "其他语句",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录写文件并展开 import 图。
    fn expand_in(dir: &Path, entry: &str) -> Result<Vec<ModuleSource>, String> {
        let path = dir.join(entry);
        let src = std::fs::read_to_string(&path).unwrap();
        expand(&path.to_string_lossy(), &src)
    }

    fn temp_dir(tag: &str) -> PathBuf {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("mio-resolve-{tag}-{nanos}"));
        std::fs::create_dir_all(&dir).expect("创建临时目录");
        dir
    }

    #[test]
    fn chain_import_produces_prefix_bindings() {
        let dir = temp_dir("chain");
        std::fs::write(
            dir.join("util.ts"),
            "function abs(x: i64): i64 {\n  return x;\n}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("app.ts"),
            "import { abs } from \"./util\";\nfunction main(): void {}\n",
        )
        .unwrap();

        let modules = expand_in(&dir, "app.ts").expect("应展开成功");
        // 先被引模块后入口（降低期签名表自底向上完整）。
        assert_eq!(modules.len(), 2);
        assert!(modules[0].path.ends_with("util.ts"));
        assert!(modules[1].is_entry);
        // 入口绑定：abs → util::abs（路径前缀）。
        assert_eq!(
            modules[1].bindings.get("abs").map(String::as_str),
            Some("util::abs")
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn extension_optional_and_dot_ts_explicit_both_work() {
        let dir = temp_dir("ext");
        std::fs::write(dir.join("m.ts"), "function f(): void {}\n").unwrap();
        std::fs::write(
            dir.join("a.ts"),
            "import { f } from \"./m\";\nfunction main(): void {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.ts"),
            "import { f } from \"./m.ts\";\nfunction main(): void {}\n",
        )
        .unwrap();

        for entry in ["a.ts", "b.ts"] {
            let modules = expand_in(&dir, entry).expect("应展开成功");
            assert_eq!(modules.len(), 2, "{entry}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn cycle_is_rejected() {
        let dir = temp_dir("cycle");
        std::fs::write(
            dir.join("a.ts"),
            "import { b } from \"./b\";\nfunction a(): void {}\nfunction main(): void {}\n",
        )
        .unwrap();
        std::fs::write(
            dir.join("b.ts"),
            "import { a } from \"./a\";\nfunction b(): void {}\n",
        )
        .unwrap();

        let err = expand_in(&dir, "a.ts").expect_err("循环导入应报错");
        assert!(err.contains("循环导入"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bare_specifier_and_missing_file_are_rejected() {
        let dir = temp_dir("bad");
        std::fs::write(
            dir.join("a.ts"),
            "import { f } from \"some-pkg\";\nfunction main(): void {}\n",
        )
        .unwrap();
        let err = expand_in(&dir, "a.ts").expect_err("裸包名应报错");
        assert!(err.contains("相对路径"), "{err}");

        std::fs::write(
            dir.join("b.ts"),
            "import { f } from \"./nope\";\nfunction main(): void {}\n",
        )
        .unwrap();
        let err = expand_in(&dir, "b.ts").expect_err("文件缺失应报错");
        assert!(err.contains("不存在"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn main_outside_entry_is_rejected() {
        let dir = temp_dir("main2");
        std::fs::write(dir.join("m.ts"), "function main(): void {}\n").unwrap();
        std::fs::write(
            dir.join("app.ts"),
            "import { main } from \"./m\";\nfunction main(): void {}\n",
        )
        .unwrap();
        let err = expand_in(&dir, "app.ts").expect_err("被引模块的 main 应报错");
        assert!(err.contains("main 只能在入口文件"), "{err}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
