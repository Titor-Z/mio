// @std/os 类型声明

declare module "@std/os" {
  /**
   * 操作系统名（Deno 口径）：macOS 返回 "darwin"，其余为 "linux" / "windows" / "freebsd" 等。
   */
  export function platform(): string;

  /**
   * CPU 架构名：如 "x86_64" / "aarch64"。
   */
  export function arch(): string;

  /**
   * 主机名。优先取环境变量，其次 `/etc/hostname`，都拿不到时为 "localhost"。
   */
  export function hostname(): string;

  /**
   * 当前用户主目录（HOME / USERPROFILE）；都缺失时返回 undefined。
   */
  export function homedir(): string | undefined;

  /**
   * 临时目录（TMPDIR / TEMP / TMP），都缺失时返回 "/tmp"。
   */
  export function tmpdir(): string;

  /**
   * 当前工作目录。目录已被删除等情况下抛 TypeError。
   */
  export function cwd(): string;

  /**
   * 当前进程号。
   */
  export function pid(): number;

  /**
   * 平台换行符：Windows 为 "\r\n"，其余为 "\n"。
   */
  export const eol: string;
}
