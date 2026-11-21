// @std/env 类型声明

declare module "@std/env" {
  /**
   * 读取环境变量；不存在时返回 undefined。
   */
  export function get(name: string): string | undefined;

  /**
   * 判断环境变量是否存在。
   */
  export function has(name: string): boolean;

  /**
   * 设置环境变量。
   *
   * 说明：改动只作用于本进程内 `@std/env` 的视图（mio 不派生子进程，故不存在子进程看不到的问题）；
   * 名称为空或非字符串时抛 TypeError。
   */
  export function set(name: string, value: string): void;

  /**
   * 删除环境变量，返回它此前是否存在。
   */
  export function remove(name: string): boolean;

  /**
   * 导出当前视图的全部环境变量（按键排序，保证输出稳定）。
   */
  export function toObject(): Record<string, string>;
}
