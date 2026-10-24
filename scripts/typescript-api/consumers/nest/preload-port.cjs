"use strict";

// Loaded with `node -r` in front of an upstream sample's dist/main.js. The samples listen on the
// fixed port 3000; this moves that listen to NEST_HARNESS_PORT so that runs never collide and the
// sample sources stay unmodified.

const net = require("node:net");

const fixedPort = 3000;
const port = Number(process.env.NEST_HARNESS_PORT);

if (Number.isInteger(port) && port > 0) {
    const listen = net.Server.prototype.listen;
    net.Server.prototype.listen = function listenOnHarnessPort(...args) {
        if (args[0] === fixedPort || args[0] === String(fixedPort)) {
            args[0] = port;
        } else if (args[0] !== null && typeof args[0] === "object" && Number(args[0].port) === fixedPort) {
            args[0] = { ...args[0], port };
        }
        return listen.apply(this, args);
    };
}
