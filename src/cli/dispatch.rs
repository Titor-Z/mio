//! 命令分发。
//!
//! 每个子命令对应一个处理函数，分发本身只做 `match` 与错误收敛。
//! 现阶段除版本与帮助外的命令返回「尚未实现」提示，
//! 后续步骤逐个替换为真实实现。

use crate::cli::CliResult;
use crate::cli::args::{Cli, Command};

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

/// 执行一次命令行调用，返回进程退出码。
pub fn dispatch(cli: &Cli) -> CliResult {
    match &cli.command {
        Command::Version => {
            println!("mio {}", crate::VERSION);
            Ok(())
        }
        Command::Help => {
            print!("{HELP_TEXT}");
            Ok(())
        }
        Command::Run { path } => Err(not_implemented(&format!("run {}", path.display()))),
        Command::Eval { .. } => Err(not_implemented("eval")),
        Command::Repl => Err(not_implemented("repl")),
        Command::Check { path } => Err(not_implemented(&format!("check {}", path.display()))),
        Command::Task { name } => Err(not_implemented(&format!(
            "task {}",
            name.as_deref().unwrap_or("<list>")
        ))),
    }
}

/// 统一构造「尚未实现」错误，避免各分支重复拼接文案。
fn not_implemented(what: &str) -> String {
    format!("{what}：该功能尚未实现")
}
