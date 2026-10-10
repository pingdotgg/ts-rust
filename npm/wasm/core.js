// The runtime-independent part of the package: a small WASI preview1 shim,
// the host file system bridge (crates/ts_wasm/src/host.rs) and one tsc run.
// It needs only WebAssembly, TextEncoder, TextDecoder, crypto and
// performance, so it runs in Node, Deno, Bun and browsers.

import { fileEntries } from "./file-map.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder();
// For file text: keeps a leading BOM (--emitBOM), which `decoder` drops.
const fileDecoder = new TextDecoder("utf-8", { ignoreBOM: true });

// crates/ts_wasm FLAG_* bits.
const FLAG_DIAGNOSTICS_JSON = 1;
const FLAG_CASE_INSENSITIVE = 2;

// WASI errno values.
const SUCCESS = 0;
const EBADF = 8;
const ENOSYS = 52;

/** Thrown by the WASI `proc_exit` import to end a run. */
export class WasiExit extends Error {
    constructor(code) {
        super(`exit ${code}`);
        this.code = code;
    }
}

// crates/ts_wasm/src/host.rs `Op`.
const OP_READ = 0;
const OP_STAT = 1;
const OP_READ_DIR = 2;
const OP_REALPATH = 3;
const OP_WRITE = 4;
const OP_APPEND = 5;
const OP_REMOVE = 6;
const OP_CHTIMES = 7;

const KIND_CHAR = { file: "f", directory: "d", symlink: "l", other: "o" };

/**
 * The result of a host write method: undefined or true when it worked,
 * false when it failed, or a string with Go's text of its error (for
 * example `open /a.js: permission denied`), which tsc then reports.
 */
function writeResult(result) {
    if (result === false) return undefined;
    if (typeof result === "string") return { error: result };
    return new Uint8Array();
}

/**
 * Runs one host file operation of crates/ts_wasm/src/host.rs on `fs`. It
 * returns the result bytes, undefined when the operation fails, or
 * `{ error }` when it fails with an error text.
 */
function fsCall(fs, op, request) {
    const text = decoder.decode(request);
    switch (op) {
        case OP_READ: {
            const data = fs.readFile(text);
            if (data === undefined) return undefined;
            return typeof data === "string" ? encoder.encode(data) : data;
        }
        case OP_STAT: {
            const stat = fs.stat(text);
            if (!stat) return undefined;
            const kind = stat.isDirectory ? "d" : stat.isFile === false ? "o" : "f";
            const ns = stat.mtimeNs ?? BigInt(Math.round((stat.mtimeMs ?? 0) * 1e6));
            return encoder.encode(`${kind} ${stat.size ?? 0} ${ns}`);
        }
        case OP_READ_DIR: {
            const entries = fs.readDirectory(text);
            if (!entries) return undefined;
            return encoder.encode(entries.map(e => `${KIND_CHAR[e.kind] ?? "o"}${e.name}\0`).join(""));
        }
        case OP_REALPATH: {
            const real = fs.realpath ? fs.realpath(text) : undefined;
            return real === undefined ? undefined : encoder.encode(real);
        }
        case OP_WRITE:
        case OP_APPEND: {
            const nul = request.indexOf(0);
            const path = decoder.decode(request.subarray(0, nul));
            const data = request.subarray(nul + 1);
            if (!fs.writeFile) return undefined;
            return writeResult(fs.writeFile(path, data, op === OP_APPEND));
        }
        case OP_REMOVE:
            if (!fs.remove) return undefined;
            return writeResult(fs.remove(text));
        case OP_CHTIMES: {
            const [path, atime, mtime] = text.split("\0");
            if (!fs.chtimes) return new Uint8Array();
            return writeResult(fs.chtimes(path, atime ? BigInt(atime) : undefined, mtime ? BigInt(mtime) : undefined));
        }
    }
    return undefined;
}

/** The state and imports of one run (`runTsc`, `runTscAsync`). */
function prepareRun(module, options) {
    const {
        args = [],
        cwd = "/",
        fs,
        env = {},
        stdout = () => {},
        stderr = () => {},
        diagnosticsJson = false,
        caseInsensitive = false,
        tty = false,
    } = options;
    let memory;
    let staged;
    let stderrText = "";
    const u8 = () => new Uint8Array(memory.buffer);
    const dv = () => new DataView(memory.buffer);

    const envBytes = Object.entries(env).map(([k, v]) => encoder.encode(`${k}=${v}\0`));
    const writeStrings = (list, ptrs, buf) => {
        const view = dv();
        let at = buf;
        list.forEach((bytes, i) => {
            view.setUint32(ptrs + i * 4, at, true);
            u8().set(bytes, at);
            at += bytes.length;
        });
        return SUCCESS;
    };
    const sizes = (list, countPtr, sizePtr) => {
        dv().setUint32(countPtr, list.length, true);
        dv().setUint32(sizePtr, list.reduce((n, b) => n + b.length, 0), true);
        return SUCCESS;
    };

    const wasi = {
        args_sizes_get: (countPtr, sizePtr) => sizes([], countPtr, sizePtr),
        args_get: () => SUCCESS,
        environ_sizes_get: (countPtr, sizePtr) => sizes(envBytes, countPtr, sizePtr),
        environ_get: (ptrs, buf) => writeStrings(envBytes, ptrs, buf),
        clock_time_get: (id, _precision, out) => {
            // Wall time with sub-millisecond steps (not Date.now): tsc leaves
            // out a statistics row whose time is zero.
            const ms = id === 0 ? performance.timeOrigin + performance.now() : performance.now();
            const ns = BigInt(Math.round(ms * 1e6));
            dv().setBigUint64(out, ns, true);
            return SUCCESS;
        },
        clock_res_get: (_id, out) => {
            dv().setBigUint64(out, 1000n, true);
            return SUCCESS;
        },
        random_get: (buf, len) => {
            for (let at = 0; at < len; at += 65536) {
                crypto.getRandomValues(u8().subarray(buf + at, buf + Math.min(len, at + 65536)));
            }
            return SUCCESS;
        },
        fd_write: (fd, iovs, iovsLen, nwritten) => {
            if (fd !== 1 && fd !== 2) return EBADF;
            const view = dv();
            let total = 0;
            for (let i = 0; i < iovsLen; i++) {
                const ptr = view.getUint32(iovs + i * 8, true);
                const len = view.getUint32(iovs + i * 8 + 4, true);
                const chunk = u8().slice(ptr, ptr + len);
                if (fd === 1) stdout(chunk);
                else {
                    stderrText += decoder.decode(chunk);
                    stderr(chunk);
                }
                total += len;
            }
            view.setUint32(nwritten, total, true);
            return SUCCESS;
        },
        fd_fdstat_get: (fd, out) => {
            if (fd > 2) return EBADF;
            // A character device without seek rights is a terminal for Rust's
            // `IsTerminal`.
            u8().fill(0, out, out + 24);
            dv().setUint8(out, fd === 1 && tty ? 2 : 0);
            return SUCCESS;
        },
        fd_prestat_get: () => EBADF,
        proc_exit: code => {
            throw new WasiExit(code);
        },
        sched_yield: () => SUCCESS,
        poll_oneoff: (_in, _out, _n, nevents) => {
            dv().setUint32(nevents, 0, true);
            return SUCCESS;
        },
    };
    const imports = { wasi_snapshot_preview1: {}, ts_host: {} };
    for (const { module: name, name: field } of WebAssembly.Module.imports(module)) {
        if (name === "wasi_snapshot_preview1") imports[name][field] = wasi[field] ?? (() => ENOSYS);
    }
    imports.ts_host.fs = (op, ptr, len) => {
        const result = fsCall(fs, op, u8().slice(ptr, ptr + len));
        if (result === undefined) return -1;
        if (result.error !== undefined) {
            staged = encoder.encode(result.error);
            return -2 - staged.length;
        }
        staged = result;
        return staged.length;
    };
    imports.ts_host.fs_take = ptr => {
        u8().set(staged, ptr);
        staged = undefined;
    };

    let exports;
    return {
        imports,
        attach(instance) {
            exports = instance.exports;
            memory = exports.memory;
            return exports;
        },
        detach() {
            exports = undefined;
            memory = undefined;
        },
        stderrText: () => stderrText,
        /** Writes the request into `instance` and returns its `ts_run`. */
        start(instance) {
            exports = instance.exports;
            memory = exports.memory;
            let flags = 0;
            if (diagnosticsJson) flags |= FLAG_DIAGNOSTICS_JSON;
            if (caseInsensitive) flags |= FLAG_CASE_INSENSITIVE;
            const request = encoder.encode([cwd, String(flags), ...args].join("\0"));
            u8().set(request, exports.ts_input(request.length));
            return exports.ts_run;
        },
        /** The result of a `ts_run` call that returned `exitCode` or threw `error`. */
        result(exitCode, error) {
            if (error !== undefined) {
                if (!(error instanceof WasiExit)) {
                    error.stderr = stderrText;
                    throw error;
                }
                exitCode = error.code;
            }
            let diagnostics;
            if (diagnosticsJson) {
                const ptr = exports.ts_output();
                const len = exports.ts_output_len();
                diagnostics = len ? JSON.parse(decoder.decode(u8().subarray(ptr, ptr + len))) : [];
            }
            return { exitCode, diagnostics };
        },
    };
}

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
