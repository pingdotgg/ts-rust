// Child processes: one-shot steps with a log file and a timeout, long-running services
// (apps, watch mode) whose output is matched while they run, free ports and HTTP requests.

import { spawn } from "node:child_process";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";

const timeoutScale = Number(process.env.NEST_HARNESS_TIMEOUT_SCALE ?? "1") || 1;

export function scaledTimeout(ms) {
    return Math.round(ms * timeoutScale);
}

function openLog(logFile, header) {
    fs.mkdirSync(path.dirname(logFile), { recursive: true });
    const fd = fs.openSync(logFile, "w");
    fs.writeSync(fd, header);
    return fd;
}

function commandLine(cmd, args) {
    return [cmd, ...args].map((part) => (/^[\w./:=@,+-]+$/.test(part) ? part : JSON.stringify(part))).join(" ");
}

// Each child leads its own process group, so a timeout or a stop also ends its children
// (nest start spawns node, npm spawns sh). The group can outlive its leader.
function killGroup(child, signal) {
    if (child.pid === undefined) {
        return;
    }
    try {
        process.kill(-child.pid, signal);
    } catch {
        // The group already exited.
    }
}

function groupAlive(child) {
    try {
        process.kill(-child.pid, 0);
        return true;
    } catch (error) {
        return error.code === "EPERM";
    }
}

// Waits until no process of the child's group is left (SIGKILL takes a moment to land).
async function waitGroupGone(child, timeoutMs = 10_000) {
    const deadline = Date.now() + timeoutMs;
    while (child.pid !== undefined && groupAlive(child) && Date.now() < deadline) {
        await new Promise((resolve) => setTimeout(resolve, 100));
    }
}

export function runCommand({ cmd, args, cwd, env, logFile, timeoutMs }) {
    const started = Date.now();
    const header = `$ cd ${cwd}\n$ ${commandLine(cmd, args)}\n\n`;
    const fd = openLog(logFile, header);
    return new Promise((resolve) => {
        const child = spawn(cmd, args, { cwd, env, detached: true, stdio: ["ignore", fd, fd] });
        let timedOut = false;
        const timer = setTimeout(() => {
            timedOut = true;
            killGroup(child, "SIGTERM");
            setTimeout(() => killGroup(child, "SIGKILL"), 5000).unref();
        }, scaledTimeout(timeoutMs));
        child.on("error", (error) => {
            clearTimeout(timer);
            fs.writeSync(fd, `\n[harness] spawn failed: ${error.message}\n`);
            fs.closeSync(fd);
            resolve({ exitCode: null, signal: null, timedOut, ms: Date.now() - started, spawnError: error.message });
        });
        child.on("exit", (exitCode, signal) => {
            clearTimeout(timer);
            // Children that kept the group alive (watchers) must not outlive the step.
            killGroup(child, "SIGKILL");
            const ms = Date.now() - started;
            fs.writeSync(fd, `\n[harness] exit=${exitCode} signal=${signal} timedOut=${timedOut} ms=${ms}\n`);
            fs.closeSync(fd);
            resolve({ exitCode, signal, timedOut, ms });
        });
    });
}

// A process that keeps running (an app, nest start --watch). Output goes to the log file and to
// an in-memory buffer that waitFor() matches against.
export function startService({ cmd, args, cwd, env, logFile }) {
    const started = Date.now();
    const header = `$ cd ${cwd}\n$ ${commandLine(cmd, args)}\n\n`;
    fs.mkdirSync(path.dirname(logFile), { recursive: true });
    const log = fs.createWriteStream(logFile);
    log.write(header);
    const child = spawn(cmd, args, { cwd, env, detached: true, stdio: ["ignore", "pipe", "pipe"] });
    const service = {
        child,
        output: "",
        exit: undefined,
        startedAt: started,
        waitFor,
        count,
        stop,
    };
    const waiters = new Set();
    const onData = (chunk) => {
        const text = chunk.toString();
        service.output += text;
        log.write(text);
        for (const waiter of waiters) {
            waiter.check();
        }
    };
    child.stdout.on("data", onData);
    child.stderr.on("data", onData);
    const exited = new Promise((resolve) => {
        child.on("error", (error) => {
            service.exit = { exitCode: null, signal: null, spawnError: error.message, ms: Date.now() - started };
            resolve(service.exit);
        });
        child.on("exit", (exitCode, signal) => {
            service.exit = { exitCode, signal, ms: Date.now() - started };
            resolve(service.exit);
        });
    });
    exited.then(() => {
        for (const waiter of waiters) {
            waiter.check();
        }
    });
    return service;

    function count(pattern) {
        const global = new RegExp(pattern.source, pattern.flags.includes("g") ? pattern.flags : `${pattern.flags}g`);
        return (service.output.match(global) ?? []).length;
    }

    // Resolves with the elapsed ms once `pattern` has matched `times` times; rejects on timeout or
    // when the process exits first.
    function waitFor(pattern, { times = 1, timeoutMs }) {
        const waitStarted = Date.now();
        return new Promise((resolve, reject) => {
            const waiter = {
                check() {
                    if (count(pattern) >= times) {
                        finish();
                        resolve(Date.now() - waitStarted);
                        return;
                    }
                    if (service.exit !== undefined) {
                        finish();
                        reject(new Error(`process exited (${service.exit.exitCode ?? service.exit.signal}) before ${pattern} x${times}`));
                    }
                },
            };
            const timer = setTimeout(() => {
                finish();
                reject(new Error(`timed out after ${scaledTimeout(timeoutMs)} ms waiting for ${pattern} x${times}`));
            }, scaledTimeout(timeoutMs));
            function finish() {
                clearTimeout(timer);
                waiters.delete(waiter);
            }
            waiters.add(waiter);
            waiter.check();
        });
    }

    async function stop() {
        if (service.exit === undefined) {
            killGroup(child, "SIGTERM");
            const killed = setTimeout(() => killGroup(child, "SIGKILL"), 5000);
            await exited;
            clearTimeout(killed);
        }
        killGroup(child, "SIGKILL");
        await waitGroupGone(child);
        log.write(`\n[harness] stopped exit=${service.exit.exitCode} signal=${service.exit.signal} ms=${service.exit.ms}\n`);
        await new Promise((resolve) => log.end(resolve));
        return service.exit;
    }
}

export function freePort() {
    return new Promise((resolve, reject) => {
        const server = net.createServer();
        server.unref();
        server.on("error", reject);
        server.listen(0, "127.0.0.1", () => {
            const { port } = server.address();
            server.close(() => resolve(port));
        });
    });
}

export function portIsOpen(port) {
    return new Promise((resolve) => {
        const socket = net.connect({ port, host: "127.0.0.1" });
        socket.once("connect", () => {
            socket.destroy();
            resolve(true);
        });
        socket.once("error", () => resolve(false));
    });
}

export async function httpRequest(url, { method = "GET", body, headers = {}, timeoutMs = 15000 } = {}) {
    const started = Date.now();
    try {
        const response = await fetch(url, {
            method,
            body: body === undefined ? undefined : JSON.stringify(body),
            headers: body === undefined ? headers : { "content-type": "application/json", ...headers },
            signal: AbortSignal.timeout(timeoutMs),
        });
        const text = await response.text();
        return { status: response.status, contentType: response.headers.get("content-type"), text, ms: Date.now() - started };
    } catch (error) {
        return { status: null, error: error.message, text: "", ms: Date.now() - started };
    }
}
