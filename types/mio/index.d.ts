// mio 运行时全局环境声明（与实现同步，v0.2）
// 用法：项目内由 `mio init` 生成到 .mio/@types/mio/，不要手改。
// 说明：不包含 DOM（没有 window/document），只提供 mio 实际注入的全局对象。
// 编辑器需配置 "lib": ["ESNext"]，不要引入 DOM 库。

/// <reference path="./std/path.d.ts" />
/// <reference path="./std/fs.d.ts" />

// =========================
// Console
// =========================

declare var console: {
  log(...args: any[]): void;
};

// =========================
// Timers & Microtasks
// =========================

declare function setTimeout<T>(callback: (...args: T[]) => void, ms?: number, ...args: T[]): number;
declare function clearTimeout(id?: number): void;

declare function setInterval<T>(callback: (...args: T[]) => void, ms?: number, ...args: T[]): number;
declare function clearInterval(id?: number): void;

declare function queueMicrotask(callback: () => void): void;

// =========================
// Performance
// =========================

interface Performance {
  /** 自运行时启动起的高精度毫秒数 */
  now(): number;
}

declare const performance: Performance;

// =========================
// URL & URLSearchParams
// =========================

declare class URL {
  constructor(input: string, base?: string | URL);
  readonly href: string;
  readonly origin: string;
  readonly protocol: string;
  readonly username: string;
  readonly password: string;
  readonly host: string;
  readonly hostname: string;
  readonly port: string;
  readonly pathname: string;
  readonly search: string;
  readonly searchParams: URLSearchParams;
  readonly hash: string;
  toString(): string;
}

type URLSearchParamsInit = string | string[][] | Record<string, string> | URLSearchParams;

declare class URLSearchParams implements Iterable<[string, string]> {
  constructor(init?: URLSearchParamsInit);
  /** 键值对数量（数据属性，随改动同步） */
  readonly size: number;
  append(name: string, value: string): void;
  delete(name: string): void;
  get(name: string): string | null;
  getAll(name: string): string[];
  has(name: string): boolean;
  set(name: string, value: string): void;
  sort(): void;
  toString(): string;
  entries(): IterableIterator<[string, string]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  forEach<T>(callback: (value: string, name: string, thisArg: T) => void, thisArg?: T): void;
  [Symbol.iterator](): Iterator<[string, string]>;
}

// =========================
// TextEncoder / TextDecoder
// =========================

declare class TextEncoder {
  readonly encoding: "utf-8";
  encode(input?: string): Uint8Array;
}

interface TextDecoderOptions {
  fatal?: boolean;
  ignoreBOM?: boolean;
}

declare class TextDecoder {
  constructor(encoding?: string, options?: TextDecoderOptions);
  readonly encoding: string;
  readonly fatal: boolean;
  readonly ignoreBOM: boolean;
  decode(input?: ArrayBuffer | ArrayBufferView): string;
}

// =========================
// Blob / FormData
// =========================

interface BlobPropertyBag {
  type?: string;
}

declare class Blob {
  constructor(parts?: (Blob | string | ArrayBuffer | Uint8Array | ArrayBufferView)[], options?: BlobPropertyBag);
  readonly size: number;
  readonly type: string;
  text(): Promise<string>;
  arrayBuffer(): Promise<ArrayBuffer>;
}

/** Blob 的文件形态：仅由 FormData 的文件字段产生，没有独立构造器。 */
type FileLike = Blob & { readonly name: string };

/** 文件字段的类型名；运行时不能 `new File()`，只能从 FormData 取得。 */
type File = FileLike;

declare class FormData implements Iterable<[string, string | Blob]> {
  constructor();
  append(name: string, value: string | Blob, filename?: string): void;
  set(name: string, value: string | Blob, filename?: string): void;
  delete(name: string): void;
  get(name: string): string | Blob | null;
  getAll(name: string): (string | Blob)[];
  has(name: string): boolean;
  forEach<T>(callback: (value: string | Blob, key: string, thisArg: T) => void, thisArg?: T): void;
  entries(): IterableIterator<[string, string | Blob]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string | Blob>;
  [Symbol.iterator](): Iterator<[string, string | Blob]>;
}

// =========================
// Headers
// =========================

type HeadersInit = Record<string, string> | string[][] | Headers;

declare class Headers implements Iterable<[string, string]> {
  constructor(init?: HeadersInit);
  append(name: string, value: string): void;
  set(name: string, value: string): void;
  delete(name: string): void;
  get(name: string): string | null;
  has(name: string): boolean;
  forEach<T>(callback: (value: string, name: string, thisArg: T) => void, thisArg?: T): void;
  entries(): IterableIterator<[string, string]>;
  keys(): IterableIterator<string>;
  values(): IterableIterator<string>;
  [Symbol.iterator](): Iterator<[string, string]>;
}

// =========================
// Response
// =========================

/** Response 构造体：字符串、Blob、ArrayBuffer、缓冲视图或 FormData。 */
type BodyInput = string | Blob | ArrayBuffer | Uint8Array | ArrayBufferView | FormData | null;

interface ResponseInit {
  status?: number;
  statusText?: string;
  headers?: HeadersInit;
}

declare class Response {
  constructor(body?: BodyInput, init?: ResponseInit);
  readonly ok: boolean;
  readonly status: number;
  readonly statusText: string;
  readonly headers: Headers;
  readonly url: string;
  arrayBuffer(): Promise<ArrayBuffer>;
  json(): Promise<any>;
  text(): Promise<string>;
}

// =========================
// Request（仅由 serve 注入，不可自行构造）
// =========================

interface RequestInit {
  method?: string;
  headers?: HeadersInit;
  body?: BodyInput;
  signal?: AbortSignal;
}

declare class Request {
  private constructor();
  readonly method: string;
  readonly url: string;
  readonly headers: Headers;
  readonly body: ArrayBuffer | null;
  arrayBuffer(): Promise<ArrayBuffer>;
  text(): Promise<string>;
  json(): Promise<any>;
}

// =========================
// AbortController / AbortSignal
// =========================

interface AbortSignal {
  readonly aborted: boolean;
  addEventListener(type: "abort", listener: () => void): void;
  removeEventListener(type: "abort", listener: () => void): void;
}

declare class AbortController {
  constructor();
  readonly signal: AbortSignal;
  abort(): void;
}

// =========================
// Fetch
// =========================

interface FetchRequestInit {
  method?: string;
  headers?: Record<string, string>;
  body?: BodyInput;
  signal?: AbortSignal;
}

interface FetchResponse {
  readonly url: string;
  readonly status: number;
  readonly statusText: string;
  readonly ok: boolean;
  readonly headers: Headers;
  text(): Promise<string>;
  json(): Promise<any>;
  arrayBuffer(): Promise<ArrayBuffer>;
}

declare function fetch(url: string, init?: FetchRequestInit): Promise<FetchResponse>;

// =========================
// Serve（mio 特有）
// =========================

interface ServerOptions {
  port?: number;
  /** 仅接受 IP 字面量，如 "127.0.0.1" */
  hostname?: string;
  /** HTTPS 证书 PEM 路径；与 key 必须同时提供 */
  cert?: string;
  /** HTTPS 私钥 PEM 路径 */
  key?: string;
}

declare function serve(
  handler: (request: Request) => Response | Promise<Response>,
  options?: ServerOptions,
): void;
