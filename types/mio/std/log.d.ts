// @std/log 类型声明

declare module "@std/log" {
  /**
   * 日志级别，与 mio.json 的 `log.level` 同一套取值。
   * silent 关闭全部输出；其余为阈值（error 只出 error，debug 全出）。
   */
  export type Level = "silent" | "error" | "warn" | "info" | "debug";

  /** 输出 debug 日志（低于当前阈值时静默丢弃）。 */
  export function debug(...data: unknown[]): void;

  /** 输出 info 日志。 */
  export function info(...data: unknown[]): void;

  /** 输出 warn 日志。 */
  export function warn(...data: unknown[]): void;

  /** 输出 error 日志。 */
  export function error(...data: unknown[]): void;

  /**
   * 运行时修改日志级别（影响后续所有调用）；非法取值抛 TypeError。
   */
  export function setLevel(level: Level): void;

  /** 读取当前日志级别。 */
  export function getLevel(): Level;
}
