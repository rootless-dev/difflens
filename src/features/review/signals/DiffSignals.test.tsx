import { render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { DEFAULT_DIFF_OPTIONS, type DiffOptions, DiffToolbar } from "@/features/review/DiffToolbar";
import { FileDiff, type FileComments } from "@/features/review/FileDiff";
import { reviewSignals, type ReviewSignals, SignalsContext } from "@/features/review/signals/signals";
import { useFindingComments } from "@/features/review/signals/use-finding-comments";
import {
  cancelModelRun,
  type CrossedLimit,
  type Finding,
  getFileVersions,
  type SignalDraft,
  writeFindingComment,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeFinding } from "@/test/insights-fixtures";
import {
  makeDuplicate,
  makeLocatedCheck,
  makeMutationRun,
  makeQualityFunction,
  makeQualityReport,
} from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  getFileVersions: vi.fn(),
  writeFindingComment: vi.fn(),
  cancelModelRun: vi.fn(),
}));

// CodeMirror is tested on its own; the stub draws every block, inline ones marked, and the lane it was given.
vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div data-testid="diff-editor" data-lane={JSON.stringify(props.lane ?? [])}>
      {(props.annotations ?? []).map((annotation) => (
        <div key={annotation.key} data-testid={`under-${annotation.key}`} data-inline={annotation.inline || undefined}>
          {props.renderAnnotation?.(annotation.key)}
        </div>
      ))}
    </div>
  ),
}));

const SERVICE = "internal/refund/service.go";
const request = { id: "refund.Service.Request", qualifiedName: "refund.Service.Request" };
const limit = (kind: CrossedLimit["kind"], value: number, bound: number): CrossedLimit => ({
  kind,
  function: request,
  value,
  limit: bound,
});
const found = (id: string, line: number, endLine: number, overrides: Partial<Finding> = {}) =>
  makeFinding(id, { line, endLine, location: { kind: "found", line, endLine, moved: false }, ...overrides });

function signals(): ReviewSignals {
  return reviewSignals(
    makeQualityReport({
      functions: [
        makeQualityFunction("refund.Service.Request", {
          startLine: 44,
          endLine: 71,
          complexity: 14,
          changedLines: { covered: 12, coverable: 25 },
          crap: 42,
        }),
        makeQualityFunction("refund.Notify", {
          file: "internal/refund/notify.go",
          startLine: 3,
          endLine: 12,
          changedLines: { covered: 0, coverable: 9 },
          untested: true,
        }),
      ],
      crossed: [limit("complexity", 14, 10), limit("crap", 42, 30), limit("changed_lines_coverage", 0.48, 0.8)],
      checkFindings: [makeLocatedCheck({ line: 60 })],
    }),
    [
      found("high", 52, 56, { title: "Refund is never recorded" }),
      found("low", 58, 58, { severity: "low", title: "Amount is logged" }),
      found("gone", 47, 47, { verdict: "dismissed", title: "Dismissed one", verdictAt: "2026-11-18T10:00:00Z" }),
    ],
  );
}

const onOpenQuality = vi.fn();
const onOpenInsights = vi.fn();
const onOpenTests = vi.fn();
const create = vi.fn<(request: unknown) => Promise<SignalDraft | null>>();
const view = vi.fn();

function comments(drafts: SignalDraft[] = []): FileComments {
  return {
    threads: [],
    sides: new Set(["old", "new"]),
    actions: {
      add: vi.fn(),
      reply: vi.fn(),
      edit: vi.fn(),
      remove: vi.fn(),
      restore: vi.fn(),
      resolve: vi.fn(),
    },
    busy: false,
    signalDrafts: { drafts: new Map(drafts.map((draft) => [draft.signal, draft])), create, view },
  };
}

function renderFile({
  path = SERVICE,
  options = {},
  shown = signals(),
  drafts = [],
}: { path?: string; options?: Partial<DiffOptions>; shown?: ReviewSignals; drafts?: SignalDraft[] } = {}) {
  return render(
    <SignalsContext value={{ signals: shown, onOpenQuality, onOpenInsights, onOpenTests }}>
      <FileDiff
        projectId="p-1"
        changeSet={makeChangeSet([makeChangedFile(path)])}
        file={makeChangedFile(path)}
        options={{ ...DEFAULT_DIFF_OPTIONS, ...options }}
        comments={comments(drafts)}
      />
    </SignalsContext>,
  );
}

const lane = () => JSON.parse(screen.getByTestId("diff-editor").dataset.lane ?? "[]") as { line: number }[];

beforeEach(() => {
  vi.resetAllMocks();
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a\n"), new: textSide("b\n") });
});

describe("signals in the Diff", () => {
  it("sums up the file in its header", async () => {
    renderFile();
    const header = await screen.findByText("2 findings");
    expect(header.closest("[data-file-signals]")).toHaveTextContent("2 findings3 limits1 check");
    expect(header.closest("em")).toHaveClass("text-sev-high");
  });

  it("marks the first line of a function over a limit, and opens its limits", async () => {
    const user = userEvent.setup();
    renderFile();

    const pill = await screen.findByRole("button", {
      name: "Service.Request over 3 limits: cyclomatic complexity 14, CRAP 42, coverage of the changed lines 48%. Show the limits",
    });
    expect(pill).toHaveTextContent("CC 14· CRAP 42· coverage 48%");
    expect(pill.closest("[data-testid]")).toHaveAttribute("data-testid", "under-inline:44");
    expect(pill.closest("[data-testid]")).toHaveAttribute("data-inline", "true");
    expect(pill).toHaveAttribute("aria-expanded", "false");

    await user.click(pill);

    const panel = within(screen.getByRole("group", { name: "Service.Request is over 3 limits" }));
    expect(screen.getByTestId("under-new:44")).toContainElement(panel.getByText("48%"));
    expect(panel.getByText("/ minimum 80%")).toBeInTheDocument();
    await user.click(panel.getByRole("button", { name: "Open in Quality" }));
    expect(onOpenQuality).toHaveBeenCalledOnce();
    expect(pill).toHaveAttribute("aria-expanded", "true");
  });

  it("notes the function no test runs, with its detail as a tooltip", async () => {
    renderFile({ path: "internal/refund/notify.go" });
    const note = await screen.findByText("No test runs this function");
    expect(note.closest("span")).toHaveAttribute(
      "title",
      "None of its 9 changed lines ran in a test. Their coverage is 0%, the minimum is 80%.",
    );
    expect(screen.getByText("1 function without a test")).toBeInTheDocument();
    expect(lane()).toEqual([{ line: 3, untested: true }]);
  });

  it("draws the findings under their last line, folding, and a dismissed one folded", async () => {
    const user = userEvent.setup();
    renderFile();

    const block = within(await screen.findByRole("group", { name: "Refund is never recorded" }));
    expect(screen.getByTestId("under-new:56")).toContainElement(block.getByText("Refund is never recorded"));
    expect(block.getByText("Model")).toBeInTheDocument();
    expect(block.getByText("High")).toBeInTheDocument();
    expect(block.getByText("Correctness, lines 52–56")).toBeInTheDocument();
    expect(block.getByText("Suggested direction")).toBeInTheDocument();
    await user.click(block.getByRole("button", { name: "Open in Insights" }));
    expect(onOpenInsights).toHaveBeenCalledOnce();

    await user.click(block.getByRole("button", { name: "Collapse" }));
    expect(block.queryByText("Suggested direction")).not.toBeInTheDocument();
    expect(block.getByRole("button", { name: "Show" })).toHaveAttribute("aria-expanded", "false");

    const dismissed = within(screen.getByRole("group", { name: "Model finding, dismissed" }));
    expect(dismissed.getByText("Dismissed one")).toHaveClass("line-through");
    expect(dismissed.queryByText(/You dismissed this/)).not.toBeInTheDocument();
    await user.click(dismissed.getByRole("button", { name: "Show" }));
    expect(dismissed.getByText(/^You dismissed this on 18 Nov/)).toBeInTheDocument();

    expect(lane().filter((mark) => mark.line >= 52 && mark.line <= 56)).toHaveLength(5);
  });

  it("draws a static check under its line, in the tool's own words", async () => {
    renderFile();
    const check = within(await screen.findByRole("group", { name: "Static check: staticcheck SA4006, line 60" }));
    expect(check.getByText("this value of err is never used")).toHaveAttribute("lang", "en");
    expect(screen.getByTestId("under-new:60")).toContainElement(check.getByText("line 60"));
    // Its severity, as the tool reports it, in words.
    expect(check.getByText("Warning")).toHaveAttribute("data-check-severity", "warning");
  });

  it("writes a draft from a check with its own text, and shows it once there is one", async () => {
    const user = userEvent.setup();
    create.mockResolvedValue({ signal: "check:x", commentId: "c-1", kind: "general" });
    renderFile();

    const check = within(await screen.findByRole("group", { name: "Static check: staticcheck SA4006, line 60" }));
    await user.click(check.getByRole("button", { name: "Create comment draft" }));
    expect(create).toHaveBeenLastCalledWith({
      signal: "check:staticcheck:SA4006:internal/refund/service.go:60:this value of err is never used",
      path: SERVICE,
      line: 60,
      endLine: 60,
      body: "staticcheck reports on this line (SA4006): “this value of err is never used”",
    });
    // A general draft is shown among the comments.
    await vi.waitFor(() => expect(view).toHaveBeenCalledWith({ signal: "check:x", commentId: "c-1", kind: "general" }));
  });

  describe("a model finding's comment", () => {
    const onWritten = vi.fn();
    const onOpenPreferences = vi.fn();
    const CHANGE_SET = makeChangeSet([makeChangedFile(SERVICE)]);
    const REVIEW = { target: CHANGE_SET.target, oldSha: CHANGE_SET.oldSha, newSha: CHANGE_SET.newSha };

    function Harness({ drafts }: { drafts: SignalDraft[] }) {
      const findings = useFindingComments("p-1", CHANGE_SET, onWritten, onOpenPreferences);
      const base = comments(drafts);
      return (
        <SignalsContext value={{ signals: signals(), onOpenQuality, onOpenInsights, onOpenTests }}>
          <FileDiff
            projectId="p-1"
            changeSet={CHANGE_SET}
            file={makeChangedFile(SERVICE)}
            options={DEFAULT_DIFF_OPTIONS}
            comments={{ ...base, signalDrafts: { ...base.signalDrafts!, findings } }}
          />
        </SignalsContext>
      );
    }

    const block = async () => within(await screen.findByRole("group", { name: "Refund is never recorded" }));

    it("is written by the model on a click, showing it is written, and can be stopped", async () => {
      const user = userEvent.setup();
      const written = deferred<SignalDraft>();
      vi.mocked(writeFindingComment).mockReturnValue(written.promise);
      vi.mocked(cancelModelRun).mockResolvedValue(undefined);
      render(<Harness drafts={[]} />);

      const write = (await block()).getByRole("button", { name: "Write comment" });
      expect(write).toHaveAttribute(
        "title",
        "The Insights model writes a short review comment from this finding, kept as a draft you can edit before publishing",
      );
      await user.click(write);
      const runId = vi.mocked(writeFindingComment).mock.calls[0][3];
      expect(runId).toMatch(/^finding-comment-/);
      expect(writeFindingComment).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "high", runId);
      // The screen writes no text of its own.
      expect(create).not.toHaveBeenCalled();
      const status = (await block()).getByRole("status");
      expect(status).toHaveTextContent("Writing the comment…");
      expect((await block()).queryByRole("button", { name: "Write comment" })).not.toBeInTheDocument();

      await user.click(within(status).getByRole("button", { name: "Stop" }));
      expect(cancelModelRun).toHaveBeenCalledExactlyOnceWith(runId);
      expect(status).toHaveTextContent("Stopping…");

      written.resolve({ signal: "finding:high", commentId: "c-1", kind: "line" });
      await vi.waitFor(() => expect(onWritten).toHaveBeenCalledOnce());
      expect((await block()).queryByRole("status")).not.toBeInTheDocument();
    });

    it("says why it could not be written and tries again; a stopped one says nothing", async () => {
      const user = userEvent.setup();
      vi.mocked(writeFindingComment)
        .mockRejectedValueOnce(apiError("model_invalid_output", "Anthropic answered, but not with the JSON asked for"))
        .mockRejectedValueOnce(apiError("model_not_configured", "Set up a model in Preferences › Model."))
        .mockRejectedValueOnce(apiError("cancelled", "Stopped."));
      render(<Harness drafts={[]} />);

      await user.click((await block()).getByRole("button", { name: "Write comment" }));
      expect(await (await block()).findByRole("alert")).toHaveTextContent(
        "The draft could not be written: Anthropic answered, but not with the JSON asked for",
      );
      expect((await block()).queryByRole("button", { name: "Open Preferences" })).not.toBeInTheDocument();

      await user.click((await block()).getByRole("button", { name: "Try again" }));
      expect(await (await block()).findByText(/Set up a model in Preferences › Model\./)).toBeInTheDocument();
      await user.click((await block()).getByRole("button", { name: "Open Preferences" }));
      expect(onOpenPreferences).toHaveBeenCalledOnce();

      await user.click((await block()).getByRole("button", { name: "Try again" }));
      await (await block()).findByRole("button", { name: "Write comment" });
      expect((await block()).queryByRole("alert")).not.toBeInTheDocument();
      expect(writeFindingComment).toHaveBeenCalledTimes(3);
      expect(onWritten).not.toHaveBeenCalled();
    });

    it("once its draft is there, shows it and asks nothing", async () => {
      const user = userEvent.setup();
      const draft: SignalDraft = { signal: "finding:high", commentId: "c-1", kind: "line" };
      render(<Harness drafts={[draft]} />);

      await user.click((await block()).getByRole("button", { name: "See draft" }));
      expect(view).toHaveBeenCalledExactlyOnceWith(draft);
      expect(writeFindingComment).not.toHaveBeenCalled();
      expect(create).not.toHaveBeenCalled();
      // The other finding still offers one.
      expect(
        within(screen.getByRole("group", { name: "Amount is logged" })).getByRole("button", { name: "Write comment" }),
      ).toBeInTheDocument();
    });
  });

  it("puts duplicated code under the last line of its block, with the way to Quality and a draft", async () => {
    const user = userEvent.setup();
    create.mockResolvedValue({ signal: "x", commentId: "c-9", kind: "line" });
    const shown = reviewSignals(
      makeQualityReport({ checkFindings: [], crossed: [], duplicates: [makeDuplicate()] }),
      null,
    );
    renderFile({ shown });

    const note = await screen.findByRole("group", { name: "Duplicated code, lines 60–67" });
    expect(note.closest("[data-testid]")).toHaveAttribute("data-testid", "under-new:67");
    expect(note).toHaveTextContent("copies internal/payment/money.go:40-47");
    expect(note).toHaveTextContent("74 tokens, 8 lines");
    expect(document.querySelector("[data-file-signals]")).toHaveTextContent("1 duplicate");
    expect(lane()).toEqual([{ line: 60, duplicate: true }]);

    await user.click(within(note).getByRole("button", { name: "Open in Quality" }));
    expect(onOpenQuality).toHaveBeenCalledOnce();
    await user.click(within(note).getByRole("button", { name: "Create comment draft" }));
    expect(create).toHaveBeenCalledWith({
      signal: "duplicate:copies_existing:internal/refund/service.go:60:67",
      path: SERVICE,
      line: 60,
      endLine: 67,
      body: "This already exists in `internal/payment/money.go:40-47`; reuse it instead of duplicating it.",
    });
  });

  it("puts a mutant that survived under its line, with the way to Tests, the prompt and a draft", async () => {
    const user = userEvent.setup();
    create.mockResolvedValue({ signal: "x", commentId: "c-9", kind: "line" });
    const run = makeMutationRun();
    const shown = reviewSignals(makeQualityReport({ checkFindings: [], crossed: [] }), null, run);
    renderFile({ shown });

    const note = await screen.findByRole("group", { name: "Mutant no test caught, line 52" });
    expect(note.closest("[data-testid]")).toHaveAttribute("data-testid", "under-new:52");
    expect(note).toHaveTextContent("if balance >= amount { became if balance > amount { and no test failed");
    expect(note).toHaveTextContent("Comparison boundary");
    expect(screen.getByRole("group", { name: "Mutant no test caught, line 58" })).toHaveTextContent(
      "Removing s.notify(refund) made no test fail",
    );
    // Only the survivors: the one that timed out, the killed one and the error are not signals.
    expect(screen.queryByRole("group", { name: /line (64|66|68)$/ })).not.toBeInTheDocument();
    expect(document.querySelector("[data-file-signals]")).toHaveTextContent("2 mutants");
    expect(lane()).toEqual([
      { line: 52, mutant: true },
      { line: 58, mutant: true },
    ]);

    await user.click(within(note).getByRole("button", { name: "Open in Tests" }));
    expect(onOpenTests).toHaveBeenCalledOnce();
    await user.click(within(note).getByRole("button", { name: "Copy as a prompt for the agent" }));
    expect(await navigator.clipboard.readText()).toContain("Write the missing test for internal/refund/service.go:52");
    await user.click(within(note).getByRole("button", { name: "Create comment draft" }));
    expect(create).toHaveBeenCalledWith({
      signal: "mutant:m-1",
      path: SERVICE,
      line: 52,
      endLine: 52,
      body: "No test fails if `>=` becomes `>` here; a test for this boundary is missing.",
    });
  });

  it("shows the draft a mutant already has, and hides mutants with the checks", async () => {
    const shown = reviewSignals(makeQualityReport({ checkFindings: [], crossed: [] }), null, makeMutationRun());
    renderFile({ shown, drafts: [{ signal: "mutant:m-1", commentId: "c-1", kind: "line" }] });
    const note = await screen.findByRole("group", { name: "Mutant no test caught, line 52" });
    expect(within(note).getByRole("button", { name: "See draft" })).toBeInTheDocument();

    renderFile({ shown, options: { showChecks: false } });
    expect(screen.getAllByRole("group", { name: "Mutant no test caught, line 52" })).toHaveLength(1);
  });

  it("hides duplicated code with the checks", async () => {
    const shown = reviewSignals(makeQualityReport({ checkFindings: [], duplicates: [makeDuplicate()] }), null);
    renderFile({ shown, options: { showChecks: false } });
    expect(await screen.findByTestId("diff-editor")).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: /^Duplicated code/ })).not.toBeInTheDocument();
  });

  it("hides the findings and the checks when told, never the limits", async () => {
    renderFile({ options: { showFindings: false, showChecks: false } });
    expect(await screen.findByRole("button", { name: /^Service.Request over 3 limits/ })).toBeInTheDocument();
    expect(screen.queryByRole("group", { name: "Refund is never recorded" })).not.toBeInTheDocument();
    expect(screen.queryByRole("group", { name: /^Static check/ })).not.toBeInTheDocument();
    expect(lane()).toEqual([{ line: 44, limit: true }]);
  });

  it("adds nothing to a file without signals", async () => {
    renderFile({ shown: reviewSignals(null, null) });
    expect(await screen.findByTestId("diff-editor")).toBeInTheDocument();
    expect(document.querySelector("[data-file-signals]")).toBeNull();
    expect(screen.queryAllByTestId(/^under-/)).toHaveLength(0);
    expect(lane()).toEqual([]);
  });
});

describe("the Diff's signal toggles", () => {
  const renderToolbar = (signalsShown: Parameters<typeof DiffToolbar>[0]["signals"], onChange = vi.fn()) =>
    render(<DiffToolbar options={DEFAULT_DIFF_OPTIONS} onChange={onChange} signals={signalsShown} />);

  it("are only there when the review has something of their kind, each counting the file shown", async () => {
    const user = userEvent.setup();
    const onChange = vi.fn();
    renderToolbar({ findings: { review: true, inFile: 4 }, checks: { review: true, inFile: 1 } }, onChange);

    const findings = screen.getByRole("checkbox", { name: "Show model findings 4 in this file" });
    expect(findings).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Show checks 1 in this file" })).toBeChecked();
    await user.click(findings);
    expect(onChange).toHaveBeenCalledWith({ ...DEFAULT_DIFF_OPTIONS, showFindings: false });
  });

  it("are absent without findings or checks", () => {
    renderToolbar({ findings: { review: false, inFile: 0 }, checks: { review: false, inFile: 0 } });
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
  });
});
