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

## 📡 HTTP 客户端

```ts
const res = await fetch("https://api.example.com/users", {
  method: "POST",
  headers: { "Content-Type": "application/json" },
  body: JSON.stringify({ name: "Alice" }),
});

const json = await res.json();
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
