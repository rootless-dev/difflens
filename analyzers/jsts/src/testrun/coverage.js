// Coverage of the project's own code as the protocol's blocks, from what the
// runners' providers write: Istanbul's coverage-final.json (Vitest with v8 or
// istanbul, Jest, c8 and nyc for Mocha), one block per statement, and the
// line counts of node --test, one block per line holding code.

import { readFileSync } from "node:fs";

import { isGenerated, isSourceFile, isTestFile, readHead } from "../files.js";

/** Merges coverage blocks: one per file and span, counts added (two runs of a block ran it twice). */
export class Coverage {
  constructor(tree) {
    this.tree = tree;
    this.blocks = new Map();
    this.measured = false;
    this.own = new Map();
  }

  /** Whether a covered file is the project's own code: in the tree, a source file, not a test, not generated. */
  ownFile(file) {
    if (this.own.has(file)) return this.own.get(file);
    const rel = this.tree.rel(file);
    const ok =
      rel !== null &&
      !rel.split("/").includes("node_modules") &&
      isSourceFile(rel) &&
      !isTestFile(rel) &&
      !isGenerated(rel, readHead(this.tree.abs(rel)));
    const out = ok ? rel : null;
    this.own.set(file, out);
    return out;
  }

  add(rel, startLine, endLine, statements, count) {
    if (startLine < 1) return;
    const end = Math.max(startLine, endLine);
    const key = `${rel}\u0000${startLine}\u0000${end}`;
    const block = this.blocks.get(key);
    if (block) {
      block.count += count;
      block.statements = Math.max(block.statements, statements);
    } else {
      this.blocks.set(key, { file: rel, startLine, endLine: end, statements, count });
    }
  }

  /** Adds an Istanbul coverage map (the contents of coverage-final.json). */
  addIstanbul(map) {
    this.measured = true;
    for (const [key, entry] of Object.entries(map ?? {})) {
      const rel = this.ownFile(entry?.path ?? key);
      if (rel === null) continue;
      // Statements spanning the same lines become one block, run when any of them ran.
      const spans = new Map();
      for (const [id, loc] of Object.entries(entry.statementMap ?? {})) {
        const start = loc?.start?.line;
        const end = loc?.end?.line ?? start;
        if (!Number.isInteger(start)) continue;
        const span = `${start}:${end}`;
        const count = Number(entry.s?.[id] ?? 0);
        const current = spans.get(span);
        if (current) {
          current.statements++;
          current.count = Math.max(current.count, count);
        } else spans.set(span, { start, end, statements: 1, count });
      }
      for (const s of spans.values()) this.add(rel, s.start, s.end, s.statements, s.count);
    }
  }

  /** Adds an Istanbul coverage-final.json file; false when it cannot be read. */
  addIstanbulFile(file) {
    let map;
    try {
      map = JSON.parse(readFileSync(file, "utf8"));
    } catch {
      return false;
    }
    this.addIstanbul(map);
    return true;
  }

  /**
   * Adds node --test's line counts ([{ path, lines: [[line, count]] }]). Node counts every line of a file, so
   * the lines without code (blank, comments, lone braces) are left out.
   */
  addLines(files) {
    this.measured = true;
    for (const f of files ?? []) {
      const rel = this.ownFile(f.path);
      if (rel === null) continue;
      let source = [];
      try {
        source = readFileSync(this.tree.abs(rel), "utf8").split(/\r?\n/);
      } catch {
        source = [];
      }
      let inComment = false;
      const code = source.map((text) => {
        const t = text.trim();
        if (inComment) {
          if (t.includes("*/")) inComment = false;
          return false;
        }
        if (t.startsWith("/*") && !t.includes("*/")) {
          inComment = true;
          return false;
        }
        return !(t === "" || t.startsWith("//") || (t.startsWith("/*") && t.endsWith("*/")) || /^[\])};,]+$/.test(t));
      });
      for (const [line, count] of f.lines ?? []) {
        if (code[line - 1] === false) continue;
        this.add(rel, line, line, 1, Number(count) || 0);
      }
    }
  }

  result() {
    const blocks = [...this.blocks.values()];
    blocks.sort((a, b) => (a.file < b.file ? -1 : a.file > b.file ? 1 : a.startLine - b.startLine || a.endLine - b.endLine));
    return { blocks };
  }
}
