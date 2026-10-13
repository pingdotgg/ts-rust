"use strict";

// The samples `declare var` these names; in the emitted JS they are free variables.
globalThis.fs = require("node:fs");
globalThis.path = require("node:path");
globalThis.os = require("node:os");
globalThis.readFileSync = require("node:fs").readFileSync;
