import type { Diagnostic, WasmModule } from "./core.js";
import type { LanguageService, LanguageServiceOptions } from "./core.js";

export type { Diagnostic, HostFileSystem, MemoryFileSystem, Position, RunOptions, WasmModule } from "./core.js";
export type { LanguageService, LanguageServiceOptions } from "./core.js";
export { memoryFileSystem, runTsc, runTscAsync } from "./core.js";

/**
 * The instance type of the global class `Name` (`URL`, `Response`) when the
 * program has it (lib `dom` or `webworker`, or `@types/node`), else `never`.
 */
type GlobalClass<Name extends string> = typeof globalThis extends Record<Name, abstract new (...args: never) => infer T>
    ? T
    : never;

export interface TscOptions {
    /**
     * The files of an in-memory run: absolute path to text. In Node, leave
     * it out to use the real file system.
     */
    files?: Map<string, string> | Record<string, string>;
    /** The current directory. Default: `/`, or in Node without `files`, `$PWD` when it names the current directory (as tsgo does), else `process.cwd()`. */
    cwd?: string;
    /** `"json"`: return the diagnostics as objects instead of printing them. */
    diagnostics?: "text" | "json";
    env?: Record<string, string | undefined>;
    /** Node: stdout is a terminal. */
    tty?: boolean;
    /** Node: write the output to this process's stdout and stderr as it comes. */
    stream?: boolean;
    /** Node: the stack of the run's worker thread, in MB (default 256). */
    stackSizeMb?: number;
    /** Browser: where to load the module from (see `loadModule`). */
    wasm?: string | GlobalClass<"URL"> | GlobalClass<"Response"> | ArrayBuffer | ArrayBufferView | WasmModule;
}

export interface TscResult {
    exitCode: number;
    stdout: string;
    stderr: string;
    /** With `diagnostics: "json"`. */
    diagnostics?: Diagnostic[];
    /** In-memory runs: every file after the run, emitted ones included. */
    files?: Map<string, string>;
}

/** Runs `tsc` with `args`. */
export function tsc(args: string[], options?: TscOptions): Promise<TscResult>;

/** The compiled module. The browser entry takes where to load it from. */
export function loadModule(source?: TscOptions["wasm"]): Promise<WasmModule>;

/** Creates an editor service. Node owns a worker with `stackSizeMb`; use a worker in browsers. */
export function createLanguageService(options?: LanguageServiceOptions & Pick<TscOptions, "wasm" | "stackSizeMb">): Promise<LanguageService>;
