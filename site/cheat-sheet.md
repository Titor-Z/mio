# Mio 快速参考卡

## 🚀 服务器端开发

```ts
// 最简单的服务器
serve((req) => new Response("Hello"));

// JSON API（没有静态 Response.json）
serve(async (req) => {
  return new Response(JSON.stringify({
    status: "ok",
    data: await fetchData(),
  }), { headers: { "Content-Type": "application/json" } });
});

// HTTPS
serve(handler, { port: 443, cert: "...", key: "..." });
```

## 🌐 远程模块导入

```ts
// 直接 import 远程 URL（Deno 风格）
import { Hono } from "https://esm.sh/hono@4.6.14";

// 也可以用 mio.json 的 imports 起别名
// { "imports": { "@lib/": "https://esm.sh/hono@4.6.14/" } }
import { Hono } from "@lib/";
```

```bash
mio run app.ts            # 命中 ~/.mio/deps 缓存，不联网
mio run app.ts --reload   # 强制重下全部远程模块
mio run app.ts --reload=https://esm.sh/hono@4.6.14
mio run app.ts --revalidate    # 条件请求，304 只刷新时间
mio run app.ts --cached-only   # 严格离线
mio run app.ts --no-remote     # 禁用远程导入
mio run app.ts --frozen        # 只校验 mio.lock
```

- 缓存：`~/.mio/deps/<scheme>/<host>/<name>_<hash8>.{ts,json}`，清空用 `rm -rf ~/.mio/deps`。
- 锁文件：`mio.lock` 默认生成（在 mio.json 所在目录），记录 URL → sha256，建议提交进 git。
- `Request` 可自行构造：`new Request(url, { method, headers, body })`，`fetch(req)` 也支持。

## 📡 HTTP 客户端

```ts
const res = await fetch("https://api.example.com/users", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "Alice" }),
});

const json = await res.json();
```

## 📁 文件操作

```ts
import * as fs from "@std/fs";

// 便捷读写（全部异步）
await fs.writeTextFile("a.txt", "hi");
const text = await fs.readTextFile("a.txt");
const bytes = await fs.readFile("a.bin");
const info = await fs.stat("a.txt");          // { isFile, size, ... }

// FD 控制层（分块读写）
const f = await fs.open("big.bin", "r");     // "r" | "w" | "a"
const buf = new Uint8Array(64 * 1024);
const n = await f.read(buf);                  // 读取字节数，0 = EOF
await f.close();
```

## 🛠️ 常用工具

```ts
import { join, dirname } from "@std/path";

join("foo", "bar");           // foo/bar
dirname("/path/to/file");     // /path/to

const url = new URL("https://x.com?a=1");
url.searchParams.get("a");    // "1"
```

## 💾 二进制数据

```ts
// Blob
const blob = new Blob(["hello"], { type: "text/plain" });
await blob.text();            // Promise<string>
await blob.arrayBuffer();     // Promise<ArrayBuffer>

// FormData（文件上传用）
const form = new FormData();
form.append("file", fileBlob);
fetch("/upload", { method: "POST", body: form });

// TextEncoder/Decoder
const bytes = new TextEncoder().encode("你好");
const text = new TextDecoder().decode(bytes);
```

## ⏱️ 计时与定时器

```ts
setTimeout(() => {}, 1000);   // 延迟 1 秒
setInterval(() => {}, 500);   // 每 500ms
queueMicrotask(() => {});     // 微任务

performance.now();            // 高精度时间戳（毫秒）
```

## 🔌 AbortController

```ts
const controller = new AbortController();
setTimeout(() => controller.abort(), 5000);

await fetch(url, { signal: controller.signal });
```

## 📝 Headers

```ts
const h = new Headers([["X-Custom", "value"]]);
h.set("Content-Type", "application/json");
h.get("X-Custom");            // "value"
h.delete("X-Custom");
```

## 🌍 环境与日志

```ts
import { platform, arch, tmpdir, cwd, pid } from "@std/os";
import { get, set, has, remove, toObject } from "@std/env";
import { info, warn, error, setLevel } from "@std/log";

console.log(platform(), arch(), tmpdir());
const home = get("HOME");        // string | undefined
set("APP_ENV", "production");
remove("APP_ENV");               // boolean

info("启动完成");                 // 写 stderr
setLevel("debug");               // 运行时改级别：silent|error|warn|info|debug
```

## 🔐 编解码

```ts
import { encodeBase64, decodeBase64, encodeHex, decodeHex } from "@std/encoding";

encodeBase64("foo");              // "Zm9v"
decodeBase64("Zm9v");             // Uint8Array
encodeHex(new Uint8Array([0,255])); // "00ff"
decodeHex("00FF");                // Uint8Array
```

## 🎯 常用模式

### JSON 响应模板
```ts
return new Response(JSON.stringify({ ok: true, data: value }), {
  headers: { "Content-Type": "application/json" },
});
```

### 错误响应
```ts
return new Response("Not found", { status: 404 });
```

### CORS 头部
```ts
const headers = new Headers();
headers.set("Access-Control-Allow-Origin", "*");
return new Response(data, { headers });
```

## ⚡ 编译为原生可执行文件

```bash
mio build app.ts            # 产出本机可执行文件 app
mio build app.ts -o fib     # 指定产物名
mio build app.ts --emit-obj # 只产 .o，不链接
```

```ts
function add(a: i64, b: i64): i64 { return a + b; }  // 必须带类型标注
function main(): void { print(add(1, 2)); }          // 入口固定 main
```

支持：机器类型（`i8..i64`/`u8..u64`/`f32`/`f64`/`bool`）、`if`/`while`/`for`/`do-while`、`switch`（穿透）、三元、`as` 扩宽、复合赋值（`+=` 等）、`print`。
多文件：`import { abs } from "./util";`（相对路径、具名、扩展名可省；循环导入报错；`main` 只能在入口文件）。
不支持：字符串、数组、对象、`class`、闭包、默认/命名空间导入——需要时用 `mio run`。
