import * as fs from "@std/fs";

// 1) 用 FD 层写入文件
const writer = await fs.open("examples/.fs_fd.txt", "w");
const payload = new TextEncoder().encode("FD 控制层：分块读写演示");
await writer.write(payload);
await writer.close();
console.log("写入字节:", payload.length);

// 2) 用 FD 层分块读取
const reader = await fs.open("examples/.fs_fd.txt", "r");
const chunk = new Uint8Array(8);
const parts: Uint8Array[] = [];
while (true) {
  const n = await reader.read(chunk);
  if (n === 0) break;
  parts.push(chunk.slice(0, n));
}
await reader.close();

const total = parts.reduce((sum, part) => sum + part.length, 0);
const merged = new Uint8Array(total);
let offset = 0;
for (const part of parts) {
  merged.set(part, offset);
  offset += part.length;
}
console.log("分块读到:", new TextDecoder().decode(merged));

// 3) 验证不阻塞事件循环：读取期间定时器仍能触发
let ticked = false;
setTimeout(() => {
  ticked = true;
  console.log("定时器在 fs 操作期间触发:", true);
}, 0);
const big = await fs.readFile("examples/.fs_fd.txt");
console.log("大文件读取字节:", big.length, "定时器已触发:", ticked);

await fs.remove("examples/.fs_fd.txt");
console.log("清理完成:", await fs.exists("examples/.fs_fd.txt"));
