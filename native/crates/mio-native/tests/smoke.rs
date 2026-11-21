//! 端到端冒烟：编译用例 → 执行产物 → 比对 stdout。
//!
//! 用例源码集中在 `cases.rs`（成功用例表 + 应当被拒绝的源码表）。
//! 每个成功用例经 `compile_file` 产可执行文件，执行后比对输出；
//! 拒绝用例只断言 `compile_file` 返回带关键字的错误。

use std::path::PathBuf;
use std::process::Command;

mod cases;

/// 为单个用例建独立临时目录（按纳米秒时间戳隔离，支持并行）。
fn workspace(tag: &str) -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("系统时间")
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("mio-native-smoke-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

#[test]
fn smoke_cases_compile_and_run() {
    for (tag, src, expected) in cases::CASES {
        let dir = workspace(tag);
        let src_path = dir.join("app.ts");
        let out_path = dir.join("app.out");
        std::fs::write(&src_path, src).expect("写入用例失败");

        mio_native::compile_file(src_path.to_str().unwrap(), &out_path, false)
            .unwrap_or_else(|error| panic!("用例 {tag} 应编译通过：{error}"));

        let output = Command::new(&out_path)
            .output()
            .unwrap_or_else(|error| panic!("用例 {tag} 应能执行：{error}"));
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(output.status.success(), "用例 {tag} 应退出码 0：{output:?}");
        assert_eq!(stdout, *expected, "用例 {tag} 的输出不符");

        std::fs::remove_dir_all(&dir).ok();
    }
}

#[test]
fn rejected_sources_fail_with_keyword() {
    for (tag, src, keyword) in cases::REJECTED {
        let dir = workspace(tag);
        let src_path = dir.join("app.ts");
        let out_path = dir.join("app.out");
        std::fs::write(&src_path, src).expect("写入用例失败");

        let result = mio_native::compile_file(src_path.to_str().unwrap(), &out_path, false);
        let error = match result {
            Ok(()) => panic!("用例 {tag} 应编译失败（预期被拒绝）"),
            Err(error) => error,
        };
        assert!(
            error.contains(keyword),
            "用例 {tag} 的错误应含 `{keyword}`，实际：{error}"
        );

        std::fs::remove_dir_all(&dir).ok();
    }
}
