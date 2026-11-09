//! 端到端测试：`mio repl`。
//!
//! 通过管道喂入脚本，验证：
//! - 跨行保持状态（变量与函数声明可复用）
//! - 表达式求值回显
//! - 多行续行（花括号配平）
//! - 错误不退出，后续输入仍可执行

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Stdio};

fn mio_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mio"))
}

/// 以管道输入运行 REPL，返回合并后的输出。
fn run_repl(input: &str) -> String {
    let mut child = Command::new(mio_binary())
        .arg("repl")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("应能启动 mio repl");

    child
        .stdin
        .take()
        .expect("应有 stdin")
        .write_all(input.as_bytes())
        .expect("应能写入输入");

    let output = child.wait_with_output().expect("应能读取输出");
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn repl_keeps_state_and_evaluates() {
    let text = run_repl("const x: number = 2 * 3;\nx + 1\n:quit\n");
    assert!(text.contains('7'), "应回显表达式结果 7：{text}");
}

#[test]
fn repl_multiline_and_error_recovery() {
    let input = "\
function f(n: number) {
  return n * n;
}
f(5)
undefined_variable_xyz
\"alive\"
:quit
";
    let text = run_repl(input);
    assert!(text.contains("25"), "多行函数应求值为 25：{text}");
    assert!(
        text.contains("ReferenceError"),
        "应报出 ReferenceError：{text}"
    );
    assert!(text.contains("alive"), "错误后应继续执行：{text}");
}
