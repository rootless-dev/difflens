// What a run reported, turned into the protocol's tests and packages, and
// the progress file kept up to date as the reporters' events arrive.
//
// A test is named by its titles joined with "/" (a "/" inside a title
// becomes "∕", U+2215, so the core can tell the levels apart), such as
// "math/divide/refuses zero", and its hierarchy lists those titles as the
// name writes them (["math", "divide", "refuses zero"]); every describe
// block (and a node:test test with subtests) is listed too, with the
// outcome of what it holds. A test's function is the callback of the
// top-level describe/test call of its file with that title, as analyze
// names it.

import { closeSync, openSync, readSync, readFileSync } from "node:fs";

import { indexFile, parseFile } from "../analyze/functions.js";
import { containerOf, isTestFile } from "../files.js";
import { keepEnd } from "../process.js";

export const MAX_OUTPUT = 64 << 10;
const PROGRESS_INTERVAL = 250;

/** A title as a level of a test's name: "/" inside it would read as a level, so it becomes "∕". */
export function escapeTitle(title) {
  return String(title).replaceAll("/", "∕");
}

/** The titles of a name written by escapeTitle. */
export function nameTitles(name) {
  return name.split("/").map((t) => t.replaceAll("∕", "/"));
}

/** Messages of a suite that could not run because it does not compile or its imports fail. */
const BUILD_FAILURE = /SyntaxError|Unexpected token|error TS\d+|TSError|Transform failed|Failed to (load|resolve|parse)|Cannot find module|ERR_MODULE_NOT_FOUND|Cannot use import statement|does not provide an export named|is not a module|Jest encountered an unexpected token|Parse failure|Expected .* but found/;

export function isBuildFailure(error) {
  return BUILD_FAILURE.test(error ?? "");
}

const RANK = { build_failed: 3, fail: 2, pass: 1, no_tests: 0 };

export class Session {
  /**
   * `tree`, `rootName` (container of root files), `progress`: a function writing the progress document, or
   * null; `commands`: { test(file, path, leaf), package(container) } giving the commands that run a test or
   * a package again (null when there is none), or null for no commands.
   */
  constructor({ tree, rootName, progress, commands = null }) {
    this.tree = tree;
    this.rootName = rootName;
    this.writeProgress = progress;
    this.commands = commands;
    this.files = new Map();
    this.order = [];
    this.expected = new Map();
    this.counted = new Set();
    this.topTests = 0;
    this.topFailed = 0;
    this.current = null;
    this.written = 0;
    this.catalogs = new Map();
    this.fatal = [];
    this.coverageFiles = [];
  }

  container(file) {
    const rel = this.tree.rel(file);
    return rel === null ? null : containerOf(rel, this.rootName);
  }

  /** One name per file: runners print the tree's real path (/private/var) where it was given another (/var). */
  canonical(file) {
    const rel = this.tree.rel(file);
    return rel === null ? file : this.tree.abs(rel);
  }

  /** Expects `files` (absolute) to report, for the progress. */
  expect(files) {
    for (const raw of files) {
      const file = this.canonical(raw);
      const container = this.container(file);
      if (container === null) continue;
      const set = this.expected.get(container) ?? new Set();
      set.add(file);
      this.expected.set(container, set);
    }
  }

  /** Containers reported without any test file running in them: they count as done from the start. */
  expectEmpty(containers) {
    for (const c of containers) if (!this.expected.has(c)) this.expected.set(c, new Set());
  }

  fileResult(raw) {
    const file = this.canonical(raw);
    let r = this.files.get(file);
    if (!r) {
      const rel = this.tree.rel(file);
      r = { file, rel, container: rel === null ? null : containerOf(rel, this.rootName), events: [], suites: [], error: null, ms: 0, ended: false, started: Date.now(), filter: null };
      this.files.set(file, r);
      this.order.push(r);
      if (r.container !== null) {
        const set = this.expected.get(r.container) ?? new Set();
        set.add(file);
        this.expected.set(r.container, set);
      }
    }
    return r;
  }

  /** Handles one event of the reporters' file. `filter` (or null) is the selection of the invocation. */
  handle(event, filter) {
    switch (event.e) {
      case "start": {
        const r = this.fileResult(event.file);
        r.filter = filter;
        this.current = r.container;
        this.report(false);
        break;
      }
      case "test":
        this.fileResult(event.file).events.push(event);
        break;
      case "suite":
        this.fileResult(event.file).suites.push(event);
        break;
      case "end": {
        const r = this.fileResult(event.file);
        r.filter ??= filter;
        if (event.error) r.error = r.error ? `${r.error}\n${event.error}` : event.error;
        r.ms = Math.max(r.ms, Number(event.ms) || 0);
        this.end(r);
        break;
      }
      case "coverage":
        this.coverageFiles.push(...(event.files ?? []));
        break;
      case "fatal":
        if (event.error) this.fatal.push(event.error);
        break;
    }
  }

  end(r) {
    if (r.ended) return;
    r.ended = true;
    if (!r.ms) r.ms = Date.now() - r.started;
    const tops = new Map();
    for (const t of this.tests(r)) {
      if (t.name.includes("/")) continue;
      tops.set(t.name, t.outcome);
    }
    for (const outcome of tops.values()) {
      this.topTests++;
      if (outcome === "fail") this.topFailed++;
    }
    if (this.current === r.container) this.current = null;
    this.report(true);
  }

  /** Ends the files an invocation left without an end: `stopped` when it was killed. */
  finishInvocation(stopped) {
    for (const r of this.order) {
      if (r.ended) continue;
      if (stopped) {
        r.error = `${r.error ? `${r.error}\n` : ""}the file did not finish before the run was stopped`;
        r.stopped = true;
      }
      this.end(r);
    }
    this.current = null;
  }

  done() {
    let done = 0;
    for (const [, files] of this.expected) {
      if ([...files].every((f) => this.files.get(f)?.ended)) done++;
    }
    return done;
  }

  report(force) {
    if (!this.writeProgress) return;
    const now = Date.now();
    if (!force && now - this.written < PROGRESS_INTERVAL) return;
    this.written = now;
    const done = this.done();
    this.writeProgress({
      packagesDone: done,
      packagesTotal: Math.max(this.expected.size, done),
      tests: this.topTests,
      failed: this.topFailed,
      current: this.current,
    });
  }

  /** The top-level test functions of a test file, by title: { id, file, line }. */
  catalog(r) {
    if (this.catalogs.has(r.file)) return this.catalogs.get(r.file);
    const byTitle = new Map();
    if (r.rel !== null) {
      try {
        const sf = parseFile(r.file, readFileSync(r.file, "utf8"), r.rel);
        const indexed = indexFile(sf, r.rel, { container: r.container, testFile: isTestFile(r.rel), testFramework: true });
        for (const { call, record, top } of indexed.testCalls) {
          if (!top || call.hook || call.title === null || byTitle.has(call.title)) continue;
          byTitle.set(call.title, { id: record.id, file: r.rel, line: record.startLine });
        }
      } catch {
        // A file that does not parse has no test functions to link.
      }
    }
    this.catalogs.set(r.file, byTitle);
    return byTitle;
  }

  /**
   * The protocol tests of one file: every test reported and every level above it, in the order they
   * finished (a level after what it holds). Skipped tests the invocation's selection did not ask for are left
   * out (Jest and Vitest report the tests a name pattern excludes as skipped).
   */
  tests(r) {
    if (r.cached) return r.cached;
    const nodes = new Map();
    const order = [];
    const node = (path) => {
      const name = path.map(escapeTitle).join("/");
      let n = nodes.get(name);
      if (!n) {
        n = { name, path, own: null, children: [], suite: null };
        nodes.set(name, n);
      }
      return n;
    };
    for (const event of r.events) {
      if (!Array.isArray(event.path) || event.path.length === 0) continue;
      if (event.status === "skip" && r.filter && !r.filter(event.path)) continue;
      const n = node(event.path);
      n.own = event;
      if (!order.includes(n)) order.push(n);
      for (let k = event.path.length - 1; k >= 1; k--) {
        const parent = node(event.path.slice(0, k));
        if (!parent.children.includes(nodes.get(event.path.slice(0, k + 1).map(escapeTitle).join("/")))) {
          parent.children.push(nodes.get(event.path.slice(0, k + 1).map(escapeTitle).join("/")));
        }
      }
    }
    for (const suite of r.suites) {
      if (!Array.isArray(suite.path) || suite.path.length === 0) continue;
      const n = node(suite.path);
      n.suite = suite;
      for (let k = suite.path.length - 1; k >= 1; k--) {
        const parent = node(suite.path.slice(0, k));
        const child = nodes.get(suite.path.slice(0, k + 1).map(escapeTitle).join("/"));
        if (!parent.children.includes(child)) parent.children.push(child);
      }
    }
    const computed = new Map();
    const compute = (n) => {
      if (computed.has(n)) return computed.get(n);
      const kids = n.children.map(compute);
      let outcome;
      let ms = n.own ? Number(n.own.ms) || 0 : kids.reduce((s, k) => s + k.ms, 0);
      // Its output: its own failure (a hook, its body), then the failures of the tests below it, each named.
      const own = [n.suite?.output, n.own?.output].filter(Boolean);
      const failures = own.length > 0 ? [`${n.name}:\n${own.join("\n\n")}`] : [];
      for (const k of kids) if (k.outcome === "fail") failures.push(...k.failures);
      const outputs = n.children.length === 0 ? own : failures;
      if (n.suite || n.own?.status === "fail" || kids.some((k) => k.outcome === "fail")) outcome = "fail";
      else if (n.own) outcome = n.own.status;
      else if (kids.length > 0 && kids.every((k) => k.outcome === "skip")) outcome = "skip";
      else outcome = "pass";
      if (n.suite && !n.own) ms += Number(n.suite.ms) || 0;
      const out = {
        name: n.name,
        path: n.path,
        // A test with nothing below it; a describe level, or a test with subtests, is not one.
        leaf: n.own !== null && n.children.length === 0,
        outcome,
        ms,
        output: outcome === "fail" ? keepEnd(outputs.join("\n\n"), MAX_OUTPUT) : null,
        failures,
      };
      computed.set(n, out);
      return out;
    };
    const listed = [];
    const emitted = new Set();
    const visitUp = (n) => {
      if (emitted.has(n)) return;
      for (const child of n.children) if (!emitted.has(child)) return;
      emitted.add(n);
      listed.push(compute(n));
      if (n.path.length > 1) visitUp(nodes.get(n.path.slice(0, -1).map(escapeTitle).join("/")));
    };
    for (const n of order) visitUp(n);
    // Levels whose children were all left out (a suite failing in a hook before its tests).
    for (const n of [...nodes.values()].sort((a, b) => b.path.length - a.path.length)) {
      if (!emitted.has(n) && (n.own || n.suite || n.children.some((c) => emitted.has(c)))) {
        emitted.add(n);
        listed.push(compute(n));
      }
    }
    r.cached = listed;
    return listed;
  }

  /** The protocol's tests and packages of everything reported, packages of `containers` included. */
  results(containers) {
    const tests = [];
    const packages = new Map();
    const addPackage = (p) => {
      const current = packages.get(p.container);
      if (!current) {
        packages.set(p.container, { ...p });
        return;
      }
      current.durationMs += p.durationMs;
      if (RANK[p.outcome] > RANK[current.outcome]) current.outcome = p.outcome;
      if (p.output) current.output = current.output ? keepEnd(`${current.output}\n${p.output}`, MAX_OUTPUT) : p.output;
    };
    for (const r of this.order) {
      if (r.container === null) continue;
      const list = this.tests(r);
      const catalog = this.catalog(r);
      for (const t of list) {
        const fn = catalog.get(t.path[0]) ?? null;
        tests.push({
          container: r.container,
          package: r.container,
          name: t.name,
          hierarchy: t.path.map(escapeTitle),
          function: fn?.id ?? null,
          file: fn?.file ?? null,
          line: fn?.line ?? null,
          outcome: t.outcome,
          durationMs: Math.max(0, Math.round(t.ms)),
          output: t.output,
          command: this.commands?.test(r.file, t.path, t.leaf) ?? null,
        });
      }
      const failedTests = list.some((t) => t.outcome === "fail" && !t.name.includes("/"));
      let outcome;
      let output = null;
      if (r.error && (list.length === 0 || r.stopped)) {
        outcome = !r.stopped && isBuildFailure(r.error) ? "build_failed" : "fail";
        output = r.error;
      } else if (r.error) {
        outcome = "fail";
        if (!failedTests) output = r.error;
      } else if (failedTests) {
        outcome = "fail";
      } else if (list.length > 0) {
        outcome = "pass";
      } else {
        outcome = "no_tests";
      }
      addPackage({ container: r.container, outcome, durationMs: Math.max(0, Math.round(r.ms)), output: output === null ? null : keepEnd(output, MAX_OUTPUT) });
    }
    for (const c of containers) if (!packages.has(c)) packages.set(c, { container: c, outcome: "no_tests", durationMs: 0, output: null });
    for (const p of packages.values()) {
      if (p.outcome === "pass" || p.outcome === "no_tests") p.output = null;
      p.command = this.commands?.package(p.container) ?? null;
    }
    const sorted = [...packages.values()].sort((a, b) => (a.container < b.container ? -1 : a.container > b.container ? 1 : 0));
    return { tests, packages: sorted };
  }
}

/** Reads the lines appended to a file since the last call. */
export class EventFile {
  constructor(path) {
    this.path = path;
    this.offset = 0;
    this.rest = "";
  }

  /** The events appended since the last read. */
  read() {
    let fd;
    const chunks = [];
    try {
      fd = openSync(this.path, "r");
      const buffer = Buffer.alloc(1 << 20);
      for (;;) {
        const n = readSync(fd, buffer, 0, buffer.length, this.offset);
        if (n <= 0) break;
        this.offset += n;
        chunks.push(Buffer.from(buffer.subarray(0, n)));
      }
    } catch {
      return [];
    } finally {
      if (fd !== undefined) closeSync(fd);
    }
    const text = this.rest + Buffer.concat(chunks).toString("utf8");
    const lines = text.split("\n");
    this.rest = lines.pop() ?? "";
    const events = [];
    for (const line of lines) {
      if (line.trim() === "") continue;
      try {
        events.push(JSON.parse(line));
      } catch {
        // A line cut by a crash.
      }
    }
    return events;
  }
}
