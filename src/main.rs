//! mio 命令行入口。
//!
//! 入口只做四件事：解析参数、加载配置、分发命令、把错误转成进程退出码。

use std::path::PathBuf;
use std::process::ExitCode;

use mio::cli::{Cli, dispatch};
use mio::config::{ConfigLoader, LoadedConfig, ProcessEnvironment};

fn main() -> ExitCode {
    let cli = match Cli::parse(std::env::args().skip(1)) {
        Ok(cli) => cli,
        Err(message) => {
            eprintln!("mio: {message}");
            return ExitCode::from(2);
        }
    };

    let loaded = match load_config(&cli) {
        Ok(loaded) => loaded,
        Err(message) => {
            eprintln!("mio: {message}");
            return ExitCode::FAILURE;
        }
    };

    print_diagnostics(&loaded);

    match dispatch(&cli, &loaded) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("mio: {message}");
            ExitCode::FAILURE
        }
    }
}

/// 加载配置文件。
///
/// 查找起点优先取待执行文件所在目录，这样 `mio run sub/dir/app.ts`
/// 会命中 `sub/dir/mio.json` 而不是工作目录下的同名文件；
/// 没有文件参数时退回当前工作目录。
fn load_config(cli: &Cli) -> Result<LoadedConfig, String> {
    let start_dir = entry_directory(cli);
    ConfigLoader::load(
        &start_dir,
        cli.config_path.as_deref(),
        &cli.overrides,
        &ProcessEnvironment,
    )
}

/// 推导配置文件查找的起始目录。
fn entry_directory(cli: &Cli) -> PathBuf {
    let entry = cli.command.entry_path();
    let directory = entry
        .and_then(|path| path.parent())
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(PathBuf::from);

    directory.unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

/// 打印配置诊断，遵循 `log.level` 设置。
fn print_diagnostics(loaded: &LoadedConfig) {
    let silenced = matches!(loaded.config.log.level.as_str(), "silent" | "error");
    if silenced {
        return;
    }
    for line in loaded.warnings() {
        eprintln!("{line}");
    }
}
