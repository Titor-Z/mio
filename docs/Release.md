# 版本更新记录

本文件按版本号倒序记录全部发布内容。
每条记录包含版本号、更新内容、以及本次发布状态涉及的 TodoID 聚合。

TodoID 规则：`TD` + UUID。TaolunID 规则：`TL` + `YYYYMMDDThhmmss`。

---

## 0.6.0（2026-11-23）

**状态**：已发布。

**更新内容**

- 远程 URL 导入：`import "https://…"` 直接可用，支持本地入口 import 远程、`imports` 别名指向 URL、`mio run <url>` 跑远程入口。
- 远程模块下载缓存：定死 `~/.mio/deps/<scheme>/<host>/<name>_<hash8>.{ts,json}`，原子写入，元数据含 etag/lastModified/fetchedAt/sha256，单模块上限 10 MiB；cache-first（命中即用、永不主动联网），`--reload` / `--reload=<url>` / `--revalidate` / `--cached-only` / `--no-remote` 显式控制。
- `mio.lock` 默认生成（URL → sha256，键排序稳定、只记不可变事实），`--frozen` 只校验不写入，供 CI 保证确定性。
- Web 标准对象对齐 W3C：`Request` 成为全局构造器（`new Request(input, init)`，`serve` 注入对象与之同类，`instanceof` 成立）；`Headers` / `Response` 迁为带原型的类；`Response.headers` 改为 live 视图；`new Response(字符串)` 自动补 `text/plain;charset=UTF-8`；`Response` 补 `clone()` / `type` / `redirected`。
- 合并两套 Response：`fetch()` 返回标准 `Response`，`fetch()` 入参扩到 `string | URL | Request`；`serve(() => fetch(...))` 可直接写线。
- 修复框架在 `await next()` 之后设置响应头不生效的问题（根因是 `Response.headers` 为快照拷贝）。
- 类型声明与手册同步（新增「从 URL 导入远程模块」章节、兼容性表新增远程导入一行、`mio init` 示例补充提示）。

**TodoLists[]**

- TD617f415d0ea84c20973c90b8c1e1623b（远程 URL 导入与下载缓存）
- TD74395b0e08ee47b8b61dfb78440e54cf（`mio.lock` 完整性校验）
- TDb0ddb60903584af8a1a6803a2168d89c（Request 全局构造器 / Headers·Response 原型化）
- TD17785f3a38fb4af0a49d95f8f9c75e84（合并两套 Response）

**关联讨论**

- TL20261123T053153（远程 URL 导入与下载缓存设计定稿）
- TL20261123T060717（Web 标准对象对齐 W3C）

---

## 0.5.0（2026-11-22）

**状态**：已发布。

**更新内容**

- `mio build` 支持多文件 `import`（相对路径、扩展名可省、具名导入；循环导入解析期检出；裸包名/别名/默认与命名空间导入明确报错）。
- `mio build` 的语法 desugar：`switch`（穿透）、`do-while`、复合赋值（`+= -= *= /= %=`）。
- 修复零周期 `setInterval` 在超时边界的假红竞态（约 1/3 概率误报「微任务队列未能排空」）：超时报错加「无未决宿主工作」条件，风暴诊断保留。

**TodoLists[]**

- TD89f5bfffd00142c1860e76121ae1ca7c（多文件 import 与 desugar）
- TDbbd019102ed44d07b9783ae4d2d9d20e（零周期 interval 超时边界假红修复）

**关联讨论**

- TL20261122T033801（多文件 import 与 desugar）
- TL20261122T034805（flake 修复）

---

## 0.4.0（2026-11-22）

**状态**：已发布。

**更新内容**

- 新增 `mio build`：TypeScript 子集 → 本机可执行文件（阶段 B 原生后端收编为主 CLI 子命令，独立模块 + feature `native` 门控，默认开启）。
- 原生后端诊断结构化（`BuildError`）：降低期错误精确到行列；类型检查期错误定位到函数声明行；渲染为 rustc 风格源码块。
- 原生后端小函数内联扩展到带控制流的函数（if/while/for/提前 return；拒绝循环体内的 return）。
- 手册新增「编译为原生可执行文件」章节与速查表条目；README 阶段 B 段改为 `mio build`。

**TodoLists[]**

- TD77d5bb566833497dbaa31d564bd0cde1（原生后端 MVP 主体）
- TDcddc66028ced4bd09874cedea8ad7ffc（MVP 语义层）
- TDa5354d28435a4e17a08f30e041173b5d（MVP 代码生成）
- TD085d68ae63a249f98e3ba0b012ca74f2（MVP 基准与报告）
- TDd15b8d585eae461c90471ebfc41c1692（Native 性能优化轮）
- TD5dfa47f778e14af9bd8756b5ac1279bb（mio build 收编与诊断改进）

**关联讨论**

- TL20261110T041429（原生后端立项）
- TL20261121T210405（性能优化轮）
- TL20261122T022000（诊断精度与 build 收编）

---

## 0.3.0（2026-11-21）

**状态**：已发布。

**更新内容**

- 新增 `@std/os`：`platform` / `arch` / `hostname` / `homedir` / `tmpdir` / `cwd` / `pid` 与常量 `eol`。
- 新增 `@std/env`：`get` / `set` / `remove` / `has` / `toObject`；写操作走进程内覆盖表，不触碰真实进程环境（避开 `std::env::set_var` 的 unsafe 约束）。
- 新增 `@std/encoding`：base64 与 hex 编解码，二进制统一 `Uint8Array`，非法输入抛 `TypeError`。
- 新增 `@std/log`：`debug` / `info` / `warn` / `error` / `setLevel` / `getLevel`；写 stderr，级别初值来自配置，运行时可改。
- 四份声明随 `mio init` / `mio types` / `mio check` 落地，`index.d.ts` 以 `/// <reference path>` 串联。
- 手册补四节 API 说明与速查表条目，`site/dist` 重建。

**TodoLists[]**

- TD0798b63b3643493fa05277e78788d69a（标准库第二批）
- TD4756fd3be1c2406b980d4e33425ce792（Std 标准库，本批收尾）

**关联讨论**

- TL20261121T190813

---

## 0.2.2（2026-11-21）

**状态**：已发布。

**更新内容**

- `mio init` 生成的 `tsconfig.json` 增加 `"moduleDetection": "force"`，与运行语义对齐：运行期与 `mio check` 都把每个文件当 ES Module，顶层 `await` 因此合法；缺这一行时，没有 import/export 的文件会被编辑器当成脚本，导致顶层 `await` 被误报。
- `mio init` 仍遵守「已存在配置不覆盖」，老项目需手工补该字段，或在文件末尾加 `export {}`（手册常见问题已写明）。
- 手册同步：生成的 tsconfig 片段展示该字段并补迁移说明。
- 本版本为补丁修复，无 API 变更。

**TodoLists[]**

- TD5a13237b8a0f4a22a7dadb631c06c727（生成的 tsconfig 补 `moduleDetection`）

**关联讨论**

- TL20261121T182811

---

## 0.2.1（2026-11-09）

**状态**：已发布。

**更新内容**

- 新增 `@std/fs`：Promise 风格文件 API，底层工作线程池执行、经事件循环单通道回传，不阻塞事件循环。
- 新增 FD 控制层：`open` / `File.read` / `File.write` / `File.close`，支持分块与流式读写；不提供同步 API。
- 新增 `mio repl`：持久状态、括号配平多行续行、错误不退出。
- 类型声明补 `@std/fs`（`FileInfo` / `File` / `OpenMode` / `open`）。
- 取消未实现的「预转译缓存」`mio build`，移除 `build` 配置项与 `.mio/cache/` 脚手架提示（未来做真正的 build）。
- 手册同步更新：`@std/fs` 章节、REPL、文件系统兼容性与快速参考。

**TodoLists[]**

- TD1e49b48058a64fc590a3cd9f93ca7c12（里程碑 2）
- TDa1c9c763253d4e7fb424909bfa5b7a56（`@std/fs` Phase 1）
- TDc384bc35814e4f169d670bb39507bc76（`@std/fs` Phase 2）
- TDc037166c009f429bb01c0f759f62d71b（REPL）

**关联讨论**

- TL20251109T160000
- TL20251109T173000
- TL20251109T180000

---

## 0.2.0（2026-11-09）

**状态**：已发布。

**更新内容**

- 新增 `types/mio/` 运行时类型声明，覆盖 Web 全局对象与 `@std/path`，编译期嵌入二进制。
- 新增 `mio init` 与 `mio types`，生成并刷新用户项目内的 `.mio/@types/mio/` 与 `tsconfig.json`。
- `mio check` 注入内置声明并使用 `--lib esnext`，项目存在 `tsconfig.json` 时不再触发 TS5112。
- 用户文档迁至 `site/`，内容与实现逐项对齐。
- 新增 `mio docs [host:port]`：VitePress 手册、包豪斯 / 构成主义主题、静态资源嵌入二进制，离线可查。
- 新增 `include_dir` 依赖（纯 Rust），用于手册目录嵌入。

**TodoLists[]**

- TDa89497f21b6645cbbe823b82c16fe651（类型声明与文档）
- TD372b08ea5a654b2492ab1cb5cff4e2d2（内置手册 `mio docs`）

**关联讨论**

- TL20261109T011352
- TL20261109T015014

---

## 0.1.0（2026-11-05）

**状态**：已发布。

**更新内容**

- 项目立项，确定定位为 TypeScript 一等公民的运行时，不做 Node.js 与 CommonJS 兼容层。
- 确定两阶段路线：阶段 A 为 TypeScript 转译后执行，阶段 B 为自研前端与字节码虚拟机输出原生代码。
- 确定类型系统扩展策略：标准 TypeScript 不裁剪，追加机器类型与结构类型；`number` 与机器类型之间的转换规则确定；类型表示分 `Unboxed` / `Managed` / `Boxed` 三轨。
- 确定配置系统形态：`mio.json`（JSONC），借鉴 `deno.json` 骨架，`imports` 与 `scopes` 采用 Import Maps 规范，字段分生效、仅提示、预留三档。
- 建立仓库骨架：单 crate、edition 2024、Rust 1.97.1、Boa 0.22、OXC 0.156。
- 建立模块分层与两个阶段接缝（`engine::Engine`、`resolver::Specifier`）。
- 配置系统落地：`tsconfig.json` 骨架 + Import Maps 模块映射 + 四档生效状态标注 + JSON 值深合并继承 + 环境变量展开 + 未知字段告警。
- 命令行支持 `--version` 与 `--help`。
- 测试 64 项通过（57 单元 + 7 端到端），clippy 与 fmt 干净。

**TodoLists[]**

- TD48d4b0c036ac4ecd989f487505fde111（立项与仓库形态）
- TD0b51717fc7d54c78ad0350af3fa5c08e（配置系统）

**关联讨论**

- TL20261030T041103
- TL20261030T042730
