import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { useState } from "react";
import { beforeEach, describe, expect, it, vi } from "vitest";

import { targetKey } from "@/features/review/review-target";
import { TerminalFooter } from "@/features/terminal/TerminalFooter";
import { TerminalsProvider } from "@/features/terminal/terminals";
import { setLanguage } from "@/i18n";
import {
  attachTerminal,
  closeTerminal,
  createWorktree,
  excludeDifflensFolder,
  getTerminalLocation,
  listTerminals,
  openTerminal,
  resizeTerminal,
  type ReviewTarget,
  type TerminalEvent,
  type TerminalKind,
  type TerminalLocation,
  type TerminalMessage,
  type TerminalSession,
  writeTerminal,
} from "@/lib/api";
import { deferred } from "@/test/fixtures";
import {
  difflensFolder,
  makeFolder,
  makeLocation,
  makeSession,
  noFolder,
  PROJECT_PATH,
} from "@/test/terminal-fixtures";
import { Terminal } from "@/test/xterm";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  attachTerminal: vi.fn(),
  closeTerminal: vi.fn(),
  createWorktree: vi.fn(),
  excludeDifflensFolder: vi.fn(),
  getTerminalLocation: vi.fn(),
  listTerminals: vi.fn(),
  openTerminal: vi.fn(),
  resizeTerminal: vi.fn(),
  writeTerminal: vi.fn(),
}));
vi.mock("@xterm/xterm", () => import("@/test/xterm"));
vi.mock("@xterm/addon-fit", () => ({ FitAddon: class { fit() {} } }));
vi.mock("@xterm/addon-web-links", () => ({ WebLinksAddon: class {} }));
vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

globalThis.ResizeObserver ??= class {
  observe() {}
  unobserve() {}
  disconnect() {}
} as unknown as typeof ResizeObserver;

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const OTHER: ReviewTarget = { kind: "branch", branch: "feat/other", base: "main" };
const encoder = new TextEncoder();

/** The core as these tests see it: every session, the channel it reports through, and what it printed. */
interface FakeSession {
  session: TerminalSession;
  channel: (message: TerminalMessage) => void;
  printed: string;
}
let core: Map<string, FakeSession>;
/** Where the next session opens. */
let folder: string;
const onNavigate = vi.fn();

beforeEach(() => {
  vi.resetAllMocks();
  Terminal.instances = [];
  core = new Map();
  folder = PROJECT_PATH;
  window.localStorage.clear();
  vi.mocked(listTerminals).mockResolvedValue([]);
  vi.mocked(getTerminalLocation).mockResolvedValue(makeLocation());
  vi.mocked(openTerminal).mockImplementation(async (projectId, target, kind, onMessage) => {
    const same = [...core.values()].filter(({ session }) => session.kind === kind && session.cwd === folder).length;
    const base = kind === "claude" ? "Claude Code" : "zsh";
    const session = makeSession(`s${core.size + 1}`, {
      kind,
      name: same === 0 ? base : `${base} ${same + 1}`,
      cwd: folder,
      projectId,
      target,
    });
    core.set(session.id, { session, channel: onMessage, printed: "" });
    return session;
  });
  vi.mocked(attachTerminal).mockImplementation(async (id, replay, onMessage) => {
    const entry = core.get(id)!;
    entry.channel = onMessage;
    if (replay && entry.printed) onMessage({ kind: "output", data: encoder.encode(entry.printed) });
    return entry.session;
  });
  vi.mocked(writeTerminal).mockResolvedValue(undefined);
  vi.mocked(resizeTerminal).mockResolvedValue(undefined);
  vi.mocked(closeTerminal).mockResolvedValue(undefined);
});

function print(id: string, text: string) {
  const entry = core.get(id)!;
  entry.printed += text;
  act(() => entry.channel({ kind: "output", data: encoder.encode(text) }));
}

function report(id: string, event: TerminalEvent) {
  act(() => core.get(id)!.channel({ kind: "event", event }));
}

/** One screen's footer; "Other review" and "Back" switch screens, as navigating does. */
function Screens({ target, request }: { target: ReviewTarget | null; request?: TerminalKind }) {
  const [shown, setShown] = useState<ReviewTarget | null>(target);
  return (
    <>
      <button type="button" onClick={() => setShown(OTHER)}>
        Other review
      </button>
      <button type="button" onClick={() => setShown(target)}>
        Back
      </button>
      <TerminalFooter
        key={shown ? targetKey(shown) : "project"}
        projectId="p-1"
        target={shown}
        theme="dark"
        onNavigate={onNavigate}
        request={shown === target ? request : undefined}
      />
    </>
  );
}

async function renderFooter(options: { target?: ReviewTarget | null; request?: TerminalKind } = {}) {
  render(
    <TerminalsProvider>
      <Screens target={options.target ?? null} request={options.request} />
    </TerminalsProvider>,
  );
  await waitFor(() => expect(getTerminalLocation).toHaveBeenCalled());
  return screen.getByRole("contentinfo", { name: "Terminal" });
}

const panel = () => screen.queryByRole("region", { name: "Terminal" });
const tab = (name: string | RegExp) => screen.getByRole("tab", { name });
const terminal = () => {
  const live = Terminal.live();
  expect(live).toHaveLength(1);
  return live[0];
};

async function newSession(user: ReturnType<typeof userEvent.setup>, item: RegExp = /^New terminal/) {
  await user.click(screen.getByRole("button", { name: "New session" }));
  await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: item }));
}

const shortcut = (target: Window | HTMLElement = window, shiftKey = false) =>
  fireEvent.keyDown(target, { key: "`", code: "Backquote", ctrlKey: true, shiftKey });

describe("the terminal footer", () => {
  it("is only a bar with the + and where sessions open while there are none", async () => {
    const footer = await renderFooter();

    expect(await within(footer).findByTitle(PROJECT_PATH)).toHaveTextContent("~/work/payments-api");
    expect(within(footer).getByText("main")).toBeInTheDocument();
    expect(within(footer).getByText("project folder")).toHaveAttribute(
      "title",
      expect.stringContaining("Commands here change your project folder"),
    );
    expect(within(footer).getByRole("button", { name: "New session" })).toBeInTheDocument();
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Expand the terminal" })).not.toBeInTheDocument();
    expect(panel()).not.toBeInTheDocument();
    expect(getTerminalLocation).toHaveBeenCalledWith("p-1", null);
  });

  it("opens a session expanded on it, shows what it prints and sends what is typed", async () => {
    const user = userEvent.setup();
    await renderFooter({ target: BRANCH });

    await newSession(user);

    expect(openTerminal).toHaveBeenCalledWith("p-1", BRANCH, "shell", expect.any(Function));
    expect(await screen.findByRole("tab", { name: "zsh" })).toHaveAttribute("aria-selected", "true");
    expect(panel()).toBeInTheDocument();
    await waitFor(() => expect(attachTerminal).toHaveBeenCalledWith("s1", true, expect.any(Function)));
    print("s1", "$ ");
    expect(terminal().text).toBe("$ ");
    expect(terminal().focused).toBe(true);

    terminal().type("l");
    terminal().type("s\r");
    await waitFor(() => expect(writeTerminal).toHaveBeenCalledTimes(2));
    expect(vi.mocked(writeTerminal).mock.calls.map(([, data]) => data).join("")).toBe("ls\r");
    expect(resizeTerminal).toHaveBeenCalledWith("s1", 80, 24);
  });

  it("closes an idle shell at once, and closing the last tab collapses the panel", async () => {
    const user = userEvent.setup();
    await renderFooter();
    await newSession(user);
    await newSession(user);
    expect(await screen.findByRole("tab", { name: "zsh 2" })).toHaveAttribute("aria-selected", "true");

    await user.click(screen.getByRole("button", { name: "Close zsh 2" }));

    expect(closeTerminal).toHaveBeenCalledWith("s2");
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(tab("zsh")).toHaveAttribute("aria-selected", "true");
    expect(panel()).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close zsh" }));

    expect(closeTerminal).toHaveBeenCalledWith("s1");
    expect(screen.queryByRole("tablist")).not.toBeInTheDocument();
    expect(panel()).not.toBeInTheDocument();
  });

  it("asks before closing a shell running a command, naming it", async () => {
    const user = userEvent.setup();
    await renderFooter();
    await newSession(user);
    await screen.findByRole("tab", { name: "zsh" });

    report("s1", { type: "foreground", command: "go test ./..." });

    expect(tab("zsh, running go test ./...")).toHaveTextContent("zsh· go test");
    await user.click(screen.getByRole("button", { name: "Close zsh" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Close this terminal?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("go test ./... is still running and will be interrupted.");

    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));
    expect(closeTerminal).not.toHaveBeenCalled();
    expect(tab(/^zsh/)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Close zsh" }));
    await user.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Close anyway" }));
    expect(closeTerminal).toHaveBeenCalledWith("s1");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
  });

  it("asks before closing Claude Code while it runs, and not once it ended", async () => {
    const user = userEvent.setup();
    await renderFooter();
    await newSession(user, /^New Claude Code/);
    expect(openTerminal).toHaveBeenCalledWith("p-1", null, "claude", expect.any(Function));
    await screen.findByRole("tab", { name: "Claude Code" });

    await user.click(screen.getByRole("button", { name: "Close Claude Code" }));
    const dialog = await screen.findByRole("alertdialog");
    expect(within(dialog).getByText("Close Claude Code?")).toBeInTheDocument();
    expect(dialog).toHaveTextContent("claude is still running and will be stopped.");
    await user.click(within(dialog).getByRole("button", { name: "Cancel" }));

    report("s1", { type: "exited", code: 127 });
    expect(terminal().text).toContain("[process exited with code 127]");
    expect(tab("Claude Code, process exited")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Close Claude Code" }));
    expect(screen.queryByRole("alertdialog")).not.toBeInTheDocument();
    expect(closeTerminal).toHaveBeenCalledWith("s1");
  });

  it("toggles with ⌃` and opens a terminal with ⌃⇧`, also from inside the terminal", async () => {
    await renderFooter();
    await within(screen.getByRole("contentinfo")).findByTitle(PROJECT_PATH);

    // With nothing open, expanding opens a terminal.
    shortcut();
    expect(await screen.findByRole("tab", { name: "zsh" })).toBeInTheDocument();
    expect(panel()).toBeInTheDocument();

    const inside = terminal().element!;
    const event = new KeyboardEvent("keydown", { key: "`", code: "Backquote", ctrlKey: true });
    expect(terminal().handles(event)).toBe(false);
    shortcut(inside);
    expect(panel()).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Expand the terminal" })).toHaveAttribute("aria-expanded", "false");

    shortcut();
    expect(panel()).toBeInTheDocument();
    shortcut(terminal().element!, true);
    expect(await screen.findByRole("tab", { name: "zsh 2" })).toHaveAttribute("aria-selected", "true");
    expect(openTerminal).toHaveBeenCalledTimes(2);
  });

  it("marks a tab that printed while it was not on screen, until it is shown", async () => {
    const user = userEvent.setup();
    await renderFooter();
    await newSession(user);
    await newSession(user);
    await screen.findByRole("tab", { name: "zsh 2" });

    print("s1", "build finished\r\n");

    expect(tab("zsh, new output")).toBeInTheDocument();
    expect(tab("zsh 2")).toBeInTheDocument();
    print("s2", "shown here\r\n");
    expect(tab("zsh 2")).toBeInTheDocument();

    await user.click(tab("zsh, new output"));
    expect(tab("zsh")).toHaveAttribute("aria-selected", "true");
    await waitFor(() => expect(terminal().text).toBe("build finished\r\n"));

    // Collapsed, nothing is on screen.
    await user.click(screen.getByRole("button", { name: "Collapse the terminal" }));
    print("s1", "more\r\n");
    expect(tab("zsh, new output")).toBeInTheDocument();
  });

  it("keeps sessions across screens and replays what they printed when it comes back", async () => {
    const user = userEvent.setup();
    vi.mocked(getTerminalLocation).mockImplementation(async (_id, target) =>
      target?.kind === "branch" && target.branch === OTHER.branch
        ? { folder: difflensFolder(OTHER.branch), branch: OTHER.branch, plan: null }
        : makeLocation({ branch: "feat/refund-flow" }),
    );
    await renderFooter({ target: BRANCH });
    await newSession(user);
    await screen.findByRole("tab", { name: "zsh" });
    print("s1", "$ go test ./...\r\nok\r\n");
    const first = terminal();

    await user.click(screen.getByRole("button", { name: "Other review" }));
    expect(first.disposed).toBe(true);
    expect(closeTerminal).not.toHaveBeenCalled();
    expect(await screen.findByRole("button", { name: "1 session in main" })).toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    print("s1", "PASS\r\n");

    await user.click(screen.getByRole("button", { name: "Back" }));
    expect(tab("zsh")).toHaveAttribute("aria-selected", "true");
    expect(panel()).toBeInTheDocument();
    await waitFor(() => expect(terminal().text).toBe("$ go test ./...\r\nok\r\nPASS\r\n"));
    expect(terminal()).not.toBe(first);
    expect(vi.mocked(attachTerminal).mock.calls.filter(([id, replay]) => id === "s1" && replay)).toHaveLength(2);
  });

  it("follows sessions a reload left running, without replaying them", async () => {
    vi.mocked(listTerminals).mockResolvedValue([makeSession("s9", { name: "zsh" })]);
    core.set("s9", { session: makeSession("s9"), channel: () => {}, printed: "old\r\n" });

    await renderFooter();

    expect(await screen.findByRole("tab", { name: "zsh" })).toBeInTheDocument();
    expect(attachTerminal).toHaveBeenCalledWith("s9", false, expect.any(Function));
    expect(panel()).not.toBeInTheDocument();
  });

  it("offers to create a worktree for a branch in no folder, and creates it only on click", async () => {
    const user = userEvent.setup();
    vi.mocked(getTerminalLocation).mockResolvedValue(noFolder("feat/refund-flow"));
    const footer = await renderFooter({ target: BRANCH });
    expect(await within(footer).findByText("no folder yet")).toBeInTheDocument();

    await newSession(user, /^New Claude Code/);

    const step = panel()!;
    expect(within(step).getByRole("heading")).toHaveTextContent("The branch feat/refund-flow is not in any folder");
    expect(step).toHaveTextContent("To open Claude Code on it, DiffLens creates a worktree inside the project.");
    expect(step).toHaveTextContent(
      "Creates the worktree in .difflens/worktrees/feat/refund-flow with the branch feat/refund-flow. The .difflens/ folder goes into .git/info/exclude; your project folder is not changed.",
    );
    expect(tab("Claude Code")).toBeInTheDocument();
    expect(openTerminal).not.toHaveBeenCalled();
    expect(excludeDifflensFolder).not.toHaveBeenCalled();
    expect(createWorktree).not.toHaveBeenCalled();

    const excluded = deferred<void>();
    const created = deferred<TerminalLocation>();
    vi.mocked(excludeDifflensFolder).mockReturnValue(excluded.promise);
    vi.mocked(createWorktree).mockReturnValue(created.promise);
    await user.click(screen.getByRole("button", { name: "Create worktree and open" }));

    expect(await screen.findByRole("status")).toHaveTextContent("Creating the worktree of feat/refund-flow");
    expect(excludeDifflensFolder).toHaveBeenCalledWith("p-1");
    await act(async () => excluded.resolve());
    expect(createWorktree).toHaveBeenCalledWith("p-1", "feat/refund-flow");
    expect(screen.getByRole("status")).toHaveTextContent("git worktree add .difflens/worktrees/feat/refund-flow");

    folder = difflensFolder("feat/refund-flow").path;
    await act(async () =>
      created.resolve({ folder: difflensFolder("feat/refund-flow"), branch: "feat/refund-flow", plan: null }),
    );

    expect(openTerminal).toHaveBeenCalledWith("p-1", BRANCH, "claude", expect.any(Function));
    expect(await screen.findByRole("tab", { name: "Claude Code" })).toHaveAttribute("aria-selected", "true");
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(within(footer).getByText("DiffLens worktree")).toBeInTheDocument();
    const path = within(footer).getByTitle(difflensFolder("feat/refund-flow").path);
    expect(path.lastElementChild).toHaveTextContent(/^\.difflens\/worktrees\/feat\/refund-flow$/);
    expect(path.firstElementChild).toHaveTextContent(/^~\/work\/payments-api\/$/);
  });

  it("explains the tracking branch for a branch only on the remote, and cancels without creating", async () => {
    const user = userEvent.setup();
    vi.mocked(getTerminalLocation).mockResolvedValue(noFolder("feat/refund-flow", "origin/feat/refund-flow"));
    await renderFooter({ target: BRANCH });
    await screen.findByText("no folder yet");

    shortcut();

    const step = await screen.findByRole("region", { name: "Terminal" });
    expect(step).toHaveTextContent("It only exists on the remote. To open a terminal on it");
    expect(step).toHaveTextContent("with a local branch feat/refund-flow that tracks origin/feat/refund-flow.");
    await user.click(within(step).getByRole("button", { name: "Cancel" }));
    expect(panel()).not.toBeInTheDocument();
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    expect(createWorktree).not.toHaveBeenCalled();
  });

  it("names a worktree of the user's own and a detached project folder", async () => {
    vi.mocked(getTerminalLocation).mockResolvedValue({
      folder: makeFolder({ path: "/Users/dev/elsewhere/refund", displayPath: "~/elsewhere/refund", kind: "worktree" }),
      branch: "feat/refund-flow",
      plan: null,
    });
    const footer = await renderFooter({ target: BRANCH });
    expect(await within(footer).findByText("worktree")).toHaveAttribute(
      "title",
      "A worktree you created outside DiffLens, where this branch is checked out.",
    );
    expect(within(footer).getByTitle("/Users/dev/elsewhere/refund")).toHaveTextContent("~/elsewhere/refund");
  });

  it("says how many sessions run in another review, and goes there", async () => {
    const user = userEvent.setup();
    vi.mocked(listTerminals).mockResolvedValue([
      makeSession("s7", { cwd: difflensFolder("feat/other").path, branch: "feat/other", target: OTHER }),
      makeSession("s8", { name: "zsh 2", cwd: difflensFolder("feat/other").path, branch: "feat/other", target: OTHER }),
    ]);
    const footer = await renderFooter();

    const chip = await within(footer).findByRole("button", { name: "2 sessions in feat/other" });
    expect(chip).toHaveAttribute("title", "zsh, zsh 2: still running in the review of feat/other. Click to go there.");
    expect(screen.queryByRole("tab")).not.toBeInTheDocument();
    await user.click(chip);
    expect(onNavigate).toHaveBeenCalledWith({
      screen: "review",
      projectId: "p-1",
      projectName: "payments-api",
      target: OTHER,
    });
  });

  it("opens the session a branch's menu asked for as the review opens", async () => {
    await renderFooter({ target: BRANCH, request: "claude" });

    expect(await screen.findByRole("tab", { name: "Claude Code" })).toHaveAttribute("aria-selected", "true");
    expect(openTerminal).toHaveBeenCalledExactlyOnceWith("p-1", BRANCH, "claude", expect.any(Function));
    expect(panel()).toBeInTheDocument();
  });

  it("changes the panel's height from its edge, within bounds, and remembers it", async () => {
    const user = userEvent.setup();
    await renderFooter();
    await newSession(user);
    const edge = await screen.findByRole("separator", { name: "Terminal height" });
    expect(edge).toHaveAttribute("aria-valuenow", "300");

    edge.focus();
    await user.keyboard("{ArrowUp}{ArrowUp}");
    expect(edge).toHaveAttribute("aria-valuenow", "348");
    expect(window.localStorage.getItem("difflens.terminal.height")).toBe("348");
    for (let step = 0; step < 20; step += 1) await user.keyboard("{ArrowDown}");
    expect(edge).toHaveAttribute("aria-valuenow", "140");
  });

  it("speaks Portuguese", async () => {
    setLanguage("pt-BR");
    const user = userEvent.setup();
    vi.mocked(getTerminalLocation).mockResolvedValue(noFolder("feat/refund-flow"));
    const footer = await renderFooter({ target: BRANCH });
    expect(await within(footer).findByText("nenhuma pasta ainda")).toBeInTheDocument();

    await user.click(within(footer).getByRole("button", { name: "Nova sessão" }));
    await user.click(within(await screen.findByRole("menu")).getByRole("menuitem", { name: /^Novo terminal/ }));

    const step = screen.getByRole("region", { name: "Terminal" });
    expect(within(step).getByRole("heading")).toHaveTextContent("A branch feat/refund-flow não está em nenhuma pasta");
    expect(step).toHaveTextContent(
      "Cria a worktree em .difflens/worktrees/feat/refund-flow com a branch feat/refund-flow. A pasta .difflens/ entra no .git/info/exclude; sua pasta do projeto não é alterada.",
    );
    expect(within(step).getByRole("button", { name: "Criar worktree e abrir" })).toBeInTheDocument();
  });
});
