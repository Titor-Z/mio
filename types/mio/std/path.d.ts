// @std/path 类型声明（第 0.1 版）

declare module "@std/path" {
  /**
   * 拼接路径各段并归一化，忽略空段；结果为空时返回 "."
   */
  export function join(...pathSegments: string[]): string;

  /**
   * 归一化路径：折叠重复斜杠、消除 . 与 ..，保留绝对性；空输入返回 "."
   */
  export function normalize(path: string): string;

  /**
   * 获取目录名：去掉最后一段；无目录部分时返回 "."（相对）或 "/"（绝对）
   */
  export function dirname(path: string): string;

  /**
   * 获取文件名：去掉目录部分；可选后缀过滤
   * @param path 待解析路径
   * @param suffix 要移除的后缀（可选）
   */
  export function basename(path: string, suffix?: string): string;

  /**
   * 获取扩展名：最后一个"."之后的部分
   */
  export function extname(path: string): string;

  /**
   * 路径分隔符（POSIX 风格为 "/"）
   */
  export const separator: "/";
}
