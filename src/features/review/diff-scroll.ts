/** A line to bring into view when a diff opens, such as the first line of a function picked in the Flow tab. */
export interface ScrollTarget {
  /** 1-based line number on `side`. */
  line: number;
  /** "old" for a line that exists only on the old side, such as a deleted function's. */
  side: "old" | "new";
}

/** The part of a CodeMirror merge chunk this needs: its range on the old (A) and new (B) side. */
interface ChunkRange {
  fromA: number;
  toA: number;
  fromB: number;
  toB: number;
}

/**
 * Where an old-side position shows in a unified diff, whose document is the new side: inside a changed
 * chunk it is the start of the chunk (where its removed lines are drawn), elsewhere it is shifted by
 * what the chunks before it added and removed. `chunks` are in document order.
 */
export function oldToNew(chunks: readonly ChunkRange[], position: number): number {
  let shift = 0;
  for (const chunk of chunks) {
    if (position < chunk.fromA) break;
    if (position < chunk.toA) return chunk.fromB;
    shift = chunk.toB - chunk.toA;
  }
  return position + shift;
}

/** Clamps a 1-based line number into a document of `lines` lines. */
export const clampLine = (line: number, lines: number) => Math.min(Math.max(1, Math.round(line)), Math.max(1, lines));
