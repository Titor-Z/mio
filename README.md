# mio

TypeScript 一等公民的运行时。TypeScript 源文件直接作为输入语言，运行时内部完成类型擦除与执行。
不提供 Node.js / CommonJS / `node_modules` 兼容层，也不解析 `package.json` 入口。

## 定位

| 项 | 说明 |
|---|---|
| 输入语言 | TypeScript（JavaScript 视为其子集，单一文法路径） |
| 模块体系 | 纯 ES Module；相对路径、绝对路径、`file:` URL、`imports` 映射、`@std/*` 内建命名空间 |
| 包管理 | 无。不查找 `node_modules`，不支持裸包名 |
| 标准库 | Web 标准库（全局对象）+ `@std/*`（模块导入） |
| 类型处理 | 类型注解与类型声明在编译期擦除；`enum` / `namespace` / 参数属性降级为等价 JavaScript |

## 两阶段路线

- **阶段 A（当前）**：TypeScript 源码 → 类型擦除 → 执行。转译由 OXC 承担，执行由 Boa 承担，两者分别隔离在 `transpiler` 与 `engine` 模块内。
- **阶段 B（预留）**：自研词法、语法、语义与字节码虚拟机，直接消费类型信息并输出原生代码。

两阶段之间只保留两个接缝：`engine::Engine` 抽象与 `resolver::Specifier` 分类。其余模块在两个阶段之间复用。

## 构建

```bash
cargo build            # 调试构建
cargo test             # 单元测试
cargo clippy --all-targets -- -D warnings
cargo fmt --check
```

工具链固定为 Rust 1.99.1（见 `rust-toolchain.toml`）。OXC 0.156 与 Boa 0.22 均可在稳定版 Rust 上编译，无需 nightly。

## 使用

```bash
mio run app.ts         # 执行 TypeScript 文件
mio eval "..."         # 执行内联源码
mio repl               # 交互式求值
mio check app.ts       # 仅类型检查（内置 tsrs 编译器）
mio task <name>        # 执行 mio.json 声明的任务
```

## 目录结构

```
src/
  main.rs        进程入口：解析参数、分发命令、转换退出码
  lib.rs         模块树与全局常量
  cli/           命令行参数解析与命令分发
  config/        mio.json 解析、发现与优先级叠加
  transpiler/    TypeScript → JavaScript（阶段 A 由 OXC 承担）
  resolver/      模块说明符分类与 Import Maps 重写
  loader/        模块实例缓存与加载
  engine/        执行引擎抽象（阶段 A 为 Boa）
  sourcemap/     错误位置映射回 TypeScript 源码
  web/           Web 标准库（全局对象）
  stdlib/        @std/* 内建模块
```

## 配置

配置文件为 `mio.json`，支持 JSONC 注释，从入口文件所在目录向上查找最近一份，找到即止。
字段优先级：内建默认 < `mio.json`（含 `extends` 链） < `MIO_*` 环境变量 < 命令行选项。

骨架借鉴 `tsconfig.json`：顶层 `compilerOptions` 承载编译器开关，`include` / `exclude` / `files` 承载文件选择，`extends` 承载继承。
与 `tsconfig.json` 的关键差异：模块映射不使用 `paths` / `baseUrl`。`paths` 是编译期概念，不影响产物且依赖打包器配合；mio 的模块解析发生在运行期，因此采用 Import Maps 规范的 `imports` / `scopes`。

```jsonc
{
  "$schema": "https://mio.dev/schema/mio.json",
  "extends": "./base/mio.json",

  "compilerOptions": {
    "target": "es2025",
    "jsx": "preserve",
    "useDefineForClassFields": true,
  },

  "include": ["src/**/*.ts"],
  "exclude": [".mio", "dist"],

  "imports": { "@app/": "./src/" },

  "stdlib": { "web": "living", "disable": ["fetch"] },
  "run": { "timeoutMs": 30000 },
  "tasks": { "start": "run src/main.ts" },
}
```

### 字段表

字段分三档生效状态：**生效**（真实影响行为）、**仅提示**（记录并告警）、**预留**（解析但不参与运行）、**不采用**（命中时告警并建议替代方案）。

| 字段 | 默认 | 状态 | 说明 |
|---|---|---|---|
| `extends` | 无 | 生效 | 单个路径或数组；JSON 值深合并，子配置覆盖父配置；检出循环继承 |
| `compilerOptions.target` | `es2025` | 生效 | 传给转译层的目标版本 |
| `compilerOptions.jsx` | `preserve` | 生效 | `preserve` / `react` / `react-jsx` |
| `compilerOptions.jsxFactory` | `React.createElement` | 生效 | 仅在 `jsx` 为 `react` 时有意义 |
| `compilerOptions.useDefineForClassFields` | `true` | 生效 | 类字段定义语义 |
| `compilerOptions.module` | `esnext` | 仅提示 | mio 只有 ES Module 一种形态 |
| `compilerOptions.moduleResolution` | `mio` | 仅提示 | 解析规则固定 |
| `compilerOptions.lib` | `["es2025","web"]` | 仅提示 | 类型面，非运行时面 |
| `compilerOptions.strict` | `false` | 生效 | `true` 时向内置 tsrs 类型检查器传入 `--strict` |
| `compilerOptions.sourceMap` | `true` | 生效 | 运行期错误栈帧行列反查回 TypeScript 源；关闭则退回 JS 坐标 |
| `compilerOptions.paths` | 无 | 不采用 | 改用 `imports`，命中时告警 |
| `compilerOptions.baseUrl` | 无 | 不采用 | 改用 `imports`，命中时告警 |
| `include` / `exclude` / `files` | 空 | 生效 | 通配模式，支持 `*`、`?`、`**`；`files` 命中优先，`exclude` 次之 |
| `imports` | 空 | 生效 | Import Maps；`/` 结尾为前缀映射，否则精确映射 |
| `scopes` | 空 | 预留 | 作用域映射，解析但不参与决策 |
| `stdlib.web` | `living` | 仅提示 | WHATWG 无版本号，非 `living` 会告警 |
| `stdlib.disable` | 空 | 生效 | 从全局注入中剔除的对象名 |
| `module.root` | `.` | 生效 | 绝对说明符的基准目录 |
| `module.extensions` | `.ts,.tsx,.mts,.js,.mjs,.json` | 生效 | 扩展名补全顺序 |
| `check.typecheck` | `off` | 生效 | `off` 或 `on`；`on` 时 `mio run` 执行前先类型检查 |
| `run.timeoutMs` | `30000` | 生效 | 事件循环超时 |
| `build.outDir` | `.mio/cache` | 生效 | 预转译产物目录（里程碑 3） |
| `build.sourcemap` | `true` | 生效 | 是否落盘源码映射（里程碑 3） |
| `log.level` | `info` | 生效 | `silent` / `error` / `warn` / `info` / `debug` |
| `log.json` | `false` | 生效 | 结构化日志 |
| `tasks` | 空 | 生效 | `mio task` 读取 |
| `test.include` / `test.exclude` | 空 | 预留 | 测试文件选择，解析但不执行 |

### 环境变量

| 变量 | 作用 |
|---|---|
| `MIO_TYPECHECK` | 覆盖 `check.typecheck` |
| `MIO_TIMEOUT_MS` | 覆盖 `run.timeoutMs` |
| `MIO_LOG_LEVEL` | 覆盖 `log.level` |
| `MIO_LOG_JSON` | 覆盖 `log.json`，取值 `1` / `true` / `yes` |
| `MIO_STDLIB_WEB` | 覆盖 `stdlib.web` |

未定义的 `$VAR` 引用原样保留，不替换为空串，避免路径静默指向意外位置。

### 未知字段处理

不启用严格模式：字段拼错时仍可运行，但会输出「未知字段 + 最接近的合法字段」告警：

```
mio: compilerOptions.targett：编译选项拼写可能有误，是否想写 target
```

## 与 Node.js / Deno 的边界

不实现的能力是设计选择，不是缺失：

| 能力 | 状态 | 原因 |
|---|---|---|
| `node_modules` 解析 | 不支持 | 无包管理概念 |
| CommonJS `require` | 不支持 | 单一 ES Module 路径 |
| `package.json` 入口推断 | 不支持 | 模块说明符必须显式 |
| 裸包名（如 `lodash`） | 不支持 | 加载时给出明确报错并建议改为相对路径或 `@std/*` |

## 许可

MIT
