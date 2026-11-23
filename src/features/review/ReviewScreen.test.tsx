import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  addComment,
  type ChangeSet,
  deleteComment,
  draftSignalComment,
  editComment,
  type FileVersions,
  getFileVersions,
  getInsights,
  getQuality,
  getReviewState,
  getTestsState,
  linkPullRequest,
  listChangedFiles,
  replyToComment,
  type ReviewTarget,
  setCommentResolved,
  setFileReviewed,
  writeFindingComment,
} from "@/lib/api";
import { absentSide, apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeFinding, makeInsights } from "@/test/insights-fixtures";
import { makeComment, makeReviewState, makeTheirComment } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead, openReviewTab, reviewTab } from "@/test/review-screen";
import { makeLocatedCheck, makeQualityReport, makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  setFileReviewed: vi.fn(),
  addComment: vi.fn(),
  replyToComment: vi.fn(),
  editComment: vi.fn(),
  deleteComment: vi.fn(),
  setCommentResolved: vi.fn(),
  linkPullRequest: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
  draftSignalComment: vi.fn(),
  writeFindingComment: vi.fn(),
}));

// CodeMirror needs layout APIs jsdom does not have; the stub shows what it was given, draws the blocks under lines
// in a list, and offers a button per line to stand for a click on it.
vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div
      data-testid="diff-editor"
      data-path={props.path}
      data-layout={props.layout}
      data-collapse={String(props.collapseUnchanged)}
      data-language={props.language ?? ""}
    >
      <pre data-testid="old-text">{props.oldText}</pre>
      <pre data-testid="new-text">{props.newText}</pre>
      {props.onLineClick && (
        <>
          <button type="button" onClick={() => props.onLineClick?.("old", 1)}>
            old line 1
          </button>
          <button type="button" onClick={() => props.onLineClick?.("new", 1)}>
            new line 1
          </button>
        </>
      )}
      {(props.annotations ?? []).map((annotation) => (
        <div key={annotation.key} data-testid={`under-${annotation.key}`}>
          {props.renderAnnotation?.(annotation.key)}
        </div>
      ))}
    </div>
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };

const files = [
  makeChangedFile("internal/refund/service.go", { status: "added", additions: 62, deletions: 0 }),
  makeChangedFile("internal/refund/handler.go", { status: "added", additions: 48, deletions: 0 }),
  makeChangedFile("internal/payment/repo.go", { additions: 14, deletions: 9 }),
  makeChangedFile("internal/storage/ledger.go", {
    status: "renamed",
    oldPath: "internal/storage/refund_repo.go",
    additions: 2,
    deletions: 2,
  }),
  makeChangedFile("go.mod", { additions: 1, deletions: 0, language: null }),
  makeChangedFile("internal/legacy/old.go", { status: "deleted", additions: 0, deletions: 30 }),
  makeChangedFile("assets/logo.png", { status: "added", binary: true, additions: 0, deletions: 0, language: null }),
];
const changeSet: ChangeSet = makeChangeSet(files);

let bar: HTMLDivElement;

function renderScreen(target: ReviewTarget = BRANCH) {
  return render(<ReviewScreen projectId="p-1" target={target} barSlot={bar} />);
}

/** Opens a review and then its Diff tab, as a reviewer would: the review itself opens on the Overview. */
async function openDiff(user = userEvent.setup(), target: ReviewTarget = BRANCH) {
  renderScreen(target);
  await openReviewTab(user, "Diff");
  return user;
}

const fileList = () => within(screen.getByRole("listbox", { name: "Changed files" }));
const option = (path: string) =>
  screen.getByRole("option", { name: new RegExp(`^${path.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")},`) });
const selectedOption = () => fileList().getByRole("option", { selected: true });
const shownPath = async () => (await screen.findByTestId("diff-editor")).dataset.path;

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockImplementation(async (_id, _old, _new, path) => ({
    old: textSide(`old ${path}\n`),
    new: textSide(`new ${path}\n`),
  }));
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
});

afterEach(() => {
  bar.remove();
});

describe("review shell", () => {
  it("shows what is compared, its size and the commit the files are read from in the top bar", async () => {
    renderScreen();

    expect(within(bar).getByTestId("compared")).toHaveTextContent("feat/refund-flow into main");
    expect(within(bar).getByRole("status", { name: "Counting changes" })).toBeInTheDocument();
    expect(await within(bar).findByText("7 files")).toBeInTheDocument();
    expect(within(bar).getByText("+127")).toBeInTheDocument();
    expect(within(bar).getByText("−41")).toBeInTheDocument();
    const sides = within(bar).getByText("e7f3b10").closest("span[title]");
    expect(sides).toHaveTextContent("At e7f3b10");
    expect(sides).toHaveAttribute("title", expect.stringContaining("compared with a1b2c3d4e5f6a7b8"));
    expect(listChangedFiles).toHaveBeenCalledExactlyOnceWith("p-1", BRANCH);
    // Once read, the base is named as the change set was compared with it: origin's main.
    expect(within(bar).getByTestId("compared")).toHaveTextContent("feat/refund-flow into origin/main");
    expect(sides).toHaveAttribute("title", expect.stringContaining("where it left origin/main"));
  });

  it("names the pull request's base when its destination commit is what the review compares with", async () => {
    vi.mocked(listChangedFiles).mockResolvedValue(
      makeChangeSet(files, { base: { name: "main", kind: "pull_request", refName: null, sha: "0123456789abcdef" } }),
    );
    renderScreen();

    expect(await within(bar).findByText("7 files")).toBeInTheDocument();
    expect(within(bar).getByTestId("compared")).toHaveTextContent("feat/refund-flow into the pull request's main");
  });

  it("compares the uncommitted changes with HEAD, read from the working tree", async () => {
    vi.mocked(listChangedFiles).mockResolvedValue(
      makeChangeSet(files, { target: { kind: "working_tree" }, newSha: null }),
    );
    renderScreen({ kind: "working_tree" });

    expect(within(bar).getByTestId("compared")).toHaveTextContent("Uncommitted changes against HEAD");
    expect(await within(bar).findByText("Working tree")).toHaveAttribute(
      "title",
      expect.stringContaining("read from disk"),
    );
    expect(listChangedFiles).toHaveBeenCalledWith("p-1", { kind: "working_tree" });
  });

  it("opens on the Overview tab, with the Diff behind it", async () => {
    renderScreen();
    await changedFilesRead();

    const tabs = within(screen.getByRole("tablist", { name: "Review views" }));
    expect(tabs.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    expect(tabs.getByRole("tab", { name: "Diff" })).toHaveAttribute("aria-selected", "false");
    const shown = screen.getByRole("tabpanel");
    expect(within(shown).getByRole("heading", { level: 1, name: "What this change does" })).toBeInTheDocument();
    expect(screen.queryByRole("listbox", { name: "Changed files" })).not.toBeInTheDocument();
  });

  it("selects Overview, offers Flow, Diff, Tests, Quality and Insights, and disables the other views", async () => {
    renderScreen();

    const tabs = within(screen.getByRole("tablist", { name: "Review views" }));
    expect(tabs.getAllByRole("tab").map((tab) => tab.textContent)).toEqual([
      "Overview",
      "Flow",
      "Diff",
      "Tests",
      "Quality",
      "Insights",
      "Run",
    ]);
    expect(tabs.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    for (const name of ["Overview", "Diff", "Flow", "Tests", "Quality", "Insights"]) {
      expect(tabs.getByRole("tab", { name })).not.toHaveAttribute("aria-disabled", "true");
    }
    for (const name of ["Run"]) {
      const tab = tabs.getByRole("tab", { name });
      expect(tab).toHaveAttribute("aria-disabled", "true");
      expect(tab).toHaveAttribute("title", "Comes in a later step");
    }

    await userEvent.setup().click(tabs.getByRole("tab", { name: "Run" }));
    expect(tabs.getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
  });

  it("keeps a view as it was behind another tab instead of building it again, and its keys quiet", async () => {
    const user = await openDiff();
    const editor = await screen.findByTestId("diff-editor");
    const tabs = within(screen.getByRole("tablist", { name: "Review views" }));

    await user.click(tabs.getByRole("tab", { name: "Tests" }));
    const diffPanel = editor.closest<HTMLElement>("[role='tabpanel']")!;
    expect(diffPanel).toHaveAttribute("hidden");
    expect(editor).toBeInTheDocument();
    // j and k move through the files only while the Diff tab is shown.
    await user.keyboard("j");

    await user.click(tabs.getByRole("tab", { name: "Diff" }));
    expect(screen.getByTestId("diff-editor")).toBe(editor);
    expect(diffPanel).not.toHaveAttribute("hidden");
    expect(selectedOption()).toHaveAccessibleName(/^go\.mod,/);
    expect(getFileVersions).toHaveBeenCalledTimes(1);

    await user.click(tabs.getByRole("tab", { name: "Tests" }));
    const testsPanel = screen.getByRole("tabpanel");
    await user.click(tabs.getByRole("tab", { name: "Diff" }));
    await user.click(tabs.getByRole("tab", { name: "Tests" }));
    expect(screen.getByRole("tabpanel")).toBe(testsPanel);
    expect(getTestsState).toHaveBeenCalledTimes(1);
  });

  it("builds a view only the first time its tab opens", async () => {
    renderScreen();
    await screen.findByTestId("diff-editor");
    await screen.findByRole("heading", { level: 1, name: "What this change does" });

    const panels = document.querySelectorAll("[role='tabpanel']");
    const empty = [...panels].filter((panel) => panel.childElementCount === 0);
    // Flow, Tests, Quality and Insights: the Overview, where the review opens, and the Diff are built.
    expect(empty).toHaveLength(4);
  });

  it("reloads the changed files, keeping the selected file", async () => {
    const user = await openDiff();
    await user.click(await screen.findByRole("option", { name: /^internal\/payment\/repo\.go,/ }));
    const reloaded = deferred<ChangeSet>();
    vi.mocked(listChangedFiles).mockReturnValueOnce(reloaded.promise);

    await user.click(within(bar).getByRole("button", { name: "Reload" }));

    expect(within(bar).getByRole("button", { name: "Reload" })).toBeDisabled();
    expect(screen.getByRole("status", { name: "Loading changed files" })).toBeInTheDocument();
    reloaded.resolve(makeChangeSet(files, { newSha: "f00dfeed00000000" }));
    expect(await within(bar).findByText("f00dfee")).toBeInTheDocument();
    expect(listChangedFiles).toHaveBeenCalledTimes(2);
    expect(await shownPath()).toBe("internal/payment/repo.go");
    expect(getFileVersions).toHaveBeenLastCalledWith("p-1", "a1b2c3d4e5f6a7b8", "f00dfeed00000000", "internal/payment/repo.go", null);
  });

  it("falls back to the first file when the selected one is gone after a reload", async () => {
    const user = await openDiff();
    await user.click(await screen.findByRole("option", { name: /^internal\/payment\/repo\.go,/ }));
    vi.mocked(listChangedFiles).mockResolvedValueOnce(makeChangeSet(files.filter((f) => f.path !== "internal/payment/repo.go")));

    await user.click(within(bar).getByRole("button", { name: "Reload" }));

    expect(await screen.findByRole("option", { selected: true })).toHaveAccessibleName(/^go\.mod,/);
  });

  it("shows an error loading the changed files inline and tries again", async () => {
    const user = userEvent.setup();
    vi.mocked(listChangedFiles).mockRejectedValueOnce(apiError("unknown_branch", "Branch feat/refund-flow no longer exists."));
    renderScreen();
    await user.click(reviewTab("Diff"));

    expect(await screen.findByRole("alert")).toHaveTextContent("Branch feat/refund-flow no longer exists.");
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByRole("listbox", { name: "Changed files" })).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("says when there is nothing to review", async () => {
    vi.mocked(listChangedFiles).mockResolvedValue(makeChangeSet([]));
    await openDiff();

    expect(
      await screen.findByText("Nothing to review: feat/refund-flow changes no file compared with origin/main."),
    ).toBeInTheDocument();
    expect(fileList().getByText("No changed files.")).toBeInTheDocument();
    expect(getFileVersions).not.toHaveBeenCalled();
  });
});

describe("file list", () => {
  it("groups files by directory, root first, with status, renames and line counts", async () => {
    await openDiff();

    await screen.findByRole("listbox", { name: "Changed files" });
    const shape = fileList()
      .getAllByRole("group")
      .map((group) => ({
        dir: document.getElementById(group.getAttribute("aria-labelledby") ?? "")?.textContent,
        files: within(group)
          .getAllByRole("option")
          .map((option) => option.dataset.path),
      }));
    expect(shape).toEqual([
      { dir: "Repository root", files: ["go.mod"] },
      { dir: "assets", files: ["assets/logo.png"] },
      { dir: "internal/legacy", files: ["internal/legacy/old.go"] },
      { dir: "internal/payment", files: ["internal/payment/repo.go"] },
      { dir: "internal/refund", files: ["internal/refund/handler.go", "internal/refund/service.go"] },
      { dir: "internal/storage", files: ["internal/storage/ledger.go"] },
    ]);

    expect(option("internal/refund/service.go")).toHaveAccessibleName("internal/refund/service.go, added, 62 added, 0 removed");
    expect(option("internal/refund/service.go")).toHaveTextContent("Aservice.go+62");
    expect(option("internal/payment/repo.go")).toHaveTextContent("Mrepo.go+14 −9");
    expect(option("internal/legacy/old.go")).toHaveTextContent("Dold.go−30");
    expect(option("internal/storage/ledger.go")).toHaveAccessibleName(
      "internal/storage/ledger.go, renamed from internal/storage/refund_repo.go, 2 added, 2 removed",
    );
    expect(option("internal/storage/ledger.go")).toHaveTextContent("Rrefund_repo.go → ledger.go+2 −2");
    expect(option("assets/logo.png")).toHaveTextContent("Alogo.pngbinary");
  });

  it("selects the first file in list order and loads both of its sides", async () => {
    await openDiff();

    expect(await shownPath()).toBe("go.mod");
    expect(selectedOption()).toHaveAccessibleName(/^go\.mod,/);
    expect(getFileVersions).toHaveBeenCalledExactlyOnceWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a", "go.mod", null);
    expect(screen.getByRole("heading", { level: 2, name: "go.mod" })).toBeInTheDocument();
  });

  it("loads a clicked file, passing the old path of a rename", async () => {
    const user = await openDiff();
    await shownPath();

    await user.click(option("internal/storage/ledger.go"));

    expect(await shownPath()).toBe("internal/storage/ledger.go");
    expect(getFileVersions).toHaveBeenLastCalledWith(
      "p-1",
      "a1b2c3d4e5f6a7b8",
      "e7f3b10c9d8e7f6a",
      "internal/storage/ledger.go",
      "internal/storage/refund_repo.go",
    );
    const header = screen.getByRole("heading", { level: 2 });
    expect(header).toHaveTextContent("internal/storage/refund_repo.go → internal/storage/ledger.go");
    expect(header.parentElement).toHaveTextContent("Renamed+2 −2");
  });

  it("moves with j and k, and with ] and [, in list order, stopping at the ends", async () => {
    const user = await openDiff();
    await shownPath();

    await user.keyboard("j");
    expect(selectedOption()).toHaveAccessibleName(/^assets\/logo\.png,/);
    await user.keyboard("]j");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/payment\/repo\.go,/);
    expect(await shownPath()).toBe("internal/payment/repo.go");
    await user.keyboard("k");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/legacy\/old\.go,/);
    await user.keyboard("[[[[");
    expect(selectedOption()).toHaveAccessibleName(/^go\.mod,/);
    await user.keyboard("jjjjjjjjjj");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/storage\/ledger\.go,/);
  });

  it("ignores j and k with a modifier", async () => {
    const user = await openDiff();
    await shownPath();

    await user.keyboard("{Control>}j{/Control}{Meta>}j{/Meta}");

    expect(selectedOption()).toHaveAccessibleName(/^go\.mod,/);
  });

  it("moves the selection and focus with the arrow keys, Home and End", async () => {
    const user = await openDiff();
    await shownPath();

    screen.getByRole("tab", { name: "Diff" }).focus();
    await user.tab();
    expect(option("go.mod")).toHaveFocus();
    await user.keyboard("{ArrowDown}{ArrowDown}");
    expect(option("internal/legacy/old.go")).toHaveFocus();
    expect(selectedOption()).toHaveAccessibleName(/^internal\/legacy\/old\.go,/);
    await user.keyboard("{End}");
    expect(option("internal/storage/ledger.go")).toHaveFocus();
    await user.keyboard("{ArrowUp}");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/refund\/service\.go,/);
    await user.keyboard("{Home}");
    expect(option("go.mod")).toHaveFocus();
    expect(option("go.mod")).toHaveAttribute("tabindex", "0");
    expect(option("assets/logo.png")).toHaveAttribute("tabindex", "-1");
  });

  it("offers no filter for 20 files or fewer", async () => {
    vi.mocked(listChangedFiles).mockResolvedValue(
      makeChangeSet(Array.from({ length: 20 }, (_, i) => makeChangedFile(`pkg/file${String(i).padStart(2, "0")}.go`))),
    );
    await openDiff();

    await screen.findByRole("listbox", { name: "Changed files" });
    expect(screen.queryByRole("searchbox", { name: "Filter files" })).not.toBeInTheDocument();
  });

  it("filters a long list by path, and j/k walk only the files shown while typing does not move", async () => {
    const user = userEvent.setup();
    const many = [
      ...Array.from({ length: 20 }, (_, i) => makeChangedFile(`pkg/file${String(i).padStart(2, "0")}.go`)),
      makeChangedFile("internal/refund/service.go"),
      makeChangedFile("internal/refund/handler.go"),
      makeChangedFile("internal/ledger.go", { status: "renamed", oldPath: "internal/refund_ledger.go" }),
    ];
    vi.mocked(listChangedFiles).mockResolvedValue(makeChangeSet(many));
    await openDiff(user);

    const filter = await screen.findByRole("searchbox", { name: "Filter files" });
    await user.type(filter, "REFUND");

    expect(fileList().getAllByRole("option").map((option) => option.dataset.path)).toEqual([
      "internal/ledger.go",
      "internal/refund/handler.go",
      "internal/refund/service.go",
    ]);
    expect(await shownPath()).toBe("internal/ledger.go");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/ledger\.go,/);

    filter.blur();
    await user.keyboard("j");
    expect(selectedOption()).toHaveAccessibleName(/^internal\/refund\/handler\.go,/);

    await user.clear(filter);
    await user.type(filter, "nothing-like-this");
    expect(fileList().getByText("No file matches.")).toBeInTheDocument();
    expect(await shownPath()).toBe("internal/refund/handler.go");

    await user.keyboard("{Escape}");
    expect(filter).toHaveValue("");
    expect(fileList().getAllByRole("option")).toHaveLength(23);
  });
});

describe("diff view", () => {
  const versions = (value: FileVersions) => vi.mocked(getFileVersions).mockResolvedValue(value);

  it("shows the two sides in a unified view with unchanged lines collapsed, highlighted by language", async () => {
    await openDiff();

    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-layout", "unified");
    expect(editor).toHaveAttribute("data-collapse", "true");
    expect(within(editor).getByTestId("old-text")).toHaveTextContent("old go.mod");
    expect(within(editor).getByTestId("new-text")).toHaveTextContent("new go.mod");
    expect(screen.getByRole("button", { name: "Changes only" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByRole("button", { name: "Unified" })).toHaveAttribute("aria-pressed", "true");
    expect(screen.getByText(/next and previous file/)).toHaveTextContent("j k or ] [ next and previous file");
  });

  it("switches to the full file and side by side, and keeps the choice across files", async () => {
    const user = await openDiff();
    await shownPath();

    await user.click(screen.getByRole("button", { name: "Full file" }));
    await user.click(screen.getByRole("button", { name: "Side by side" }));

    expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-collapse", "false");
    expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-layout", "split");
    await user.keyboard("jj");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/legacy/old.go");
    expect(editor).toHaveAttribute("data-collapse", "false");
    expect(editor).toHaveAttribute("data-layout", "split");
    expect(editor).toHaveAttribute("data-language", "go");
  });

  it("shows an added file as all additions and a deleted one as all removals", async () => {
    const user = userEvent.setup();
    versions({ old: absentSide, new: textSide("package main\n") });
    await openDiff(user);

    expect(within(await screen.findByTestId("diff-editor")).getByTestId("old-text")).toHaveTextContent(/^$/);
    expect(screen.getByTestId("new-text")).toHaveTextContent("package main");

    versions({ old: textSide("package legacy\n"), new: absentSide });
    await user.click(option("internal/legacy/old.go"));
    await screen.findByText("Deleted");
    expect(await screen.findByTestId("old-text")).toHaveTextContent("package legacy");
    expect(screen.getByTestId("new-text")).toHaveTextContent(/^$/);
  });

  it.each([
    [{ old: absentSide, new: { text: null, reason: "binary", size: 2048 } }, "This file is binary, so it is not shown."],
    [
      { old: textSide("x"), new: { text: null, reason: "too_large", size: 5 * 1024 * 1024 } },
      "The new version is larger than 2 MB (5 MB), so it is not shown.",
    ],
    [
      { old: { text: null, reason: "not_utf8", size: 300 }, new: { text: null, reason: "not_utf8", size: 310 } },
      "This file is not UTF-8 text, so it is not shown.",
    ],
    [{ old: textSide("same\n"), new: textSide("same\n") }, "The content of this file did not change."],
  ] as [FileVersions, string][])("explains a file it cannot diff: %j", async (value, message) => {
    versions(value);
    await openDiff();

    expect(await screen.findByText(message)).toBeInTheDocument();
    expect(screen.queryByTestId("diff-editor")).not.toBeInTheDocument();
  });

  it("shows a skeleton while a file loads, and its error inline with a retry", async () => {
    const user = userEvent.setup();
    const first = deferred<FileVersions>();
    vi.mocked(getFileVersions).mockReturnValueOnce(first.promise);
    await openDiff(user);

    expect(await screen.findByRole("status", { name: "Loading file" })).toBeInTheDocument();
    first.reject(apiError("unknown_file", "go.mod is not part of this review."));
    expect(await screen.findByRole("alert")).toHaveTextContent("go.mod is not part of this review.");
    expect(screen.getByRole("listbox", { name: "Changed files" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByTestId("diff-editor")).toBeInTheDocument();
    expect(getFileVersions).toHaveBeenCalledTimes(2);
  });
});

describe("review state", () => {
  const REF = { target: BRANCH, oldSha: "a1b2c3d4e5f6a7b8", newSha: "e7f3b10c9d8e7f6a" };
  const marked = (...entries: [string, "reviewed" | "reopened"][]) =>
    makeReviewState({ files: entries.map(([path, status]) => ({ path, status, reviewedAt: "2026-11-19T09:00:00Z" })) });
  const markButton = (name: string | RegExp) => screen.findByRole("button", { name });
  const under = (key: string) => within(screen.getByTestId(`under-${key}`));

  it("reads the state of the review for the sides of its change set", async () => {
    await openDiff();
    await shownPath();

    expect(getReviewState).toHaveBeenCalledWith("p-1", REF, []);
  });

  it("shows the progress in the tab row and the marks in the file list", async () => {
    vi.mocked(getReviewState).mockResolvedValue(
      marked(["internal/payment/repo.go", "reviewed"], ["go.mod", "reopened"], ["gone.go", "reviewed"]),
    );
    await openDiff();

    expect(await screen.findByText(/^Reviewed/)).toHaveTextContent("Reviewed 1 of 7 files");
    expect(screen.getByRole("progressbar", { name: "Files reviewed" })).toHaveAttribute("aria-valuenow", "1");
    expect(option("internal/payment/repo.go")).toHaveAccessibleName(
      "internal/payment/repo.go, modified, 14 added, 9 removed, reviewed",
    );
    expect(option("internal/payment/repo.go")).toHaveTextContent("Mrepo.go+14 −9✓");
    expect(option("go.mod")).toHaveAccessibleName(/, changed since your review$/);
    expect(option("go.mod")).toHaveTextContent(/changed$/);
  });

  it("marks the shown file as reviewed, and a press on a reviewed file unmarks it", async () => {
    const user = await openDiff();
    const button = await markButton("Mark as reviewed");
    vi.mocked(getReviewState).mockResolvedValue(marked(["go.mod", "reviewed"]));

    await user.click(button);

    expect(setFileReviewed).toHaveBeenCalledExactlyOnceWith("p-1", REF, "go.mod", true);
    expect(await markButton("✓ Reviewed")).toBeEnabled();
    expect(await screen.findByText(/^Reviewed/)).toHaveTextContent("Reviewed 1 of 7 files");

    vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
    await user.click(await markButton("✓ Reviewed"));
    expect(setFileReviewed).toHaveBeenLastCalledWith("p-1", REF, "go.mod", false);
    expect(await markButton("Mark as reviewed")).toBeInTheDocument();
  });

  it("offers to mark a file that changed since it was reviewed again", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(marked(["go.mod", "reopened"]));
    await openDiff(user);

    await user.click(await markButton("Changed since your review. Mark as reviewed again"));

    expect(setFileReviewed).toHaveBeenCalledWith("p-1", REF, "go.mod", true);
  });

  it("says why a save failed", async () => {
    const user = userEvent.setup();
    vi.mocked(setFileReviewed).mockRejectedValue(apiError("unknown_file", '"go.mod" is not a changed file of this review.'));
    await openDiff(user);

    await user.click(await markButton("Mark as reviewed"));

    expect(await screen.findByRole("alert")).toHaveTextContent("is not a changed file of this review");
  });

  it("writes a comment on a clicked line, saved with Cmd+Enter", async () => {
    const user = userEvent.setup();
    vi.mocked(addComment).mockResolvedValue("c-1");
    await openDiff(user);
    expect(await screen.findByText("Click a line to comment")).toBeInTheDocument();

    await user.click(await screen.findByRole("button", { name: "new line 1" }));
    const box = under("new:1").getByRole("textbox", { name: "Comment on this line" });
    expect(box).toHaveFocus();
    // Typing j and k does not move to another file.
    await user.type(box, "Why jk here?");
    expect(selectedOption()).toHaveAccessibleName(/^go\.mod,/);
    expect(under("new:1").getByRole("button", { name: "Add comment" })).toBeEnabled();

    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [makeComment("c-1", { body: "Why jk here?" })] }));
    await user.keyboard("{Meta>}{Enter}{/Meta}");

    expect(addComment).toHaveBeenCalledExactlyOnceWith("p-1", REF, "go.mod", "new", 1, "Why jk here?");
    expect(await under("new:1").findByText("Why jk here?")).toBeInTheDocument();
    expect(under("new:1").getByText("You")).toBeInTheDocument();
    expect(under("new:1").queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("cancels a comment with Escape or Cancel, and never saves an empty one", async () => {
    const user = await openDiff();

    await user.click(await screen.findByRole("button", { name: "old line 1" }));
    expect(under("old:1").getByRole("button", { name: "Add comment" })).toBeDisabled();
    await user.keyboard("   {Meta>}{Enter}{/Meta}");
    expect(addComment).not.toHaveBeenCalled();
    await user.keyboard("{Escape}");
    expect(screen.queryByRole("textbox", { name: "Comment on this line" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "new line 1" }));
    await user.click(under("new:1").getByRole("button", { name: "Cancel" }));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();
  });

  it("shows a thread with its replies, flags code that changed, and replies, edits, resolves and deletes", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        comments: [
          makeComment("root", { body: "Wrap this error.", outdated: true }),
          makeComment("reply", { parentId: "root", body: "Agreed." }),
          makeTheirComment("theirs", { parentId: "root", body: "On it.", remoteAuthorId: "u-2", remoteAuthorName: "Dana" }),
        ],
      }),
    );
    await openDiff(user);

    const thread = await screen.findByRole("group", { name: "Comment by You" });
    expect(thread).toHaveTextContent("Wrap this error.");
    expect(within(thread).getByText("Code changed since this comment")).toBeInTheDocument();
    expect(within(thread).getByText("Agreed.")).toBeInTheDocument();
    expect(within(thread).getByText("Dana")).toBeInTheDocument();
    // Edit and Delete only on the user's own comments: the first one and the first reply.
    expect(within(thread).getAllByRole("button", { name: "Edit" })).toHaveLength(2);

    await user.click(within(thread).getByRole("button", { name: "Reply" }));
    await user.type(within(thread).getByRole("textbox", { name: "Reply" }), "Done now.");
    await user.click(within(thread).getByRole("button", { name: "Reply" }));
    expect(replyToComment).toHaveBeenCalledWith("p-1", "root", "Done now.");

    await user.click(within(thread).getAllByRole("button", { name: "Edit" })[1]);
    const edit = within(thread).getByRole("textbox", { name: "Edit comment" });
    expect(edit).toHaveValue("Agreed.");
    await user.clear(edit);
    await user.type(edit, "Agreed, mostly.");
    await user.click(within(thread).getByRole("button", { name: "Save" }));
    expect(editComment).toHaveBeenCalledWith("p-1", "reply", "Agreed, mostly.");

    await user.click(within(thread).getByRole("button", { name: "Resolve" }));
    expect(setCommentResolved).toHaveBeenCalledWith("p-1", "root", true);

    await user.click(within(thread).getAllByRole("button", { name: "Delete" })[1]);
    expect(deleteComment).toHaveBeenCalledWith("p-1", "reply");
  });

  it("folds a resolved thread to one line that can be shown or reopened", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({ comments: [makeComment("root", { body: "Wrap this error.", resolved: true })] }),
    );
    await openDiff(user);

    const folded = await screen.findByRole("group", { name: "Resolved comment by You" });
    expect(folded).toHaveTextContent("ResolvedYou: Wrap this error.");
    expect(within(folded).queryByRole("button", { name: "Reply" })).not.toBeInTheDocument();
    await user.click(within(folded).getByRole("button", { name: "Show" }));
    expect(screen.getByRole("group", { name: "Comment by You" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Reopen" }));
    expect(setCommentResolved).toHaveBeenCalledWith("p-1", "root", false);
  });

  it("offers Since last review only once there is something new, and says why not", async () => {
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
    await openDiff();
    const since = await screen.findByRole("button", { name: "Since last review" });

    await waitForTitle(since, /^Mark a file or a function as reviewed first/);
    expect(since).toHaveAttribute("aria-disabled", "true");
    await userEvent.setup().click(since);
    expect(screen.getByRole("button", { name: "Whole branch" })).toHaveAttribute("aria-pressed", "true");
  });

  it("explains that uncommitted changes have no last review to compare", async () => {
    vi.mocked(listChangedFiles).mockResolvedValue(makeChangeSet(files, { target: { kind: "working_tree" }, newSha: null }));
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({ since: { kind: "unavailable", reason: "working_tree" } }),
    );
    await openDiff(userEvent.setup(), { kind: "working_tree" });

    await waitForTitle(await screen.findByRole("button", { name: "Since last review" }), /^Uncommitted changes have no commits/);
  });

  it("since the last review, lists only what changed and diffs it from the checkpoint", async () => {
    const user = userEvent.setup();
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        checkpoint: { sha: "c0ffee0000000000", at: "2026-11-19T09:00:00Z" },
        since: {
          kind: "available",
          checkpointSha: "c0ffee0000000000",
          commits: 2,
          files: [makeChangedFile("internal/payment/repo.go", { additions: 1, deletions: 1 })],
        },
        comments: [makeComment("old side", { side: "old", body: "Old side note" })],
      }),
    );
    await openDiff(user);
    // Under its line of the file shown (the hidden Comments view lists it too).
    expect(await screen.findByRole("group", { name: "Comment by You" })).toHaveTextContent("Old side note");
    await user.click(option("go.mod"));

    await user.click(screen.getByRole("button", { name: "Since last review" }));

    expect(fileList().getAllByRole("option").map((row) => row.dataset.path)).toEqual(["internal/payment/repo.go"]);
    // go.mod stays selected, with nothing new to show, and can still be marked.
    expect(await screen.findByText("Nothing in this file changed since your last review.")).toBeInTheDocument();
    expect(screen.getByRole("heading", { level: 2, name: "go.mod" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Mark as reviewed" })).toBeInTheDocument();

    await user.click(option("internal/payment/repo.go"));
    expect(await shownPath()).toBe("internal/payment/repo.go");
    expect(getFileVersions).toHaveBeenLastCalledWith("p-1", "c0ffee0000000000", "e7f3b10c9d8e7f6a", "internal/payment/repo.go", null);
    // The old side is the checkpoint, not the branch's base: it takes no comments here.
    await user.click(screen.getByRole("button", { name: "old line 1" }));
    expect(screen.queryByRole("textbox")).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Whole branch" }));
    expect(fileList().getAllByRole("option")).toHaveLength(7);
  });
});

describe("findings and checks in the Diff", () => {
  const signal = "finding:f-1";

  beforeEach(() => {
    vi.mocked(getInsights).mockResolvedValue(
      makeInsights({ findings: [makeFinding("f-1", { title: "Refund is never recorded" })] }),
    );
    vi.mocked(getQuality).mockResolvedValue({
      kind: "report",
      report: makeQualityReport({ crossed: [], checkFindings: [makeLocatedCheck()] }),
    });
  });

  it("has the model write a finding's draft through the core, and the link it keeps decides the button", async () => {
    const draft = { signal, commentId: "c-9", kind: "line" as const };
    const written = deferred<typeof draft>();
    vi.mocked(writeFindingComment).mockReturnValue(written.promise);
    const user = await openDiff();
    await user.click(option("internal/refund/service.go"));
    const block = within(await screen.findByRole("group", { name: "Refund is never recorded" }));

    await user.click(block.getByRole("button", { name: "Write comment" }));

    const runId = vi.mocked(writeFindingComment).mock.calls[0][3];
    expect(writeFindingComment).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      { target: BRANCH, oldSha: changeSet.oldSha, newSha: changeSet.newSha },
      "f-1",
      runId,
    );
    expect(draftSignalComment).not.toHaveBeenCalled();
    expect(block.getByRole("status")).toHaveTextContent("Writing the comment…");
    // Read back once written: the review state now links the draft to its finding, on its line.
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        comments: [
          makeComment("c-9", {
            path: "internal/refund/service.go",
            line: 56,
            currentLine: 56,
            body: "The refund is paid before it is saved.\n\n**Suggestion:** Save it as pending first.",
          }),
        ],
        signalDrafts: [draft],
      }),
    );
    written.resolve(draft);
    expect(await block.findByRole("button", { name: "See draft" })).toBeInTheDocument();
    // A normal draft: under its line (and in the hidden Comments view), published only with the others.
    expect(await screen.findAllByText(/The refund is paid before it is saved\./)).not.toHaveLength(0);
    expect(writeFindingComment).toHaveBeenCalledOnce();

    // Deleted elsewhere: the link went with it, and the button comes back.
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
    await user.click(screen.getByRole("button", { name: "Reload" }));
    await vi.waitFor(() =>
      expect(
        within(screen.getByRole("group", { name: "Refund is never recorded" })).getByRole("button", {
          name: "Write comment",
        }),
      ).toBeInTheDocument(),
    );
  });

  it("keeps the toggles while the review is open, and only offers them when there is something to hide", async () => {
    const user = await openDiff();
    await user.click(option("internal/refund/service.go"));
    const findings = await screen.findByRole("checkbox", { name: /^Show model findings/ });
    expect(findings).toHaveAccessibleName("Show model findings 1 in this file");
    expect(screen.getByRole("checkbox", { name: "Show checks 1 in this file" })).toBeChecked();

    await user.click(findings);
    expect(screen.queryByRole("group", { name: "Refund is never recorded" })).not.toBeInTheDocument();
    await user.click(option("internal/refund/handler.go"));
    expect(await screen.findByRole("checkbox", { name: "Show model findings 0 in this file" })).not.toBeChecked();
    await user.click(option("internal/refund/service.go"));
    expect(await shownPath()).toBe("internal/refund/service.go");
    expect(screen.queryByRole("group", { name: "Refund is never recorded" })).not.toBeInTheDocument();
    expect(screen.getByRole("group", { name: "Static check: staticcheck SA4006, line 50" })).toBeInTheDocument();
  });
});

/** Segment titles are set once the review state is read. */
function waitForTitle(element: HTMLElement, title: RegExp) {
  return vi.waitFor(() => expect(element).toHaveAttribute("title", expect.stringMatching(title)));
}
