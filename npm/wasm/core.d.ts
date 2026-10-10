/**
 * A compiled module: `WebAssembly.Module` when the program has the
 * `WebAssembly` types (lib `dom` or `webworker`), else `object`. So these
 * types need no lib `dom`.
 */
export type WasmModule = typeof globalThis extends { WebAssembly: { Module: abstract new (...args: never) => infer M } }
    ? M
    : object;

/** A position: zero-based line, and UTF-16 character in that line. */
export interface Position {
    line: number;
    character: number;
}

/**
 * A diagnostic (`diagnostics: "json"`). It is the `DiagnosticResponse` of
 * the TypeScript API: positions are UTF-16 offsets, and fields with an empty
 * value are left out.
 */
export interface Diagnostic {
    /** The file, when the diagnostic belongs to one. */
    fileName?: string;
    /** Start offset in the file, or -1. */
    pos: number;
    /** End offset in the file, or -1. */
    end: number;
    startPosition?: Position;
    endPosition?: Position;
    /** The source lines that a renderer needs to show the diagnostic. */
    sourceLines?: { line: number; text: string }[];
    /** The error code, for example 2322 for `TS2322`. */
    code: number;
    /** 0 warning, 1 error, 2 suggestion, 3 message. */
    category: 0 | 1 | 2 | 3;
    /** A code prefix other than the default `TS`. */
    source?: string;
    /** The message text. */
    text: string;
    reportsUnnecessary?: boolean;
    reportsDeprecated?: boolean;
    messageChain?: Diagnostic[];
    relatedInformation?: Diagnostic[];
}

/** undefined or true: it worked. false or an error text: it failed. */
export type WriteResult = boolean | string | void;

/** A file system for `runTsc`. Paths are absolute, with `/` separators. */
export interface HostFileSystem {
    readFile(path: string): Uint8Array | string | undefined;
    /**
     * Follows links. `isFile: false` with `isDirectory: false` is another
     * kind (a FIFO or a device). `mtimeNs` is more exact than `mtimeMs`.
     */
    stat(
        path: string,
    ): { isDirectory: boolean; isFile?: boolean; size?: number; mtimeNs?: bigint; mtimeMs?: number } | undefined;
    readDirectory(path: string): { name: string; kind: "file" | "directory" | "symlink" | "other" }[] | undefined;
    realpath?(path: string): string | undefined;
    /**
     * Makes missing parent directories. On failure, returns false or Go's
     * text of the error (for example `open /a.js: permission denied`),
     * which tsc reports.
     */
    writeFile?(path: string, data: Uint8Array, append: boolean): WriteResult;
    /** Removes a file or a directory tree. Failure as in `writeFile`. */
    remove?(path: string): WriteResult;
    /** Times in nanoseconds since the Unix epoch; undefined keeps a time. */
    chtimes?(path: string, atimeNs: bigint | undefined, mtimeNs: bigint | undefined): WriteResult;
}

export interface MemoryFileSystem extends HostFileSystem {
    /** Every file, written ones included. */
    files: Map<string, string>;
}

/** An in-memory file system. Writes go into the same map. */
export function memoryFileSystem(files?: Map<string, string> | Record<string, string>): MemoryFileSystem;

export interface RunOptions {
    args?: string[];
    /** Absolute, with `/` separators. Default `/`. */
    cwd?: string;
    fs: HostFileSystem;
    env?: Record<string, string | undefined>;
    stdout?(chunk: Uint8Array): void;
    stderr?(chunk: Uint8Array): void;
    /** Return the diagnostics as objects instead of printing them. */
    diagnosticsJson?: boolean;
    caseInsensitive?: boolean;
    /** stdout is a terminal: tsc then defaults to `--pretty`. */
    tty?: boolean;
}

/** Thrown by WASI `proc_exit`; `runTsc` turns it into the exit code. */
export class WasiExit extends Error {
    code: number;
}

/**
 * Runs tsc once, in a new instance of `module`, on the calling thread. A
 * crash (a trap, or running out of stack or memory) throws, with the
 * stderr text so far in `error.stderr`.
 */
export function runTsc(
    module: WasmModule,
    options: RunOptions,
): { exitCode: number; diagnostics?: Diagnostic[] };

/**
 * `runTsc` as a promise. A page's main thread may call it too. Off a page's
 * main thread, where the engine has JSPI (`WebAssembly.promising`), the run
 * gets its own stack, which is about 2 times deeper in a Chrome worker.
 */
export function runTscAsync(
    module: WasmModule,
    options: RunOptions,
): Promise<{ exitCode: number; diagnostics?: Diagnostic[] }>;

export interface LanguageServiceOptions {
    /** In-memory project files, keyed by absolute paths. Copied on creation. */
    files?: ReadonlyMap<string, string> | Readonly<Record<string, string>>;
    /** Compiler arguments, including root files or `-p /path/to/tsconfig.json`. */
    args?: readonly string[];
    cwd?: string;
    caseInsensitive?: boolean;
    /** LSP client capabilities, including semantic-token legend and supported response shapes. */
    capabilities?: unknown;
}

export interface LanguageService {
    /** Requests use LSP method names and parameter/result shapes, with UTF-16 positions. */
    request(method: string, params?: unknown): Promise<unknown>;
    /** Adds or replaces project files. The next request rebuilds the program. */
    updateFiles(files: ReadonlyMap<string, string> | Readonly<Record<string, string>>): Promise<void>;
    deleteFiles(paths: readonly string[]): Promise<void>;
    /** Releases the project. Repeated disposal is safe. */
    dispose(): Promise<void>;
}

/** Creates one editor instance from a compiled module. It never shares state with compiler runs. */
export function createLanguageService(module: WasmModule, options?: LanguageServiceOptions): Promise<LanguageService>;
