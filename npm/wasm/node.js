// Node entry. Each run gets its own worker thread, because the checker
// recurses deeply and a worker can have a large stack (`stackSizeMb`); the
// main thread's stack is about 1 MB. The worker makes a new instance of
// the module, which is compiled once per process.

import { statSync } from "node:fs";
import { readFile } from "node:fs/promises";
import { Worker } from "node:worker_threads";
import { createLanguageService as createCoreLanguageService } from "./core.js";
export { memoryFileSystem, runTsc, runTscAsync } from "./core.js";

/** Creates a persistent, in-memory editor service on this thread. */
export async function createLanguageService(options = {}) {
    return createCoreLanguageService(await loadModule(), options);
}

const wasmUrl = new URL("./ts_rust.wasm", import.meta.url);

/** Stack size of a run's worker thread, in MB. */
const STACK_SIZE_MB = 256;

let modulePromise;

/** The compiled module, compiled on first use. */
export function loadModule() {
    modulePromise ??= readFile(wasmUrl).then(bytes => WebAssembly.compile(bytes));
    return modulePromise;
}

/**
 * Runs `tsc` with `args`.
 *
 * Without `options.files`, it reads and writes the real file system from
 * `options.cwd` (default: the current directory as `getwd` gives it, the
 * same path as the native tsgo). With `options.files` (path to
 * text, absolute paths), it reads only those files, and `result.files`
 * holds every file after the run, emitted ones included.
 *
 * Returns `{ exitCode, stdout, stderr, diagnostics?, files? }`.
 * `options.diagnostics: "json"` returns the diagnostics as objects in
 * `diagnostics` and does not print them.
 */
export async function tsc(args, options = {}) {
    const module = await loadModule();
    const files = options.files === undefined
        ? undefined
        : options.files instanceof Map
        ? options.files
        : new Map(Object.entries(options.files));
    const request = {
        module,
        args,
        cwd: toPosix(options.cwd ?? getwd()),
        files,
        env: options.env ?? {},
        diagnosticsJson: options.diagnostics === "json",
        tty: options.tty ?? false,
        stream: options.stream ?? false,
    };
    // Bun ignores `stackSizeMb`, and its workers have less stack than its
    // main thread, so the run stays on this thread there.
    if (process.versions.bun) {
        const { runRequest } = await import("./node-run.js");
        return settle(runRequest(request));
    }
    const worker = new Worker(new URL("./node-worker.js", import.meta.url), {
        workerData: request,
        resourceLimits: { stackSizeMb: options.stackSizeMb ?? STACK_SIZE_MB },
        stdout: !options.stream,
        stderr: !options.stream,
    });
    return new Promise((resolve, reject) => {
        let result;
        worker.on("message", message => (result = message));
        worker.on("error", reject);
        worker.on("exit", () => {
            try {
                if (!result) throw new Error("ts-rust worker ended without a result");
                resolve(settle(result));
            } catch (error) {
                reject(error);
            }
        });
    });
}

/** The result of `runRequest`, or its crash as a thrown error. */
function settle(result) {
    if (result.error) throw Object.assign(new Error(result.error), { stderr: result.stderr });
    return result;
}

/**
 * The current directory as Go's `os.Getwd` gives it: `$PWD` when it is an
 * absolute path to the current directory, else `process.cwd()`. The two
 * differ when a shell entered the directory through a symlink (`/tmp` on
 * macOS is `/private/tmp`). Then `process.cwd()` is the real path, and tsc
 * would print other paths than the native tsgo.
 */
function getwd() {
    const cwd = process.cwd();
    const pwd = process.env.PWD;
    if (process.platform === "win32" || !pwd?.startsWith("/") || pwd === cwd) return cwd;
    try {
        const dot = statSync(".");
        const dir = statSync(pwd);
        return dot.dev === dir.dev && dot.ino === dir.ino ? pwd : cwd;
    } catch {
        return cwd;
    }
}

/** `C:\a\b` to `C:/a/b`: tsc paths use `/`. */
function toPosix(path) {
    return path.replaceAll("\\", "/");
}
