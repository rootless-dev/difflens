// Loaded with `--import` into every process of a Stryker run (through
// NODE_OPTIONS). It does two things.
//
// Resolution. StrykerJS lives in the tools folder (`prepare --tools`), and its
// runner plugins import the project's test runner by name (`vitest/node`,
// `mocha`, `jest-environment-node`), which Node looks for in node_modules
// folders above the plugin, in the tools folder, where it is not. When a bare
// name imported or required from inside the tools folder cannot be found
// there, this hook resolves it again from the project's root, so the plugins
// get the project's own runner, and then from this analyzer's folder (its
// typescript, which Stryker loads to read a tsconfig.json). Nothing else is
// touched: a module outside the tools folder, and anything the tools folder
// has, resolve as usual.
//
// Watchdog. Stryker runs in a process group of its own (process.js), which
// `mutate` stops at its deadline; but when `mutate` itself is stopped (the
// core kills its process group, which Stryker's is not), Stryker would go on.
// So Stryker's own process (the child of `mutate`) checks every second that
// `mutate` is still its parent, and when it is not, kills its process group:
// itself, its workers and the test runners they started.
//
//   DIFFLENS_STRYKER_TOOLS    the folder of the Stryker install (its real path)
//   DIFFLENS_STRYKER_PROJECT  the project's root (the unit's folder mutate runs Stryker in)
//   DIFFLENS_MUTATE_PID       the process id of `mutate`

import module from "node:module";
import { join } from "node:path";
import { pathToFileURL } from "node:url";

const tools = process.env.DIFFLENS_STRYKER_TOOLS;
const project = process.env.DIFFLENS_STRYKER_PROJECT;
const parent = Number(process.env.DIFFLENS_MUTATE_PID);

if (Number.isInteger(parent) && parent > 0 && process.ppid === parent) {
  const watch = setInterval(() => {
    if (process.ppid === parent) return;
    try {
      // 0: this process's group, which Stryker leads.
      process.kill(0, "SIGKILL");
    } catch {
      process.exit(1);
    }
  }, 1000);
  watch.unref();
}

if (tools && project) {
  const toolsUrl = pathToFileURL(tools.endsWith("/") ? tools : `${tools}/`).href;
  const fallbacks = [pathToFileURL(join(project, "package.json")).href, new URL("../../package.json", import.meta.url).href];

  if (typeof module.registerHooks === "function") {
    // Node 22.15 and later: synchronous hooks, for import and require alike.
    const bare = (specifier) => !/^(\.|\/|[a-z][a-z0-9+.-]*:)/i.test(specifier);
    module.registerHooks({
      resolve(specifier, context, nextResolve) {
        try {
          return nextResolve(specifier, context);
        } catch (error) {
          if (!bare(specifier) || typeof context.parentURL !== "string" || !context.parentURL.startsWith(toolsUrl)) throw error;
          for (const parentURL of fallbacks) {
            try {
              return nextResolve(specifier, { ...context, parentURL });
            } catch {
              // Not there either.
            }
          }
          throw error;
        }
      },
    });
  } else if (typeof module.register === "function") {
    // Older Node: the asynchronous hooks, which see ESM imports (the vitest and mocha plugins' case).
    const source = `
      const toolsUrl = ${JSON.stringify(toolsUrl)};
      const fallbacks = ${JSON.stringify(fallbacks)};
      const bare = (s) => !/^(\\.|\\/|[a-z][a-z0-9+.-]*:)/i.test(s);
      export async function resolve(specifier, context, nextResolve) {
        try {
          return await nextResolve(specifier, context);
        } catch (error) {
          if (!bare(specifier) || typeof context.parentURL !== "string" || !context.parentURL.startsWith(toolsUrl)) throw error;
          for (const parentURL of fallbacks) {
            try {
              return await nextResolve(specifier, { ...context, parentURL });
            } catch {}
          }
          throw error;
        }
      }`;
    module.register(`data:text/javascript,${encodeURIComponent(source)}`);
  }
}
