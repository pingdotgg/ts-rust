// Bundles the unmodified TS 6.0 test runner (src/testRunner) like `hereby tests` does, with every
// product `ts` / `ts.server` namespace barrel replaced by the shim, so the tests run against the
// library under test instead of TS 6.0's own sources.
// usage: node build-swapped.mjs <ts60 checkout> <outfile> <shim module path>
import path from "node:path";
import { createRequire } from "node:module";

const [root, outfile, shim] = process.argv.slice(2);
if (!root || !outfile || !shim) {
    console.error("usage: node build-swapped.mjs <ts60 checkout> <outfile> <shim module path>");
    process.exit(2);
}

const esbuild = createRequire(path.join(root, "package.json"))("esbuild");

// TS 6.0: Herebyfile.mjs:202 (usePublicAPI) does the same remap for tsserver.
const PRODUCT = /[\\/]src[\\/](compiler|services|jsTyping|server|typingsInstallerCore|deprecatedCompat)[\\/]_namespaces[\\/](ts(?:\.server)?)\.js$/;

const swapBarrels = {
    name: "swap-ts-barrels",
    setup(build) {
        build.onResolve({ filter: /_namespaces[\\/]ts(\.server)?\.js$/ }, args => {
            const abs = path.resolve(args.resolveDir, args.path);
            const match = PRODUCT.exec(abs);
            if (!match) {
                return undefined;
            }
            return { path: abs, namespace: "swapped", pluginData: { barrel: match[2] } };
        });
        build.onLoad({ filter: /.*/, namespace: "swapped" }, args => {
            const request = JSON.stringify(shim);
            const contents = args.pluginData.barrel === "ts"
                ? `module.exports = require(${request});`
                : `module.exports = require(${request}).server;`;
            return { contents, loader: "js", resolveDir: root };
        });
    },
};

// TS 6.0: Herebyfile.mjs:180 createBundler (cjs, node14.17, treeShaking false, packages external).
await esbuild.build({
    entryPoints: [path.join(root, "src/testRunner/_namespaces/Harness.ts")],
    bundle: true,
    outfile,
    platform: "node",
    target: ["es2020", "node14.17"],
    format: "cjs",
    sourcemap: "linked",
    sourcesContent: false,
    treeShaking: false,
    packages: "external",
    logLevel: "warning",
    external: [shim],
    plugins: [swapBarrels],
});
console.log("built", outfile);
