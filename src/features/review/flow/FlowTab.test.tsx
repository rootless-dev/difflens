import "@/test/elk";
import "@/test/react-flow";

import { act, fireEvent, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { buildView, groupFunctions, NOTHING_OPEN } from "@/features/review/flow/flow-groups";
import { layoutFlow } from "@/features/review/flow/flow-layout";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  type FlowGraph,
  type FlowOptions,
  type FlowResult,
  getFileVersions,
  getFlow,
  getInsights,
  getMutationState,
  getQuality,
  getReviewState,
  getTestsState,
  listChangedFiles,
  type ReviewTarget,
  setFunctionReviewed,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeFinding, NO_INSIGHTS } from "@/test/insights-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead, openReviewTab } from "@/test/review-screen";
import {
  makeDuplicate,
  makeLocatedCheck,
  makeMutationRun,
  makeMutationState,
  makeQualityFunction,
  makeQualityReport,
  makeTestsState,
} from "@/test/tests-fixtures";
import {
  makeFlowCall,
  makeFlowEdge,
  makeFlowGraph,
  makeFlowNode,
  REFUND_IDS as id,
  refundFlowGraph,
  refundNodes,
} from "@/test/flow-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getFlow: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  setFunctionReviewed: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
}));

// CodeMirror needs layout APIs jsdom does not have; the stub shows what it was given.
vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div
      data-testid="diff-editor"
      data-path={props.path}
      data-scroll-line={props.scrollTo?.line ?? ""}
      data-scroll-side={props.scrollTo?.side ?? ""}
    />
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const DEFAULTS: FlowOptions = { includeTests: false, includeCallers: false, includeCallees: false };

const changeSet = makeChangeSet([
  makeChangedFile("go.mod", { language: null }),
  makeChangedFile("internal/payment/consumer.go", { additions: 4, deletions: 1 }),
  makeChangedFile("internal/payment/repo.go", { additions: 3, deletions: 3 }),
  makeChangedFile("internal/refund/handler.go", { status: "added", additions: 48, deletions: 0 }),
  makeChangedFile("internal/refund/legacy.go", { status: "deleted", additions: 0, deletions: 34 }),
  makeChangedFile("internal/refund/service.go", { status: "added", additions: 62, deletions: 0 }),
  makeChangedFile("internal/refund/validate.go", { status: "added", additions: 31, deletions: 0 }),
  makeChangedFile("internal/storage/queries.sql.go", { additions: 6, deletions: 2 }),
  makeChangedFile("internal/storage/refund_repo.go", { additions: 2, deletions: 2 }),
]);

const graphResult = (overrides: Parameters<typeof refundFlowGraph>[0] = {}): FlowResult => ({
  kind: "graph",
  graph: refundFlowGraph(overrides),
});

const resultOf = (graph: FlowGraph): FlowResult => ({ kind: "graph", graph });

let bar: HTMLDivElement;

function renderScreen() {
  return render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
}

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const tab = (name: string) => within(screen.getByRole("tablist", { name: "Review views" })).getByRole("tab", { name });
const boxesNamed = (name: string) =>
  screen
    .queryAllByRole("button", { name: new RegExp(`^${escape(name)},`) })
    .filter((element) => element.closest(".flow-canvas"));
/** A box of the canvas (not a row of the left panel), a function or a group, by the name its label starts with. */
const box = (name: string) =>
  vi.waitFor(() => {
    const [found, ...others] = boxesNamed(name);
    if (!found || others.length > 0) throw new Error(`Expected one box named ${name}, found ${others.length + (found ? 1 : 0)}`);
    return found;
  });
const queryBox = (name: string) => boxesNamed(name)[0] ?? null;
const popover = () => screen.getByRole("dialog");
const level = (name: string) => within(screen.getByRole("group", { name: "Level" })).getByRole("button", { name });
// A plain click: user-event's mousedown has no `view`, which React Flow's panning (d3-zoom) reads on a node.
const clickBox = async (name: string) => fireEvent.click(await box(name));
const clickButton = async (name: string) => fireEvent.click(await screen.findByRole("button", { name }));

async function openFlow(user = userEvent.setup()) {
  renderScreen();
  await openReviewTab(user, "Flow");
  return user;
}

/** Opens the Flow tab and switches the canvas to one box per function. */
async function openFunctions() {
  const user = await openFlow();
  await box("internal/refund/service.go");
  await user.click(level("Functions"));
  await box("Service.Request");
  return user;
}

// Loads React Flow and starts ELK once, so the first test does not spend its time limit on it.
beforeAll(async () => {
  await import("@/features/review/flow/FlowWorkspace");
  const graph = refundFlowGraph();
  await layoutFlow(buildView(graph, groupFunctions(graph.nodes), "files", NOTHING_OPEN));
}, 20_000);

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
  vi.mocked(getFlow).mockResolvedValue(graphResult());
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
});

afterEach(() => {
  bar.remove();
  vi.useRealTimers();
});

describe("Flow tab", () => {
  it("computes the flow when the review opens, since the Overview shows it, for the commits of the change set", async () => {
    const user = userEvent.setup();
    renderScreen();
    await changedFilesRead();

    expect(tab("Overview")).toHaveAttribute("aria-selected", "true");
    expect(tab("Flow")).not.toHaveAttribute("aria-disabled", "true");
    expect(getFlow).toHaveBeenCalledExactlyOnceWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a", DEFAULTS);

    await user.click(tab("Flow"));

    expect(tab("Flow")).toHaveAttribute("aria-selected", "true");
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    // Opening the tab draws what was computed; it does not compute again.
    expect(getFlow).toHaveBeenCalledOnce();
  });

  it("shows how long the analysis has been running and that later opens use the cache", async () => {
    vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
    const pending = deferred<FlowResult>();
    vi.mocked(getFlow).mockReturnValue(pending.promise);
    await openFlow();

    const progress = screen.getByRole("status", { name: "Analysing the code" });
    expect(progress).toHaveTextContent("Analysing the functions on both sides of this review0 s");
    expect(progress).toHaveTextContent("Later opens of the same commits use the cache.");
    act(() => {
      vi.advanceTimersByTime(12_000);
    });
    expect(progress).toHaveTextContent("12 s");

    vi.useRealTimers();
    pending.resolve(graphResult());
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Analysing the code" })).not.toBeInTheDocument();
  });

  it("keeps the flow across tab switches, and Reload makes the next open compute it again", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");

    await user.click(tab("Diff"));
    await user.click(tab("Flow"));
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(getFlow).toHaveBeenCalledTimes(1);

    await user.click(tab("Diff"));
    vi.mocked(listChangedFiles).mockResolvedValueOnce(makeChangeSet(changeSet.files, { newSha: "f00dfeed00000000" }));
    await user.click(within(bar).getByRole("button", { name: "Reload" }));
    await within(bar).findByText("f00dfee");
    expect(getFlow).toHaveBeenCalledTimes(1);

    await user.click(tab("Flow"));
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(getFlow).toHaveBeenLastCalledWith("p-1", "a1b2c3d4e5f6a7b8", "f00dfeed00000000", DEFAULTS);
  });

  it("recomputes at once when Reload is pressed on the Flow tab", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");

    await user.click(within(bar).getByRole("button", { name: "Reload" }));

    await vi.waitFor(() => expect(getFlow).toHaveBeenCalledTimes(2));
  });

  it("says calmly which languages have no analyzer and leads to the Diff tab", async () => {
    vi.mocked(getFlow).mockResolvedValue({ kind: "unavailable", reason: "no_analyzer", languages: ["typescript"] });
    const user = await openFlow();

    expect(await screen.findByText("No analyzer for TypeScript yet. The Diff tab works for every language.")).toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open the Diff tab" }));
    expect(tab("Diff")).toHaveAttribute("aria-selected", "true");
  });

  it("shows which side failed to analyse and tries again", async () => {
    vi.mocked(getFlow).mockResolvedValueOnce({
      kind: "unavailable",
      reason: "analysis_failed",
      side: "new",
      message: "go list: internal/refund/service.go:12:2: expected declaration",
    });
    const user = await openFlow();

    expect(await screen.findByRole("alert")).toHaveTextContent("Analysing the new side of this review failed.");
    expect(screen.getByText("go list: internal/refund/service.go:12:2: expected declaration")).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(getFlow).toHaveBeenCalledTimes(2);
  });

  it("shows a failed call inline and tries again", async () => {
    vi.mocked(getFlow).mockRejectedValueOnce(apiError("unknown_branch", "Branch feat/refund-flow no longer exists."));
    const user = await openFlow();

    expect(await screen.findByRole("alert")).toHaveTextContent("Branch feat/refund-flow no longer exists.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
  });
});

describe("flow levels and groups", () => {
  it("opens by file for a review of 11 files and 10 links between them, with a level switch", async () => {
    await openFlow();

    const service = await box("internal/refund/service.go");
    expect(service).toHaveAccessibleName(
      "internal/refund/service.go, file, 1 changed function, 28 added, 0 removed",
    );
    expect(service).toHaveTextContent("internal/refund/service.go1 changed function+28−0");
    expect(screen.getAllByRole("button", { name: /^internal\/[\w./]+, file,/ })).toHaveLength(11);
    expect(await box("github.com/acme/gateway")).toHaveAttribute("data-state", "external");
    expect(queryBox("Service.Request")).not.toBeInTheDocument();
    expect(level("Files")).toHaveAttribute("aria-pressed", "true");
    expect(level("Packages")).toHaveAttribute("aria-pressed", "false");
    expect(level("Functions")).toHaveAttribute("aria-pressed", "false");
  });

  it("opens by package when the review spans more than 20 files", async () => {
    const nodes = Array.from({ length: 21 }, (_, n) =>
      makeFlowNode(`p.F${n}`, {
        name: `F${n}`,
        state: "modified",
        container: `internal/p${n % 3}`,
        file: `internal/p${n % 3}/f${n}.go`,
      }),
    );
    vi.mocked(getFlow).mockResolvedValue(resultOf(makeFlowGraph(nodes, [makeFlowEdge("p.F0", "p.F1")])));
    await openFlow();

    const pkg = await box("internal/p0");
    expect(pkg).toHaveAccessibleName(/^internal\/p0, package, 7 files, 7 changed functions/);
    expect(pkg).toHaveTextContent("7 files · 7 changed functions");
    expect(level("Packages")).toHaveAttribute("aria-pressed", "true");
  });

  it("opens a package in place to show its files, the other packages staying closed", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");
    await user.click(level("Packages"));

    await clickBox("internal/refund");

    expect(await screen.findByRole("button", { name: "Close internal/refund" })).toHaveAttribute("aria-expanded", "true");
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(await box("internal/payment")).toHaveAttribute("aria-expanded", "false");
    expect(queryBox("internal/payment/repo.go")).not.toBeInTheDocument();
    expect(queryBox("Service.Request")).not.toBeInTheDocument();

    await clickButton("Close internal/refund");

    expect(await box("internal/refund")).toBeInTheDocument();
    expect(queryBox("internal/refund/service.go")).not.toBeInTheDocument();
  });

  it("opens a file in place to show its functions", async () => {
    await openFlow();
    await box("internal/refund/service.go");

    await clickButton("Show the functions of internal/refund/service.go");

    expect(await box("Service.Request")).toBeInTheDocument();
    expect(await screen.findByRole("button", { name: "Close internal/refund/service.go" })).toBeInTheDocument();
    expect(queryBox("Handler.Create")).not.toBeInTheDocument();
    expect(await box("internal/refund/handler.go")).toBeInTheDocument();
  });

  it("keeps an opened group where it was on screen instead of fitting the canvas again", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");
    await user.click(level("Packages"));
    await box("internal/refund");
    const screenX = () => {
      const viewport = document.querySelector<HTMLElement>(".react-flow__viewport")!.style.transform;
      const [, vx, vy, zoom] = /translate\(([-\d.]+)px, ([-\d.]+)px\) scale\(([\d.]+)\)/.exec(viewport)!.map(Number);
      const node = document.querySelector<HTMLElement>('.react-flow__node[data-id="package:internal/refund"]')!;
      const [, nx, ny] = /translate\(([-\d.]+)px, ?([-\d.]+)px\)/.exec(node.style.transform)!.map(Number);
      return { x: vx + nx * zoom, y: vy + ny * zoom };
    };
    await vi.waitFor(() => expect(screenX().x).not.toBeNaN());
    const before = screenX();

    await clickBox("internal/refund");
    await screen.findByRole("button", { name: "Close internal/refund" });

    await vi.waitFor(() => {
      const after = screenX();
      expect(after.x).toBeCloseTo(before.x, 0);
      expect(after.y).toBeCloseTo(before.y, 0);
    });
  });

  it("goes back to closed groups when the level changes", async () => {
    const user = await openFlow();
    await clickButton("Show the functions of internal/refund/service.go");
    await box("Service.Request");

    await user.click(level("Packages"));
    expect(await box("internal/refund")).toHaveAttribute("aria-expanded", "false");
    await user.click(level("Files"));
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(queryBox("Service.Request")).not.toBeInTheDocument();
  });
});

/** Routes registers Handler.Issue and the logger, Issue passes audit as a value, calls the store, and no longer calls legacyIssue. */
function tokenGraph(): FlowGraph {
  const node = (nodeId: string, name: string, file: string, state: "new" | "modified" | "deleted") =>
    makeFlowNode(nodeId, { name, qualifiedName: `auth.${name}`, container: "internal/auth", file, state, additions: 2 });
  return makeFlowGraph(
    [
      node("a.Routes", "Routes", "internal/auth/routes.go", "modified"),
      node("a.Issue", "Handler.Issue", "internal/auth/handler.go", "new"),
      node("a.audit", "audit", "internal/auth/handler.go", "new"),
      node("a.legacy", "legacyIssue", "internal/auth/handler.go", "deleted"),
      node("a.Logger", "Logger", "internal/auth/middleware.go", "modified"),
      node("a.Save", "Store.Save", "internal/auth/store.go", "modified"),
    ],
    [
      makeFlowEdge("a.Routes", "a.Issue", {
        line: 12,
        relation: "registration",
        evidence: "route POST /token",
        kind: "async",
        added: true,
      }),
      makeFlowEdge("a.Routes", "a.Logger", { line: 9, relation: "registration", evidence: "middleware", kind: "async" }),
      makeFlowEdge("a.Issue", "a.audit", { line: 30, relation: "reference", kind: "async", added: true }),
      makeFlowEdge("a.Issue", "a.Save", { line: 34 }),
      makeFlowEdge("a.Issue", "a.legacy", { line: 38, removed: true }),
    ],
  );
}

describe("flow links", () => {
  const link = (from: string, to: string) => screen.getByTestId(`link ${from}->${to}`);
  const line = (from: string, to: string) => link(from, to).querySelectorAll("path")[1];

  it("draws the type of a relation as its line style and label, and its state as its colour", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(tokenGraph()));
    const user = await openFlow();
    await box("internal/auth/routes.go");
    await user.click(level("Functions"));
    await box("Handler.Issue");

    expect(link("a.Routes", "a.Issue")).toHaveAttribute("data-relation", "registration");
    expect(line("a.Routes", "a.Issue")).toHaveAttribute("stroke-dasharray", "1.5 3.5");
    expect(line("a.Routes", "a.Issue")).toHaveClass("stroke-primary");
    expect(line("a.Routes", "a.Logger")).toHaveClass("stroke-faint");
    expect(line("a.Issue", "a.audit")).toHaveAttribute("stroke-dasharray", "1 6");
    expect(line("a.Issue", "a.Save")).not.toHaveAttribute("stroke-dasharray");
    expect(line("a.Issue", "a.legacy")).not.toHaveAttribute("stroke-dasharray");
    expect(line("a.Issue", "a.legacy")).toHaveClass("stroke-del");

    const label = (name: string) => screen.getByRole("button", { name });
    expect(label("Routes → Handler.Issue: registers route POST /token")).toHaveTextContent("registers route POST /token");
    expect(label("Routes → Logger: registers middleware")).toBeInTheDocument();
    expect(label("Handler.Issue → audit: passes as value")).toBeInTheDocument();
    expect(label("Handler.Issue → Store.Save: calls")).toBeInTheDocument();
    const removed = label("Handler.Issue → legacyIssue: calls, removed");
    expect(removed).toHaveTextContent("×calls");
    expect(within(removed).getByText("calls")).toHaveClass("line-through");

    const types = screen.getByRole("list", { name: "Links" });
    expect(types).toHaveTextContent("Calls");
    expect(types).toHaveTextContent("Registers (route, middleware…)");
    expect(types).toHaveTextContent("Passes as value");
    expect(types).toHaveTextContent("Removed in this review");
  });

  it("counts the relations of a link between groups, by state and by type", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(tokenGraph()));
    await openFlow();
    await box("internal/auth/routes.go");

    expect(screen.getByRole("button", { name: "routes.go → handler.go: 1 added · 1 registration" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "routes.go → middleware.go: 1 preserved · 1 registration" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "handler.go → store.go: 1 preserved" })).toBeInTheDocument();
    expect(screen.queryByTestId(/^link file:internal\/auth\/handler\.go->file:internal\/auth\/handler\.go$/)).toBeNull();
  });

  /** service.go and refund_repo.go linked by 2 added, 3 kept and 1 removed calls. */
  function mixedGraph(): FlowGraph {
    const fn = (nodeId: string, file: string, state: "new" | "modified" | "deleted", startLine: number) =>
      makeFlowNode(nodeId, {
        name: nodeId.slice(2),
        container: file.startsWith("internal/refund") ? "internal/refund" : "internal/storage",
        file,
        state,
        startLine,
      });
    const service = "internal/refund/service.go";
    const repo = "internal/storage/refund_repo.go";
    return makeFlowGraph(
      [
        fn("p.Create", service, "new", 30),
        fn("p.List", service, "modified", 5),
        fn("p.Update", service, "modified", 10),
        fn("p.Insert", repo, "modified", 3),
        fn("p.Get", repo, "modified", 20),
        fn("p.Delete", repo, "deleted", 40),
      ],
      [
        makeFlowEdge("p.Create", "p.Insert", { line: 33, added: true }),
        makeFlowEdge("p.Create", "p.Get", { line: 35, added: true }),
        makeFlowEdge("p.List", "p.Get", { line: 8 }),
        makeFlowEdge("p.Update", "p.Get", { line: 12 }),
        makeFlowEdge("p.Update", "p.Insert", { line: 14 }),
        makeFlowEdge("p.Update", "p.Delete", { line: 20, removed: true }),
      ],
    );
  }

  it("never calls a link between groups added because some of it is, and lists what it stands for", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(mixedGraph()));
    await openFlow();
    await box("internal/refund/service.go");

    const edge = screen.getByTestId("link file:internal/refund/service.go->file:internal/storage/refund_repo.go");
    expect(edge).toHaveAttribute("data-state", "mixed");
    expect(edge.querySelectorAll("path")[1]).toHaveClass("stroke-muted-foreground");
    const name = "service.go → refund_repo.go: 2 added · 3 preserved · 1 removed";
    fireEvent.click(screen.getByRole("button", { name }));

    const dialog = popover();
    expect(dialog).toHaveAccessibleName("service.go → refund_repo.go");
    expect(dialog).toHaveTextContent("2 added · 3 preserved · 1 removed6 calls");
    const relations = within(within(dialog).getByRole("region", { name: "Relations" })).getAllByRole("listitem");
    expect(relations.map((item) => item.textContent)).toEqual([
      "Create → Insertline 33callsaddedDiff",
      "Create → Getline 35callsaddedDiff",
      "List → Getline 8callspreservedDiff",
      "Update → Getline 12callspreservedDiff",
      "Update → Insertline 14callspreservedDiff",
      "Update → Deleteline 20callsremovedDiff",
    ]);
    expect(screen.getByRole("button", { name })).toHaveAttribute("aria-expanded", "true");
  });

  it("goes from a listed relation to its line in the diff, on the old side when it was removed", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(mixedGraph()));
    const user = await openFlow();
    await box("internal/refund/service.go");
    fireEvent.click(screen.getByRole("button", { name: /^service\.go → refund_repo\.go:/ }));

    await user.click(within(popover()).getByRole("button", { name: "View diff at line 20 of service.go" }));

    expect(tab("Diff")).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "20");
    expect(editor).toHaveAttribute("data-scroll-side", "old");
  });

  it("closes the list with Escape, giving the keyboard back to the label", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(mixedGraph()));
    const user = await openFlow();
    await box("internal/refund/service.go");
    const label = () => screen.getByRole("button", { name: /^service\.go → refund_repo\.go:/ });
    fireEvent.click(label());

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    await vi.waitFor(() => expect(label()).toHaveFocus());
  });
});

describe("isolated boxes", () => {
  it("says a box has no connection in this view, and nothing about unused code", async () => {
    const lone = makeFlowNode("p.Lone", {
      name: "Lone",
      state: "new",
      container: "internal/extra",
      file: "internal/extra/lone.go",
      additions: 4,
    });
    vi.mocked(getFlow).mockResolvedValue(resultOf(makeFlowGraph([...refundNodes, lone], refundFlowGraph().edges)));
    const user = await openFlow();

    const file = await box("internal/extra/lone.go");
    expect(file).toHaveTextContent("No connection found in this view");
    expect(file).toHaveAccessibleName(/no connection found in this view$/);
    expect(await box("internal/refund/service.go")).not.toHaveTextContent("No connection");

    await user.click(level("Functions"));
    expect(await box("Lone")).toHaveTextContent("No connection found in this view");
    expect(screen.queryByText(/unused|dead code|never called/i)).not.toBeInTheDocument();
  });
});

describe("function boxes", () => {
  it("draws every function with its state, lines and entry point, and the legends", async () => {
    await openFunctions();

    const create = await box("Handler.Create");
    expect(create).toHaveAccessibleName("Handler.Create, new, 18 added, 0 removed, entry point POST /refunds");
    expect(create).toHaveTextContent("Handler.Createrefund · POST /refunds+18−0");
    expect(await box("legacyRefund")).toHaveAttribute("data-state", "deleted");
    expect(await box("Queries.InsertRefund")).toHaveTextContent("generated");
    const gateway = await box("github.com/acme/gateway");
    expect(gateway).toHaveAccessibleName("github.com/acme/gateway, external package, 2 functions called");
    expect(gateway).toHaveTextContent("github.com/acme/gateway2 functions");
    expect(screen.getAllByRole("button", { name: /^[\w./]+, (new|modified|deleted|untouched|external)/ })).toHaveLength(12);
    expect(screen.getByRole("list", { name: "Function states" })).toHaveTextContent("External package");

    expect(screen.getByRole("button", { name: "This branch" })).toHaveAttribute("aria-pressed", "true");
    const wholeApp = screen.getByRole("button", { name: "Whole app" });
    expect(wholeApp).toHaveAttribute("aria-disabled", "true");
    expect(wholeApp).toHaveAttribute("title", "Comes in a later step");
  });

  it("draws links along their laid out routes, in straight runs around the boxes", async () => {
    await openFunctions();
    await box("Handler.Create");

    const lines = [...document.querySelectorAll<SVGPathElement>(".flow-canvas [data-testid^='link '] path[stroke-width]")];
    expect(lines.length).toBeGreaterThan(0);
    for (const line of lines) expect(line.getAttribute("d")).not.toContain("C");
  });

  it("lets boxes be dragged only while the move key is held", async () => {
    await openFunctions();
    await box("Handler.Create");
    const canvas = document.querySelector(".flow-canvas")!;
    // jsdom is no Mac, so the key is Ctrl; both are pressed so the test holds on either.
    const key = { key: "Control", ctrlKey: true, metaKey: true };

    expect(canvas).not.toHaveAttribute("data-moving");
    expect(canvas.querySelector(".react-flow__node.draggable")).toBeNull();
    fireEvent.keyDown(window, key);
    expect(canvas).toHaveAttribute("data-moving", "true");
    expect(canvas.querySelector(".react-flow__node.draggable")).not.toBeNull();
    fireEvent.keyUp(window, { key: "Control" });
    expect(canvas).not.toHaveAttribute("data-moving");
    fireEvent.keyDown(window, key);
    fireEvent.blur(window);
    expect(canvas).not.toHaveAttribute("data-moving");
  });

  it("names only the changed states in the legend when nothing else is drawn", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(tokenGraph()));
    await openFlow();
    await box("internal/auth/routes.go");

    expect(screen.getByRole("list", { name: "Function states" }).textContent).toBe("NewModifiedDeleted");
  });

  it("opens a popover with the function's facts, dims the rest, and closes on Escape", async () => {
    const user = await openFunctions();

    await clickBox("Service.Request");

    const dialog = popover();
    expect(dialog).toHaveAccessibleName("refund.Service.Request");
    expect(dialog).toHaveTextContent("New+28 −0internal/refund/service.go:44–71");
    expect(dialog).toHaveTextContent("Added in this review.");
    const facts = within(dialog).getAllByRole("term").map((term) => `${term.textContent}: ${term.nextSibling?.textContent}`);
    expect(facts).toEqual(["Cyclomatic complexity: 11", "Callers shown: 1", "Calls shown: 4"]);
    expect(dialog).toHaveTextContent("Measured by difflens-go 0.1.0");
    expect(await box("Service.Request")).toHaveAttribute("aria-expanded", "true");
    expect(await box("Validate")).not.toHaveAttribute("data-dimmed");
    expect(await box("Notifier.Send")).toHaveAttribute("data-dimmed", "true");

    await user.keyboard("{Escape}");

    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await box("Notifier.Send")).not.toHaveAttribute("data-dimmed");
    await vi.waitFor(async () => expect(await box("Service.Request")).toHaveFocus());
  });

  it("closes the popover with its button, a second click on the node, or a press outside", async () => {
    const user = await openFunctions();

    await clickBox("Repo.Get");
    await user.click(within(popover()).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await clickBox("Repo.Get");
    await clickBox("Repo.Get");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();

    await clickBox("Repo.Get");
    await user.click(screen.getByRole("list", { name: "Function states" }));
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("goes to the diff of the function's file, at its first line", async () => {
    const user = await openFunctions();
    await clickBox("Service.Request");

    await user.click(within(popover()).getByRole("button", { name: "View diff of service.go" }));

    expect(tab("Diff")).toHaveAttribute("aria-selected", "true");
    expect(screen.getByRole("option", { selected: true })).toHaveAccessibleName(/^internal\/refund\/service\.go,/);
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "44");
    expect(editor).toHaveAttribute("data-scroll-side", "new");

    await user.click(screen.getByRole("option", { name: /^go\.mod,/ }));
    await user.click(screen.getByRole("option", { name: /^internal\/refund\/service\.go,/ }));
    expect(await screen.findByTestId("diff-editor")).toHaveAttribute("data-scroll-line", "");
  });

  it("opens the old side for a deleted function and offers no diff for an external package", async () => {
    const user = await openFunctions();

    await clickBox("github.com/acme/gateway");
    expect(popover()).toHaveTextContent("Outside the repository");
    expect(within(popover()).queryByRole("button", { name: /^View diff/ })).not.toBeInTheDocument();

    await clickBox("DB.QueryRow");
    expect(popover()).toHaveTextContent("File not changed in this review");

    await clickBox("legacyRefund");
    await user.click(within(popover()).getByRole("button", { name: "View diff of legacy.go" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/legacy.go");
    expect(editor).toHaveAttribute("data-scroll-line", "10");
    expect(editor).toHaveAttribute("data-scroll-side", "old");
  });

  it("lists every relation by line, marking callees not shown, added and removed, and registrations", async () => {
    const withRoute = refundFlowGraph({
      nodes: refundNodes.map((node) =>
        node.id === id.request
          ? {
              ...node,
              calls: [
                ...node.calls,
                makeFlowCall(id.validate, "refund.Validate", {
                  line: 70,
                  added: true,
                  relation: "registration",
                  evidence: "middleware",
                }),
              ],
            }
          : node,
      ),
    });
    vi.mocked(getFlow).mockResolvedValue(resultOf(withRoute));
    await openFunctions();
    await clickBox("Service.Request");

    const calls = within(popover()).getByRole("region", { name: "Calls and registrations, by line" });
    expect(within(calls).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "payment.Repo.Getline 45added",
      "refund.Validateline 49added",
      "github.com/acme/gatewayline 53added",
      "storage.RefundRepo.Saveline 58added",
      "refund.Service.auditline 66not shownadded",
      "refund.Validateline 70registers middlewareadded",
    ]);
    expect(within(calls).queryByRole("button", { name: "refund.Service.audit" })).not.toBeInTheDocument();
  });

  it("marks a removed call, and opens the popover of a shown callee", async () => {
    const user = await openFunctions();
    await clickBox("Handler.Create");

    const calls = within(popover()).getByRole("region", { name: "Calls and registrations, by line" });
    expect(within(calls).getAllByRole("listitem")[0]).toHaveTextContent("refund.legacyRefundline 30removed");

    await user.click(within(calls).getByRole("button", { name: "refund.Service.Request" }));

    expect(popover()).toHaveAccessibleName("refund.Service.Request");
    expect(await box("Service.Request")).toHaveAttribute("aria-expanded", "true");
  });
});

describe("file popover", () => {
  it("lists the file's changed functions, each with a way to its diff, and opens the file in place", async () => {
    const user = await openFlow();
    await clickBox("internal/refund/service.go");

    const dialog = popover();
    expect(dialog).toHaveAccessibleName("internal/refund/service.go");
    expect(dialog).toHaveTextContent("1 changed function+28 −0internal/refund");
    const functions = within(within(dialog).getByRole("region", { name: "Changed functions" })).getAllByRole("listitem");
    expect(functions.map((item) => item.textContent)).toEqual(["Service.Request+28 −0Diff"]);
    expect(await box("internal/refund/service.go")).toHaveAttribute("aria-expanded", "true");

    await user.click(within(dialog).getByRole("button", { name: "Show its functions" }));

    expect(await box("Service.Request")).toBeInTheDocument();
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("goes to the diff of a listed function and opens its popover from its name", async () => {
    const user = await openFlow();
    await clickBox("internal/refund/handler.go");
    await user.click(within(popover()).getByRole("button", { name: "Handler.Create" }));

    expect(await box("Handler.Create")).toHaveAttribute("aria-expanded", "true");
    expect(popover()).toHaveAccessibleName("refund.Handler.Create");

    await clickBox("internal/refund/legacy.go");
    await user.click(within(popover()).getByRole("button", { name: "View diff of legacyRefund" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/legacy.go");
    expect(editor).toHaveAttribute("data-scroll-side", "old");
  });
});

describe("flow left panel", () => {
  it("lists the entry points touched and the packages changed", async () => {
    await openFlow();
    await box("internal/refund/service.go");

    const entries = within(screen.getByRole("region", { name: "Entry points touched" }));
    expect(entries.getAllByRole("button").map((row) => row.textContent)).toEqual([
      "POST /refundsnew",
      "GET /payments/{id}",
      "payment.settledconsumer",
    ]);
    const packages = within(screen.getByRole("region", { name: "Packages changed" }));
    expect(packages.getAllByRole("button").map((row) => row.textContent)).toEqual([
      "internal/refund4 functions",
      "internal/payment2 functions",
      "internal/storage2 functions",
    ]);
  });

  it("says how to find entry points when no changed function is one", async () => {
    vi.mocked(getFlow).mockResolvedValue(resultOf(tokenGraph()));
    await openFlow();
    await box("internal/auth/routes.go");

    expect(screen.getByRole("region", { name: "Entry points touched" })).toHaveTextContent(
      "No changed function is an entry point. Show callers to walk up to them.",
    );
  });

  it("opens what holds an entry point's function, then its popover", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");

    const row = screen.getByRole("button", { name: "POST /refunds, new" });
    await user.click(row);

    expect(row).toHaveAttribute("aria-pressed", "true");
    expect(await box("Handler.Create")).toHaveAttribute("aria-expanded", "true");
    expect(popover()).toHaveAccessibleName("refund.Handler.Create");
    expect(await box("internal/payment/repo.go")).toHaveAttribute("data-dimmed", "true");
  });

  it("focuses a package, dimming the boxes outside it", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");

    const row = screen.getByRole("button", { name: "internal/storage, 2 functions" });
    await user.click(row);

    expect(row).toHaveAttribute("aria-pressed", "true");
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    expect(await box("internal/storage/refund_repo.go")).not.toHaveAttribute("data-dimmed");
    expect(await box("internal/storage/db.go")).not.toHaveAttribute("data-dimmed");
    expect(await box("internal/refund/service.go")).toHaveAttribute("data-dimmed", "true");

    await user.keyboard("{Escape}");
    expect(row).toHaveAttribute("aria-pressed", "false");
  });
});

describe("flow notices", () => {
  it("says when the graph was cut and lists the analyzer's problems on demand", async () => {
    vi.mocked(getFlow).mockResolvedValue(
      graphResult({
        truncated: true,
        diagnostics: [
          "old: internal/tools: build constraints exclude all Go files",
          "new: internal/legacy/broken.go:12:3: undefined: oldClient",
        ],
      }),
    );
    const user = await openFlow();

    expect(await screen.findByRole("note")).toHaveTextContent("This graph was cut to stay readable.");
    const problems = screen.getByText("2 problems while analysing");
    expect(screen.getByText("new: internal/legacy/broken.go:12:3: undefined: oldClient")).not.toBeVisible();
    await user.click(problems);
    expect(screen.getByText("new: internal/legacy/broken.go:12:3: undefined: oldClient")).toBeVisible();
  });

  it("shows neither when the graph is whole and clean", async () => {
    await openFlow();
    await box("internal/refund/service.go");

    expect(screen.queryByRole("note")).not.toBeInTheDocument();
    expect(screen.queryByText(/while analysing/)).not.toBeInTheDocument();
  });

  it("says so when no function changed outside tests", async () => {
    vi.mocked(getFlow).mockResolvedValue(
      graphResult({
        nodes: [],
        edges: [],
        entryPoints: [],
        hidden: { changedTests: 0, callers: 0, callees: 0, externalPackages: 0 },
      }),
    );
    await openFlow();

    expect(await screen.findByText("No functions changed outside tests.")).toBeInTheDocument();
  });

  it("says when the review only changes tests, and shows them once they are included", async () => {
    const tests = { changedTests: 2, callers: 0, callees: 0, externalPackages: 0 };
    const testNode = makeFlowNode("p.TestRefund", { name: "TestRefund", state: "modified", test: true, additions: 3 });
    vi.mocked(getFlow).mockImplementation(async (_project, _old, _new, options) =>
      options.includeTests
        ? graphResult({ nodes: [testNode], edges: [], entryPoints: [], hidden: tests, options })
        : graphResult({ nodes: [], edges: [], entryPoints: [], hidden: tests, options }),
    );
    const user = await openFlow();

    expect(
      await screen.findByText("No functions changed outside tests. Include tests to see the 2 changed test functions."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Include tests (2)" }));

    expect(await box("internal/app/app.go")).toBeInTheDocument();
    await user.click(level("Functions"));
    const shown = await box("TestRefund");
    expect(shown).toHaveAccessibleName(/^TestRefund, modified, 3 added, 0 removed, test code/);
    expect(shown).toHaveTextContent("test");
    expect(screen.queryByText(/only tests changed|outside tests/)).not.toBeInTheDocument();
  });
});

describe("flow options", () => {
  const toggle = (name: RegExp) => screen.getByRole("button", { name });

  it("labels each toggle with what it would add and asks for a new graph with it", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");

    const tests = toggle(/^Include tests/);
    const callers = toggle(/^Show callers/);
    const callees = toggle(/^Show what they call/);
    expect(tests).toHaveAccessibleName("Include tests (3)");
    expect(tests).toHaveAttribute("aria-pressed", "false");
    expect(callers).toHaveAccessibleName("Show callers (4)");
    expect(callers).toHaveAttribute("title", "4 unchanged functions on the paths from entry points down to the changed ones");
    expect(callees).toHaveAccessibleName("Show what they call (3)");
    expect(callees).toHaveAttribute("title", "2 functions of the repository and 1 external package the changed functions call");

    await user.click(callers);
    expect(getFlow).toHaveBeenLastCalledWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a", {
      ...DEFAULTS,
      includeCallers: true,
    });
    expect(toggle(/^Show callers/)).toHaveAttribute("aria-pressed", "true");

    await user.click(toggle(/^Show what they call/));
    expect(getFlow).toHaveBeenLastCalledWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a", {
      includeTests: false,
      includeCallers: true,
      includeCallees: true,
    });

    await user.click(toggle(/^Include tests/));
    expect(getFlow).toHaveBeenLastCalledWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a", {
      includeTests: true,
      includeCallers: true,
      includeCallees: true,
    });
    expect(getFlow).toHaveBeenCalledTimes(4);
  });

  it("disables a toggle that would add nothing and says why", async () => {
    vi.mocked(getFlow).mockResolvedValue(
      graphResult({ hidden: { changedTests: 0, callers: 0, callees: 0, externalPackages: 0 } }),
    );
    const user = await openFlow();
    await box("internal/refund/service.go");

    const tests = toggle(/^Include tests/);
    expect(tests).toHaveAccessibleName("Include tests (0)");
    expect(tests).toHaveAttribute("aria-disabled", "true");
    expect(tests).toHaveAttribute("title", "No test function changed in this review");
    const callers = toggle(/^Show callers/);
    expect(callers).toHaveAttribute("aria-disabled", "true");
    expect(callers).toHaveAttribute("title", "Nothing else calls the changed functions");
    const callees = toggle(/^Show what they call/);
    expect(callees).toHaveAttribute("aria-disabled", "true");
    expect(callees).toHaveAttribute("title", "The changed functions call nothing else");

    await user.click(tests);
    await user.click(callers);
    await user.click(callees);
    expect(getFlow).toHaveBeenCalledTimes(1);
  });

  it("keeps the graph on screen, marked as updating, until the new one arrives", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");
    const pending = deferred<FlowResult>();
    vi.mocked(getFlow).mockReturnValueOnce(pending.promise);

    await user.click(toggle(/^Include tests/));

    expect(screen.getByRole("status")).toHaveTextContent("Updating the graph");
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
    expect(document.querySelector(".flow-canvas")).toHaveAttribute("aria-busy", "true");
    expect(screen.queryByRole("status", { name: "Analysing the code" })).not.toBeInTheDocument();

    const withTest = makeFlowNode("p.TestRefund", { name: "TestRefund", state: "new", test: true });
    pending.resolve(graphResult({ nodes: [...refundNodes, withTest], options: { ...DEFAULTS, includeTests: true } }));

    expect(await box("internal/app/app.go")).toBeInTheDocument();
    await vi.waitFor(() => expect(screen.queryByText("Updating the graph")).not.toBeInTheDocument());
    expect(await box("internal/refund/service.go")).toBeInTheDocument();
  });

  it("remembers the options across tab switches", async () => {
    const user = await openFlow();
    await box("internal/refund/service.go");
    await user.click(toggle(/^Show callers/));
    await vi.waitFor(() => expect(getFlow).toHaveBeenCalledTimes(2));

    await user.click(tab("Diff"));
    await user.click(tab("Flow"));

    expect(toggle(/^Show callers/)).toHaveAttribute("aria-pressed", "true");
    expect(getFlow).toHaveBeenCalledTimes(2);
  });
});

describe("functions marked as reviewed", () => {
  const REF = { target: BRANCH, oldSha: changeSet.oldSha, newSha: changeSet.newSha };
  const markedAs = (status: "reviewed" | "reopened") =>
    makeReviewState({ functions: [{ id: id.request, status, reviewedAt: "2026-11-19T09:00:00Z" }] });

  it("asks for the state of the changed functions of the graph, and names both marks in the legend", async () => {
    await openFunctions();

    await vi.waitFor(() => {
      const [, , functions] = vi.mocked(getReviewState).mock.lastCall ?? [];
      expect(functions?.map((span) => span.id)).toContain(id.request);
      // Untouched and external functions cannot be marked, so they are not asked about.
      expect(functions?.map((span) => span.id)).not.toContain(id.queryRow);
      expect(functions?.map((span) => span.id)).not.toContain(id.gateway);
    });
    expect(screen.getByRole("list", { name: "Review marks" })).toHaveTextContent("ReviewedChanged since your review");
  });

  it("marks a function from its popover, and unmarks a reviewed one", async () => {
    vi.mocked(setFunctionReviewed).mockResolvedValue(undefined);
    await openFunctions();
    await clickBox("Service.Request");
    vi.mocked(getReviewState).mockResolvedValue(markedAs("reviewed"));

    await clickButton("Mark as reviewed");

    expect(setFunctionReviewed).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      REF,
      { id: id.request, file: "internal/refund/service.go", startLine: 44, endLine: 71, deleted: false },
      true,
    );
    await vi.waitFor(async () => expect(await box("Service.Request")).toHaveAccessibleName(/, reviewed$/));
    await clickButton("✓ Reviewed");
    expect(setFunctionReviewed).toHaveBeenLastCalledWith("p-1", REF, expect.objectContaining({ id: id.request }), false);
  });

  it("rings a function that changed since it was reviewed and offers to mark it again", async () => {
    vi.mocked(getReviewState).mockResolvedValue(markedAs("reopened"));
    await openFunctions();

    expect(await box("Service.Request")).toHaveAccessibleName(/, changed since your review$/);
    await clickBox("Service.Request");
    await clickButton("Mark as reviewed again");
    expect(setFunctionReviewed).toHaveBeenCalledWith("p-1", REF, expect.objectContaining({ id: id.request }), true);
  });

  it("offers no mark for a function the review did not change", async () => {
    await openFunctions();

    await clickBox("DB.QueryRow");

    expect(within(popover()).queryByRole("button", { name: /reviewed/i })).not.toBeInTheDocument();
  });
});

describe("Flow tab, with findings, limits and checks", () => {
  const request = { id: id.request, qualifiedName: "refund.Service.Request" };
  // Service.Request: 2 findings (high and medium), 3 limits, 1 check. Validate: no test runs it. A check on
  // validate.go outside any function, and a dismissed finding that counts nowhere.
  const signalsReport = () =>
    makeQualityReport({
      functions: [
        makeQualityFunction("refund.Service.Request", {
          id: id.request,
          name: "Service.Request",
          file: "internal/refund/service.go",
          startLine: 44,
          endLine: 71,
          complexity: 14,
          changedLines: { covered: 12, coverable: 25 },
          crap: 42,
        }),
        makeQualityFunction("refund.Validate", {
          id: id.validate,
          name: "Validate",
          file: "internal/refund/validate.go",
          startLine: 12,
          endLine: 31,
          complexity: 4,
          changedLines: { covered: 0, coverable: 9 },
          crap: 20,
          untested: true,
        }),
      ],
      crossed: [
        { kind: "complexity", function: request, value: 14, limit: 10 },
        { kind: "crap", function: request, value: 42, limit: 30 },
        { kind: "changed_lines_coverage", function: request, value: 0.48, limit: 0.8 },
        // Replaced by "no test runs it": the function does not count twice.
        { kind: "changed_lines_coverage", function: { id: id.validate, qualifiedName: "refund.Validate" }, value: 0, limit: 0.8 },
      ],
      checkFindings: [
        makeLocatedCheck({ line: 60, function: request }),
        makeLocatedCheck({
          tool: "go vet",
          rule: "printf",
          file: "internal/refund/validate.go",
          line: 3,
          message: "fmt.Sprintf format %d has arg id of wrong type string",
          function: null,
        }),
      ],
    });
  const signalsFindings = () => ({
    ...NO_INSIGHTS,
    findings: [
      makeFinding("f-medium", { severity: "medium", title: "Amount is not checked", line: 62, endLine: 62, location: { kind: "found", line: 62, endLine: 62, moved: false } }),
      makeFinding("f-high", { title: "Refund can be paid out and never recorded" }),
      makeFinding("f-gone", { verdict: "dismissed", title: "Dismissed one" }),
    ],
  });

  beforeEach(() => {
    vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: signalsReport() });
    vi.mocked(getInsights).mockResolvedValue(signalsFindings());
  });

  it("puts a badge on the boxes, says it in their names, and adds its legend", async () => {
    await openFlow();

    // A closed file holds everything in it, outside its functions too.
    const service = await box("internal/refund/service.go");
    await vi.waitFor(() =>
      expect(service).toHaveAccessibleName(
        /, 2 model findings, the most severe of high severity, 3 limits crossed, 1 static check finding$/,
      ),
    );
    expect(await box("internal/refund/validate.go")).toHaveAccessibleName(
      /, 1 static check finding, 1 function no test runs$/,
    );
    const legend = screen.getByRole("list", { name: "Findings, limits and checks" });
    expect(legend).toHaveTextContent("High");
    expect(legend).toHaveTextContent("Static check finding (go vet, ruff, ESLint, clippy...)");

    await openFunctionsLevel();
    const fn = await box("Service.Request");
    expect(fn).toHaveAccessibleName(
      "Service.Request, new, 28 added, 0 removed, 2 model findings, the most severe of high severity, " +
        "3 limits crossed, 1 static check finding",
    );
    // The badge: the findings (their count), the limits (their count), the one check (no count), in this order.
    const badge = fn.querySelector("[data-signal-badge]");
    expect(badge).toHaveAttribute("aria-hidden", "true");
    expect([...(badge?.children ?? [])].map((part) => part.textContent)).toEqual(["2", "", "3", "", ""]);
    expect(badge?.firstElementChild).toHaveClass("text-sev-high");
    // No test runs Validate: the flask, which stands for its coverage, with no number.
    const validate = await box("Validate");
    expect(validate).toHaveAccessibleName(/, no test runs it$/);
    expect(validate.querySelector("[data-signal-badge]")?.textContent).toBe("");
    // A box without signals has no badge and keeps its name.
    expect((await box("Repo.Get")).querySelector("[data-signal-badge]")).toBeNull();
  });

  it("lists the limits, the checks and the model's findings in the popover, the complexity moved out of the facts", async () => {
    vi.mocked(getFileVersions).mockImplementation(async (_id, _old, _new, path) => ({
      old: textSide(`old ${path}\n`),
      new: textSide(`new ${path}\n`),
    }));
    const user = userEvent.setup();
    renderScreen();
    await openReviewTab(user, "Flow");
    await openFunctionsLevel(user);
    await clickBox("Service.Request");

    const dialog = within(popover());
    // The facts are the popover's first list of terms; the limits keep the complexity, shown once.
    const facts = within(popover().querySelector("dl")!).getAllByRole("term").map((term) => term.textContent);
    expect(facts).toEqual(["Callers shown", "Calls shown"]);
    expect(dialog.getAllByText("Cyclomatic complexity")).toHaveLength(1);
    const limits = within(dialog.getByRole("region", { name: "Quality limits" }));
    expect(limits.getByText("3 crossed")).toBeInTheDocument();
    expect(limits.getByText("Cyclomatic complexity").closest("[data-limit]")).toHaveAttribute("data-over", "true");
    expect(limits.getByText("42")).toHaveClass("text-del");
    expect(limits.getByText("48%")).toBeInTheDocument();
    expect(limits.getByText("/ minimum 80%")).toBeInTheDocument();

    const checks = within(dialog.getByRole("region", { name: "Static checks" }));
    const message = checks.getByRole("button", { name: "this value of err is never used" });
    expect(message).toHaveAttribute("lang", "en");
    expect(checks.getByText("Line 60")).toBeInTheDocument();
    expect(checks.getByText("Warning")).toHaveAttribute("data-check-severity", "warning");

    const findings = within(dialog.getByRole("region", { name: "Model findings" }));
    expect(findings.getAllByRole("listitem").map((item) => item.querySelector("button")?.textContent)).toEqual([
      "Refund can be paid out and never recorded",
      "Amount is not checked",
    ]);
    expect(findings.getByText("Correctness, lines 52–56")).toBeInTheDocument();

    await user.click(findings.getByRole("button", { name: "See in Diff, line 52" }));
    await vi.waitFor(() =>
      expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-scroll-line", "52"),
    );
    expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-path", "internal/refund/service.go");
  });

  it("marks duplicated code on the function holding it and lists it in the popover", async () => {
    vi.mocked(getQuality).mockResolvedValue({
      kind: "report",
      report: { ...signalsReport(), duplicates: [makeDuplicate({ function: request })] },
    });
    vi.mocked(getFileVersions).mockImplementation(async (_id, _old, _new, path) => ({
      old: textSide(`old ${path}\n`),
      new: textSide(`new ${path}\n`),
    }));
    const user = userEvent.setup();
    renderScreen();
    await openReviewTab(user, "Flow");
    await openFunctionsLevel(user);

    const fn = await box("Service.Request");
    await vi.waitFor(() => expect(fn).toHaveAccessibleName(/, 1 static check finding, 1 duplicated block$/));
    expect(screen.getByRole("list", { name: "Findings, limits and checks" })).toHaveTextContent("Duplicated code");
    await clickBox("Service.Request");

    const duplication = within(within(popover()).getByRole("region", { name: "Duplication" }));
    expect(duplication.getByText("1 duplicate")).toBeInTheDocument();
    expect(duplication.getByRole("listitem")).toHaveTextContent("copies internal/payment/money.go:40-47");
    expect(duplication.getByText("Lines 60–67 · 74 tokens, 8 lines")).toBeInTheDocument();
    await user.click(duplication.getByRole("button", { name: "See in Diff, lines 60–67" }));
    await vi.waitFor(() => expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-scroll-line", "60"));
    expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-path", "internal/refund/service.go");
  });

  it("marks the mutants that survived on the function holding them and lists them in the popover", async () => {
    vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: signalsReport() });
    const run = makeMutationRun({
      mutants: makeMutationRun().mutants.map((mutant) => ({ ...mutant, function: request })),
    });
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ run }));
    vi.mocked(getFileVersions).mockImplementation(async (_id, _old, _new, path) => ({
      old: textSide(`old ${path}\n`),
      new: textSide(`new ${path}\n`),
    }));
    const user = userEvent.setup();
    renderScreen();
    await openReviewTab(user, "Flow");
    await openFunctionsLevel(user);

    const fn = await box("Service.Request");
    await vi.waitFor(() => expect(fn).toHaveAccessibleName(/, 1 static check finding, 2 mutants no test caught$/));
    expect(screen.getByRole("list", { name: "Findings, limits and checks" })).toHaveTextContent(
      "A change no test caught (mutation testing)",
    );
    await clickBox("Service.Request");

    const mutation = within(within(popover()).getByRole("region", { name: "Mutation testing" }));
    expect(mutation.getByText("2 mutants")).toBeInTheDocument();
    expect(mutation.getAllByRole("listitem")[0]).toHaveTextContent(
      "if balance >= amount { became if balance > amount { and no test failed",
    );
    expect(mutation.getByText("Line 58 · Call removed")).toBeInTheDocument();
    await user.click(mutation.getByRole("button", { name: "See in Diff, line 52" }));
    await vi.waitFor(() => expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-scroll-line", "52"));
    expect(screen.getByTestId("diff-editor")).toHaveAttribute("data-path", "internal/refund/service.go");
  });

  it("says no test runs a function in place of its coverage", async () => {
    await openFlow();
    await openFunctionsLevel();
    await clickBox("Validate");

    const limits = within(within(popover()).getByRole("region", { name: "Quality limits" }));
    expect(limits.getByText("no test")).toBeInTheDocument();
    expect(limits.getByText("No test runs this function")).toBeInTheDocument();
    expect(limits.getByText("coverage of its 9 changed lines")).toBeInTheDocument();
    expect(limits.queryByText("Coverage of changed lines")).not.toBeInTheDocument();
    expect(within(popover()).queryByRole("region", { name: "Model findings" })).not.toBeInTheDocument();
  });

  it("draws nothing of it when there is no signal", async () => {
    vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: makeQualityReport({ functions: [], crossed: [] }) });
    vi.mocked(getInsights).mockResolvedValue(NO_INSIGHTS);
    await openFlow();
    await openFunctionsLevel();
    await clickBox("Service.Request");

    expect(screen.queryByRole("list", { name: "Findings, limits and checks" })).not.toBeInTheDocument();
    expect(document.querySelector("[data-signal-badge]")).toBeNull();
    expect(await box("Service.Request")).toHaveAccessibleName("Service.Request, new, 28 added, 0 removed");
    expect(within(popover()).queryByRole("region", { name: "Quality limits" })).not.toBeInTheDocument();
  });
});

async function openFunctionsLevel(user = userEvent.setup()) {
  await box("internal/refund/service.go");
  await user.click(level("Functions"));
  await box("Service.Request");
}
