//! 配置系统的端到端测试。
//!
//! 通过真实调用 `mio` 可执行文件验证配置加载链路：
//! 发现 → 继承 → 环境变量展开 → 优先级叠加 → 诊断输出。
//!
//! 之所以用进程级测试而非库内测试，是因为需要覆盖「向上查找配置文件」
//! 与「环境变量参与优先级」这两项与进程上下文相关的行为。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 测试用临时目录，按用例名隔离，避免并行执行互相干扰。
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mio_e2e_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能创建测试目录");
    dir
}

/// 写入一个文件，自动创建父目录。
fn write(dir: &Path, relative: &str, content: &str) {
    let path = dir.join(relative);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).expect("应能创建父目录");
    }
    std::fs::write(path, content).expect("应能写入文件");
}

/// 在指定目录下运行 mio，返回输出。
fn run_mio(dir: &Path, args: &[&str], environment: &[(&str, &str)]) -> Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_mio"));
    command.current_dir(dir).args(args);
    for (key, value) in environment {
        command.env(key, value);
    }
    command.output().expect("应能启动 mio")
}

/// 把输出拼接成便于断言的字符串。
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

#[test]
fn discovers_config_in_current_directory() {
    let dir = workspace("discover");
    write(&dir, "mio.json", r#"{ "tasks": { "hello": "run a.ts" } }"#);

    let output = run_mio(&dir, &["task"], &[]);
    let text = combined(&output);

    assert!(output.status.success(), "应成功退出：{text}");
    assert!(text.contains("hello"), "应列出任务：{text}");
}

#[test]
fn discovers_config_from_parent_directory() {
    let dir = workspace("parent");
    write(&dir, "mio.json", r#"{ "tasks": { "up": "run a.ts" } }"#);
    write(&dir, "src/deep/keep.txt", "");

    let output = run_mio(&dir.join("src/deep"), &["task"], &[]);
    let text = combined(&output);

    assert!(text.contains("up"), "应向上找到配置：{text}");
}

#[test]
fn warns_on_unknown_field_with_suggestion() {
    let dir = workspace("unknown");
    write(
        &dir,
        "mio.json",
        r#"{ "compilerOptions": { "targett": "es2025" } }"#,
    );

    let output = run_mio(&dir, &["task"], &[]);
    let text = combined(&output);

    assert!(
        text.contains("compilerOptions.targett"),
        "应提示拼写：{text}"
    );
    assert!(text.contains("target"), "应给出建议：{text}");
}

#[test]
fn silent_log_level_suppresses_warnings() {
    let dir = workspace("silent");
    write(
        &dir,
        "mio.json",
        r#"{ "compilerOptions": { "targett": "es2025" } }"#,
    );

    let output = run_mio(&dir, &["task"], &[("MIO_LOG_LEVEL", "silent")]);
    let text = combined(&output);

    assert!(
        !text.contains("compilerOptions.targett"),
        "静默级别不应输出告警：{text}"
    );
}

#[test]
fn extends_inherits_parent_fields() {
    let dir = workspace("extends");
    write(
        &dir,
        "base.json",
        r#"{ "tasks": { "fromBase": "run base.ts" } }"#,
    );
    write(
        &dir,
        "mio.json",
        r#"{ "extends": "./base.json", "tasks": { "fromChild": "run child.ts" } }"#,
    );

    let output = run_mio(&dir, &["task"], &[]);
    let text = combined(&output);

    // 父配置的任务被保留，子配置的同名键才会覆盖（此处键不同，两者都在）
    assert!(text.contains("fromBase"), "应继承父配置任务：{text}");
    assert!(text.contains("fromChild"), "应保留子配置任务：{text}");
}

#[test]
fn cycles_are_reported() {
    let dir = workspace("cycle");
    write(&dir, "a.json", r#"{ "extends": "./b.json" }"#);
    write(&dir, "b.json", r#"{ "extends": "./a.json" }"#);

    let output = run_mio(&dir, &["--config", "a.json", "task"], &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "循环继承应失败：{text}");
    assert!(text.contains("循环继承"), "应说明原因：{text}");
}

#[test]
fn missing_task_reports_error() {
    let dir = workspace("missing_task");
    write(&dir, "mio.json", r#"{ "tasks": { "only": "run a.ts" } }"#);

    let output = run_mio(&dir, &["task", "absent"], &[]);
    let text = combined(&output);

    assert!(!output.status.success(), "未知任务应失败：{text}");
    assert!(text.contains("不存在任务"), "应说明原因：{text}");
}
