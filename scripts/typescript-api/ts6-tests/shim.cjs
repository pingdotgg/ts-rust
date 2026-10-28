"use strict";

// Every product `ts` barrel of the swapped test bundle is this module, and every `ts.server` barrel is
// its `server` member. The library under test is a full TS 6.0.3 bundle (internals included), so
// there is no second TypeScript copy behind it.
const path = require("node:path");

const libraryDir = process.env.TS6_TESTS_LIBRARY;
if (!libraryDir) {
    throw new Error("TS6_TESTS_LIBRARY is not set: run the tests through scripts/typescript-api/ts6-tests/run.sh");
}

module.exports = require(path.join(libraryDir, "lib", "typescript.js"));
