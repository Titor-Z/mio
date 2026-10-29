//! 命令行参数解析。
//!
//! 手写解析而非依赖第三方库：mio 只有五个子命令，参数形态稳定；
//! 且解析结果需要与 `mio.json` 做优先级叠加（命令行高于配置文件），
//! 手写可以精确控制覆盖顺序。

use std::path::PathBuf;

use crate::config::ConfigOverride;

/// 顶层命令集合。每个变体对应一个用户可见子命令。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// 执行一个源文件。
    Run { path: PathBuf },

    /// 执行一段内联源码。
    Eval { source: String },

    /// 交互式求值环境。
    Repl,

    /// 仅做类型检查，不执行。
    Check { path: PathBuf },

    /// 执行配置文件中声明的任务；`name` 为 `None` 时列出全部任务。
    Task { name: Option<String> },

    /// 打印版本文本。
    Version,

    /// 打印帮助文本。
    Help,
}

/// 一次完整的命令行调用：命令本体 + 需要覆盖配置的选项。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Cli {
    /// 具体要执行的命令。
    pub command: Command,

    /// 显式指定的配置文件路径，对应 `--config <path>`。
    pub config_path: Option<PathBuf>,

    /// 需要叠加到配置上的命令行覆盖项。
    pub overrides: ConfigOverride,
}

impl Cli {
    /// 从进程参数解析出 [`Cli`]。
    ///
    /// `args` 不含可执行文件名本身，即 `std::env::args().skip(1)` 的结果。
    pub fn parse<I, S>(args: I) -> Result<Self, String>
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        let words: Vec<String> = args.into_iter().map(Into::into).collect();
        let mut positional: Vec<String> = Vec::new();
        let mut config_path: Option<PathBuf> = None;
        let mut overrides = ConfigOverride::default();
        let mut early: Option<Command> = None;

        let mut cursor = 0;
        while cursor < words.len() {
            let word = words[cursor].as_str();
            match word {
                "--version" | "-v" => early = Some(Command::Version),
                "--help" | "-h" => early = Some(Command::Help),
                "--typecheck" => overrides.typecheck = Some(String::from("tsgo")),
                "--no-typecheck" => overrides.typecheck = Some(String::from("off")),
                "--config" | "-c" => {
                    let value = words
                        .get(cursor + 1)
                        .ok_or_else(|| String::from("选项 --config 需要一个路径参数"))?;
                    config_path = Some(PathBuf::from(value));
                    cursor += 1;
                }
                flag if flag.starts_with('-') => {
                    return Err(format!("未知选项：{flag}，可执行 mio --help 查看用法"));
                }
                value => positional.push(value.to_string()),
            }
            cursor += 1;
        }

        let command = early.unwrap_or(Self::resolve_command(&positional)?);
        Ok(Self {
            command,
            config_path,
            overrides,
        })
    }

    /// 依据位置参数判定子命令。
    ///
    /// 使用切片模式匹配而非 `if/else` 链，保证每一种合法形态与非法形态都显式列出。
    fn resolve_command(positional: &[String]) -> Result<Command, String> {
        let words: Vec<&str> = positional.iter().map(String::as_str).collect();
        match words.as_slice() {
            [] => Ok(Command::Help),
            ["run", path] => Ok(Command::Run {
                path: PathBuf::from(path),
            }),
            ["eval", source] => Ok(Command::Eval {
                source: (*source).to_string(),
            }),
            ["repl"] => Ok(Command::Repl),
            ["check", path] => Ok(Command::Check {
                path: PathBuf::from(path),
            }),
            ["task"] => Ok(Command::Task { name: None }),
            ["task", name] => Ok(Command::Task {
                name: Some((*name).to_string()),
            }),
            ["run"] => Err(String::from("run 子命令需要一个文件路径")),
            ["eval"] => Err(String::from("eval 子命令需要一段源码")),
            ["check"] => Err(String::from("check 子命令需要一个文件路径")),
            [unknown, ..] => Err(format!("未知子命令：{unknown}，可执行 mio --help 查看用法")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_run_command() {
        let cli = Cli::parse(["run", "app.ts"]).expect("应解析成功");
        assert_eq!(
            cli.command,
            Command::Run {
                path: PathBuf::from("app.ts")
            }
        );
    }

    #[test]
    fn parses_version_flag() {
        let cli = Cli::parse(["--version"]).expect("应解析成功");
        assert_eq!(cli.command, Command::Version);
    }

    #[test]
    fn parses_config_option() {
        let cli = Cli::parse(["--config", "cfg/mio.json", "run", "app.ts"]).expect("应解析成功");
        assert_eq!(cli.config_path, Some(PathBuf::from("cfg/mio.json")));
        assert_eq!(
            cli.command,
            Command::Run {
                path: PathBuf::from("app.ts")
            }
        );
    }

    #[test]
    fn rejects_unknown_flag() {
        let err = Cli::parse(["--nope"]).expect_err("应报错");
        assert!(err.contains("未知选项"));
    }

    #[test]
    fn rejects_missing_run_path() {
        let err = Cli::parse(["run"]).expect_err("应报错");
        assert!(err.contains("需要一个文件路径"));
    }
}
