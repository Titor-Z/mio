# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

## 0.1.0（未发布）

### 立项与仓库形态

- 新建 `mio` 仓库，定位为 TypeScript 一等公民的运行时：TypeScript 源文件直接作为输入语言，运行时内部完成类型擦除与执行。
- 项目分两阶段：阶段 A 借助 OXC 与 Boa 完成转译与执行；阶段 B 自研词法、语法、语义与字节码虚拟机，直接消费类型信息并输出原生代码。
- 明确不提供 Node.js 与 CommonJS 兼容层：不解析 `node_modules`、不支持 `require`、不推断 `package.json` 入口、不支持裸包名。模块图因此完全静态可分析。
- 技术栈固定为 Rust 1.97.1、Boa 0.22、OXC 0.156，均可在稳定版 Rust 上编译，无需 nightly。

### 工程骨架

- 单 crate，edition 2024，模块按面向对象分层：`cli`、`config`、`transpiler`、`resolver`、`loader`、`engine`、`sourcemap`、`web`、`stdlib`。
- 建立两个阶段共用的接缝：`engine::Engine` 执行引擎抽象，`resolver::Specifier` 说明符分类。
- `engine::Engine` 内置 `boa_*` 依赖隔离约束，其他模块不得直接引用具体引擎。
- 命令行支持 `--version` 与 `--help`，其余子命令返回未实现提示。
- 单元测试 14 项通过，`clippy -D warnings` 与 `cargo fmt --check` 均为干净状态。
