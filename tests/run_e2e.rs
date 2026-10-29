//! 端到端测试：`mio run` 执行 TypeScript 文件。
//!
//! 覆盖里程碑 1 的最小垂直切片：类型/接口擦除、enum 求值、console 可观测输出、
//! 运行期异常收敛为退出码 1。
//!
//! 与 `config_e2e.rs` 不同，本测试不依赖任何配置，仅用临时目录隔离用例。

use std::path::PathBuf;
use std::process::{Command, Output};

fn mio_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mio"))
}

fn run(args: &[&str]) -> Output {
    Command::new(mio_binary())
        .args(args)
        .output()
        .expect("mio 二进制应能被调用")
}

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system time")
        .as_nanos();
    dir.push(format!("mio-run-{tag}-{nanos}"));
    std::fs::create_dir_all(&dir).expect("创建临时目录失败");
    dir
}

#[test]
fn run_erases_types_and_prints() {
    let dir = unique_dir("basic");
    let file = dir.join("hello.ts");
    std::fs::write(
        &file,
        "interface User { name: string }\n\
         type Id = number;\n\
         const u: User = { name: 'ada' };\n\
         console.log('id', 1 + 2, u.name);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "run 应成功退出：{output:?}");
    assert_eq!(stdout.trim(), "id 3 ada");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_evaluates_enum_members() {
    let dir = unique_dir("enum");
    let file = dir.join("en.ts");
    std::fs::write(
        &file,
        "enum Dir { Up = 1, Down = 2 }\n\
         console.log(Dir.Up + Dir.Down);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "enum 求值应成功：{output:?}");
    assert_eq!(stdout.trim(), "3");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_reports_runtime_error_with_exit_one() {
    let dir = unique_dir("throw");
    let file = dir.join("boom.ts");
    std::fs::write(&file, "throw new Error('boom');\n").unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "运行期异常应以 1 退出");
    assert!(stderr.contains("boom"), "stderr 应包含异常信息：{stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_reports_syntax_error_with_exit_one() {
    let dir = unique_dir("syntax");
    let file = dir.join("bad.ts");
    std::fs::write(&file, "const x: = ;\n").unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1), "语法/转译错误应以 1 退出");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn eval_prints_completion_value() {
    let output = run(&["eval", "const f=(n:number):number=>n*2; f(21)"]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "eval 应成功：{output:?}");
    assert_eq!(stdout.trim(), "42");
}

#[test]
fn run_resolves_relative_imports_across_files() {
    let dir = unique_dir("import");
    std::fs::write(
        dir.join("greet.ts"),
        "export interface N { name: string }\n\
         export function greet(n: N): string { return `hi ${n.name}`; }\n",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("math")).unwrap();
    std::fs::write(
        dir.join("math/index.ts"),
        "export const add = (a: number, b: number) => a + b;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { greet } from './greet.ts';\n\
         import { add } from './math';\n\
         console.log(greet({ name: 'mio' }), add(2, 3));\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "多文件 import 应成功：{output:?}");
    assert_eq!(stdout.trim(), "hi mio 5");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_reports_unresolvable_import_with_exit_one() {
    let dir = unique_dir("missing");
    std::fs::write(
        dir.join("main.ts"),
        "import { x } from './nope.ts';\nconsole.log(x);\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(
        output.status.code(),
        Some(1),
        "无法解析的 import 应以 1 退出"
    );
    assert!(stderr.contains("nope"), "stderr 应指名出错说明符：{stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_resolves_import_map_alias_against_config_root() {
    // 配置在根、入口在子目录：@app/ 映射应相对配置目录解析，而非引用者目录；
    // 同时覆盖别名省略扩展名与带扩展名两种写法。
    let dir = unique_dir("importmap");
    std::fs::write(
        dir.join("mio.json"),
        "{ \"imports\": { \"@app/\": \"./src/\" } }",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("src")).unwrap();
    std::fs::create_dir_all(dir.join("sub")).unwrap();
    std::fs::write(
        dir.join("src/math.ts"),
        "export const add = (a: number, b: number): number => a + b;\n",
    )
    .unwrap();
    std::fs::write(dir.join("src/label.ts"), "export const tag = 'ok';\n").unwrap();
    std::fs::write(
        dir.join("sub/entry.ts"),
        "import { add } from '@app/math';\n\
         import { tag } from '@app/label.ts';\n\
         console.log(add(2, 3), tag);\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("sub/entry.ts").to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "别名导入应成功：{output:?}");
    assert_eq!(stdout.trim(), "5 ok");

    std::fs::remove_dir_all(&dir).ok();
}
