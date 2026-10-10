import { prepareRun } from "./runtime.js";

const encoder = new TextEncoder();
const decoder = new TextDecoder();

/** Internal synchronous transport for the classic TypeScript facade. */
export function createServiceChannel(module, options) {
    const run = prepareRun(module, options);
    let exports = run.attach(new WebAssembly.Instance(module, run.imports));
    let busy = false;
    let failure;

    function call(request) {
        if (!exports) throw new Error("language service is disposed");
        if (failure) throw failure;
        if (busy) throw new Error("language service cannot reenter a host callback");
        const input = encoder.encode(JSON.stringify(request));
        let response;
        busy = true;
        try {
            const ptr = exports.ts_input(input.length);
            new Uint8Array(exports.memory.buffer).set(input, ptr);
            exports.ts_service();
            response = JSON.parse(decoder.decode(new Uint8Array(
                exports.memory.buffer, exports.ts_output(), exports.ts_output_len(),
            )));
        } catch (error) {
            failure = Object.assign(error, { stderr: run.stderrText() });
            throw failure;
        } finally { busy = false; }
        if (response.error) throw Object.assign(new Error(response.error.message), {
            code: response.error.code, data: response.error.data,
        });
        return response.result;
    }

    function dispose() {
        if (!exports) return;
        if (busy) throw new Error("language service cannot dispose during a host callback");
        try {
            if (!failure) call({ action: "dispose" });
        } finally {
            exports = undefined;
            failure = undefined;
            run.detach();
        }
    }

    try {
        call({ action: "initialize", cwd: options.cwd ?? "/", args: options.args ?? [],
            caseInsensitive: options.caseInsensitive ?? false, capabilities: options.capabilities ?? {} });
    } catch (error) {
        exports = undefined;
        run.detach();
        throw error;
    }
    return { call, dispose };
}
