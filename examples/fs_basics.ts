import * as fs from "@std/fs";

// 写一个文本文件，再读回来
await fs.writeTextFile("examples/.fs_demo.txt", "你好，@std/fs！");
const text = await fs.readTextFile("examples/.fs_demo.txt");
console.log("读到:", text);

// stat 元数据
const info = await fs.stat("examples/.fs_demo.txt");
console.log("是文件:", info.isFile, "大小:", info.size);

// 读目录
const entries = await fs.readdir("examples");
console.log("examples 条目数:", entries.length);

// 清理
await fs.remove("examples/.fs_demo.txt");
console.log("已删除:", await fs.exists("examples/.fs_demo.txt"));
