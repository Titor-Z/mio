// @std/fs 类型声明（Phase 1：Promise 便捷层）

declare module "@std/fs" {
  /** 文件元数据（对齐 Deno 的 FileInfo 命名）。 */
  export interface FileInfo {
    /** 是否为普通文件 */
    isFile: boolean;
    /** 是否为目录 */
    isDirectory: boolean;
    /** 是否为符号链接 */
    isSymlink: boolean;
    /** 文件大小（字节） */
    size: number;
    /** 最后修改时间（Unix 毫秒时间戳） */
    modified?: number;
    /** 最后访问时间（Unix 毫秒时间戳） */
    accessed?: number;
    /** 创建时间（Unix 毫秒时间戳） */
    created?: number;
    /** 权限位（八进制，仅类 Unix 平台） */
    permissions?: number;
  }

  /** 打开模式：只读 / 写截断 / 追加。 */
  export type OpenMode = "r" | "w" | "a";

  /** FD 控制层的文件句柄。 */
  export interface File {
    /** 句柄 id（内部使用） */
    readonly _fd: number;
    /** 读取至多 buffer 长度字节并写回 buffer，返回实际读取字节数 */
    read(buffer: Uint8Array): Promise<number>;
    /** 写入字节，返回写入字节数 */
    write(data: Uint8Array): Promise<number>;
    /** 关闭句柄 */
    close(): Promise<void>;
  }

  /** 打开文件，返回 FD 句柄（mode 缺省为 "r"）。 */
  export function open(path: string, mode?: OpenMode): Promise<File>;

  /** 读取整个文件为字节数组。 */
  export function readFile(path: string): Promise<Uint8Array>;

  /** 读取整个文件为 UTF-8 文本。 */
  export function readTextFile(path: string): Promise<string>;

  /** 写入整个文件（覆盖）。 */
  export function writeFile(path: string, data: Uint8Array): Promise<void>;

  /** 写入 UTF-8 文本到文件（覆盖）。 */
  export function writeTextFile(path: string, text: string): Promise<void>;

  /** 查询文件元数据。 */
  export function stat(path: string): Promise<FileInfo>;

  /** 判断路径是否存在。 */
  export function exists(path: string): Promise<boolean>;

  /** 递归创建目录（已存在则忽略）。 */
  export function mkdir(path: string): Promise<void>;

  /** 列出目录内容（返回完整路径）。 */
  export function readdir(path: string): Promise<string[]>;

  /** 删除文件；若为目录则递归删除。 */
  export function remove(path: string): Promise<void>;

  /** 复制文件。 */
  export function copyFile(src: string, dest: string): Promise<void>;

  /** 重命名/移动文件或目录。 */
  export function rename(oldPath: string, newPath: string): Promise<void>;
}
