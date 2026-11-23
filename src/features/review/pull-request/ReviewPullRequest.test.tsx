import { openUrl } from "@tauri-apps/plugin-opener";
import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  addGeneralComment,
  deleteComment,
  getFileVersions,
  getReviewState,
  getTestsState,
  linkPullRequest,
  listChangedFiles,
  publishComments,
  replyToComment,
  restoreComment,
  type ReviewTarget,
  setCommentResolved,
  setPullRequestVerdict,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeLinked, ME } from "@/test/pull-request-fixtures";
import { makeComment, makeGeneralComment, makeReviewState, makeTheirComment } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab, reviewTab } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  addGeneralComment: vi.fn(),
  replyToComment: vi.fn(),
  deleteComment: vi.fn(),
  restoreComment: vi.fn(),
  setCommentResolved: vi.fn(),
  linkPullRequest: vi.fn(),
  publishComments: vi.fn(),
  setPullRequestVerdict: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn() }));

// The editor needs layout APIs jsdom lacks; the stub draws the blocks under lines in a list.
vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div data-testid="diff-editor">
      {(props.annotations ?? []).map((annotation) => (
        <div key={annotation.key} data-testid={`under-${annotation.key}`}>
          {props.renderAnnotation?.(annotation.key)}
        </div>
      ))}
    </div>
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const changeSet = makeChangeSet([makeChangedFile("go.mod", { language: null })]);
const REF = { target: changeSet.target, oldSha: changeSet.oldSha, newSha: changeSet.newSha };

let bar: HTMLDivElement;

function renderScreen(target: ReviewTarget = BRANCH, onOpenPreferences = vi.fn()) {
  render(<ReviewScreen projectId="p-1" target={target} barSlot={bar} onOpenPreferences={onOpenPreferences} />);
  return { onOpenPreferences };
}

/** Opens the review on its Diff tab, where the comments are written and published. */
async function openDiff(user = userEvent.setup(), target: ReviewTarget = BRANCH) {
  renderScreen(target);
  await openReviewTab(user, "Diff");
  return user;
}

const inBar = () => within(bar);
const publishButton = () => screen.findByRole("button", { name: /^Publish comments|^Publishing/ });
const under = (key: string) => within(screen.getByTestId(`under-${key}`));

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(listChangedFiles).mockImplementation(async (_id, target) => ({ ...changeSet, target }));
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("old\n"), new: textSide("new\n") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue(makeLinked());
  vi.mocked(openUrl).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("the pull request in the review bar", () => {
  it("shows the pull request with its title and state, and opens it in the browser", async () => {
    const user = userEvent.setup();
    renderScreen();

    const link = await inBar().findByRole("link", { name: /PR #318/ });
    expect(inBar().getByText("Adds the refund flow")).toBeInTheDocument();
    expect(inBar().getByText("Open")).toBeInTheDocument();
    expect(linkPullRequest).toHaveBeenCalledWith("p-1", REF);

    await user.click(link);
    expect(openUrl).toHaveBeenCalledWith("https://bitbucket.org/acme/payments-api/pull-requests/318");
  });

  it("says why there is none, and offers to connect", async () => {
    const user = userEvent.setup();
    vi.mocked(linkPullRequest).mockResolvedValue({ kind: "not_connected", hostName: "Bitbucket Cloud" });
    const { onOpenPreferences } = renderScreen();

    expect(await inBar().findByText("Connect Bitbucket Cloud to see the pull request")).toBeInTheDocument();
    await user.click(inBar().getByRole("button", { name: "Connect" }));
    expect(onOpenPreferences).toHaveBeenCalledOnce();
  });

  it("stays calm about hosts it has no integration for, and about errors", async () => {
    vi.mocked(linkPullRequest).mockResolvedValue({ kind: "unsupported_host", host: "github.com" });
    await openDiff();
    expect(await inBar().findByText("No pull request integration for github.com yet")).toBeInTheDocument();
    expect(await screen.findByTestId("publish-note")).toHaveTextContent("comments stay in DiffLens");
  });

  it("shows a failure to read the pull request as its message", async () => {
    vi.mocked(linkPullRequest).mockRejectedValue(
      apiError("auth_failed", "Bitbucket did not accept the email and API token."),
    );
    renderScreen();
    expect(await inBar().findByText("Bitbucket did not accept the email and API token.")).toBeInTheDocument();
    // The review itself goes on.
    expect(await screen.findByTestId("diff-editor")).toBeInTheDocument();
  });

  it("is not looked for in a review of uncommitted changes", async () => {
    await openDiff(userEvent.setup(), { kind: "working_tree" });
    await screen.findByTestId("diff-editor");
    expect(linkPullRequest).not.toHaveBeenCalled();
    expect(screen.queryByRole("button", { name: /Publish comments/ })).not.toBeInTheDocument();
  });
});

describe("Publish comments", () => {
  const withDrafts = (unpublished: number, staleDrafts = 0) =>
    makeReviewState({
      comments: [makeComment("c-1")],
      summary: { ...makeReviewState().summary, unpublished, staleDrafts },
    });

  it("is disabled with the reason while the review does not match the pull request", async () => {
    vi.mocked(getReviewState).mockResolvedValue(withDrafts(2));
    vi.mocked(linkPullRequest).mockResolvedValue(
      makeLinked("Your local branch has commits the pull request doesn't have yet: push first, then Reload."),
    );
    await openDiff();

    const reason = "Your local branch has commits the pull request doesn't have yet: push first, then Reload.";
    // Said by the Publish button, and by the verdict buttons in the bar, which follow the same rule.
    expect(await screen.findAllByText(reason)).toHaveLength(2);
    expect(screen.getByTestId("publish-note")).toHaveTextContent(reason);
    expect(await publishButton()).toBeDisabled();
  });

  it("sends the drafts on a click, then reads the pull request again and says what happened", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(withDrafts(2, 1));
    const sent = deferred<Awaited<ReturnType<typeof publishComments>>>();
    vi.mocked(publishComments).mockReturnValue(sent.promise);
    await openDiff(user);

    const button = await screen.findByRole("button", { name: "Publish comments (2)" });
    expect(screen.getByTestId("publish-note")).toHaveTextContent("1 draft held back: code changed since it was written");
    // Nothing is sent until the click.
    expect(publishComments).not.toHaveBeenCalled();
    await user.click(button);
    expect(publishComments).toHaveBeenCalledExactlyOnceWith("p-1", REF);
    expect(await screen.findByRole("button", { name: "Publishing…" })).toBeDisabled();

    vi.mocked(getReviewState).mockResolvedValue(withDrafts(1, 1));
    sent.resolve({
      published: 1,
      failed: [{ commentId: "c-2", path: "go.mod", line: 1, message: "Bitbucket refused this: bad anchor" }],
      stale: 1,
    });

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Published 1 change. 1 change could not be published and stays here. go.mod:1: Bitbucket refused this: bad anchor",
    );
    expect(linkPullRequest).toHaveBeenCalledTimes(2);
    expect(await screen.findByRole("button", { name: "Publish comments (1)" })).toBeEnabled();
  });

  it("shows a publish refused as a whole", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(withDrafts(1));
    vi.mocked(publishComments).mockRejectedValue(
      apiError("rate_limited", "Bitbucket is limiting requests from this account. Try again in 30 seconds."),
    );
    await openDiff(user);

    await user.click(await screen.findByRole("button", { name: "Publish comments (1)" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Try again in 30 seconds.");
  });
});

describe("approving and requesting changes", () => {
  const reviewed = (verdict: "approved" | "changes_requested" | null) =>
    makeLinked(null, {
      participants: [
        { account: ME, reviewer: true, verdict },
        { account: { id: "{b2}", name: "Bruno" }, reviewer: true, verdict: "approved" },
      ],
    });
  const withUnpublished = (unpublished: number) =>
    makeReviewState({ summary: { ...makeReviewState().summary, unpublished } });

  it("approves on a click, shows it from the pull request, and takes it back on the next", async () => {
    const user = userEvent.setup();
    vi.mocked(linkPullRequest).mockResolvedValue(reviewed(null));
    vi.mocked(setPullRequestVerdict).mockResolvedValueOnce(reviewed("approved"));
    vi.mocked(setPullRequestVerdict).mockResolvedValueOnce(reviewed(null));
    renderScreen();

    const approve = await inBar().findByRole("button", { name: "Approve" });
    expect(approve).toHaveAttribute("aria-pressed", "false");
    // Never on its own.
    expect(setPullRequestVerdict).not.toHaveBeenCalled();
    await user.click(approve);
    expect(setPullRequestVerdict).toHaveBeenCalledWith("p-1", REF, "approved", true);

    const approved = await inBar().findByRole("button", { name: "✓ Approved" });
    expect(approved).toHaveAttribute("aria-pressed", "true");
    await user.click(approved);
    expect(setPullRequestVerdict).toHaveBeenLastCalledWith("p-1", REF, "approved", false);
    expect(await inBar().findByRole("button", { name: "Approve" })).toBeInTheDocument();
  });

  it("shows changes requested and takes the request back", async () => {
    const user = userEvent.setup();
    vi.mocked(linkPullRequest).mockResolvedValue(reviewed("changes_requested"));
    vi.mocked(setPullRequestVerdict).mockResolvedValue(reviewed(null));
    renderScreen();

    await user.click(await inBar().findByRole("button", { name: "✕ Changes requested" }));
    expect(setPullRequestVerdict).toHaveBeenCalledWith("p-1", REF, "changes_requested", false);
    expect(await inBar().findByRole("button", { name: "Request changes" })).toBeInTheDocument();
  });

  it("is disabled with the reason while the pull request is not at the reviewed commit", async () => {
    vi.mocked(linkPullRequest).mockResolvedValue(
      makeLinked(null, {}, "The pull request has commits your local branch doesn't have: pull them, then Reload."),
    );
    renderScreen();

    expect(await inBar().findByRole("button", { name: "Approve" })).toBeDisabled();
    expect(inBar().getByRole("button", { name: "Request changes" })).toBeDisabled();
    expect(inBar().getByTestId("verdict-note")).toHaveTextContent("pull them, then Reload.");
  });

  it("shows a refused verdict as its message, such as a missing scope", async () => {
    const user = userEvent.setup();
    vi.mocked(setPullRequestVerdict).mockRejectedValue(
      apiError(
        "missing_scope",
        "Bitbucket refused this. The API token may be missing a scope: this needs write:pullrequest:bitbucket.",
      ),
    );
    renderScreen();

    await user.click(await inBar().findByRole("button", { name: "Approve" }));
    expect(await inBar().findByRole("alert")).toHaveTextContent("write:pullrequest:bitbucket");
  });

  it("with unpublished comments, asks whether to publish them first", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(withUnpublished(2));
    vi.mocked(publishComments).mockResolvedValue({ published: 2, failed: [], stale: 0 });
    vi.mocked(setPullRequestVerdict).mockResolvedValue(reviewed("approved"));
    renderScreen();

    // Cancel sends nothing.
    await user.click(await inBar().findByRole("button", { name: "Approve" }));
    let dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("You have 2 unpublished comments")).toBeInTheDocument();
    await user.click(dialog.getByRole("button", { name: "Cancel" }));
    expect(setPullRequestVerdict).not.toHaveBeenCalled();
    expect(publishComments).not.toHaveBeenCalled();

    // Publishing first, then approving.
    await user.click(inBar().getByRole("button", { name: "Approve" }));
    dialog = within(await screen.findByRole("alertdialog"));
    await user.click(dialog.getByRole("button", { name: "Publish 2 comments and approve" }));
    expect(publishComments).toHaveBeenCalledExactlyOnceWith("p-1", REF);
    expect(await inBar().findByRole("button", { name: "✓ Approved" })).toBeInTheDocument();
    expect(setPullRequestVerdict).toHaveBeenCalledExactlyOnceWith("p-1", REF, "approved", true);
  });

  it("can give the verdict without the unpublished comments, and holds it back when publishing fails", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(withUnpublished(1));
    vi.mocked(publishComments).mockResolvedValue({
      published: 0,
      failed: [{ commentId: "c", path: "go.mod", line: 1, message: "Bitbucket refused this: bad anchor" }],
      stale: 0,
    });
    vi.mocked(setPullRequestVerdict).mockResolvedValue(reviewed("changes_requested"));
    renderScreen();

    await user.click(await inBar().findByRole("button", { name: "Request changes" }));
    let dialog = within(await screen.findByRole("alertdialog"));
    await user.click(dialog.getByRole("button", { name: "Publish 1 comment and request changes" }));
    expect(await screen.findByText(/could not be published/)).toBeInTheDocument();
    expect(setPullRequestVerdict).not.toHaveBeenCalled();

    await user.click(inBar().getByRole("button", { name: "Request changes" }));
    dialog = within(await screen.findByRole("alertdialog"));
    await user.click(dialog.getByRole("button", { name: "Request changes without them" }));
    expect(setPullRequestVerdict).toHaveBeenCalledExactlyOnceWith("p-1", REF, "changes_requested", true);
    expect(publishComments).toHaveBeenCalledOnce();
  });

  it("offers only the verdict when the comments cannot be published now", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(withUnpublished(1));
    vi.mocked(linkPullRequest).mockResolvedValue(
      makeLinked("This review compares with develop, but the pull request merges into main.", {}, null),
    );
    renderScreen();

    await user.click(await inBar().findByRole("button", { name: "Approve" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByRole("button", { name: "Publish 1 comment and approve" })).toBeDisabled();
    expect(dialog.getByText(/They cannot be published now: This review compares with develop/)).toBeInTheDocument();
  });

  it("lists the other reviewers with their verdicts, counting those past the first few", async () => {
    vi.mocked(linkPullRequest).mockResolvedValue(
      makeLinked(null, {
        participants: [
          { account: ME, reviewer: true, verdict: "approved" },
          { account: { id: "{1}", name: "Ana" }, reviewer: true, verdict: null },
          { account: { id: "{2}", name: "Bruno" }, reviewer: true, verdict: "approved" },
          { account: { id: "{3}", name: "Carla" }, reviewer: false, verdict: "changes_requested" },
          // Only commented: not a reviewer.
          { account: { id: "{4}", name: "Edu" }, reviewer: false, verdict: null },
          { account: { id: "{5}", name: "Fia" }, reviewer: true, verdict: null },
        ],
      }),
    );
    renderScreen();

    const reviewers = within(await inBar().findByRole("list", { name: "Reviewers" }));
    const items = reviewers.getAllByRole("listitem").map((item) => item.textContent);
    expect(items).toEqual([
      "✓Brunoapproved",
      "✕Carlarequested changes",
      "○Anahas not reviewed yet",
      "+1 more: Fia has not reviewed yet",
    ]);
    expect(reviewers.queryByText("Dana")).not.toBeInTheDocument();
    expect(reviewers.queryByText("Edu")).not.toBeInTheDocument();
  });
});

describe("comments on the pull request", () => {
  it("lets others' comments be answered, resolved and reopened, but never edited or deleted", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({ comments: [makeTheirComment("theirs", { body: "Is this retried?" })] }),
    );
    vi.mocked(setCommentResolved).mockResolvedValue(undefined);
    await openDiff(user);

    const thread = await screen.findByRole("group", { name: "Comment by Bruno" });
    expect(within(thread).getByText("Is this retried?")).toBeInTheDocument();
    expect(within(thread).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    expect(within(thread).queryByRole("button", { name: "Delete" })).not.toBeInTheDocument();
    expect(within(thread).getByRole("button", { name: "Reply" })).toBeInTheDocument();
    // Published: not a draft.
    expect(within(thread).queryByText("Draft")).not.toBeInTheDocument();

    await user.click(within(thread).getByRole("button", { name: "Resolve" }));
    expect(setCommentResolved).toHaveBeenCalledWith("p-1", "theirs", true);
  });

  it("marks drafts and edits that are not published yet", async () => {
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        comments: [
          makeComment("draft", { body: "Draft one" }),
          makeComment("edited", {
            line: 2,
            currentLine: 2,
            state: "published",
            remoteId: "r-1",
            remoteAuthorId: "{me}",
            pendingChange: "edit",
          }),
          makeComment("failed", { line: 3, currentLine: 3, publishError: "Bitbucket refused this: bad anchor" }),
        ],
      }),
    );
    await openDiff();

    await screen.findByTestId("under-new:1");
    expect(await under("new:1").findByText("Draft")).toBeInTheDocument();
    expect(under("new:2").getByText("Edited, not published yet")).toBeInTheDocument();
    expect(under("new:3").getByText("Not published: Bitbucket refused this: bad anchor")).toBeInTheDocument();
  });

  it("asks before deleting a published comment, and lets the delete be undone until it is published", async () => {
    const user = userEvent.setup();
    const published = makeComment("mine", { state: "published", remoteId: "r-1", remoteAuthorId: "{me}" });
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [published] }));
    vi.mocked(deleteComment).mockResolvedValue(undefined);
    vi.mocked(restoreComment).mockResolvedValue(undefined);
    const confirmSpy = vi.spyOn(window, "confirm");
    await openDiff(user);

    const thread = await screen.findByRole("group", { name: "Comment by You" });
    await user.click(within(thread).getByRole("button", { name: "Delete" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByText("Delete this comment from the pull request?")).toBeInTheDocument();
    await user.click(dialog.getByRole("button", { name: "Keep it" }));
    expect(deleteComment).not.toHaveBeenCalled();

    await user.click(within(thread).getByRole("button", { name: "Delete" }));
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({ comments: [{ ...published, pendingChange: "delete" }] }),
    );
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Delete comment" }));
    expect(deleteComment).toHaveBeenCalledExactlyOnceWith("p-1", "mine");
    expect(confirmSpy).not.toHaveBeenCalled();

    const deleted = await screen.findByRole("group", { name: "Deleted comment" });
    expect(deleted).toHaveTextContent("You deleted this comment. It leaves the pull request when you publish.");
    await user.click(within(deleted).getByRole("button", { name: "Undo" }));
    expect(restoreComment).toHaveBeenCalledWith("p-1", "mine");
  });

  it("deletes a draft at once: it never left DiffLens", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [makeComment("draft")] }));
    vi.mocked(deleteComment).mockResolvedValue(undefined);
    await openDiff(user);

    const thread = await screen.findByRole("group", { name: "Comment by You" });
    await user.click(within(thread).getByRole("button", { name: "Delete" }));
    expect(deleteComment).toHaveBeenCalledWith("p-1", "draft");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });
});

const shownGroup = () => within(screen.getByRole("group", { name: "Shown" }));
const commentsSegment = () => shownGroup().getByRole("button", { name: /^Comments/ });

/** Switches the Diff tab to its Comments view. */
async function openComments(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /^Comments \(/ }));
}

describe("comments on the pull request as a whole", () => {
  const general = () => within(screen.getByRole("region", { name: "Pull request comments" }));
  const summary = (unpublished: number) => ({ ...makeReviewState().summary, unpublished });

  it("are shown in the Comments view, from anyone, with drafts marked, and replies go in their thread", async () => {
    const user = userEvent.setup();
    const theirs = makeTheirComment("theirs", {
      kind: "general",
      path: "",
      line: 0,
      lineText: "",
      currentLine: null,
      body: "Is the refund idempotent?",
    });
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        comments: [theirs, makeGeneralComment("mine", { body: "Retries can double-refund." }), makeComment("on-line")],
        summary: summary(1),
      }),
    );
    vi.mocked(replyToComment).mockResolvedValue("reply");
    await openDiff(user);

    // Not above the files any more: the diff starts at the top of the pane.
    expect(await screen.findByTestId("diff-editor")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Pull request comments" })).not.toBeInTheDocument();
    // The line comment is under its line.
    expect(under("new:1").getByText("Comment on-line")).toBeInTheDocument();

    await openComments(user);
    const block = await screen.findByRole("region", { name: "Pull request comments" });
    expect(within(block).getByText("2 threads")).toBeInTheDocument();
    const bruno = within(block).getByRole("group", { name: "Comment by Bruno" });
    expect(bruno).toHaveTextContent("Is the refund idempotent?");
    expect(within(bruno).queryByRole("button", { name: "Edit" })).not.toBeInTheDocument();
    const mine = within(block).getByRole("group", { name: "Comment by You" });
    expect(within(mine).getByText("Draft")).toBeInTheDocument();
    expect(within(mine).getByRole("button", { name: "Edit" })).toBeInTheDocument();
    // The line comment is listed below, not among these.
    expect(block).not.toHaveTextContent("Comment on-line");
    expect(await publishButton()).toHaveTextContent("Publish comments (1)");

    await user.click(within(bruno).getByRole("button", { name: "Reply" }));
    await user.type(within(bruno).getByRole("textbox", { name: "Reply" }), "Yes, keyed by payment");
    await user.click(within(bruno).getByRole("button", { name: "Reply" }));
    expect(replyToComment).toHaveBeenCalledExactlyOnceWith("p-1", "theirs", "Yes, keyed by payment");
  });

  it("are written by hand, kept as drafts and counted in Publish comments and the verdict", async () => {
    const user = userEvent.setup();
    vi.mocked(addGeneralComment).mockResolvedValue("new");
    await openDiff(user);
    await openComments(user);

    expect(await general().findByText("No comments on the pull request as a whole yet.")).toBeInTheDocument();
    await user.click(general().getByRole("button", { name: "Write a comment" }));
    const box = general().getByRole("textbox", { name: "Comment on the pull request" });
    expect(box).toHaveFocus();
    await user.type(box, "Please split the migration out.");
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        comments: [makeGeneralComment("new", { body: "Please split the migration out." })],
        summary: summary(1),
      }),
    );
    await user.click(general().getByRole("button", { name: "Add comment" }));

    expect(addGeneralComment).toHaveBeenCalledExactlyOnceWith("p-1", REF, "Please split the migration out.");
    expect(await general().findByRole("group", { name: "Comment by You" })).toHaveTextContent(
      "Please split the migration out.",
    );
    expect(general().queryByRole("textbox")).not.toBeInTheDocument();
    expect(await publishButton()).toHaveTextContent("Publish comments (1)");
    // The verdict asks to publish it first, like any other.
    await user.click(await inBar().findByRole("button", { name: "Approve" }));
    const dialog = within(await screen.findByRole("alertdialog"));
    expect(dialog.getByRole("button", { name: "Publish 1 comment and approve" })).toBeInTheDocument();
  });
});

describe("the Comments view of the Diff tab", () => {
  const twoFiles = makeChangeSet([
    makeChangedFile("app/refund.go"),
    makeChangedFile("app/refund_test.go"),
  ]);
  const lineComments = () => within(screen.getByRole("region", { name: "Line comments" }));
  const comments = [
    makeGeneralComment("general", { body: "Split the migration out." }),
    makeGeneralComment("general-done", { body: "Settled.", resolved: true }),
    makeComment("draft", { path: "app/refund_test.go", line: 12, currentLine: 12, body: "Cover the retry.\nSecond line." }),
    makeTheirComment("theirs", { path: "app/refund.go", line: 40, currentLine: 41, body: "Why a new lock?", outdated: true }),
    makeComment("theirs-reply", { parentId: "theirs", path: "app/refund.go", body: "It guards the cache." }),
    makeComment("mine-done", { path: "app/refund.go", line: 7, currentLine: 7, state: "published", resolved: true }),
    makeComment("deleted", { path: "app/refund.go", line: 9, currentLine: 9, state: "published", pendingChange: "delete" }),
  ];

  beforeEach(() => {
    vi.mocked(listChangedFiles).mockImplementation(async (_id, target) => ({ ...twoFiles, target }));
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({ comments, summary: { ...makeReviewState().summary, unpublished: 2 } }),
    );
  });

  it("is chosen beside the diff's own switches, counts the open conversations and keeps Publish", async () => {
    const user = await openDiff();

    const files = await shownGroup().findByRole("button", { name: "Files" });
    expect(files).toHaveAttribute("aria-pressed", "true");
    // Open: the general draft, the draft and their thread on lines; not what is resolved or deleted.
    expect(commentsSegment()).toHaveTextContent("Comments (3)");
    expect(commentsSegment()).toHaveAttribute("aria-pressed", "false");
    expect(commentsSegment()).toHaveAttribute(
      "title",
      "3 open conversations, on the pull request and on lines. Resolved ones are listed, not counted.",
    );
    expect(screen.getByRole("group", { name: "Lines shown" })).toBeInTheDocument();

    await user.click(commentsSegment());
    expect(commentsSegment()).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("region", { name: "Comments" })).toBeInTheDocument();
    // What only applies to the files is not offered here; Publish is, with the same count.
    expect(screen.queryByRole("group", { name: "Lines shown" })).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Layout" })).not.toBeInTheDocument();
    expect(screen.queryByText("Click a line to comment")).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Diff of app/refund.go" })).not.toBeInTheDocument();
    expect(await publishButton()).toHaveTextContent("Publish comments (2)");

    await user.click(shownGroup().getByRole("button", { name: "Files" }));
    expect(screen.queryByRole("region", { name: "Comments" })).not.toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Diff of app/refund.go" })).toBeInTheDocument();
  });

  it("lists the general conversations, then the line ones by file with their state", async () => {
    const user = await openDiff();
    await openComments(user);

    const general = within(screen.getByRole("region", { name: "Pull request comments" }));
    expect(general.getByText("2 threads")).toBeInTheDocument();
    expect(general.getByRole("button", { name: "Write a comment" })).toBeInTheDocument();
    expect(general.getByRole("group", { name: "Comment by You" })).toHaveTextContent("Split the migration out.");

    // By file in the file list's order, each by line.
    const refund = within(lineComments().getByRole("list", { name: "app/refund.go" }));
    const rows = refund.getAllByRole("button");
    expect(rows.map((row) => row.textContent)).toEqual([
      "Line 7Comment mine-doneYouPublishedResolved",
      "Line 9Comment deletedYouDeleted",
      "Line 41Why a new lock?BrunoPublishedOutdated1 reply",
    ]);
    const tests = within(lineComments().getByRole("list", { name: "app/refund_test.go" }));
    // Only the first line of the first comment.
    expect(tests.getByRole("button")).toHaveTextContent(/^Line 12Cover the retry\.YouDraft$/);
    expect(lineComments().getByText("4 threads")).toBeInTheDocument();
  });

  it("opens a line conversation in Files, at its line, focused", async () => {
    const user = await openDiff();
    await openComments(user);

    await user.click(lineComments().getByRole("button", { name: /^Line 12/ }));

    expect(shownGroup().getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("region", { name: "Diff of app/refund_test.go" })).toBeInTheDocument();
    const thread = await screen.findByRole("group", { name: "Comment by You" });
    await waitFor(() => expect(thread).toHaveFocus());
    expect(thread).toHaveTextContent("Cover the retry.");
  });

  it("is kept while the review is open, and a file chosen in the list is shown", async () => {
    const user = await openDiff();
    await openComments(user);

    await user.click(reviewTab(/^Tests/));
    await user.click(reviewTab("Diff"));
    expect(commentsSegment()).toHaveAttribute("aria-pressed", "true");

    await user.click(within(screen.getByRole("listbox", { name: "Changed files" })).getByRole("option", { name: /refund_test\.go/ }));
    expect(shownGroup().getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
    expect(await screen.findByRole("region", { name: "Diff of app/refund_test.go" })).toBeInTheDocument();
  });

  it("does not step through files on j or k, in a composer or outside one", async () => {
    const user = await openDiff();
    const selected = () =>
      within(screen.getByRole("listbox", { name: "Changed files" })).getByRole("option", { selected: true });
    await waitFor(() => expect(selected()).toHaveTextContent("refund.go"));
    await openComments(user);

    await user.keyboard("j");
    const general = within(screen.getByRole("region", { name: "Pull request comments" }));
    await user.click(general.getByRole("button", { name: "Write a comment" }));
    // "[[" types a "[".
    await user.type(general.getByRole("textbox", { name: "Comment on the pull request" }), "jk[[]");

    expect(general.getByRole("textbox", { name: "Comment on the pull request" })).toHaveValue("jk[]");
    expect(commentsSegment()).toHaveAttribute("aria-pressed", "true");
    expect(selected()).toHaveTextContent(/refund\.go/);
    expect(selected()).not.toHaveTextContent(/refund_test/);
    // Back in Files, j steps again.
    await user.click(shownGroup().getByRole("button", { name: "Files" }));
    await user.keyboard("j");
    await waitFor(() => expect(selected()).toHaveTextContent(/refund_test\.go/));
  });
});
