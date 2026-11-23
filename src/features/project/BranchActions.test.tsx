import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { FullAnalysesProvider } from "@/features/full-analysis/full-analyses";
import { ProjectScreen } from "@/features/project/ProjectScreen";
import { setLanguage } from "@/i18n";
import {
  type BranchBase,
  compareBranches,
  type FullAnalysisEvent,
  type FullAnalysisJob,
  getFullAnalyses,
  getJiraConnection,
  getProject,
  linkPullRequest,
  listChangedFiles,
  listenToFullAnalyses,
  planFullAnalysis,
  type ReviewTarget,
  startFullAnalysis,
  type Worktree,
} from "@/lib/api";
import { makeFolder, makeWorktree } from "@/test/terminal-fixtures";
import { apiError, makeBase, makeBranch, makeChangeSet, makeDetail, makeSummary } from "@/test/fixtures";
import { makeFullAnalysisPlan, makeJob } from "@/test/full-analysis-fixtures";
import { makeModelInUse } from "@/test/insights-fixtures";
import { makeJiraConnection } from "@/test/jira-fixtures";
import { makeLinked } from "@/test/pull-request-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getProject: vi.fn(),
  fetchProject: vi.fn(),
  compareBranches: vi.fn(),
  compareWorkingTree: vi.fn(),
  getJiraConnection: vi.fn(),
  listChangedFiles: vi.fn(),
  linkPullRequest: vi.fn(),
  getFullAnalyses: vi.fn(),
  listenToFullAnalyses: vi.fn(),
  planFullAnalysis: vi.fn(),
  startFullAnalysis: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

// The branch of `makeJob`, so the jobs of the fixtures are its full analyses.
const TARGET: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const changeSet = makeChangeSet([], { target: TARGET });
const REVIEW = { target: TARGET, oldSha: changeSet.oldSha, newSha: changeSet.newSha };

const onReview = vi.fn();
const onOpenPreferences = vi.fn();
/** Sends an event as the core would. */
let emit: (event: FullAnalysisEvent) => void = () => {};

function renderScreen() {
  render(
    <FullAnalysesProvider>
      <ProjectScreen
        projectId="p-1"
        projectName="payments-api"
        onBack={() => {}}
        onReview={onReview}
        onOpenPreferences={onOpenPreferences}
      />
    </FullAnalysesProvider>,
  );
}

const actionsButton = () => screen.findByRole("button", { name: "Actions for feat/refund-flow" });

async function openMenu(user: ReturnType<typeof userEvent.setup>) {
  await user.click(await actionsButton());
  return within(await screen.findByRole("menu"));
}

/** The row's status cell: the column right before the date. */
const rowStatus = () =>
  within(screen.getByRole("button", { name: "feat/refund-flow" }).closest("li") as HTMLElement).queryByRole("status");

function emitJob(job: FullAnalysisJob) {
  const { log: _log, ...summary } = job;
  act(() => emit({ job: summary, line: null }));
}

/** `feat/refund-flow`, committed to just now so it is listed among the active branches. */
const branch = (base: BranchBase | null = makeBase("main")) =>
  makeBranch("feat/refund-flow", { base, head: { sha: "1", subject: "", authoredAt: new Date().toISOString() } });

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getProject).mockResolvedValue(makeDetail({ branches: [branch()] }));
  vi.mocked(compareBranches).mockImplementation(async (_id, branch, base) => makeSummary(branch, base));
  vi.mocked(getJiraConnection).mockResolvedValue(makeJiraConnection());
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
  vi.mocked(getFullAnalyses).mockResolvedValue([]);
  vi.mocked(listenToFullAnalyses).mockImplementation((handler) => {
    emit = handler;
    return Promise.resolve(() => {});
  });
});

describe("the actions of a branch row", () => {
  it("opens its menu from the ⋮ without opening the review, and Escape gives the focus back", async () => {
    const user = userEvent.setup();
    renderScreen();

    const menu = await openMenu(user);
    expect(menu.getByRole("menuitem", { name: "Full analysis…" })).toBeInTheDocument();
    expect(menu.getAllByRole("menuitem").map((item) => item.textContent)).toEqual([
      "Full analysis…",
      "Overview",
      "Tests",
      "Insights",
      "Comments",
      "Open terminalasks to create a worktree",
      "Open in Claude Codeasks to create a worktree",
    ]);
    expect(onReview).not.toHaveBeenCalled();
    expect(listChangedFiles).not.toHaveBeenCalled();

    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("menu")).not.toBeInTheDocument());
    expect(await actionsButton()).toHaveFocus();
    expect(onReview).not.toHaveBeenCalled();
  });

  it("names the ⋮ with the branch, in Portuguese too", async () => {
    setLanguage("pt-BR");
    renderScreen();
    expect(await screen.findByRole("button", { name: "Ações de feat/refund-flow" })).toBeInTheDocument();
  });

  it.each([
    ["Overview", { view: "Overview" }],
    ["Tests", { view: "Tests" }],
    ["Insights", { view: "Insights" }],
    ["Comments", { view: "Diff", diffMode: "comments" }],
  ])("opens the review on %s from its shortcut", async (name, opening) => {
    const user = userEvent.setup();
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name }));

    expect(onReview).toHaveBeenCalledExactlyOnceWith(TARGET, opening);
  });

  it.each([
    ["Open terminal", "shell"],
    ["Open in Claude Code", "claude"],
  ])("%s opens the review with a session of its kind", async (name, kind) => {
    const user = userEvent.setup();
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: new RegExp(`^${name}`) }));

    expect(onReview).toHaveBeenCalledExactlyOnceWith(TARGET, undefined, kind);
  });

  it("says under the terminal items where they open", async () => {
    const user = userEvent.setup();
    const where = async (worktrees: Worktree[]) => {
      vi.mocked(getProject).mockResolvedValue(makeDetail({ branches: [branch()], worktrees }));
      const view = render(
        <FullAnalysesProvider>
          <ProjectScreen projectId="p-1" projectName="payments-api" onBack={() => {}} onReview={onReview} />
        </FullAnalysesProvider>,
      );
      const menu = await openMenu(user);
      const texts = ["Open terminal", "Open in Claude Code"].map(
        (name) => menu.getByRole("menuitem", { name: new RegExp(`^${name}`) }).querySelector("small")?.textContent,
      );
      await user.keyboard("{Escape}");
      view.unmount();
      return texts;
    };

    expect(await where([])).toEqual(["asks to create a worktree", "asks to create a worktree"]);
    expect(await where([makeWorktree("feat/refund-flow")])).toEqual([
      ".difflens/worktrees/feat/refund-flow",
      ".difflens/worktrees/feat/refund-flow",
    ]);
    expect(
      await where([{ folder: makeFolder(), branch: "feat/refund-flow", createdAt: null, changedFiles: null }]),
    ).toEqual(["in the project folder", "in the project folder"]);

    setLanguage("pt-BR");
    vi.mocked(getProject).mockResolvedValue(makeDetail({ branches: [branch()] }));
    renderScreen();
    await user.click(await screen.findByRole("button", { name: "Ações de feat/refund-flow" }));
    const item = within(await screen.findByRole("menu")).getByRole("menuitem", { name: /^Abrir no Claude Code/ });
    expect(item).toHaveTextContent("pede para criar uma worktree");
  });

  it("still opens the review on its Overview from the rest of the row", async () => {
    const user = userEvent.setup();
    renderScreen();

    await user.click(await screen.findByRole("button", { name: "feat/refund-flow" }));

    expect(onReview).toHaveBeenCalledExactlyOnceWith(TARGET);
  });

  it("cannot open the menu of a branch without a base", async () => {
    vi.mocked(getProject).mockResolvedValue(makeDetail({ branches: [branch(null)], bases: [] }));
    renderScreen();
    expect(await actionsButton()).toBeDisabled();
  });
});

describe("a full analysis from a branch row", () => {
  it("shows the Overview's confirmation in a dialog, and starts only on its button", async () => {
    const user = userEvent.setup();
    vi.mocked(linkPullRequest).mockResolvedValue(makeLinked());
    vi.mocked(planFullAnalysis).mockResolvedValue(makeFullAnalysisPlan({ running: 4, limit: 4 }));
    vi.mocked(startFullAnalysis).mockResolvedValue(makeJob({ status: "queued", progress: 0 }));
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));

    const dialog = within(await screen.findByRole("dialog", { name: "Full analysis of feat/refund-flow" }));
    const lines = within(await dialog.findByRole("list", { name: "Full analysis" }));
    expect(lines.getByText("Tests run in the sandbox, with coverage.")).toBeInTheDocument();
    expect(
      lines.getByText("Insights: 60 files in 3 calls, about 90k tokens, with Claude Code, model opus."),
    ).toBeInTheDocument();
    expect(dialog.getByText(/sent to Claude Code, model opus/)).toBeInTheDocument();
    expect(dialog.getByText(/4 of 4 full analyses are running/)).toBeInTheDocument();
    // Read as opening the review reads it: the change set, then its pull request.
    expect(listChangedFiles).toHaveBeenCalledExactlyOnceWith("p-1", TARGET);
    expect(linkPullRequest).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);
    const pullRequest = { title: "Adds the refund flow", description: "Refunds settled payments." };
    expect(planFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, pullRequest);
    expect(startFullAnalysis).not.toHaveBeenCalled();
    expect(onReview).not.toHaveBeenCalled();

    await user.click(dialog.getByRole("button", { name: "Start full analysis" }));

    expect(startFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, pullRequest);
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(onReview).not.toHaveBeenCalled();
    // Queued: the row says so, and its ⋮ spins.
    expect(rowStatus()).toHaveTextContent("Queued");
    const button = await actionsButton();
    expect(button).toHaveAttribute("aria-busy", "true");
    await waitFor(() => expect(button).toHaveFocus());
  });

  it("is offered without a pull request when none can be read", async () => {
    const user = userEvent.setup();
    vi.mocked(linkPullRequest).mockRejectedValue(apiError("network_failed", "No network."));
    vi.mocked(planFullAnalysis).mockResolvedValue(makeFullAnalysisPlan());
    vi.mocked(startFullAnalysis).mockResolvedValue(makeJob({ status: "queued", progress: 0 }));
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));
    await user.click(await screen.findByRole("button", { name: "Start full analysis" }));

    expect(planFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null);
    expect(startFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null);
  });

  it("closes on Cancel and on Escape without starting, giving the focus back to the ⋮", async () => {
    const user = userEvent.setup();
    vi.mocked(planFullAnalysis).mockResolvedValue(makeFullAnalysisPlan());
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));
    const dialog = await screen.findByRole("dialog");
    await user.click(await within(dialog).findByRole("button", { name: "Cancel" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(async () => expect(await actionsButton()).toHaveFocus());

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));
    await within(await screen.findByRole("dialog")).findByRole("button", { name: "Start full analysis" });
    await user.keyboard("{Escape}");
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    await waitFor(async () => expect(await actionsButton()).toHaveFocus());

    expect(startFullAnalysis).not.toHaveBeenCalled();
  });

  it("says what is wrong when the review cannot be read", async () => {
    const user = userEvent.setup();
    vi.mocked(listChangedFiles).mockRejectedValue(apiError("no_common_history", "No common history."));
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));

    const dialog = within(await screen.findByRole("dialog"));
    expect(
      await dialog.findByText("Could not work out what a full analysis would do: No common history."),
    ).toBeInTheDocument();
    expect(planFullAnalysis).not.toHaveBeenCalled();
    await user.click(dialog.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });

  it("does not offer to start when no model is set up, and leads to Preferences", async () => {
    const user = userEvent.setup();
    const missing = makeModelInUse({ ready: false, problem: "Save an API key." });
    vi.mocked(planFullAnalysis).mockResolvedValue(
      makeFullAnalysisPlan({}, { insights: missing, summary: missing, explanations: missing }),
    );
    renderScreen();

    await user.click((await openMenu(user)).getByRole("menuitem", { name: "Full analysis…" }));

    const dialog = within(await screen.findByRole("dialog"));
    expect(await dialog.findByText("No model is set up. Save an API key.")).toBeInTheDocument();
    expect(dialog.queryByRole("button", { name: "Start full analysis" })).not.toBeInTheDocument();
    await user.click(dialog.getByRole("button", { name: "Open Preferences" }));
    expect(onOpenPreferences).toHaveBeenCalledOnce();
  });

  it.each([
    ["queued", "Full analysis queued"],
    ["running", "Full analysis running"],
  ] as const)("does not offer a second one while one is %s", async (status, label) => {
    const user = userEvent.setup();
    vi.mocked(getFullAnalyses).mockResolvedValue([makeJob({ status })]);
    renderScreen();

    await waitFor(async () => expect(await actionsButton()).toHaveAttribute("aria-busy", "true"));
    const item = (await openMenu(user)).getByRole("menuitem", { name: label });
    expect(item).toHaveAttribute("aria-disabled", "true");
    await user.click(item);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(planFullAnalysis).not.toHaveBeenCalled();
  });
});

describe("the status of a branch's full analysis", () => {
  it("follows the core's events: queued, its percent, then how it ended", async () => {
    renderScreen();
    await actionsButton();
    expect(rowStatus()).not.toBeInTheDocument();

    emitJob(makeJob({ status: "queued", progress: 0 }));
    expect(rowStatus()).toHaveTextContent("Queued");
    expect(rowStatus()).toHaveAttribute("title", "Full analysis: Queued");

    emitJob(makeJob({ progress: 42 }));
    expect(rowStatus()).toHaveTextContent("42%");
    expect(await actionsButton()).toHaveAttribute("aria-busy", "true");

    emitJob(makeJob({ status: "finished", outcome: "succeeded", progress: 100 }));
    expect(rowStatus()).toHaveTextContent("Analyzed");
    expect(await actionsButton()).not.toHaveAttribute("aria-busy");

    emitJob(makeJob({ status: "finished", outcome: "failed", progress: 100 }));
    expect(rowStatus()).toHaveTextContent("Failed");

    emitJob(makeJob({ status: "finished", outcome: "cancelled", progress: 30 }));
    expect(rowStatus()).toHaveTextContent("Cancelled");
  });

  it("is worded in Portuguese", async () => {
    setLanguage("pt-BR");
    renderScreen();
    await screen.findByRole("button", { name: "Ações de feat/refund-flow" });

    emitJob(makeJob({ status: "queued", progress: 0 }));
    expect(rowStatus()).toHaveTextContent("Na fila");
    emitJob(makeJob({ status: "finished", outcome: "succeeded", progress: 100 }));
    expect(rowStatus()).toHaveTextContent("Analisada");
    emitJob(makeJob({ status: "finished", outcome: "failed", progress: 100 }));
    expect(rowStatus()).toHaveTextContent("Falhou");
  });
});
