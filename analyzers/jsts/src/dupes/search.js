// Steps 4 to 6 of the duplicate search (dupes.schema.json): the windows of
// --min-tokens tokens and their rolling hashes, the candidate groups of
// windows sharing a hash with one that touches the change, and the maximal
// pairs grown from them. The files given are those of one scope (step 2).
//
// A file here is `{ ids, startLines, endLines, splits, touched }`: the typed
// arrays of tokenize() and `touched`, a prefix count of the tokens that start
// on an added line (touched[i] is how many of the first i tokens do).

/** Candidate groups larger than this are boilerplate and dropped (step 5). */
export const MAX_GROUP = 64;

const BASE_A = 0x01000193;
const BASE_B = 0x5bd1e995;
const SCRAMBLE = 0x9e3779b1 | 0;
/** 2^21: a window's key is its first lane times this plus 21 bits of its second, exact in a double. */
const LANE = 2097152;

/** `touched` for tokens whose start lines are `startLines`, given the file's added lines (a flag per line). */
export function touchedPrefix(startLines, addedLines) {
  const touched = new Int32Array(startLines.length + 1);
  for (let i = 0; i < startLines.length; i++) touched[i + 1] = touched[i] + (addedLines[startLines[i]] === 1 ? 1 : 0);
  return touched;
}

/** Whether the window of `size` tokens at `pos` touches the change: one of its tokens starts on an added line. */
export function touches(file, pos, size) {
  return file.touched[pos + size] - file.touched[pos] > 0;
}

/**
 * The hash key of every window of `size` tokens of a file (index: the window's first token), -1 for a window
 * that crosses a split. Two lanes of a polynomial rolling hash modulo 2^32; equal windows have equal keys, and
 * unequal ones rarely do (pairs are verified token by token).
 */
export function windowKeys(ids, splits, size) {
  const count = ids.length - size + 1;
  if (count <= 0) return new Float64Array(0);
  const keys = new Float64Array(count);
  let powA = 1;
  let powB = 1;
  for (let k = 1; k < size; k++) {
    powA = Math.imul(powA, BASE_A);
    powB = Math.imul(powB, BASE_B);
  }
  const value = (i) => Math.imul(ids[i] + 1, SCRAMBLE);
  let a = 0;
  let b = 0;
  for (let k = 0; k < size; k++) {
    a = (Math.imul(a, BASE_A) + value(k)) | 0;
    b = (Math.imul(b, BASE_B) + value(k)) | 0;
  }
  for (let i = 0; ; i++) {
    keys[i] = splits[i] === splits[i + size - 1] ? (a >>> 0) * LANE + ((b >>> 0) & (LANE - 1)) : -1;
    if (i + 1 === count) break;
    const out = value(i);
    const into = value(i + size);
    a = (Math.imul((a - Math.imul(out, powA)) | 0, BASE_A) + into) | 0;
    b = (Math.imul((b - Math.imul(out, powB)) | 0, BASE_B) + into) | 0;
  }
  return keys;
}

/**
 * Step 5: the windows sharing the hash of a window that touches the change, grouped by hash. `keys[f]` are the
 * window keys of `files[f]`. Returns groups of windows, each `{ files: Int32Array, positions: Int32Array }` in
 * file then position order; a group of one, or of more than `maxGroup`, is dropped.
 */
export function candidateGroups(files, keys, size, maxGroup = MAX_GROUP) {
  // First pass: the hashes of the windows that touch the change.
  const wanted = new Set();
  for (let f = 0; f < files.length; f++) {
    const file = files[f];
    if (file.touched[file.touched.length - 1] === 0) continue;
    const fileKeys = keys[f];
    for (let pos = 0; pos < fileKeys.length; pos++) {
      if (fileKeys[pos] >= 0 && touches(file, pos, size)) wanted.add(fileKeys[pos]);
    }
  }
  if (wanted.size === 0) return [];
  // Second pass: every window of every file with one of those hashes.
  const found = { key: [], file: [], pos: [] };
  for (let f = 0; f < files.length; f++) {
    const fileKeys = keys[f];
    for (let pos = 0; pos < fileKeys.length; pos++) {
      if (wanted.has(fileKeys[pos])) {
        found.key.push(fileKeys[pos]);
        found.file.push(f);
        found.pos.push(pos);
      }
    }
  }
  const order = Uint32Array.from(found.key, (_, i) => i);
  order.sort((x, y) => found.key[x] - found.key[y] || x - y);
  const groups = [];
  for (let start = 0; start < order.length; ) {
    let end = start + 1;
    while (end < order.length && found.key[order[end]] === found.key[order[start]]) end++;
    const length = end - start;
    if (length >= 2 && length <= maxGroup) {
      const members = order.subarray(start, end);
      groups.push({
        files: Int32Array.from(members, (i) => found.file[i]),
        positions: Int32Array.from(members, (i) => found.pos[i]),
      });
    }
    start = end;
  }
  return groups;
}

/** Whether the `size` tokens at `p` of `a` and at `q` of `b` are equal (a hash can collide). */
function sameTokens(a, p, b, q, size) {
  for (let k = 0; k < size; k++) if (a.ids[p + k] !== b.ids[q + k]) return false;
  return true;
}

/**
 * Grows the equal windows at `p` of `a` and `q` of `b` backward, then forward, while their tokens are equal,
 * within the splits of both, and, in one file (`p` < `q`), without the two ranges overlapping. Returns
 * `{ p, q, length }`: the first token of each side and the number of tokens.
 */
export function extend(a, p, b, q, size) {
  const limit = a === b ? q - p : Infinity;
  const splitA = a.splits[p];
  const splitB = b.splits[q];
  let length = size;
  while (
    p > 0 &&
    q > 0 &&
    length < limit &&
    a.ids[p - 1] === b.ids[q - 1] &&
    a.splits[p - 1] === splitA &&
    b.splits[q - 1] === splitB
  ) {
    p--;
    q--;
    length++;
  }
  while (
    p + length < a.ids.length &&
    q + length < b.ids.length &&
    length < limit &&
    a.ids[p + length] === b.ids[q + length] &&
    a.splits[p + length] === splitA &&
    b.splits[q + length] === splitB
  ) {
    length++;
  }
  return { p, q, length };
}

/**
 * Step 6: the maximal pairs of the candidate groups, each `{ a, p, b, q, length }` (file indexes, first tokens,
 * tokens), `a` before `b` in file then position order, and each kept once. Two windows pair when one of them
 * touches the change and they do not overlap. A window inside a maximal pair already found on the same diagonal
 * (the same two files at the same offset) would grow into that pair again, so it is skipped: a long clone is
 * extended once, not once per window. Stops at `deadline` and sets `timedOut` on the result.
 */
export function maximalPairs(files, groups, size, deadline = Infinity) {
  const pairs = [];
  const seen = new Set();
  // Per pair of files, per offset: the token ranges of side a already covered by a maximal pair.
  const covered = new Map();
  let timedOut = false;
  let steps = 0;
  outer: for (const group of groups) {
    const n = group.files.length;
    for (let i = 0; i < n; i++) {
      const fa = group.files[i];
      const p = group.positions[i];
      const a = files[fa];
      const touchesA = touches(a, p, size);
      for (let j = i + 1; j < n; j++) {
        if ((++steps & 1023) === 0 && Date.now() > deadline) {
          timedOut = true;
          break outer;
        }
        const fb = group.files[j];
        const q = group.positions[j];
        const b = files[fb];
        if (!touchesA && !touches(b, q, size)) continue;
        if (fa === fb && q - p < size) continue;
        const diagonalKey = fa * files.length + fb;
        let diagonals = covered.get(diagonalKey);
        if (diagonals === undefined) {
          diagonals = new Map();
          covered.set(diagonalKey, diagonals);
        }
        const offset = q - p;
        let ranges = diagonals.get(offset);
        if (ranges !== undefined && ranges.some((r) => r[0] <= p && p + size <= r[1])) continue;
        if (!sameTokens(a, p, b, q, size)) continue;
        const grown = extend(a, p, b, q, size);
        if (ranges === undefined) {
          ranges = [];
          diagonals.set(offset, ranges);
        }
        ranges.push([grown.p, grown.p + grown.length]);
        const key = `${fa}:${grown.p}:${fb}:${grown.q}:${grown.length}`;
        if (seen.has(key)) continue;
        seen.add(key);
        pairs.push({ a: fa, p: grown.p, b: fb, q: grown.q, length: grown.length });
      }
    }
  }
  return { pairs, timedOut };
}
