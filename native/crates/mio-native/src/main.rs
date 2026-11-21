//! mio-native 命令行入口。
//!
//! 用法：`mio-native build <app.ts> -o <out> [--emit-obj]`

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() < 4 || args[1] != "build" {
        eprintln!("用法: mio-native build <app.ts> -o <out> [--emit-obj]");
        std::process::exit(2);
    }
    let src = args[2].clone();
    let mut out: Option<std::path::PathBuf> = None;
    let mut emit_obj = false;
    let mut i = 3;
    while i < args.len() {
        match args[i].as_str() {
            "-o" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("-o 后缺少输出路径");
                    std::process::exit(2);
                }
                out = Some(std::path::PathBuf::from(&args[i]));
            }
            "--emit-obj" => emit_obj = true,
            other => {
                eprintln!("未知参数: {other}");
                std::process::exit(2);
            }
        }
        i += 1;
    }
    let Some(out) = out else {
        eprintln!("用法: mio-native build <app.ts> -o <out> [--emit-obj]");
        std::process::exit(2);
    };
    if let Err(e) = mio_native::compile_file(&src, &out, emit_obj) {
        eprintln!("mio-native: {e}");
        std::process::exit(1);
    }
}
