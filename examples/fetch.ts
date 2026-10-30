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
