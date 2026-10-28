"use strict";

// lib/typescript.js calls install once, when it loads. The hook points are the functions that
// scripts/build.mjs lists in HOOKS; `ts` is the whole TypeScript 6.0.3 module object.
exports.install = function install(ts, hooks, originals) {
    require("./objects.js").install(ts);
    require("./program.js").install(ts, hooks, originals);
    require("./builder.js").install(ts, hooks, originals);
    require("./transpile.js").install(ts, hooks, originals);
};
