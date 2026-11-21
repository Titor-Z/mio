# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

## 0.5.0

### `mio build`（阶段 B 原生后端）

- **多文件 import**：`import { abs } from "./util"`（相对路径 `./` `../`、扩展名 `.ts` 可省、具名导入）。被引模块的函数编译期加「相对入口路径」前缀，入口文件必须声明 `main`。循环导入在解析期检出；裸包名、`node:` 前缀、`as` 别名、默认/命名空间导入、`import type` 一律明确报错（本阶段无 `node_modules`）。
- **语法 desugar**：`switch`（穿透合并——无 `break`/`return` 结尾的 case 与下一 case 同组；`default` 仅支持末尾；`break` 仅支持 case 体末尾）、`do-while`、复合赋值 `+= -= *= /= %=`。
- 诊断：编译错误带阶段名与位置（降低期精确到行列，类型检查期定位到函数声明行），rustc 风格渲染（源码行 + 插入符）。

### 修复

- 零周期 `setInterval` 在 `run.timeoutMs` 边界偶发误报「微任务队列未能排空」（exit 1）的竞态：超时判定加「无未决宿主工作」条件，正常终局统一走优雅收尾（exit 0）；真风暴（无任何宿主工作）的诊断保留。

## 0.4.0

### 新增：`mio build`——编译为原生可执行文件（阶段 B 收编为可选模块）

- 新增 `mio build <file> [-o out] [--emit-obj]`：把带类型标注的 TypeScript 子集 AOT 编译为本机可执行文件（OXC → TIR → Cranelift → `cc` 链接），不经 JavaScript 引擎。
- 原生后端作为**独立可选模块**接入：`native/` 仍是独立 workspace，主 crate 单向依赖它（与 `tsrs_execute` 同一模式）；由 cargo feature `native` 门控，默认开启，`cargo build --no-default-features` 产出的瘦 binary 不含 Cranelift，`mio build` 会给出明确提示而非静默失效。
- 运行时（`mio run`）不受影响：类型仍擦除执行，不强制类型标注。
- 诊断改进：编译错误带阶段名与位置——降低期错误精确到行列，类型检查期错误定位到函数声明行，渲染为 rustc 风格（源码行 + 插入符）。

### 性能（阶段 B 实测，release，macOS x86_64）

- `fib(35)`：mio 0.07s / Node 0.24s / cc 0.03s（3.3x Node）。
- 热点循环 1e8 次：小函数内联（本次扩展到带 if/提前 return 的函数）后 0.20s → 0.07s，与手工内联持平；浮点循环与 cc 持平。
- 已知边界：支持机器类型与纯计算子集；字符串/数组/对象/闭包/`import` 暂不支持（显式报错）。数字与边界见 `docs/NativeMVP报告.md`。

## 0.3.0

### 标准库补齐：`@std/os` / `@std/env` / `@std/encoding` / `@std/log`

- 新增 `@std/os`：`platform()`、`arch()`、`hostname()`、`homedir()`、`tmpdir()`、`cwd()`、`pid()` 与常量 `eol`，全部同步无副作用。
- 新增 `@std/env`：`get` / `set` / `remove` / `has` / `toObject`。`set` / `remove` 走进程内覆盖表（不触碰真实进程环境，规避 `std::env::set_var` 在多线程下的 unsafe）。
- 新增 `@std/encoding`：`encodeBase64` / `decodeBase64` / `encodeHex` / `decodeHex`，二进制统一 `Uint8Array`，非法输入抛 `TypeError`。
- 新增 `@std/log`：`debug` / `info` / `warn` / `error` / `setLevel` / `getLevel`，输出到 stderr，级别初值来自 `log.level`（或 `MIO_LOG_LEVEL`），脚本可运行时修改。
- 声明文件随 `mio init` / `mio types` / `mio check` 一并落地；`types/mio/index.d.ts` 用 `/// <reference path>` 串起全部 `@std/*` 声明。
- 手册新增四节 API 说明与速查表条目。

## 0.2.2

### 修复

- `mio init` 生成的 `tsconfig.json` 增加 `"moduleDetection": "force"`：运行期把每个文件都当 ES Module，顶层 `await` 因此合法；缺失这一行时，没有 import/export 的文件会被编辑器当成脚本，导致顶层 `await` 被误报。老项目需手工补上（`mio init` 不覆盖已有配置），或在文件末尾加 `export {}`。

手册同步：`mio init` 生成的 tsconfig 片段展示该字段，常见问题新增「编辑器把顶层 `await` 标红」的两种迁移方式。

## 0.2.1

### 高性能文件系统 `@std/fs`

- 新增 `@std/fs`：`readFile` / `readTextFile` / `writeFile` / `writeTextFile` / `stat` / `exists` / `mkdir` / `readdir` / `remove` / `copyFile` / `rename`，全部返回 Promise。
- 底层 IO 走小型工作线程池执行，经与 `fetch` 共用的事件循环单通道回传，**不阻塞事件循环**（定时器与在途请求照常运转）。
- 新增 FD 控制层：`open(path, mode?)` 返回 `File`，`File.read(buffer)` / `File.write(data)` / `File.close()`，支持分块与流式读写；不提供同步 API，用 `await` 表达同步语义。
- 类型声明补 `@std/fs`（含 `FileInfo`、`File`、`OpenMode`、`open`），随 `mio init` / `mio types` / `mio check` 一并落地。

### 交互式 REPL

- 新增 `mio repl`：复用同一引擎，变量与函数声明跨输入保持；按括号配平自动续行；错误不退出；`:quit` 或 Ctrl-D 结束。

### 收敛

- 取消未实现的「预转译缓存」方案（`mio build`），移除 `build` 配置项（`outDir` / `sourcemap`）与 `.mio/cache/` 脚手架提示，避免谎报能力。未来的 build 将按真正的构建语义另行设计。
- 手册（`site/`）同步更新：新增 `@std/fs` 章节、REPL 说明、文件系统兼容性对照与快速参考片段。

### 测试

- 新增 `tests/fs_e2e.rs`（读写往返、FD 分块、512KB 读取期间定时器仍触发）与 `tests/repl_e2e.rs`（状态保持、多行、错误恢复）。
- 全套测试通过；clippy 与 fmt 干净。
