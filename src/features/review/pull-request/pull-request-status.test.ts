import { describe, expect, it } from "vitest";

import {
  barNote,
  outcomeText,
  publishControl,
  type PullRequestStatus,
} from "@/features/review/pull-request/pull-request-status";
import { makeLinked } from "@/test/pull-request-fixtures";
import { makeReviewState } from "@/test/review-fixtures";

const summary = (unpublished: number, staleDrafts = 0) => ({
  ...makeReviewState().summary,
  unpublished,
  staleDrafts,
});

describe("the review bar's pull request note", () => {
  it("says why there is no pull request, and nothing when there is one", () => {
    const notes: [PullRequestStatus, string | null][] = [
      [{ kind: "loading" }, null],
      [{ kind: "working_tree" }, null],
      [makeLinked(), null],
      [{ kind: "no_remote" }, "No pull request: origin is not on a server"],
      [{ kind: "unsupported_host", host: "github.com" }, "No pull request integration for github.com yet"],
      [{ kind: "not_connected", hostName: "Bitbucket Cloud" }, "Connect Bitbucket Cloud to see the pull request"],
      [
        { kind: "no_pull_request", hostName: "Bitbucket Cloud", branch: "feat/x" },
        "No open pull request for this branch",
      ],
      [{ kind: "error", message: "Could not reach Bitbucket: it did not answer in time." }, "Could not reach Bitbucket: it did not answer in time."],
    ];
    for (const [status, note] of notes) expect(barNote(status)).toBe(note);
  });
});

describe("Publish comments", () => {
  it("is not offered for uncommitted changes", () => {
    expect(publishControl({ kind: "working_tree" }, summary(2), false).visible).toBe(false);
  });

  it("counts what it would send and is enabled when the review matches the pull request", () => {
    expect(publishControl(makeLinked(), summary(3), false)).toEqual({
      visible: true,
      enabled: true,
      label: "Publish comments (3)",
      note: null,
    });
  });

  it("says why it cannot send", () => {
    const reasons: [PullRequestStatus, string][] = [
      [{ kind: "loading" }, "Looking for the pull request"],
      [{ kind: "no_remote" }, "comments stay in DiffLens"],
      [{ kind: "unsupported_host", host: "gitlab.com" }, "No pull request integration for gitlab.com yet"],
      [{ kind: "not_connected", hostName: "Bitbucket Cloud" }, "Connect Bitbucket Cloud in Preferences"],
      [{ kind: "no_pull_request", hostName: "Bitbucket Cloud", branch: "feat/x" }, "No open pull request for feat/x"],
      [{ kind: "error", message: "Bitbucket is limiting requests." }, "could not be read: Bitbucket is limiting requests."],
      [
        makeLinked("Your local branch has commits the pull request doesn't have yet: push first, then Reload."),
        "push first, then Reload",
      ],
    ];
    for (const [status, reason] of reasons) {
      const control = publishControl(status, summary(2), false);
      expect(control.enabled).toBe(false);
      expect(control.note).toContain(reason);
    }
  });

  it("has nothing to send, or holds back drafts whose line changed", () => {
    expect(publishControl(makeLinked(), summary(0), false)).toMatchObject({ enabled: false, note: "Nothing to publish" });
    expect(publishControl(makeLinked(), summary(0, 2), false)).toMatchObject({
      enabled: false,
      note: "2 drafts held back: code changed since they were written",
    });
    expect(publishControl(makeLinked(), summary(1, 1), false)).toMatchObject({
      enabled: true,
      note: "1 draft held back: code changed since it was written",
    });
  });

  it("waits while publishing", () => {
    expect(publishControl(makeLinked(), summary(2), true)).toMatchObject({ enabled: false, label: "Publishing…" });
  });

  it("reports what was published and which changes failed", () => {
    expect(outcomeText({ kind: "report", report: { published: 3, failed: [], stale: 0 } })).toBe("Published 3 changes.");
    expect(
      outcomeText({
        kind: "report",
        report: {
          published: 1,
          failed: [{ commentId: "c", path: "a.go", line: 8, message: "Bitbucket refused this: bad anchor" }],
          stale: 0,
        },
      }),
    ).toBe("Published 1 change. 1 change could not be published and stays here. a.go:8: Bitbucket refused this: bad anchor");
    expect(outcomeText({ kind: "error", message: "Connect Bitbucket Cloud in Preferences first." })).toBe(
      "Connect Bitbucket Cloud in Preferences first.",
    );
  });
});
