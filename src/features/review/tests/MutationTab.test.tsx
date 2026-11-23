import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { PROGRESS_POLL_MS, RUN_POLL_MS } from "@/features/review/tests/use-tests";
import { i18n } from "@/i18n";
import {
  cancelMutation,
  getFileVersions,
  getMutationProgress,
  getMutationState,
  getReviewState,
  getTestsProgress,
  getTestsState,
  listChangedFiles,
  type MutationRun,
  type ReviewTarget,
  runMutation,
} from "@/lib/api";
import { deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab } from "@/test/review-screen";
import { makeMutationRun, makeMutationState, makeTestRun, makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getTestsProgress: vi.fn(),
  runTests: vi.fn(),
  cancelTests: vi.fn(),
  getMutationState: vi.fn(),
  getMutationProgress: vi.fn(),
  runMutation: vi.fn(),
  cancelMutation: vi.fn(),
  getQuality: vi.fn(),
  getReviewState: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getInsights: vi.fn(),
}));

// CodeMirror needs layout APIs jsdom does not have; the stub draws the blocks under the lines, as it was given them.
vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div data-testid="diff-editor" data-path={props.path} data-scroll-line={props.scrollTo?.line ?? ""}>
      {(props.annotations ?? []).map((annotation) => (
        <div key={annotation.key}>{props.renderAnnotation?.(annotation.key)}</div>
      ))}
    </div>
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const OLD = "a1b2c3d4e5f6a7b8";
const NEW = "e7f3b10c9d8e7f6a";

const changeSet = makeChangeSet([
  makeChangedFile("internal/refund/handler.go", { status: "added" }),
  makeChangedFile("internal/refund/service.go", { status: "added" }),
  makeChangedFile("internal/refund/service_test.go", { status: "added" }),
]);

let bar: HTMLDivElement;

const section = () => within(screen.getByRole("region", { name: "Mutation testing" }));
const survivor = (place: string) => within(screen.getByRole("article", { name: `Mutant at ${place}` }));

function fakeClock() {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
}

/** Back to the real clock, letting jsdom's own animation frame interval started meanwhile run out first. */
function realClock() {
  if (vi.isFakeTimers()) {
    act(() => {
      vi.advanceTimersByTime(100);
    });
  }
  vi.useRealTimers();
}

async function openTests(user = userEvent.setup()) {
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
  await openReviewTab(user, /^Tests/);
  await screen.findByRole("heading", { name: "Mutation testing" });
  return user;
}

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a\n"), new: textSide("b\n") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeTestRun() }));
  vi.mocked(getTestsProgress).mockResolvedValue(null);
  vi.mocked(getMutationProgress).mockResolvedValue(null);
  vi.mocked(cancelMutation).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
  realClock();
});

describe("Mutation testing before a run", () => {
  it("only reads the last run, says what running does, and runs nothing until clicked", async () => {
    await openTests();

    expect(getMutationState).toHaveBeenCalledWith("p-1", OLD, NEW);
    expect(runMutation).not.toHaveBeenCalled();
    expect(section().getByText(/A change no test notices is code the tests run without verifying it\./)).toBeInTheDocument();
    expect(section().getByText(/Up to 200 mutants, sampled across the changed functions\./)).toBeInTheDocument();
    expect(section().getByRole("button", { name: "Run mutation testing" })).toBeEnabled();
  });

  it("needs a test run with coverage first, and says how to get one", async () => {
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ unavailable: { reason: "needs_coverage" } }));
    await openTests();

    const button = await section().findByRole("button", { name: "Run mutation testing" });
    expect(button).toBeDisabled();
    expect(button).toHaveAccessibleDescription(
      "Mutation testing needs a test run with coverage first: it only tries the changed lines the tests run. " +
        "Run the tests with “Measure coverage” on.",
    );
  });

  it("is not there when no analyzer of the project's languages runs tests", async () => {
    vi.mocked(getMutationState).mockResolvedValue(
      makeMutationState({ unavailable: { reason: "no_runner", languages: ["go"] } }),
    );
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await openReviewTab(userEvent.setup(), /^Tests/);
    await screen.findByRole("heading", { name: "1 test failed" });
    expect(screen.queryByRole("region", { name: "Mutation testing" })).not.toBeInTheDocument();
  });

  it("is written in the language shown", async () => {
    await openTests();
    await act(async () => {
      await i18n.changeLanguage("pt-BR");
    });
    const region = within(await screen.findByRole("region", { name: "Teste de mutação" }));
    expect(region.getByRole("button", { name: "Rodar teste de mutação" })).toBeInTheDocument();
    expect(region.getByText(/Uma alteração que nenhum teste percebe é código que os testes executam sem verificar\./))
      .toBeInTheDocument();
  });
});

describe("Mutation testing while it runs", () => {
  it("shows the phase, the mutants done and what became of them, and cancels", async () => {
    fakeClock();
    const pending = deferred<MutationRun>();
    vi.mocked(runMutation).mockReturnValue(pending.promise);
    const user = await openTests(userEvent.setup({ advanceTimers: vi.advanceTimersByTime }));

    await user.click(section().getByRole("button", { name: "Run mutation testing" }));
    expect(runMutation).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW);
    const status = section().getByRole("status", { name: "Running mutation testing" });
    expect(status).toHaveTextContent("Preparing the run");
    expect(status).toHaveTextContent("0:00");

    vi.mocked(getMutationProgress).mockResolvedValue({
      phase: "baseline",
      total: 0,
      done: 0,
      killed: 0,
      survived: 0,
      timeout: 0,
      error: 0,
      current: null,
    });
    await act(async () => {
      vi.advanceTimersByTime(PROGRESS_POLL_MS);
    });
    expect(getMutationProgress).toHaveBeenCalledWith("p-1", NEW);
    expect(status).toHaveTextContent("Running the tests on the unchanged code first");

    vi.mocked(getMutationProgress).mockResolvedValue({
      phase: "mutating",
      total: 12,
      done: 5,
      killed: 3,
      survived: 1,
      timeout: 1,
      error: 0,
      current: "internal/refund/service.go:58",
    });
    await act(async () => {
      vi.advanceTimersByTime(PROGRESS_POLL_MS);
    });
    const progress = within(status).getByRole("progressbar", { name: "Mutants tested" });
    expect(progress).toHaveAttribute("aria-valuenow", "5");
    expect(progress).toHaveAttribute("aria-valuemax", "12");
    expect(status).toHaveTextContent("5 of 12 mutants");
    expect(status).toHaveTextContent("3 killed");
    expect(status).toHaveTextContent("1 survived");
    expect(status).toHaveTextContent("1 timeout");
    expect(status).toHaveTextContent("now in internal/refund/service.go:58");

    await user.click(within(status).getByRole("button", { name: "Cancel" }));
    expect(cancelMutation).toHaveBeenCalledExactlyOnceWith("p-1", NEW);
    expect(section().getByRole("status", { name: "Cancelling mutation testing" })).toBeInTheDocument();

    realClock();
    await act(async () => {
      pending.resolve(makeMutationRun({ status: "cancelled", durationMs: 12_400, mutants: [] }));
    });
    expect(await section().findByText("The last mutation run was cancelled after 12 s.")).toBeInTheDocument();
    expect(section().getByRole("button", { name: "Run mutation testing" })).toBeEnabled();
  });

  it("goes on while another tab is shown, and its survivors are in the Diff once it ends", async () => {
    const pending = deferred<MutationRun>();
    vi.mocked(runMutation).mockReturnValue(pending.promise);
    const user = await openTests();

    await user.click(section().getByRole("button", { name: "Run mutation testing" }));
    await openReviewTab(user, /^Diff/);
    await openReviewTab(user, /^Tests/);
    expect(section().getByRole("status", { name: "Running mutation testing" })).toBeInTheDocument();
    expect(runMutation).toHaveBeenCalledOnce();

    await act(async () => {
      pending.resolve(makeMutationRun());
    });
    expect(await section().findByRole("article", { name: "Mutant at internal/refund/service.go:52" })).toBeInTheDocument();

    await openReviewTab(user, /^Diff/);
    await user.click(screen.getByRole("option", { name: /service\.go/ }));
    const note = await screen.findByRole("group", { name: "Mutant no test caught, line 52" });
    expect(note).toHaveTextContent("if balance >= amount { became if balance > amount { and no test failed");
  });

  it("follows a run started elsewhere until it ends, then shows its result", async () => {
    fakeClock();
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ running: true }));
    await openTests(userEvent.setup({ advanceTimers: vi.advanceTimersByTime }));
    expect(section().getByRole("status", { name: "Running mutation testing" })).toBeInTheDocument();

    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ run: makeMutationRun() }));
    await act(async () => {
      vi.advanceTimersByTime(RUN_POLL_MS);
    });
    expect(section().getByRole("article", { name: "Mutant at internal/refund/service.go:52" })).toBeInTheDocument();
    expect(runMutation).not.toHaveBeenCalled();
  });
});

describe("Mutation testing result", () => {
  it("gives the score with timeouts apart, what else the run says, and the sampled note", async () => {
    const run = makeMutationRun();
    vi.mocked(getMutationState).mockResolvedValue(
      makeMutationState({
        run: { ...run, summary: { ...run.summary, sampled: true, candidates: 340, cached: 2 } },
      }),
    );
    await openTests();

    const region = section();
    expect(await region.findByText(/1 of 4 killed · 1 timeout/)).toHaveTextContent("25% 1 of 4 killed · 1 timeout");
    expect(region.getByText("1 could not be tested")).toBeInTheDocument();
    expect(region.getByText("2 reused from earlier runs")).toBeInTheDocument();
    expect(region.getByText("by difflens-go mutator 0.1.0")).toBeInTheDocument();
    expect(
      region.getByText("Only 200 of 340 candidates were tested (sampled evenly across the changed functions)."),
    ).toBeInTheDocument();
    expect(region.getByRole("button", { name: "Run again" })).toBeEnabled();
  });

  it("lists the survivors and timeouts with their line as it is and as mutated, and opens them in the Diff", async () => {
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ run: makeMutationRun() }));
    const user = await openTests();

    const boundary = survivor("internal/refund/service.go:52");
    expect(boundary.getByText("survived")).toBeInTheDocument();
    expect(boundary.getByText("Comparison boundary")).toBeInTheDocument();
    expect(boundary.getByText("refund.Service.Request")).toBeInTheDocument();
    const code = boundary.getByLabelText("internal/refund/service.go:52, as it is and as mutated");
    expect([...code.querySelectorAll("[data-sign]")].map((row) => row.textContent)).toEqual([
      "- 52  \tif balance >= amount {",
      "+ 52  \tif balance > amount {",
    ]);
    expect([...code.querySelectorAll("mark")].map((mark) => mark.textContent)).toEqual([">=", ">"]);
    expect(survivor("internal/refund/service.go:58").getByText("Call removed")).toBeInTheDocument();
    const timeout = survivor("internal/refund/service.go:64");
    expect(timeout.getByText("timeout")).toBeInTheDocument();
    expect(timeout.queryByRole("button", { name: "Copy as a prompt for the agent" })).not.toBeInTheDocument();

    await user.click(boundary.getByRole("button", { name: "internal/refund/service.go:52" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "52");
  });

  it("folds the killed ones, saying which test killed them, and those that could not be tested", async () => {
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ run: makeMutationRun() }));
    await openTests();

    const killed = section().getByText("Killed").closest("details")!;
    expect(killed).not.toHaveAttribute("open");
    expect(within(killed).getByRole("listitem")).toHaveTextContent("internal/refund/service.go:66");
    expect(within(killed).getByRole("listitem")).toHaveTextContent("by TestRequest");
    const untested = section().getByText("Not tested").closest("details")!;
    expect(untested).not.toHaveAttribute("open");
    expect(within(untested).getByRole("listitem")).toHaveTextContent("err → nil");
    expect(within(untested).getByRole("listitem")).toHaveTextContent("error");
  });

  it("copies a survivor as a prompt for the agent", async () => {
    vi.mocked(getMutationState).mockResolvedValue(makeMutationState({ run: makeMutationRun() }));
    const user = await openTests();

    await user.click(survivor("internal/refund/service.go:52").getByRole("button", { name: "Copy as a prompt for the agent" }));
    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    const prompt = await navigator.clipboard.readText();
    expect(prompt).toContain("Write the missing test for internal/refund/service.go:52, in refund.Service.Request.");
    expect(prompt).toContain("```diff\n- \tif balance >= amount {\n+ \tif balance > amount {\n```");
    expect(prompt).toContain("Tests that catch other changes in this function: TestRequest.");
  });
});
