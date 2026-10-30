# `compiler/` —— TypeScript 编译器（fork 自 `oxc-project/tsrs`）

这个目录专门承载 **TypeScript 编译器**本身：词法、语法、绑定、类型检查、`.d.ts` 生成。
它与 `src/`（运行时、引擎、模块加载、标准库）平级，是 mio 的「前端」半边。

## 来历与定位

- 源码 fork 自 `oxc-project/tsrs`（Apache-2.0，`rust-version 1.99`，稳定版构建、无 nightly
  feature gate）。它是微软 TypeScript 7 的 Rust 移植，跑上游 `compiler / conformance /
  fourslash` 一致性测试。
- 选型结论见 `docs/Taolun.md`（相对官方 Go 版 tsgo，Rust 版且 Apache-2.0 可 fork，才能
  让我们往里加自定义语法/类型）。
- **终局叙事**：阶段 B 的「自研前端」= 以本目录为地基做**增量定制**（机器类型 `i32`/`f64`
  等、表示决策表、native IR 后端），不再从零手写 TS 编译器。
- 保留上游 `LICENSE`/`NOTICE` 与文件头 copyright，并以 `Modified by mio` 标注改动。

## 目录结构（落地后）

```
compiler/
  Cargo.toml            # 独立 workspace，或并入根 workspace（见下「集成形态」）
  LICENSE / NOTICE      # 随 fork 带入，保留
  crates/
    tsrs_scanner/       # 词法（对应并取代 src/lexer 的自研试验）
    tsrs_parser/        # 语法
    tsrs_ast/           # AST
    tsrs_binder/        # 符号绑定（我们在此挂机器类型）
    tsrs_checker/       # 类型检查（mio check 的后端）
    tsrs_diagnostics/   # 诊断（TS 行列，供错误定位）
    ...                 # 其余 crate 按阶段逐步纳入，不一次吞 39 个
```

## 集成形态（待定，二选一）

- **方案一（推荐，先做）**：`compiler/` 为**独立 workspace**，`mio` 主 crate 只以 path 依赖
  引入 `checker/binder/parser/diagnostics` 等**必需的几个** crate。好处：主构建不被未适配源码
  拖挂，增量可控；坏处：跨 workspace 的 path 依赖需协调依赖版本。
- **方案二**：把整个仓库升级为**单一 workspace**（members = 根 mio + `compiler/crates/*`），
  统一 `Cargo.lock`，直接验证 boa / oxc / tsrs 三方依赖能否共存。坏处：任一次 `cargo build`
  会编译整条编译器，且全仓库须锁 1.99。

## 阶段计划

1. **可行性验证**：升 `rust-toolchain.toml` 到 `1.99.1`；把 `crates/tsrs_*` 源码拉入本目录；
   做一次试编译，检查是否与 boa 0.22 / oxc 0.156 的 pin（rustc-hash、hashbrown、indexmap、
   smallvec 等）冲突。**这一步的结果决定「直接并入」还是「退回对照自研」。**
2. **裁剪落地**：先纳入 `tsrs_scanner / parser / ast / binder / checker / diagnostics`，
   `mio check` 接 `tsrs_checker` 做真·类型检查；`src/lexer` 的自研试验降级为对照 oracle。
3. **定制扩展**：在 binder/checker 里加入机器类型与表示决策（阶段 B），逐步替换 Boa 为自研 VM。

## 当前状态

**阶段 1（可行性验证）已通过**（2026-10-30，工具链 `1.99.1`）：

- `tsrs_checker` 在**稳定版 1.99.1** 上编译通过（1m44s），**无 nightly feature gate**。
- `mio`（boa 0.22 + oxc 0.156）在 1.99.1 上编译通过（3m02s），向后兼容，无需补丁。
- 依赖共存：rustc-hash/indexmap/smallvec/bitflags/serde/base64/num-bigint/memchr 均可共用；
  **唯一注意项**是 `hashbrown`（mio 0.14 vs tsrs 0.17）会在图中各存一份共存——非冲突、能编，
  且 boa/oxc 与 tsrs 不经公共 API 互传 hashbrown 类型，故不阻塞。
- 待办：正式导入 `crates/tsrs_*` 到 `compiler/`、升 `rust-toolchain.toml` 到 1.99.1、
  确认 path 依赖只引所需 crate（不触发 tsrs 二进制的 mimalloc global_allocator）。
- 源码缓存：`/tmp/tsrs-probe`（浅克隆，供正式导入拷贝）。
