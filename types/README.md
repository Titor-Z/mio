# Mio 运行时类型声明

为编辑器提供 mio 运行时与 `@std/*` 的类型提示。**项目里不要手写或手改，由 `mio init` 生成到 `.mio/@types/mio/`。**

## 使用

```bash
mio init     # 创建 mio.json、tsconfig.json、.mio/@types/mio/、src/main.ts
mio types    # 运行时升级后刷新 .mio/@types/mio/
```

生成的 `tsconfig.json` 关键项：

```jsonc
{
  "compilerOptions": {
    "lib": ["ESNext"],              // 关键：不要 DOM
    "typeRoots": [".mio/@types"],
    "types": ["mio"]
  },
  "include": ["src/**/*.ts"]
}
```

`.mio/@types/` 建议提交进 git。

## 已声明 API

| 模块 | 内容 |
|---|---|
| `console` | 仅 `log` |
| 定时器 | `setTimeout` / `setInterval` / `clearTimeout` / `clearInterval` / `queueMicrotask` |
| `performance` | `now()` |
| `URL` / `URLSearchParams` | WHATWG URL |
| `TextEncoder` / `TextDecoder` | UTF-8 |
| `Blob` / `FormData` | 二进制与表单数据；`File` 仅作为 FormData 文件字段的类型名 |
| `Headers` / `Response` | 可构造的 Fetch 标准对象；`Response` **无静态 `json`/`text`** |
| `Request` | 仅由 `serve` 注入，不能 `new Request()` |
| `fetch` / `AbortController` | HTTP/HTTPS 客户端与取消 |
| `serve` | HTTP/HTTPS 服务端（mio 特有） |
| `@std/path` | `join` / `normalize` / `dirname` / `basename` / `extname` / `separator` |

## 示例

```ts
import { join } from "@std/path";

const json = (data: unknown, status = 200) =>
  new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });

serve((req) => {
  console.log(`${req.method} ${req.url}`);
  return json({ ok: true });
}, { port: 3000 });
```

## 注意事项

1. 没有 DOM：`window`、`document`、`XMLHttpRequest` 均不存在，`lib` 不要带 `DOM`
2. 没有 Node.js API：无 `fs` / `node:path` / `require`
3. 纯 ES Module
4. 异步优先：I/O 返回 `Promise`

完整文档见 [`site/guide.md`](../site/guide.md)。
