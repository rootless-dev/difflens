import "@/test/elk";
import "@/test/react-flow";

import { act, fireEvent, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeAll, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { buildView, groupFunctions, NOTHING_OPEN } from "@/features/review/flow/flow-groups";
import { layoutFlow } from "@/features/review/flow/flow-layout";
import { EXPLAINED_EXTRA, NODE_HEIGHT, nodeSize } from "@/features/review/flow/flow-model";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  cancelExplanation,
  type Explanations,
  explainFunctions,
  getExplanations,
  getFileVersions,
  getFlow,
  getModelInUse,
  getReviewState,
  getTestsState,
  listChangedFiles,
  planExplanationsTranslation,
  type ReviewTarget,
  translateExplanations,
} from "@/lib/api";
import { setLanguage } from "@/i18n";
import { makeBrief, makeExplanation } from "@/test/explanation-fixtures";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { REFUND_IDS as id, refundFlowGraph } from "@/test/flow-fixtures";
import { makeModelInUse } from "@/test/insights-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getFlow: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
  getExplanations: vi.fn(),
  getModelInUse: vi.fn(),
  explainFunctions: vi.fn(),
  cancelExplanation: vi.fn(),
  planExplanationsTranslation: vi.fn(),
  translateExplanations: vi.fn(),
}));

vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => <div data-testid="diff-editor" data-path={props.path} />,
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const changeSet = makeChangeSet([
  makeChangedFile("internal/refund/handler.go", { status: "added" }),
  makeChangedFile("internal/refund/service.go", { status: "added" }),
  makeChangedFile("internal/refund/validate.go", { status: "added" }),
]);
const REVIEW = { target: changeSet.target, oldSha: changeSet.oldSha, newSha: changeSet.newSha };
const MODEL = "Claude Code, model opus";

let bar: HTMLDivElement;

const escape = (text: string) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
const boxesNamed = (name: string) =>
  screen
    .queryAllByRole("button", { name: new RegExp(`^${escape(name)},`) })
    .filter((element) => element.closest(".flow-canvas"));
const box = (name: string) =>
  vi.waitFor(() => {
    const [found, ...others] = boxesNamed(name);
    if (!found || others.length > 0) throw new Error(`Expected one box named ${name}`);
    return found;
  });
const level = (name: string) => within(screen.getByRole("group", { name: "Level" })).getByRole("button", { name });

/** Detailed explanations (as every one written before short ones was), and short ones. */
const explained = (functions: Explanations["functions"], briefs: Explanations["briefs"] = []): Explanations => ({
  functions,
  briefs,
});
const shortly = (briefs: Explanations["briefs"]): Explanations => explained([], briefs);

async function openFunctions() {
  const user = userEvent.setup();
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
  await openReviewTab(user, "Flow");
  await box("internal/refund/service.go");
  await user.click(level("Functions"));
  await box("Service.Request");
  return user;
}

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
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a\n"), new: textSide("b\n") });
  vi.mocked(getFlow).mockResolvedValue({ kind: "graph", graph: refundFlowGraph() });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(getModelInUse).mockResolvedValue(makeModelInUse());
  vi.mocked(getExplanations).mockResolvedValue(explained([]));
  vi.mocked(cancelExplanation).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("explanations of functions", () => {
  it("makes room on an explained box for two lines", () => {
    const node = refundFlowGraph().nodes[0];
    expect(nodeSize(node, false, true).height).toBe(NODE_HEIGHT + EXPLAINED_EXTRA);
    expect(nodeSize(node).height).toBe(NODE_HEIGHT);
  });

  it("shows one kept before short ones existed as the detailed one, with Explain there for the short one", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained([
        makeExplanation(id.create, { why: "Inferred: support needs it.", whyInferred: true }),
        makeExplanation(id.showPayment, {
          kind: "context",
          whatChanged: "",
          why: "",
          purpose: "Shows a payment. Used by the dashboard.",
        }),
      ]),
    );
    await openFunctions();

    // The first sentence on the box, read with it.
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(
        expect.stringContaining("explained: Adds the `POST /refunds` handler."),
      ),
    );
    expect(await box("Handler.Show")).toHaveAccessibleName(expect.stringContaining("explained: Shows a payment."));
    const spans = vi.mocked(getExplanations).mock.calls.at(-1)![2];
    // Every function with a place in the code, the unchanged ones too: an explanation can be of the context.
    expect(spans.map((span) => span.id)).toEqual(expect.arrayContaining([id.create, id.showPayment]));
    expect(spans.map((span) => span.id)).not.toContain(id.gateway);
    expect(getExplanations).toHaveBeenCalledWith("p-1", REVIEW, expect.any(Array));
    expect(explainFunctions).not.toHaveBeenCalled();

    fireEvent.click(await box("Handler.Create"));
    const popover = within(screen.getByRole("dialog"));
    // In the second column, once the popover has widened for it: a line that says there is only the detailed one,
    // which is shown once asked for, asking no model.
    const section = within(await popover.findByRole("region", { name: /Explanation/ }));
    expect(section.getByText("Only a detailed explanation was written for this function.")).toBeInTheDocument();
    expect(section.queryByText("What changed")).toBeNull();
    fireEvent.click(section.getByRole("button", { name: "Show details" }));
    expect(explainFunctions).not.toHaveBeenCalled();
    expect(section.getByText("What changed")).toBeInTheDocument();
    expect(section.getByText("Inferred: support needs it.")).toBeInTheDocument();
    expect(section.getByText("inferred")).toHaveAttribute(
      "title",
      "Nothing in the card, the pull request or the commits says why: this is the model's inference.",
    );
    expect(section.getByText("Turns a refund request over HTTP into a call to the refund service.")).toBeInTheDocument();
    expect(section.getByText(/Written by Claude Code, model opus/)).toBeInTheDocument();
    // It is the detailed one: asking for the short one is still there, and so is detailing again.
    expect(section.getByRole("button", { name: "Explain" })).toHaveAttribute(
      "title",
      `Explain Handler.Create with ${MODEL}: one model call`,
    );
    expect(section.getByRole("button", { name: "Explain in detail again" })).toHaveAttribute(
      "title",
      `Ask ${MODEL} again what changed in Handler.Create, why and what it is for: one model call`,
    );
  });

  it("shows the short one on the box and in the column, then Explain in detail for the full one", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      shortly([makeBrief(id.create, { text: "New `POST /refunds` handler. It calls the refund service." })]),
    );
    const detail = deferred<Explanations>();
    vi.mocked(explainFunctions).mockReturnValue(detail.promise);
    await openFunctions();

    // Its first lines on the box.
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(
        expect.stringContaining("explained: New `POST /refunds` handler. It calls the refund service."),
      ),
    );
    fireEvent.click(await box("Handler.Create"));
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    const column = within(dialog.querySelector<HTMLElement>("[data-column=aside]")!);
    expect(column.getByText("POST /refunds").tagName).toBe("CODE");
    expect(column.queryByText("What changed")).toBeNull();
    const button = column.getByRole("button", { name: "Explain in detail" });
    expect(button).toHaveAttribute(
      "title",
      `Ask ${MODEL} what changed in Handler.Create, why and what it is for: one model call`,
    );
    expect(column.getByRole("button", { name: "Explain again" })).toBeInTheDocument();
    expect(explainFunctions).not.toHaveBeenCalled();

    // One call, for this function alone, at the detailed level.
    fireEvent.click(button);
    expect(explainFunctions).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, [id.create], "detail", null, expect.any(String));
    expect(column.getByRole("status")).toHaveTextContent("Explaining Handler.Create in detail");
    const cancel = column.getByRole("button", { name: "Cancel" });
    fireEvent.click(cancel);
    expect(cancelExplanation).toHaveBeenCalledExactlyOnceWith(vi.mocked(explainFunctions).mock.calls[0][5]);
    // The short one is not being written again meanwhile.
    expect(column.getByRole("button", { name: "Explain again" })).toBeEnabled();

    await act(async () =>
      detail.resolve(
        explained(
          [makeExplanation(id.create, { why: "Inferred: support needs it.", whyInferred: true })],
          [makeBrief(id.create, { text: "New `POST /refunds` handler. It calls the refund service." })],
        ),
      ),
    );
    // The three parts below the short text, in the same column, shown as they arrive.
    const sections = within(await column.findByRole("region", { name: "In detail" }));
    expect(column.getByRole("button", { name: "Hide details" })).toHaveAttribute("aria-expanded", "true");
    expect(sections.getByText("What changed")).toBeInTheDocument();
    expect(sections.getByText("Inferred: support needs it.")).toBeInTheDocument();
    expect(sections.getByText("inferred")).toBeInTheDocument();
    expect(sections.getByText("What it is for")).toBeInTheDocument();
    // The short text stays above them (both name the endpoint).
    expect(column.getAllByText("POST /refunds")).toHaveLength(2);
    expect(column.getByRole("button", { name: "Explain in detail again" })).toBeInTheDocument();
    // The box keeps the short one.
    expect(await box("Handler.Create")).toHaveAccessibleName(
      expect.stringContaining("explained: New `POST /refunds` handler."),
    );

    // Explain again writes the short one again.
    vi.mocked(explainFunctions).mockResolvedValue(shortly([makeBrief(id.create)]));
    fireEvent.click(column.getByRole("button", { name: "Explain again" }));
    expect(explainFunctions).toHaveBeenLastCalledWith("p-1", REVIEW, [id.create], "brief", null, expect.any(String));
  });

  it("names the model of each level's task in the tooltips of its buttons", async () => {
    vi.mocked(getModelInUse).mockImplementation(async (task) =>
      task === "detailed_explanation"
        ? makeModelInUse({ provider: "openai", providerName: "OpenAI API", model: "gpt-5.5", parallelCalls: 3 })
        : makeModelInUse(),
    );
    vi.mocked(getExplanations).mockResolvedValue(shortly([makeBrief(id.create)]));
    await openFunctions();

    expect(getModelInUse).toHaveBeenCalledWith("brief_explanation");
    expect(getModelInUse).toHaveBeenCalledWith("detailed_explanation");
    // A box's button explains in short.
    expect(await screen.findByRole("button", { name: `Explain Validate with ${MODEL}: one model call` })).toBeInTheDocument();

    fireEvent.click(await box("Handler.Create"));
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    const column = within(dialog.querySelector<HTMLElement>("[data-column=aside]")!);
    expect(column.getByRole("button", { name: "Explain in detail" })).toHaveAttribute(
      "title",
      "Ask OpenAI API, model gpt-5.5 what changed in Handler.Create, why and what it is for: one model call",
    );
    expect(column.getByRole("button", { name: "Explain again" })).toHaveAttribute(
      "title",
      `Explain Handler.Create again with ${MODEL}: one model call`,
    );
    expect(explainFunctions).not.toHaveBeenCalled();
  });

  it("says why a detailed explanation failed, beside its button", async () => {
    vi.mocked(getExplanations).mockResolvedValue(shortly([makeBrief(id.create)]));
    vi.mocked(explainFunctions).mockRejectedValue(apiError("claude_cli_failed", "The model did not answer."));
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
    );
    fireEvent.click(await box("Handler.Create"));
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    const column = within(dialog.querySelector<HTMLElement>("[data-column=aside]")!);
    fireEvent.click(column.getByRole("button", { name: "Explain in detail" }));
    expect(
      await column.findByText("The explanation failed: The model did not answer.", { exact: false }),
    ).toBeInTheDocument();
    expect(column.getByRole("button", { name: "Explain in detail" })).toBeEnabled();
  });

  it("asks the model for one function only on its button's click, which can stop it", async () => {
    const answer = deferred<Explanations>();
    vi.mocked(explainFunctions).mockReturnValue(answer.promise);
    await openFunctions();

    const explain = await screen.findByRole("button", { name: `Explain Validate with ${MODEL}: one model call` });
    expect(explainFunctions).not.toHaveBeenCalled();
    fireEvent.click(explain);
    expect(explainFunctions).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, [id.validate], "brief", null, expect.any(String));
    const runId = vi.mocked(explainFunctions).mock.calls[0][5];

    fireEvent.click(await screen.findByRole("button", { name: "Stop explaining Validate" }));
    expect(cancelExplanation).toHaveBeenCalledExactlyOnceWith(runId);

    await act(async () => answer.resolve(shortly([makeBrief(id.validate, { text: "Checks the amount." })])));
    await waitFor(async () =>
      expect(await box("Validate")).toHaveAccessibleName(expect.stringContaining("explained: Checks the amount.")),
    );
  });

  it("says why an explanation failed, and lets the button try again", async () => {
    vi.mocked(explainFunctions).mockRejectedValue(apiError("claude_cli_failed", "The model did not answer."));
    await openFunctions();
    fireEvent.click(await screen.findByRole("button", { name: `Explain Validate with ${MODEL}: one model call` }));
    const again = await screen.findByRole("button", { name: `Explain Validate with ${MODEL}: one model call` });
    await waitFor(() => expect(again.getAttribute("title")).toMatch(/^The explanation failed: The model did not answer\./));
  });

  it("explains every function of a group in one call from the group's button", async () => {
    const user = userEvent.setup();
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await openReviewTab(user, "Flow");
    await box("internal/refund/service.go");
    vi.mocked(explainFunctions).mockResolvedValue(explained([]));

    const graph = refundFlowGraph();
    const inFile = graph.nodes.filter((node) => node.file === "internal/storage/refund_repo.go").map((node) => node.id);
    const name = inFile.length === 1 ? "the function of refund_repo.go" : `the ${inFile.length} functions of refund_repo.go`;
    fireEvent.click(await screen.findByRole("button", { name: `Explain ${name} with ${MODEL}: one model call` }));
    expect(explainFunctions).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, inFile, "brief", null, expect.any(String));
  });

  it("opens every popover one column wide, and widens it for an explanation the function already has", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      shortly([makeBrief(id.create), makeBrief(id.showPayment, { kind: "context", text: "Shows a payment." })]),
    );
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
    );

    fireEvent.click(await box("Validate"));
    const dialog = screen.getByRole("dialog", { name: /Validate/ });
    expect(dialog).toHaveAttribute("data-columns", "1");
    expect(dialog.querySelector("[data-column=aside]")).toBeNull();
    expect(within(dialog).getByRole("button", { name: "Explain" })).toBeInTheDocument();
    expect(dialog).not.toHaveClass("motion-safe:transition-[width]");

    // The same popover, now of a function explained before it was opened: one column at first, then two.
    fireEvent.click(await box("Handler.Create"));
    expect(screen.getByRole("dialog", { name: /Handler\.Create/ })).toBe(dialog);
    expect(dialog).toHaveAttribute("data-columns", "1");
    expect(dialog.querySelector("[data-column=aside]")).toBeNull();
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    // It widens, and the explanation fades in once the widening is over.
    expect(dialog).toHaveClass("motion-safe:transition-[width]", "motion-safe:duration-250");
    const aside = dialog.querySelector<HTMLElement>("[data-column=aside]")!;
    expect(aside).toHaveClass("motion-safe:animate-in", "motion-safe:fade-in", "motion-safe:delay-250");
    expect(within(aside).getByRole("region", { name: /Explanation/ })).toBeInTheDocument();
    expect(within(dialog).queryByRole("button", { name: "Explain" })).toBeNull();

    // Another explained function is a fresh opening: narrow at once, without animating, then widened again.
    fireEvent.click(await box("Handler.Show"));
    expect(screen.getByRole("dialog", { name: /Handler\.Show/ })).toBe(dialog);
    expect(dialog).toHaveAttribute("data-columns", "1");
    expect(dialog).not.toHaveClass("motion-safe:transition-[width]");
    expect(dialog.querySelector("[data-column=aside]")).toBeNull();
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    expect(dialog).toHaveClass("motion-safe:transition-[width]");
    const next = dialog.querySelector<HTMLElement>("[data-column=aside]")!;
    expect(within(next).getByText("Shows a payment.")).toBeInTheDocument();
  });

  it("opens at its final width, without animating, when the system asks for less motion", async () => {
    vi.stubGlobal("matchMedia", (media: string) => ({
      media,
      matches: media === "(prefers-reduced-motion: reduce)",
      onchange: null,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      dispatchEvent: () => false,
    }));
    try {
      vi.mocked(getExplanations).mockResolvedValue(explained([makeExplanation(id.create)]));
      await openFunctions();
      await waitFor(async () =>
        expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
      );

      fireEvent.click(await box("Handler.Create"));
      const dialog = screen.getByRole("dialog", { name: /Handler\.Create/ });
      expect(dialog).toHaveAttribute("data-columns", "2");
      expect(dialog).not.toHaveClass("motion-safe:transition-[width]");
      const aside = dialog.querySelector<HTMLElement>("[data-column=aside]")!;
      expect(within(aside).getByRole("region", { name: /Explanation/ })).toBeInTheDocument();
      expect(aside).not.toHaveClass("motion-safe:animate-in");
    } finally {
      vi.unstubAllGlobals();
    }
  });

  it("keeps one column while explaining and fades the explanation in beside it when it arrives", async () => {
    const answer = deferred<Explanations>();
    vi.mocked(explainFunctions).mockReturnValue(answer.promise);
    await openFunctions();

    const validate = await box("Validate");
    fireEvent.click(validate);
    const dialog = screen.getByRole("dialog");
    expect(dialog).toHaveAttribute("data-columns", "1");
    fireEvent.click(within(dialog).getByRole("button", { name: "Explain" }));
    expect(explainFunctions).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, [id.validate], "brief", null, expect.any(String));
    // The keyboard is somewhere else meanwhile.
    act(() => validate.focus());

    // While the model writes: a spinner and a way to stop it, in the same single column.
    expect(within(dialog).getByRole("status")).toHaveTextContent("Explaining Validate");
    expect(within(dialog).getByRole("button", { name: "Cancel" })).toBeInTheDocument();
    expect(dialog).toHaveAttribute("data-columns", "1");
    expect(dialog.querySelector("[data-column=aside]")).toBeNull();

    await act(async () => answer.resolve(shortly([makeBrief(id.validate, { text: "Checks the amount." })])));
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    const aside = dialog.querySelector<HTMLElement>("[data-column=aside]")!;
    expect(dialog).toHaveClass("motion-safe:transition-[width]");
    expect(aside).toHaveClass("motion-safe:animate-in", "motion-safe:fade-in");
    expect(within(aside).getByText("Checks the amount.")).toBeInTheDocument();
    expect(within(aside).getByRole("button", { name: "Explain again" })).toBeInTheDocument();
    expect(within(aside).getByRole("button", { name: "Explain in detail" })).toBeInTheDocument();
    // The way to ask for a first explanation has gone from the first column.
    expect(within(dialog).queryByRole("button", { name: "Explain" })).toBeNull();
    // Focus was not in the popover: it stays where it is.
    expect(validate).toHaveFocus();
  });

  it("moves the keyboard on to the column's first button when the explanation it asked for arrives", async () => {
    const answer = deferred<Explanations>();
    vi.mocked(explainFunctions).mockReturnValue(answer.promise);
    await openFunctions();

    fireEvent.click(await box("Validate"));
    const dialog = screen.getByRole("dialog");
    // Pressed from the keyboard: focused, then clicked. The button goes as the model starts writing.
    const explain = within(dialog).getByRole("button", { name: "Explain" });
    act(() => explain.focus());
    fireEvent.click(explain);
    expect(explainFunctions).toHaveBeenCalledOnce();
    expect(explain).not.toBeInTheDocument();

    await act(async () => answer.resolve(shortly([makeBrief(id.validate)])));
    const detail = await within(dialog).findByRole("button", { name: "Explain in detail" });
    await waitFor(() => expect(detail).toHaveFocus());
  });

  it("leaves the keyboard where it is when the function was explained before the popover opened", async () => {
    vi.mocked(getExplanations).mockResolvedValue(shortly([makeBrief(id.create), makeBrief(id.validate)]));
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
    );

    const create = await box("Handler.Create");
    act(() => create.focus());
    fireEvent.click(create);
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    expect(within(dialog).getByRole("button", { name: "Explain again" })).not.toHaveFocus();
    expect(create).toHaveFocus();

    // Opened from a call in the list, whose button goes with the popover's change: still not taken into the column.
    fireEvent.click(await box("Service.Request"));
    const call = within(dialog).getByRole("button", { name: "refund.Validate" });
    act(() => call.focus());
    fireEvent.click(call);
    expect(call).not.toBeInTheDocument();
    expect(screen.getByRole("dialog", { name: /Validate/ })).toBe(dialog);
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    expect(within(dialog).getByRole("button", { name: "Explain again" })).not.toHaveFocus();
  });

  it("says in the first column why a first explanation failed, without opening the second", async () => {
    vi.mocked(explainFunctions).mockRejectedValue(apiError("claude_cli_failed", "The model did not answer."));
    await openFunctions();

    fireEvent.click(await box("Validate"));
    const dialog = screen.getByRole("dialog");
    fireEvent.click(within(dialog).getByRole("button", { name: "Explain" }));
    expect(
      await within(dialog).findByText("The explanation failed: The model did not answer.", { exact: false }),
    ).toBeInTheDocument();
    expect(within(dialog).getByRole("button", { name: "Explain" })).toBeInTheDocument();
    expect(dialog).toHaveAttribute("data-columns", "1");
    expect(dialog.querySelector("[data-column=aside]")).toBeNull();
  });

  it("offers to explain again a function whose code changed since", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained([makeExplanation(id.create, { outdated: true })], [makeBrief(id.create, { outdated: true })]),
    );
    await openFunctions();

    expect(
      await screen.findByRole("button", { name: `Explain Handler.Create again with ${MODEL}: one model call` }),
    ).toBeInTheDocument();
    fireEvent.click(await box("Handler.Create"));
    await waitFor(() => expect(screen.getByRole("dialog")).toHaveAttribute("data-columns", "2"));
    // In the second column, for both levels, with the tag that says so.
    const aside = within(screen.getByRole("dialog").querySelector<HTMLElement>("[data-column=aside]")!);
    fireEvent.click(aside.getByRole("button", { name: "Show details" }));
    expect(aside.getAllByText("Its code changed since it was explained.")).toHaveLength(2);
    expect(aside.getByText("outdated")).toBeInTheDocument();
    expect(aside.getByRole("region", { name: "In detail" })).toHaveClass("text-muted-foreground");
  });

  it("tells only the short one is outdated when the detailed one was written for the code as it is", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained([makeExplanation(id.create)], [makeBrief(id.create, { outdated: true })]),
    );
    await openFunctions();
    fireEvent.click(await box("Handler.Create"));
    await waitFor(() => expect(screen.getByRole("dialog")).toHaveAttribute("data-columns", "2"));
    const aside = within(screen.getByRole("dialog").querySelector<HTMLElement>("[data-column=aside]")!);
    fireEvent.click(aside.getByRole("button", { name: "Show details" }));
    expect(aside.getAllByText("Its code changed since it was explained.")).toHaveLength(1);
    expect(aside.getByRole("region", { name: "In detail" })).not.toHaveClass("text-muted-foreground");
  });

  it("shows a kept detailed explanation only when asked, asking no model, and hides it again", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained(
        [makeExplanation(id.create, { why: "Inferred: support needs it.", whyInferred: true })],
        [makeBrief(id.create, { text: "New `POST /refunds` handler." })],
      ),
    );
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
    );
    fireEvent.click(await box("Handler.Create"));
    const dialog = screen.getByRole("dialog");
    await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
    const column = within(dialog.querySelector<HTMLElement>("[data-column=aside]")!);

    // Only the short one, its byline and the buttons at first; detailing again waits for the detail to be shown.
    expect(column.getByText("POST /refunds").tagName).toBe("CODE");
    expect(column.getAllByText(/Written by Claude Code, model opus/)).toHaveLength(1);
    expect(column.queryByRole("region", { name: "In detail" })).toBeNull();
    expect(column.queryByText("Only a detailed explanation was written for this function.")).toBeNull();
    expect(column.queryByRole("button", { name: "Explain in detail" })).toBeNull();
    expect(column.queryByRole("button", { name: "Explain in detail again" })).toBeNull();
    expect(column.getByRole("button", { name: "Explain again" })).toBeInTheDocument();
    const show = column.getByRole("button", { name: "Show details" });
    expect(show).toHaveAttribute("aria-expanded", "false");

    fireEvent.click(show);
    const detail = column.getByRole("region", { name: "In detail" });
    expect(within(detail).getByText("Inferred: support needs it.")).toBeInTheDocument();
    expect(explainFunctions).not.toHaveBeenCalled();
    // The same button hides it again, so the keyboard stays on it.
    const hide = column.getByRole("button", { name: "Hide details" });
    expect(hide).toBe(show);
    expect(hide).toHaveAttribute("aria-expanded", "true");
    expect(hide).toHaveAttribute("aria-controls", detail.id);
    expect(column.getByRole("button", { name: "Explain in detail again" })).toHaveAttribute(
      "title",
      `Ask ${MODEL} again what changed in Handler.Create, why and what it is for: one model call`,
    );
    expect(column.getByRole("button", { name: "Explain again" })).toBeInTheDocument();

    fireEvent.click(hide);
    expect(column.queryByRole("region", { name: "In detail" })).toBeNull();
    expect(column.getByRole("button", { name: "Show details" })).toBeInTheDocument();
    expect(column.queryByRole("button", { name: "Explain in detail again" })).toBeNull();

    // Explain in detail again asks the model once, for this function alone.
    fireEvent.click(column.getByRole("button", { name: "Show details" }));
    vi.mocked(explainFunctions).mockReturnValue(new Promise(() => {}));
    fireEvent.click(column.getByRole("button", { name: "Explain in detail again" }));
    expect(explainFunctions).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, [id.create], "detail", null, expect.any(String));
    // Hidden while the model writes, its run is still there to be stopped.
    fireEvent.click(column.getByRole("button", { name: "Hide details" }));
    expect(column.getByRole("status")).toHaveTextContent("Explaining Handler.Create in detail");
    expect(column.getByRole("button", { name: "Cancel" })).toBeEnabled();
  });

  it("opens with the detailed explanation hidden again after closing, and for another function", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained(
        [makeExplanation(id.create), makeExplanation(id.validate)],
        [makeBrief(id.create), makeBrief(id.validate, { text: "Checks the amount." })],
      ),
    );
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained:")),
    );
    const aside = (dialog: HTMLElement) => within(dialog.querySelector<HTMLElement>("[data-column=aside]")!);
    const openOn = async (name: string) => {
      fireEvent.click(await box(name));
      const dialog = screen.getByRole("dialog", { name: new RegExp(escape(name)) });
      await waitFor(() => expect(dialog).toHaveAttribute("data-columns", "2"));
      return dialog;
    };

    let dialog = await openOn("Handler.Create");
    fireEvent.click(aside(dialog).getByRole("button", { name: "Show details" }));
    expect(aside(dialog).getByRole("region", { name: "In detail" })).toBeInTheDocument();

    // Another function: its detail is hidden.
    dialog = await openOn("Validate");
    expect(aside(dialog).getByText("Checks the amount.")).toBeInTheDocument();
    expect(aside(dialog).queryByRole("region", { name: "In detail" })).toBeNull();
    expect(aside(dialog).getByRole("button", { name: "Show details" })).toBeInTheDocument();

    // Back to the first one: hidden too.
    fireEvent.click(aside(dialog).getByRole("button", { name: "Show details" }));
    dialog = await openOn("Handler.Create");
    expect(aside(dialog).queryByRole("region", { name: "In detail" })).toBeNull();

    // Closed and opened again: hidden.
    fireEvent.click(aside(dialog).getByRole("button", { name: "Show details" }));
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    expect(screen.queryByRole("dialog")).toBeNull();
    dialog = await openOn("Handler.Create");
    expect(aside(dialog).queryByRole("region", { name: "In detail" })).toBeNull();
    expect(aside(dialog).getByRole("button", { name: "Show details" })).toBeInTheDocument();
    expect(explainFunctions).not.toHaveBeenCalled();
  });

  it("offers to translate explanations written in another language, only on click", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained(
        [
          makeExplanation(id.create, { language: "pt-BR", whatChanged: "Adiciona o handler." }),
          makeExplanation(id.validate, {
            language: "pt-BR",
            translations: [{ language: "en", whatChanged: "Checks the amount.", why: "Asked.", purpose: "Validates." }],
          }),
        ],
        // A short one translated too: both levels of Validate read in English.
        [
          makeBrief(id.validate, {
            language: "pt-BR",
            text: "Confere o valor.",
            translations: [{ language: "en", text: "Checks the amount." }],
          }),
        ],
      ),
    );
    vi.mocked(planExplanationsTranslation).mockResolvedValue({
      items: 1,
      calls: [{ ids: [id.create], tokens: 300 }],
      tokens: 300,
      model: makeModelInUse(),
    });
    vi.mocked(translateExplanations).mockResolvedValue(explained([]));
    const user = await openFunctions();

    // The translated one reads in English; only the other is named.
    await waitFor(async () =>
      expect(await box("Validate")).toHaveAccessibleName(expect.stringContaining("explained: Checks the amount.")),
    );
    const banner = within(await screen.findByRole("region", { name: "Language of this text" }));
    expect(banner.getByText("An explanation of a function was written in Portuguese.")).toBeInTheDocument();
    expect(planExplanationsTranslation).not.toHaveBeenCalled();

    await user.click(banner.getByRole("button", { name: "Translate" }));
    expect(planExplanationsTranslation).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "en");
    expect(
      await banner.findByText(
        "Translate sends the text of 1 explanation to Claude Code, model opus, in 1 call, about 300 tokens in. The code is not sent again.",
      ),
    ).toBeInTheDocument();
    expect(translateExplanations).not.toHaveBeenCalled();
    await user.click(banner.getByRole("button", { name: "Translate" }));
    expect(translateExplanations).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      REVIEW,
      "en",
      [id.create],
      expect.any(Array),
      expect.any(String),
    );
  });

  it("offers to translate a function whose short one alone is in another language", async () => {
    vi.mocked(getExplanations).mockResolvedValue(
      explained([makeExplanation(id.create)], [makeBrief(id.create, { language: "pt-BR", text: "Novo handler." })]),
    );
    await openFunctions();
    await waitFor(async () =>
      expect(await box("Handler.Create")).toHaveAccessibleName(expect.stringContaining("explained: Novo handler.")),
    );
    const banner = within(await screen.findByRole("region", { name: "Language of this text" }));
    expect(banner.getByText("An explanation of a function was written in Portuguese.")).toBeInTheDocument();
    expect(planExplanationsTranslation).not.toHaveBeenCalled();
  });

  it("switching the language asks no model", async () => {
    vi.mocked(getExplanations).mockResolvedValue(explained([makeExplanation(id.create)]));
    await openFunctions();
    await screen.findByRole("button", { name: `Explain Handler.Create with ${MODEL}: one model call` });
    act(() => setLanguage("pt-BR"));
    expect(await screen.findByText("A explicação de uma função foi escrita em inglês.")).toBeInTheDocument();
    expect(explainFunctions).not.toHaveBeenCalled();
    expect(translateExplanations).not.toHaveBeenCalled();
  });
});
