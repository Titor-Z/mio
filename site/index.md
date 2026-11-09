---
layout: home

hero:
  name: MIO
  text: TypeScript 运行时
  tagline: TypeScript 源码直接作为输入。Web 标准 API 开箱即用。没有 node_modules，没有 CommonJS，没有兼容层。
  actions:
    - theme: brand
      text: 开始使用
      link: /guide
    - theme: alt
      text: 快速参考
      link: /cheat-sheet

features:
  - icon: "01"
    title: 直接执行 TS
    details: 类型在运行时内部擦除，enum / namespace / 参数属性降级为等价 JavaScript。没有构建目录，没有额外打包步骤。
  - icon: "02"
    title: Web 标准
    details: fetch / Request / Response / Headers / URL / Blob / FormData / TextEncoder 全部内置，语义对齐 WHATWG，而不是 Node 的私有形态。
  - icon: "03"
    title: 内置服务端
    details: serve(handler, options) 直接起 HTTP/HTTPS 服务，同步或异步 handler 都能工作，keep-alive 与并发连接已就绪。
  - icon: "04"
    title: 无包管理
    details: 只有 ES Module。相对路径、Import Maps、@std/* 内建命名空间；不查 node_modules，不解析 package.json。
  - icon: "05"
    title: 编辑器有提示
    details: mio init 生成 .mio/@types/mio/ 与 tsconfig.json，任意 TypeScript LSP 都能识别全局与 @std 模块，无需专用插件。
  - icon: "06"
    title: 纯 Rust 运行时
    details: 转译由 OXC 承担，执行由 Boa 承担，TLS 使用 rustls 纯 Rust 后端。不引入 C 依赖。
  - icon: "07"
    title: 高性能文件系统
    details: "@std/fs 提供 Promise 风格读写，IO 走工作线程池不阻塞事件循环；支持 FD 控制层分块与流式读写。"
---
