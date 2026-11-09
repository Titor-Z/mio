# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

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
