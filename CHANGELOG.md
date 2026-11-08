# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

## 0.2.0

### 运行时类型声明与项目脚手架

- 新增 `types/mio/`：手写声明与实现逐项对齐，覆盖 `console`、定时器、`performance`、`URL` / `URLSearchParams`、`TextEncoder` / `TextDecoder`、`Blob` / `FormData` / `File`、`Headers` / `Response` / `Request`、`fetch` / `AbortController` 与 `serve`，以及 `@std/path`。
- 声明在编译期通过 `include_str!` 嵌入二进制，不依赖外部文件。
- 新增 `mio init`：生成 `mio.json`、`tsconfig.json`、`.mio/@types/mio/`、`src/main.ts` 与 `.gitignore`；已存在的配置不覆盖。
- 新增 `mio types`：运行时升级后刷新项目内的类型声明。
- `tsconfig.json` 定位为“仅编辑器的类型适配层”：`lib: ["ESNext"]`、`typeRoots: [".mio/@types"]`、`types: ["mio"]`，运行配置仍以 `mio.json` 为唯一真相。
- `mio check` 内置声明注入：声明落到系统临时目录并作为额外根源文件传入，同时以 `--lib esnext` 关闭 tsrs 默认 DOM lib，避免与 mio 的 `URL` / `Response` 声明冲突；新增 `--ignoreConfig`，项目存在 `tsconfig.json` 时不再触发 TS5112。
- 修正 `/// <reference>` 位置：三斜线指令必须位于文件前导注释，放在末尾不会生效。

### 使用文档

- 用户文档从 `docs/` 剥离到 `site/`：上手指南、快速参考、Node/Deno 兼容性对比。
- 文档内容与实现对齐：`Response` 无静态 `json/text`、`console` 只有 `log`、`setTimeout` 返回数值 id、`Request` 不可自行构造、`fetch` 的 headers 只接受普通对象。

### 内置手册 `mio docs`

- 新增 `mio docs [host:port]`：启动内置手册服务，默认 `127.0.0.1:4173`，离线可用。
- 手册使用 VitePress 构建，自定义包豪斯 / 构成主义主题：纸张底色、3px 纯黑结构线、红/蓝/黄三原色、拼贴式布局与几何色块。
- `site/dist/` 构建产物提交进仓库，Rust 侧通过 `include_dir!` 编译期整目录嵌入，`cargo build` 不再需要 Node.js。
- 静态服务使用 `TcpListener`，支持 GET/HEAD、干净链接（`/guide` → `guide.html`）、内置 404 页与百分号解码；只查嵌入文件表，不触磁盘。
- 新增依赖 `include_dir`，保持纯 Rust。

### 其它

- 新增仓库根 `tsconfig.json`，供编辑器验证 `examples/` 下的示例代码。
- README 补充 `mio init`、`mio types`、`mio docs` 与新目录结构。
