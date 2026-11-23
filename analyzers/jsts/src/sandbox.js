// A read-only sandbox for the checkers. `check` runs the project's own tsc
// and eslint, and eslint loads the project's configuration, which is code;
// on macOS they run under sandbox-exec with the network off and writes
// allowed only in the temporary folder, so a configuration can read the
// tree but not change it or anything else. Elsewhere they run as they are.

import { existsSync, realpathSync } from "node:fs";
import { tmpdir } from "node:os";

const SANDBOX_EXEC = "/usr/bin/sandbox-exec";

function quote(path) {
  return `"${path.replaceAll("\\", "\\\\").replaceAll('"', '\\"')}"`;
}

/** The profile: everything allowed but the network and writes outside `writable` and the null devices. */
export function readOnlyProfile(writable) {
  const paths = [...new Set(writable.flatMap((p) => [p, safeReal(p)]))];
  return [
    "(version 1)",
    "(allow default)",
    "(deny network*)",
    "(deny file-write*)",
    `(allow file-write* ${paths.map((p) => `(subpath ${quote(p)})`).join(" ")})`,
    '(allow file-write-data (literal "/dev/null") (literal "/dev/tty") (literal "/dev/dtracehelper") (regex #"^/dev/fd/"))',
    "(allow file-ioctl)",
  ].join("\n");
}

function safeReal(path) {
  try {
    return realpathSync(path);
  } catch {
    return path;
  }
}

/** `command` and `args` wrapped in the read-only sandbox when this system has one; `scratch` stays writable. */
export function readOnly(command, args, scratch = tmpdir()) {
  if (process.platform !== "darwin" || !existsSync(SANDBOX_EXEC) || process.env.DIFFLENS_JSTS_NO_SANDBOX === "1") {
    return { command, args, sandboxed: false };
  }
  return { command: SANDBOX_EXEC, args: ["-p", readOnlyProfile([scratch]), command, ...args], sandboxed: true };
}
