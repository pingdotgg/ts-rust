// The runtime-independent part of the package: a small WASI preview1 shim,
// the host file system bridge (crates/ts_wasm/src/host.rs) and one tsc run.
// It needs only WebAssembly, TextEncoder, TextDecoder, crypto and
// performance, so it runs in Node, Deno, Bun and browsers.

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

/**
 * The state and imports of one run (`runTsc`, `runTscAsync`). Exported for
 * a held instance: make the instance yourself (`new WebAssembly.Instance(
 * module, run.imports)`), call the `ts_run` that `run.start(instance)`
 * returns once per request, and read each reply with `run.result`. One
 * instance serves many requests (the multi-run flow), so a probe that
 * holds it sees the warm runs.
 */
export function prepareRun(module, options) {
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
    // The request and the environment are NUL-separated, so a NUL in one
    // value would turn it into more values (`["a.ts\0--outDir", "/x"]`).
    // child_process refuses such values too.
    for (const value of [cwd, ...args, ...Object.entries(env).flat()]) {
        if (typeof value === "string" && value.includes("\0")) {
            throw new TypeError(`ts-rust: cwd, args and env must not contain NUL bytes: ${JSON.stringify(value)}`);
        }
    }
    let memory;
    let staged;
    let stderrText = "";
    const stderrDecoder = new TextDecoder();
    const u8 = () => new Uint8Array(memory.buffer);
    const dv = () => new DataView(memory.buffer);

    // An undefined value unsets the variable, as in child_process.
    const envBytes = Object.entries(env)
        .filter(([, v]) => v !== undefined)
        .map(([k, v]) => encoder.encode(`${k}=${v}\0`));
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
                    stderrText += stderrDecoder.decode(chunk, { stream: true });
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
        /** Writes the request into `instance` and returns its `ts_run`. */
        start(instance) {
            exports = instance.exports;
            memory = exports.memory;
            let flags = 0;
            if (diagnosticsJson) flags |= FLAG_DIAGNOSTICS_JSON;
            if (caseInsensitive) flags |= FLAG_CASE_INSENSITIVE;
            const request = encoder.encode([cwd, String(flags), ...args].join("\0"));
            const input = exports.ts_input(request.length);
            u8().set(request, input);
            return exports.ts_run;
        },
        /** The result of a `ts_run` call that returned `exitCode` or threw `error`. */
        result(exitCode, error) {
            if (error !== undefined) {
                if (!(error instanceof WasiExit)) {
                    error.stderr = stderrText + stderrDecoder.decode();
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
    const jspi = typeof WebAssembly.promising === "function" && typeof document === "undefined";
    const call = jspi ? WebAssembly.promising(tsRun) : tsRun;
    let exitCode;
    let error;
    try {
        exitCode = await call();
    } catch (thrown) {
        error = thrown;
    }
    return run.result(exitCode, error);
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
        if (path === "/") return true;
        const prefix = dirPrefix(path);
        for (const key of map.keys()) if (key.startsWith(prefix)) return true;
        return false;
    };
    return {
        files: map,
        readFile: path => map.get(path),
        stat(path) {
            const text = map.get(path);
            if (text !== undefined) return { isDirectory: false, size: encoder.encode(text).length, mtimeMs: 0 };
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
            if (isDir(path)) return `open ${path}: is a directory`;
            for (let slash = path.indexOf("/"); slash >= 0; slash = path.indexOf("/", slash + 1)) {
                const parent = path.slice(0, slash);
                if (map.has(parent)) return `mkdir ${parent}: not a directory`;
            }
            const text = fileDecoder.decode(data);
            map.set(path, append ? (map.get(path) ?? "") + text : text);
        },
        remove(path) {
            const prefix = dirPrefix(path);
            for (const key of [...map.keys()]) if (key === path || key.startsWith(prefix)) map.delete(key);
        },
    };
}
