import { i18n } from "@/i18n";
import type { Folder, TerminalSession, Worktree } from "@/lib/api";

/** The terminal's own shortcuts, written as macOS writes them. */
export const TOGGLE_KEYS = "⌃`";
export const NEW_TERMINAL_KEYS = "⌃⇧`";

/**
 * ⌃` (expand and collapse) or ⌃⇧` (a new terminal). The physical key, so they work on layouts where ` is typed
 * elsewhere, or the character itself.
 */
export function isTerminalShortcut(event: KeyboardEvent): boolean {
  return (
    event.ctrlKey && !event.metaKey && !event.altKey && (event.code === "Backquote" || event.key === "`")
  );
}

/**
 * A folder's display path split in two, so a long one loses the middle: the start (with its ~) ellipsizes, the end
 * always stays whole. The end is a DiffLens worktree's path inside the project, or the folder's own name.
 */
export function splitPath(folder: Folder): [head: string, tail: string] {
  const path = folder.displayPath;
  if (folder.relativePath && path.endsWith(folder.relativePath)) {
    return [path.slice(0, path.length - folder.relativePath.length), folder.relativePath];
  }
  const slash = path.lastIndexOf("/");
  if (slash <= 0 || slash === path.length - 1) return ["", path];
  return [path.slice(0, slash + 1), path.slice(slash + 1)];
}

/** "go test" of "go test ./...": the program and its first argument, as a tab shows what runs in it. */
export const shortCommand = (command: string) => command.split(/\s+/).slice(0, 2).join(" ");

/** The tab's accessible name: its session, what runs in it, and whether it printed something not seen yet. */
export function tabLabel(session: TerminalSession, unseen: boolean): string {
  let label = session.name;
  if (session.ended) label = i18n.t("terminal.tab.ended", { name: session.name });
  else if (session.kind === "shell" && session.foreground !== null) {
    label = session.foreground
      ? i18n.t("terminal.tab.running", { name: session.name, command: session.foreground })
      : i18n.t("terminal.tab.runningSomething", { name: session.name });
  }
  return unseen ? i18n.t("terminal.tab.newOutput", { label }) : label;
}

/** A live Claude Code session, or a shell running something in the foreground: closing either interrupts work. */
export const closingInterrupts = (session: TerminalSession) =>
  !session.ended && (session.kind === "claude" || session.foreground !== null);

/** The last part of a path: where a session runs when it has no branch to name. */
export const folderName = (path: string) => path.split("/").filter(Boolean).pop() ?? path;

/** Where a branch's terminal opens, as the ⋮ menu says under its items: a worktree's path, or a sentence. */
export function whereBranchOpens(branch: string, worktrees: Worktree[]): { text: string; path: boolean } {
  const worktree = worktrees.find((candidate) => candidate.branch === branch);
  if (!worktree) return { text: i18n.t("project.actions.createsWorktree"), path: false };
  if (worktree.folder.kind === "project") return { text: i18n.t("project.actions.inProjectFolder"), path: false };
  return { text: worktree.folder.relativePath ?? worktree.folder.displayPath, path: true };
}
