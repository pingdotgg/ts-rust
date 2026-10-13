"use strict";

// Prints which typescript the workspace loads, run from a package directory of the clone.
// usage: node probe.cjs <expected-package-dir|stock>
const fs = require("node:fs");
const { createRequire } = require("node:module");
const path = require("node:path");

const expected = process.argv[2];
const requireHere = createRequire(path.join(process.cwd(), "probe.cjs"));
const ts = requireHere("typescript");
const tsserver = requireHere("typescript/lib/tsserverlibrary");
const resolvedPackageJson = requireHere("typescript/package.json");

const shim = fs.realpathSync(requireHere.resolve("typescript"));
const loaded = Object.keys(require.cache)
    .filter((file) => /[\\/]lib[\\/]typescript\.js$/.test(file))
    .map((file) => fs.realpathSync(file));
const real = loaded.find((file) => file !== shim) ?? shim;
const loadedFrom = path.dirname(path.dirname(real));

const result = {
    version: ts.version,
    versionMajorMinor: ts.versionMajorMinor,
    exports: Object.keys(ts).length,
    tsserverlibraryIsTypescript: tsserver === ts,
    typescriptPackageJsonVersion: resolvedPackageJson.version,
    shim,
    loadedFrom,
};
process.stdout.write(`${JSON.stringify(result, null, 4)}\n`);

if (expected === "stock") {
    if (!/[\\/]typescript@6\.0\.3[\\/]node_modules[\\/]typescript$/.test(loadedFrom)) {
        console.error(`stock run loads typescript from ${loadedFrom}, not the store's typescript@6.0.3`);
        process.exit(1);
    }
} else if (fs.realpathSync(expected) !== loadedFrom) {
    console.error(`typescript loads from ${loadedFrom}, not ${fs.realpathSync(expected)}`);
    process.exit(1);
}
