//! mio 命令行入口。
//!
//! 入口只做三件事：解析参数、分发命令、把错误转成进程退出码。

use std::process::ExitCode;

use mio::cli::{Cli, dispatch};

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("mio: {message}");
            return ExitCode::from(2);
        }
    };

    match dispatch(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mio: {message}");
            ExitCode::FAILURE
        }
    }
}
