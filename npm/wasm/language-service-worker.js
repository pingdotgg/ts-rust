import { parentPort, workerData } from "node:worker_threads";
import { createLanguageService } from "./core.js";

function sendError(id, error) {
    parentPort.postMessage({ id, error: { message: String(error.message ?? error), code: error.code, data: error.data, stderr: error.stderr } });
}

try {
    const service = await createLanguageService(workerData.module, workerData.options);
    parentPort.on("message", async ({ id, method, args }) => {
        try {
            const result = await service[method](...args);
            parentPort.postMessage({ id, result });
        } catch (error) { sendError(id, error); }
    });
    parentPort.postMessage({ id: 0 });
} catch (error) { sendError(0, error); }
