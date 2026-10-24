"use strict";

// Loads the tsc-rs Node-API addon (crates/ts_napi). scripts/build-native.mjs builds it into
// native/ts_napi.<platform>-<arch>.node; TSRS_NAPI_PATH points at another build.

const fs = require("node:fs");
const path = require("node:path");

const nativeDir = path.join(__dirname, "..", "..", "native");

let addon;

function addonPath() {
    return process.env.TSRS_NAPI_PATH || path.join(nativeDir, `ts_napi.${process.platform}-${process.arch}.node`);
}

function loadAddon() {
    if (addon !== undefined) {
        return addon;
    }
    const file = addonPath();
    if (!fs.existsSync(file)) {
        throw new Error(
            `The tsc-rs native addon is missing: ${file}. Build it with \`node npm/typescript-api/scripts/build-native.mjs\`, or set TSRS_NAPI_PATH to a built ts_napi library.`,
        );
    }
    // process.dlopen, not require: TSRS_NAPI_PATH may name a .so, .dylib or .dll.
    const module = { exports: {} };
    process.dlopen(module, file);
    addon = module.exports;
    return addon;
}

module.exports = { addonPath, loadAddon };
