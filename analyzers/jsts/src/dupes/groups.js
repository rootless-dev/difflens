// Steps 7 to 10 of the duplicate search (dupes.schema.json): which side of a
// maximal pair is new, the blocks the new sides make, and the groups printed,
// for the pairs of one scope (step 2; index.js adds the scope and puts the
// production groups before the test groups).
// Pure functions of line ranges, so the rules can be tested on their own.
//
// A side is `{ file, startLine, endLine }` (file relative to the tree, lines
// 1-based and inclusive); a pair here is `{ sides: [side, side], fresh:
// [bool, bool], tokens }`, `fresh` saying which sides are new.

/** Groups printed at most per scope (step 10). */
export const MAX_GROUPS = 500;

/** Whether a side spanning `startLine`..`endLine` is new: at least half of its lines are added (step 7). */
export function isNewSide(startLine, endLine, addedLines) {
  let added = 0;
  for (let line = startLine; line <= endLine; line++) if (addedLines[line] === 1) added++;
  return 2 * added >= endLine - startLine + 1;
}

/** Orders ranges by file, then start line, then end line. */
export function compareRanges(x, y) {
  if (x.file !== y.file) return x.file < y.file ? -1 : 1;
  return x.startLine - y.startLine || x.endLine - y.endLine;
}

/** `ranges` with those of a file whose lines overlap merged into one (the union of their lines), sorted. */
export function mergeRanges(ranges) {
  const sorted = ranges.map((r) => ({ file: r.file, startLine: r.startLine, endLine: r.endLine })).sort(compareRanges);
  const out = [];
  for (const range of sorted) {
    const last = out.at(-1);
    if (last !== undefined && last.file === range.file && range.startLine <= last.endLine) {
      last.endLine = Math.max(last.endLine, range.endLine);
    } else {
      out.push(range);
    }
  }
  return out;
}

/** The block of `blocks` (merged, sorted) holding `side`. */
function blockOf(blocks, side) {
  let lo = 0;
  let hi = blocks.length - 1;
  while (lo < hi) {
    const mid = (lo + hi + 1) >> 1;
    if (compareRanges(blocks[mid], { file: side.file, startLine: side.startLine, endLine: Infinity }) <= 0) lo = mid;
    else hi = mid - 1;
  }
  return lo;
}

const range = ({ file, startLine, endLine }) => ({ file, startLine, endLine });

/**
 * Steps 8 to 10: the duplicates of `pairs` (maximal pairs with at least one new side), sorted, at most `limit`.
 * Returns `{ duplicates, dropped }`, `dropped` the number of groups past the limit.
 */
export function buildGroups(pairs, limit = MAX_GROUPS) {
  // Step 8: the new sides of a file whose lines overlap make one block.
  const blocks = mergeRanges(pairs.flatMap((pair) => pair.sides.filter((_, k) => pair.fresh[k]))).map((b) => ({
    ...b,
    existing: [],
    existingTokens: 0,
  }));
  // Occurrences: an existing side as it is, a new side as the block holding it.
  const links = [];
  for (const pair of pairs) {
    for (let k = 0; k < 2; k++) {
      if (!pair.fresh[k]) continue;
      const block = blocks[blockOf(blocks, pair.sides[k])];
      const other = pair.sides[1 - k];
      if (pair.fresh[1 - k]) {
        links.push({ x: block, y: blocks[blockOf(blocks, other)], tokens: pair.tokens });
      } else {
        block.existing.push(other);
        block.existingTokens = Math.max(block.existingTokens, pair.tokens);
      }
    }
  }

  // Step 9: a block with an existing occurrence copies it; the others repeated in the change group together.
  const groups = [];
  for (const block of blocks) {
    if (block.existing.length === 0) continue;
    groups.push({
      kind: "copies_existing",
      block: range(block),
      others: mergeRanges(block.existing),
      tokens: block.existingTokens,
      lines: block.endLine - block.startLine + 1,
    });
  }
  const parent = new Map();
  const root = (block) => {
    while (parent.get(block) !== block) {
      parent.set(block, parent.get(parent.get(block)));
      block = parent.get(block);
    }
    return block;
  };
  for (const block of blocks) if (block.existing.length === 0) parent.set(block, block);
  const repeated = links.filter((link) => parent.has(link.x) && parent.has(link.y));
  for (const link of repeated) parent.set(root(link.x), root(link.y));
  const components = new Map();
  for (const block of blocks) {
    if (!parent.has(block)) continue;
    const top = root(block);
    if (!components.has(top)) components.set(top, { members: [], tokens: 0 });
    components.get(top).members.push(block);
  }
  for (const link of repeated) {
    const component = components.get(root(link.x));
    component.tokens = Math.max(component.tokens, link.tokens);
  }
  for (const { members, tokens } of components.values()) {
    // A block whose only occurrences are itself, or blocks copying existing code, has nothing to list.
    if (members.length < 2) continue;
    const [first, ...rest] = members.sort(compareRanges);
    groups.push({
      kind: "repeated_in_change",
      block: range(first),
      others: rest.map(range),
      tokens,
      lines: first.endLine - first.startLine + 1,
    });
  }

  // Step 10, within the scope.
  groups.sort((x, y) => compareRanges(x.block, y.block));
  return { duplicates: groups.slice(0, limit), dropped: Math.max(0, groups.length - limit) };
}
