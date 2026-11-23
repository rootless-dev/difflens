import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { ProjectScreen } from "@/features/project/ProjectScreen";
import {
  type Branch,
  compareBranches,
  compareWorkingTree,
  fetchProject,
  type FetchResult,
  getJiraConnection,
  getProject,
  type ProjectDetail,
  removeProject,
  setBranchBase,
} from "@/lib/api";
import { apiError, deferred, makeBase, makeBranch, makeDetail, makeProject, makeSummary } from "@/test/fixtures";
import { JIRA_CONNECTED, makeJiraConnection } from "@/test/jira-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getProject: vi.fn(),
  fetchProject: vi.fn(),
  compareBranches: vi.fn(),
  compareWorkingTree: vi.fn(),
  setBranchBase: vi.fn(),
  removeProject: vi.fn(),
  getJiraConnection: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

const NOW = new Date("2026-11-16T12:00:00Z");
const DAY = 24 * 60 * 60 * 1000;
const ago = (ms: number) => new Date(NOW.getTime() - ms).toISOString();

function committed(name: string, daysAgo: number, overrides: Partial<Branch> = {}): Branch {
  return makeBranch(name, {
    head: { sha: `sha-${name}`, subject: `Work on ${name}`, authoredAt: ago(daysAgo * DAY) },
    ...overrides,
  });
}

const detail: ProjectDetail = makeDetail({
  project: makeProject({ name: "acme-api", displayPath: "~/code/acme-api", lastFetchedAt: ago(2 * 60_000) }),
  bases: ["develop", "main", "release/2.4"],
  branches: [
    committed("feature/ABC-178", 0, { isCurrent: true, ahead: 6 }),
    committed("feature/ABC-176", 1, {
      ahead: 4,
      upstream: { name: "origin/feature/ABC-176", ahead: 0, behind: 2 },
    }),
    committed("hotfix/ABC-181", 2, { base: makeBase("main", "user") }),
    committed("feature/ABC-184", 1, { location: "remote_only" }),
    committed("feature/ABC-121", 17),
    committed("spike/token-cache", 45, { card: null }),
    committed("feature/ABC-100", 60, { location: "remote_only" }),
  ],
  workingTree: { dirty: false, filesChanged: 0 },
});

const onBack = vi.fn();
const onReview = vi.fn();

function renderScreen() {
  render(<ProjectScreen projectId="p-1" projectName="acme-api" onBack={onBack} onReview={onReview} />);
}

const list = async (name: string) => within(await screen.findByRole("list", { name }));
/** Branch names of the rows in a list: the row buttons, leaving out the base selectors and the ⋮ menus. */
const rowNames = (scope: ReturnType<typeof within>) =>
  scope
    .queryAllByRole("button", { name: /^(?!Base of |Actions for ).*\// })
    .map((button: HTMLElement) => button.textContent);
const row = (name: string) => within(screen.getByRole("button", { name }).closest("li") as HTMLElement);
const comparedBranches = () => vi.mocked(compareBranches).mock.calls.map(([, branch, base]) => `${branch}@${base}`);

beforeEach(() => {
  vi.resetAllMocks();
  vi.useFakeTimers({ toFake: ["Date"], now: NOW });
  vi.mocked(getProject).mockResolvedValue(detail);
  vi.mocked(compareBranches).mockImplementation(async (_id, branch, base) => makeSummary(branch, base));
  vi.mocked(compareWorkingTree).mockResolvedValue({ headSha: "abc", filesChanged: 3, additions: 40, deletions: 2 });
  vi.mocked(fetchProject).mockResolvedValue({ fetchedAt: NOW.toISOString(), branchesUpdated: 0 });
  vi.mocked(getJiraConnection).mockResolvedValue(makeJiraConnection());
});

afterEach(() => {
  vi.useRealTimers();
});

describe("project page", () => {
  it("shows the header with the display path, languages, branch counts and last fetch", async () => {
    renderScreen();

    expect(screen.getByRole("status", { name: "Loading project" })).toBeInTheDocument();
    expect(await screen.findByText("~/code/acme-api")).toHaveAttribute("title", "/Users/dev/work/payments-api");
    expect(getProject).toHaveBeenCalledWith("p-1");
    expect(screen.getByRole("heading", { level: 1, name: "acme-api" })).toBeInTheDocument();
    expect(screen.getByText("Go")).toBeInTheDocument();
    expect(screen.getByText("5 branches, 2 only on the remote")).toBeInTheDocument();
    expect(screen.getByText(/^Fetched/)).toHaveTextContent("Fetched 2 min ago");
    expect(screen.getByRole("button", { name: "Project settings" })).toBeDisabled();
  });

  it("shows each branch's card key as text, and as a link to the card once Jira is connected", async () => {
    renderScreen();
    await list("Active");
    expect(row("feature/ABC-176").getByText("ABC-176")).not.toHaveAttribute("href");
    expect(screen.queryByRole("link", { name: "ABC-176" })).not.toBeInTheDocument();
  });

  it("links the card key to Jira without opening the review", async () => {
    const user = userEvent.setup();
    vi.mocked(getJiraConnection).mockResolvedValue(JIRA_CONNECTED);
    renderScreen();

    const link = await screen.findByRole("link", { name: "ABC-176" });
    expect(link).toHaveAttribute("href", "https://acme.atlassian.net/browse/ABC-176");
    expect(link).toHaveAttribute("title", "Open ABC-176 in Jira");
    await user.click(link);
    expect(onReview).not.toHaveBeenCalled();
    // One read of the connection for the whole page, none per row.
    expect(getJiraConnection).toHaveBeenCalledOnce();
  });

  it("groups branches into checked out, active and older, with older collapsed", async () => {
    const user = userEvent.setup();
    renderScreen();

    expect(rowNames(await list("Checked out"))).toEqual(["feature/ABC-178"]);
    expect(rowNames(await list("Active"))).toEqual(["feature/ABC-176", "hotfix/ABC-181"]);
    const older = await list("Older");
    expect(rowNames(older)).toEqual([]);

    await user.click(older.getByRole("button", { name: "Show 2 older branches" }));

    expect(rowNames(older)).toEqual(["feature/ABC-121", "spike/token-cache"]);
    await user.click(older.getByRole("button", { name: "Hide older branches" }));
    expect(rowNames(older)).toEqual([]);
  });

  it("puts a branch committed exactly 14 days ago in active and one older than that in older", async () => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({
        branches: [
          makeBranch("feat/edge", { head: { sha: "1", subject: "", authoredAt: ago(14 * DAY) } }),
          makeBranch("feat/past", { head: { sha: "2", subject: "", authoredAt: ago(14 * DAY + 1000) } }),
        ],
      }),
    );
    renderScreen();

    expect(rowNames(await list("Active"))).toEqual(["feat/edge"]);
    await user.click(screen.getByRole("button", { name: "Show 1 older branch" }));
    expect(rowNames(await list("Older"))).toEqual(["feat/past"]);
  });

  it("never lists integration branches, and shows them at the bottom", async () => {
    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      branches: [...detail.branches, committed("develop", 0), committed("release/2.4", 0)],
    });
    renderScreen();

    await screen.findByRole("list", { name: "Active" });
    expect(screen.queryByRole("button", { name: "develop" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "release/2.4" })).not.toBeInTheDocument();
    expect(screen.getByText("release/*", { selector: "code" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Change in Project settings" })).toBeDisabled();
  });

  it("filters by branch name, searching older branches without expanding them", async () => {
    const user = userEvent.setup();
    renderScreen();

    await user.type(await screen.findByRole("searchbox", { name: "Filter branches" }), "token");

    expect(rowNames(await list("Older"))).toEqual(["spike/token-cache"]);
    expect(screen.queryByRole("button", { name: /older branches/ })).not.toBeInTheDocument();
    expect(screen.getByText("No active branch matches.")).toBeInTheDocument();
    expect(screen.getByText("The checked out branch does not match the filter.")).toBeInTheDocument();
  });

  it("filters by card, ignoring case", async () => {
    const user = userEvent.setup();
    renderScreen();

    await user.type(await screen.findByRole("searchbox", { name: "Filter branches" }), "abc-121");

    expect(rowNames(await list("Older"))).toEqual(["feature/ABC-121"]);
    expect(rowNames(await list("Active"))).toEqual([]);
  });

  it("lists remote-only branches only when asked, marked as such", async () => {
    const user = userEvent.setup();
    renderScreen();
    const onMachine = await screen.findByRole("button", { name: "On this machine" });
    const alsoRemote = screen.getByRole("button", { name: "Also on the remote" });
    expect(onMachine).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("button", { name: "feature/ABC-184" })).not.toBeInTheDocument();

    await user.click(alsoRemote);

    expect(alsoRemote).toHaveAttribute("aria-pressed", "true");
    expect(rowNames(await list("Active"))).toEqual(["feature/ABC-176", "hotfix/ABC-181", "feature/ABC-184"]);
    expect(row("feature/ABC-184").getByText("remote only")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Show 3 older branches" })).toBeInTheDocument();
  });

  it("shows each row's card, base, commits ahead and date, and the upstream hint", async () => {
    renderScreen();
    await screen.findByRole("list", { name: "Active" });

    const ahead = row("feature/ABC-176");
    expect(ahead.getByText("ABC-176")).toBeInTheDocument();
    // The pill names the ref the review compares with, origin's develop.
    expect(ahead.getByRole("button", { name: "Base of feature/ABC-176: origin/develop" })).toHaveTextContent(
      "into origin/develop",
    );
    expect(ahead.getByText("4 commits")).toBeInTheDocument();
    expect(ahead.getByText("15 Nov")).toBeInTheDocument();
    expect(ahead.getByText("2 new commits on origin")).toBeInTheDocument();
    expect(row("feature/ABC-178").queryByText(/new commit/)).not.toBeInTheDocument();
  });

  it("opens the review of a branch against its current base from its row", async () => {
    const user = userEvent.setup();
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "hotfix/ABC-181" }));

    expect(onReview).toHaveBeenCalledExactlyOnceWith({ kind: "branch", branch: "hotfix/ABC-181", base: "main" });
  });

  it("opens the review from the keyboard, with the base just chosen", async () => {
    const user = userEvent.setup();
    vi.mocked(setBranchBase).mockResolvedValue(
      committed("feature/ABC-178", 0, { isCurrent: true, ahead: 6, base: makeBase("release/2.4", "user") }),
    );
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Base of feature/ABC-178: origin/develop" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "release/2.4" }));
    await screen.findByRole("button", { name: "Base of feature/ABC-178: origin/release/2.4, chosen by you" });
    const button = screen.getByRole("button", { name: "feature/ABC-178" });
    button.focus();
    await user.keyboard("{Enter}");

    expect(onReview).toHaveBeenCalledExactlyOnceWith({
      kind: "branch",
      branch: "feature/ABC-178",
      base: "release/2.4",
    });
  });

  it("cannot review a branch without a base", async () => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({ branches: [committed("feat/lonely", 1, { base: null })], bases: [] }),
    );
    renderScreen();

    const button = await screen.findByRole("button", { name: "feat/lonely" });
    await user.click(button);

    expect(button).toHaveAttribute("aria-disabled", "true");
    expect(button).toHaveAttribute("title", "Nothing to compare with: this repository has no integration branch.");
    expect(onReview).not.toHaveBeenCalled();
  });

  it("shows the error, retries, and still allows removing when the project cannot be loaded", async () => {
    vi.mocked(getProject).mockRejectedValueOnce(apiError("path_not_found", "/work/acme-api was not found."));
    renderScreen();

    expect(await screen.findByRole("alert")).toHaveTextContent("/work/acme-api was not found.");
    expect(screen.getByRole("button", { name: "Remove from DiffLens" })).toBeInTheDocument();

    await userEvent.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByText("~/code/acme-api")).toBeInTheDocument();
  });

  it("marks a project whose language has no analyzer as diff only", async () => {
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({ project: makeProject({ languages: ["typescript"], analysisSupport: "diff_only" }) }),
    );
    renderScreen();

    expect(await screen.findByText("Diff only")).toBeInTheDocument();
    expect(screen.getByText(/Language not supported yet/)).toBeInTheDocument();
  });
});

describe("branch numbers", () => {
  it("compares only the rows on screen, with each branch's own base", async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole("list", { name: "Checked out" });

    expect(await row("feature/ABC-178").findByText(/12 files/)).toHaveTextContent("12 files +384 −57");
    expect(comparedBranches().sort()).toEqual([
      "feature/ABC-176@develop",
      "feature/ABC-178@develop",
      "hotfix/ABC-181@main",
    ]);

    await user.click(screen.getByRole("button", { name: "Show 2 older branches" }));

    await vi.waitFor(() => expect(compareBranches).toHaveBeenCalledTimes(5));
    expect(comparedBranches()).toContain("spike/token-cache@develop");
    expect(comparedBranches()).not.toContain("feature/ABC-184@develop");
  });

  it("compares an older branch matched by the filter, and does not compare again when it reappears", async () => {
    const user = userEvent.setup();
    renderScreen();
    const filter = await screen.findByRole("searchbox", { name: "Filter branches" });

    await user.type(filter, "token");
    await vi.waitFor(() => expect(comparedBranches()).toContain("spike/token-cache@develop"));
    await user.clear(filter);
    await user.type(filter, "token");

    await row("spike/token-cache").findByText(/12 files/);
    expect(comparedBranches().filter((call) => call.startsWith("spike/"))).toHaveLength(1);
  });

  it("shows a skeleton while loading and a short error per row", async () => {
    const pending = deferred<never>();
    vi.mocked(compareBranches).mockImplementation(async (_id, branch, base) => {
      if (branch === "feature/ABC-176") throw apiError("no_common_history", "No commit in common with develop.");
      if (branch === "hotfix/ABC-181") return pending.promise;
      return makeSummary(branch, base);
    });
    renderScreen();

    const failed = await screen.findByText("No common history");
    expect(failed).toHaveAttribute("title", "No commit in common with develop.");
    expect(row("hotfix/ABC-181").getByRole("status", { name: "Loading changes" })).toBeInTheDocument();
  });

  it("compares again a branch whose base moved, and only that one", async () => {
    const user = userEvent.setup();
    renderScreen();
    await screen.findByRole("button", { name: "feature/ABC-178" });
    await row("feature/ABC-178").findByText(/12 files/);
    const before = comparedBranches().length;

    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      branches: detail.branches.map((branch) =>
        branch.name === "feature/ABC-178"
          ? { ...branch, base: makeBase("develop", "history", "sha-develop-moved") }
          : branch,
      ),
    });
    await user.click(screen.getByRole("button", { name: "Fetch" }));
    await screen.findByText("Fetched: nothing new on the remote.");

    await vi.waitFor(() => expect(comparedBranches()).toHaveLength(before + 1));
    expect(comparedBranches().at(-1)).toBe("feature/ABC-178@develop");
  });

  it("does not compare a branch with nothing ahead of its base", async () => {
    vi.mocked(getProject).mockResolvedValue(makeDetail({ branches: [committed("feat/merged", 1, { ahead: 0 })] }));
    renderScreen();

    expect(await screen.findByText("Nothing new")).toBeInTheDocument();
    expect(compareBranches).not.toHaveBeenCalled();
  });
});

describe("base of a branch", () => {
  it("lists the bases, pins the chosen one and compares with it", async () => {
    const user = userEvent.setup();
    const pinned = committed("feature/ABC-176", 1, { ahead: 2, base: makeBase("release/2.4", "user") });
    vi.mocked(setBranchBase).mockResolvedValue(pinned);
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Base of feature/ABC-176: origin/develop" }));
    const options = await screen.findAllByRole("menuitemradio");
    expect(options.map((option) => option.textContent)).toEqual(["develop", "main", "release/2.4"]);
    expect(screen.queryByRole("menuitem", { name: "Use the base from history" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("menuitemradio", { name: "release/2.4" }));

    expect(setBranchBase).toHaveBeenCalledWith("p-1", "feature/ABC-176", "release/2.4");
    expect(
      await screen.findByRole("button", { name: "Base of feature/ABC-176: origin/release/2.4, chosen by you" }),
    ).toBeInTheDocument();
    await vi.waitFor(() => expect(comparedBranches()).toContain("feature/ABC-176@release/2.4"));
  });

  it("goes back to the base from history", async () => {
    const user = userEvent.setup();
    vi.mocked(setBranchBase).mockResolvedValue(committed("hotfix/ABC-181", 2));
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Base of hotfix/ABC-181: origin/main, chosen by you" }));
    await user.click(await screen.findByRole("menuitem", { name: "Use the base from history" }));

    expect(setBranchBase).toHaveBeenCalledWith("p-1", "hotfix/ABC-181", null);
    expect(await screen.findByRole("button", { name: "Base of hotfix/ABC-181: origin/develop" })).toBeInTheDocument();
  });

  it("does not compare again a base already known when switching back", async () => {
    const user = userEvent.setup();
    vi.mocked(setBranchBase)
      .mockResolvedValueOnce(committed("hotfix/ABC-181", 2))
      .mockResolvedValueOnce(committed("hotfix/ABC-181", 2, { base: makeBase("main", "user") }));
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Base of hotfix/ABC-181: origin/main, chosen by you" }));
    await user.click(await screen.findByRole("menuitem", { name: "Use the base from history" }));
    await user.click(await screen.findByRole("button", { name: "Base of hotfix/ABC-181: origin/develop" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "main" }));

    await screen.findByRole("button", { name: "Base of hotfix/ABC-181: origin/main, chosen by you" });
    expect(comparedBranches().filter((call) => call.startsWith("hotfix/"))).toEqual([
      "hotfix/ABC-181@main",
      "hotfix/ABC-181@develop",
    ]);
  });

  it("shows the error inline when the base cannot be changed", async () => {
    const user = userEvent.setup();
    vi.mocked(setBranchBase).mockRejectedValue(apiError("unknown_branch", "Branch feature/ABC-176 no longer exists."));
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Base of feature/ABC-176: origin/develop" }));
    await user.click(await screen.findByRole("menuitemradio", { name: "main" }));

    expect(await row("feature/ABC-176").findByRole("alert")).toHaveTextContent(
      "Could not change the base: Branch feature/ABC-176 no longer exists.",
    );
  });

  it("can be opened and chosen with the keyboard", async () => {
    const user = userEvent.setup();
    vi.mocked(setBranchBase).mockResolvedValue(committed("feature/ABC-176", 1, { base: makeBase("main", "user") }));
    renderScreen();

    (await screen.findByRole("button", { name: "Base of feature/ABC-176: origin/develop" })).focus();
    await user.keyboard("{Enter}");
    const develop = await screen.findByRole("menuitemradio", { name: "develop" });
    await vi.waitFor(() => expect(develop).toHaveFocus());
    await user.keyboard("{ArrowDown}");
    expect(screen.getByRole("menuitemradio", { name: "main" })).toHaveFocus();
    await user.keyboard("{Enter}");

    await vi.waitFor(() => expect(setBranchBase).toHaveBeenCalledWith("p-1", "feature/ABC-176", "main"));
  });
});

describe("uncommitted changes", () => {
  it("shows the working tree row with numbers from the working tree comparison", async () => {
    vi.mocked(getProject).mockResolvedValue({ ...detail, workingTree: { dirty: true, filesChanged: 3 } });
    renderScreen();

    const changes = within(await screen.findByRole("listitem", { name: "Uncommitted changes" }));
    expect(changes.getByText(/changed in your working tree/)).toHaveTextContent(
      "3 files changed in your working tree and not committed yet",
    );
    expect(await changes.findByText("+40")).toBeInTheDocument();
    expect(changes.getByText("−2")).toBeInTheDocument();
    expect(compareWorkingTree).toHaveBeenCalledWith("p-1");
  });

  it("opens the review of the uncommitted changes", async () => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue({ ...detail, workingTree: { dirty: true, filesChanged: 3 } });
    renderScreen();

    const changes = within(await screen.findByRole("listitem", { name: "Uncommitted changes" }));
    await user.click(changes.getByRole("button", { name: "Review them" }));

    expect(onReview).toHaveBeenCalledExactlyOnceWith({ kind: "working_tree" });
  });

  it("leaves the row out when the working tree is clean", async () => {
    renderScreen();

    await screen.findByRole("list", { name: "Checked out" });
    expect(screen.queryByRole("listitem", { name: "Uncommitted changes" })).not.toBeInTheDocument();
    expect(compareWorkingTree).not.toHaveBeenCalled();
  });

  it("still shows the uncommitted changes when no branch to review is checked out", async () => {
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({ branches: [committed("feat/a", 1)], workingTree: { dirty: true, filesChanged: 1 } }),
    );
    renderScreen();

    expect(await screen.findByText("No branch to review is checked out.")).toBeInTheDocument();
    expect(screen.getByRole("listitem", { name: "Uncommitted changes" })).toBeInTheDocument();
  });
});

describe("fetch", () => {
  it("fetches on demand, reloads the branches and says how many were updated", async () => {
    const user = userEvent.setup();
    const fetching = deferred<FetchResult>();
    vi.mocked(fetchProject).mockReturnValue(fetching.promise);
    renderScreen();
    await user.click(await screen.findByRole("button", { name: "Fetch" }));

    expect(fetchProject).toHaveBeenCalledWith("p-1");
    expect(screen.getByRole("button", { name: "Fetching…" })).toBeDisabled();
    expect(screen.getByRole("list", { name: "Active" })).toBeInTheDocument();

    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      project: { ...detail.project, lastFetchedAt: NOW.toISOString() },
      branches: [...detail.branches, committed("feature/ABC-200", 0)],
    });
    fetching.resolve({ fetchedAt: NOW.toISOString(), branchesUpdated: 3 });

    expect(await screen.findByText("Fetched: 3 branches updated.")).toBeInTheDocument();
    expect(getProject).toHaveBeenCalledTimes(2);
    expect(screen.getByRole("button", { name: "feature/ABC-200" })).toBeInTheDocument();
    expect(screen.getByText("just now", { selector: "time" }).parentElement).toHaveTextContent("Fetched just now");
    expect(screen.getByRole("button", { name: "Fetch" })).toBeEnabled();
  });

  it.each([
    ["no_remote", "This repository has no remote to fetch from."],
    ["fetch_failed", "git fetch failed: could not resolve host."],
  ] as const)("shows the %s error inline", async (code, message) => {
    const user = userEvent.setup();
    vi.mocked(fetchProject).mockRejectedValue(apiError(code, message));
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "Fetch" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(message);
    expect(screen.getByRole("button", { name: "Fetch" })).toBeEnabled();
    expect(getProject).toHaveBeenCalledTimes(1);
  });

  it("fetches automatically, without blocking the page, when the last fetch is stale", async () => {
    const fetching = deferred<FetchResult>();
    vi.mocked(fetchProject).mockReturnValue(fetching.promise);
    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      project: { ...detail.project, lastFetchedAt: ago(30 * 60_000) },
    });
    renderScreen();

    expect(await screen.findByRole("button", { name: "feature/ABC-178" })).toBeInTheDocument();
    expect(fetchProject).toHaveBeenCalledTimes(1);
    expect(screen.getByRole("button", { name: "Fetching…" })).toBeDisabled();
    expect(screen.getByText(/^Fetched/)).toHaveTextContent("Fetched 30 min ago");

    fetching.resolve({ fetchedAt: NOW.toISOString(), branchesUpdated: 0 });

    expect(await screen.findByText("Fetched: nothing new on the remote.")).toBeInTheDocument();
    expect(getProject).toHaveBeenCalledTimes(2);
    expect(fetchProject).toHaveBeenCalledTimes(1);
  });

  it("fetches automatically a project that was never fetched", async () => {
    vi.mocked(getProject).mockResolvedValue({ ...detail, project: { ...detail.project, lastFetchedAt: null } });
    renderScreen();

    expect(await screen.findByText("Never fetched")).toBeInTheDocument();
    await vi.waitFor(() => expect(fetchProject).toHaveBeenCalledTimes(1));
  });

  it("does not fetch automatically when the last fetch is recent", async () => {
    renderScreen();

    await screen.findByRole("button", { name: "feature/ABC-178" });
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchProject).not.toHaveBeenCalled();
  });

  it("does not fetch an unavailable project, automatically or by hand", async () => {
    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      project: { ...detail.project, available: false, lastFetchedAt: null },
    });
    renderScreen();

    expect(await screen.findByRole("button", { name: "Fetch" })).toBeDisabled();
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchProject).not.toHaveBeenCalled();
  });

  it("does not fetch a repository without a remote, automatically or by hand", async () => {
    vi.mocked(getProject).mockResolvedValue({
      ...detail,
      project: { ...detail.project, hasRemote: false, lastFetchedAt: null },
    });
    renderScreen();

    const button = await screen.findByRole("button", { name: "Fetch" });
    expect(button).toBeDisabled();
    expect(button).toHaveAttribute("title", "This repository has no remote");
    await new Promise((resolve) => setTimeout(resolve, 20));
    expect(fetchProject).not.toHaveBeenCalled();
  });
});

describe("remove from DiffLens", () => {
  async function openDialog() {
    const user = userEvent.setup();
    renderScreen();
    await user.click(await screen.findByRole("button", { name: "Remove from DiffLens" }));
    return { user, dialog: within(await screen.findByRole("alertdialog")) };
  }

  it("asks for confirmation before removing", async () => {
    const { user, dialog } = await openDialog();

    expect(dialog.getByText("Remove acme-api from DiffLens?")).toBeInTheDocument();
    expect(removeProject).not.toHaveBeenCalled();

    await user.click(dialog.getByRole("button", { name: "Cancel" }));

    expect(removeProject).not.toHaveBeenCalled();
    expect(onBack).not.toHaveBeenCalled();
  });

  it("removes the project and goes back to the list once confirmed", async () => {
    vi.mocked(removeProject).mockResolvedValue(undefined);
    const { user, dialog } = await openDialog();

    await user.click(dialog.getByRole("button", { name: "Remove project" }));

    expect(removeProject).toHaveBeenCalledWith("p-1");
    await vi.waitFor(() => expect(onBack).toHaveBeenCalledTimes(1));
  });

  it("shows the API error in the dialog and stays on the page", async () => {
    vi.mocked(removeProject).mockRejectedValue(apiError("internal", "Could not update the project list."));
    const { user, dialog } = await openDialog();

    await user.click(dialog.getByRole("button", { name: "Remove project" }));

    expect(await dialog.findByRole("alert")).toHaveTextContent("Could not update the project list.");
    expect(onBack).not.toHaveBeenCalled();
    expect(dialog.getByRole("button", { name: "Remove project" })).toBeEnabled();
  });
});
