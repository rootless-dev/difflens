import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  cancelModelRun,
  getFileVersions,
  getInsights,
  getReviewState,
  getTestsState,
  type Insights,
  linkPullRequest,
  listChangedFiles,
  planInsights,
  type ReviewTarget,
  runInsightsBatch,
  setFindingFixed,
  setFindingVerdict,
  startInsights,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import {
  makeAnalysis,
  makeBatch,
  makeFinding,
  makeInsights,
  makeModelInUse,
  makePlan,
  NO_INSIGHTS,
  prototypeFindings,
} from "@/test/insights-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  linkPullRequest: vi.fn(),
  getInsights: vi.fn(),
  planInsights: vi.fn(),
  startInsights: vi.fn(),
  runInsightsBatch: vi.fn(),
  setFindingVerdict: vi.fn(),
  setFindingFixed: vi.fn(),
  cancelModelRun: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
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

const changeSet = makeChangeSet([
  makeChangedFile("internal/payment/repo.go"),
  makeChangedFile("internal/refund/handler.go", { status: "added" }),
  makeChangedFile("internal/refund/service.go", { status: "added" }),
]);

const REVIEW = { target: BRANCH, oldSha: "a1b2c3d4e5f6a7b8", newSha: "e7f3b10c9d8e7f6a" };

let bar: HTMLDivElement;
const onOpenPreferences = vi.fn();

const tabs = () => within(screen.getByRole("tablist", { name: "Review views" }));
const tab = (name: RegExp) => tabs().getByRole("tab", { name });
const rows = () => within(screen.getByRole("list", { name: "Findings" })).getAllByRole("button");
const detail = () => screen.getByRole("region", { name: screen.getAllByRole("heading", { level: 1 })[0].textContent! });

function renderScreen() {
  return render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} onOpenPreferences={onOpenPreferences} />);
}

async function openInsights(user = userEvent.setup()) {
  renderScreen();
  await openReviewTab(user, /^Insights/);
  return user;
}

/** An analysis just started: its calls are all pending. */
function started(batches: number, parallelCalls = 1): Insights {
  return {
    analysis: makeAnalysis({
      parallelCalls,
      batches: Array.from({ length: batches }, (_, index) => makeBatch(index, { status: "pending" })),
    }),
    findings: [],
  };
}

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
  vi.mocked(getInsights).mockResolvedValue(NO_INSIGHTS);
  vi.mocked(planInsights).mockResolvedValue(makePlan());
  vi.mocked(cancelModelRun).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("Insights tab", () => {
  it("reads the findings with the review for the tab's count and the progress line, and asks the model nothing", async () => {
    vi.mocked(getInsights).mockResolvedValue(
      makeInsights({ findings: makeInsights().findings.map((f, i) => (i < 3 ? { ...f, verdict: "confirmed" } : f)) }),
    );
    const user = userEvent.setup();
    renderScreen();

    await waitFor(() => expect(tab(/^Insights/)).toHaveAccessibleName("Insights, 5 findings"));
    expect(tab(/^Insights/)).toHaveTextContent(/^Insights5$/);
    expect(screen.getByText(/findings classified/).parentElement).toHaveTextContent(
      "Reviewed 0 of 3 files3 of 5 findings classified",
    );
    expect(getInsights).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);
    expect(planInsights).not.toHaveBeenCalled();

    await user.click(tab(/^Insights/));
    expect(await screen.findByRole("list", { name: "Findings" })).toBeInTheDocument();
    // Opening the tab works out what Analyze would send, nothing more.
    expect(planInsights).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);

    await user.click(within(bar).getByRole("button", { name: "Reload" }));
    await waitFor(() => expect(getInsights).toHaveBeenCalledTimes(2));
    expect(startInsights).not.toHaveBeenCalled();
    expect(runInsightsBatch).not.toHaveBeenCalled();
  });

  it("explains what Analyze does and what it would send before anything is sent", async () => {
    await openInsights();

    expect(await screen.findByRole("heading", { name: "Ask the model to read this change" })).toBeInTheDocument();
    expect(screen.getByText(/nothing is sent until you click Analyze/)).toBeInTheDocument();
    expect(await screen.findByLabelText("What Analyze sends")).toHaveTextContent(
      "5 files in 1 call, about 48k tokens in, read by Claude Code, model opus.",
    );
    expect(screen.getByText("2 files are listed to the model but not sent: 1 binary and 1 deleted.")).toBeInTheDocument();
    expect(screen.getByText(/The code of the changed files is sent to Claude Code/)).toBeInTheDocument();
    expect(startInsights).not.toHaveBeenCalled();
  });

  it("says when no model is set up and links to Preferences instead of offering Analyze", async () => {
    vi.mocked(planInsights).mockResolvedValue(
      makePlan({
        model: makeModelInUse({
          provider: "openai",
          providerName: "OpenAI API",
          model: null,
          ready: false,
          problem: "Choose a model for the OpenAI API in Preferences › Model.",
        }),
      }),
    );
    const user = await openInsights();

    expect(
      await screen.findByText("No model is set up to read it. Choose a model for the OpenAI API in Preferences › Model."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Analyze" })).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open Preferences" }));
    expect(onOpenPreferences).toHaveBeenCalledOnce();
  });

  it("asks Claude Code's calls one at a time, showing which call is being asked", async () => {
    vi.mocked(startInsights).mockResolvedValue(started(2));
    const first = deferred<Insights>();
    const second = deferred<Insights>();
    vi.mocked(runInsightsBatch).mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    vi.mocked(getInsights).mockResolvedValueOnce(NO_INSIGHTS).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: "Analyze" }));

    expect(startInsights).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);
    expect(await screen.findByRole("status")).toHaveTextContent("Call 1 of 2");
    expect(runInsightsBatch).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "an-1", 0, expect.any(String));
    expect(screen.getByText("Asking Claude Code, model claude-opus-5-5. Findings appear as each call answers.")).toBeInTheDocument();

    first.resolve({
      analysis: makeAnalysis({ batches: [makeBatch(0), makeBatch(1, { status: "pending" })] }),
      findings: [makeFinding("f1")],
    });
    await waitFor(() => expect(screen.getByRole("status")).toHaveTextContent("Call 2 of 2"));
    expect(runInsightsBatch).toHaveBeenLastCalledWith("p-1", REVIEW, "an-1", 1, expect.any(String));
    // What the first call found shows while the second is asked.
    expect(rows()).toHaveLength(1);

    second.resolve(makeInsights({ analysis: makeAnalysis({ batches: [makeBatch(0), makeBatch(1)] }) }));
    await waitFor(() => expect(rows()).toHaveLength(5));
    expect(screen.queryByRole("status")).not.toBeInTheDocument();
    expect(screen.getByText(/Read by Claude Code, model claude-opus-5-5 at e7f3b10/)).toBeInTheDocument();
  });

  it("asks up to three calls of an API at once, and Cancel stops them and starts no other", async () => {
    const anthropic = makeModelInUse({ provider: "anthropic", providerName: "Anthropic API", parallelCalls: 3 });
    vi.mocked(planInsights).mockResolvedValue(makePlan({ model: anthropic }));
    vi.mocked(startInsights).mockResolvedValue(started(4, 3));
    const calls = [deferred<Insights>(), deferred<Insights>(), deferred<Insights>()];
    calls.forEach((call) => vi.mocked(runInsightsBatch).mockReturnValueOnce(call.promise));
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: "Analyze" }));
    await waitFor(() => expect(runInsightsBatch).toHaveBeenCalledTimes(3));
    expect(screen.getByRole("status")).toHaveTextContent("Calls 1, 2 and 3 of 4");
    const runIds = vi.mocked(runInsightsBatch).mock.calls.map((call) => call[4]);
    expect(new Set(runIds).size).toBe(3);

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(screen.getByRole("button", { name: "Stopping" })).toBeDisabled();
    expect(vi.mocked(cancelModelRun).mock.calls.map((call) => call[0]).sort()).toEqual([...runIds].sort());

    const stopped = makeAnalysis({
      parallelCalls: 3,
      batches: [0, 1, 2].map((index) =>
        makeBatch(index, { status: "failed", error: apiError("cancelled", "Stopped.") }),
      ).concat(makeBatch(3, { status: "pending" })),
    });
    vi.mocked(getInsights).mockResolvedValue({ analysis: stopped, findings: [] });
    calls.forEach((call) => call.resolve({ analysis: stopped, findings: [] }));

    const failed = await screen.findByRole("list", { name: "Calls that did not answer" });
    // The fourth call was never asked.
    expect(runInsightsBatch).toHaveBeenCalledTimes(3);
    expect(within(failed).getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "Call 1 of 4 failed. Stopped.file-0.goRetry",
      "Call 2 of 4 failed. Stopped.file-1.goRetry",
      "Call 3 of 4 failed. Stopped.file-2.goRetry",
      "Call 4 of 4 did not run. file-3.goRetry",
    ]);
  });

  it("keeps what answered when a call fails, with its error and a Retry for that call only", async () => {
    const limited = apiError("rate_limited", "The Anthropic API is limiting requests; try again in 20 seconds.");
    const answered = makeInsights({ analysis: makeAnalysis({ batches: [makeBatch(0), makeBatch(1)] }) });
    vi.mocked(getInsights)
      .mockResolvedValueOnce(
        makeInsights({
          analysis: makeAnalysis({ batches: [makeBatch(0), makeBatch(1, { status: "failed", error: limited })] }),
        }),
      )
      .mockResolvedValue(answered);
    vi.mocked(runInsightsBatch).mockResolvedValue(answered);
    const user = await openInsights();

    const failed = await screen.findByRole("list", { name: "Calls that did not answer" });
    expect(failed).toHaveTextContent("Call 2 of 2 failed. The Anthropic API is limiting requests; try again in 20 seconds.");
    expect(rows()).toHaveLength(5);

    await user.click(within(failed).getByRole("button", { name: "Retry" }));
    expect(runInsightsBatch).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "an-1", 1, expect.any(String));
    await waitFor(() => expect(screen.queryByRole("list", { name: "Calls that did not answer" })).not.toBeInTheDocument());
    expect(startInsights).not.toHaveBeenCalled();
  });

  it("shows the cost shape again before Analyze again sends anything", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    vi.mocked(startInsights).mockResolvedValue(started(1));
    vi.mocked(runInsightsBatch).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: "Analyze again" }));
    expect(startInsights).not.toHaveBeenCalled();
    expect(screen.getByLabelText("What Analyze sends")).toHaveTextContent("5 files in 1 call, about 48k tokens in");
    expect(screen.getByText(/Nothing is deleted, and dismissed findings are not sent again/)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Analyze" }));
    expect(startInsights).toHaveBeenCalledOnce();
    await waitFor(() => expect(runInsightsBatch).toHaveBeenCalledOnce());
  });

  it("lists the findings by severity with a filter, and shows the selected one in full", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await screen.findByRole("list", { name: "Findings" });
    const filter = screen.getByRole("group", { name: "Severity" });
    expect(within(filter).getAllByRole("button").map((button) => button.textContent)).toEqual([
      "All 5",
      "High 2",
      "Medium 2",
      "Low 1",
    ]);
    expect(screen.getByText("Read by the model, not measured. Check before trusting.")).toBeInTheDocument();
    expect(rows()[0]).toHaveAccessibleName(
      "Refund can be paid out and never recorded, High, Correctness, internal/refund/service.go:52, Pending",
    );
    expect(rows()[0]).toHaveAttribute("aria-pressed", "true");

    const shown = detail();
    expect(within(shown).getByText("internal/refund/service.go:52–56")).toBeInTheDocument();
    const code = within(shown).getByLabelText("Code at internal/refund/service.go:52–56");
    const cited = [...code.querySelectorAll("[data-cited]")].map((line) => line.textContent);
    expect(cited[0]).toBe("52  \tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)");
    expect(cited).toHaveLength(5);
    expect(code.firstElementChild).toHaveTextContent("50");
    expect(within(shown).getByRole("region", { name: "Suggested direction" })).toHaveTextContent(
      "Save the refund as pending before calling the gateway",
    );
    expect(within(shown).getByRole("region", { name: /What shows it/ })).toHaveTextContent(
      "What shows itread from the code, not reproducedLine 52 asks the gateway to refund",
    );
    expect(within(shown).getByRole("region", { name: "Related, but not proof" })).toHaveTextContent(
      "No test makes Save fail",
    );
    expect(within(shown).getByText(/^By Claude Code, model claude-opus-5-5, /)).toBeInTheDocument();

    await user.click(within(filter).getByRole("button", { name: "Medium 2" }));
    expect(rows().map((row) => row.querySelector("strong")?.textContent)).toEqual([
      "Two concurrent refunds can exceed the balance",
      "Partial refund over the limit has no test",
    ]);
    expect(screen.getAllByRole("heading", { level: 1 })[0]).toHaveTextContent("Two concurrent refunds can exceed the balance");
  });

  it("does not show a finding it cannot place as if it were fine", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: /^Partial refund over the limit has no test/ }));
    const shown = detail();
    expect(
      within(shown).getByText(
        "Location not found: internal/refund/validate.go is not a file this review changes, so there is no code to show.",
      ),
    ).toBeInTheDocument();
    expect(within(shown).queryByLabelText(/^Code at/)).not.toBeInTheDocument();
    expect(within(shown).getByRole("button", { name: "Open in Diff" })).toBeDisabled();
  });

  it("opens a finding in the Diff tab at its line", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: /^Retrying POST \/refunds refunds twice/ }));
    await user.click(within(detail()).getByRole("button", { name: "Open in Diff" }));

    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    await waitFor(() => expect(editor.dataset.path).toBe("internal/refund/handler.go"));
    expect(screen.getByTestId("diff-editor").dataset.scrollLine).toBe("31");
    expect(screen.getByTestId("diff-editor").dataset.scrollSide).toBe("new");
  });

  it("records a verdict and says when the code under one changed", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    const confirmed = makeInsights();
    confirmed.findings[0] = { ...confirmed.findings[0], verdict: "confirmed", verdictAt: "2026-11-19T11:00:00Z" };
    vi.mocked(setFindingVerdict).mockResolvedValue(confirmed);
    const user = await openInsights();

    const verdict = within(await screen.findByRole("group", { name: "Verdict" }));
    expect(verdict.getAllByRole("button").map((button) => button.textContent)).toEqual([
      "Pending",
      "Confirmed",
      "Dismissed",
    ]);
    expect(verdict.getByRole("button", { name: "Pending" })).toHaveAttribute("aria-pressed", "true");
    await user.click(verdict.getByRole("button", { name: "Confirmed" }));

    expect(setFindingVerdict).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "f1", "confirmed");
    await waitFor(() => expect(verdict.getByRole("button", { name: "Confirmed" })).toHaveAttribute("aria-pressed", "true"));
    expect(rows()[0]).toHaveTextContent(/Confirmed$/);
    expect(screen.getByText(/findings classified/).parentElement).toHaveTextContent("1 of 5 findings classified");
  });

  it("asks to check a classified finding again once the code around it changed", async () => {
    vi.mocked(getInsights).mockResolvedValue(
      makeInsights({
        findings: [
          makeFinding("f1", { verdict: "confirmed", verdictAt: "2026-11-12T09:00:00Z", recheck: true }),
          makeFinding("f2", {
            title: "Old one",
            verdict: "dismissed",
            verdictAt: "2026-11-12T09:00:00Z",
            source: { ...makeFinding("x").source, earlier: true, analyzedAt: "2026-11-12T08:00:00Z" },
          }),
        ],
      }),
    );
    const user = await openInsights();

    expect(
      await screen.findByText("You confirmed this on 12 Nov. The code around it changed since then, so check it again."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: /^Old one/ }));
    expect(screen.getByText("From the earlier analysis of 12 Nov, by Claude Code, model claude-opus-5-5.")).toBeInTheDocument();
    expect(rows()[1].querySelector("strong")).toHaveClass("line-through");
  });

  describe("fixed findings", () => {
    const JUST_NOW = () => new Date().toISOString();

    function withFixed(): Insights {
      const [open, byModel, byYou, left] = prototypeFindings();
      return makeInsights({
        findings: [
          open,
          {
            ...byModel,
            source: { ...byModel.source, earlier: true },
            resolution: {
              kind: "model_checked",
              model: "claude-opus-5-5-20261101",
              at: JUST_NOW(),
              reason: "The refund is now saved as pending before the gateway call.",
            },
          },
          { ...byYou, verdict: "confirmed", resolution: { kind: "user_marked", model: null, at: JUST_NOW(), reason: null } },
          { ...left, resolution: { kind: "file_left_change", model: null, at: JUST_NOW(), reason: null } },
        ],
      });
    }

    const fixedGroup = () => screen.getByRole("button", { name: /^Fixed \(/ });
    const fixedRows = () => within(screen.getByRole("list", { name: "Fixed findings" })).getAllByRole("button");

    it("are kept in a Fixed group folded at first, out of the counts, each saying who found it fixed", async () => {
      vi.mocked(getInsights).mockResolvedValue(withFixed());
      const user = userEvent.setup();
      renderScreen();

      await waitFor(() => expect(tab(/^Insights/)).toHaveAccessibleName("Insights, 1 finding"));
      expect(screen.getByText(/findings? classified/).parentElement).toHaveTextContent("0 of 1 finding classified");
      await user.click(tab(/^Insights/));

      await screen.findByRole("list", { name: "Findings" });
      expect(rows()).toHaveLength(1);
      expect(screen.getByRole("button", { name: "All 1" })).toBeInTheDocument();
      expect(fixedGroup()).toHaveTextContent("Fixed (3)");
      expect(fixedGroup()).toHaveAttribute("aria-expanded", "false");
      expect(screen.queryByRole("list", { name: "Fixed findings" })).not.toBeInTheDocument();

      await user.click(fixedGroup());
      expect(fixedGroup()).toHaveAttribute("aria-expanded", "true");
      expect(fixedRows().map((row) => row.textContent)).toEqual([
        expect.stringMatching(/Checked by claude-opus-5-5-20261101 just nowFixed$/),
        expect.stringMatching(/Marked by you just nowFixed$/),
        expect.stringMatching(/Fixed just now: the file is no longer in the changeFixed$/),
      ]);
      expect(fixedRows()[1]).toHaveAccessibleName(
        "Two concurrent refunds can exceed the balance, Medium, Concurrency, internal/payment/repo.go:27, Fixed, " +
          "Marked by you just now",
      );

      await user.click(fixedRows()[0]);
      const shown = detail();
      expect(within(shown).getByText("Checked by claude-opus-5-5-20261101 just now")).toBeInTheDocument();
      expect(
        within(shown).getByText("The refund is now saved as pending before the gateway call."),
      ).toBeInTheDocument();
      // Its verdict stays as it was.
      expect(within(shown).getByRole("button", { name: "Pending" })).toHaveAttribute("aria-pressed", "true");
      expect(within(shown).queryByRole("button", { name: "Mark as fixed" })).not.toBeInTheDocument();
    });

    it("reopens a fixed finding, asking no model", async () => {
      vi.mocked(getInsights).mockResolvedValue(withFixed());
      const reopened = withFixed();
      reopened.findings[2] = { ...reopened.findings[2], resolution: null };
      vi.mocked(setFindingFixed).mockResolvedValue(reopened);
      const user = await openInsights();

      await user.click(await screen.findByRole("button", { name: "Fixed (3)" }));
      await user.click(fixedRows()[1]);
      await user.click(within(detail()).getByRole("button", { name: "Reopen" }));

      expect(setFindingFixed).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "f3", false);
      await waitFor(() => expect(rows()).toHaveLength(2));
      expect(fixedGroup()).toHaveTextContent("Fixed (2)");
      // Still the one shown, now open again.
      expect(within(detail()).getByRole("button", { name: "Mark as fixed" })).toBeInTheDocument();
      expect(startInsights).not.toHaveBeenCalled();
      expect(runInsightsBatch).not.toHaveBeenCalled();
    });

    it("marks an open finding fixed by hand, but offers it on no dismissed one", async () => {
      const findings = prototypeFindings().slice(0, 2);
      findings[1] = { ...findings[1], verdict: "dismissed" };
      vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings }));
      const marked = makeInsights({
        findings: [
          { ...findings[0], resolution: { kind: "user_marked", model: null, at: JUST_NOW(), reason: null } },
          findings[1],
        ],
      });
      vi.mocked(setFindingFixed).mockResolvedValue(marked);
      const user = await openInsights();

      await screen.findByRole("list", { name: "Findings" });
      expect(screen.queryByRole("button", { name: /^Fixed \(/ })).not.toBeInTheDocument();
      await user.click(within(detail()).getByRole("button", { name: "Mark as fixed" }));

      expect(setFindingFixed).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "f1", true);
      await waitFor(() => expect(fixedGroup()).toHaveTextContent("Fixed (1)"));
      expect(rows()).toHaveLength(1);
      // The one just marked stays shown, with the way back.
      expect(within(detail()).getByText("Marked by you just now")).toBeInTheDocument();
      expect(within(detail()).getByRole("button", { name: "Reopen" })).toBeInTheDocument();

      await user.click(rows()[0]);
      expect(within(detail()).queryByRole("button", { name: "Mark as fixed" })).not.toBeInTheDocument();
    });

    it("says every finding is fixed when none is left open", async () => {
      const [only] = prototypeFindings();
      vi.mocked(getInsights).mockResolvedValue(
        makeInsights({
          findings: [{ ...only, resolution: { kind: "user_marked", model: null, at: JUST_NOW(), reason: null } }],
        }),
      );
      await openInsights();

      expect(await screen.findByText("Every finding is fixed.")).toBeInTheDocument();
      expect(screen.getByText("0 findings")).toBeInTheDocument();
      expect(fixedGroup()).toHaveTextContent("Fixed (1)");
    });
  });

  it("copies a finding as a self-contained prompt for the agent", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights());
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: "Copy as a prompt for the agent" }));

    expect(await screen.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    const prompt = await navigator.clipboard.readText();
    expect(prompt).toContain("Fix a problem a code review found in internal/refund/service.go:52–56.");
    expect(prompt).toContain("Problem: Refund can be paid out and never recorded\nThe gateway refund runs first");
    expect(prompt).toContain("- Line 52 asks the gateway to refund");
    expect(prompt).toContain("52  \tref, err := s.gateway.Refund(ctx, p.ChargeID, amt)");
    expect(prompt).toContain("Suggested direction: Save the refund as pending");
    expect(prompt).toContain("add a test that fails without the fix");
  });

  it("shows why starting an analysis failed", async () => {
    vi.mocked(startInsights).mockRejectedValue(
      apiError("model_not_configured", "Save an API key for the OpenAI API in Preferences › Model."),
    );
    const user = await openInsights();

    await user.click(await screen.findByRole("button", { name: "Analyze" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("Save an API key for the OpenAI API in Preferences › Model.");
    expect(runInsightsBatch).not.toHaveBeenCalled();
  });
});
