import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { ProjectScreen } from "@/features/project/ProjectScreen";
import { TerminalsProvider } from "@/features/terminal/terminals";
import { setLanguage } from "@/i18n";
import {
  attachTerminal,
  compareBranches,
  getJiraConnection,
  getProject,
  listTerminals,
  removeWorktree,
  type Worktree,
} from "@/lib/api";
import { apiError, makeBranch, makeDetail, makeSummary } from "@/test/fixtures";
import { makeJiraConnection } from "@/test/jira-fixtures";
import { difflensFolder, makeFolder, makeSession, makeWorktree } from "@/test/terminal-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getProject: vi.fn(),
  compareBranches: vi.fn(),
  getJiraConnection: vi.fn(),
  removeWorktree: vi.fn(),
  listTerminals: vi.fn(),
  attachTerminal: vi.fn(),
}));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

const recent = () => ({ sha: "1", subject: "", authoredAt: new Date().toISOString() });
const MAIN: Worktree = { folder: makeFolder(), branch: "main", createdAt: null, changedFiles: null };
const onReview = vi.fn();

function detailWith(worktrees: Worktree[]) {
  return makeDetail({
    branches: [makeBranch("feat/clean", { head: recent() }), makeBranch("feat/dirty", { head: recent() })],
    worktrees,
  });
}

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getProject).mockResolvedValue(
    detailWith([MAIN, makeWorktree("feat/clean"), makeWorktree("feat/dirty", { changedFiles: 3 })]),
  );
  vi.mocked(compareBranches).mockImplementation(async (_id, branch, base) => makeSummary(branch, base));
  vi.mocked(getJiraConnection).mockResolvedValue(makeJiraConnection());
  vi.mocked(listTerminals).mockResolvedValue([]);
  vi.mocked(attachTerminal).mockImplementation(async (id) => makeSession(id));
  vi.mocked(removeWorktree).mockResolvedValue(undefined);
});

function renderScreen() {
  render(
    <TerminalsProvider>
      <ProjectScreen projectId="p-1" projectName="payments-api" onBack={() => {}} onReview={onReview} />
    </TerminalsProvider>,
  );
}

const section = async () => within(await screen.findByRole("list", { name: "DiffLens worktrees" }));
const row = async (branch: string) =>
  within((await section()).getByText(branch, { selector: "code" }).closest("li") as HTMLElement);

describe("the DiffLens worktrees of the project screen", () => {
  it("lists only DiffLens's worktrees, between the branches and the integration branches", async () => {
    renderScreen();

    const list = await section();
    expect(list.getAllByRole("listitem")).toHaveLength(2);
    expect(list.queryByText("main")).not.toBeInTheDocument();
    const clean = await row("feat/clean");
    expect(clean.getByText(".difflens/worktrees/feat/clean")).toHaveAttribute("title", difflensFolder("feat/clean").path);
    expect(clean.getByText("clean")).toBeInTheDocument();
    expect(clean.getByText(/^created /)).toBeInTheDocument();
    expect(clean.getByText("no terminal")).toBeInTheDocument();
    expect((await row("feat/dirty")).getByText("uncommitted changes: 3 files")).toBeInTheDocument();

    const headings = screen.getAllByRole("heading", { level: 2 }).map((heading) => heading.textContent);
    const index = headings.findIndex((text) => text?.startsWith("DiffLens worktrees"));
    expect(headings[index - 1]).toBe("Older");
    expect(headings[index + 1]).toMatch(/^Integration branches/);
  });

  it("is not there without a DiffLens worktree", async () => {
    vi.mocked(getProject).mockResolvedValue(detailWith([MAIN]));
    renderScreen();
    await screen.findByRole("list", { name: "Active" });
    expect(screen.queryByText("DiffLens worktrees")).not.toBeInTheDocument();
  });

  it("removes a clean worktree at once and reloads the project", async () => {
    const user = userEvent.setup();
    renderScreen();
    vi.mocked(getProject).mockResolvedValue(detailWith([MAIN, makeWorktree("feat/dirty", { changedFiles: 3 })]));

    await user.click((await row("feat/clean")).getByRole("button", { name: "Remove the worktree of feat/clean" }));

    expect(removeWorktree).toHaveBeenCalledExactlyOnceWith("p-1", difflensFolder("feat/clean").path, false);
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Worktree of feat/clean removed. The branch stays in the list.",
    );
    await waitFor(async () => expect((await section()).getAllByRole("listitem")).toHaveLength(1));
    expect(getProject).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
  });

  it("asks before removing a worktree with uncommitted changes, and then forces it", async () => {
    const user = userEvent.setup();
    renderScreen();

    await user.click((await row("feat/dirty")).getByRole("button", { name: "Remove the worktree of feat/dirty" }));

    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Remove the worktree of feat/dirty?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent(
      ".difflens/worktrees/feat/dirty has 3 files with uncommitted changes, which will be lost.",
    );
    expect(dialog).toHaveTextContent("The branch feat/dirty and its commits are not changed.");
    expect(removeWorktree).not.toHaveBeenCalled();

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(removeWorktree).not.toHaveBeenCalled();

    await user.click((await row("feat/dirty")).getByRole("button", { name: "Remove the worktree of feat/dirty" }));
    await user.click(
      within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Remove and lose the changes" }),
    );
    expect(removeWorktree).toHaveBeenCalledExactlyOnceWith("p-1", difflensFolder("feat/dirty").path, true);
    expect(await screen.findByRole("status")).toHaveTextContent(
      "Worktree of feat/dirty removed, with its uncommitted changes.",
    );
  });

  it("asks when the core finds changes the list did not show", async () => {
    const user = userEvent.setup();
    vi.mocked(removeWorktree).mockRejectedValueOnce(apiError("worktree_dirty", "dirty"));
    renderScreen();

    await user.click((await row("feat/clean")).getByRole("button", { name: "Remove the worktree of feat/clean" }));

    expect(await screen.findByRole("alertdialog")).toBeInTheDocument();
  });

  it("refuses while a terminal is open in it, and offers its review", async () => {
    const user = userEvent.setup();
    vi.mocked(listTerminals).mockResolvedValue([
      makeSession("s1", { cwd: difflensFolder("feat/clean").path }),
      makeSession("s2", { name: "Claude Code", kind: "claude", cwd: difflensFolder("feat/clean").path }),
    ]);
    renderScreen();
    const clean = await row("feat/clean");
    expect(await clean.findByText("2 open terminals")).toHaveAttribute("title", "zsh, Claude Code");

    await user.click(clean.getByRole("button", { name: "Remove the worktree of feat/clean" }));

    expect(clean.getByRole("alert")).toHaveTextContent(
      "Close the terminals open in this worktree before removing it: zsh, Claude Code.",
    );
    expect(removeWorktree).not.toHaveBeenCalled();
    await user.click(clean.getByRole("button", { name: "Open the review" }));
    expect(onReview).toHaveBeenCalledWith({ kind: "branch", branch: "feat/clean", base: "develop" });
  });

  it("speaks Portuguese", async () => {
    setLanguage("pt-BR");
    renderScreen();
    const clean = within(
      (await screen.findByRole("list", { name: "Worktrees do DiffLens" })).querySelector("li") as HTMLElement,
    );
    expect(clean.getByText("limpa")).toBeInTheDocument();
    expect(clean.getByText("nenhum terminal")).toBeInTheDocument();
    expect(clean.getByRole("button", { name: "Remover a worktree de feat/clean" })).toBeInTheDocument();
  });
});
