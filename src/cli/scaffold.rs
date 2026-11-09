//! 项目脚手架与类型声明生成：`mio init` / `mio types`。
//!
//! 用户项目里的 `.mio/@types/mio/` 是生成物，内容直接来自编译进二进制的
//! `types/mio/` 源码，因此运行时升级后声明不会与实现脱节。

use std::fs;
use std::path::{Path, PathBuf};

use super::CliResult;

/// 全局类型声明（编译期嵌入，路径相对本文件）。
const TYPES_INDEX: &str = include_str!("../../types/mio/index.d.ts");

/// `@std/path` 类型声明。
const STD_PATH: &str = include_str!("../../types/mio/std/path.d.ts");

/// `@std/fs` 类型声明。
const STD_FS: &str = include_str!("../../types/mio/std/fs.d.ts");

/// 初始化生成的运行配置。
const MIO_JSON: &str = r#"{
  // mio 运行配置：影响运行行为的唯一真相。
  // 编辑器类型提示见 tsconfig.json（由 mio init 生成）。
  "compilerOptions": {
    "target": "es2025"
  },
  "include": ["src/**/*.ts"],
  "run": {
    "timeoutMs": 30000
  }
}
"#;

/// 初始化生成的编辑器配置。只服务于 TypeScript LSP，不参与运行。
const TSCONFIG: &str = r#"{
  // 由 mio init 生成：仅供编辑器类型提示，运行配置以 mio.json 为准。
  "compilerOptions": {
    "target": "ESNext",
    "module": "ESNext",
    "moduleResolution": "bundler",
    // 关键：不要 DOM，mio 没有 window/document
    "lib": ["ESNext"],
    "typeRoots": [".mio/@types"],
    "types": ["mio"],
    "strict": true,
    // mio 支持直接 import "./x.ts"
    "allowImportingTsExtensions": true,
    "noEmit": true
  },
  "include": ["src/**/*.ts"]
}
"#;

/// 初始化生成的入口示例。
const MAIN_TS: &str = r#"const message: string = "Hello, mio!";
console.log(message);
"#;

/// `.gitignore` 初始内容：类型声明应提交，故默认不忽略任何目录。
const GITIGNORE: &str = "# 项目级忽略规则写在这里\n";

/// `mio init`：在当前目录补齐项目骨架，已存在的配置不覆盖。
pub(super) fn init_current_dir() -> CliResult {
    let root = std::env::current_dir().map_err(|error| format!("无法获取当前目录：{error}"))?;

    ensure_file(&root.join("mio.json"), MIO_JSON)?;
    ensure_file(&root.join("tsconfig.json"), TSCONFIG)?;
    ensure_file(&root.join("src/main.ts"), MAIN_TS)?;
    ensure_file(&root.join(".gitignore"), GITIGNORE)?;
    write_types(&root)?;

    println!("mio 项目已就绪：");
    println!("  mio.json            运行配置");
    println!("  tsconfig.json       编辑器类型提示（生成的适配层）");
    println!("  .mio/@types/mio/    运行时类型声明");
    println!("  src/main.ts         入口示例");
    Ok(())
}

/// `mio types`：只刷新 `.mio/@types/mio/`，用于运行时升级后同步声明。
pub(super) fn refresh_types_current_dir() -> CliResult {
    let root = std::env::current_dir().map_err(|error| format!("无法获取当前目录：{error}"))?;
    write_types(&root)?;
    println!("已刷新 .mio/@types/mio/");
    Ok(())
}

/// 把内置声明落到进程级临时目录，返回 `index.d.ts` 路径。
///
/// 供 `mio check` 作为内置 lib 传给 tsrs：按版本分目录，不做项目目录写入。
pub(super) fn materialize_builtin_types() -> Result<PathBuf, String> {
    let dir = std::env::temp_dir().join(format!("mio-types-{}", crate::VERSION));
    let index = dir.join("mio/index.d.ts");
    write_file(&index, TYPES_INDEX)?;
    write_file(&dir.join("mio/std/path.d.ts"), STD_PATH)?;
    write_file(&dir.join("mio/std/fs.d.ts"), STD_FS)?;
    Ok(index)
}

/// 写入类型声明，总是覆盖（生成物以运行时自带版本为准）。
fn write_types(root: &Path) -> CliResult {
    write_file(&root.join(".mio/@types/mio/index.d.ts"), TYPES_INDEX)?;
    write_file(&root.join(".mio/@types/mio/std/path.d.ts"), STD_PATH)?;
    write_file(&root.join(".mio/@types/mio/std/fs.d.ts"), STD_FS)
}

/// 文件不存在才写入，避免覆盖用户已有配置。
fn ensure_file(path: &Path, contents: &str) -> CliResult {
    if path.exists() {
        return Ok(());
    }
    write_file(path, contents)
}

/// 写入文件并自动创建父目录。
fn write_file(path: &Path, contents: &str) -> CliResult {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("无法创建目录 {}：{error}", parent.display()))?;
    }
    fs::write(path, contents).map_err(|error| format!("无法写入 {}：{error}", path.display()))
}
