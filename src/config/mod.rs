//! 配置层：`mio.json` 的解析、发现、继承与优先级叠加。
//!
//! 骨架借鉴 `tsconfig.json`：顶层 `compilerOptions` 承载编译器开关，
//! `include` / `exclude` / `files` 承载文件选择，`extends` 承载配置继承，支持 JSONC 注释。
//!
//! 与 `tsconfig.json` 的关键差异：模块映射不使用 `paths` / `baseUrl`。
//! `paths` 是编译期概念，不影响产物且依赖打包器配合；
//! mio 的模块解析发生在运行期，因此采用 Import Maps 规范的 `imports` / `scopes`。
//!
//! 配置来源优先级（低到高）：内建默认 → 配置文件（含 `extends` 链）→ `MIO_*` 环境变量 → 命令行选项。
//!
//! 每个字段必须标注生效状态，共三档：
//! - 生效：本版本真实影响运行时行为
//! - 仅提示：本版本只记录并告警，不改变行为
//! - 预留：字段被解析但完全不参与运行时

mod compiler_options;
mod diagnostics;
mod glob;
mod jsonc;
mod loader;
mod overrides;

pub use compiler_options::CompilerOptions;
pub use diagnostics::Diagnostic;
pub use glob::Glob;
pub use loader::{ConfigLoader, Environment, LoadedConfig, MapEnvironment, ProcessEnvironment};
pub use overrides::ConfigOverride;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::resolver::ModuleResolver;

/// 顶层配置聚合根，字段与 `mio.json` 的顶层键一一对应。
///
/// 所有字段均带 `#[serde(default)]`，因此配置文件可以只写需要覆盖的部分。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct MioConfig {
    /// 配置文件继承来源，取值可为单个路径或路径数组。
    /// 生效：父配置先加载，子配置逐字段覆盖。
    pub extends: Option<Extends>,

    /// 编译器开关，语义与 `tsconfig.json` 的 `compilerOptions` 接近。
    /// 生效：其中被 OXC 支持的选项真实影响转译结果。
    pub compiler_options: CompilerOptions,

    /// Import Maps 精确映射与 `/` 结尾前缀映射。生效。
    pub imports: BTreeMap<String, String>,

    /// Import Maps 作用域映射：键为引用者路径前缀。
    /// 预留：本版本解析但不参与解析决策。
    pub scopes: BTreeMap<String, BTreeMap<String, String>>,

    /// 参与构建的文件通配模式。生效（`mio build` 使用）。
    pub include: Vec<String>,

    /// 排除的文件通配模式。生效。
    pub exclude: Vec<String>,

    /// 显式列出的文件，优先于 `include` 通配。生效。
    pub files: Vec<String>,

    /// 运行时全局对象的注入面。
    /// 与 `compilerOptions.lib` 的区别：`lib` 是类型面，本字段是运行时面。
    pub stdlib: StdlibConfig,

    /// 模块解析细节。生效。
    pub module: ModuleConfig,

    /// 类型检查开关。生效。
    pub check: CheckConfig,

    /// 执行期行为。生效。
    pub run: RunConfig,

    /// 构建产物。生效（里程碑 3）。
    pub build: BuildConfig,

    /// mio 自身诊断输出。生效。
    pub log: LogConfig,

    /// 任务脚本：任务名 → 命令串。生效。
    pub tasks: BTreeMap<String, String>,

    /// 测试文件选择。预留：本版本解析但不执行。
    pub test: FileSelection,
}

/// `extends` 的取值：单个路径或路径数组。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(untagged)]
pub enum Extends {
    /// 单个配置路径。
    One(String),
    /// 多个配置路径，按顺序加载，后者覆盖前者。
    Many(Vec<String>),
}

impl Extends {
    /// 展开为路径列表，供加载器依次处理。
    pub fn as_paths(&self) -> Vec<String> {
        match self {
            Self::One(path) => vec![path.clone()],
            Self::Many(paths) => paths.clone(),
        }
    }
}

/// 运行时全局对象的注入面。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct StdlibConfig {
    /// Web 标准库版本。WHATWG 无版本号，`living` 是唯一有意义的值。
    /// 仅提示：本版本不校验取值。
    pub web: String,

    /// 需要从全局注入中剔除的对象名。生效。
    pub disable: Vec<String>,
}

impl Default for StdlibConfig {
    fn default() -> Self {
        Self {
            web: String::from("living"),
            disable: Vec::new(),
        }
    }
}

/// 模块解析细节。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct ModuleConfig {
    /// 绝对说明符与内建模块的基准目录。生效。
    pub root: String,

    /// 扩展名补全顺序。生效：改变本字段即改变解析行为。
    pub extensions: Vec<String>,
}

impl Default for ModuleConfig {
    fn default() -> Self {
        Self {
            root: String::from("."),
            extensions: vec![
                String::from(".ts"),
                String::from(".tsx"),
                String::from(".mts"),
                String::from(".js"),
                String::from(".mjs"),
                String::from(".json"),
            ],
        }
    }
}

/// 类型检查开关。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CheckConfig {
    /// `off` 表示不检查，`tsgo` 表示执行前调用外部 tsgo。生效。
    pub typecheck: String,

    /// tsgo 可执行文件路径，支持 `$ENV` 与环境变量引用。生效。
    pub tsgo_path: Option<String>,
}

impl Default for CheckConfig {
    fn default() -> Self {
        Self {
            typecheck: String::from("off"),
            tsgo_path: None,
        }
    }
}

/// 执行期行为。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct RunConfig {
    /// 事件循环超时毫秒数，防止 `await` 永久挂起。生效。
    pub timeout_ms: u64,
}

impl Default for RunConfig {
    fn default() -> Self {
        Self { timeout_ms: 30_000 }
    }
}

/// 构建产物配置。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct BuildConfig {
    /// 预转译产物目录。生效（里程碑 3）。
    pub out_dir: String,

    /// 是否落盘源码映射。生效（里程碑 3）。
    pub sourcemap: bool,
}

impl Default for BuildConfig {
    fn default() -> Self {
        Self {
            out_dir: String::from(".mio/cache"),
            sourcemap: true,
        }
    }
}

/// mio 自身诊断输出配置。注意与脚本里的 `console.log` 无关。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct LogConfig {
    /// 诊断级别：`silent` / `error` / `warn` / `info` / `debug`。生效。
    pub level: String,

    /// 是否输出结构化日志，便于 CI 解析。生效。
    pub json: bool,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: String::from("info"),
            json: false,
        }
    }
}

/// 文件选择规则，用于 `test` 等按范围筛选的命令。
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct FileSelection {
    /// 纳入范围的通配模式。
    pub include: Vec<String>,
    /// 排除的通配模式。
    pub exclude: Vec<String>,
}

impl MioConfig {
    /// 依据配置中的 Import Maps 构建模块解析器。
    ///
    /// 映射目标为 `npm:` / `node:` / 裸包名时返回错误：
    /// 这类目标需要包管理支持，而 mio 不提供包管理。
    pub fn build_resolver(&self) -> Result<ModuleResolver, String> {
        let mut resolver = ModuleResolver::default();

        for (from, to) in &self.imports {
            Self::validate_mapping(from, to)?;
            if from.ends_with('/') {
                resolver.insert_prefix(from.clone(), to.clone());
            } else {
                resolver.insert_exact(from.clone(), to.clone());
            }
        }

        Ok(resolver)
    }

    /// 校验一条 Import Maps 映射。
    fn validate_mapping(from: &str, to: &str) -> Result<(), String> {
        if from.ends_with('/') != to.ends_with('/') {
            return Err(format!(
                "imports 映射两端必须同时以 / 结尾或同时不以 / 结尾：{from} → {to}"
            ));
        }

        let rejected =
            to.starts_with("npm:") || to.starts_with("node:") || Self::is_bare_specifier(to);
        if rejected {
            return Err(format!(
                "imports 映射目标不受支持：{from} → {to}。mio 不提供包管理，请改为相对路径或 @std/*"
            ));
        }

        Ok(())
    }

    /// 判断一个说明符是否为裸包名（既非相对、也非绝对、也非 URL）。
    fn is_bare_specifier(specifier: &str) -> bool {
        !(specifier.starts_with("./")
            || specifier.starts_with("../")
            || specifier.starts_with('/')
            || specifier.starts_with("file:")
            || specifier.starts_with("@std/"))
    }

    /// 判断某个全局对象是否应被注入，依据 `stdlib.disable`。
    pub fn is_global_enabled(&self, name: &str) -> bool {
        !self.stdlib.disable.iter().any(|item| item == name)
    }

    /// 依据 `include` / `exclude` / `files` 判定某个相对路径是否属于配置范围。
    ///
    /// 判定顺序与 `tsconfig.json` 一致：`files` 显式命中优先；
    /// 其次 `exclude` 命中即排除；最后 `include` 命中即纳入。
    /// `include` 为空时视为匹配全部（排除规则仍然生效）。
    pub fn is_in_scope(&self, relative_path: &str) -> bool {
        if self.files.iter().any(|file| file == relative_path) {
            return true;
        }

        let excluded = self
            .exclude
            .iter()
            .filter_map(|pattern| Glob::compile(pattern).ok())
            .any(|glob| glob.matches(relative_path));
        if excluded {
            return false;
        }

        if self.include.is_empty() {
            return true;
        }

        self.include
            .iter()
            .filter_map(|pattern| Glob::compile(pattern).ok())
            .any(|glob| glob.matches(relative_path))
    }

    /// 解析配置目录下的相对路径，产出绝对路径。
    pub fn resolve_path(&self, base_dir: &Path, raw: &str) -> PathBuf {
        let candidate = PathBuf::from(raw);
        if candidate.is_absolute() {
            candidate
        } else {
            base_dir.join(candidate)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_has_expected_values() {
        let config = MioConfig::default();
        assert_eq!(config.stdlib.web, "living");
        assert_eq!(config.run.timeout_ms, 30_000);
        assert!(config.module.extensions.contains(&String::from(".ts")));
        assert_eq!(config.check.typecheck, "off");
    }

    #[test]
    fn globals_can_be_disabled() {
        let mut config = MioConfig::default();
        config.stdlib.disable.push(String::from("fetch"));
        assert!(!config.is_global_enabled("fetch"));
        assert!(config.is_global_enabled("URL"));
    }

    #[test]
    fn resolver_builds_exact_and_prefix_mappings() {
        let mut config = MioConfig::default();
        config
            .imports
            .insert(String::from("utils/tool"), String::from("./src/tool.ts"));
        config
            .imports
            .insert(String::from("@app/"), String::from("./src/"));

        let resolver = config.build_resolver().expect("映射应合法");
        assert_eq!(resolver.rewrite("utils/tool"), "./src/tool.ts");
        assert_eq!(resolver.rewrite("@app/main.ts"), "./src/main.ts");
    }

    #[test]
    fn resolver_rejects_npm_target() {
        let mut config = MioConfig::default();
        config
            .imports
            .insert(String::from("lodash"), String::from("npm:lodash@4"));

        let error = config.build_resolver().expect_err("应拒绝 npm 目标");
        assert!(error.contains("不提供包管理"));
    }

    #[test]
    fn resolver_rejects_mismatched_trailing_slash() {
        let mut config = MioConfig::default();
        config
            .imports
            .insert(String::from("@app/"), String::from("./src"));

        let error = config.build_resolver().expect_err("应拒绝斜杠不一致");
        assert!(error.contains("必须同时以 / 结尾"));
    }

    #[test]
    fn scope_follows_files_exclude_include_order() {
        let mut config = MioConfig::default();
        config.files.push(String::from("src/entry.ts"));
        config.exclude.push(String::from("dist/**"));
        config.include.push(String::from("src/**/*.ts"));

        assert!(config.is_in_scope("src/entry.ts"));
        assert!(config.is_in_scope("src/deep/mod.ts"));
        assert!(!config.is_in_scope("src/deep/mod.js"));
        assert!(!config.is_in_scope("dist/entry.ts"));
    }
}
