import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  type CrossedLimit,
  draftSignalComment,
  getFileVersions,
  getQuality,
  getReviewState,
  getTestsState,
  listChangedFiles,
  type QualityResult,
  type ReviewTarget,
  runTests,
  type SignalDraft,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeComment, makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, changedFilesRead, openReviewTab } from "@/test/review-screen";
import {
  makeCheck,
  makeDuplicate,
  makeQualityFunction,
  makeQualityReport,
  makeTestRun,
  makeTestsState,
  refundFunctions,
} from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  runTests: vi.fn(),
  getQuality: vi.fn(),
  getReviewState: vi.fn(),
  draftSignalComment: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
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

const changeSet = makeChangeSet([
  makeChangedFile("internal/payment/repo.go"),
  makeChangedFile("internal/refund/service.go", { status: "added" }),
  makeChangedFile("internal/refund/validate.go", { status: "added" }),
  makeChangedFile("internal/shipping/cost.go", { status: "added" }),
]);

const report = (overrides: Parameters<typeof makeQualityReport>[0] = {}): QualityResult => ({
  kind: "report",
  report: makeQualityReport(overrides),
});

/** The example of a function over two limits: complexity 14 and 40% of its changed lines covered. */
const shipping = makeQualityFunction("shipping.calculateShipping", {
  container: "internal/shipping",
  file: "internal/shipping/cost.go",
  startLine: 44,
  endLine: 46,
  complexity: 14,
  changedLines: { covered: 4, coverable: 10 },
  crap: 29,
  code: ["func calculateShipping(weight int, express bool) int {", "\treturn weight * 2", "}"],
});
const shippingRef = { id: shipping.id, qualifiedName: shipping.qualifiedName };
const shippingCrossed: CrossedLimit[] = [
  { kind: "complexity", function: shippingRef, value: 14, limit: 10 },
  { kind: "changed_lines_coverage", function: shippingRef, value: 0.4, limit: 0.8 },
];

let bar: HTMLDivElement;

const tab = (name: RegExp) => within(screen.getByRole("tablist", { name: "Review views" })).getByRole("tab", { name });
const pane = (name: RegExp) =>
  within(screen.getByRole("group", { name: "Quality views" })).getByRole("button", { name });
const overList = () => screen.getByRole("list", { name: "Functions over a limit" });
const overItems = () => within(overList()).getAllByRole("listitem");
/** The row of each function over a limit: the button that opens it in the Diff. */
const overRows = () => overItems().map((item) => within(item).getAllByRole("button")[0]);
const checks = () => screen.getByRole("region", { name: "Static checks" });
/** Each term of the static checks list with what follows it. */
const checkLines = () =>
  within(checks())
    .getAllByRole("term")
    .map((term) => `${term.textContent}: ${term.nextElementSibling?.textContent}`);

async function openQuality(user = userEvent.setup()) {
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
  await openReviewTab(user, /^Quality/);
  return user;
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
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeTestRun() }));
  vi.mocked(getQuality).mockResolvedValue(report());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
});

afterEach(() => {
  bar.remove();
  vi.useRealTimers();
});

describe("Quality tab", () => {
  it("measures when the review opens, since the Overview shows it, and counts the limits crossed in the tab", async () => {
    const user = userEvent.setup();
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await changedFilesRead();

    expect(tab(/^Overview/)).toHaveAttribute("aria-selected", "true");
    // The report is asked for by an effect of the render that shows the files, which React may run after the files
    // are on screen: wait for the call rather than expect it at once.
    await vi.waitFor(() =>
      expect(getQuality).toHaveBeenCalledExactlyOnceWith("p-1", "a1b2c3d4e5f6a7b8", "e7f3b10c9d8e7f6a"),
    );
    await vi.waitFor(() => expect(tab(/^Quality/)).toHaveAccessibleName("Quality, 4 limits crossed"));
    expect(tab(/^Quality/)).toHaveTextContent(/^Quality4/);

    await user.click(tab(/^Quality/));

    expect(await screen.findByRole("list", { name: "Functions over a limit" })).toBeInTheDocument();
    // Opening the tab shows what was measured; it does not measure again.
    expect(getQuality).toHaveBeenCalledOnce();
  });

  it("splits into Functions, Checks and Duplication, each counting what needs attention", async () => {
    vi.mocked(getQuality).mockResolvedValue(
      report({
        checks: [
          makeCheck("go vet"),
          makeCheck("staticcheck", {
            status: "findings",
            findings: [
              { file: "internal/refund/service.go", line: 53, message: "unused", rule: "SA4006", severity: "error" },
            ],
          }),
        ],
        raceDetector: "races",
        duplicates: [makeDuplicate()],
        testDuplicates: [makeDuplicate({ block: { file: "a_test.go", startLine: 1, endLine: 8, code: [] } })],
      }),
    );
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    expect(pane(/^Functions/)).toHaveTextContent("Functions (2)");
    expect(pane(/^Functions/)).toHaveAttribute("aria-pressed", "true");
    // One finding and the races.
    expect(pane(/^Checks/)).toHaveTextContent("Checks (2)");
    // The copy between tests is not counted.
    expect(pane(/^Duplication/)).toHaveTextContent("Duplication (1)");
    expect(pane(/^Duplication/)).toHaveAttribute("title", expect.stringMatching(/copies between tests are not counted/));

    await user.click(pane(/^Checks/));
    expect(pane(/^Checks/)).toHaveAttribute("aria-pressed", "true");
    expect(checks()).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Functions over a limit" })).not.toBeInTheDocument();
  });

  it("shows how long the static checks have been running", async () => {
    // Only Date: the elapsed seconds are read from it; no interval is left on a fake clock for later tests.
    vi.useFakeTimers({ toFake: ["Date"] });
    const pending = deferred<QualityResult>();
    vi.mocked(getQuality).mockReturnValue(pending.promise);
    await openQuality();

    const progress = screen.getByRole("status", { name: "Measuring the code" });
    expect(progress).toHaveTextContent("Measuring the changed functions and running the static checks0 s");
    expect(progress).toHaveTextContent("Nothing of the project runs");

    vi.useRealTimers();
    pending.resolve(report());
    expect(await screen.findByRole("list", { name: "Functions over a limit" })).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Measuring the code" })).not.toBeInTheDocument();
  });

  it("lists only the functions over a limit, each with the limits it crosses, and folds the others", async () => {
    const user = await openQuality();

    await screen.findByRole("list", { name: "Functions over a limit" });
    expect(overRows().map((row) => row.textContent)).toEqual(["refund.Validate1264%21", "refund.Service.Request1148%34.2"]);
    expect(overItems().map((item) => within(item).getByText(/^Crosses/).parentElement?.textContent)).toEqual([
      "Crosses: complexity 12 (limit 10)",
      "Crosses: complexity 11 (limit 10) · CRAP 34.2 (limit 30)",
    ]);
    expect(overRows()[1]).toHaveAccessibleName(
      "refund.Service.Request, complexity 11, over the limit, changed lines covered 48%, CRAP 34.2, over the limit",
    );
    const [, complexity, , crap] = overRows()[1].children;
    expect(complexity).toHaveClass("text-del");
    expect(crap).toHaveClass("text-del");
    const [, validateComplexity, validateCoverage, validateCrap] = overRows()[0].children;
    expect(validateComplexity).toHaveClass("text-del");
    expect(validateCoverage).toHaveClass("text-mod");
    expect(validateCrap).not.toHaveClass("text-del");

    // The function within the limits is one folded row.
    const folded = screen.getByRole("button", { name: "1 function within the limits" });
    expect(folded).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("button", { name: /^payment\.Repo\.Get/ })).not.toBeInTheDocument();
    await user.click(folded);
    expect(folded).toHaveAttribute("aria-expanded", "true");
    const within_ = within(screen.getByRole("list", { name: "Functions within the limits" })).getAllByRole("button");
    expect(within_.map((row) => row.textContent)).toEqual(["payment.Repo.Get490%4"]);
    expect(within_[0].children[1]).toHaveAttribute("title", "3 before this branch");

    // The review-wide coverage, which is no function's, stays.
    expect(
      screen.getByText("Changed lines of the whole branch are under the coverage target: 67% covered, target is 80%."),
    ).toBeInTheDocument();
    expect(screen.getByText(/^Coverage and CRAP from the test run started/)).toBeInTheDocument();
    expect(
      screen.getByText("Limits: complexity 10 and CRAP 30 per function, 80% of changed lines covered."),
    ).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Limits crossed" })).not.toBeInTheDocument();
  });

  it("says when no changed function crosses a limit, and still lists them folded", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ crossed: [] }));
    await openQuality();

    expect(await screen.findByText("No changed function crosses a limit.")).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Functions over a limit" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "3 functions within the limits" })).toBeInTheDocument();
    expect(pane(/^Functions/)).toHaveTextContent("Functions (0)");
    expect(tab(/^Quality/)).toHaveAccessibleName("Quality");
  });

  it("says when no function changed", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ functions: [], crossed: [] }));
    await openQuality();

    expect(await screen.findByText("No production function changed in this branch.")).toBeInTheDocument();
  });

  it("opens a function in the Diff tab at its first line", async () => {
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    await user.click(overRows()[1]);

    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "44");
    expect(editor).toHaveAttribute("data-scroll-side", "new");
  });

  it("shows coverage and CRAP as unknown without a test run, and leads to the Tests tab", async () => {
    const untested = refundFunctions.map((fn) => ({ ...fn, changedLines: null, crap: null }));
    vi.mocked(getQuality).mockResolvedValue(
      report({
        functions: untested,
        crossed: makeQualityReport().crossed.filter((limit) => limit.kind === "complexity"),
        testRunAt: null,
        coverage: { state: "no_run", reasons: [] },
        raceDetector: "not_run",
      }),
    );
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
    const user = await openQuality();

    await screen.findByRole("list", { name: "Functions over a limit" });
    expect(overRows()[0]).toHaveTextContent("refund.Validate12——");
    expect(screen.getByText(/^Coverage and CRAP need a test run\./)).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Run the tests" }));
    expect(tab(/^Tests/)).toHaveAttribute("aria-selected", "true");
    expect(runTests).not.toHaveBeenCalled();
  });

  it("says why coverage and CRAP are empty after a run that measured no coverage, in the analyzer's words", async () => {
    // The run behind "coverage and CRAP always show a dash": it completed, coverage was asked for, and the
    // project's environment had no coverage tool.
    const reason =
      "coverage was not measured: coverage.py is not installed in the project's environment (/envs/acme/bin/python)";
    const untested = refundFunctions.map((fn) => ({ ...fn, changedLines: null, crap: null }));
    vi.mocked(getQuality).mockResolvedValue(
      report({
        functions: untested,
        crossed: makeQualityReport().crossed.filter((limit) => limit.kind === "complexity"),
        coverage: { state: "not_measured", reasons: [reason] },
      }),
    );
    const user = await openQuality();

    await screen.findByRole("list", { name: "Functions over a limit" });
    const note = screen.getByRole("note");
    expect(note).toHaveTextContent(
      "The last test run measured no coverage, so Coverage and CRAP are empty. Install what the analyzer says is missing, then run the tests again.",
    );
    expect(within(note).getByLabelText("What the analyzer reported")).toHaveTextContent(reason);
    expect(screen.queryByText(/^Coverage and CRAP from the test run started/)).not.toBeInTheDocument();

    await user.click(within(note).getByRole("button", { name: "Run the tests" }));
    expect(tab(/^Tests/)).toHaveAttribute("aria-selected", "true");
  });

  it("says when the last run had coverage turned off", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ coverage: { state: "off", reasons: [] } }));
    await openQuality();

    expect(await screen.findByRole("note")).toHaveTextContent(
      "The last test run did not measure coverage, so Coverage and CRAP are empty. Run the tests with coverage on to fill them.",
    );
  });

  it("writes a short review comment draft on a function over a limit, once, and then shows it", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ functions: [shipping], crossed: shippingCrossed }));
    const signal = "quality:internal/shipping/cost.go:shipping.calculateShipping";
    const draft: SignalDraft = { signal, commentId: "c-7", kind: "line" };
    vi.mocked(draftSignalComment).mockImplementation(async () => {
      // Read back after the save: the review state now links the draft to its function.
      vi.mocked(getReviewState).mockResolvedValue(
        makeReviewState({
          comments: [makeComment("c-7", { path: "internal/shipping/cost.go", line: 44, currentLine: 44, body: "x" })],
          signalDrafts: [draft],
        }),
      );
      return draft;
    });
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    await user.click(screen.getByRole("button", { name: "Comment on the PR" }));

    expect(draftSignalComment).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      { target: BRANCH, oldSha: changeSet.oldSha, newSha: changeSet.newSha },
      {
        signal,
        path: "internal/shipping/cost.go",
        line: 44,
        endLine: 46,
        body:
          "`calculateShipping` has complexity 14 (limit 10) and 40% of its changed lines covered (target 80%); " +
          "consider splitting it into smaller functions and covering the new branches.",
      },
    );
    // No second draft: the button now shows the one there is.
    const view = await screen.findByRole("button", { name: "View draft" });
    expect(screen.queryByRole("button", { name: "Comment on the PR" })).not.toBeInTheDocument();
    await user.click(view);
    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
    expect(draftSignalComment).toHaveBeenCalledOnce();
  });

  it("comments on every function over a limit that has no draft yet", async () => {
    const existing: SignalDraft = {
      signal: "quality:internal/refund/validate.go:refund.Validate",
      commentId: "c-1",
      kind: "line",
    };
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ signalDrafts: [existing] }));
    vi.mocked(draftSignalComment).mockImplementation(async (_project, _review, request) => ({
      signal: request.signal,
      commentId: "c-2",
      kind: "line",
    }));
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });
    expect(within(overItems()[0]).getByRole("button", { name: "View draft" })).toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Comment on all" }));

    expect(draftSignalComment).toHaveBeenCalledOnce();
    expect(vi.mocked(draftSignalComment).mock.calls[0][2]).toMatchObject({
      signal: "quality:internal/refund/service.go:refund.Service.Request",
      body:
        "`Service.Request` has complexity 11 (limit 10) and a CRAP score of 34.2 (limit 30); " +
        "consider splitting it into smaller functions and covering the new branches.",
    });
  });

  it("copies a prompt for a coding agent to fix a function, with its place, metrics and code", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ functions: [shipping], crossed: shippingCrossed }));
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    await user.click(screen.getByRole("button", { name: "Copy as prompt" }));

    expect(await navigator.clipboard.readText()).toBe(
      [
        "Refactor the function `calculateShipping` in internal/shipping/cost.go, lines 44 to 46: in a code review it crosses the quality limits below.",
        "Metrics against the limits:\n- complexity 14 (limit 10)\n- 40% of the changed lines covered (target 80%)",
        "Its code (internal/shipping/cost.go:44-46):\n```\n44  func calculateShipping(weight int, express bool) int {\n45  \treturn weight * 2\n46  }\n```",
        "Keep the behaviour exactly as it is: callers must see no difference. Reduce the cyclomatic complexity, for example by extracting smaller functions, returning early or simplifying the conditions. Cover the changed lines with tests, the new branches especially, so the tests fail if the behaviour changes. Run the tests when you are done.",
      ].join("\n\n"),
    );
    expect(screen.getByRole("button", { name: "Copied" })).toBeInTheDocument();
    // A single function has no list-wide actions.
    expect(screen.queryByRole("button", { name: "Copy prompt for all" })).not.toBeInTheDocument();
  });

  it("copies one prompt for every function over a limit", async () => {
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    await user.click(screen.getByRole("button", { name: "Copy prompt for all" }));

    const prompt = await navigator.clipboard.readText();
    expect(prompt).toMatch(/^Refactor the 2 functions below: in a code review each crosses/);
    expect(prompt).toContain("1. `Validate` in internal/refund/validate.go, lines 12 to 39");
    expect(prompt).toContain("2. `Service.Request` in internal/refund/service.go, lines 44 to 71");
    expect(prompt).toContain("- CRAP 34.2 (limit 30)");
    expect(prompt).toContain("Its code could not be read; open internal/refund/validate.go:12-39.");
    expect(prompt).toMatch(/Run the tests when you are done\.$/);
  });

  it("shows each production duplicate beside what it copies or its other times in the change", async () => {
    vi.mocked(getQuality).mockResolvedValue(
      report({ duplicates: [makeDuplicate(), makeDuplicate({ kind: "repeated_in_change" })] }),
    );
    const user = await openQuality();
    await user.click(pane(/^Duplication/));

    const section = await screen.findByRole("region", { name: "Duplication" });
    const items = within(within(section).getByRole("list", { name: "Duplicated code" })).getAllByRole("listitem");
    expect(items).toHaveLength(2);
    expect(items[0]).toHaveTextContent("copies internal/payment/money.go:40-47");
    expect(items[0]).toHaveTextContent("74 tokens, 8 lines");
    expect(within(items[0]).getByText("internal/payment/money.go:40-47", { selector: "strong code" })).toBeInTheDocument();
    const copied = within(items[0]).getByLabelText("Code at internal/payment/money.go:40-47");
    expect(copied.children[0].textContent).toBe("40  \tcents40 := amount * 0");
    const fresh = within(items[0]).getByLabelText("Code at internal/refund/service.go:60-67");
    expect(fresh.children).toHaveLength(8);
    expect(within(items[0]).getByText("New code")).toBeInTheDocument();
    expect(within(items[0]).getByText("Already in the project")).toBeInTheDocument();
    // The original is outside the review: named, not a way to the Diff.
    expect(within(items[0]).getByText("internal/payment/money.go:40-47", { selector: "figcaption code" })).toHaveAttribute(
      "title",
      expect.stringMatching(/not/i),
    );

    expect(items[1]).toHaveTextContent("repeated 3× in the change");
    expect(within(items[1]).getAllByText("Also in this change")).toHaveLength(2);
    expect(within(section).getByText(/^Copies of at least 50 tokens and 6 lines/)).toHaveTextContent(
      /Tests and translation catalogs are left out/,
    );

    await user.click(within(items[1]).getByRole("button", { name: "internal/refund/validate.go:20-27" }));
    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/validate.go");
    expect(editor).toHaveAttribute("data-scroll-line", "20");
  });

  it("keeps the copies between tests apart, folded and not counted", async () => {
    const between = makeDuplicate({
      kind: "repeated_in_change",
      block: { file: "internal/refund/service_test.go", startLine: 10, endLine: 17, code: [] },
      others: [{ file: "internal/refund/validate_test.go", startLine: 4, endLine: 11, code: [] }],
      function: null,
    });
    vi.mocked(getQuality).mockResolvedValue(report({ testDuplicates: [between] }));
    const user = await openQuality();
    await user.click(pane(/^Duplication/));

    expect(pane(/^Duplication/)).toHaveTextContent("Duplication (0)");
    expect(screen.getByText("No duplicated production code in this branch.")).toBeInTheDocument();
    const tests = screen.getByRole("button", { name: "Tests: 1 copy between tests" });
    expect(tests).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("list", { name: "Duplicated test code" })).not.toBeInTheDocument();

    await user.click(tests);

    const items = within(screen.getByRole("list", { name: "Duplicated test code" })).getAllByRole("listitem");
    expect(items).toHaveLength(1);
    expect(items[0]).toHaveTextContent("internal/refund/service_test.go:10-17");
    expect(screen.getByText(/never between a test and production code/)).toBeInTheDocument();
  });

  it("says when the branch introduces no duplicated code", async () => {
    const user = await openQuality();
    await user.click(pane(/^Duplication/));
    const section = await screen.findByRole("region", { name: "Duplication" });
    expect(within(section).getByText("No duplicated production code in this branch.")).toBeInTheDocument();
    expect(screen.getByText("Tests: no copy between tests")).toBeInTheDocument();
  });

  it("lists the static checks with their findings, each opening its line in the Diff tab", async () => {
    vi.mocked(getQuality).mockResolvedValue(
      report({
        checks: [
          makeCheck("go vet"),
          makeCheck("staticcheck", {
            status: "findings",
            findings: [
              {
                file: "internal/refund/service.go",
                line: 53,
                message: "this value of err is never used",
                rule: "SA4006",
                severity: "error",
              },
              {
                file: "internal/ledger/ledger.go",
                line: 8,
                message: "should omit comparison to bool",
                rule: null,
                severity: "info",
              },
            ],
          }),
          makeCheck("golangci-lint", { status: "unavailable", message: "golangci-lint was not found on PATH." }),
          makeCheck("errcheck", { status: "failed", message: "errcheck: package internal/refund: type error" }),
        ],
        raceDetector: "races",
      }),
    );
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });
    await user.click(pane(/^Checks/));

    expect(checkLines()).toEqual([
      "go vet: Clean",
      "staticcheck: 2 findings",
      "golangci-lint: Not installed",
      "errcheck: Failed",
      "Race detector: Races foundSee the Tests tab",
    ]);
    const findings = within(screen.getByRole("list", { name: "Findings of staticcheck" }));
    expect(findings.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "internal/refund/service.go:53Errorthis value of err is never usedSA4006",
      "internal/ledger/ledger.go:8Infoshould omit comparison to bool",
    ]);
    // The severity in words, not only in the rule's colour.
    expect(findings.getByText("Error")).toHaveAttribute("data-check-severity", "error");
    expect(findings.getByText("Info")).toHaveAttribute("data-check-severity", "info");
    expect(findings.queryByRole("button", { name: "internal/ledger/ledger.go:8" })).not.toBeInTheDocument();
    expect(within(checks()).getByText("golangci-lint was not found on PATH.")).toBeInTheDocument();
    expect(within(checks()).getByLabelText("Output of errcheck")).toHaveTextContent("type error");

    await user.click(findings.getByRole("button", { name: "internal/refund/service.go:53" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "53");
  });

  it("says the race detector was off in the last run, or needs one", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ raceDetector: "not_run" }));
    const user = await openQuality();
    await user.click(pane(/^Checks/));

    expect(within(checks()).getByText("It was off in the last test run.")).toBeInTheDocument();
    expect(checkLines()).toContain("Race detector: Not run");
  });

  it("says calmly which languages have no analyzer and leads to the Diff tab", async () => {
    vi.mocked(getQuality).mockResolvedValue({ kind: "unavailable", reason: "no_analyzer", languages: ["typescript"] });
    const user = await openQuality();

    expect(
      await screen.findByText("No analyzer for TypeScript yet. The Diff tab works for every language."),
    ).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open the Diff tab" }));
    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
  });

  it("shows which side failed to analyse, and a failed call, each with a way to try again", async () => {
    vi.mocked(getQuality)
      .mockResolvedValueOnce({ kind: "unavailable", reason: "analysis_failed", side: "old", message: "go list: exit 1" })
      .mockRejectedValueOnce(apiError("unknown_project", "This project is no longer known."));
    const user = await openQuality();

    expect(await screen.findByRole("alert")).toHaveTextContent("Analysing the old side of this review failed.");
    expect(screen.getByLabelText("Analyzer output")).toHaveTextContent("go list: exit 1");
    await user.click(screen.getByRole("button", { name: "Try again" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("This project is no longer known.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("list", { name: "Functions over a limit" })).toBeInTheDocument();
  });

  it("lists the checker's problems on demand", async () => {
    vi.mocked(getQuality).mockResolvedValue(report({ diagnostics: ["staticcheck: skipped internal/gen"] }));
    const user = await openQuality();

    await user.click(await screen.findByText("1 problem while measuring"));
    expect(screen.getByText("staticcheck: skipped internal/gen")).toBeVisible();
  });

  it("keeps the report on screen, marked as updating, while a completed test run refreshes it", async () => {
    const user = await openQuality();
    await screen.findByRole("list", { name: "Functions over a limit" });

    const refreshed = deferred<QualityResult>();
    vi.mocked(getQuality).mockReturnValueOnce(refreshed.promise);
    vi.mocked(runTests).mockResolvedValue(makeTestRun({ startedAt: "2026-11-17T11:00:00Z" }));
    await user.click(tab(/^Tests/));
    await user.click(await screen.findByRole("button", { name: "Run all" }));
    await vi.waitFor(() => expect(getQuality).toHaveBeenCalledTimes(2));
    await user.click(tab(/^Quality/));

    expect(screen.getByRole("status")).toHaveTextContent("Updating with the latest test run");
    expect(overRows()).toHaveLength(2);

    const better = makeQualityFunction("refund.Service.Request", { complexity: 11, crap: 12 });
    await act(async () => {
      refreshed.resolve(report({ functions: [better], crossed: [] }));
    });

    expect(await screen.findByText("No changed function crosses a limit.")).toBeInTheDocument();
    expect(screen.queryByText("Updating with the latest test run")).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "1 function within the limits" })).toBeInTheDocument();
  });
});
