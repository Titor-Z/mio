# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

## 0.1.0（未发布）

### 立项与仓库形态

- 新建 `mio` 仓库，定位为 TypeScript 一等公民的运行时：TypeScript 源文件直接作为输入语言，运行时内部完成类型擦除与执行。
- 项目分两阶段：阶段 A 借助 OXC 与 Boa 完成转译与执行；阶段 B 自研词法、语法、语义与字节码虚拟机，直接消费类型信息并输出原生代码。
- 明确不提供 Node.js 与 CommonJS 兼容层：不解析 `node_modules`、不支持 `require`、不推断 `package.json` 入口、不支持裸包名。模块图因此完全静态可分析。
- 技术栈固定为 Rust 1.97.1、Boa 0.22、OXC 0.156，均可在稳定版 Rust 上编译，无需 nightly。

### 配置系统

- 配置文件 `mio.json` 采用 `tsconfig.json` 骨架：顶层 `compilerOptions`、`include` / `exclude` / `files` 文件选择、`extends` 继承、JSONC 注释、`$schema` 提示。
- 模块映射不采用 `compilerOptions.paths` / `baseUrl`：二者为编译期概念，不影响产物且依赖打包器配合。改用 Import Maps 规范的 `imports` / `scopes`，支持精确映射与最长前缀映射。映射目标为 `npm:` / `node:` / 裸包名时给出明确报错。
- 字段分四档生效状态：生效、仅提示、预留、不采用。后两档在加载时打印告警，避免「写进配置却无人读取」。
- `extends` 在 JSON 值层面深合并，对象逐字段合并、数组整体覆盖；支持字符串与数组形态、省略 `.json` 后缀、检出循环继承、链深度上限 16。
- 配置发现从入口文件所在目录向上查找最近一份，找到即止；`--config` 可强制指定。
- 优先级：内建默认 < 配置文件（含继承链） < `MIO_*` 环境变量 < 命令行选项。
- 字符串支持 `$VAR` 与 `${VAR}` 展开；未定义变量原样保留，不替换为空串。
- 未知字段不导致失败，但输出「未知字段 + 编辑距离最接近的合法字段」告警。
- 通配匹配以路径段为单位实现，支持 `*`、`?`、`**`，配合记忆化避免指数级回溯。
- `mio task` 可读取配置中声明的任务并按名列出。

### 工程骨架

- 单 crate，edition 2024，模块按面向对象分层：`cli`、`config`、`transpiler`、`resolver`、`loader`、`engine`、`sourcemap`、`web`、`stdlib`。
- 建立两个阶段共用的接缝：`engine::Engine` 执行引擎抽象，`resolver::Specifier` 说明符分类。
- `engine::Engine` 内置 `boa_*` 依赖隔离约束，其他模块不得直接引用具体引擎。
- 命令行支持 `--version` 与 `--help`，其余子命令返回未实现提示。
- 测试 64 项通过（57 单元 + 7 端到端），`clippy -D warnings` 与 `cargo fmt --check` 均为干净状态。
