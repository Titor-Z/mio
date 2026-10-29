# mio

TypeScript 一等公民的运行时。TypeScript 源文件直接作为输入语言，运行时内部完成类型擦除与执行。
不提供 Node.js / CommonJS / `node_modules` 兼容层，也不解析 `package.json` 入口。

## 定位

| 项 | 说明 |
|---|---|
| 输入语言 | TypeScript（JavaScript 视为其子集，单一文法路径） |
| 模块体系 | 纯 ES Module；相对路径、绝对路径、`file:` URL、`imports` 映射、`@std/*` 内建命名空间 |
| 包管理 | 无。不查找 `node_modules`，不支持裸包名 |
| 标准库 | Web 标准库（全局对象）+ `@std/*`（模块导入） |
| 类型处理 | 类型注解与类型声明在编译期擦除；`enum` / `namespace` / 参数属性降级为等价 JavaScript |

## 两阶段路线

- **阶段 A（当前）**：TypeScript 源码 → 类型擦除 → 执行。转译由 OXC 承担，执行由 Boa 承担，两者分别隔离在 `transpiler` 与 `engine` 模块内。
- **阶段 B（预留）**：自研词法、语法、语义与字节码虚拟机，直接消费类型信息并输出原生代码。

两阶段之间只保留两个接缝：`engine::Engine` 抽象与 `resolver::Specifier` 分类。其余模块在两个阶段之间复用。

## 构建

```bash
cargo build            # 调试构建
cargo test             # 单元测试
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

工具链固定为 Rust 1.97.1（见 `rust-toolchain.toml`）。OXC 0.156 与 Boa 0.22 均可在稳定版 Rust 上编译，无需 nightly。

## 使用

```bash
mio run app.ts         # 执行 TypeScript 文件
mio eval "..."         # 执行内联源码
mio repl               # 交互式求值
mio check app.ts       # 仅类型检查（依赖外部 tsgo）
mio task <name>        # 执行 mio.json 声明的任务
```

## 目录结构

```
src/
  main.rs        进程入口：解析参数、分发命令、转换退出码
  lib.rs         模块树与全局常量
  cli/           命令行参数解析与命令分发
  config/        mio.json 解析、发现与优先级叠加
  transpiler/    TypeScript → JavaScript（阶段 A 由 OXC 承担）
  resolver/      模块说明符分类与 Import Maps 重写
  loader/        模块实例缓存与加载
  engine/        执行引擎抽象（阶段 A 为 Boa）
  sourcemap/     错误位置映射回 TypeScript 源码
  web/           Web 标准库（全局对象）
  stdlib/        @std/* 内建模块
```

## 配置

配置文件为 `mio.json`，支持 JSONC 注释，从入口文件所在目录向上查找最近一份。
字段优先级：内建默认 < `mio.json` < `MIO_*` 环境变量 < 命令行选项。

## 与 Node.js / Deno 的边界

不实现的能力是设计选择，不是缺失：

| 能力 | 状态 | 原因 |
|---|---|---|
| `node_modules` 解析 | 不支持 | 无包管理概念 |
| CommonJS `require` | 不支持 | 单一 ES Module 路径 |
| `package.json` 入口推断 | 不支持 | 模块说明符必须显式 |
| 裸包名（如 `lodash`） | 不支持 | 加载时给出明确报错并建议改为相对路径或 `@std/*` |

## 许可

MIT
