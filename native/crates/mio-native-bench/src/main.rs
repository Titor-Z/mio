//! 三方基准：mio-native 产物 vs node vs `cc -O2`。
//!
//! 用例：`fib(35)` 递归、整数循环累加 1e8、浮点循环累加 1e8。
//! 每个用例三份同语义源码（TS / C / JS）内联于本文件：
//! - TS 经 `mio-native build` 编译为本机可执行文件；
//! - C 经 `cc -O2` 编译；
//! - JS 由 node 直接执行（不编译）。
//!
//! 计时：编译各 1 次；执行各 3 次取最优（降噪）。
//!
//! 门槛见 `docs/语法讨论书.md` 与 TL20261110T041429：
//! fib(35) ≥ 3x Node、循环 ≤ 2x vs C、编译 ≤ 1s。

use std::path::PathBuf;
use std::process::Command;
use std::time::Instant;

/// 一个用例的三方同语义源码。
struct Case {
    name: &'static str,
    ts: &'static str,
    c: &'static str,
    js: &'static str,
}

const CASES: &[Case] = &[
    Case {
        name: "fib(35)",
        ts: "function fib(n: i64): i64 {\n\
             \x20 if (n < 2) {\n\
             \x20   return n;\n\
             \x20 }\n\
             \x20 return fib(n - 1) + fib(n - 2);\n\
             }\n\
             function main(): void {\n\
             \x20 print(fib(35));\n\
             }\n",
        c: "#include <stdio.h>\n\
            long long fib(long long n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }\n\
            int main() { printf(\"%lld\\n\", fib(35)); return 0; }\n",
        js: "function fib(n) { if (n < 2) return n; return fib(n - 1) + fib(n - 2); }\n\
             console.log(fib(35));\n",
    },
    Case {
        name: "loop 1e8 int",
        ts: "function main(): void {\n\
             \x20 let sum: i64 = 0;\n\
             \x20 for (let i: i64 = 0; i < 100000000; i = i + 1) {\n\
             \x20   sum = sum + i;\n\
             \x20 }\n\
             \x20 print(sum);\n\
             }\n",
        c: "#include <stdio.h>\n\
            int main() { long long sum = 0; for (long long i = 0; i < 100000000; i = i + 1) sum = sum + i; printf(\"%lld\\n\", sum); return 0; }\n",
        js: "let sum = 0; for (let i = 0; i < 100000000; i = i + 1) sum = sum + i; console.log(sum);\n",
    },
    Case {
        name: "loop 1e8 call",
        ts: "function add(a: i64, b: i64): i64 {\n\
             \x20 return a + b;\n\
             }\n\
             function main(): void {\n\
             \x20 let sum: i64 = 0;\n\
             \x20 for (let i: i64 = 0; i < 100000000; i = i + 1) {\n\
             \x20   sum = add(sum, i);\n\
             \x20 }\n\
             \x20 print(sum);\n\
             }\n",
        c: "#include <stdio.h>\n\
            static inline long long add(long long a, long long b) { return a + b; }\n\
            int main() { long long sum = 0; for (long long i = 0; i < 100000000; i = i + 1) sum = add(sum, i); printf(\"%lld\\n\", sum); return 0; }\n",
        js: "function add(a, b) { return a + b; }\n\
             let sum = 0; for (let i = 0; i < 100000000; i = i + 1) sum = add(sum, i); console.log(sum);\n",
    },
    Case {
        name: "loop 1e8 control",
        ts: "function abs(x: i64): i64 {\n\
             \x20 if (x < 0) {\n\
             \x20   return -x;\n\
             \x20 }\n\
             \x20 return x;\n\
             }\n\
             function add(a: i64, b: i64): i64 {\n\
             \x20 return a + b;\n\
             }\n\
             function main(): void {\n\
             \x20 let sum: i64 = 0;\n\
             \x20 for (let i: i64 = 0; i < 100000000; i = i + 1) {\n\
             \x20   let d: i64 = i - 50;\n\
             \x20   let a: i64 = abs(d);\n\
             \x20   sum = add(sum, a);\n\
             \x20 }\n\
             \x20 print(sum);\n\
             }\n",
        c: "#include <stdio.h>\n\
            static inline long long absc(long long x) { return x < 0 ? -x : x; }\n\
            static inline long long add(long long a, long long b) { return a + b; }\n\
            int main() { long long sum = 0; for (long long i = 0; i < 100000000; i = i + 1) { long long d = i - 50; sum = add(sum, absc(d)); } printf(\"%lld\\n\", sum); return 0; }\n",
        js: "function abs(x) { return x < 0 ? -x : x; }\n\
             function add(a, b) { return a + b; }\n\
             let sum = 0; for (let i = 0; i < 100000000; i = i + 1) { const d = i - 50; sum = add(sum, abs(d)); } console.log(sum);\n",
    },
    Case {
        name: "loop 1e8 float",
        ts: "function main(): void {\n\
             \x20 let acc: f64 = 0.0;\n\
             \x20 for (let i: i64 = 0; i < 100000000; i = i + 1) {\n\
             \x20   acc = acc + 1.5;\n\
             \x20 }\n\
             \x20 print(acc);\n\
             }\n",
        c: "#include <stdio.h>\n\
            int main() { double acc = 0.0; for (long long i = 0; i < 100000000; i = i + 1) acc = acc + 1.5; printf(\"%g\\n\", acc); return 0; }\n",
        js: "let acc = 0.0; for (let i = 0; i < 100000000; i = i + 1) acc = acc + 1.5; console.log(acc);\n",
    },
];

/// 定位 mio-native 二进制：优先环境变量 `MIO_NATIVE_BIN`，其次 workspace 内的
/// release / debug 产物（bench 的 cwd 是本 crate 目录）。
fn mio_native_bin() -> PathBuf {
    if let Ok(path) = std::env::var("MIO_NATIVE_BIN") {
        return PathBuf::from(path);
    }
    // CARGO_MANIFEST_DIR 是编译期常量（本 crate 目录），与运行时 cwd 无关。
    for candidate in [
        "../../target/release/mio-native",
        "../../target/debug/mio-native",
    ] {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(candidate);
        if path.exists() {
            return path;
        }
    }
    eprintln!(
        "找不到 mio-native 二进制：请先 cargo build -p mio-native --release，或设 MIO_NATIVE_BIN"
    );
    std::process::exit(2);
}

/// 计时执行一条命令，返回 (耗时, stdout)。
fn timed(program: &str, args: &[&str]) -> (f64, String) {
    let start = Instant::now();
    let output = Command::new(program)
        .args(args)
        .output()
        .unwrap_or_else(|error| panic!("无法启动 {program}：{error}"));
    let elapsed = start.elapsed().as_secs_f64();
    if !output.status.success() {
        panic!(
            "{program} 执行失败：{}\n{}",
            output.status,
            String::from_utf8_lossy(&output.stderr)
        );
    }
    (
        elapsed,
        String::from_utf8_lossy(&output.stdout).trim().to_string(),
    )
}

fn main() {
    let bin = mio_native_bin();
    let dir = std::env::temp_dir().join("mio-native-bench");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).expect("创建基准目录失败");

    println!(
        "{:<14} {:>10} {:>10} │ {:>10} {:>10} {:>10}",
        "用例", "编译(mio)", "编译(cc)", "运行(mio)", "运行(node)", "运行(cc)"
    );
    println!("{}", "-".repeat(78));

    for case in CASES {
        let ts_path = dir.join(format!("{}.ts", slug(case.name)));
        let c_path = dir.join(format!("{}.c", slug(case.name)));
        let js_path = dir.join(format!("{}.js", slug(case.name)));
        let mio_out = dir.join(format!("{}.mio.out", slug(case.name)));
        let cc_out = dir.join(format!("{}.cc.out", slug(case.name)));
        std::fs::write(&ts_path, case.ts).expect("写 TS 失败");
        std::fs::write(&c_path, case.c).expect("写 C 失败");
        std::fs::write(&js_path, case.js).expect("写 JS 失败");

        // 编译计时：mio-native（release 二进制）与 cc -O2。
        let (mio_compile, _) = timed(
            bin.to_str().unwrap(),
            &[
                "build",
                ts_path.to_str().unwrap(),
                "-o",
                mio_out.to_str().unwrap(),
            ],
        );
        let (cc_compile, _) = timed(
            "cc",
            &[
                "-O2",
                c_path.to_str().unwrap(),
                "-o",
                cc_out.to_str().unwrap(),
            ],
        );

        // 执行计时：各 3 次取最优。
        let mio_run = best_of(3, || timed(mio_out.to_str().unwrap(), &[]).0);
        let node_run = best_of(3, || timed("node", &[js_path.to_str().unwrap()]).0);
        let cc_run = best_of(3, || timed(cc_out.to_str().unwrap(), &[]).0);

        // 输出正确性交叉验证：三方数值必须一致（浮点格式串差异不比较文本）。
        let (_, mio_stdout) = timed(mio_out.to_str().unwrap(), &[]);
        let (_, node_stdout) = timed("node", &[js_path.to_str().unwrap()]);
        let (_, cc_stdout) = timed(cc_out.to_str().unwrap(), &[]);
        let values = [
            parse_number(&mio_stdout, case.name, "mio-native"),
            parse_number(&node_stdout, case.name, "node"),
            parse_number(&cc_stdout, case.name, "cc"),
        ];
        for window in values.windows(2) {
            assert!(
                (window[0] - window[1]).abs() <= 1e-6 * window[0].abs().max(1.0),
                "{}：三方数值不符：{values:?}",
                case.name
            );
        }

        println!(
            "{:<14} {:>9.2}s {:>9.2}s │ {:>9.2}s {:>9.2}s {:>9.2}s",
            case.name, mio_compile, cc_compile, mio_run, node_run, cc_run
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
    println!("\n输出正确性已三方交叉验证一致；执行耗时取 3 次最优。");
}

/// 执行 n 次取最优耗时。
fn best_of(n: usize, mut run: impl FnMut() -> f64) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..n {
        best = best.min(run());
    }
    best
}

/// 把 stdout 解析为数值（整数与浮点统一按 f64 比较）。
fn parse_number(text: &str, case: &str, who: &str) -> f64 {
    text.trim()
        .parse::<f64>()
        .unwrap_or_else(|_| panic!("{case}：{who} 的输出不是数值：{text}"))
}

/// 用例名 → 文件名安全片段。
fn slug(name: &str) -> String {
    name.chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '_' })
        .collect()
}
