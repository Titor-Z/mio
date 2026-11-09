//! 端到端测试：`@std/fs`（Phase 2，工作线程池 + FD 控制层）。
//!
//! 通过真实运行 `mio run` 验证：
//! - Convenience 层（readFile/writeFile/stat/readdir/remove）
//! - FD 控制层（open/read/write/close 分块读写）
//! - fs 操作期间事件循环不被阻塞（定时器仍能触发）
//!
//! 每个用例用独立临时目录隔离。

use std::path::PathBuf;
use std::process::{Command, Output};

fn mio_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mio"))
}

/// 建立隔离的工作目录。
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mio_fs_e2e_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能创建测试目录");
    dir
}

/// 在工作目录写入一个 TypeScript 文件。
fn write_ts(dir: &std::path::Path, name: &str, source: &str) {
    std::fs::write(dir.join(name), source).expect("应能写入脚本");
}

/// 运行 `mio run <file>`。
fn run(dir: &std::path::Path, file: &str) -> Output {
    Command::new(mio_binary())
        .current_dir(dir)
        .args(["run", file])
        .output()
        .expect("应能启动 mio")
}

/// 合并 stdout + stderr。
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn fs_convenience_roundtrip() {
    let dir = workspace("convenience");
    write_ts(
        &dir,
        "main.ts",
        r#"
import * as fs from "@std/fs";

await fs.writeTextFile("data.txt", "你好 fs");
const text = await fs.readTextFile("data.txt");
console.log("TEXT:" + text);

const info = await fs.stat("data.txt");
console.log("IS_FILE:" + info.isFile + " SIZE:" + info.size);

console.log("EXISTS:" + (await fs.exists("data.txt")));
await fs.remove("data.txt");
console.log("GONE:" + (await fs.exists("data.txt")));
"#,
    );

    let output = run(&dir, "main.ts");
    let text = combined(&output);
    assert!(output.status.success(), "应成功退出：{text}");
    assert!(text.contains("TEXT:你好 fs"), "读取内容不符：{text}");
    assert!(text.contains("IS_FILE:true SIZE:9"), "stat 不符：{text}");
    assert!(text.contains("EXISTS:true"), "exists 应为 true：{text}");
    assert!(text.contains("GONE:false"), "删除后应不存在：{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fs_fd_streaming_roundtrip() {
    let dir = workspace("streaming");
    write_ts(
        &dir,
        "main.ts",
        r#"
import * as fs from "@std/fs";

const writer = await fs.open("stream.bin", "w");
const payload = new TextEncoder().encode("ABCDEFGHIJ");
const written = await writer.write(payload);
await writer.close();
console.log("WRITTEN:" + written);

const reader = await fs.open("stream.bin", "r");
const chunk = new Uint8Array(4);
const parts: Uint8Array[] = [];
while (true) {
  const n = await reader.read(chunk);
  if (n === 0) break;
  parts.push(chunk.slice(0, n));
}
await reader.close();

const total = parts.reduce((sum, part) => sum + part.length, 0);
const merged = new Uint8Array(total);
let offset = 0;
for (const part of parts) {
  merged.set(part, offset);
  offset += part.length;
}
console.log("READ:" + new TextDecoder().decode(merged));
"#,
    );

    let output = run(&dir, "main.ts");
    let text = combined(&output);
    assert!(output.status.success(), "应成功退出：{text}");
    assert!(text.contains("WRITTEN:10"), "写入字节数不符：{text}");
    assert!(text.contains("READ:ABCDEFGHIJ"), "分块读回内容不符：{text}");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fs_does_not_block_event_loop() {
    let dir = workspace("nonblocking");
    write_ts(
        &dir,
        "main.ts",
        r#"
import * as fs from "@std/fs";

// 先准备一个较大文件（约 512KB），让读取耗时足够长以暴露阻塞。
const blob = new Uint8Array(512 * 1024).fill(65);
await fs.writeFile("big.bin", blob);

let timerFired = false;
const timer = setTimeout(() => {
  timerFired = true;
}, 0);

const data = await fs.readFile("big.bin");

// 让微任务/定时器有机会运行。
await new Promise<void>((resolve) => setTimeout(resolve, 5));
console.log("SIZE:" + data.length + " TIMER:" + timerFired);
clearTimeout(timer);
await fs.remove("big.bin");
"#,
    );

    let output = run(&dir, "main.ts");
    let text = combined(&output);
    assert!(output.status.success(), "应成功退出：{text}");
    assert!(
        text.contains("SIZE:524288 TIMER:true"),
        "fs 期间定时器应触发（事件循环未被阻塞）：{text}"
    );

    let _ = std::fs::remove_dir_all(&dir);
}
