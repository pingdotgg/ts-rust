// lib/getExePath.js of the tsc-rs package (npm/pack.mjs --name tsc-rs). It replaces Go's
// getExePath.js, which finds `@typescript/<name>-<platform>-<arch>` from the package.json name. The
// bin launcher (lib/tsc.js), the JS API and the postinstall (lib/install.js) import it as
// `#getExePath`.
//
// The native tsc is lib/tsc (lib/tsc.exe on Windows) in the platform package
// `@tsc-rs/<platform>-<arch>`. That name is fixed, not built from package.json `name`, so a copy of
// the package under another name still finds it. (An alias install, `"typescript":
// "npm:tsc-rs@<v>"`, keeps package.json `name`.) The drop-in @tsc-rs/typescript keeps Go's
// getExePath.js: its optionalDependencies install the same packages under Go's names.
import fs from "node:fs";
import module from "node:module";
import path from "node:path";
import { fileURLToPath } from "node:url";

export default function getExePath() {
    const platformPackageName = `@tsc-rs/${process.platform}-${process.arch}`;
    let exeDir;
    try {
        const packageJson = typeof import.meta.resolve === "function"
            ? fileURLToPath(import.meta.resolve(`${platformPackageName}/package.json`))
            : module.createRequire(import.meta.url).resolve(`${platformPackageName}/package.json`);
        exeDir = path.join(path.dirname(packageJson), "lib");
    }
    catch {
        throw new Error(`Unable to resolve ${platformPackageName}. Either your platform is unsupported, or you are missing the package on disk.`);
    }
    const exe = path.join(exeDir, process.platform === "win32" ? "tsc.exe" : "tsc");
    if (!fs.existsSync(exe)) {
        throw new Error(`Executable not found: ${exe}`);
    }
    return exe;
}
