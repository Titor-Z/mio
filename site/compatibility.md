# Mio vs Node.js vs Deno 对比

## Web API 支持

| API | mio | Node.js | Deno | 备注 |
|---|---|---|---|---|
| `fetch()` | ✅ | ⚠️ 18+ | ✅ | Node 需要 polyfill |
| `Response` | ✅ | ❌ | ✅ | Node 无原生 Response |
| `Request` | ✅ 可构造 | ❌ | ✅ | `new Request(...)`、`instanceof` 成立 |
| `Headers` | ✅ | ❌ | ✅ | - |
| `URL` | ✅ | ✅ | ✅ | WHATWG URL |
| `Blob` | ✅ | ✅ | ✅ | - |
| `FormData` | ✅ | ✅ | ✅ | - |
| `TextEncoder` | ✅ | ✅ | ✅ | - |
| `TextDecoder` | ✅ | ✅ | ✅ | - |
| `performance.now()` | ✅ | ✅ | ✅ | - |
| `setTimeout` | ✅ | ✅ | ✅ | - |
| 远程 URL 导入 | ✅ `import "https://…"` | ❌ | ✅ | mio 缓存到 `~/.mio/deps`，另有 `mio.lock` |

## 服务端开发

| 能力 | mio | Node.js | Deno |
|---|---|---|---|
| `serve(handler)` | ✅ 内置 | ❌ (`http` 模块) | ✅ `Deno.serve` |
| HTTPS | ✅ rustls | ⚠️ `https` 模块 | ✅ `Deno.serveTLS` |
| HTTP/2 | ❌ | ✅ | ✅ |
| WebSocket | ❌ | ✅ (`ws`) | ✅ (`deno_websocket`) |

## 文件系统

| API | mio | Node.js | Deno |
|---|---|---|---|
| `readFile` / `readTextFile` | ✅ `@std/fs` | ✅ | ✅ |
| `writeFile` / `writeTextFile` | ✅ `@std/fs` | ✅ | ✅ |
| `stat` / `exists` / `mkdir` / `readdir` / `remove` / `copyFile` / `rename` | ✅ `@std/fs` | ✅ | ✅ |
| `open` + `read` / `write` / `close`（FD 流式） | ✅ `@std/fs` | ✅ | ✅ |
| 同步 API（`readFileSync` 等） | ❌ 用 `await` | ✅ | ⚠️ |
| 文件监听 `watch` | ❌ | ✅ | ⚠️ |

> mio 的文件 API 全部返回 Promise，底层 IO 在工作线程池执行，不阻塞事件循环；不提供同步版本（用 `await` 即可表达同步语义）。

## 标准库

| 模块 | mio | Node.js | Deno |
|---|---|---|---|
| `@std/path` | ✅ 自有实现 | `path` | `std/path` |
| `@std/fs` | ✅ 自有实现 | `fs` | `std/fs` |
| `crypto` | ❌ | ✅ | ✅ |
| `process.env` | ❌ | ✅ | `Deno.env` |

## 运行时差异

### 远程模块：URL 即模块身份

```ts
import { Hono } from "https://esm.sh/hono@4.6.14";
```

- 缓存位置定死 `~/.mio/deps/`，永久有效、命中即用、永不主动联网；`--reload` 才重下。
- `mio.lock` 记录「URL → 内容哈希」，`--frozen` 供 CI 校验；它只记不可变事实，不含时间戳/ETag。
- 与 Deno 的差异：**不内置 npm**（没有 `npm:` 协议、没有 `node_modules`）、不做 TTL 自动过期、
  没有 `mio vendor`；`mio check` 暂不支持远程导入（类型检查侧只拿到缓存文件路径）。
- 与 Bun 的差异：Bun 运行期不认 `https://` 说明符，远程代码要先经包管理器拉进 `node_modules`。

### DOM vs No DOM

mio **不提供**浏览器环境：
- ❌ `window`, `document`, `localStorage`
- ❌ `XMLHttpRequest`, `EventSource`
- ✅ 仅提供 WHATWG Web API

### CommonJS vs ES Module

mio **仅支持** ES Module：
```ts
// ✅ 有效
import { join } from "@std/path";
export function foo() {}

// ❌ 无效
const path = require("path");  // 不支持
module.exports = {};           // 不支持
```

### 类型擦除

mio 在**运行期**擦除 TypeScript 类型：
```ts
// tsconfig: {"target": "ES2025"}
function add(a: number, b: number): number {
  return a + b;
}
// ↓ 编译后
function add(a, b) {
  return a + b;
}
```

而 Bun/Deno 是 JIT 编译或解释执行。

## 迁移指南

### 从 Node.js 迁移

#### 1. 替换 HTTP 模块

```js
// Node.js
const http = require("http");
http.createServer((req, res) => {
  res.end("Hello");
}).listen(3000);
```

```ts
// mio
serve((req) => new Response("Hello"), { port: 3000 });
```

#### 2. 替换路径处理

```js
// Node.js
const path = require("path");
const joined = path.join(__dirname, "foo", "bar");
```

```ts
// mio
import { join } from "@std/path";
const joined = join("foo", "bar");  // 没有 __dirname
```

#### 3. 使用 fetch 而不是 request

```js
// Node.js (with node-fetch or axios)
const res = await request.get(url);
```

```ts
// mio (native!)
const res = await fetch(url);
```

#### 4. 替换文件读写

```js
// Node.js
const fs = require("fs");
const text = fs.readFileSync("a.txt", "utf8");
```

```ts
// mio：全部异步，用 await 表达同步语义
import * as fs from "@std/fs";
const text = await fs.readTextFile("a.txt");
```

### 从 Deno 迁移

大多数代码可以直接迁移：

```ts
// Deno
await Deno.serve({ port: 3000 }, (req) => 
  new Response("Hello")
);
console.log(Deno.env.get("FOO"));
```

```ts
// mio
serve((req) => new Response("Hello"), { port: 3000 });
```

> **注意**: mio v0.1 尚无环境变量 API（`Deno.env` / `process.env` 都不存在）。

## 最佳实践

### 1. 避免引入外部类型包

```jsonc
{
  "compilerOptions": {
    // ❌ 不要这样（会提示 window/document）
    "lib": ["ES2025", "DOM"]
    
    // ✅ 应该这样
    "lib": ["ESNext"],
    "typeRoots": [".mio/@types"],
    "types": ["mio"]
  }
}
```

### 2. 用 mio init 生成配置

```bash
mio init
```

`@std/*` 的声明随 `.mio/@types/mio/` 一起注入，无需 `paths` 映射。

### 3. 使用 Promise 而非回调

所有 I/O 都是异步的：
```ts
// ✅
await fetch(url);

// ❌ 不支持
fetch(url, (res) => console.log(res));
```
