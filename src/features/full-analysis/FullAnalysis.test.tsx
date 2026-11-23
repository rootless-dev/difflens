import { act, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { applyEvent, FullAnalysesProvider, mergeListed } from "@/features/full-analysis/full-analyses";
import { currentLabel, logText, logTime } from "@/features/full-analysis/log-text";
import { RunningAnalyses } from "@/features/full-analysis/RunningAnalyses";
import { FullAnalysisSection } from "@/features/preferences/FullAnalysisSection";
import { BranchRow } from "@/features/project/BranchLists";
import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { setLanguage } from "@/i18n";
import {
  cancelFullAnalysis,
  dismissFullAnalysis,
  type FullAnalysisEvent,
  getFileVersions,
  getFullAnalyses,
  getFullAnalysisLimit,
  getOverview,
  getReviewState,
  getTestsState,
  linkPullRequest,
  listChangedFiles,
  listenToFullAnalyses,
  planFullAnalysis,
  type ReviewTarget,
  setFullAnalysisLimit,
  startFullAnalysis,
  summarizeOverview,
} from "@/lib/api";
import { apiError, makeBranch, makeChangeSet, makeChangedFile, textSide } from "@/test/fixtures";
import { logLine, makeFullAnalysisPlan, makeJob } from "@/test/full-analysis-fixtures";
import { makeModelInUse } from "@/test/insights-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead, reviewTab } from "@/test/review-screen";
import { makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  linkPullRequest: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  summarizeOverview: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  getInsights: vi.fn(),
  getFullAnalyses: vi.fn(),
  getFullAnalysisLimit: vi.fn(),
  setFullAnalysisLimit: vi.fn(),
  listenToFullAnalyses: vi.fn(),
  planFullAnalysis: vi.fn(),
  startFullAnalysis: vi.fn(),
  cancelFullAnalysis: vi.fn(),
  dismissFullAnalysis: vi.fn(),
}));

vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => <div data-testid="diff-editor" data-path={props.path} />,
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const changeSet = makeChangeSet([makeChangedFile("internal/refund/service.go", { status: "added" })], {
  target: BRANCH,
});
const REVIEW = { target: BRANCH, oldSha: changeSet.oldSha, newSha: changeSet.newSha };

let bar: HTMLDivElement;
/** Sends an event as the core would. */
let emit: (event: FullAnalysisEvent) => void = () => {};

function renderReview() {
  return render(
    <FullAnalysesProvider>
      <ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />
    </FullAnalysesProvider>,
  );
}

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening(BRANCH.kind === "branch" ? BRANCH.branch : "");
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a\n"), new: textSide("b\n") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
  vi.mocked(getFullAnalyses).mockResolvedValue([]);
  vi.mocked(listenToFullAnalyses).mockImplementation((handler) => {
    emit = handler;
    return Promise.resolve(() => {});
  });
  vi.mocked(cancelFullAnalysis).mockResolvedValue(undefined);
  vi.mocked(dismissFullAnalysis).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("the words of a full analysis", () => {
  it("words each log line in the language shown", () => {
    const finished = { code: "tests_finished", passed: 41, failed: 1, skipped: 0, durationMs: 37_200 } as const;
    expect(logText(finished)).toBe("41 passed, 1 failed (37 s)");
    expect(logText({ code: "insights_planned", files: 60, calls: 3 })).toBe("Insights: 60 files in 3 calls");
    expect(logText({ code: "insights_call_answered", number: 1, total: 3, findings: 4 })).toBe(
      "Call 1 of 3 answered: 4 findings",
    );
    expect(logText({ code: "step_failed", step: "summary", error: apiError("internal", "boom") })).toBe(
      "Summary failed: boom",
    );

    setLanguage("pt-BR");
    expect(logText({ code: "tests_started" })).toBe("Rodando testes no sandbox…");
    expect(logText(finished)).toBe("41 passaram, 1 falhou (37 s)");
    expect(logText({ code: "insights_planned", files: 60, calls: 3 })).toBe("Insights: 60 arquivos em 3 chamadas");
    expect(logText({ code: "insights_call_answered", number: 1, total: 3, findings: 4 })).toBe(
      "Chamada 1 de 3 respondeu: 4 achados",
    );
    expect(logText({ code: "explanations_planned", changed: 3, calls: 1 })).toBe(
      "Explicações curtas: 3 funções alteradas em 1 chamada",
    );
    expect(logText({ code: "finished_with_failures", steps: ["tests", "summary"] })).toBe(
      "Concluída com falhas em: Testes e Resumo",
    );
  });

  it("says what is in progress, call by call", () => {
    const job = makeJob({
      steps: [
        { step: "tests", status: "done", done: 0, total: 0, error: null },
        { step: "metrics", status: "done", done: 3, total: 3, error: null },
        { step: "insights", status: "running", done: 1, total: 3, error: null },
        { step: "summary", status: "done", done: 0, total: 0, error: null },
        { step: "explanations", status: "running", done: 0, total: 0, error: null },
      ],
    });
    expect(currentLabel(job)).toBe("Insights 1 of 3 · Explanations");
    expect(currentLabel(makeJob({ status: "queued" }))).toBe("Queued");
  });

  it("keeps each job's whole log from its events, and the longer of two pictures", () => {
    const job = makeJob({ log: [logLine({ code: "started" })] });
    const { log: _log, ...summary } = job;
    const next = applyEvent(new Map([[job.id, job]]), {
      job: { ...summary, progress: 60 },
      line: logLine({ code: "tests_started" }),
    });
    expect(next.get(job.id)?.progress).toBe(60);
    expect(next.get(job.id)?.log.map((line) => line.code)).toEqual(["started", "tests_started"]);

    const merged = mergeListed(next, [job, makeJob({ id: "job-2" })]);
    expect(merged.get(job.id)?.log).toHaveLength(2);
    expect([...merged.keys()]).toEqual(["job-1", "job-2"]);
  });
});

describe("a full analysis of a review", () => {
  it("starts only on the confirmation's click, after saying what runs, what is sent and to whom", async () => {
    const user = userEvent.setup();
    vi.mocked(planFullAnalysis).mockResolvedValue(makeFullAnalysisPlan());
    vi.mocked(startFullAnalysis).mockResolvedValue(makeJob({ status: "queued", log: [logLine({ code: "queued" })] }));
    renderReview();
    await changedFilesRead();

    await user.click(await screen.findByRole("button", { name: "Full analysis" }));
    expect(planFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null);
    const lines = within(await screen.findByRole("list", { name: "Full analysis" }));
    expect(lines.getByText("Tests run in the sandbox, with coverage.")).toBeInTheDocument();
    expect(
      lines.getByText("Insights: 60 files in 3 calls, about 90k tokens, with Claude Code, model opus."),
    ).toBeInTheDocument();
    expect(
      lines.getByText("Summary of the production code: 1 call, about 12k tokens, with Claude Code, model opus."),
    ).toBeInTheDocument();
    expect(
      lines.getByText(
        "Short explanations: 5 changed functions in 1 call, about 8k tokens, with Claude Code, model opus. Explain " +
          "in detail any one later on the Flow tab.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByText(/sent to Claude Code, model opus/)).toBeInTheDocument();
    expect(startFullAnalysis).not.toHaveBeenCalled();
    expect(summarizeOverview).not.toHaveBeenCalled();

    await user.click(screen.getByRole("button", { name: "Start full analysis" }));
    expect(startFullAnalysis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null);
    // Queued: the review is covered at once, and says it waits.
    expect(await screen.findByRole("heading", { level: 1, name: "Full analysis" })).toBeInTheDocument();
    expect(screen.getAllByText("Queued").length).toBeGreaterThan(0);
  });

  it("does not offer to start when no model is set up", async () => {
    const user = userEvent.setup();
    const missing = makeModelInUse({ ready: false, problem: "Save an API key." });
    vi.mocked(planFullAnalysis).mockResolvedValue(
      makeFullAnalysisPlan({}, { insights: missing, summary: missing, explanations: missing }),
    );
    renderReview();
    await changedFilesRead();
    await user.click(await screen.findByRole("button", { name: "Full analysis" }));
    expect(await screen.findByText("No model is set up. Save an API key.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start full analysis" })).not.toBeInTheDocument();
  });

  it("names the model of each part when the parts use different ones", async () => {
    const user = userEvent.setup();
    vi.mocked(planFullAnalysis).mockResolvedValue(
      makeFullAnalysisPlan(
        {},
        {
          insights: makeModelInUse({ provider: "anthropic", providerName: "Anthropic API", model: "claude-sonnet-5-5" }),
          summary: makeModelInUse({ provider: "openai", providerName: "OpenAI API", model: "gpt-5.5" }),
        },
      ),
    );
    renderReview();
    await changedFilesRead();
    await user.click(await screen.findByRole("button", { name: "Full analysis" }));
    const lines = within(await screen.findByRole("list", { name: "Full analysis" }));
    expect(
      lines.getByText("Insights: 60 files in 3 calls, about 90k tokens, with Anthropic API, model claude-sonnet-5-5."),
    ).toBeInTheDocument();
    expect(
      lines.getByText("Summary of the production code: 1 call, about 12k tokens, with OpenAI API, model gpt-5.5."),
    ).toBeInTheDocument();
    expect(lines.getByText(/^Short explanations: .* with Claude Code, model opus\./)).toBeInTheDocument();
    expect(
      screen.getByText(
        "The code, the card, the pull request and the commit messages are sent to the model named on each part.",
      ),
    ).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Start full analysis" })).toBeEnabled();
  });

  it("does not offer to start when the model chosen for one part is no longer set up, and says which", async () => {
    const user = userEvent.setup();
    vi.mocked(planFullAnalysis).mockResolvedValue(
      makeFullAnalysisPlan(
        {},
        {
          summary: makeModelInUse({
            provider: "openai",
            providerName: "OpenAI API",
            model: "gpt-5.5",
            ready: false,
            problem: {
              code: "model_not_configured",
              message: "The OpenAI API was chosen for the summary, and it is not set up any more.",
              reason: "task_provider_not_set_up",
              params: {
                provider: { message: "The OpenAI API", reason: "provider_the_api_capitalized", params: { name: "OpenAI API" } },
                task: { message: "the summary", reason: "task_summary" },
              },
            },
          }),
        },
      ),
    );
    renderReview();
    await changedFilesRead();
    await user.click(await screen.findByRole("button", { name: "Full analysis" }));
    expect(
      await screen.findByText(
        "The OpenAI API was chosen for the summary, and it is not set up any more. Set it up again, or choose another " +
          "model for the summary, in Preferences › Model.",
      ),
    ).toBeInTheDocument();
    expect(screen.queryByText(/No model is set up/)).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Start full analysis" })).not.toBeInTheDocument();
  });

  it("covers the review while it runs: tabs disabled, progress, a log that follows and folds, and Cancel", async () => {
    const user = userEvent.setup();
    const job = makeJob({ log: [logLine({ code: "started" }), logLine({ code: "tests_started" })] });
    vi.mocked(getFullAnalyses).mockResolvedValue([job]);
    renderReview();

    const log = await screen.findByRole("log", { name: "Log of the full analysis" });
    expect(within(log).getByText("Running tests in the sandbox…")).toBeInTheDocument();
    // Each line has its time, in the local time zone: "14:03:27".
    expect(within(log).getAllByText(logTime("2026-11-19T14:03:27Z"))).toHaveLength(2);
    expect(logTime("2026-11-19T14:03:27Z")).toMatch(/^\d\d:\d\d:27$/);
    expect(screen.getByRole("progressbar", { name: "Progress of the full analysis" })).toHaveAttribute(
      "aria-valuenow",
      "45",
    );
    await changedFilesRead();
    for (const name of ["Overview", "Flow", "Diff", "Tests", "Quality", "Insights"]) {
      expect(reviewTab(name)).toHaveAttribute("aria-disabled", "true");
    }

    // A line the core logs arrives as an event.
    const { log: _log, ...summary } = job;
    act(() =>
      emit({
        job: { ...summary, progress: 60 },
        line: logLine({ code: "tests_finished", passed: 41, failed: 1, skipped: 0, durationMs: 37_000 }),
      }),
    );
    expect(within(log).getByText("41 passed, 1 failed (37 s)")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "▾ Details" }));
    expect(screen.queryByRole("log")).not.toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "▸ Details" }));
    expect(screen.getByRole("log")).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(cancelFullAnalysis).toHaveBeenCalledExactlyOnceWith("job-1");
  });

  it("gives the review back with the results once every step succeeded", async () => {
    const job = makeJob();
    vi.mocked(getFullAnalyses).mockResolvedValue([job]);
    renderReview();
    await screen.findByRole("log");
    await changedFilesRead();
    // The overview is first read by an effect that may run after the files are on screen; counted before it, its
    // first read would pass for the read again below.
    await waitFor(() => expect(getOverview).toHaveBeenCalled());
    const reads = vi.mocked(getOverview).mock.calls.length;

    const { log: _log, ...summary } = job;
    act(() =>
      emit({
        job: { ...summary, status: "finished", outcome: "succeeded", progress: 100 },
        line: logLine({ code: "finished" }),
      }),
    );
    await waitFor(() => expect(screen.queryByRole("log")).not.toBeInTheDocument());
    expect(reviewTab("Overview")).toHaveAttribute("aria-selected", "true");
    expect(reviewTab("Flow")).not.toHaveAttribute("aria-disabled", "true");
    expect(dismissFullAnalysis).toHaveBeenCalledExactlyOnceWith("job-1");
    // What the steps wrote is read again.
    await waitFor(() => expect(vi.mocked(getOverview).mock.calls.length).toBeGreaterThan(reads));
  });

  it("stays with what failed until See results", async () => {
    const user = userEvent.setup();
    const failed = makeJob({
      status: "finished",
      outcome: "failed",
      progress: 100,
      steps: makeJob().steps.map((step) =>
        step.step === "summary"
          ? { ...step, status: "failed", error: apiError("claude_cli_failed", "The model did not answer.") }
          : { ...step, status: "done" },
      ),
      log: [logLine({ code: "finished_with_failures", steps: ["summary"] })],
    });
    vi.mocked(getFullAnalyses).mockResolvedValue([failed]);
    renderReview();

    expect(await screen.findByRole("heading", { name: "The full analysis finished with failures" })).toBeInTheDocument();
    expect(screen.getByText("The model did not answer.")).toBeInTheDocument();
    expect(screen.getByText("Finished with failures in: Summary")).toBeInTheDocument();
    await changedFilesRead();
    expect(reviewTab("Flow")).toHaveAttribute("aria-disabled", "true");

    await user.click(screen.getByRole("button", { name: "See results" }));
    expect(dismissFullAnalysis).toHaveBeenCalledExactlyOnceWith("job-1");
    expect(screen.queryByRole("heading", { name: "The full analysis finished with failures" })).not.toBeInTheDocument();
    expect(reviewTab("Flow")).not.toHaveAttribute("aria-disabled", "true");
  });
});

describe("the limit in Preferences", () => {
  it("is 4 until the user picks another, kept by the core", async () => {
    const user = userEvent.setup();
    vi.mocked(getFullAnalysisLimit).mockResolvedValue(4);
    vi.mocked(setFullAnalysisLimit).mockResolvedValue(2);
    render(<FullAnalysisSection />);

    const select = await screen.findByRole("combobox", { name: "Simultaneous full analyses" });
    expect(select).toHaveTextContent("4");
    await user.click(select);
    await user.click(await screen.findByRole("option", { name: "2" }));
    expect(setFullAnalysisLimit).toHaveBeenCalledExactlyOnceWith(2);
    await waitFor(() => expect(select).toHaveTextContent("2"));
  });
});

describe("the indicators outside the review", () => {
  it("shows the analyses running in the top bar, each opening its review", async () => {
    const user = userEvent.setup();
    const onOpen = vi.fn();
    vi.mocked(getFullAnalyses).mockResolvedValue([
      makeJob(),
      makeJob({
        id: "job-2",
        status: "queued",
        review: { target: { kind: "working_tree" }, oldSha: "a", newSha: null },
        queuedAt: "2026-11-19T14:04:00Z",
      }),
      makeJob({ id: "job-3", status: "finished", outcome: "succeeded", queuedAt: "2026-11-19T14:01:00Z" }),
    ]);
    render(
      <FullAnalysesProvider>
        <RunningAnalyses onOpen={onOpen} />
      </FullAnalysesProvider>,
    );

    await user.click(await screen.findByRole("button", { name: "2 analyses running" }));
    const menu = within(await screen.findByRole("menu"));
    expect(menu.getByText("45%")).toBeInTheDocument();
    expect(menu.getByText("Queued")).toBeInTheDocument();
    await user.click(menu.getByRole("menuitem", { name: "Open payments / feat/refund-flow" }));
    expect(onOpen).toHaveBeenCalledWith(expect.objectContaining({ id: "job-1" }));
  });

  it("shows nothing when no full analysis runs", async () => {
    render(
      <FullAnalysesProvider>
        <RunningAnalyses onOpen={() => {}} />
      </FullAnalysesProvider>,
    );
    await waitFor(() => expect(getFullAnalyses).toHaveBeenCalled());
    expect(screen.queryByRole("button", { name: /running/ })).not.toBeInTheDocument();
  });

  it("says on a branch's row how far its analysis is, or that it waits", () => {
    const row = (analysis: ReturnType<typeof makeJob>) =>
      render(
        <ul>
          <BranchRow
            projectId="p-1"
            branch={makeBranch("feat/refund-flow")}
            bases={["main"]}
            compare={undefined}
            onChangeBase={() => Promise.resolve()}
            onReview={() => {}}
            analysis={analysis}
          />
        </ul>,
      );
    const { unmount } = row(makeJob());
    expect(screen.getByRole("status", { name: "Full analysis: 45%" })).toHaveTextContent("45%");
    expect(screen.getByRole("button", { name: "Actions for feat/refund-flow" })).toHaveAttribute("aria-busy", "true");
    unmount();
    setLanguage("pt-BR");
    row(makeJob({ status: "queued" }));
    expect(screen.getByText("Na fila")).toBeInTheDocument();
  });
});
