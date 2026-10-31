// await fetch：顶层 await（ES2022）+ WHATWG fetch 的最小用法。
// 运行需要一个本地 HTTP 服务，例如：
//   python3 -m http.server 8931
// 然后执行：
//   mio run examples/fetch.ts

export {};

const response = await fetch("http://127.0.0.1:8931/");
console.log("status", response.status, "ok", response.ok);
console.log("server header:", response.headers.get("server"));

const body = await response.text();
console.log("body length:", body.length);

// 二进制消费（约定 1）：arrayBuffer() 返回 Uint8Array。
const bytes = await (await fetch("http://127.0.0.1:8931/")).arrayBuffer();
console.log("bytes:", bytes.byteLength);

// 取消（约定 4）：abort() 让在途 Promise 以 AbortError 拒绝。
// 挂起连接可用 `nc -l 8932` 之类不回应的端口模拟。
const controller = new AbortController();
const pending = fetch("http://127.0.0.1:8932/", { signal: controller.signal });
controller.abort();
try {
  await pending;
} catch (err) {
  console.log("cancelled:", (err as Error).name);
}

// POST：method / headers / 字符串 body（需公网可达 httpbin）。
// const posted = await fetch("https://httpbin.org/post", {
//   method: "POST",
//   headers: { "content-type": "application/json", "x-mio": "demo" },
//   body: JSON.stringify({ hello: "mio" }),
// });
// console.log("echo:", (await posted.json()).json);
