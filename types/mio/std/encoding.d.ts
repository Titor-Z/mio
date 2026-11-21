// @std/encoding 类型声明

declare module "@std/encoding" {
  /**
   * base64 编码。字符串按 UTF-8 取字节；二进制传 Uint8Array / ArrayBuffer。
   * 输出标准形态（带 "=" 填充）。
   */
  export function encodeBase64(input: string | Uint8Array | ArrayBuffer): string;

  /**
   * base64 解码，返回 Uint8Array。忽略 ASCII 空白、允许省略末尾填充；
   * 非法字符或长度非法时抛 TypeError。
   */
  export function decodeBase64(text: string): Uint8Array;

  /**
   * hex 编码（小写）。字符串按 UTF-8 取字节。
   */
  export function encodeHex(input: string | Uint8Array | ArrayBuffer): string;

  /**
   * hex 解码，返回 Uint8Array。接受大小写；长度必须为偶数，否则抛 TypeError。
   */
  export function decodeHex(text: string): Uint8Array;
}
