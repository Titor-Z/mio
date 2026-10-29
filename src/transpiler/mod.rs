//! 转译层：TypeScript 源码 → JavaScript 源码。
//!
//! 阶段 A 由 OXC 承担，封装为 [`Transpiler`] 结构体。
//! 阶段 B 将替换为自研前端，但对外接口保持「源码进、源码出」。
//!
//! 依赖约束：`oxc_*` 只允许出现在本模块内部。
//!
//! 管线四段：`Parser` 产出 AST → `SemanticBuilder` 建作用域（开启枚举求值）
//! → `Transformer` 依据作用域擦除类型并降级 → `Codegen` 出文本与源码映射。
//! 各段的诊断在此聚合为一个字符串列表向上返回，避免调用方各自处理三套错误类型。

use std::path::Path;

use oxc_allocator::Allocator;
use oxc_codegen::{Codegen, CodegenOptions};
use oxc_parser::Parser;
use oxc_semantic::SemanticBuilder;
use oxc_span::SourceType;
use oxc_transformer::{TransformOptions, Transformer};

/// 转译产出：擦除类型后的 JavaScript 源码 + 可选的源码映射。
#[derive(Debug, Clone)]
pub struct TranspileOutput {
    /// 擦除类型后的 JavaScript 源码。
    pub code: String,
    /// 源码映射原文，用于把运行时错误位置映射回 TypeScript 行列。
    pub sourcemap: Option<String>,
}

/// 转译选项。字段由上层从配置（`compilerOptions`）填入，本模块不反向依赖配置层。
#[derive(Debug, Clone, Default)]
pub struct TranspileConfig {
    /// 语法降级目标，如 `es2025`；`None` 表示不做额外的语法降级（仅擦除类型）。
    pub target: Option<String>,
    /// 是否产出源码映射。开启后 [`TranspileOutput::sourcemap`] 携带 JSON 文本。
    pub sourcemap: bool,
}

/// TypeScript → JavaScript 转译器。无状态，可被多个模块复用。
#[derive(Debug, Default)]
pub struct Transpiler;

impl Transpiler {
    /// 转译一份源码。`path` 用于判定 `SourceType`（扩展名）与源码映射的文件名。
    ///
    /// 返回 `Err(diagnostics)` 表示存在语法或转译诊断；诊断非空时不产出代码，
    /// 以免把半成品交给执行层。
    pub fn transpile(
        source: &str,
        path: &Path,
        config: &TranspileConfig,
    ) -> Result<TranspileOutput, Vec<String>> {
        let allocator = Allocator::default();
        let source_type = Self::source_type(path);

        let parsed = Parser::new(&allocator, source, source_type).parse();
        let mut diagnostics: Vec<String> = parsed
            .diagnostics
            .iter()
            .map(|item| item.to_string())
            .collect();
        let mut program = parsed.program;

        // 开启枚举求值，否则 `enum` 成员取值会降级错误（OXC 需显式打开）。
        let scoping = SemanticBuilder::new()
            .with_enum_eval(true)
            .build(&program)
            .semantic
            .into_scoping();

        let options = Self::transform_options(config);
        let transformed =
            Transformer::new(&allocator, path, &options).build_with_scoping(scoping, &mut program);
        diagnostics.extend(transformed.diagnostics.iter().map(|item| item.to_string()));

        if !diagnostics.is_empty() {
            return Err(diagnostics);
        }

        let mut codegen_options = CodegenOptions::default();
        if config.sourcemap {
            codegen_options.source_map_path = Some(path.to_path_buf());
        }
        let emitted = Codegen::new()
            .with_source_text(source)
            .with_options(codegen_options)
            .build(&program);

        Ok(TranspileOutput {
            code: emitted.code,
            sourcemap: emitted.map.map(|map| map.to_json_string()),
        })
    }

    /// 由文件扩展名推断源码类型；未知扩展名退回「ES Module 的 TypeScript」。
    fn source_type(path: &Path) -> SourceType {
        SourceType::from_path(path).unwrap_or_else(|_| {
            SourceType::default()
                .with_module(true)
                .with_typescript(true)
        })
    }

    /// 组装 OXC 转译选项。默认即执行 TypeScript 类型擦除；
    /// 仅在指定了目标版本时追加语法降级（如更低的 `target`）。
    fn transform_options(config: &TranspileConfig) -> TransformOptions {
        match config.target.as_deref().filter(|value| !value.is_empty()) {
            Some(target) => TransformOptions::from_target(target).unwrap_or_default(),
            None => TransformOptions::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在内存里转译一段源码，返回代码文本；诊断非空则 panic 抛出诊断。
    fn code_of(source: &str) -> String {
        let output =
            Transpiler::transpile(source, Path::new("mem.ts"), &TranspileConfig::default())
                .unwrap_or_else(|diagnostics| panic!("转译诊断：{diagnostics:?}"));
        output.code
    }

    #[test]
    fn erases_interface_declaration() {
        let code =
            code_of("interface Point { x: number; y: number }\nconst p = 1;\nexport { p };\n");
        assert!(
            !code.contains("interface"),
            "interface 应被擦除，实际：{code}"
        );
        assert!(!code.contains("Point"), "仅类型声明不落地为运行时代码");
        assert!(code.contains("const p = 1"), "值声明应保留：{code}");
    }

    #[test]
    fn erases_type_annotations() {
        let code = code_of("function add(a: number, b: number): number { return a + b; }\n");
        assert!(!code.contains(": number"), "类型注解应被擦除，实际：{code}");
        assert!(code.contains("function add"), "函数本体保留：{code}");
    }

    #[test]
    fn lowers_enum_with_evaluated_members() {
        let code = code_of("enum Direction { Up = 1, Down = 2 }\n");
        assert!(code.contains("Direction"), "枚举名作为运行时值保留：{code}");
        // enum_eval 开启后成员取具体数值，而非被整体丢弃
        assert!(
            code.contains('1') && code.contains('2'),
            "枚举成员值应落地：{code}"
        );
    }

    #[test]
    fn reports_diagnostics_on_syntax_error() {
        let result = Transpiler::transpile(
            "const = ;",
            Path::new("bad.ts"),
            &TranspileConfig::default(),
        );
        assert!(result.is_err(), "非法语法应返回诊断而非半成品代码");
    }

    #[test]
    fn emits_sourcemap_when_requested() {
        let config = TranspileConfig {
            sourcemap: true,
            ..TranspileConfig::default()
        };
        let output = Transpiler::transpile(
            "const a: number = 1;\nexport { a };\n",
            Path::new("mem.ts"),
            &config,
        )
        .expect("应转译成功");
        let map = output.sourcemap.expect("开启后应产出源码映射");
        assert!(map.contains("mappings"), "源码映射应含映射表：{map}");
    }
}
