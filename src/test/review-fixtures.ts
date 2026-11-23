import type { ReviewComment, ReviewState } from "@/lib/api";

/** Nothing recorded yet: no marks, no comments, no last review. */
export function makeReviewState(overrides: Partial<ReviewState> = {}): ReviewState {
  return {
    files: [],
    functions: [],
    comments: [],
    checkpoint: null,
    since: { kind: "unavailable", reason: "no_checkpoint" },
    summary: {
      totalFiles: 0,
      reviewedFiles: 0,
      reopenedFiles: 0,
      reopenedFunctions: 0,
      filesChangedSince: 0,
      outdatedComments: 0,
      unpublished: 0,
      staleDrafts: 0,
    },
    signalDrafts: [],
    ...overrides,
  };
}

/** `publishError` may be given as its English text: it becomes a remote_failed error without a reason. */
export function makeComment(
  id: string,
  overrides: Partial<Omit<ReviewComment, "publishError">> & {
    publishError?: ReviewComment["publishError"] | string;
  } = {},
): ReviewComment {
  const { publishError = null, ...rest } = overrides;
  return {
    id,
    kind: "line",
    parentId: null,
    path: "go.mod",
    side: "new",
    line: 1,
    lineText: "new go.mod",
    commitSha: "e7f3b10c9d8e7f6a",
    body: `Comment ${id}`,
    state: "draft",
    remoteId: null,
    remoteAuthorId: null,
    remoteAuthorName: null,
    resolved: false,
    createdAt: "2026-11-19T10:00:00Z",
    updatedAt: "2026-11-19T10:00:00Z",
    currentLine: 1,
    outdated: false,
    mine: true,
    pendingChange: null,
    ...rest,
    publishError:
      typeof publishError === "string" ? { code: "remote_failed", message: publishError } : publishError,
  };
}

/** A comment on the pull request as a whole: on no file or line. */
export function makeGeneralComment(id: string, overrides: Parameters<typeof makeComment>[1] = {}): ReviewComment {
  return makeComment(id, { kind: "general", path: "", line: 0, lineText: "", currentLine: null, ...overrides });
}

/** A comment someone else wrote on the pull request. */
export function makeTheirComment(id: string, overrides: Partial<ReviewComment> = {}): ReviewComment {
  return makeComment(id, {
    state: "published",
    remoteId: `r-${id}`,
    remoteAuthorId: "{b2}",
    remoteAuthorName: "Bruno",
    mine: false,
    ...overrides,
  });
}
