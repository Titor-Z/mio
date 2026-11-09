//! 交互式求值环境（REPL）。
//!
//! 设计：
//! - 复用同一个 [`BoaEngine`]，因此变量、函数声明在多次输入之间保持；
//! - 每行先按 TypeScript 转译（擦除类型），再以脚本语义求值；
//! - 支持基于括号/花括号配平的续行（提示符由 `> ` 变为 `... `）；
//! - 错误不退出，打印后继续；`:quit` / `exit` / EOF 结束。
//!
//! 限制：脚本语义不支持顶层 `await`（与 `mio eval` 一致）；需要异步时请写文件用 `mio run`。

use std::io::{self, BufRead, IsTerminal, Write};

use crate::engine::{BoaEngine, Engine};
use crate::transpiler::{TranspileConfig, Transpiler};

use super::CliResult;

/// REPL 的结束指令。
fn is_quit(command: &str) -> bool {
    matches!(command, ":quit" | ":q" | "exit" | "quit")
}

/// 计算一段源码里尚未配平的括号数（>0 表示需要续行）。
///
/// 只做粗粒度计数，忽略字符串与注释内部的括号——对交互输入足够，且实现简单。
fn open_brackets(source: &str) -> i32 {
    let mut depth = 0i32;
    let mut in_single = false;
    let mut in_double = false;
    let mut escaped = false;
    for ch in source.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_single || in_double => escaped = true,
            '\'' if !in_double => in_single = !in_single,
            '"' if !in_single => in_double = !in_double,
            '(' | '[' | '{' if !in_single && !in_double => depth += 1,
            ')' | ']' | '}' if !in_single && !in_double => depth -= 1,
            _ => {}
        }
    }
    depth
}

/// 启动 REPL。
pub(super) fn run() -> CliResult {
    let mut engine = BoaEngine::new();
    let interactive = io::stdin().is_terminal();
    let stdin = io::stdin();
    let mut lines = stdin.lock().lines();

    if interactive {
        println!("mio REPL —— 输入 :quit 退出");
    }

    loop {
        let mut buffer = String::new();
        match read_statement(&mut lines, &mut buffer, interactive)? {
            ReadOutcome::Eof => break,
            ReadOutcome::Empty => continue,
            ReadOutcome::Command(text) => {
                if is_quit(text.trim()) {
                    break;
                }
                evaluate(&mut engine, &text);
            }
        }
    }
    Ok(())
}

/// 一次读取的结果。
enum ReadOutcome {
    /// 输入结束（EOF）。
    Eof,
    /// 空行（无内容）。
    Empty,
    /// 一条完整语句。
    Command(String),
}

/// 读取一条完整语句，必要时按括号配平续行。
fn read_statement(
    lines: &mut impl Iterator<Item = io::Result<String>>,
    buffer: &mut String,
    interactive: bool,
) -> Result<ReadOutcome, String> {
    loop {
        prompt(interactive, buffer.is_empty());
        let Some(line) = lines.next() else {
            return Ok(if buffer.trim().is_empty() {
                ReadOutcome::Eof
            } else {
                ReadOutcome::Command(buffer.clone())
            });
        };
        let line = line.map_err(|error| format!("读取输入失败：{error}"))?;

        if buffer.is_empty() && line.trim().is_empty() {
            return Ok(ReadOutcome::Empty);
        }

        buffer.push_str(&line);
        buffer.push('\n');

        if open_brackets(buffer) > 0 {
            continue;
        }
        return Ok(ReadOutcome::Command(buffer.clone()));
    }
}

/// 打印提示符：仅交互式终端打印，避免污染管道输入。
fn prompt(interactive: bool, fresh: bool) {
    if !interactive {
        return;
    }
    if fresh {
        print!("> ");
    } else {
        print!("... ");
    }
    let _ = io::stdout().flush();
}

/// 转译并求值一条语句，回显非空结果；错误只打印不退出。
fn evaluate(engine: &mut BoaEngine, source: &str) {
    let transpiled = match Transpiler::transpile(
        source,
        std::path::Path::new("repl.ts"),
        &TranspileConfig::default(),
    ) {
        Ok(output) => output,
        Err(diagnostics) => {
            eprintln!("{}", diagnostics.join("\n"));
            return;
        }
    };

    match engine.eval_script(&transpiled.code) {
        Ok(value) => {
            if !value.is_empty() {
                println!("{value}");
            }
        }
        Err(error) => eprintln!("{error}"),
    }
}
