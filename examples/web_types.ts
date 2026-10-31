// Web 数据类型 + 定时器：URL / URLSearchParams / TextEncoder / TextDecoder /
// Blob / FormData / setTimeout / queueMicrotask，全部无需网络。
//   mio run examples/web_types.ts

export {};

const url = new URL("https://example.com/a/b?x=1&y=2&x=3#frag");
console.log("url:", url.origin, url.pathname, url.search, url.hash);
console.log("resolve:", new URL("../c?k=v", url).href);

const params = url.searchParams;
params.append("q", "hello world");
params.set("x", "9");
console.log("params:", params.toString(), "x =", params.get("x"), "size =", params.size);
for (const [name, value] of params) {
  console.log("  pair:", name, "=", value);
}

const bytes = new TextEncoder().encode("中文a");
console.log("encode:", bytes.length, "bytes, decode:", new TextDecoder().decode(bytes));

const blob = new Blob(["hello", " 世界"], { type: "Text/Plain" });
console.log("blob:", blob.size, blob.type, await blob.text());

const form = new FormData();
form.append("a", "1");
form.append("file", blob, "note.txt");
const file = form.get("file") as File;
console.log("form:", form.get("a"), file.name, file.type, file.size);
// form 作为 fetch body 时自动编码为 multipart/form-data：
// await fetch("http://127.0.0.1:8931/", { method: "POST", body: form });

console.log("sync: microtask beats timer");
setTimeout(() => console.log("timer(0)"), 0);
setTimeout(() => console.log("timer(30)"), 30);
queueMicrotask(() => console.log("microtask"));
let ticks = 0;
const handle = setInterval(() => {
  ticks += 1;
  console.log("interval tick", ticks);
  if (ticks === 2) clearInterval(handle);
}, 20);
await new Promise((resolve) => setTimeout(resolve, 80));
