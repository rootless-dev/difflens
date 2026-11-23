// Finds how the program is reached from outside, and the registrations that
// hand functions of the tree to a mechanism that calls them later.
//
// Recognised, each by its shape in the code (types are not needed, since
// dependencies are usually not installed in the analysed tree):
//
// - package.json: the modules of `bin` (main, labelled with the command), of
//   `main` in a private package (main, labelled with the package), and the
//   exports of `main`, `module` and `exports` of a published package
//   (exported_api, "<package>.<name>"); paths into build output (dist/...)
//   are mapped back to the sources.
// - tests: describe/it/test/suite callbacks at the top of a test module
//   (test, labelled with the title), registered by the module; nested ones
//   and hooks are registrations of their parent.
// - HTTP routes: <router>.get/post/put/patch/delete/head/options/all(path,
//   ...handlers) with a constant path (Express, Koa routers, Hono, Fastify,
//   restify), route(path).get(...) chains, Fastify route({ method, url,
//   handler }) and register(plugin, { prefix }), Hono on(method, path, ...);
//   use(path?, ...) is middleware, and the prefix of use(prefix, router) /
//   route(prefix, app) / register(plugin, { prefix }) is put before the routes
//   of that router. NestJS controllers (@Controller + @Get/@Post/...),
//   message handlers (@MessagePattern, @EventPattern, @SubscribeMessage,
//   @OnEvent), jobs (@Cron, @Interval, @Timeout), gRPC (@GrpcMethod) and
//   GraphQL resolvers (@Query, @Mutation, @Subscription). Next.js route
//   handlers (app/**/route.ts exporting GET, POST...) and API routes
//   (pages/api/**), Remix / React Router route modules (loader, action),
//   when the package depends on the framework.
// - CLI commands: commander and cac `.command("name").action(fn)`, yargs
//   `.command("name", desc, builder, handler)` and `.command({ command,
//   handler })`.
// - Jobs: node-cron `schedule(expr, fn)`, cron `new CronJob(expr, fn)`.
// - Event listeners: on/once/addListener/prependListener/addEventListener
//   ("event <name>"), and Fastify hooks; registrations without an entry point.

import { dirname, join } from "node:path";

import ts from "typescript";

import { readJson } from "../files.js";
import { nameChain, stringValue, unwrap } from "./functions.js";
import { exportTargets, sourceOf } from "./programs.js";
import { safe } from "./resolve.js";

const DYNAMIC = "{dynamic}";
const VERBS = new Set(["get", "post", "put", "patch", "delete", "del", "head", "options", "all", "any"]);
const HTTP_VERBS = ["GET", "POST", "PUT", "PATCH", "DELETE", "HEAD", "OPTIONS"];
const SERVER_NAME = /^(app|application|router|routes?|server|api|fastify|instance|hono|koa|web|http)$|(Router|App|Server|Api|Routes)$/;
const SERVER_MODULES = new Set(["express", "fastify", "koa", "@koa/router", "koa-router", "hono", "restify", "polka", "@hapi/hapi", "h3", "elysia"]);
const CLI_MODULES = new Set(["commander", "cac", "sade", "yargs", "yargs/yargs"]);
const EVENT_METHODS = new Set(["on", "once", "addListener", "prependListener", "prependOnceListener", "addEventListener"]);
const NEST_ROUTES = { Get: "GET", Post: "POST", Put: "PUT", Patch: "PATCH", Delete: "DELETE", Head: "HEAD", Options: "OPTIONS", All: "ANY" };
const NEST_MESSAGES = new Set(["MessagePattern", "EventPattern", "SubscribeMessage", "OnEvent"]);
const NEST_JOBS = new Set(["Cron", "Interval", "Timeout"]);
const NEST_GRAPHQL = new Set(["Query", "Mutation", "Subscription", "ResolveField"]);

/**
 * Runs every detector. `ctx`: { resolver, index (files: Map<fileName, {rel, sf, records, module, testCalls,
 * checker}>), root, packages: Map<name, {dir, pkg}> }. Returns found items: { funcId, entryPoint?,
 * registration?: { caller, line, evidence, value } } where value is the expression denoting the function.
 */
export function detect(ctx) {
  const found = [];
  const add = (item) => found.push(item);
  testEntryPoints(ctx, add);
  packageEntryPoints(ctx, add);
  frameworkFiles(ctx, add);
  const groups = collectGroups(ctx);
  for (const file of ctx.index.files.values()) {
    if (!file.checker) continue;
    callRegistrations(ctx, file, groups, add);
    nestHandlers(ctx, file, add);
  }
  return found;
}

function testEntryPoints(ctx, add) {
  for (const file of ctx.index.files.values()) {
    for (const { call, record, top } of file.testCalls) {
      const caller = ctx.resolver.enclosing(call.node);
      const label = call.title ?? call.api;
      const registration = caller
        ? { caller: caller.id, line: lineOf(call.node), evidence: call.hook ? call.api : `${call.api} ${label}`, value: call.callback }
        : null;
      const entryPoint = top && !call.hook ? { kind: "test", label } : null;
      add({ funcId: record.id, entryPoint, registration });
    }
  }
}

function lineOf(node) {
  return node.getSourceFile().getLineAndCharacterOfPosition(node.getStart()).line + 1;
}

/** The file of the tree a package path names, mapped from build output to sources. */
function packageFile(ctx, dir, path) {
  if (typeof path !== "string") return null;
  const abs = sourceOf(dir, path);
  return abs ? (ctx.index.files.get(abs) ?? null) : null;
}

function packageEntryPoints(ctx, add) {
  for (const { dir, pkg } of ctx.packages.values()) {
    const name = typeof pkg.name === "string" ? pkg.name : dir.split("/").at(-1);
    // bin: the command's module runs.
    const bins = typeof pkg.bin === "string" ? { [name.split("/").at(-1)]: pkg.bin } : pkg.bin && typeof pkg.bin === "object" ? pkg.bin : {};
    for (const [command, path] of Object.entries(bins)) {
      const file = packageFile(ctx, dir, path);
      if (file?.module) add({ funcId: file.module.id, entryPoint: { kind: "main", label: command } });
    }
    const library = pkg.private !== true;
    const mainFile = packageFile(ctx, dir, pkg.main);
    if (!library) {
      if (mainFile?.module) add({ funcId: mainFile.module.id, entryPoint: { kind: "main", label: name } });
      continue;
    }
    const entries = [];
    if (pkg.exports !== undefined) {
      for (const t of exportTargets(pkg.exports)) entries.push({ subpath: t.subpath, path: t.target });
    }
    for (const field of ["main", "module", "source"]) if (typeof pkg[field] === "string") entries.push({ subpath: ".", path: pkg[field] });
    const seen = new Set();
    for (const entry of entries) {
      if (entry.subpath.includes("*")) continue;
      const file = packageFile(ctx, dir, entry.path);
      if (!file || !file.checker) continue;
      const label = entry.subpath === "." ? name : `${name}/${entry.subpath.replace(/^\.\//, "")}`;
      if (seen.has(`${file.rel}|${label}`)) continue;
      seen.add(`${file.rel}|${label}`);
      exportedApi(ctx, file, label, add);
    }
  }
}

/** The exported functions of a module, and the public methods of its exported classes. */
function exportedApi(ctx, file, label, add) {
  const checker = file.checker;
  // A CommonJS module has no symbol at its location, only the one the binder gave its file.
  const moduleSymbol = safe(() => checker.getSymbolAtLocation(file.sf)) ?? file.sf.symbol;
  if (!moduleSymbol) return;
  const exports = safe(() => checker.getExportsOfModule(moduleSymbol)) ?? [];
  for (const exported of exports) {
    const exportName = exported.escapedName === "export=" ? "default" : String(exported.escapedName);
    let symbol = exported;
    if (symbol.flags & ts.SymbolFlags.Alias) symbol = safe(() => checker.getAliasedSymbol(symbol)) ?? symbol;
    for (const decl of symbol.declarations ?? []) {
      if (ts.isClassLike(decl)) {
        for (const member of decl.members) {
          const record = ctx.resolver.recordOfNode(member);
          if (!record || !isPublic(member)) continue;
          const memberName = record.name.slice(record.name.lastIndexOf(".") + 1);
          add({ funcId: record.id, entryPoint: { kind: "exported_api", label: `${label}.${exportName}.${memberName}` } });
        }
        continue;
      }
      const record = ctx.resolver.recordOfDeclaration(decl, checker);
      if (record && record.kind !== "module") {
        add({ funcId: record.id, entryPoint: { kind: "exported_api", label: `${label}.${exportName}` } });
        break;
      }
    }
  }
}

function isPublic(member) {
  if (member.name && ts.isPrivateIdentifier(member.name)) return false;
  const flags = ts.getCombinedModifierFlags(member);
  return (flags & (ts.ModifierFlags.Private | ts.ModifierFlags.Protected)) === 0;
}

/** The dependencies of the package holding `dir` (the nearest package.json up to the tree root). */
function dependenciesOf(ctx, rel) {
  let dir = dirname(join(ctx.root, rel));
  while (dir.startsWith(ctx.root)) {
    const pkg = readJson(join(dir, "package.json"));
    if (pkg) return { ...(pkg.dependencies ?? {}), ...(pkg.devDependencies ?? {}), ...(pkg.peerDependencies ?? {}) };
    if (dir === ctx.root) break;
    dir = dirname(dir);
  }
  return {};
}

/** Next.js route handlers and API routes, Remix / React Router route modules. */
function frameworkFiles(ctx, add) {
  const depsCache = new Map();
  for (const file of ctx.index.files.values()) {
    const parts = file.rel.split("/");
    const base = parts.at(-1).replace(/\.[cm]?[jt]sx?$/, "");
    const dirKey = parts.slice(0, -1).join("/");
    if (!depsCache.has(dirKey)) depsCache.set(dirKey, dependenciesOf(ctx, file.rel));
    const deps = depsCache.get(dirKey);
    const named = (name) => file.records.find((r) => r.name === name);
    if (deps.next) {
      const app = parts.lastIndexOf("app");
      if (app >= 0 && base === "route") {
        const route = routePath(parts.slice(app + 1, -1), "next");
        for (const verb of HTTP_VERBS) {
          const record = named(verb);
          if (record) add({ funcId: record.id, entryPoint: { kind: "http_route", label: `${verb} ${route}` } });
        }
      }
      const pages = parts.indexOf("pages");
      if (pages >= 0 && parts[pages + 1] === "api") {
        const segments = parts.slice(pages + 1, -1);
        if (base !== "index") segments.push(base);
        const record = named("default");
        if (record) add({ funcId: record.id, entryPoint: { kind: "http_route", label: `ANY ${routePath(segments, "next")}` } });
      }
    }
    const remix = Object.keys(deps).some((d) => d.startsWith("@remix-run/") || d === "@react-router/dev");
    const routes = parts.indexOf("routes");
    if (remix && routes > 0 && parts[routes - 1] === "app") {
      const name = parts.length - routes > 2 ? parts[routes + 1] : base;
      if (parts.length - routes > 2 && !["route", "index"].includes(base)) continue;
      const route = remixPath(name);
      for (const [exportName, verb] of [["loader", "GET"], ["action", "POST"]]) {
        const record = named(exportName);
        if (record) add({ funcId: record.id, entryPoint: { kind: "http_route", label: `${verb} ${route}` } });
      }
    }
  }
}

function routePath(segments) {
  const kept = segments.filter((s) => !(s.startsWith("(") && s.endsWith(")")) && !s.startsWith("@"));
  return `/${kept.join("/")}`;
}

/** The URL of a Remix flat route file name: users.$id -> /users/:id, _index -> /, _auth.login -> /login. */
function remixPath(name) {
  const segments = name
    .split(".")
    .filter((s) => s !== "_index" && !s.startsWith("_"))
    .map((s) => s.replace(/_$/, ""))
    .map((s) => (s.startsWith("$") ? (s === "$" ? "*" : `:${s.slice(1)}`) : s));
  return `/${segments.join("/")}`;
}

/** The constant text of a path expression, with {dynamic} for the parts that are not constant; null for none. */
function pathLabel(expr) {
  const e = unwrap(expr);
  if (!e) return null;
  const text = stringValue(e);
  if (text !== null) return text;
  if (ts.isTemplateExpression(e)) {
    let out = e.head.text;
    for (const span of e.templateSpans) out += DYNAMIC + span.literal.text;
    return out;
  }
  if (ts.isBinaryExpression(e) && e.operatorToken.kind === ts.SyntaxKind.PlusToken) {
    const left = pathLabel(e.left) ?? DYNAMIC;
    const right = pathLabel(e.right) ?? DYNAMIC;
    return (left + right).replaceAll(DYNAMIC + DYNAMIC, DYNAMIC);
  }
  if (ts.isRegularExpressionLiteral(e)) return e.text;
  return null;
}

/** Whether a path label looks like a URL path (so x.get("/a", f) is a route and map.get(key) is not). */
function isRoutePath(label) {
  return label !== null && (label.startsWith("/") || label === "*" || label.startsWith(DYNAMIC + "/") || label === DYNAMIC);
}

function joinPath(prefix, path) {
  if (!prefix) return path;
  if (path === "/" || path === "") return prefix;
  return `${prefix.replace(/\/+$/, "")}/${path.replace(/^\/+/, "")}`;
}

/** The declaration key of the variable an expression names (imports followed), to match routers across files. */
function valueKey(expr, checker) {
  const e = unwrap(expr);
  if (!ts.isIdentifier(e) && !ts.isPropertyAccessExpression(e)) return null;
  let symbol = safe(() => checker.getSymbolAtLocation(e));
  if (symbol && symbol.flags & ts.SymbolFlags.Alias) symbol = safe(() => checker.getAliasedSymbol(symbol)) ?? symbol;
  const decl = symbol?.valueDeclaration ?? symbol?.declarations?.[0];
  if (!decl) return null;
  return `${decl.getSourceFile().fileName}:${decl.pos}`;
}

/**
 * Prefixes routers are mounted at: use(prefix, router) and route(prefix, app) by the router's variable, and
 * register(plugin, { prefix }) by the plugin function's id. Each with the receiver it was mounted on, so that
 * nested mounts add up.
 */
function collectGroups(ctx) {
  const byValue = new Map();
  const byFunction = new Map();
  for (const file of ctx.index.files.values()) {
    const checker = file.checker;
    if (!checker) continue;
    const visit = (node) => {
      if (ts.isCallExpression(node) && ts.isPropertyAccessExpression(node.expression)) {
        const method = node.expression.name.text;
        const receiver = node.expression.expression;
        const args = node.arguments;
        if ((method === "use" || method === "route") && args.length >= 2) {
          const prefix = pathLabel(args[0]);
          if (isRoutePath(prefix)) {
            for (const arg of args.slice(1)) {
              const key = valueKey(arg, checker);
              if (key && ctx.resolver.valueTargets(arg, checker).length === 0) push(byValue, key, { prefix, receiver, checker });
            }
          }
        }
        if (method === "register" && args.length >= 2 && ts.isObjectLiteralExpression(unwrap(args[1]))) {
          const prefixProp = unwrap(args[1]).properties.find((p) => ts.isPropertyAssignment(p) && p.name.getText() === "prefix");
          const prefix = prefixProp ? pathLabel(prefixProp.initializer) : null;
          if (isRoutePath(prefix)) {
            for (const t of ctx.resolver.valueTargets(args[0], checker)) push(byFunction, t.record.id, { prefix, receiver, checker });
          }
        }
      }
      ts.forEachChild(node, visit);
    };
    visit(file.sf);
  }
  return { byValue, byFunction };
}

function push(map, key, value) {
  const list = map.get(key);
  if (list) list.push(value);
  else map.set(key, [value]);
}

/** The prefixes routes registered on `receiver` get from where it is mounted ([""] when it is not). */
function prefixesOf(ctx, groups, receiver, checker, depth = 0) {
  if (depth > 5) return [""];
  const out = [];
  const key = valueKey(receiver, checker);
  for (const mount of (key && groups.byValue.get(key)) ?? []) {
    for (const outer of prefixesOf(ctx, groups, mount.receiver, mount.checker, depth + 1)) out.push(joinPath(outer, mount.prefix));
  }
  // The instance a plugin function gets as its first parameter.
  const e = unwrap(receiver);
  if (ts.isIdentifier(e)) {
    const symbol = safe(() => checker.getSymbolAtLocation(e));
    const decl = symbol?.valueDeclaration;
    if (decl && ts.isParameter(decl) && decl.parent.parameters[0] === decl) {
      const owner = ctx.resolver.recordOfNode(decl.parent);
      for (const mount of (owner && groups.byFunction.get(owner.id)) ?? []) {
        for (const outer of prefixesOf(ctx, groups, mount.receiver, mount.checker, depth + 1)) out.push(joinPath(outer, mount.prefix));
      }
    }
  }
  return out.length > 0 ? [...new Set(out)] : [""];
}

/** Whether `receiver` looks like a server or router: a known framework's instance, or a conventional name. */
function looksLikeServer(ctx, receiver, checker) {
  const e = unwrap(receiver);
  if (ts.isCallExpression(e)) return looksLikeServer(ctx, e.expression, checker) || serverOrigin(ctx, e.expression, checker);
  if (ts.isPropertyAccessExpression(e)) return SERVER_NAME.test(e.name.text) || looksLikeServer(ctx, e.expression, checker);
  if (!ts.isIdentifier(e)) return false;
  if (SERVER_NAME.test(e.text)) return true;
  const symbol = safe(() => checker.getSymbolAtLocation(e));
  const decl = symbol?.valueDeclaration;
  if (decl && ts.isVariableDeclaration(decl) && decl.initializer) {
    const init = unwrap(decl.initializer);
    if ((ts.isCallExpression(init) || ts.isNewExpression(init)) && serverOrigin(ctx, init.expression, checker)) return true;
  }
  return false;
}

function serverOrigin(ctx, expr, checker) {
  const origin = ctx.resolver.expressionOrigin(expr, checker);
  return origin !== null && SERVER_MODULES.has(origin.module);
}

/** Registrations made by calls: routes, middleware, plugins, hooks, commands, jobs and event listeners. */
function callRegistrations(ctx, file, groups, add) {
  const checker = file.checker;
  const resolver = ctx.resolver;
  const specifiers = new Set(file.imports);
  const cli = [...specifiers].some((s) => CLI_MODULES.has(s));
  const register = (node, targets, evidence, entryPoint) => {
    const caller = resolver.enclosing(node);
    const line = lineOf(node);
    for (const t of targets) {
      const registration = caller && caller.id !== t.record.id ? { caller: caller.id, line, evidence, value: t.node } : null;
      add({ funcId: t.record.id, entryPoint: entryPoint ? { ...entryPoint, file: file.rel, line } : null, registration });
    }
  };
  const handlersOf = (args) =>
    args.flatMap((arg) => {
      const a = unwrap(arg);
      if (ts.isObjectLiteralExpression(a)) {
        const handler = a.properties.find((p) => ts.isPropertyAssignment(p) && p.name.getText() === "handler");
        return handler ? resolver.valueTargets(handler.initializer, checker) : [];
      }
      return resolver.valueTargets(arg, checker);
    });
  const route = (node, verbs, path, receiver, handlerArgs) => {
    const targets = handlersOf(handlerArgs);
    if (targets.length === 0) return;
    for (const prefix of prefixesOf(ctx, groups, receiver, checker)) {
      for (const verb of verbs) {
        const label = `${verb} ${joinPath(prefix, path)}`;
        register(node, targets, `route ${label}`, { kind: "http_route", label });
      }
    }
  };

  const visit = (node) => {
    if (ts.isCallExpression(node) || ts.isNewExpression(node)) inspect(node);
    ts.forEachChild(node, visit);
  };
  const inspect = (node) => {
    const args = [...(node.arguments ?? [])];
    const callee = unwrap(node.expression);
    if (ts.isNewExpression(node)) {
      const origin = resolver.expressionOrigin(callee, checker);
      if (origin?.module === "cron" && origin.names.at(-1) === "CronJob" && args.length >= 2) {
        const expr = pathLabel(args[0]) ?? DYNAMIC;
        register(node, resolver.valueTargets(args[1], checker), `schedule ${expr}`, { kind: "scheduled_job", label: expr });
      }
      return;
    }
    if (!ts.isPropertyAccessExpression(callee)) {
      if (ts.isIdentifier(callee) && callee.text === "addEventListener" && args.length >= 2) {
        const event = stringValue(args[0]) ?? DYNAMIC;
        register(node, resolver.valueTargets(args[1], checker), `event ${event}`, null);
      }
      return;
    }
    const method = callee.name.text;
    const receiver = callee.expression;

    if (VERBS.has(method)) {
      const verb = method === "all" || method === "any" ? "ANY" : method === "del" ? "DELETE" : method.toUpperCase();
      const path = args.length >= 2 ? pathLabel(args[0]) : null;
      if (isRoutePath(path)) {
        route(node, [verb], path, receiver, args.slice(1));
        return;
      }
      // route("/x").get(h).post(h2)
      const chained = routeChain(receiver);
      if (chained !== null && args.length >= 1) route(node, [verb], chained.path, chained.receiver, args);
      return;
    }
    if (method === "route" && args.length === 1 && ts.isObjectLiteralExpression(unwrap(args[0]))) {
      const options = unwrap(args[0]);
      const prop = (name) => options.properties.find((p) => ts.isPropertyAssignment(p) && p.name.getText() === name);
      const methods = prop("method") ? constantStrings(prop("method").initializer) : [];
      const url = prop("url") ?? prop("path");
      const path = url ? pathLabel(url.initializer) : null;
      if (methods.length > 0 && isRoutePath(path) && prop("handler")) {
        route(node, methods.map((m) => m.toUpperCase()), path, receiver, [prop("handler").initializer]);
        for (const hook of ["preHandler", "onRequest", "preValidation", "preParsing", "preSerialization", "onSend", "onResponse"]) {
          if (prop(hook)) register(node, resolver.valueTargets(prop(hook).initializer, checker), "middleware", null);
        }
      }
      return;
    }
    if (method === "on" && args.length >= 3 && isRoutePath(pathLabel(args[1]))) {
      const verbs = constantStrings(args[0]).map((m) => m.toUpperCase());
      if (verbs.length > 0) {
        route(node, verbs, pathLabel(args[1]), receiver, args.slice(2));
        return;
      }
    }
    if (method === "use" && looksLikeServer(ctx, receiver, checker)) {
      const prefix = args.length >= 2 ? pathLabel(args[0]) : null;
      const rest = isRoutePath(prefix) ? args.slice(1) : args;
      const targets = rest.flatMap((arg) => resolver.valueTargets(arg, checker));
      register(node, targets, isRoutePath(prefix) ? `middleware ${prefix}` : "middleware", null);
      return;
    }
    if (method === "register" && args.length >= 1 && looksLikeServer(ctx, receiver, checker)) {
      register(node, resolver.valueTargets(args[0], checker), "plugin", null);
      return;
    }
    if (method === "addHook" && args.length >= 2) {
      const hook = stringValue(args[0]) ?? DYNAMIC;
      register(node, resolver.valueTargets(args[1], checker), `hook ${hook}`, null);
      return;
    }
    if (cli && method === "action" && args.length >= 1) {
      const command = commandChain(receiver);
      if (command !== null) {
        register(node, resolver.valueTargets(args[0], checker), `command ${command}`, { kind: "cli_command", label: command });
      }
      return;
    }
    if (cli && method === "command" && args.length >= 1) {
      const first = unwrap(args[0]);
      if (ts.isObjectLiteralExpression(first)) {
        const prop = (name) => first.properties.find((p) => ts.isPropertyAssignment(p) && p.name.getText() === name);
        const name = prop("command") ? commandName(stringValue(prop("command").initializer)) : null;
        if (name !== null && prop("handler")) {
          register(node, resolver.valueTargets(prop("handler").initializer, checker), `command ${name}`, { kind: "cli_command", label: name });
        }
      } else if (args.length >= 3) {
        const name = commandName(stringValue(first));
        if (name !== null) {
          register(node, resolver.valueTargets(args.at(-1), checker), `command ${name}`, { kind: "cli_command", label: name });
        }
      }
      return;
    }
    if (method === "schedule" && args.length >= 2) {
      const origin = resolver.expressionOrigin(receiver, checker);
      if (origin?.module === "node-cron") {
        const expr = pathLabel(args[0]) ?? DYNAMIC;
        register(node, resolver.valueTargets(args[1], checker), `schedule ${expr}`, { kind: "scheduled_job", label: expr });
      }
      return;
    }
    if (EVENT_METHODS.has(method) && args.length >= 2) {
      const event = stringValue(args[0]);
      if (event !== null || method === "addEventListener") {
        register(node, resolver.valueTargets(args[1], checker), `event ${event ?? DYNAMIC}`, null);
      }
    }
  };
  visit(file.sf);
}

/** For x.route("/p").get(...).post(...): the path and the router; null when the receiver is not such a chain. */
function routeChain(receiver) {
  let e = unwrap(receiver);
  while (ts.isCallExpression(e) && ts.isPropertyAccessExpression(e.expression)) {
    const method = e.expression.name.text;
    if (method === "route" && e.arguments.length === 1) {
      const path = pathLabel(e.arguments[0]);
      return isRoutePath(path) ? { path, receiver: e.expression.expression } : null;
    }
    if (!VERBS.has(method)) return null;
    e = unwrap(e.expression.expression);
  }
  return null;
}

/** For program.command("serve <port>").description(...).action(fn): "serve"; null without a command. */
function commandChain(receiver) {
  let e = unwrap(receiver);
  while (ts.isCallExpression(e) && ts.isPropertyAccessExpression(e.expression)) {
    if (e.expression.name.text === "command" && e.arguments.length >= 1) return commandName(stringValue(e.arguments[0]));
    e = unwrap(e.expression.expression);
  }
  return null;
}

function commandName(text) {
  if (text === null) return null;
  const word = text.trim().split(/\s+/)[0];
  if (!word || word === "$0" || word.startsWith("<") || word.startsWith("[")) return null;
  return word;
}

/** The constant strings of a string or an array of strings. */
function constantStrings(expr) {
  const e = unwrap(expr);
  if (ts.isArrayLiteralExpression(e)) return e.elements.map(stringValue).filter((s) => s !== null);
  const s = stringValue(e);
  return s === null ? [] : [s];
}

/** NestJS controllers, message handlers, jobs, gRPC methods and GraphQL resolvers, from their decorators. */
function nestHandlers(ctx, file, add) {
  if (!file.imports.some((s) => s.startsWith("@nestjs/"))) return;
  const checker = file.checker;
  const decorators = (node) =>
    (ts.canHaveDecorators(node) ? (ts.getDecorators(node) ?? []) : []).flatMap((d) => {
      const e = d.expression;
      const call = ts.isCallExpression(e) ? e : null;
      const nameNode = call ? call.expression : e;
      const chain = nameChain(nameNode);
      if (!chain) return [];
      const origin = ctx.resolver.expressionOrigin(nameNode, checker);
      if (origin && !origin.module.startsWith("@nestjs/")) return [];
      return [{ name: chain.at(-1), args: call ? [...call.arguments] : [] }];
    });
  const visit = (node) => {
    if (ts.isClassLike(node)) {
      const classDecorators = decorators(node);
      const controller = classDecorators.find((d) => d.name === "Controller");
      let prefix = "";
      if (controller && controller.args.length > 0) {
        const arg = unwrap(controller.args[0]);
        if (ts.isObjectLiteralExpression(arg)) {
          const path = arg.properties.find((p) => ts.isPropertyAssignment(p) && p.name.getText() === "path");
          prefix = path ? (pathLabel(path.initializer) ?? DYNAMIC) : "";
        } else prefix = pathLabel(arg) ?? DYNAMIC;
      }
      for (const member of node.members) {
        const record = ctx.resolver.recordOfNode(member);
        if (!record) continue;
        const memberName = member.name?.getText() ?? "";
        for (const d of decorators(member)) {
          const firstText = d.args.length > 0 ? (pathLabel(d.args[0]) ?? d.args[0].getText()) : null;
          if (controller && NEST_ROUTES[d.name]) {
            const path = d.args.length > 0 ? (pathLabel(d.args[0]) ?? DYNAMIC) : "";
            const full = `/${[prefix, path].filter((p) => p !== "").map((p) => p.replace(/^\/+|\/+$/g, "")).join("/")}`;
            add({ funcId: record.id, entryPoint: { kind: "http_route", label: `${NEST_ROUTES[d.name]} ${full}` } });
          } else if (NEST_MESSAGES.has(d.name)) {
            add({ funcId: record.id, entryPoint: { kind: "message_handler", label: firstText ?? memberName } });
          } else if (NEST_JOBS.has(d.name)) {
            add({ funcId: record.id, entryPoint: { kind: "scheduled_job", label: firstText ?? memberName } });
          } else if (d.name === "GrpcMethod") {
            const service = d.args.length > 0 ? stringValue(d.args[0]) : null;
            const rpc = d.args.length > 1 ? stringValue(d.args[1]) : null;
            add({ funcId: record.id, entryPoint: { kind: "rpc_method", label: [service, rpc ?? memberName].filter(Boolean).join(".") } });
          } else if (NEST_GRAPHQL.has(d.name) && classDecorators.some((c) => c.name === "Resolver")) {
            const named = d.args.find((a) => stringValue(a) !== null);
            add({ funcId: record.id, entryPoint: { kind: "rpc_method", label: `${d.name} ${named ? stringValue(named) : memberName}` } });
          }
        }
      }
    }
    ts.forEachChild(node, visit);
  };
  visit(file.sf);
}
