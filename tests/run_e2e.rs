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

/// 启动一个本地 mock HTTP 服务器（仅监听回环地址，不依赖公网）。
///
/// 路由：`/hello` 200 文本+重复自定义头；`/json` 200 JSON；`/missing` 404；
/// 其余 200 回显路径；`/slow` 接受连接但不回应，用于触发宿主超时。
/// 每个连接响应后关闭（`connection: close`）。服务器随测试进程结束自然回收。
fn spawn_mock_server() -> u16 {
    use std::io::{BufRead, BufReader, Read, Write};
    use std::net::TcpListener;

    let listener = TcpListener::bind("127.0.0.1:0").expect("mock 服务器应能绑定 ephemeral 端口");
    let port = listener
        .local_addr()
        .expect("mock 服务器应有本地地址")
        .port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("克隆连接应成功"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            let method = request_line
                .split_whitespace()
                .next()
                .unwrap_or("GET")
                .to_string();
            // 耗尽请求头（避免过早断开影响客户端发送），顺便取 content-length 与 x-test。
            let mut content_length = 0usize;
            let mut test_header = String::new();
            let mut header_block = String::new();
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        header_block.push_str(&line);
                        let lower = line.to_ascii_lowercase();
                        if let Some(value) = lower.strip_prefix("content-length:") {
                            content_length = value.trim().parse().unwrap_or(0);
                        }
                        if let Some(value) = lower.strip_prefix("x-test:") {
                            test_header = value.trim().to_string();
                        }
                        if line.trim().is_empty() {
                            break;
                        }
                    }
                    Err(_) => break,
                }
            }
            let path = request_line
                .split_whitespace()
                .nth(1)
                .unwrap_or("/")
                .to_string();
            if path == "/slow" {
                // 不回应：挂住连接，直到宿主事件循环预算耗尽。
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_secs(30));
                    drop(stream);
                });
                continue;
            }
            let mut body_buffer = vec![0u8; content_length];
            if content_length > 0 && reader.read_exact(&mut body_buffer).is_err() {
                continue;
            }
            let request_body_raw = String::from_utf8_lossy(&body_buffer).into_owned();
            // 回显值直接嵌进 JSON 响应：转义反斜杠与双引号（测试可控范围内够用）。
            let request_body = request_body_raw.replace('\\', "\\\\").replace('"', "\\\"");
            let test_header = test_header.replace('\\', "\\\\").replace('"', "\\\"");
            let (status, body, extra): (&str, String, String) = match path.as_str() {
                "/hello" => (
                    "200 OK",
                    String::from("hello mio"),
                    String::from("x-mio: ok\r\nx-mio: again\r\n"),
                ),
                "/json" => (
                    "200 OK",
                    String::from(r#"{"answer":42}"#),
                    String::from("content-type: application/json\r\n"),
                ),
                "/missing" => ("404 Not Found", String::from("nope"), String::new()),
                // 回显方法、请求体与 x-test 头；值均为可控 ASCII，无需 JSON 转义。
                // 原样回显请求头与请求体：multipart 等含控制字符的内容走 text/plain 直传。
                "/raw" => (
                    "200 OK",
                    format!("headers:{header_block}body:{request_body_raw}"),
                    String::from("content-type: text/plain; charset=utf-8\r\n"),
                ),
                "/echo" => (
                    "200 OK",
                    format!(
                        r#"{{"method":"{method}","body":"{request_body}","xTest":"{test_header}"}}"#
                    ),
                    String::from("content-type: application/json\r\n"),
                ),
                other => ("200 OK", format!("path={other}"), String::new()),
            };
            let response = format!(
                "HTTP/1.1 {status}\r\ncontent-length: {}\r\nconnection: close\r\n{extra}\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });
    port
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
fn check_reports_type_error_with_exit_one() {
    let dir = unique_dir("check-bad");
    let file = dir.join("bad.ts");
    std::fs::write(
        &file,
        "interface User { name: string }\n\
         const u: User = { name: 'ada' };\n\
         const n: number = u.name;\n",
    )
    .unwrap();

    let output = run(&["check", file.to_str().unwrap()]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(output.status.code(), Some(1), "含类型错误应以 1 退出");
    assert!(
        combined.contains("TS2322"),
        "应报出 TS2322 类型不兼容：{combined}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_with_typecheck_flag_aborts_on_type_error() {
    let dir = unique_dir("run-check-abort");
    let file = dir.join("bad.ts");
    std::fs::write(
        &file,
        "interface User { name: string }\n\
         const u: User = { name: 'ada' };\n\
         const n: number = u.name;\n\
         console.log('SHOULD_NOT_PRINT');\n",
    )
    .unwrap();

    let output = run(&["run", "--typecheck", file.to_str().unwrap()]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(
        output.status.code(),
        Some(1),
        "--typecheck 下含错应中断执行"
    );
    assert!(combined.contains("TS2322"), "应报出 TS2322：{combined}");
    assert!(
        !combined.contains("SHOULD_NOT_PRINT"),
        "不应推进到运行时：{combined}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_supports_top_level_await() {
    let dir = unique_dir("tla");
    let file = dir.join("tla.ts");
    // ES2022 顶层 await：模块求值应挂起至 Promise settle 后再继续。
    std::fs::write(
        &file,
        "interface Box { v: number }\n\
         const b: Box = { v: 41 };\n\
         const got: number = await Promise.resolve(b.v + 1);\n\
         console.log('tla', got);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "顶层 await 应能执行：{output:?}");
    assert_eq!(stdout.trim(), "tla 42");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn run_default_does_not_typecheck() {
    let dir = unique_dir("run-no-check");
    let file = dir.join("bad.ts");
    // 同样的类型错误文件：默认不设 typecheck 时，应直接运行到运行时（类型擦除后合法）。
    std::fs::write(
        &file,
        "interface User { name: string }\n\
         const u: User = { name: 'ada' };\n\
         const n: number = u.name;\n\
         console.log('RAN');\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "默认不开检查时不应阻断：{output:?}"
    );
    assert_eq!(stdout.trim(), "RAN");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_uses_strict_from_config() {
    let dir = unique_dir("check-strict");
    // 写入一个开启 strict 的配置，与一个含“隐式 any”的文件。
    std::fs::write(
        dir.join("mio.json"),
        "{ \"compilerOptions\": { \"strict\": true } }",
    )
    .unwrap();
    let file = dir.join("any.ts");
    std::fs::write(
        &file,
        "function greet(name) { return 'Hi ' + name; }\nconsole.log(greet);\n",
    )
    .unwrap();

    let output = run(&[
        "check",
        "-c",
        dir.join("mio.json").to_str().unwrap(),
        file.to_str().unwrap(),
    ]);
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
    assert_eq!(output.status.code(), Some(1), "strict 下隐式 any 应不通过");
    assert!(
        combined.contains("TS7006"),
        "应报出 TS7006 隐式 any：{combined}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_passes_clean_file() {
    let dir = unique_dir("check-good");
    let file = dir.join("good.ts");
    std::fs::write(
        &file,
        "interface User { name: string }\n\
         const u: User = { name: 'ada' };\n\
         console.log(u.name);\n",
    )
    .unwrap();

    let output = run(&["check", file.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "无类型错误时 check 应成功退出：{output:?}"
    );

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

#[test]
fn run_resolves_std_path_builtin() {
    let dir = unique_dir("stdpath");
    std::fs::write(
        dir.join("main.ts"),
        "import { join, extname } from '@std/path';\n\
         console.log(join('a', 'b'), extname('x.tar.gz'));\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "@std/path 应可用：{output:?}");
    assert_eq!(stdout.trim(), "a/b .gz");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn import_map_can_override_std_namespace() {
    // 用户把 @std/path 重映射到本地文件时，应优先采用本地实现而非内建。应
    let dir = unique_dir("override");
    std::fs::write(
        dir.join("mio.json"),
        "{ \"imports\": { \"@std/path\": \"./local/path.ts\" } }",
    )
    .unwrap();
    std::fs::create_dir_all(dir.join("local")).unwrap();
    std::fs::write(
        dir.join("local/path.ts"),
        "export const join = (...p: string[]): string => `over:${p.join('!')}`;\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { join } from '@std/path';\nconsole.log(join('a', 'b'));\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "覆盖应成功：{output:?}");
    assert_eq!(stdout.trim(), "over:a!b");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn unknown_std_module_reports_error() {
    let dir = unique_dir("unknown-std");
    std::fs::write(
        dir.join("main.ts"),
        "import { x } from '@std/doesnotexist';\nconsole.log(x);\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "未知内建模块应以 1 退出");
    assert!(stderr.contains("doesnotexist"), "应指名未知模块：{stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_resolves_response_status_headers_and_text() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-basic");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const r = await fetch('http://127.0.0.1:{port}/hello');\n\
             console.log(r.status, r.ok, r.headers.get('x-mio'), r.headers.has('nope'), await r.text());\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "fetch 用例应成功退出：{output:?}");
    assert_eq!(stdout.trim(), "200 true ok, again false hello mio");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_json_body_parses_via_await() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-json");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "interface Payload {{ answer: number }}\n\
             const r = await fetch('http://127.0.0.1:{port}/json');\n\
             const data: Payload = await r.json();\n\
             console.log('answer', data.answer, 'type', r.headers.get('content-type'));\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "json 用例应成功退出：{output:?}");
    assert_eq!(stdout.trim(), "answer 42 type application/json");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_error_status_is_not_rejection() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-404");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const r = await fetch('http://127.0.0.1:{port}/missing');\n\
             console.log(r.status, r.ok, await r.text());\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "404 不是拒绝：{output:?}");
    assert_eq!(stdout.trim(), "404 false nope");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_rejects_invalid_url_with_type_error() {
    let dir = unique_dir("fetch-bad-url");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "try {\n\
         \x20   await fetch('not-a-url');\n\
         \x20   console.log('NOT_REJECTED');\n\
         } catch (e) {\n\
         \x20   console.log('rejected', e instanceof TypeError);\n\
         }\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "捕获后应正常退出：{output:?}");
    assert_eq!(stdout.trim(), "rejected true");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_timeout_rejects_with_budget_from_config() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-timeout");
    std::fs::write(dir.join("mio.json"), "{ \"run\": { \"timeoutMs\": 300 } }").unwrap();
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "try {{\n\
             \x20   await fetch('http://127.0.0.1:{port}/slow');\n\
             \x20   console.log('NOT_REJECTED');\n\
             }} catch (e) {{\n\
             \x20   console.log('timeout?', String(e).includes('timed out'));\n\
             }}\n"
        ),
    )
    .unwrap();

    let output = run(&[
        "-c",
        dir.join("mio.json").to_str().unwrap(),
        "run",
        file.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "超时应被捕获而非挂死：{output:?}");
    assert_eq!(stdout.trim(), "timeout? true");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn check_accepts_top_level_await_and_fetch_types() {
    let dir = unique_dir("check-fetch");
    let file = dir.join("app.ts");
    // 顶层 await + fetch：tsrs 自带 DOM lib，且 moduleDetection 已按运行时语义对齐，应零诊断。
    std::fs::write(
        &file,
        "interface Payload { answer: number }\n\
         const r = await fetch('http://example.com');\n\
         const data: Payload = await r.json();\n\
         console.log(data.answer);\n",
    )
    .unwrap();

    let output = run(&["check", file.to_str().unwrap()]);
    assert!(
        output.status.success(),
        "合法 fetch/TLA 代码应通过检查：{:?}",
        String::from_utf8_lossy(&output.stdout)
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runtime_error_stack_points_to_typescript_lines() {
    let dir = unique_dir("sourcemap-run");
    let file = dir.join("drift.ts");
    // 前 6 行类型声明会被整行擦除：调用点 TS 第 13 行对应 JS 第 6 行，两坐标必然不同。
    std::fs::write(
        &file,
        "interface Widget {\n  name: string;\n}\n\ntype Label = string;\n\n\
         function boom(w: Widget): Label {\n  undefinedFunctionCall();\n  return w.name;\n}\n\n\
         const widget: Widget = { name: \"x\" };\nboom(widget);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "运行期错误应退出码 1");
    assert!(
        stderr.contains("drift.ts:13:1"),
        "主帧应指回 TS 第 13 行（JS 坐标是第 6 行）：{stderr}"
    );
    assert!(
        !stderr.contains("drift.ts:6:1"),
        "不应残留 JS 坐标的调用帧：{stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn runtime_error_in_imported_module_maps_to_its_source() {
    let dir = unique_dir("sourcemap-import");
    // 抛错语句放进函数：Boa 对模块顶层直接抛错不记录行列（输出 `?:?`），
    // 函数帧才有坐标；顶层调用帧则始终以 `?:?` 出现。
    std::fs::write(
        dir.join("dep.ts"),
        "type P = number;\n\nfunction kaboom(): P {\n  undefinedCall();\n  return 1;\n}\n\nexport const x: P = kaboom();\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("main.ts"),
        "import { x } from './dep.ts';\nconsole.log(x);\n",
    )
    .unwrap();

    let output = run(&["run", dir.join("main.ts").to_str().unwrap()]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "子模块运行期错误应退出码 1");
    // kaboom 调用在 dep.ts 第 8 行（擦除后 JS 在第 5 行）；
    // `at kaboom` 帧由 Boa 指向函数声明行，不做断言。
    assert!(
        stderr.contains("dep.ts:8:"),
        "子文件错误应反查回 dep.ts 第 8 行：{stderr}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn sourcemap_disabled_falls_back_to_js_coordinates() {
    let dir = unique_dir("sourcemap-off");
    std::fs::write(
        dir.join("mio.json"),
        "{ \"compilerOptions\": { \"sourceMap\": false } }",
    )
    .unwrap();
    let file = dir.join("drift.ts");
    std::fs::write(
        &file,
        "interface Widget {\n  name: string;\n}\n\ntype Label = string;\n\n\
         function boom(w: Widget): Label {\n  undefinedFunctionCall();\n  return w.name;\n}\n\n\
         const widget: Widget = { name: \"x\" };\nboom(widget);\n",
    )
    .unwrap();

    let output = run(&[
        "-c",
        dir.join("mio.json").to_str().unwrap(),
        "run",
        file.to_str().unwrap(),
    ]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(output.status.code(), Some(1), "关闭映射不影响错误退出码");
    assert!(
        stderr.contains("ReferenceError"),
        "错误本体仍完整报告：{stderr}"
    );
    // 注册表为空 → 主帧退回 JS 坐标（第 6 行；开启映射时是 TS 第 13 行）。
    assert!(
        stderr.contains("drift.ts:6:"),
        "无映射时应退回 JS 坐标：{stderr}"
    );
    assert!(!stderr.contains("drift.ts:13:"), "不应再做反查：{stderr}");
    assert!(!stderr.contains("panic"), "不应 panic：{stderr}");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_post_sends_method_headers_and_string_body() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-post");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const r = await fetch('http://127.0.0.1:{port}/echo', {{\n\
             \x20 method: 'POST',\n\
             \x20 headers: {{ 'X-Test': 'abc', 'content-type': 'application/json' }},\n\
             \x20 body: JSON.stringify({{ a: 1 }}),\n\
             }});\n\
             const data = await r.json();\n\
             console.log(data.method, data.body, data.xTest);\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "POST 用例应成功退出：{output:?}");
    assert_eq!(stdout.trim(), "POST {\"a\":1} abc");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_post_accepts_uint8array_body() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-uint8-body");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const payload = new Uint8Array([104, 101, 108, 108, 111]);\n\
             const r = await fetch('http://127.0.0.1:{port}/echo', {{ method: 'POST', body: payload }});\n\
             const data = await r.json();\n\
             console.log(data.body, data.method);\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Uint8Array body 用例应成功退出：{output:?}"
    );
    assert_eq!(stdout.trim(), "hello POST");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_arraybuffer_returns_uint8array() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-arraybuffer");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const r = await fetch('http://127.0.0.1:{port}/hello');\n\
             const b = await r.arrayBuffer();\n\
             console.log(b instanceof Uint8Array, b.byteLength, b[0]);\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "arrayBuffer 用例应成功退出：{output:?}"
    );
    // "hello mio"：9 字节，首字节 'h' = 104。
    assert_eq!(stdout.trim(), "true 9 104");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn abort_controller_rejects_inflight_fetch_with_abort_error() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-abort");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const ac = new AbortController();\n\
             console.log('before', ac.signal.aborted);\n\
             const p = fetch('http://127.0.0.1:{port}/slow', {{ signal: ac.signal }});\n\
             ac.abort();\n\
             console.log('after', ac.signal.aborted);\n\
             try {{\n\
             \x20   await p;\n\
             \x20   console.log('NOT_REJECTED');\n\
             }} catch (e) {{\n\
             \x20   console.log('aborted', e.name === 'AbortError');\n\
             }}\n\
             const ok = await fetch('http://127.0.0.1:{port}/hello');\n\
             console.log('still', ok.status);\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "abort 用例应成功退出：{output:?}");
    assert_eq!(
        stdout.trim(),
        "before false\nafter true\naborted true\nstill 200"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_with_already_aborted_signal_rejects_immediately() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-abort-pre");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const ac = new AbortController();\n\
             ac.abort();\n\
             fetch('http://127.0.0.1:{port}/hello', {{ signal: ac.signal }}).then(\n\
             \x20 () => console.log('RESOLVED'),\n\
             \x20 (e) => console.log('pre', e.name),\n\
             );\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "预 abort 用例应成功退出：{output:?}"
    );
    assert_eq!(stdout.trim(), "pre AbortError");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_get_with_body_throws_type_error() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-get-body");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "try {{\n\
             \x20   fetch('http://127.0.0.1:{port}/hello', {{ body: 'x' }});\n\
             \x20   console.log('NO_THROW');\n\
             }} catch (e) {{\n\
             \x20   console.log('throws', e instanceof TypeError);\n\
             }}\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "同步抛错被捕获：{output:?}");
    assert_eq!(stdout.trim(), "throws true");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn timers_fire_in_order_and_process_exits_when_drained() {
    let dir = unique_dir("timers-order");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "// 微任务先于定时器；queueMicrotask 与 Promise 同队列同序；\n\
         // clearTimeout 取消一次性、clearInterval 在回调内自我取消。\n\
         console.log('sync');\n\
         queueMicrotask(() => console.log('qm'));\n\
         Promise.resolve().then(() => console.log('promise'));\n\
         setTimeout(() => console.log('to0'), 0);\n\
         setTimeout((msg: string) => console.log('to10', msg), 10, 'extra');\n\
         let count = 0;\n\
         const iv = setInterval(() => {\n\
         \x20 count++;\n\
         \x20 console.log('tick', count);\n\
         \x20 if (count === 3) {\n\
         \x20   clearInterval(iv);\n\
         \x20 }\n\
         }, 20);\n\
         const cancelled = setTimeout(() => console.log('NEVER'), 5);\n\
         clearTimeout(cancelled);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(output.status.success(), "定时器用例应成功退出：{output:?}");
    assert_eq!(
        stdout.trim(),
        "sync\nqm\npromise\nto0\nto10 extra\ntick 1\ntick 2\ntick 3"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn event_loop_wakes_for_timers_while_fetch_in_flight() {
    let port = spawn_mock_server();
    let dir = unique_dir("timers-with-fetch");
    // 预算压到 300ms：/slow 不会回，定时器必须在 fetch 在途期间准点触发。
    std::fs::write(
        dir.join("mio.json"),
        "{\n  \"run\": { \"timeoutMs\": 300 }\n}\n",
    )
    .unwrap();
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "setTimeout(() => console.log('tick-30'), 30);\n\
             try {{\n\
             \x20   await fetch('http://127.0.0.1:{port}/slow');\n\
             \x20   console.log('RESOLVED');\n\
             }} catch (e) {{\n\
             \x20   console.log('rejected');\n\
             }}\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "定时器+fetch 混合用例应成功退出：{output:?}"
    );
    assert_eq!(stdout.trim(), "tick-30\nrejected");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_accepts_url_instance_as_input() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-url-input");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const u = new URL('http://127.0.0.1:{port}/hello');\n\
             const r = await fetch(u);\n\
             console.log(r.status, await r.text());\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "URL 入参用例应成功退出：{output:?}"
    );
    assert_eq!(stdout.trim(), "200 hello mio");

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn fetch_formdata_body_serializes_multipart_and_sets_content_type() {
    let port = spawn_mock_server();
    let dir = unique_dir("fetch-formdata");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        format!(
            "const fd = new FormData();\n\
             fd.append('a', '1');\n\
             const blob = new Blob(['XY'], {{ type: 'text/plain' }});\n\
             fd.append('f', blob, 'n.txt');\n\
             const r = await fetch('http://127.0.0.1:{port}/raw', {{ method: 'POST', body: fd }});\n\
             const t = await r.text();\n\
             console.log(\n\
             \x20 t.includes('multipart/form-data; boundary=----MioFormBoundary'),\n\
             \x20 t.includes('name=\"a\"'),\n\
             \x20 t.includes('filename=\"n.txt\"'),\n\
             \x20 t.includes('content-type: text/plain'),\n\
             \x20 t.includes('XY'),\n\
             );\n"
        ),
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "FormData 用例应成功退出：{output:?}"
    );
    assert_eq!(
        stdout.trim(),
        "true true true true true",
        "multipart 各要素应齐备：{stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn web_types_textencoder_blob_formdata_semantics() {
    let dir = unique_dir("web-types");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "const enc = new TextEncoder();\n\
         const bytes = enc.encode('中文a');\n\
         const dec = new TextDecoder();\n\
         console.log('roundtrip', dec.decode(bytes), bytes.length);\n\
         const blob = new Blob(['hello', ' 世界'], { type: 'Text/Plain' });\n\
         console.log('blob', blob.size, blob.type, await blob.text());\n\
         const ab = await blob.arrayBuffer();\n\
         console.log('ab', ab instanceof Uint8Array, ab[0]);\n\
         const fd = new FormData();\n\
         fd.append('a', '1');\n\
         fd.append('a', '2');\n\
         fd.append('file', blob, 'note.txt');\n\
         console.log('fd', fd.get('a'), fd.getAll('a').length, fd.has('nope'));\n\
         const file = fd.get('file') as File;\n\
         console.log('file', file.name, file.type, file.size);\n\
         fd.set('a', '9');\n\
         console.log('after set', fd.getAll('a').join(','));\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "Web 类型用例应成功退出：{output:?}"
    );
    assert_eq!(
        stdout.trim(),
        "roundtrip 中文a 7\nblob 12 text/plain hello 世界\nab true 104\nfd 1 2 false\nfile note.txt text/plain 12\nafter set 9"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn microtask_storm_is_stopped_by_run_budget() {
    let dir = unique_dir("microtask-storm");
    std::fs::write(dir.join("mio.json"), "{ \"run\": { \"timeoutMs\": 800 } }").unwrap();
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "export {};\n\
         const boom = (): void => {\n\
         \x20 queueMicrotask(boom);\n\
         };\n\
         boom();\n\
         console.log('entered host loop');\n",
    )
    .unwrap();

    let started = std::time::Instant::now();
    let output = run(&[
        "-c",
        dir.join("mio.json").to_str().unwrap(),
        "run",
        file.to_str().unwrap(),
    ]);
    let elapsed = started.elapsed();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "自递归的微任务必须被预算拦下，而不是无限排空：{output:?}"
    );
    assert!(stderr.contains("预算"), "错误应指明预算耗尽：{stderr}");
    assert_eq!(
        stdout.trim(),
        "entered host loop",
        "脚本体的输出应先于预算中止：{stdout}"
    );
    assert!(
        elapsed.as_secs() < 5,
        "预算 800ms 却在 {elapsed:?} 后才结束：看门狗未生效"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn finite_microtask_chain_still_completes_under_budget() {
    let dir = unique_dir("microtask-chain");
    std::fs::write(
        dir.join("mio.json"),
        "{ \"run\": { \"timeoutMs\": 15000 } }",
    )
    .unwrap();
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "export {};\n\
         const walk = async (): Promise<number> => {\n\
         \x20 let hops = 0;\n\
         \x20 for (let i = 0; i < 20000; i++) {\n\
         \x20   await Promise.resolve();\n\
         \x20   hops++;\n\
         \x20 }\n\
         \x20 return hops;\n\
         };\n\
         setTimeout(() => console.log('timer runs after the queue drains'), 5);\n\
         console.log('hops', await walk());\n",
    )
    .unwrap();

    let output = run(&[
        "-c",
        dir.join("mio.json").to_str().unwrap(),
        "run",
        file.to_str().unwrap(),
    ]);
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(
        output.status.success(),
        "有限长度的微任务接力不得被看门狗误伤：{output:?}"
    );
    // 微任务优先于定时器（与 Node 一致）：接力排空后定时器才有机会。
    assert_eq!(
        stdout.trim(),
        "hops 20000\ntimer runs after the queue drains"
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn performance_now_is_monotonic_with_sub_millisecond_resolution() {
    let dir = unique_dir("performance-now");
    let file = dir.join("app.ts");
    std::fs::write(
        &file,
        "export {};\n\
         const samples: number[] = [];\n\
         for (let i = 0; i < 200; i++) {\n\
         \x20 samples.push(performance.now());\n\
         }\n\
         let strict = true;\n\
         for (let i = 1; i < samples.length; i++) {\n\
         \x20 if (samples[i] <= samples[i - 1]) {\n\
         \x20   strict = false;\n\
         \x20 }\n\
         }\n\
         const t0 = performance.now();\n\
         while (performance.now() - t0 < 30) {}\n\
         console.log('strictlyIncreasing', strict);\n\
         console.log('busyWait>=30', performance.now() - t0 >= 30);\n\
         console.log('subMillisecond', (performance.now() % 1) !== 0);\n",
    )
    .unwrap();

    let output = run(&["run", file.to_str().unwrap()]);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        output.status.success(),
        "performance 用例应成功退出：{output:?}"
    );
    assert_eq!(
        stdout.trim(),
        "strictlyIncreasing true\nbusyWait>=30 true\nsubMillisecond true",
        "连续 200 次读数必须严格递增且带亚毫秒位：{stdout}"
    );

    std::fs::remove_dir_all(&dir).ok();
}
