//! 命令分发。
//!
//! 每个子命令对应一个处理函数，分发本身只做 `match` 与错误收敛。
//! 除版本、帮助与任务列表外的命令返回「尚未实现」提示，
//! 后续步骤逐个替换为真实实现。

use crate::cli::CliResult;
use crate::cli::args::{Cli, Command};
use crate::config::LoadedConfig;
use crate::engine::{BoaEngine, Engine};
use crate::transpiler::{TranspileConfig, Transpiler};

/// 帮助文本。面向使用者描述可用子命令，不含内部实现细节。
const HELP_TEXT: &str = "\
mio —— TypeScript 一等公民的运行时

用法:
  mio run <file>        执行 TypeScript 或 JavaScript 文件
  mio eval <code>       执行一段内联源码
  mio repl              启动交互式求值环境
  mio check <file>      仅做类型检查（依赖外部 tsgo）
  mio task [name]       执行 mio.json 中声明的任务

选项:
  -c, --config <path>   指定配置文件路径
      --typecheck       本次执行前做类型检查
      --no-typecheck    本次执行前不做类型检查
  -v, --version         打印版本号
  -h, --help            打印本帮助
";

/// 执行一次命令行调用。
pub fn dispatch(cli: &Cli, config: &LoadedConfig) -> CliResult {
    match &cli.command {
        Command::Version => {
            println!("mio {}", crate::VERSION);
            Ok(())
        }
        Command::Help => {
            print!("{HELP_TEXT}");
            Ok(())
        }
        Command::Task { name } => run_task(config, name.as_deref()),
        Command::Run { path } => run_file(path, config),
        Command::Eval { source } => eval_source(source),
        Command::Repl => Err(not_implemented("repl")),
        Command::Check { path } => Err(not_implemented(&format!("check {}", path.display()))),
    }
}

/// 执行一个源码文件：读取 → 转译擦除类型 → 以 ES Module 语义求值。
///
/// `mio.json` 的 `imports` 与配置目录会组装为 [`ModuleResolver`]，传给引擎后
/// 内部的 `TsModuleLoader` 就能识别 Import Maps 别名与基准目录。
fn run_file(path: &std::path::Path, config: &LoadedConfig) -> CliResult {
    let resolver = config
        .config
        .build_resolver()?
        .with_base(config.root_dir.clone());
    BoaEngine::with_resolver(resolver)
        .eval_module_file(path)
        .map(|_| ())
        .map_err(|error| error.to_string())
}

/// 执行一段内联源码：先按 TypeScript 转译擦除类型，再以脚本求值并回显非空结果。
fn eval_source(source: &str) -> CliResult {
    let output = Transpiler::transpile(
        source,
        std::path::Path::new("eval.ts"),
        &TranspileConfig::default(),
    )
    .map_err(|diagnostics| diagnostics.join("\n"))?;
    let value = BoaEngine::new()
        .eval_script(&output.code)
        .map_err(|error| error.to_string())?;
    if !value.is_empty() {
        println!("{value}");
    }
    Ok(())
}

/// 处理 `mio task`：无参时列出全部任务，有参时查找任务。
///
/// 列出任务的输出按任务名排序，保证输出稳定，便于脚本与人工比对。
fn run_task(config: &LoadedConfig, name: Option<&str>) -> CliResult {
    match name {
        None => {
            if config.config.tasks.is_empty() {
                println!("未在配置中声明任何任务");
                return Ok(());
            }
            let mut names: Vec<&String> = config.config.tasks.keys().collect();
            names.sort();
            for task_name in names {
                let command = &config.config.tasks[task_name];
                println!("{task_name}\t{command}");
            }
            Ok(())
        }
        Some(task_name) => match config.config.tasks.get(task_name) {
            Some(command) => Err(format!(
                "任务 {task_name}（{command}）：任务执行功能尚未实现"
            )),
            None => Err(format!("配置中不存在任务 {task_name}")),
        },
    }
}

/// 统一构造「尚未实现」错误，避免各分支重复拼接文案。
fn not_implemented(what: &str) -> String {
    format!("{what}：该功能尚未实现")
}
