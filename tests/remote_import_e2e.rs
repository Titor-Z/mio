//! 端到端测试：远程 URL 导入与下载缓存（`TL20261123T053153`）。
//!
//! 全部用例都在本机 mock HTTP 服务器上跑，不依赖公网；`HOME` 与工作目录均指向
//! 临时目录，既隔离 `~/.mio/deps/` 缓存，也避免往仓库根写 `mio.lock`。

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

/// 一个 mock 路由：状态码、content-type、响应体。
struct Route {
    status: u16,
    content_type: &'static str,
    body: String,
}

/// 启动 mock 服务器，返回端口与请求计数器（计数器用来断言缓存是否真的避免了网络）。
fn spawn_server(routes: Vec<(&'static str, Route)>) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("mock 服务器应能绑定 ephemeral 端口");
    let port = listener
        .local_addr()
        .expect("mock 服务器应有本地地址")
        .port();
    let counter = Arc::new(AtomicUsize::new(0));
    let hits = Arc::clone(&counter);
    let table: HashMap<String, Route> = routes
        .into_iter()
        .map(|(path, route)| (path.to_string(), route))
        .collect();

    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let mut reader = BufReader::new(stream.try_clone().expect("克隆连接应成功"));
            let mut request_line = String::new();
            if reader.read_line(&mut request_line).is_err() {
                continue;
            }
            // 吞掉请求头直到空行。
            loop {
                let mut line = String::new();
                if reader.read_line(&mut line).unwrap_or(0) == 0 || line == "\r\n" {
                    break;
                }
            }
            hits.fetch_add(1, Ordering::SeqCst);

            let path = request_line.split_whitespace().nth(1).unwrap_or("/");
            let response = match table.get(path) {
                Some(route) => format!(
                    "HTTP/1.1 {} X\r\ncontent-type: {}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    route.status,
                    route.content_type,
                    route.body.len(),
                    route.body
                ),
                None => String::from(
                    "HTTP/1.1 404 Not Found\r\ncontent-length: 0\r\nconnection: close\r\n\r\n",
                ),
            };
            let _ = stream.write_all(response.as_bytes());
            let _ = stream.flush();
        }
    });

    (port, counter)
}

/// 一个隔离的运行环境：独立 HOME（缓存）、独立工作目录（mio.lock）。
struct Sandbox {
    home: PathBuf,
    work: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let root = unique_dir(tag);
        let home = root.join("home");
        let work = root.join("work");
        std::fs::create_dir_all(&home).expect("创建临时 HOME 失败");
        std::fs::create_dir_all(&work).expect("创建工作目录失败");
        Self { home, work }
    }

    /// 在当前沙箱里执行一次 `mio`。
    fn run(&self, args: &[&str]) -> Output {
        Command::new(mio_binary())
            .args(args)
            .current_dir(&self.work)
            .env("HOME", &self.home)
            .output()
            .expect("mio 二进制应能被调用")
    }

    /// 写一个本地文件到工作目录。
    fn write(&self, name: &str, content: &str) -> PathBuf {
        let path = self.work.join(name);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("创建子目录失败");
        }
        std::fs::write(&path, content).expect("写入文件失败");
        path
    }

    /// 缓存里是否已有某个文件（按扩展名统计）。
    fn cached_files(&self) -> Vec<PathBuf> {
        let mut found = Vec::new();
        collect(&self.home.join(".mio").join("deps"), &mut found);
        found
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        if let Some(root) = self.home.parent() {
            std::fs::remove_dir_all(root).ok();
        }
    }
}

fn mio_binary() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_mio"))
}

fn unique_dir(tag: &str) -> PathBuf {
    let mut dir = std::env::temp_dir();
    dir.push(format!(
        "mio-remote-e2e-{tag}-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("system time")
            .as_nanos()
    ));
    dir
}

/// 递归收集目录下的文件名（只看文件名，用例据此判断缓存内容）。
fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect(&path, out);
        } else {
            out.push(path);
        }
    }
}

/// 两个模块：入口 `main.ts` 相对导入 `lib/util.ts`。
fn two_module_routes() -> Vec<(&'static str, Route)> {
    vec![
        (
            "/main.ts",
            Route {
                status: 200,
                // 故意用 python http.server 给 `.ts` 的 `video/mp2t`：末段是源码扩展名时忽略。
                content_type: "video/mp2t",
                body: String::from(
                    "import { greet } from \"./lib/util.ts\";\nconsole.log(greet(\"远程\"));\n",
                ),
            },
        ),
        (
            "/lib/util.ts",
            Route {
                status: 200,
                content_type: "video/mp2t",
                body: String::from(
                    "export function greet(name: string): string {\n  return `你好，${name}`;\n}\n",
                ),
            },
        ),
    ]
}

#[test]
fn downloads_then_serves_from_cache() {
    let (port, hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("cache");
    let url = format!("http://127.0.0.1:{port}/main.ts");

    let first = sandbox.run(&["run", &url]);
    assert!(
        first.status.success(),
        "首跑应成功：{}",
        String::from_utf8_lossy(&first.stderr)
    );
    assert!(String::from_utf8_lossy(&first.stdout).contains("你好，远程"));
    assert_eq!(hits.load(Ordering::SeqCst), 2, "入口与依赖应各下载一次");

    // 缓存里有源码与元数据各两份。
    let cached = sandbox.cached_files();
    assert_eq!(
        cached
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "ts"))
            .count(),
        2
    );
    assert_eq!(
        cached
            .iter()
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .count(),
        2
    );

    // 二跑：服务器一次都不该被访问。
    let second = sandbox.run(&["run", &url]);
    assert!(second.status.success());
    assert!(String::from_utf8_lossy(&second.stdout).contains("你好，远程"));
    assert_eq!(
        hits.load(Ordering::SeqCst),
        2,
        "第二次运行必须全部命中缓存，不得发起网络请求"
    );
}

#[test]
fn reload_forces_refetch() {
    let (port, hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("reload");
    let url = format!("http://127.0.0.1:{port}/main.ts");

    assert!(sandbox.run(&["run", &url]).status.success());
    assert_eq!(hits.load(Ordering::SeqCst), 2);

    assert!(sandbox.run(&["run", "--reload", &url]).status.success());
    assert_eq!(
        hits.load(Ordering::SeqCst),
        4,
        "--reload 应重下全部远程模块"
    );
}

#[test]
fn cached_only_reports_miss() {
    let (port, _hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("cached-only");
    let url = format!("http://127.0.0.1:{port}/main.ts");

    let output = sandbox.run(&["run", "--cached-only", &url]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("--cached-only"),
        "应提示严格离线未命中：{stderr}"
    );
}

#[test]
fn no_remote_rejects_url_specifier() {
    let (port, hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("no-remote");
    let url = format!("http://127.0.0.1:{port}/main.ts");

    let output = sandbox.run(&["run", "--no-remote", &url]);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--no-remote"));
    assert_eq!(hits.load(Ordering::SeqCst), 0, "禁用后不得发起任何网络请求");
}

#[test]
fn http_error_is_reported() {
    let (port, _hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("404");
    let url = format!("http://127.0.0.1:{port}/missing.ts");

    let output = sandbox.run(&["run", &url]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("404"), "应报出状态码：{stderr}");
}

#[test]
fn lock_file_is_generated_and_frozen_checks_it() {
    let (port, _hits) = spawn_server(two_module_routes());
    let sandbox = Sandbox::new("lock");
    let url = format!("http://127.0.0.1:{port}/main.ts");

    assert!(sandbox.run(&["run", &url]).status.success());
    let lock = sandbox.work.join("mio.lock");
    let content = std::fs::read_to_string(&lock).expect("应生成 mio.lock");
    assert!(content.contains(url.as_str()));
    assert!(content.contains("sha256-"));

    // 未改动时 --frozen 通过。
    let ok = sandbox.run(&["run", "--frozen", &url]);
    assert!(
        ok.status.success(),
        "--frozen 在内容一致时应通过：{}",
        String::from_utf8_lossy(&ok.stderr)
    );

    // 改动锁文件中的哈希后应失败。
    let tampered = content.replacen("sha256-", "sha256-0000", 1);
    std::fs::write(&lock, tampered).unwrap();
    let bad = sandbox.run(&["run", "--frozen", &url]);
    assert!(!bad.status.success(), "哈希不符时 --frozen 必须失败");
    assert!(String::from_utf8_lossy(&bad.stderr).contains("不一致"));
}

#[test]
fn import_map_can_point_at_url() {
    let (port, hits) = spawn_server(vec![(
        "/lib/util.ts",
        Route {
            status: 200,
            content_type: "application/typescript",
            body: String::from("export const value: number = 42;\n"),
        },
    )]);
    let sandbox = Sandbox::new("importmap");
    sandbox.write(
        "mio.json",
        &format!(
            "{{\n  \"imports\": {{\n    \"@lib/\": \"http://127.0.0.1:{port}/lib/\"\n  }}\n}}\n"
        ),
    );
    let entry = sandbox.write(
        "main.ts",
        "import { value } from \"@lib/util.ts\";\nconsole.log(value);\n",
    );

    let output = sandbox.run(&["run", entry.to_str().expect("路径应为 UTF-8")]);
    assert!(
        output.status.success(),
        "别名指向 URL 应可运行：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stdout).contains("42"));
    assert_eq!(hits.load(Ordering::SeqCst), 1);
}
