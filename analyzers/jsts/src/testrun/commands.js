// Name patterns, and the commands a developer runs from the root of the
// tree to run one test, or one package's tests, again on their own with the
// runner that ran them (tests.schema.json, `command`):
//
//   vitest   npx vitest run '<file>' -t '<pattern>'
//   jest     npx jest '<file>' -t '<pattern>'
//   mocha    npx mocha '<file>' --grep '<pattern>'
//   node     node [the test script's flags, such as --import tsx] --test --test-name-pattern='<pattern>' '<file>'
//
// with `cd <folder> && ` in front for a unit that is not the root, files
// relative to that folder, and every argument quoted for the shell. A
// package's command is the same without the pattern, on its test files.
//
// The pattern is what the runners match: the titles of the test and its
// ancestors joined by spaces, regexp-escaped (with the real "/" a title
// holds), anchored at the start, and at the end too for a test without
// tests below it; a level with tests below it matches them all. node --test
// is given the top-level test (`^title$`): a subtest made with `t.test`
// exists only while its parent runs, and a parent that matches runs every
// test below it.

const regexEscape = (s) => s.replace(/[.*+?^${}()|[\]\\/]/g, "\\$&");

/** A name pattern matching the tests whose titles start with one of `prefixes`, joined by spaces. */
export function namePattern(prefixes) {
  const alternatives = prefixes.map((p) => p.map(regexEscape).join(" "));
  return `^(?:${alternatives.join("|")})(?: |$)`;
}

/** A name pattern matching exactly the test whose titles are `path`. */
export function exactPattern(path) {
  return `^(?:${path.map(regexEscape).join(" ")})$`;
}

/** `text` as one shell word in single quotes: a `'` inside becomes `'\''`. */
export function shellQuote(text) {
  return `'${String(text).replaceAll("'", "'\\''")}'`;
}

/** `text` as one shell word, quoted only when it holds anything the shell would read. */
function shellWord(text) {
  return /^[\w@%+=:,./-]+$/.test(text) ? text : shellQuote(text);
}

/** `line`, run in the folder of `unit`, from the root of the tree. */
function inFolder(unit, line) {
  return unit.rel === "." ? line : `cd ${shellWord(unit.rel)} && ${line}`;
}

/** The pattern a runner is given for the test whose titles are `path`; `leaf` when no test is below it. */
export function testPattern(runner, path, leaf) {
  if (runner === "node") return exactPattern(path.slice(0, 1));
  return leaf ? exactPattern(path) : namePattern([path]);
}

/**
 * The command that runs one test again: `unit` (units.js) ran it, `file` is its test file relative to the unit's
 * folder, `path` its titles. Null for a runner this analyzer does not run.
 */
export function testCommand(unit, file, path, leaf) {
  const pattern = shellQuote(testPattern(unit.runner, path, leaf));
  const quoted = shellQuote(file);
  switch (unit.runner) {
    case "vitest":
      return inFolder(unit, `npx vitest run ${quoted} -t ${pattern}`);
    case "jest":
      return inFolder(unit, `npx jest ${quoted} -t ${pattern}`);
    case "mocha":
      return inFolder(unit, `npx mocha ${quoted} --grep ${pattern}`);
    case "node":
      return inFolder(unit, ["node", ...unit.flags.map(shellWord), "--test", `--test-name-pattern=${pattern}`, quoted].join(" "));
    default:
      return null;
  }
}

/** The command that runs the tests of `files` (relative to the unit's folder) again, as `unit` ran them. */
export function packageCommand(unit, files) {
  if (files.length === 0) return null;
  const quoted = files.map(shellQuote).join(" ");
  switch (unit.runner) {
    case "vitest":
      return inFolder(unit, `npx vitest run ${quoted}`);
    case "jest":
      return inFolder(unit, `npx jest ${quoted}`);
    case "mocha":
      return inFolder(unit, `npx mocha ${quoted}`);
    case "node":
      return inFolder(unit, ["node", ...unit.flags.map(shellWord), "--test", quoted].join(" "));
    default:
      return null;
  }
}
