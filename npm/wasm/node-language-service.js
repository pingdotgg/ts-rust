import { Worker } from "node:worker_threads";
import { fileEntries } from "./file-map.js";

/** Each persistent Node service owns a worker and its large native stack. */
export async function createWorkerLanguageService(module, options) {
    const worker = new Worker(new URL("./language-service-worker.js", import.meta.url), {
        workerData: {
            module,
            options: {
                files: new Map(fileEntries(options.files)),
                cwd: options.cwd,
                args: options.args,
                capabilities: options.capabilities,
                caseInsensitive: options.caseInsensitive,
            },
        },
        resourceLimits: { stackSizeMb: options.stackSizeMb ?? 256 },
    });
    const pending = new Map();
    let nextId = 1;
    let failure;
    let disposal;
    let disposed = false;
    const ready = new Promise((resolve, reject) => pending.set(0, { resolve, reject }));
    const fail = error => {
        failure ??= error;
        for (const request of pending.values()) request.reject(failure);
        pending.clear();
    };
    worker.on("message", message => {
        const request = pending.get(message.id);
        if (!request) return;
        pending.delete(message.id);
        if (message.error) { request.reject(deserializeError(message.error)); return; }
        request.resolve(message.result);
    });
    worker.on("error", fail);
    worker.on("exit", code => {
        if (!disposed) fail(new Error(`language-service worker exited with code ${code}`));
    });

    function invoke(method, args) {
        if (failure) return Promise.reject(failure);
        const id = nextId++;
        return postRequest(worker, pending, { id, method, args });
    }
    function request(method, args) {
        if (disposal) return Promise.reject(new Error("language service is disposed"));
        return invoke(method, args);
    }
    try { await ready; }
    catch (error) { disposed = true; await worker.terminate(); throw error; }
    return {
        request: (method, params = {}) => request("request", [method, params]),
        updateFiles(files) {
            try { return request("updateFiles", [new Map(fileEntries(files))]); }
            catch (error) { return Promise.reject(error); }
        },
        deleteFiles: paths => request("deleteFiles", [Array.from(paths)]),
        dispose() {
            disposal ??= (failure ? Promise.resolve() : invoke("dispose", [])).finally(async () => {
                disposed = true;
                await worker.terminate();
                fail(new Error("language service is disposed"));
            });
            return disposal;
        },
    };
}

function postRequest(worker, pending, message) {
    return new Promise((resolve, reject) => {
        pending.set(message.id, { resolve, reject });
        try { worker.postMessage(message); }
        catch (error) { pending.delete(message.id); reject(error); }
    });
}

function deserializeError(error) {
    return Object.assign(new Error(error.message), { code: error.code, data: error.data, stderr: error.stderr });
}
