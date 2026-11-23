// Running the project's own tools: a child process in its own process group,
// stopped with everything it started when its time is up, its output kept
// (the end of it, past a limit).

import { spawn } from "node:child_process";
import { accessSync, constants, existsSync, readFileSync } from "node:fs";
import { delimiter, dirname, join } from "node:path";

/** Keeps the last `max` characters of what is appended. */
export class Tail {
  constructor(max) {
    this.max = max;
    this.text = "";
  }

  push(chunk) {
    this.text += chunk;
    if (this.text.length > 2 * this.max) this.text = this.text.slice(-this.max);
  }

  toString() {
    return this.text.length > this.max ? this.text.slice(-this.max) : this.text;
  }
}

/** At most `max` bytes of `text`, its end. */
export function keepEnd(text, max) {
  if (Buffer.byteLength(text) <= max) return text;
  let out = text.slice(-max);
  while (Buffer.byteLength(out) > max) out = out.slice(1);
  return out;
}

/**
 * Runs `command` with `args` in `cwd` and resolves to { code, signal, stdout, stderr, timedOut, error }. Stdout
 * is kept whole up to `maxStdout` characters (then its end), stderr's end up to `maxStderr`. At `deadline` (a
 * Date.now() value) the process group is sent SIGTERM, then SIGKILL two seconds later. `onStdout` receives
 * stdout as it comes.
 */
export function run(command, args, { cwd, env = process.env, deadline = Infinity, maxStdout = 64 << 20, maxStderr = 256 << 10, onStdout } = {}) {
  return new Promise((resolvePromise) => {
    let child;
    try {
      child = spawn(command, args, { cwd, env, stdio: ["ignore", "pipe", "pipe"], detached: true });
    } catch (error) {
      resolvePromise({ code: null, signal: null, stdout: "", stderr: "", timedOut: false, error });
      return;
    }
    const stdout = new Tail(maxStdout);
    const stderr = new Tail(maxStderr);
    let timedOut = false;
    let killTimer = null;
    const kill = (signal) => {
      try {
        process.kill(-child.pid, signal);
      } catch {
        try {
          child.kill(signal);
        } catch {
          // Already gone.
        }
      }
    };
    const wait = deadline === Infinity ? null : Math.max(0, deadline - Date.now());
    const timer =
      wait === null
        ? null
        : setTimeout(() => {
            timedOut = true;
            kill("SIGTERM");
            killTimer = setTimeout(() => kill("SIGKILL"), 2000);
          }, wait);
    child.stdout.setEncoding("utf8");
    child.stderr.setEncoding("utf8");
    child.stdout.on("data", (chunk) => {
      stdout.push(chunk);
      onStdout?.(chunk);
    });
    child.stderr.on("data", (chunk) => stderr.push(chunk));
    let settled = false;
    const finish = (result) => {
      if (settled) return;
      settled = true;
      if (timer) clearTimeout(timer);
      if (killTimer) clearTimeout(killTimer);
      // Whatever the tool left running (watchers, workers) goes with it.
      kill("SIGKILL");
      resolvePromise({ stdout: stdout.toString(), stderr: stderr.toString(), timedOut, ...result });
    };
    child.on("error", (error) => finish({ code: null, signal: null, error }));
    child.on("close", (code, signal) => finish({ code, signal, error: null }));
  });
}

/** The executable `name` on the PATH, or null. */
export function which(name, path = process.env.PATH ?? "") {
  for (const dir of path.split(delimiter)) {
    if (dir === "") continue;
    const candidate = join(dir, name);
    try {
      accessSync(candidate, constants.X_OK);
      return candidate;
    } catch {
      // Not here.
    }
  }
  return null;
}

/**
 * The folder of package `name` installed for `from`: node_modules/<name> in `from` or a folder above it, up to
 * and including `stop`. Null when it is not installed.
 */
export function packageDir(name, from, stop) {
  let dir = from;
  for (;;) {
    const candidate = join(dir, "node_modules", name);
    if (existsSync(join(candidate, "package.json"))) return candidate;
    if (dir === stop || dirname(dir) === dir) return null;
    dir = dirname(dir);
  }
}

/** The JavaScript file of a package's command `bin` (its package.json "bin"), or null. */
export function packageBin(dir, bin) {
  let pkg;
  try {
    pkg = JSON.parse(readFileSync(join(dir, "package.json"), "utf8"));
  } catch {
    return null;
  }
  const field = pkg.bin;
  const rel = typeof field === "string" ? field : field && typeof field === "object" ? (field[bin] ?? Object.values(field)[0]) : null;
  if (typeof rel !== "string") return null;
  const file = join(dir, rel);
  return existsSync(file) ? file : null;
}

/** The version of an installed package, or null. */
export function packageVersion(dir) {
  try {
    return JSON.parse(readFileSync(join(dir, "package.json"), "utf8")).version ?? null;
  } catch {
    return null;
  }
}

/** The major version of a version string ("9.3.0" -> 9), or 0. */
export function major(version) {
  const m = /^(\d+)/.exec(version ?? "");
  return m ? Number(m[1]) : 0;
}
