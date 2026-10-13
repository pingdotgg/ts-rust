"use strict";

// APISample_parseConfig only exports createProgram: call it on the fixture and print the program.
const path = require("node:path");
const ts = require("typescript");

const sample = require(path.resolve(process.argv[2]));
const compilerOptions = { strict: true, target: "es2020", module: "commonjs", noEmit: true };
const program = sample.createProgram(process.argv.slice(3), JSON.stringify({ compilerOptions }));
console.log("roots", program.getRootFileNames().map(file => path.basename(file)).join(","));
for (const diagnostic of ts.getPreEmitDiagnostics(program)) {
    const file = diagnostic.file && path.basename(diagnostic.file.fileName);
    console.log(diagnostic.code, file, ts.flattenDiagnosticMessageText(diagnostic.messageText, "\n"));
}
