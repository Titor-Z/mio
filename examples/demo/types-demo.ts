
import { join, dirname, basename, extname, normalize } from "@std/path";

// ========== 1. Serve 基础示例 =========

/** 构造 JSON 响应（运行时的 Response 没有静态 json 方法） */
const json = (data: unknown, status = 200) =>
  new Response(JSON.stringify(data), {
    status,
    headers: { "Content-Type": "application/json" },
  });

serve((req) => {
  console.log(`${req.method} ${req.url}`);
  
  const contentType = req.headers.get("Content-Type");
  if (contentType) {
    console.log(`Content-Type: ${contentType}`);
  }
  
  return json({
    message: "Hello Mio!",
    timestamp: Date.now(),
    method: req.method,
    url: req.url,
  });
}, { port: 3000 });

console.log("Server started at http://localhost:3000");

// ========== 2. URL & URLSearchParams =========

const testUrl = new URL("https://example.com:8080/api/v1/users?id=123&name=test#profile");

console.log("URL parts:");
console.log(`  Origin: ${testUrl.origin}`);          // https://example.com:8080
console.log(`  Hostname: ${testUrl.hostname}`);      // example.com
console.log(`  Port: ${testUrl.port}`);              // 8080
console.log(`  Pathname: ${testUrl.pathname}`);      // /api/v1/users
console.log(`  Search: ${testUrl.search}`);          // ?id=123&name=test
console.log(`  Hash: ${testUrl.hash}`);             // #profile

const params = testUrl.searchParams;
console.log(`  id param: ${params.get("id")}`);     // 123
console.log(`  name param: ${params.get("name")}`); // test

// ========== 3. URLSearchParams methods =========

const search = new URLSearchParams([
  ["q", "hello"],
  ["lang", "en"],
]);

search.append("page", "1");
search.set("lang", "zh");
search.delete("q");

console.log("Search params:", search.toString());   // lang=zh&page=1

// ========== 4. Blob & FormData =========

// Blob
const blob = new Blob(["Hello", " ", "World"], { type: "text/plain" });
console.log(`Blob size: ${blob.size}`);       // 12
console.log(`Blob type: ${blob.type}`);       // text/plain

blob.text().then((text) => {
  console.log(`Blob text: ${text}`);         // HelloWorld
});

// FormData
const form = new FormData();
form.append("username", "alice");
form.append("password", "secret123");

// 支持追加相同字段名（多值）
form.append("tags", "news");
form.append("tags", "update");

console.log(`Username: ${form.get("username")}`);           // alice
console.log(`Tags:`, form.getAll("tags"));                  // ["news", "update"]

// ========== 5. Headers & Response =========

const headers = new Headers([
  ["X-Custom-Header", "value1"],
  ["X-Custom-Header", "value2"],
]);

headers.set("Content-Type", "application/json");
headers.append("Authorization", "Bearer token");

console.log("Headers get:", headers.get("Content-Type"));       // application/json
console.log("Headers all custom:", headers.get("X-Custom-Header")); // value1, value2

// ========== 6. TextEncoder/TextDecoder =========

const encoder = new TextEncoder();
const bytes = encoder.encode("你好世界");
console.log(`UTF-8 bytes:`, bytes);

const decoder = new TextDecoder();
const text = decoder.decode(bytes);
console.log(`Decoded text:`, text);  // 你好世界

// ========== 7. Path utils =========

console.log("\nPath utilities:");
console.log(`join("foo", "bar") = ${join("foo", "bar")}`);      // foo/bar
console.log(`normalize("/a/b/../c") = ${normalize("/a/b/../c")}`); // /a/c
console.log(`dirname("/path/to/file") = ${dirname("/path/to/file")}`); // /path/to
console.log(`basename("file.txt") = ${basename("file.txt")}`);   // file.txt
console.log(`extname("archive.tar.gz") = ${extname("archive.tar.gz")}`); // .gz

// ========== 8. Performance timing =========

const start = performance.now();
// simulate work
for (let i = 0; i < 1000000; i++) {}
const end = performance.now();

console.log(`Elapsed: ${(end - start).toFixed(2)}ms`);

// ========== 9. AbortController (示例) =========

const controller = new AbortController();
const timeoutId = setTimeout(() => controller.abort(), 100);

// fetch("https://example.com", { signal: controller.signal })
//   .then(...)
//   .catch(err => console.log("Fetch aborted:", err));

console.log("All examples completed!");
