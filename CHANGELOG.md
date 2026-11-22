# Changelog

本文件只保留最新一个版本的发布说明，供 GitHub Actions 生成 release 使用。
历史版本记录见 `docs/Release.md`。

## 0.6.0

### 远程 URL 导入（`import "https://…"`）

- 说明符支持 `http(s)://`：可直接 `import { Hono } from "https://esm.sh/hono@4.6.14"`；本地入口 import 远程、`imports` 别名指向 URL、`mio run <url>` 直接跑远程入口均可用。
- 下载缓存定死 `~/.mio/deps/<scheme>/<host>/<name>_<hash8>.{ts,json}`：`.tmp` + `rename` 原子写入，元数据含 url/status/contentType/etag/lastModified/fetchedAt/sha256，单模块上限 10 MiB，扩展名归一（仅 `.tsx` 保留，其余一律 `.ts`）。
- **cache-first**：命中即返回、永不主动联网，断网也能跑。`--reload`（全部重下）、`--reload=<url>`、`--revalidate`（条件请求，304 只刷新时间）、`--cached-only`（严格离线）、`--no-remote`（禁用远程导入）显式控制，仅对 `mio run` 生效。
- `mio.lock` 默认生成：`{version, remote: {url: "sha256-…"}}`，键排序稳定，只记不可变事实（不含时间戳/ETag/终址）；`--frozen` 只校验不写入，供 CI 使用。
- `imports` 映射允许 URL 目标（此前被当作裸包名拒绝）。

### Web 标准对象对齐 W3C

- `Request` 成为全局构造器：`new Request(input, init)`——`input` 支持绝对 URL / `URL` / 另一个 `Request`，`init` 支持 `method` / `headers` / `body` / `signal`；`serve` 注入的对象与之同源铸造，因此 `req instanceof Request` 成立。相对地址按规范抛 `TypeError`。
- `Headers` / `Response` 迁为带原型的类：`instanceof` 与 `Object.prototype.toString.call(x)` 均按规范返回。
- `Response.headers` 改为 **live 视图**：构造之后 `res.headers.set(...)` 会真的写进响应头（修复框架在 `await next()` 之后补头静默丢失的问题）。
- `new Response(字符串体)` 按规范自动补 `content-type: text/plain;charset=UTF-8`（`Blob` 体沿用其 `type`）。
- `Response` 补 `clone()` / `type` / `redirected`；`Request` 补 `clone()`。
- 合并两套 Response：`fetch()` 返回的就是标准 `Response`（`instanceof` 成立、headers 具备完整方法、`serve(() => fetch(...))` 可直接写线）。
- `fetch()` 入参扩到 `string | URL | Request`。
- 类型声明同步：`Request` 去掉 `private constructor()`，`FetchResponse` 改为 `Response` 的类型别名。
- `mio init` 生成的入口示例补充 `serve` 与远程导入提示。

### 文档

- 手册新增「从 URL 导入远程模块」整章（缓存位置、何时联网、`mio.lock`、与 Deno/Bun 的差异），并补充 `Request` 可构造、`Response` 自动 content-type 与 live headers、`fetch(Request)` 等用法；兼容性对照表与速查表同步。
- 兼容性表新增「远程 URL 导入」一行。
