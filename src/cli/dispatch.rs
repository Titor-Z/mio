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
  mio check <file>      仅做类型检查（内置 tsrs 编译器，无外部依赖）
  mio task [name]       执行 mio.json 中声明的任务
  mio init              在当前目录创建项目骨架与类型声明
  mio types             刷新 .mio/@types/ 下的运行时类型声明
  mio docs [host:port]  启动内置手册服务（默认 127.0.0.1:4173）

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
        Command::Init => super::scaffold::init_current_dir(),
        Command::Types => super::scaffold::refresh_types_current_dir(),
        Command::Docs { addr } => crate::docs::serve(addr.as_deref()),
        Command::Run { path } => run_file(path, config),
        Command::Eval { source } => eval_source(source),
        Command::Repl => super::repl::run(),
        Command::Check { path } => check_file(path, config),
    }
}

/// 执行一次纯类型检查：复用 fork 进来的 tsrs 编译器驱动 `tsrs_execute::command_line`。
///
/// 参数对齐 mio 的运行语义：每个文件都按 ES Module 检查（`moduleDetection force` +
/// `target/module esnext`），顶层 await 因此合法；`compilerOptions.strict` 附加 `--strict`。
/// 内置声明（`types/mio/`）落到临时目录后作为额外根源文件传入，并用 `--lib esnext`
/// 关掉 tsrs 默认的 DOM lib，避免与 mio 自己的 `URL`/`Response` 等声明冲突。
/// 诊断由编译器自身按 `file:line:col error TSxxxx` 格式写到 stdout/stderr；
/// 本函数只负责选择参数、在足够大的栈线程里跑（类型检查递归很深）、把退出码翻译成结果。
fn check_file(path: &std::path::Path, config: &LoadedConfig) -> CliResult {
    use tsrs_execute::{execute, sys, tsc};

    let strict = config.config.compiler_options.strict;
    let path = path.to_path_buf();
    let builtin_types = super::scaffold::materialize_builtin_types()?;
    let status = std::thread::Builder::new()
        .stack_size(512 << 20)
        .spawn(move || {
            let host: &'static dyn tsc::System = Box::leak(Box::new(sys::new_system()));
            let mut args = vec![
                String::from("--noEmit"),
                // 项目里的 tsconfig.json 属于编辑器配置；mio check 只按运行语义检查入口文件，
                // 不加载它（同时避开 tsgo 对「有 tsconfig 又显式给文件」的报错）。
                String::from("--ignoreConfig"),
                // 与 mio 运行时的模块语义对齐：每个文件都是 ES Module（顶层 await 因此合法）。
                String::from("--moduleDetection"),
                String::from("force"),
                String::from("--target"),
                String::from("esnext"),
                String::from("--module"),
                String::from("esnext"),
                // mio 的全局面不是 DOM：只加载 ES lib，其余由内置声明提供。
                String::from("--lib"),
                String::from("esnext"),
            ];
            if strict {
                args.push(String::from("--strict"));
            }
            args.push(builtin_types.to_string_lossy().into_owned());
            args.push(path.to_string_lossy().into_owned());
            let result = execute::command_line(host, args);
            tsc::System::flush(host);
            result.status
        })
        .map_err(|error| format!("无法启动类型检查线程：{error}"))?
        .join()
        .map_err(|_| String::from("类型检查线程异常退出"))?;

    match status {
        tsc::ExitStatus::Success => Ok(()),
        tsc::ExitStatus::NotImplemented => Err(String::from("该类型检查请求尚未被编译器支持")),
        other => Err(format!("类型检查未通过（退出码 {}）", other as i32)),
    }
}

/// 执行一个源码文件：可选的预置类型检查 → 读取 → 转译擦除类型 → 以 ES Module 语义求值。
///
/// `mio.json` 的 `imports` 与配置目录会组装为 [`ModuleResolver`]，传给引擎后
/// 内部的 `TsModuleLoader` 就能识别 Import Maps 别名与基准目录。
/// 若配置 `check.typecheck == "on"`（或命令行 `--typecheck`），则先跑类型检查，任一诊断即中断。
fn run_file(path: &std::path::Path, config: &LoadedConfig) -> CliResult {
    if config.config.check.typecheck == "on" {
        check_file(path, config)?;
    }
    let resolver = config
        .config
        .build_resolver()?
        .with_base(config.root_dir.clone());
    let compiler = &config.config.compiler_options;
    let transpile = TranspileConfig {
        target: Some(compiler.target.clone()),
        sourcemap: compiler.source_map,
    };
    BoaEngine::with_transpile(resolver, transpile)
        .with_timeout(config.config.run.timeout_ms)
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
