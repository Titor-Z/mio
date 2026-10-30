//! 配置加载：发现、继承、环境变量展开与优先级叠加。
//!
//! 加载管线分五段，顺序固定：
//! 1. 发现配置文件（显式指定，或从起始目录向上查找 `mio.json`）
//! 2. 沿 `extends` 链自底向上深合并 JSON 值（父配置在前，子配置覆盖）
//! 3. 展开字符串中的 `$ENV` / `${ENV}` 引用
//! 4. 叠加 `MIO_*` 环境变量
//! 5. 叠加命令行覆盖项
//!
//! 合并发生在 JSON 值层面而非结构体层面：结构体无法区分「显式写成默认值」
//! 与「没有写」，只有值层面合并才能正确表达继承。

use std::path::{Path, PathBuf};

use serde_json::Value;

use super::diagnostics::{self, Diagnostic};
use super::{ConfigOverride, MioConfig};

/// 配置文件的候选文件名。
pub const CONFIG_FILE_NAME: &str = "mio.json";

/// `extends` 链的最大深度，用于防止意外的深层继承。
const MAX_EXTENDS_DEPTH: usize = 16;

/// 环境变量读取抽象。
///
/// 之所以不直接调用 `std::env::var`，是为了让测试可以注入受控环境，
/// 不依赖进程真实环境变量，避免测试之间互相污染。
pub trait Environment {
    /// 读取一个环境变量。
    fn get(&self, key: &str) -> Option<String>;
}

/// 读取进程真实环境变量。
#[derive(Debug, Clone, Copy, Default)]
pub struct ProcessEnvironment;

impl Environment for ProcessEnvironment {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }
}

/// 测试与嵌入式场景使用的内存环境变量表。
#[derive(Debug, Clone, Default)]
pub struct MapEnvironment {
    /// 变量名到变量值的映射。
    entries: rustc_hash::FxHashMap<String, String>,
}

impl MapEnvironment {
    /// 注册一个变量。
    pub fn set(&mut self, key: impl Into<String>, value: impl Into<String>) {
        self.entries.insert(key.into(), value.into());
    }
}

impl Environment for MapEnvironment {
    fn get(&self, key: &str) -> Option<String> {
        self.entries.get(key).cloned()
    }
}

/// 一次加载的完整结果。
#[derive(Debug, Clone, Default)]
pub struct LoadedConfig {
    /// 叠加完成的最终配置。
    pub config: MioConfig,

    /// 最终配置所属目录，相对路径字段以它为基准解析。
    pub root_dir: PathBuf,

    /// 实际使用的配置文件路径；使用内建默认时为 `None`。
    pub source_path: Option<PathBuf>,

    /// 加载过程中产出的诊断。
    pub diagnostics: Vec<Diagnostic>,
}

impl LoadedConfig {
    /// 取出告警文本，供调用方打印。
    pub fn warnings(&self) -> Vec<String> {
        self.diagnostics.iter().map(Diagnostic::render).collect()
    }
}

/// 配置加载器。
#[derive(Debug, Clone, Copy, Default)]
pub struct ConfigLoader;

impl ConfigLoader {
    /// 加载配置。
    ///
    /// `start_dir` 为查找起点（通常是入口文件所在目录）；
    /// `explicit` 为 `--config` 显式指定的路径，优先于自动发现。
    pub fn load(
        start_dir: &Path,
        explicit: Option<&Path>,
        overrides: &ConfigOverride,
        environment: &dyn Environment,
    ) -> Result<LoadedConfig, String> {
        let source_path = match explicit {
            Some(path) => Some(path.to_path_buf()),
            None => Self::discover(start_dir),
        };

        let Some(path) = source_path else {
            let mut loaded = LoadedConfig {
                root_dir: start_dir.to_path_buf(),
                ..LoadedConfig::default()
            };
            Self::apply_environment(&mut loaded.config, environment);
            Self::apply_overrides(&mut loaded.config, overrides);
            return Ok(loaded);
        };

        let mut visited = Vec::new();
        let merged = Self::load_chain(&path, environment, &mut visited, 0)?;

        let mut diagnostics = diagnostics::check_top_level(&merged);
        diagnostics.extend(diagnostics::check_compiler_options(&merged));

        let config: MioConfig = serde_json::from_value(merged)
            .map_err(|error| format!("配置文件 {} 结构不合法：{error}", path.display()))?;

        let mut loaded = LoadedConfig {
            config,
            root_dir: path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| PathBuf::from(".")),
            source_path: Some(path),
            diagnostics,
        };

        Self::collect_unsupported(&mut loaded.diagnostics, &loaded.config);
        Self::apply_environment(&mut loaded.config, environment);
        Self::apply_overrides(&mut loaded.config, overrides);

        Ok(loaded)
    }

    /// 从起始目录向上查找最近的配置文件。
    ///
    /// 与 `tsconfig.json` 的行为一致：找到即止，不继续向上。
    pub fn discover(start_dir: &Path) -> Option<PathBuf> {
        let mut current = start_dir
            .canonicalize()
            .unwrap_or_else(|_| start_dir.to_path_buf());

        loop {
            let candidate = current.join(CONFIG_FILE_NAME);
            if candidate.is_file() {
                return Some(candidate);
            }
            if !current.pop() {
                return None;
            }
        }
    }

    /// 递归加载一份配置文件及其 `extends` 链，返回合并后的 JSON 值。
    fn load_chain(
        path: &Path,
        environment: &dyn Environment,
        visited: &mut Vec<PathBuf>,
        depth: usize,
    ) -> Result<Value, String> {
        if depth > MAX_EXTENDS_DEPTH {
            return Err(format!(
                "extends 链超过 {MAX_EXTENDS_DEPTH} 层，可能是循环继承"
            ));
        }

        let key = path.canonicalize().unwrap_or_else(|_| path.to_path_buf());
        if visited.contains(&key) {
            return Err(format!("extends 出现循环继承：{}", path.display()));
        }
        visited.push(key);

        let raw = std::fs::read_to_string(path)
            .map_err(|error| format!("无法读取配置文件 {}：{error}", path.display()))?;
        let stripped = super::jsonc::strip(&raw);
        let mut value: Value = serde_json::from_str(&stripped)
            .map_err(|error| format!("配置文件 {} 解析失败：{error}", path.display()))?;

        let base_dir = path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));

        let parents = value
            .get("extends")
            .and_then(Self::extends_paths)
            .unwrap_or_default();

        let mut merged = Value::Object(serde_json::Map::new());
        for parent in parents {
            let parent_path = Self::resolve_extends(&base_dir, &parent);
            let parent_value = Self::load_chain(&parent_path, environment, visited, depth + 1)?;
            merged = Self::deep_merge(merged, parent_value);
        }

        // `extends` 本身不参与最终配置
        if let Some(object) = value.as_object_mut() {
            object.remove("extends");
        }

        let mut combined = Self::deep_merge(merged, value);
        Self::expand_env(&mut combined, environment);
        Ok(combined)
    }

    /// 取出 `extends` 的路径列表。
    fn extends_paths(value: &Value) -> Option<Vec<String>> {
        match value {
            Value::String(single) => Some(vec![single.clone()]),
            Value::Array(items) => Some(
                items
                    .iter()
                    .filter_map(Value::as_str)
                    .map(String::from)
                    .collect(),
            ),
            _ => None,
        }
    }

    /// 把 `extends` 中的路径解析为绝对路径。
    fn resolve_extends(base_dir: &Path, raw: &str) -> PathBuf {
        let candidate = base_dir.join(raw);
        if candidate.is_file() {
            return candidate;
        }
        // 允许省略 `.json` 后缀
        let with_suffix = candidate.with_extension("json");
        if with_suffix.is_file() {
            return with_suffix;
        }
        candidate
    }

    /// JSON 值深合并：对象逐字段合并，其余类型由子配置整体覆盖。
    fn deep_merge(base: Value, overlay: Value) -> Value {
        match (base, overlay) {
            (Value::Object(mut left), Value::Object(right)) => {
                for (key, value) in right {
                    let merged = match left.remove(&key) {
                        Some(existing) => Self::deep_merge(existing, value),
                        None => value,
                    };
                    left.insert(key, merged);
                }
                Value::Object(left)
            }
            (_, overlay) => overlay,
        }
    }

    /// 递归展开字符串中的 `$VAR` 与 `${VAR}` 引用。
    ///
    /// 未定义的变量原样保留，避免静默产出空串导致路径指向意外位置。
    fn expand_env(value: &mut Value, environment: &dyn Environment) {
        match value {
            Value::String(text) => *text = Self::expand_string(text, environment),
            Value::Array(items) => {
                for item in items {
                    Self::expand_env(item, environment);
                }
            }
            Value::Object(map) => {
                for item in map.values_mut() {
                    Self::expand_env(item, environment);
                }
            }
            _ => {}
        }
    }

    /// 展开单个字符串中的环境变量引用。
    fn expand_string(text: &str, environment: &dyn Environment) -> String {
        let chars: Vec<char> = text.chars().collect();
        let mut output = String::with_capacity(text.len());
        let mut index = 0;

        while index < chars.len() {
            if chars[index] != '$' {
                output.push(chars[index]);
                index += 1;
                continue;
            }

            if chars.get(index + 1) == Some(&'{') {
                let close = chars[index + 2..].iter().position(|item| *item == '}');
                let Some(offset) = close else {
                    output.push('$');
                    index += 1;
                    continue;
                };
                let name: String = chars[index + 2..index + 2 + offset].iter().collect();
                match environment.get(&name) {
                    Some(resolved) => output.push_str(&resolved),
                    None => output.push_str(&format!("${{{name}}}")),
                }
                index += 2 + offset + 1;
                continue;
            }

            let start = index + 1;
            let mut end = start;
            while end < chars.len() && (chars[end].is_ascii_alphanumeric() || chars[end] == '_') {
                end += 1;
            }

            if end == start {
                output.push('$');
                index += 1;
                continue;
            }

            let name: String = chars[start..end].iter().collect();
            match environment.get(&name) {
                Some(resolved) => output.push_str(&resolved),
                None => output.push_str(&format!("${name}")),
            }
            index = end;
        }

        output
    }

    /// 叠加 `MIO_*` 环境变量。
    fn apply_environment(config: &mut MioConfig, environment: &dyn Environment) {
        if let Some(value) = environment.get("MIO_TYPECHECK") {
            config.check.typecheck = value;
        }
        if let Some(value) = environment
            .get("MIO_TIMEOUT_MS")
            .and_then(|raw| raw.parse::<u64>().ok())
        {
            config.run.timeout_ms = value;
        }
        if let Some(value) = environment.get("MIO_LOG_LEVEL") {
            config.log.level = value;
        }
        if let Some(value) = environment.get("MIO_LOG_JSON") {
            config.log.json = matches!(value.as_str(), "1" | "true" | "yes");
        }
        if let Some(value) = environment.get("MIO_STDLIB_WEB") {
            config.stdlib.web = value;
        }
    }

    /// 叠加命令行覆盖项，优先级最高。
    fn apply_overrides(config: &mut MioConfig, overrides: &ConfigOverride) {
        if let Some(value) = &overrides.typecheck {
            config.check.typecheck = value.clone();
        }
        if let Some(value) = overrides.timeout_ms {
            config.run.timeout_ms = value;
        }
        if let Some(value) = &overrides.log_level {
            config.log.level = value.clone();
        }
    }

    /// 汇总「已解析但本版本不采用」的字段告警。
    fn collect_unsupported(diagnostics: &mut Vec<Diagnostic>, config: &MioConfig) {
        for field in config.compiler_options.unsupported_fields() {
            let message = if field.ends_with("paths") || field.ends_with("baseUrl") {
                "本版本不采用该字段：mio 的模块映射发生在运行期，请改用顶层 imports / scopes"
            } else {
                "本版本不采用该字段，将被忽略"
            };
            diagnostics.push(Diagnostic::warn(field, message));
        }

        if !config.scopes.is_empty() {
            diagnostics.push(Diagnostic::warn(
                "scopes",
                "作用域映射本版本解析但不参与解析决策",
            ));
        }

        if !config.stdlib.web.eq_ignore_ascii_case("living") {
            diagnostics.push(Diagnostic::warn(
                "stdlib.web",
                "WHATWG 标准无版本号，仅 living 有实际意义",
            ));
        }

        if !config.test.include.is_empty() || !config.test.exclude.is_empty() {
            diagnostics.push(Diagnostic::warn("test", "测试文件选择本版本解析但不执行"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 在临时目录中创建一份配置文件，返回目录路径。
    fn write_config(dir_name: &str, file_name: &str, content: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("mio_config_{dir_name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能创建临时目录");
        std::fs::write(dir.join(file_name), content).expect("应能写入配置文件");
        dir
    }

    #[test]
    fn loads_jsonc_config_with_comments() {
        let dir = write_config(
            "jsonc",
            CONFIG_FILE_NAME,
            r#"{
              // 目标版本
              "compilerOptions": { "target": "es2023" },
              "run": { "timeoutMs": 1000 },
            }"#,
        );

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        assert_eq!(loaded.config.compiler_options.target, "es2023");
        assert_eq!(loaded.config.run.timeout_ms, 1000);
        assert!(loaded.source_path.is_some());
    }

    #[test]
    fn falls_back_to_defaults_without_config_file() {
        let dir = std::env::temp_dir().join("mio_config_absent");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("应能创建临时目录");

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        assert_eq!(loaded.config.run.timeout_ms, 30_000);
        assert!(loaded.source_path.is_none());
    }

    #[test]
    fn extends_chain_merges_parent_then_child() {
        let dir = write_config(
            "extends",
            "base.json",
            r#"{ "compilerOptions": { "target": "es2022", "jsx": "react" } }"#,
        );
        std::fs::write(
            dir.join(CONFIG_FILE_NAME),
            r#"{ "extends": "./base.json", "compilerOptions": { "target": "es2025" } }"#,
        )
        .expect("应能写入子配置");

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        // 子配置覆盖 target，父配置的 jsx 被继承
        assert_eq!(loaded.config.compiler_options.target, "es2025");
        assert_eq!(loaded.config.compiler_options.jsx, "react");
    }

    #[test]
    fn extends_detects_cycle() {
        let dir = write_config("cycle", "a.json", r#"{ "extends": "./b.json" }"#);
        std::fs::write(dir.join("b.json"), r#"{ "extends": "./a.json" }"#)
            .expect("应能写入循环配置");

        let error = ConfigLoader::load(
            &dir,
            Some(&dir.join("a.json")),
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect_err("应检出循环继承");

        assert!(error.contains("循环继承"));
    }

    #[test]
    fn expands_braced_and_bare_env_references() {
        let mut environment = MapEnvironment::default();
        environment.set("MIO_OUT", "/tmp/out");

        let dir = write_config(
            "env",
            CONFIG_FILE_NAME,
            r#"{ "build": { "outDir": "${MIO_OUT}/dist" } }"#,
        );

        let loaded = ConfigLoader::load(&dir, None, &ConfigOverride::default(), &environment)
            .expect("应加载成功");

        assert_eq!(loaded.config.build.out_dir, "/tmp/out/dist");
    }

    #[test]
    fn keeps_undefined_env_reference_intact() {
        let dir = write_config(
            "env_missing",
            CONFIG_FILE_NAME,
            r#"{ "build": { "outDir": "$MIO_ABSENT" } }"#,
        );

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        assert_eq!(loaded.config.build.out_dir, "$MIO_ABSENT");
    }

    #[test]
    fn cli_override_beats_environment_and_file() {
        let mut environment = MapEnvironment::default();
        environment.set("MIO_TYPECHECK", "on");

        let dir = write_config(
            "priority",
            CONFIG_FILE_NAME,
            r#"{ "check": { "typecheck": "off" } }"#,
        );

        let overrides = ConfigOverride {
            typecheck: Some(String::from("off")),
            timeout_ms: Some(5),
            log_level: None,
        };

        let loaded = ConfigLoader::load(&dir, None, &overrides, &environment).expect("应加载成功");
        assert_eq!(loaded.config.check.typecheck, "off");
        assert_eq!(loaded.config.run.timeout_ms, 5);
    }

    #[test]
    fn warns_on_unknown_field_with_suggestion() {
        let dir = write_config(
            "unknown",
            CONFIG_FILE_NAME,
            r#"{ "compilerOptions": { "targett": "es2025" } }"#,
        );

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        assert!(
            loaded
                .warnings()
                .iter()
                .any(|line| line.contains("compilerOptions.targett")),
            "应提示未知字段，实际：{:?}",
            loaded.warnings()
        );
    }

    #[test]
    fn warns_on_tsconfig_paths_field() {
        let dir = write_config(
            "paths",
            CONFIG_FILE_NAME,
            r#"{ "compilerOptions": { "paths": { "@app/*": ["./src/*"] } } }"#,
        );

        let loaded = ConfigLoader::load(
            &dir,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");

        assert!(
            loaded
                .warnings()
                .iter()
                .any(|line| line.contains("imports"))
        );
    }

    #[test]
    fn discovers_config_in_parent_directory() {
        let dir = write_config(
            "discover",
            CONFIG_FILE_NAME,
            r#"{ "run": { "timeoutMs": 7 } }"#,
        );
        let nested = dir.join("src").join("deep");
        std::fs::create_dir_all(&nested).expect("应能创建子目录");

        let discovered = ConfigLoader::discover(&nested).expect("应能找到配置");
        assert_eq!(
            discovered.file_name().and_then(|n| n.to_str()),
            Some(CONFIG_FILE_NAME)
        );

        let loaded = ConfigLoader::load(
            &nested,
            None,
            &ConfigOverride::default(),
            &MapEnvironment::default(),
        )
        .expect("应加载成功");
        assert_eq!(loaded.config.run.timeout_ms, 7);
    }
}
