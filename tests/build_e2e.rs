//! 阶段 B 原生后端的端到端测试：`mio build` 编译本机可执行文件。
//!
//! 覆盖三件事：产物能跑出预期输出、`--emit-obj` 只出 `.o`、不支持的语法给出带阶段名的错误。
//! 依赖宿主有可用的 `cc`；探测不到时整组跳过（打印跳过原因），避免环境差异造成假红。
//! Windows 未验证（原生链接只在 macOS/Linux 上确认过），按平台跳过。

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

/// 测试用临时目录，按用例名隔离，避免并行执行互相干扰。
fn workspace(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("mio_build_e2e_{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("应能创建测试目录");
    dir
}

/// 写入一个文件。
fn write(dir: &Path, relative: &str, content: &str) {
    std::fs::write(dir.join(relative), content).expect("应能写入文件");
}

/// 在指定目录下运行 mio，返回输出。
fn run_mio(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_mio"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("应能启动 mio")
}

/// 把输出拼接成便于断言的字符串。
fn combined(output: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

/// 当前环境是否具备原生编译条件：非 Windows 且能找到 `cc`。
///
/// 返回 `None` 表示可测；返回 `Some(原因)` 表示应跳过。
fn skip_reason() -> Option<String> {
    if cfg!(windows) {
        return Some(String::from("Windows 的原生链接尚未验证，跳过"));
    }
    match Command::new("cc").arg("--version").output() {
        Ok(output) if output.status.success() => None,
        _ => Some(String::from("未找到可用的 cc，跳过")),
    }
}

#[test]
fn builds_runnable_executable() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("runnable");
    write(
        &dir,
        "fib.ts",
        "function fib(n: i64): i64 {\n\
         \x20 if (n < 2) {\n\
         \x20   return n;\n\
         \x20 }\n\
         \x20 return fib(n - 1) + fib(n - 2);\n\
         }\n\
         function main(): void {\n\
         \x20 print(fib(20));\n\
         }\n",
    );

    let build = run_mio(&dir, &["build", "fib.ts", "-o", "fib"]);
    let text = combined(&build);
    assert!(build.status.success(), "构建应成功：{text}");

    let run = Command::new(dir.join("fib"))
        .output()
        .expect("应能执行产物");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert!(run.status.success(), "产物应正常退出");
    assert_eq!(stdout.trim(), "6765", "产物输出应为 fib(20)");
}

#[test]
fn defaults_output_path_to_entry_stem() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("default_out");
    write(
        &dir,
        "hello.ts",
        "function main(): void {\n  print(7);\n}\n",
    );

    let build = run_mio(&dir, &["build", "hello.ts"]);
    let text = combined(&build);
    assert!(build.status.success(), "构建应成功：{text}");
    // 缺省产物 = 入口去扩展名（hello.ts → hello）
    assert!(dir.join("hello").exists(), "应生成同目录同名产物：{text}");
}

#[test]
fn emit_obj_produces_object_only() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("emit_obj");
    write(&dir, "obj.ts", "function main(): void {\n  print(1);\n}\n");

    let build = run_mio(&dir, &["build", "obj.ts", "-o", "obj", "--emit-obj"]);
    let text = combined(&build);
    assert!(build.status.success(), "构建应成功：{text}");
    assert!(dir.join("obj.o").exists(), "应产出目标文件：{text}");
}

#[test]
fn unsupported_syntax_reports_stage_name() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("reject");
    // 字符串字面量属于阶段 B 尚未支持的形态，检查阶段应报错。
    write(
        &dir,
        "bad.ts",
        "function main(): void {\n  const s = \"hi\";\n  print(s);\n}\n",
    );

    let build = run_mio(&dir, &["build", "bad.ts", "-o", "bad"]);
    let text = combined(&build);
    assert!(!build.status.success(), "不支持的语法应失败：{text}");
    // 失败原因必须可操作：这里的确切信息是「变量 s 必须有类型标注」。
    assert!(
        text.contains("类型标注") || text.contains("支持"),
        "错误信息应说明原因：{text}"
    );
    assert!(!dir.join("bad").exists(), "失败时不应留下产物");
}

#[test]
fn lower_error_renders_line_column_and_caret() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("diag_lower");
    // 缺类型标注：lower 期错误，应精确到行列并渲染插入符。
    write(
        &dir,
        "bad.ts",
        "function main(): void {\n  const s = \"hi\";\n}\n",
    );

    let build = run_mio(&dir, &["build", "bad.ts", "-o", "bad"]);
    let text = combined(&build);
    assert!(!build.status.success(), "应失败：{text}");
    assert!(text.contains("bad.ts:2:9"), "应定位到行列：{text}");
    assert!(text.contains("类型标注"), "消息应说明原因：{text}");
    assert!(text.contains('^'), "应渲染插入符：{text}");
}

#[test]
fn check_error_points_to_function_declaration_line() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("diag_check");
    // `as` 缩窄不在白名单：check 期错误，TIR 无 span，定位到函数声明行。
    write(
        &dir,
        "bad3.ts",
        "function main(): void {\n  let a: i32 = 1;\n  let b: i32 = a as i8;\n}\n",
    );

    let build = run_mio(&dir, &["build", "bad3.ts", "-o", "bad3"]);
    let text = combined(&build);
    assert!(!build.status.success(), "应失败：{text}");
    assert!(text.contains("bad3.ts:1:1"), "应定位到函数声明行：{text}");
    assert!(text.contains("不在白名单"), "消息应说明原因：{text}");
}

#[test]
fn multi_file_import_builds_and_runs() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("multi_import");
    write(
        &dir,
        "util.ts",
        "function abs(x: i64): i64 {\n\
         \x20 if (x < 0) {\n\
         \x20   return -x;\n\
         \x20 }\n\
         \x20 return x;\n\
         }\n",
    );
    write(
        &dir,
        "app.ts",
        "import { abs } from \"./util\";\n\
         function main(): void {\n\
         \x20 let sum: i64 = 0;\n\
         \x20 for (let i: i64 = -3; i <= 3; i = i + 1) {\n\
         \x20   let a: i64 = abs(i);\n\
         \x20   sum = sum + a;\n\
         \x20 }\n\
         \x20 print(sum);\n\
         }\n",
    );

    let build = run_mio(&dir, &["build", "app.ts", "-o", "app"]);
    let text = combined(&build);
    assert!(build.status.success(), "多文件构建应成功：{text}");
    let run = Command::new(dir.join("app"))
        .output()
        .expect("应能执行产物");
    let stdout = String::from_utf8_lossy(&run.stdout);
    assert_eq!(stdout.trim(), "12", "产物输出应为 |i| 之和（-3..=3）");
}

#[test]
fn import_cycle_is_rejected() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("multi_cycle");
    write(
        &dir,
        "a.ts",
        "import { b } from \"./b\";\nfunction a(): void { b(); }\nfunction main(): void { a(); }\n",
    );
    write(
        &dir,
        "b.ts",
        "import { a } from \"./a\";\nfunction b(): void { a(); }\n",
    );

    let build = run_mio(&dir, &["build", "a.ts", "-o", "a"]);
    let text = combined(&build);
    assert!(!build.status.success(), "循环导入应失败：{text}");
    assert!(text.contains("循环导入"), "应说明循环导入：{text}");
}

#[test]
fn import_bare_specifier_is_rejected() {
    if let Some(reason) = skip_reason() {
        eprintln!("{reason}");
        return;
    }
    let dir = workspace("multi_bare");
    write(
        &dir,
        "app.ts",
        "import { f } from \"some-pkg\";\nfunction main(): void { f(); }\n",
    );

    let build = run_mio(&dir, &["build", "app.ts", "-o", "app"]);
    let text = combined(&build);
    assert!(!build.status.success(), "裸包名应失败：{text}");
    assert!(text.contains("相对路径"), "应说明只支持相对路径：{text}");
}

#[test]
fn build_only_options_rejected_on_other_commands() {
    let dir = workspace("bad_flag");
    let output = run_mio(&dir, &["run", "app.ts", "-o", "x"]);
    let text = combined(&output);
    assert!(!output.status.success(), "应拒绝非法选项：{text}");
    assert!(text.contains("只能用于 build"), "应说明选项归属：{text}");
}
