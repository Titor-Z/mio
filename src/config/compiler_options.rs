//! 编译器开关。
//!
//! 命名与取值风格对齐 `tsconfig.json` 的 `compilerOptions`，因为这是 TypeScript
//! 使用者的既有习惯。但只保留两类字段：
//!
//! 1. 本版本真实生效的选项（会被传给 OXC 的转译管线）；
//! 2. `tsconfig.json` 中常见但本项目不采用的选项，声明为「仅提示」，
//!    以便给出精确告警而不是笼统的未知字段提示。
//!
//! 不采用 `paths` 与 `baseUrl`：它们是编译期概念，不影响产物且需要打包器配合；
//! mio 的模块映射发生在运行期，由顶层 `imports` / `scopes`（Import Maps）承担。

use serde::Deserialize;

/// 编译器开关集合。
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CompilerOptions {
    /// 输出目标版本，例如 `es2025`。生效：传给 OXC 的转换目标。
    pub target: String,

    /// 模块形态。仅提示：mio 只有 ES Module 一种形态，取值不被校验。
    pub module: String,

    /// 模块解析策略。仅提示：mio 固定使用自身解析规则。
    pub module_resolution: String,

    /// JSX 处理方式：`preserve` / `react` / `react-jsx`。生效。
    pub jsx: String,

    /// JSX 元素工厂名，仅在 `jsx` 为 `react` 时有意义。生效。
    pub jsx_factory: String,

    /// 类字段是否使用定义语义。生效：传给 OXC 的类字段转换。
    pub use_define_for_class_fields: bool,

    /// 类型声明库列表，例如 `es2025`、`web`。
    /// 仅提示：本版本由 tsrs 默认库决定，不传递该列表给编译器。
    pub lib: Vec<String>,

    /// 类型严格度开关。生效：`true` 时向内置 tsrs 类型检查器传入 `--strict`。
    pub strict: bool,

    /// 是否产出源码映射。生效：运行期错误栈帧的行列据此反查回 TypeScript 源坐标。
    /// 关闭时注册表为空，错误输出退回转译后 JS 坐标。
    pub source_map: bool,

    /// TypeScript `paths` 映射。不采用：命中时给出明确告警并指向 `imports`。
    pub paths: Option<serde_json::Value>,

    /// TypeScript `baseUrl`。不采用：命中时给出明确告警并指向 `imports`。
    pub base_url: Option<String>,
}

impl Default for CompilerOptions {
    fn default() -> Self {
        Self {
            target: String::from("es2025"),
            module: String::from("esnext"),
            module_resolution: String::from("mio"),
            jsx: String::from("preserve"),
            jsx_factory: String::from("React.createElement"),
            use_define_for_class_fields: true,
            lib: vec![String::from("es2025"), String::from("web")],
            strict: false,
            source_map: true,
            paths: None,
            base_url: None,
        }
    }
}

impl CompilerOptions {
    /// 判定 JSX 是否需要由转译层处理。
    ///
    /// `preserve` 表示保留 JSX 原样交给下游，其余取值需要 OXC 改写为函数调用。
    pub fn lower_jsx(&self) -> bool {
        matches!(self.jsx.as_str(), "react" | "react-jsx" | "react-jsxdev")
    }

    /// 列出本版本不采用、但已被解析的字段名，用于统一告警。
    ///
    /// 把「不采用」集中在此处而非散落在加载器里，
    /// 是为了让将来启用某个选项时只改一个地方。
    pub fn unsupported_fields(&self) -> Vec<&'static str> {
        let mut fields = Vec::new();
        if self.paths.is_some() {
            fields.push("compilerOptions.paths");
        }
        if self.base_url.is_some() {
            fields.push("compilerOptions.baseUrl");
        }
        fields
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_tsconfig_style() {
        let options = CompilerOptions::default();
        assert_eq!(options.target, "es2025");
        assert_eq!(options.jsx, "preserve");
        assert!(options.use_define_for_class_fields);
        assert!(options.source_map);
    }

    #[test]
    fn jsx_preserve_does_not_lower() {
        let options = CompilerOptions::default();
        assert!(!options.lower_jsx());
    }

    #[test]
    fn jsx_react_lowers() {
        let options = CompilerOptions {
            jsx: String::from("react-jsx"),
            ..CompilerOptions::default()
        };
        assert!(options.lower_jsx());
    }

    #[test]
    fn unsupported_fields_reports_paths_and_base_url() {
        let options = CompilerOptions {
            paths: Some(serde_json::json!({})),
            base_url: Some(String::from(".")),
            ..CompilerOptions::default()
        };
        assert_eq!(
            options.unsupported_fields(),
            vec!["compilerOptions.paths", "compilerOptions.baseUrl"]
        );
    }
}
