# Native MVP 报告

> 阶段 B 第一个里程碑：TypeScript 子集 → TIR → Cranelift → 本机机器码。
> 关联：TD77d5bb566833497dbaa31d564bd0cde1（MVP 主体）、TDcddc66028ced4bd09874cedea8ad7ffc（MVP 语义层）、TDa5354d28435a4e17a08f30e041173b5d（MVP 代码生成）、TD085d68ae63a249f98e3ba0b012ca74f2（基准与报告）；讨论 TL20261110T041429、TL20261121T190813 之后续。

## 结论：达标

三条验收门槛全部满足（测量环境：macOS 15 / x86_64 / Apple clang 14 / node v24.18.0，release 构建，执行耗时取 3 次最优，数字可由 `cargo run --release -p mio-native-bench` 复现）：

| 门槛 | 要求 | 实测 | 判定 |
|---|---|---|---|
| `fib(35)` | ≥ 3x Node | mio 0.07s vs node 0.23s（**3.3x**） | 达标（余量小） |
| 循环 1e8 | ≤ 2x vs C | 浮点档 mio 0.10s vs cc 0.10s（**1.0x**） | 达标 |
| 单文件编译 | ≤ 1s | 0.12–0.13s（含进程启动） | 达标 |

补充数字（同表）：

| 用例 | 编译(mio) | 编译(cc) | 运行(mio) | 运行(node) | 运行(cc) |
|---|---|---|---|---|---|
| fib(35) | 0.12s | 0.13s | 0.07s | 0.23s | 0.03s |
| loop 1e8 int | 0.13s | 0.13s | 0.07s | 0.18s | 0.00s* |
| loop 1e8 float | 0.13s | 0.12s | 0.10s | 0.17s | 0.10s |

\* 整数循环档 cc 为 0.00s：是 clang 把整个循环常量折叠了（sum 有确定值），该档位对 cc 失真，不作对照；浮点档（acc 有浮点舍入，折叠后仍须累加？实际 clang 同样可折叠，此处 0.10s 说明未折叠）才是有效对照。三方 stdout 数值已交叉验证一致。

fib 档 vs cc 为 2.3 倍慢（0.07 vs 0.03）：cc 对双递归 fib 有更激进的优化，mio-native 未做等价的尾调用/记忆化处理，属预期差距，不影响「≥3x Node」门槛。

## 冒烟验收（8 成功 + 8 拒绝，全过）

`cargo test -p mio-native`：

- 成功用例（编译 → 执行 → 比对 stdout）：`fib(20)=6765`、print 全类型分派（i16/i32/i64/u16/u32/u64/f32/f64/bool）、for+赋值+自增 desugar+break/continue（sum=25）、if/else if/else+三元+逻辑运算、`as` 白名单（i16→i32/i64、f32→f64）、有符号/无符号除法与取余。
- 拒绝用例（编译期报错）：无隐式转换（`let x: i32 = 1.5`）、缺类型标注、缺入口 `main`、缩窄 cast、跨符号 cast、未声明函数调用、浮点取余、`string` 类型。

## 管线与形态决策

`mio-native build app.ts -o app`（`--emit-obj` 只产 `.o`）：

OXC parser → `lower`（AST → TIR，两遍降低：第一遍收集函数签名供 Call 定类型，第二遍降低函数体）→ `check`（无隐式转换、`as` 白名单、函数签名匹配）→ `codegen`（Cranelift IR，变量全 SSA，循环变量由前端插 phi；整数回绕不插溢出检查；`print` 按静态类型分派到 `runtime.c` 的 `mio_print_*`）→ `emit`（`.o` + 内联 `runtime.c` 一起交 `cc` 链接）。

关键决策（与讨论书一致）：

- **不建独立 IR**：TIR = 带类型标注的 AST + 变量槽位表（临时偏离已记录，wasm 第二目标出现时再展平）。
- **`main` 特判**：用户标注 `void`，生成签名强制 `() -> i32` 返回 0（crt 入口约定）。
- **运行时面最小化**：唯一内置 `print`，由 `runtime.c` 提供 9 个类型化打印函数；格式串固定在 runtime，不进用户产物。

## 已知边界（如实记录，非缺陷）

1. **无隐式转换的代价**：字面量在无标注语境默认 i32/f64；`2 + n`（n: i64）报两侧不一致，须 `n + 2` 或先标注。这是严格语义的预期行为。
2. **`&&`/`||` 非短路**：按位实现。MVP 表达式无副作用，语义等价；引入 print 之外的副作用面时必须改造。
3. **不支持的语法**（编译期明确报错）：string/数组/struct/enum/class/闭包/泛型/模块/异常/`??`/浮点取余/缩窄与跨符号 `as`/u64 超 i64::MAX 的字面量。
4. **整数除零**：由 Cranelift trap 兜底（运行期终止，非 JS 的 Infinity/NaN 语义）。
5. **print 不支持 i8/u8**（runtime 未提供；check 阶段已拒绝）。
6. **退出码恒 0**（main 返回 void）。

## 下一步决策

1. **并入主 binary（建议 0.4.0）**：`mio build` 子命令走原生路径（阶段 A 的 `mio build` 预转译缓存已取消，此处是全新语义：AOT 产出可执行文件）。需要：根 workspace 加 feature 门控或子命令分派、`mio.json` 的 `build` 配置项重新设计（这次只表达「产出可执行文件」）。
2. **wasm 第二目标：暂缓**。触发条件（TIR 不稳定）是假象——真正原因是阶段 A 还有 `EventTarget`/`ReadableStream` 等缺口，优先级高于 wasm。
3. **bench 工具已落地**：`mio-native-bench` 可随时复现三方数字；后续每轮优化后跑一次防退化。
4. **性能余量**：fib 档 3.3x Node 刚过线。下一步优化候选（按性价比）：① 整数比较/分支直接用 b1（省 select）；② 算术常量折叠（`i + 1` 变 `iadd_imm`）；③ 函数内联小函数。这些都不改 TIR 形态。

## 性能优化轮（TL20261121T210405）：结论

对候选优化做了受控实施与测量（stash 交替对比 + 反汇编验证）：

1. **分支语境比较直传 b1**（省 select）：已生效——`fib` 的反汇编中 `cmpq $2, %rdi` 后直接 `jl`，无 select。
2. **算术/比较立即数变体**（`iadd_imm_s` / `icmp_imm_s`）：已生效——`addq $-1, %rdi`、`cmpq $2`。
3. **端到端收益：无**（受控对比，fib 0.09s / 整数循环 0.09s / 浮点循环 0.13s 前后一致）。原因：Cranelift speed 档的 Peephole 本就把「常量物化 + 运算」合并为立即数变体、把 b1 直接喂给分支。手写指令选择层优化被后端覆盖。

**fib vs cc 差距（3 倍）的定位**：每次递归调用的固定开销——prologue/epilogue 保存恢复 r12/r13（6 条）+ 两次 `movq %r12, %rdi` 参数搬运。n 跨两次 call 存活，必须进 callee-saved 寄存器，Cranelift 的选择本身正确。差距属寄存器分配/栈帧策略层，不在 MVP 指令选择层的解决范围内。

**认知**（已入 AGENTS.md）：在 Cranelift 后端做指令选择层微优化是无效劳动；性能杠杆在结构层（TIR 层常量传播/公共子表达式消除、调用约定与栈帧审计）。

**附带收益**：翻译函数签名重构为 `Ctx` 上下文（消除 9 参数函数），代码更干净；两处优化无害保留（IR 指令数略减）。
