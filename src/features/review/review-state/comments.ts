// Comments as the Diff tab shows them: threads (a first comment and its replies), placed under lines of one file.

import { i18n } from "@/i18n";
import type { ReviewComment } from "@/lib/api";

export type CommentSide = ReviewComment["side"];

export interface Thread {
  root: ReviewComment;
  /** Oldest first. */
  replies: ReviewComment[];
}

/** Identity of a line of the diff, the key its notes are drawn under. */
export const lineKey = (side: CommentSide, line: number) => `${side}:${line}`;

export function parseLineKey(key: string): { side: CommentSide; line: number } {
  const [side, line] = key.split(":");
  return { side: side === "old" ? "old" : "new", line: Number(line) };
}

/** Threads in the order their first comments were written; a reply whose thread is gone is dropped. */
export function threadsOf(comments: readonly ReviewComment[]): Thread[] {
  const threads = new Map<string, Thread>();
  for (const comment of comments) {
    if (comment.parentId === null) threads.set(comment.id, { root: comment, replies: [] });
  }
  for (const comment of comments) {
    if (comment.parentId !== null) threads.get(comment.parentId)?.replies.push(comment);
  }
  return [...threads.values()];
}

/** The threads of one file that can be placed, by the line they sit under now, on the sides shown. */
export function threadsByLine(
  threads: readonly Thread[],
  path: string,
  sides: ReadonlySet<CommentSide>,
): ReadonlyMap<string, Thread[]> {
  const byLine = new Map<string, Thread[]>();
  for (const thread of threads) {
    const { root } = thread;
    if (root.kind !== "line" || root.path !== path || root.currentLine === null || !sides.has(root.side)) continue;
    const key = lineKey(root.side, root.currentLine);
    byLine.set(key, [...(byLine.get(key) ?? []), thread]);
  }
  return byLine;
}

/** The threads on the pull request as a whole (general comments), in the order they were started. */
export const generalThreads = (threads: readonly Thread[]): Thread[] =>
  threads.filter((thread) => thread.root.kind === "general");

/** A thread that still asks for attention: not resolved, and not deleted waiting for the next publish. */
export const isOpen = (thread: Thread) => !thread.root.resolved && thread.root.pendingChange !== "delete";

/**
 * The line threads, by file: the files in `order` first (the order of the review's file list), then any other file
 * by path; in a file, by the line they sit under (or were written on, when they can no longer be placed).
 */
export function lineThreadsByFile(
  threads: readonly Thread[],
  order: readonly string[],
): { path: string; threads: Thread[] }[] {
  const byPath = new Map<string, Thread[]>();
  for (const thread of threads) {
    if (thread.root.kind !== "line") continue;
    byPath.set(thread.root.path, [...(byPath.get(thread.root.path) ?? []), thread]);
  }
  const rank = new Map(order.map((path, index) => [path, index]));
  const paths = [...byPath.keys()].sort((left, right) => {
    const a = rank.get(left) ?? Number.MAX_SAFE_INTEGER;
    const b = rank.get(right) ?? Number.MAX_SAFE_INTEGER;
    return a !== b ? a - b : left.localeCompare(right);
  });
  const lineOf = (thread: Thread) => thread.root.currentLine ?? thread.root.line;
  return paths.map((path) => ({
    path,
    threads: [...(byPath.get(path) ?? [])].sort((left, right) => lineOf(left) - lineOf(right)),
  }));
}

/** The thread a comment (its first one or a reply) is in. */
export const threadOf = (threads: readonly Thread[], commentId: string): Thread | null =>
  threads.find((thread) => thread.root.id === commentId || thread.replies.some((reply) => reply.id === commentId)) ??
  null;

/** Written by the user, here or on the pull request by the connected account, so it can be edited and deleted here. */
export const isOwn = (comment: ReviewComment) => comment.mine;

/** Who wrote a comment, as the screen names them in the language shown: "You", or the author's name on the host. */
export const authorOf = (comment: ReviewComment) =>
  isOwn(comment) ? i18n.t("review.comments.you") : (comment.remoteAuthorName ?? i18n.t("review.comments.someoneElse"));

/** What is being written: a comment on a line or on the pull request as a whole, a reply in a thread, or a new text for a comment. */
export type Draft =
  | { kind: "new"; side: CommentSide; line: number; text: string }
  | { kind: "general"; text: string }
  | { kind: "reply"; rootId: string; text: string }
  | { kind: "edit"; commentId: string; text: string };
