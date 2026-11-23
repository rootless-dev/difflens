import type { ChangedFile } from "@/lib/api";

/** Above this many changed files the file list gets a filter box. */
export const FILTER_THRESHOLD = 20;

export interface FileGroup {
  /** Directory of the files, "" for the repository root. */
  dir: string;
  files: ChangedFile[];
}

const dirOf = (path: string) => {
  const slash = path.lastIndexOf("/");
  return slash === -1 ? "" : path.slice(0, slash);
};

export const baseName = (path: string) => path.slice(path.lastIndexOf("/") + 1);

/** Compares directories segment by segment, so "a/b" sorts right before "a/b/c" and the root comes first. */
function compareDirs(a: string, b: string): number {
  if (a === b) return 0;
  if (a === "") return -1;
  if (b === "") return 1;
  const left = a.split("/");
  const right = b.split("/");
  for (let i = 0; i < Math.min(left.length, right.length); i++) {
    if (left[i] !== right[i]) return left[i] < right[i] ? -1 : 1;
  }
  return left.length - right.length;
}

/**
 * Groups files by the directory they are in (the new path for a rename), root first and then
 * directories in path order, with files sorted by name inside each group.
 */
export function groupFiles(files: ChangedFile[]): FileGroup[] {
  const byDir = new Map<string, ChangedFile[]>();
  for (const file of files) {
    const dir = dirOf(file.path);
    const group = byDir.get(dir);
    if (group) {
      group.push(file);
    } else {
      byDir.set(dir, [file]);
    }
  }
  return [...byDir.entries()]
    .sort(([a], [b]) => compareDirs(a, b))
    .map(([dir, group]) => ({
      dir,
      files: [...group].sort((a, b) => {
        const left = baseName(a.path);
        const right = baseName(b.path);
        return left === right ? 0 : left < right ? -1 : 1;
      }),
    }));
}

/** Files whose path, or path before a rename, contains the query, ignoring case. */
export function filterFiles(files: ChangedFile[], query: string): ChangedFile[] {
  const needle = query.trim().toLowerCase();
  if (needle === "") return files;
  return files.filter(
    (file) => file.path.toLowerCase().includes(needle) || (file.oldPath?.toLowerCase().includes(needle) ?? false),
  );
}

/** Paths in the order the grouped list shows them, which is the order j/k walks through. */
export const orderedPaths = (groups: FileGroup[]) => groups.flatMap((group) => group.files.map((file) => file.path));

/**
 * The path `step` positions away from `current` in `paths`, staying put at either end.
 * When `current` is not in the list, moving forward lands on the first path and moving back on the last.
 */
export function adjacentPath(paths: string[], current: string | null, step: 1 | -1): string | null {
  if (paths.length === 0) return null;
  const index = current === null ? -1 : paths.indexOf(current);
  if (index === -1) return step === 1 ? paths[0] : paths[paths.length - 1];
  return paths[Math.min(paths.length - 1, Math.max(0, index + step))];
}

/**
 * Name shown in the list: the file name, and for a rename where it came from — the old file name
 * when it stayed in the same directory, the whole old path when it moved.
 */
export function fileLabel(file: ChangedFile): { name: string; renamedFrom: string | null } {
  const name = baseName(file.path);
  if (file.status !== "renamed" || file.oldPath === null) {
    return { name, renamedFrom: null };
  }
  return {
    name,
    renamedFrom: dirOf(file.oldPath) === dirOf(file.path) ? baseName(file.oldPath) : file.oldPath,
  };
}
