# 版本更新记录

本文件按版本号倒序记录全部发布内容。
每条记录包含版本号、更新内容、以及本次发布状态涉及的 TodoID 聚合。

TodoID 规则：`TD` + UUID。TaolunID 规则：`TL` + `YYYYMMDDThhmmss`。

---

## 0.1.0（未发布）

**状态**：开发中，尚未发布。

**更新内容**

- 项目立项，确定定位为 TypeScript 一等公民的运行时，不做 Node.js 与 CommonJS 兼容层。
- 确定两阶段路线：阶段 A 为 TypeScript 转译后执行，阶段 B 为自研前端与字节码虚拟机输出原生代码。
- 确定类型系统扩展策略：标准 TypeScript 不裁剪，追加机器类型与结构类型；`number` 与机器类型之间的转换规则确定；类型表示分 `Unboxed` / `Managed` / `Boxed` 三轨。
- 确定配置系统形态：`mio.json`（JSONC），借鉴 `deno.json` 骨架，`imports` 与 `scopes` 采用 Import Maps 规范，字段分生效、仅提示、预留三档。
- 建立仓库骨架：单 crate、edition 2024、Rust 1.97.1、Boa 0.22、OXC 0.156。
- 建立模块分层与两个阶段接缝（`engine::Engine`、`resolver::Specifier`）。
- 命令行支持 `--version` 与 `--help`。
- 单元测试 14 项通过，clippy 与 fmt 干净。

**TodoLists[]**

- TD48d4b0c036ac4ecd989f487505fde111（立项与仓库形态）

**关联讨论**

- TL20261030T041103
