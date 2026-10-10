// Compiler and editor calls share the same WASI and host-file-system bridge.
import { fileEntries } from "./file-map.js";
import { prepareRun } from "./runtime.js";
export { WasiExit } from "./runtime.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
// Preserve a leading BOM in file text, including emitted files.
const fileDecoder = new TextDecoder("utf-8", { ignoreBOM: true });

/**
 * Runs `tsc` once in a new instance of `module` (a compiled ts-rust wasm
 * module).
 *
 * - `args`: the tsc arguments.
 * - `cwd`: the current directory, an absolute path with `/` separators.
 * - `fs`: the host file system (see `memoryFileSystem` and node.js).
 * - `env`: environment variables, for example `{ NO_COLOR: "1" }`.
 * - `stdout`, `stderr`: called with each chunk of output bytes.
 * - `diagnosticsJson`: return the diagnostics as objects, not as text.
 * - `caseInsensitive`: the file system ignores case.
 * - `tty`: stdout is a terminal (tsc then defaults to `--pretty`).
 *
 * Returns `{ exitCode, diagnostics }`. A crash (a trap or an out of memory
 * error) throws, with the stderr text so far in `error.stderr`.
 */
export function runTsc(module, options) {
    const run = prepareRun(module, options);
    const tsRun = run.start(new WebAssembly.Instance(module, run.imports));
    let exitCode;
    let error;
    try {
        exitCode = tsRun();
    } catch (thrown) {
        error = thrown;
    }
    return run.result(exitCode, error);
}

/**
 * `runTsc` as a promise, for browsers. It makes the instance with
 * `WebAssembly.instantiate`, which a page's main thread may also do. Off a
 * page's main thread, where JSPI is available (`WebAssembly.promising`),
 * the run gets its own stack: in a Chrome worker it reaches about 2 times
 * deeper. On a page's main thread JSPI gave less depth (Firefox), so it is
 * not used there.
 */
export async function runTscAsync(module, options) {
    const run = prepareRun(module, options);
    const tsRun = run.start(await WebAssembly.instantiate(module, run.imports));
    const call = asyncWasmCall(tsRun);
    let exitCode;
    let error;
    try {
        exitCode = await call();
    } catch (thrown) {
        error = thrown;
    }
    return run.result(exitCode, error);
}

/** A persistent, in-memory editor service. Run it in a worker in browsers. */
export async function createLanguageService(module, options = {}) {
    const files = new Map(fileEntries(options.files));
    const fs = memoryFileSystem(files);
    const run = prepareRun(module, { ...options, fs });
    let exports = run.attach(await WebAssembly.instantiate(module, run.imports));
    let invoke = asyncWasmCall(exports.ts_service);
    let queue = Promise.resolve();
    let disposal;
    let failure;

    async function call(request) {
        if (failure) throw failure;
        const input = encoder.encode(JSON.stringify(request));
        let result;
        try {
            const ptr = exports.ts_input(input.length);
            new Uint8Array(exports.memory.buffer).set(input, ptr);
            await invoke();
            result = JSON.parse(decoder.decode(new Uint8Array(
                exports.memory.buffer, exports.ts_output(), exports.ts_output_len(),
            )));
        } catch (error) {
            failure = Object.assign(error, { stderr: run.stderrText() });
            throw failure;
        }
        if (result.error) throw Object.assign(new Error(result.error.message), { code: result.error.code, data: result.error.data });
        return result.result;
    }

    function enqueue(operation) {
        if (disposal) return Promise.reject(new Error("language service is disposed"));
        const result = queue.then(operation);
        queue = result.catch(() => {});
        return result;
    }

    await call({ action: "initialize", cwd: options.cwd ?? "/", args: options.args ?? [], caseInsensitive: options.caseInsensitive ?? false, capabilities: options.capabilities ?? {} });
    return {
        request(method, params = {}) {
            return enqueue(() => call({ action: "request", method, params }));
        },
        updateFiles(changes) {
            let entries;
            try { entries = fileEntries(changes); } catch (error) { return Promise.reject(error); }
            return enqueue(async () => {
                await call({ action: "invalidate" });
                for (const [path, text] of entries) files.set(path, text);
            });
        },
        deleteFiles(paths) {
            const owned = Array.from(paths);
            if (owned.some(path => typeof path !== "string" || !path.startsWith("/"))) {
                return Promise.reject(new TypeError("file paths must be absolute"));
            }
            return enqueue(async () => {
                await call({ action: "invalidate" });
                for (const path of owned) files.delete(path);
            });
        },
        dispose() {
            disposal ??= queue.then(async () => {
                try {
                    if (!failure) await call({ action: "dispose" });
                } finally {
                    exports = undefined;
                    invoke = undefined;
                    failure = undefined;
                    run.detach();
                    files.clear();
                }
            });
            return disposal;
        },
    };
}

function asyncWasmCall(fn) {
    const useJspi = typeof WebAssembly.promising === "function" && typeof document === "undefined";
    return useJspi ? WebAssembly.promising(fn) : fn;
}

/**
 * An in-memory file system for `runTsc`. `files` maps absolute paths to
 * their text. Writes (emitted files) go into the same map, so read them
 * from `files` after the run.
 */
export function memoryFileSystem(files = new Map()) {
    const map = files instanceof Map ? files : new Map(Object.entries(files));
    const dirPrefix = path => (path.endsWith("/") ? path : `${path}/`);
    const isDir = path => {
        const prefix = dirPrefix(path);
        for (const key of map.keys()) if (key.startsWith(prefix)) return true;
        return false;
    };
    return {
        files: map,
        readFile: path => map.get(path),
        stat(path) {
            const text = map.get(path);
            if (text !== undefined) return { isDirectory: false, size: text.length, mtimeMs: 0 };
            return isDir(path) ? { isDirectory: true, size: 0, mtimeMs: 0 } : undefined;
        },
        readDirectory(path) {
            const prefix = dirPrefix(path);
            const entries = new Map();
            for (const key of map.keys()) {
                if (!key.startsWith(prefix)) continue;
                const rest = key.slice(prefix.length);
                const slash = rest.indexOf("/");
                const name = slash < 0 ? rest : rest.slice(0, slash);
                if (!entries.has(name)) entries.set(name, slash < 0 ? "file" : "directory");
            }
            return entries.size || isDir(path) ? [...entries].map(([name, kind]) => ({ name, kind })) : undefined;
        },
        realpath: path => (map.has(path) || isDir(path) ? path : undefined),
        writeFile(path, data, append) {
            const text = fileDecoder.decode(data);
            map.set(path, append ? (map.get(path) ?? "") + text : text);
        },
        remove(path) {
            const prefix = dirPrefix(path);
            for (const key of [...map.keys()]) if (key === path || key.startsWith(prefix)) map.delete(key);
        },
    };
}
