import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import App from "@/App";
import {
  compareBranches,
  compareWorkingTree,
  fetchProject,
  getFileVersions,
  getJiraConnection,
  getProject,
  getReviewState,
  getConnections,
  getModelSettings,
  getTerminalLocation,
  getTestsState,
  linkPullRequest,
  listChangedFiles,
  listProjects,
  listTerminals,
} from "@/lib/api";
import { makeLocation } from "@/test/terminal-fixtures";
import { makeBranch, makeChangedFile, makeChangeSet, makeDetail, makeProject, makeSummary, textSide } from "@/test/fixtures";
import { makeModelSettings } from "@/test/model-fixtures";
import { makeJiraConnection } from "@/test/jira-fixtures";
import { makeConnection } from "@/test/pull-request-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead, reviewTab } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listProjects: vi.fn(),
  getProject: vi.fn(),
  fetchProject: vi.fn(),
  compareBranches: vi.fn(),
  compareWorkingTree: vi.fn(),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  getConnections: vi.fn(),
  getModelSettings: vi.fn(),
  linkPullRequest: vi.fn(),
  getJiraConnection: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
  listTerminals: vi.fn(),
  getTerminalLocation: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-dialog", () => ({ open: vi.fn() }));
vi.mock("@/features/review/DiffEditor", () => ({ default: () => <div data-testid="diff-editor" /> }));

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening("feat/refund-flow");
  window.localStorage.clear();
  document.documentElement.className = "dark";
  vi.mocked(listProjects).mockResolvedValue([makeProject()]);
  vi.mocked(getProject).mockResolvedValue(makeDetail());
  vi.mocked(fetchProject).mockResolvedValue({ fetchedAt: "2026-11-16T12:00:00Z", branchesUpdated: 0 });
  vi.mocked(compareBranches).mockImplementation(async (_id, branch, base) => makeSummary(branch, base));
  vi.mocked(compareWorkingTree).mockResolvedValue({ headSha: "abc", filesChanged: 1, additions: 2, deletions: 0 });
  vi.mocked(listChangedFiles).mockImplementation(async (_id, target) =>
    makeChangeSet([makeChangedFile("main.go")], { target, newSha: target.kind === "branch" ? "e7f3b10c9d8e" : null }),
  );
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a"), new: textSide("b") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
  vi.mocked(getModelSettings).mockResolvedValue(makeModelSettings());
  vi.mocked(getJiraConnection).mockResolvedValue(makeJiraConnection());
  vi.mocked(listTerminals).mockResolvedValue([]);
  vi.mocked(getTerminalLocation).mockResolvedValue(makeLocation());
});

const breadcrumb = () => within(screen.getByRole("navigation", { name: "Breadcrumb" }));

async function openProject(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await screen.findByRole("button", { name: /payments-api/ }));
  await screen.findByRole("heading", { level: 1, name: "payments-api" });
}

describe("app shell", () => {
  it("opens Preferences from the top bar and goes back to where it was", async () => {
    const user = userEvent.setup();
    vi.mocked(getConnections).mockResolvedValue([makeConnection()]);
    render(<App />);
    await openProject(user);

    await user.click(within(screen.getByRole("banner")).getByRole("button", { name: "Preferences" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Preferences" })).toBeInTheDocument();
    expect(await screen.findByRole("form", { name: "Bitbucket Cloud" })).toBeInTheDocument();
    expect(within(screen.getByRole("banner")).getByRole("button", { name: "Preferences" })).toHaveAttribute(
      "aria-current",
      "page",
    );

    await user.click(screen.getByRole("button", { name: "← Back" }));
    expect(await screen.findByRole("heading", { level: 1, name: "payments-api" })).toBeInTheDocument();
  });

  it("navigates into a project and back through the breadcrumb", async () => {
    const user = userEvent.setup();
    render(<App />);

    await openProject(user);

    expect(breadcrumb().getByRole("button", { name: "payments-api" })).toHaveAttribute("aria-current", "page");

    await user.click(breadcrumb().getByRole("button", { name: "Projects" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Projects" })).toBeInTheDocument();
    expect(breadcrumb().queryByRole("button", { name: "payments-api" })).not.toBeInTheDocument();
  });

  it("opens the review of a branch from its row, and goes back to the project through the breadcrumb", async () => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({ branches: [makeBranch("feat/refund-flow", { head: { sha: "1", subject: "", authoredAt: new Date().toISOString() } })] }),
    );
    render(<App />);
    await openProject(user);

    await user.click(await screen.findByRole("button", { name: "feat/refund-flow" }));

    await changedFilesRead();
    expect(reviewTab("Overview")).toHaveAttribute("aria-selected", "true");
    expect(listChangedFiles).toHaveBeenCalledExactlyOnceWith("p-1", {
      kind: "branch",
      branch: "feat/refund-flow",
      base: "develop",
    });
    expect(breadcrumb().getByRole("button", { name: "feat/refund-flow" })).toHaveAttribute("aria-current", "page");
    expect(breadcrumb().getByRole("button", { name: "payments-api" })).not.toHaveAttribute("aria-current");
    // Named as the change set was read: against origin's develop.
    expect(screen.getByRole("banner")).toHaveTextContent("feat/refund-flow into origin/develop");
    expect(await within(screen.getByRole("banner")).findByText("e7f3b10")).toBeInTheDocument();

    await user.click(breadcrumb().getByRole("button", { name: "payments-api" }));

    expect(await screen.findByRole("heading", { level: 1, name: "payments-api" })).toBeInTheDocument();
    expect(breadcrumb().queryByRole("button", { name: "feat/refund-flow" })).not.toBeInTheDocument();
    expect(screen.getByRole("banner")).not.toHaveTextContent("into develop");
  });

  it.each([
    ["Overview", "Overview"],
    ["Tests", "Tests"],
    ["Insights", "Insights"],
    ["Comments", "Diff"],
  ])("opens the review on a tab from the %s shortcut of a branch row", async (shortcut, tab) => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue(
      makeDetail({
        branches: [
          makeBranch("feat/refund-flow", { head: { sha: "1", subject: "", authoredAt: new Date().toISOString() } }),
        ],
      }),
    );
    render(<App />);
    await openProject(user);

    await user.click(await screen.findByRole("button", { name: "Actions for feat/refund-flow" }));
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: shortcut }));

    await changedFilesRead();
    expect(reviewTab(tab)).toHaveAttribute("aria-selected", "true");
    expect(listChangedFiles).toHaveBeenCalledExactlyOnceWith("p-1", {
      kind: "branch",
      branch: "feat/refund-flow",
      base: "develop",
    });
    if (shortcut === "Comments") {
      // The Diff on its comments, not its files.
      expect(screen.getByRole("button", { name: /^Comments/, pressed: true })).toBeInTheDocument();
      expect(await screen.findByRole("region", { name: "Comments" })).toBeInTheDocument();
    } else {
      expect(screen.queryByRole("region", { name: "Comments" })).not.toBeInTheDocument();
    }
  });

  it("opens the review of the uncommitted changes from the working tree row", async () => {
    const user = userEvent.setup();
    vi.mocked(getProject).mockResolvedValue(makeDetail({ workingTree: { dirty: true, filesChanged: 1 } }));
    render(<App />);
    await openProject(user);

    await user.click(screen.getByRole("button", { name: "Review them" }));

    await changedFilesRead();
    expect(reviewTab("Overview")).toHaveAttribute("aria-selected", "true");
    expect(listChangedFiles).toHaveBeenCalledExactlyOnceWith("p-1", { kind: "working_tree" });
    expect(breadcrumb().getByRole("button", { name: "Uncommitted changes" })).toHaveAttribute("aria-current", "page");
    expect(screen.getByRole("banner")).toHaveTextContent("Uncommitted changes against HEAD");
    expect(within(screen.getByRole("banner")).getByText("Working tree")).toBeInTheDocument();

    await user.click(breadcrumb().getByRole("button", { name: "Projects" }));

    expect(await screen.findByRole("heading", { level: 1, name: "Projects" })).toBeInTheDocument();
  });

  it("starts dark, switches to light and remembers the choice", async () => {
    const user = userEvent.setup();
    const { unmount } = render(<App />);

    await user.click(screen.getByRole("button", { name: "Light theme" }));

    expect(document.documentElement).not.toHaveClass("dark");
    expect(screen.getByRole("button", { name: "Dark theme" })).toBeInTheDocument();

    unmount();
    render(<App />);

    expect(screen.getByRole("button", { name: "Dark theme" })).toBeInTheDocument();
  });
});
