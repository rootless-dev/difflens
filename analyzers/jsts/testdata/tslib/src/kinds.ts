// Call kinds and resolutions, one case per function.
import { Circle, Square } from "./shapes";

export function early(flag: boolean): number {
  first();
  if (flag) {
    return 0;
  }
  second();
  return 1;
}

export function loops(items: number[], limit: number): void {
  for (const item of items) {
    first();
    void item;
  }
  while (true) {
    second();
    if (limit-- < 0) break;
    third();
  }
  do {
    first();
  } while (limit > 0);
}

export function branches(value: string | undefined, other?: { run(): void }): string {
  const a = value ?? fallback();
  other?.run();
  try {
    first();
  } catch {
    second();
  } finally {
    third();
  }
  return value ? a : fallback();
}

export function union(shape: Circle | Square): number {
  return shape.area();
}

export async function waits(): Promise<void> {
  await later();
  later();
  later().then(first);
}

function first(): void {}
function second(): void {}
function third(): void {}
function fallback(): string {
  return "x";
}
async function later(): Promise<void> {}
