//! 冒烟用例：`fib(20)` 与 `print` 必须跑通（TL20261110T041429 的验收底线）。

/// 用例源码与期望 stdout 的对应表。
pub const CASES: &[(&str, &str, &str)] = &[
    // 递归 fib：覆盖函数调用、整数运算、比较、return。
    (
        "fib",
        "function fib(n: i64): i64 {\n\
         \x20 if (n < 2) {\n\
         \x20   return n;\n\
         \x20 }\n\
         \x20 return fib(n - 1) + fib(n - 2);\n\
         }\n\
         function main(): void {\n\
         \x20 print(fib(20));\n\
         }\n",
        "6765\n",
    ),
    // print 全类型分派 + 字面量类型服从标注。
    (
        "print-types",
        "function main(): void {\n\
         \x20 let a: i16 = 10;\n\
         \x20 let b: i32 = 20;\n\
         \x20 let c: i64 = 30;\n\
         \x20 let d: u16 = 40;\n\
         \x20 let e: u32 = 50;\n\
         \x20 let f: u64 = 60;\n\
         \x20 let g: f32 = 1.5;\n\
         \x20 let h: f64 = 2.5;\n\
         \x20 let i: bool = true;\n\
         \x20 print(a); print(b); print(c); print(d); print(e); print(f);\n\
         \x20 print(g); print(h); print(i);\n\
         }\n",
        "10\n20\n30\n40\n50\n60\n1.5\n2.5\ntrue\n",
    ),
    // for 三段式 + 赋值 + 自增 desugar + break/continue。
    (
        "loops",
        "function main(): void {\n\
         \x20 let sum: i32 = 0;\n\
         \x20 for (let i: i32 = 0; i < 10; i = i + 1) {\n\
         \x20   if (i == 3) {\n\
         \x20     continue;\n\
         \x20   }\n\
         \x20   if (i == 8) {\n\
         \x20     break;\n\
         \x20   }\n\
         \x20   sum = sum + i;\n\
         \x20 }\n\
         \x20 print(sum);\n\
         \x20 let n: i32 = 0;\n\
         \x20 while (n < 5) {\n\
         \x20   n++;\n\
         \x20 }\n\
         \x20 print(n);\n\
         }\n",
        "25\n5\n",
    ),
    // if / else if / else + 三元 + 逻辑运算。
    (
        "branch",
        "function classify(n: i32): i32 {\n\
         \x20 if (n < 0) {\n\
         \x20   return 1;\n\
         \x20 } else if (n == 0) {\n\
         \x20   return 0;\n\
         \x20 } else {\n\
         \x20   return -1;\n\
         \x20 }\n\
         }\n\
         function main(): void {\n\
         \x20 print(classify(-5));\n\
         \x20 print(classify(0));\n\
         \x20 print(classify(7));\n\
         \x20 let big: bool = 7 > 3 && 2 < 5;\n\
         \x20 print(big);\n\
         \x20 let small: bool = !(7 > 3);\n\
         \x20 print(small);\n\
         \x20 let either: i32 = big ? 100 : 200;\n\
         \x20 print(either);\n\
         }\n",
        "1\n0\n-1\ntrue\nfalse\n100\n",
    ),
    // `as` 白名单：同符号整数扩宽 + f32→f64。
    (
        "cast",
        "function main(): void {\n\
         \x20 let small: i16 = 1234;\n\
         \x20 let wide: i64 = small as i64;\n\
         \x20 let wide2: i32 = small as i32;\n\
         \x20 let f: f32 = 2.5;\n\
         \x20 let d: f64 = f as f64;\n\
         \x20 print(wide); print(wide2); print(d);\n\
         }\n",
        "1234\n1234\n2.5\n",
    ),
    // 有符号/无符号除法与取余。
    (
        "div-rem",
        "function main(): void {\n\
         \x20 let a: i32 = -7;\n\
         \x20 let b: i32 = 2;\n\
         \x20 print(a / b); print(a % b);\n\
         \x20 let c: u32 = 7;\n\
         \x20 let d: u32 = 2;\n\
         \x20 print(c / d); print(c % d);\n\
         }\n",
        "-3\n-1\n3\n1\n",
    ),
    // 小函数内联：循环内调用 add（内联后应与手工展开版同输出）。
    (
        "inline-loop",
        "function add(a: i64, b: i64): i64 {\n\
         \x20 return a + b;\n\
         }\n\
         function main(): void {\n\
         \x20 let sum: i64 = 0;\n\
         \x20 for (let i: i64 = 0; i < 100; i = i + 1) {\n\
         \x20   sum = add(sum, i);\n\
         \x20 }\n\
         \x20 print(sum);\n\
         }\n",
        "4950\n",
    ),
    // 嵌套语境（print 的实参是调用）不展开，函数定义保留，输出不变。
    (
        "inline-nested-kept",
        "function add(a: i32, b: i32): i32 {\n\
         \x20 return a + b;\n\
         }\n\
         function main(): void {\n\
         \x20 print(add(40, 2));\n\
         }\n",
        "42\n",
    ),
];

/// 应当被拒绝的源码与错误关键字。
pub const REJECTED: &[(&str, &str, &str)] = &[
    // 无隐式转换：i32 变量接收 f64。
    (
        "no-implicit-conversion",
        "function main(): void {\n  let x: i32 = 1.5;\n}\n",
        "不能赋给整数类型",
    ),
    // 缺类型标注。
    (
        "missing-annotation",
        "function main(): void {\n  let x = 1;\n}\n",
        "类型标注",
    ),
    // 缺入口。
    (
        "missing-main",
        "function helper(): void {}\n",
        "缺少入口函数 main",
    ),
    // 缩窄 cast 不在白名单。
    (
        "narrowing-cast",
        "function main(): void {\n  let a: i64 = 1;\n  let b: i32 = a as i32;\n}\n",
        "不在白名单",
    ),
    // 跨符号 cast 拒绝。
    (
        "cross-sign-cast",
        "function main(): void {\n  let a: i32 = 1;\n  let b: u32 = a as u32;\n}\n",
        "不在白名单",
    ),
    // 未知函数调用。
    (
        "unknown-call",
        "function main(): void {\n  print(nope(1));\n}\n",
        "未声明",
    ),
    // 浮点取余不支持。
    (
        "float-rem",
        "function main(): void {\n  let a: f64 = 1.0;\n  let b: f64 = a % 2.0;\n  print(b);\n}\n",
        "浮点取余",
    ),
    // 字符串不支持（MVP 无堆）。
    (
        "string-unsupported",
        "function main(): void {\n  let s: string = \"x\";\n}\n",
        "不支持",
    ),
];
