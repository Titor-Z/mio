# Mio 运行时上手指南

本 guide 介绍如何使用 **mio** 进行开发。包括安装、配置和常用 API 的使用示例。

## 1. 快速开始

### 1.1 运行第一个程序

```bash
# 创建一个 TypeScript 文件
echo 'console.log("Hello, Mio!");' > hello.ts

# 执行它
mio run hello.ts
```

### 1.2 执行内联代码

```bash
mio eval "console.log('Inline code!')"
```

## 2. 编辑器配置

要在编辑器中获得完整的类型提示，你需要引用我们的类型声明。

### 2.1 初始化项目

```bash
mio init
```

它会创建：

- `mio.json`（运行配置）
- `tsconfig.json`（仅编辑器的类型提示）
- `.mio/@types/mio/`（运行时类型声明）
- `src/main.ts`（入口示例）

生成的 `tsconfig.json` 关键项：

```jsonc
{
  "compilerOptions": {
    "lib": ["ESNext"],              // 不要 DOM
    "typeRoots": [".mio/@types"],
    "types": ["mio"]
  },
  "include": ["src/**/*.ts"]
}
```

运行时升级后执行 `mio types` 刷新声明。

### 2.2 验证提示

打开你的编辑器，在文件中输入：

```ts
serve(async (req) => {  // ← 应该有 serve 的类型提示
  console.log(`${req.method} ${req.url}`);  // ← Request 方法/URL 应该可用
  return new Response("ok");  // ← Response 类型提示
});
```

如果你看到类型提示和错误检查（而不是红色的下划线），就成功了！

## 3. 核心 API

### 3.1 HTTP 服务端 - `serve()`

mio 内置了轻量级的 HTTP 服务器，只需一个 handler 函数：

```ts
// 基础版本
serve((req) => {
  console.log(`${req.method} ${req.url}`);
  return new Response("Hello");
});

// 带端口配置
serve((req) => new Response('{"user":"guest"}', {
  headers: { "Content-Type": "application/json" },
}), { port: 3000 });

// HTTPS 版本（需提供证书路径）
serve(handler, {
  port: 443,
  cert: "/path/to/cert.pem",
  key: "/path/to/key.pem",
});

// 异步 handler
serve(async (req) => {
  const res = await fetch("https://api.example.com/data");
  return new Response(JSON.stringify(await res.json()), {
    headers: { "Content-Type": "application/json" },
  });
});
```

#### Request 对象属性

| 属性 | 类型 | 说明 |
|---|---|---|
| `method` | `string` | HTTP 方法 (GET/POST 等) |
| `url` | `string` | 请求 URL |
| `headers` | `Headers` | HTTP 头表 |
| `body` | `ArrayBuffer \| null` | 请求体原始字节 |

#### Headers API

```ts
const contentType = req.headers.get("Content-Type");
req.headers.set("X-Custom-Header", "value");
req.headers.append("Set-Cookie", "session=abc");
```

#### Response 构造与返回

```ts
// 文本响应
return new Response("Hello World");

// JSON 响应（没有静态 Response.json，需要自己序列化）
return new Response(JSON.stringify({ foo: "bar" }), {
  headers: { "Content-Type": "application/json" },
});

// 自定义状态码
return new Response("Error", { status: 500, statusText: "Server Error" });

// 二进制数据
return new Response(uint8Array, { headers: { "Content-Type": "image/png" } });
```

### 3.2 HTTP 客户端 - `fetch()`

mio 实现了 WHATWG Fetch API：

```ts
// GET 请求
const res = await fetch("https://example.com/api");
console.log(await res.json());

// POST 请求
const res = await fetch("https://api.example.com/users", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "Alice" }),
});

// 取消请求
const controller = new AbortController();
setTimeout(() => controller.abort(), 5000); // 5 秒超时

const res = await fetch(url, { signal: controller.signal });
```

### 3.3 其他 Web API

#### `URL` 和 `URLSearchParams`

```ts
const url = new URL("https://example.com/foo/bar?x=1&y=2");
console.log(url.pathname);        // "/foo/bar"
console.log(url.searchParams.get("x"));  // "1"

url.searchParams.set("x", "10");
console.log(url.href);            // "https://example.com/foo/bar?x=10&y=2"
```

#### `Blob` / `FormData`

```ts
// Blob
const blob = new Blob(["hello", "world"], { type: "text/plain" });
console.log(blob.size);           // 10
console.log(await blob.text());   // "helloworld"

// FormData（表单上传用）
const form = new FormData();
form.append("username", "alice");
form.append("avatar", imageBlob);  // 文件对象

// fetch 中自动编码为 multipart/form-data
await fetch("/upload", { method: "POST", body: form });
```

#### `TextEncoder` / `TextDecoder`

```ts
const encoder = new TextEncoder();
const bytes = encoder.encode("你好");  // Uint8Array

const decoder = new TextDecoder();
const text = decoder.decode(bytes);   // "你好"
```

### 3.4 定时器与微任务

```ts
// 延迟执行
setTimeout(() => {
  console.log("Delayed");
}, 1000);

// 循环执行
const id = setInterval(() => {
  console.log("Tick");
}, 500);
clearInterval(id);

// 微任务队列
queueMicrotask(() => {
  console.log("This runs before next microtask checkpoint");
});
```

### 3.5 高精度计时器

```ts
const start = performance.now();
// ... 一些耗时操作
const end = performance.now();
console.log(`Elapsed: ${(end - start).toFixed(2)}ms`);
```

### 3.6 `@std/path` 模块

```ts
import { join, dirname, basename, extname, normalize } from "@std/path";

join("foo", "bar", "baz");          // "foo/bar/baz"
dirname("/foo/bar/baz");            // "/foo/bar"
basename("file.txt", ".txt");       // "file"
extname("archive.tar.gz");          // ".gz"
normalize("/foo/../bar/./baz");     // "/bar/baz"
```

## 4. 调试技巧

### 4.1 控制台输出

当前只提供 `console.log()`：

```ts
console.log("Debug:", value);
```

### 4.2 超时诊断

如果程序卡住，通常是事件循环没有驱动微任务。确保在你的 handler 中正确处理 Promise 链。

## 5. 离线手册

运行时自带完整手册，不联网也能查：

```bash
mio docs                  # http://127.0.0.1:4173/
mio docs 0.0.0.0:8080     # 指定监听地址
```

手册页面由 VitePress 构建，静态资源已嵌入二进制。

## 6. 常见问题

### Q: 为什么编辑器提示 `window is not defined`？

A: mio 是后端运行时，没有浏览器环境。请确保：
1. 不要在代码中使用 `window`, `document` 等 DOM API
2. `tsconfig.json` 中不要添加 `"lib": ["dom"]`

### Q: 为什么 `@std/path` 找不到？

A: 先确认已执行 `mio init`，并且 `tsconfig.json` 里有：

```json
{
  "typeRoots": [".mio/@types"],
  "types": ["mio"]
}
```

### Q: 如何知道我的代码是否使用了正确的类型？

A: 运行 `mio check yourfile.ts` 会进行类型检查，如果有类型错误会报错。

## 下一步

- 查看 [兼容性文档](./compatibility.md) 了解与 Node.js/Deno 的差异
- 阅读仓库内的 `examples/` 目录，每个文件都可以直接 `mio run`
