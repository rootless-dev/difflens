import { cleanup, render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { setLanguage } from "@/i18n";
import {
  cancelModelRun,
  cancelTestDiagnosis,
  deleteComment,
  diagnoseTestFailure,
  getFileVersions,
  getModelInUse,
  getReviewState,
  getRunningTestDiagnoses,
  getTestDiagnoses,
  getTestsState,
  joinTestDiagnosis,
  listChangedFiles,
  planTestDiagnosesTranslation,
  type ReviewTarget,
  runTests,
  type TestDiagnosis,
  translateTestDiagnoses,
  type WrittenReviewComment,
  writeReviewComment,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeModelInUse } from "@/test/insights-fixtures";
import { makeComment, makeGeneralComment, makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab, reviewTab } from "@/test/review-screen";
import { makeFailingRun, makeTestCase, makeTestRun, makeTestsState } from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getTestsProgress: vi.fn(),
  runTests: vi.fn(),
  cancelTests: vi.fn(),
  getQuality: vi.fn(),
  getReviewState: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  getFlow: vi.fn(),
  getInsights: vi.fn(),
  getModelInUse: vi.fn(),
  cancelModelRun: vi.fn(),
  getTestDiagnoses: vi.fn(),
  diagnoseTestFailure: vi.fn(),
  getRunningTestDiagnoses: vi.fn(),
  joinTestDiagnosis: vi.fn(),
  cancelTestDiagnosis: vi.fn(),
  writeReviewComment: vi.fn(),
  deleteComment: vi.fn(),
  planTestDiagnosesTranslation: vi.fn(),
  translateTestDiagnoses: vi.fn(),
}));

vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div data-testid="diff-editor" data-path={props.path} data-scroll-line={props.scrollTo?.line ?? ""} />
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
const REVIEW = { target: BRANCH, oldSha: "a1b2c3d4e5f6a7b8", newSha: "e7f3b10c9d8e7f6a" };
const RACE_TEST = "TestSignUpFromEdge_CreatesAndLogsIn";
const ASSERTION_TEST = "TestAuthenticateAD_EveryRefusalIsTheGenericError";

const changeSet = makeChangeSet([
  makeChangedFile("app/services/auth_service/service.go"),
  makeChangedFile("app/services/auth_service/password_policy.go", { status: "added" }),
  makeChangedFile("app/services/auth_service/auth_service_test.go"),
  makeChangedFile("app/controllers/edge_login_test.go", { status: "added" }),
]);

function makeDiagnosis(overrides: Partial<TestDiagnosis> = {}): TestDiagnosis {
  return {
    key: `app/controllers::${RACE_TEST}`,
    package: "app/controllers",
    test: RACE_TEST,
    cause:
      "`checkAsync` writes `lastLogin` in a goroutine that keeps running after `SignUpFromEdge` returns, and the test reads it right away.",
    fault: "code",
    confidence: "high",
    location: { file: "app/services/auth_service/service.go", line: 661 },
    fix: "Hold the mutex `Service` already has while writing `lastLogin`.",
    fixCode: "s.mu.Lock()\ndefer s.mu.Unlock()\ns.lastLogin = at",
    language: "en",
    translations: [],
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "claude-opus-5-5",
    createdAt: new Date().toISOString(),
    runStartedAt: "2026-11-21T09:00:00Z",
    outdated: false,
    commentId: null,
    ...overrides,
  };
}

const card = (name: string) => within(screen.getByRole("article", { name }));
const explainButton = (name: string) => card(name).getByRole("button", { name: "Explain the failure" });

const READING = "Reading the explanations already written";

let bar: HTMLDivElement;

/** Opens the review on the Tests tab, its failure cards shown. */
async function openTests(user = userEvent.setup()) {
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
  await openReviewTab(user, /^Tests/);
  await screen.findByRole("article", { name: RACE_TEST });
  return user;
}

/** Opens the Tests tab and waits for the explanations already written to be read. */
async function openFailing(user = userEvent.setup()) {
  await openTests(user);
  await waitFor(() => expect(screen.queryByText(READING)).not.toBeInTheDocument());
  return user;
}

beforeEach(() => {
  vi.resetAllMocks();
  answerReviewOpening();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockImplementation(async (_id, _old, _new, path) => ({
    old: textSide(`old ${path}\n`),
    new: textSide(`new ${path}\n`),
  }));
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeFailingRun() }));
  vi.mocked(getModelInUse).mockResolvedValue(makeModelInUse());
  vi.mocked(getTestDiagnoses).mockResolvedValue([]);
  vi.mocked(cancelModelRun).mockResolvedValue(undefined);
  vi.mocked(getRunningTestDiagnoses).mockResolvedValue([]);
  vi.mocked(joinTestDiagnosis).mockResolvedValue(null);
  vi.mocked(cancelTestDiagnosis).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("Explain the failure", () => {
  it("is offered on each failure card only, naming the model and its one call, and asks nothing on open", async () => {
    await openFailing();

    expect(getTestDiagnoses).toHaveBeenCalledWith("p-1", REVIEW);
    expect(getModelInUse).toHaveBeenCalledWith("test_diagnosis");
    const buttons = screen.getAllByRole("button", { name: "Explain the failure" });
    // Two failing tests and a package that did not build.
    expect(buttons).toHaveLength(3);
    for (const name of [RACE_TEST, ASSERTION_TEST, "app/containers"]) {
      expect(explainButton(name)).toHaveAttribute("title", "Explain the failure with Claude Code, model opus: one model call");
    }
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });

  it("asks nothing and offers nothing when no test failed", async () => {
    vi.mocked(getTestsState).mockResolvedValue(
      makeTestsState({
        run: makeTestRun({
          tests: [makeTestCase("TestRefundIDs", { file: "internal/refund/ids_test.go", line: 9 })],
          summary: { passed: 1, failed: 0, skipped: 0, added: 0 },
          packages: [
            { container: "internal/refund", outcome: "pass", durationMs: 900, output: null, failure: null, command: null },
          ],
        }),
      }),
    );
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await openReviewTab(userEvent.setup(), /^Tests/);
    await screen.findByRole("heading", { level: 1 });

    expect(screen.queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();
    expect(getTestDiagnoses).not.toHaveBeenCalled();
  });

  it("shows the explanation being written with Stop, then why it failed, where, and the fix", async () => {
    const answer = deferred<TestDiagnosis>();
    vi.mocked(diagnoseTestFailure).mockReturnValue(answer.promise);
    const user = await openFailing();

    await user.click(explainButton(RACE_TEST));
    const runId = vi.mocked(diagnoseTestFailure).mock.calls[0][4];
    expect(diagnoseTestFailure).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "app/controllers", RACE_TEST, runId);
    const loading = card(RACE_TEST).getByRole("region", { name: "Explaining the failure with Claude Code, model opus" });
    expect(loading).toHaveAttribute("aria-busy", "true");
    expect(within(loading).getByRole("status")).toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();

    answer.resolve(makeDiagnosis());
    const block = within(await card(RACE_TEST).findByRole("region", { name: "Why it failed" }));
    expect(block.getByText("Model")).toBeInTheDocument();
    expect(block.getByText("code").closest("em")).toHaveTextContent("origin: code");
    expect(block.getByText("code").closest("em")).toHaveAttribute("title", "Origin: code, test, flaky or environment");
    expect(block.getByText(/Written by Claude Code, model claude-opus-5-5, /)).toBeInTheDocument();
    const terms = block.getAllByRole("term").map((term) => term.textContent);
    expect(terms).toEqual(["Cause", "Where", "Proposed fix"]);
    expect(block.getByText("checkAsync", { selector: "code" })).toBeInTheDocument();
    expect(block.getByLabelText("Proposed code")).toHaveTextContent("defer s.mu.Unlock()");
    expect(block.getByRole("button", { name: "Explain again" })).toHaveAttribute(
      "title",
      "Explain the failure again with Claude Code, model opus: one model call",
    );

    // Where opens the Diff at the line, the file being in the review.
    await user.click(block.getByRole("button", { name: "app/services/auth_service/service.go:661" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "app/services/auth_service/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "661");
  });

  it("names the place as plain text, with why, when the review does not have the file", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([
      makeDiagnosis({
        key: "app/containers::",
        package: "app/containers",
        test: null,
        cause: "`NewService` now takes a `PasswordPolicy`, and the container still calls it with two arguments.",
        location: { file: "app/containers/auth_container.go", line: 47 },
        fixCode: "",
      }),
    ]);
    await openFailing();

    const block = within(card("app/containers").getByRole("region", { name: "Why it failed" }));
    expect(block.queryByRole("button", { name: /auth_container\.go/ })).not.toBeInTheDocument();
    const place = block.getByText("app/containers/auth_container.go:47");
    expect(place).toHaveAttribute("title", expect.stringMatching(/^app\/containers\/auth_container\.go:47\. .+not changed in this review/));
    expect(block.queryByLabelText("Proposed code")).not.toBeInTheDocument();
  });

  it("stops the call on Stop and offers it again", async () => {
    const answer = deferred<TestDiagnosis>();
    vi.mocked(diagnoseTestFailure).mockReturnValue(answer.promise);
    vi.mocked(cancelTestDiagnosis).mockImplementation(async () => answer.reject(apiError("cancelled", "Stopped.")));
    const user = await openFailing();

    await user.click(explainButton(RACE_TEST));
    await user.click(card(RACE_TEST).getByRole("button", { name: "Stop" }));
    expect(cancelTestDiagnosis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, `app/controllers::${RACE_TEST}`);
    expect(await card(RACE_TEST).findByRole("button", { name: "Explain the failure" })).toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("alert")).not.toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("region", { name: "Why it failed" })).not.toBeInTheDocument();
  });

  it("says why it could not explain and tries again", async () => {
    vi.mocked(diagnoseTestFailure)
      .mockRejectedValueOnce(apiError("rate_limited", "Claude Code is limiting requests; try again in a moment."))
      .mockResolvedValueOnce(makeDiagnosis());
    const user = await openFailing();

    await user.click(explainButton(RACE_TEST));
    const failed = within(await card(RACE_TEST).findByRole("region", { name: "Why it failed" }));
    expect(failed.getByRole("alert")).toHaveTextContent(
      "The explanation failed: Claude Code is limiting requests; try again in a moment.",
    );
    expect(card(RACE_TEST).queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();

    await user.click(failed.getByRole("button", { name: "Try again" }));
    expect(diagnoseTestFailure).toHaveBeenCalledTimes(2);
    expect(await card(RACE_TEST).findByText("Cause")).toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("alert")).not.toBeInTheDocument();
  });

  it("explains several failures, as many at once as the model takes, the others waiting their turn", async () => {
    // The core keeps the calls and their turns: each click is sent at once, and waits there.
    const first = deferred<TestDiagnosis>();
    const second = deferred<TestDiagnosis>();
    const third = deferred<TestDiagnosis>();
    vi.mocked(diagnoseTestFailure)
      .mockReturnValueOnce(first.promise)
      .mockReturnValueOnce(second.promise)
      .mockReturnValueOnce(third.promise);
    vi.mocked(cancelTestDiagnosis).mockImplementation(async () => third.reject(apiError("cancelled", "Stopped.")));
    const user = await openFailing();

    await user.click(explainButton(RACE_TEST));
    await user.click(explainButton(ASSERTION_TEST));
    await user.click(explainButton("app/containers"));
    expect(diagnoseTestFailure).toHaveBeenCalledTimes(3);
    // Claude Code takes one call at a time.
    expect(card(RACE_TEST).getByRole("region", { name: /^Explaining the failure/ })).toBeInTheDocument();
    expect(card(ASSERTION_TEST).getByRole("region", { name: /^Waiting for another explanation to finish/ })).toBeInTheDocument();
    // One still waiting is stopped where it waits.
    await user.click(card("app/containers").getByRole("button", { name: "Stop" }));
    expect(cancelTestDiagnosis).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, "app/containers::");
    expect(await card("app/containers").findByRole("button", { name: "Explain the failure" })).toBeInTheDocument();

    first.resolve(makeDiagnosis());
    second.resolve(
      makeDiagnosis({ key: `app/services/auth_service::${ASSERTION_TEST}`, package: "app/services/auth_service", test: ASSERTION_TEST }),
    );
    expect(await card(ASSERTION_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(card(RACE_TEST).getByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(vi.mocked(diagnoseTestFailure).mock.calls[1].slice(2, 4)).toEqual(["app/services/auth_service", ASSERTION_TEST]);
    expect(cancelModelRun).not.toHaveBeenCalled();
  });

  it("runs explanations side by side when the model takes several calls at once", async () => {
    vi.mocked(getModelInUse).mockResolvedValue(
      makeModelInUse({ provider: "anthropic", providerName: "Anthropic API", model: "claude-opus-5-5", parallelCalls: 3 }),
    );
    vi.mocked(diagnoseTestFailure).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();
    await waitFor(() =>
      expect(explainButton(RACE_TEST)).toHaveAttribute(
        "title",
        "Explain the failure with Anthropic API, model claude-opus-5-5: one model call",
      ),
    );

    await user.click(explainButton(RACE_TEST));
    await user.click(explainButton(ASSERTION_TEST));
    expect(diagnoseTestFailure).toHaveBeenCalledTimes(2);
    expect(card(ASSERTION_TEST).getByRole("region", { name: /^Explaining the failure/ })).toBeInTheDocument();
  });

  it("finds an explanation still being written on coming back to the review, with Stop, then shows what it wrote", async () => {
    // Asked, then the review is left before the model answers: the call goes on in the core.
    vi.mocked(diagnoseTestFailure).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();
    await user.click(explainButton(RACE_TEST));
    expect(diagnoseTestFailure).toHaveBeenCalledOnce();
    cleanup();

    const joined = deferred<TestDiagnosis | null>();
    vi.mocked(getRunningTestDiagnoses).mockResolvedValue([
      { key: `app/controllers::${RACE_TEST}`, package: "app/controllers", test: RACE_TEST, queued: false },
      { key: "app/containers::", package: "app/containers", test: null, queued: true },
    ]);
    vi.mocked(joinTestDiagnosis).mockImplementation((_project, _review, key) =>
      key === "app/containers::" ? new Promise(() => {}) : joined.promise,
    );
    await openFailing(user);

    // Shown as being written, never offered again, and joined instead of asked again.
    const loading = card(RACE_TEST).getByRole("region", { name: "Explaining the failure with Claude Code, model opus" });
    expect(loading).toHaveAttribute("aria-busy", "true");
    expect(card(RACE_TEST).queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();
    expect(card("app/containers").getByRole("region", { name: /^Waiting for another explanation to finish/ })).toBeInTheDocument();
    expect(joinTestDiagnosis).toHaveBeenCalledWith("p-1", REVIEW, `app/controllers::${RACE_TEST}`);
    expect(explainButton(ASSERTION_TEST)).toBeInTheDocument();

    // Stop reaches the call left behind.
    await user.click(card("app/containers").getByRole("button", { name: "Stop" }));
    expect(cancelTestDiagnosis).toHaveBeenCalledWith("p-1", REVIEW, "app/containers::");

    joined.resolve(makeDiagnosis());
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(diagnoseTestFailure).toHaveBeenCalledOnce();
  });

  it("reads what was written when a call left behind ended before it could be joined", async () => {
    vi.mocked(getRunningTestDiagnoses)
      .mockResolvedValueOnce([{ key: `app/controllers::${RACE_TEST}`, package: "app/controllers", test: RACE_TEST, queued: false }])
      .mockResolvedValue([]);
    vi.mocked(joinTestDiagnosis).mockResolvedValue(null);
    vi.mocked(getTestDiagnoses).mockResolvedValueOnce([]).mockResolvedValue([makeDiagnosis()]);
    await openTests();

    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });

  it("shows again what was written, on opening the tab, and says when it is outdated", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis({ outdated: true, confidence: "low" })]);
    const user = await openFailing();

    const block = within(await card(RACE_TEST).findByRole("region", { name: "Why it failed" }));
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
    expect(block.getByText("outdated")).toBeInTheDocument();
    expect(block.getByText("unsure")).toHaveAttribute("title", "The model says it is not sure of this reading.");
    expect(
      block.getByText("The test ran again with another result, or its code changed, since this was written."),
    ).toBeInTheDocument();

    vi.mocked(diagnoseTestFailure).mockResolvedValue(makeDiagnosis());
    await user.click(block.getByRole("button", { name: "Explain again" }));
    expect(diagnoseTestFailure).toHaveBeenCalledOnce();
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).not.toHaveTextContent("outdated");
  });

  it("shows an explanation again on coming back to the review, never offering it again while it is read", async () => {
    vi.mocked(diagnoseTestFailure).mockResolvedValue(makeDiagnosis());
    const user = await openFailing();
    await user.click(explainButton(RACE_TEST));
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();

    // Leaving the review and coming back (or restarting the app): what was written is read again, which takes as
    // long as reading the last run again.
    cleanup();
    const kept = deferred<TestDiagnosis[]>();
    vi.mocked(getTestDiagnoses).mockReturnValue(kept.promise);
    await openTests(user);
    expect(screen.getByText(READING)).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();

    kept.resolve([makeDiagnosis()]);
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(screen.queryByText(READING)).not.toBeInTheDocument();
    expect(card(RACE_TEST).queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();
    // The failures never explained are offered it once the read is in.
    expect(explainButton(ASSERTION_TEST)).toBeInTheDocument();
    expect(diagnoseTestFailure).toHaveBeenCalledOnce();
  });

  it("says why the explanations already written could not be read, offering none to explain, and reads again", async () => {
    vi.mocked(getTestDiagnoses)
      .mockRejectedValueOnce(apiError("internal", "The analysis stopped unexpectedly."))
      .mockResolvedValueOnce([makeDiagnosis()]);
    const user = await openTests();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "Could not read the explanations already written: The analysis stopped unexpectedly.",
    );
    expect(screen.queryByRole("button", { name: "Explain the failure" })).not.toBeInTheDocument();

    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(getTestDiagnoses).toHaveBeenCalledTimes(2);
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
    expect(explainButton(ASSERTION_TEST)).toBeInTheDocument();
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });

  it("reads the explanations again against each run that reports", async () => {
    vi.mocked(getTestDiagnoses)
      .mockResolvedValueOnce([makeDiagnosis()])
      .mockResolvedValueOnce([makeDiagnosis({ outdated: true })]);
    vi.mocked(runTests).mockResolvedValue(makeFailingRun({ startedAt: "2026-11-21T11:00:00Z" }));
    const user = await openFailing();
    const block = () => within(card(RACE_TEST).getByRole("region", { name: "Why it failed" }));
    expect(await card(RACE_TEST).findByRole("region", { name: "Why it failed" })).toBeInTheDocument();
    expect(block().queryByText("outdated")).not.toBeInTheDocument();

    await user.click(card(RACE_TEST).getByRole("button", { name: "Run this test" }));
    expect(await block().findByText("outdated")).toBeInTheDocument();
    expect(getTestDiagnoses).toHaveBeenCalledTimes(2);
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });

  it("copies a prompt for the agent in the language shown", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis()]);
    const user = await openFailing();

    const block = within(await card(RACE_TEST).findByRole("region", { name: "Why it failed" }));
    await user.click(block.getByRole("button", { name: "Copy as a prompt for the agent" }));
    expect(await block.findByRole("button", { name: "Copied" })).toBeInTheDocument();
    const prompt = await navigator.clipboard.readText();
    expect(prompt).toContain(`Fix the failing test ${RACE_TEST} in the package app/controllers.`);
    expect(prompt).toContain("The failure (data race):\n- Write: app/services/auth_service/service.go:661 ((*Service).recordLastLogin)");
    expect(prompt).toContain("Why it fails, as a model diagnosed it (check it): `checkAsync` writes `lastLogin`");
    expect(prompt).toContain("Origin: code.");
    expect(prompt).toContain("Where to change it: app/services/auth_service/service.go:661");
    expect(prompt).toContain("Proposed fix: Hold the mutex `Service` already has while writing `lastLogin`.\n```\ns.mu.Lock()");
    expect(prompt).toContain(`Check it with:\ngo test -race -run '^${RACE_TEST}$' ./app/controllers`);

    setLanguage("pt-BR");
    // Still saying it copied, now in Portuguese.
    await user.click(await card(RACE_TEST).findByRole("button", { name: /^Copiado/ }));
    const portuguese = await navigator.clipboard.readText();
    expect(portuguese).toContain(`Corrija o teste que falha ${RACE_TEST} no pacote app/controllers.`);
    expect(portuguese).toContain("Origem: código.");
    expect(portuguese).toContain("Por que falha, segundo o diagnóstico de um modelo (confira):");
  });

  it("offers to translate an explanation written in another language, only on click", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis({ language: "pt-BR", cause: "A goroutine grava `lastLogin`." })]);
    vi.mocked(planTestDiagnosesTranslation).mockResolvedValue({
      items: 1,
      calls: [{ ids: [`app/controllers::${RACE_TEST}`], tokens: 900 }],
      tokens: 900,
      model: makeModelInUse(),
    });
    vi.mocked(translateTestDiagnoses).mockResolvedValue([
      makeDiagnosis({
        language: "pt-BR",
        cause: "A goroutine grava `lastLogin`.",
        translations: [{ language: "en", cause: "The goroutine writes `lastLogin`.", fix: "Hold the mutex." }],
      }),
    ]);
    const user = await openFailing();

    const banner = within(await screen.findByRole("region", { name: "Language of this text" }));
    expect(banner.getByText("An explanation of a failure was written in Portuguese.")).toBeInTheDocument();
    expect(planTestDiagnosesTranslation).not.toHaveBeenCalled();
    await user.click(banner.getByRole("button", { name: "Translate" }));
    expect(planTestDiagnosesTranslation).toHaveBeenCalledWith("p-1", REVIEW, "en", [`app/controllers::${RACE_TEST}`]);
    expect(await banner.findByText(/Translate sends the text of 1 explanation of a failure to Claude Code, model opus/)).toBeInTheDocument();
    expect(translateTestDiagnoses).not.toHaveBeenCalled();
    await user.click(banner.getByRole("button", { name: "Translate" }));

    expect(translateTestDiagnoses).toHaveBeenCalledWith("p-1", REVIEW, "en", [`app/controllers::${RACE_TEST}`], expect.any(String));
    expect(await card(RACE_TEST).findByText("lastLogin", { selector: "code" })).toBeInTheDocument();
    expect(card(RACE_TEST).getByText(/The goroutine writes/)).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: "Language of this text" })).not.toBeInTheDocument();
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });
});

describe("Write review comment", () => {
  const KEY = `app/controllers::${RACE_TEST}`;
  const SERVICE = "app/services/auth_service/service.go";
  const why = () => within(card(RACE_TEST).getByRole("region", { name: "Why it failed" }));
  const lineDraft = makeComment("c-1", { path: SERVICE, line: 661, currentLine: 661, body: "Hold `s.mu`." });
  const generalDraft = makeGeneralComment("g-1", { body: "`app/containers/auth_container.go:47`: pass the policy." });

  it("writes a draft from the explanation, staying on the tab, then offers to view it", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis()]);
    const written = deferred<WrittenReviewComment>();
    vi.mocked(writeReviewComment).mockReturnValue(written.promise);
    const user = await openFailing();

    // Only where an explanation is shown, beside Copy as a prompt.
    expect(card(ASSERTION_TEST).queryByRole("button", { name: "Write review comment" })).not.toBeInTheDocument();
    const write = why().getByRole("button", { name: "Write review comment" });
    expect(write).toHaveAttribute(
      "title",
      "Write a review comment for the author from this explanation, with Claude Code, model opus: one model call. It is kept as a draft in the Diff until you publish comments.",
    );
    expect(getModelInUse).toHaveBeenCalledWith("review_comment");
    await user.click(write);
    const runId = vi.mocked(writeReviewComment).mock.calls[0][3];
    expect(writeReviewComment).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, KEY, runId);
    expect(why().getByRole("status")).toHaveTextContent("Writing the review comment with Claude Code, model opus");
    expect(why().queryByRole("button", { name: "Write review comment" })).not.toBeInTheDocument();

    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [lineDraft] }));
    written.resolve({ key: KEY, commentId: "c-1", kind: "line" });
    const view = await why().findByRole("button", { name: "View comment in Diff" });
    expect(view).toHaveAttribute("title", `The draft on ${SERVICE}:661, in the Diff`);
    // The tab does not change.
    expect(reviewTab(/^Tests/)).toHaveAttribute("aria-selected", "true");
    expect(reviewTab("Diff")).toHaveAttribute("aria-selected", "false");
    expect(diagnoseTestFailure).not.toHaveBeenCalled();
  });

  const shown = () => within(screen.getByRole("group", { name: "Shown" }));

  it("views a line draft in the Diff's Files view, at its line, from whichever view was left", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis({ commentId: "c-1" })]);
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [lineDraft] }));
    const user = await openFailing();
    // The Diff was left on its Comments view.
    await user.click(reviewTab("Diff"));
    await user.click(await shown().findByRole("button", { name: /^Comments/ }));
    await user.click(reviewTab(/^Tests/));

    await user.click(await why().findByRole("button", { name: "View comment in Diff" }));
    expect(reviewTab("Diff")).toHaveAttribute("aria-selected", "true");
    expect(shown().getByRole("button", { name: "Files" })).toHaveAttribute("aria-pressed", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", SERVICE);
    expect(editor).toHaveAttribute("data-scroll-line", "661");
    expect(writeReviewComment).not.toHaveBeenCalled();
  });

  it("views a general draft in the Diff's Comments view, focused, and offers to write again once it is deleted", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis({ commentId: "g-1" })]);
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState({ comments: [generalDraft] }));
    vi.mocked(deleteComment).mockResolvedValue(undefined);
    const user = await openFailing();

    const view = await why().findByRole("button", { name: "View comment in Diff" });
    expect(view).toHaveAttribute("title", "The draft among the pull request comments, in the Diff's Comments view");
    await user.click(view);
    expect(reviewTab("Diff")).toHaveAttribute("aria-selected", "true");
    expect(shown().getByRole("button", { name: /^Comments/ })).toHaveAttribute("aria-pressed", "true");
    const block = within(screen.getByRole("region", { name: "Pull request comments" }));
    const thread = block.getByRole("group", { name: "Comment by You" });
    await waitFor(() => expect(thread).toHaveFocus());
    expect(thread).toHaveAttribute("data-focused", "true");
    expect(thread).toHaveTextContent("pass the policy.");

    // Deleted from the Diff: the explanation offers to write one again.
    vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
    await user.click(within(thread).getByRole("button", { name: "Delete" }));
    expect(deleteComment).toHaveBeenCalledExactlyOnceWith("p-1", "g-1");
    await user.click(reviewTab(/^Tests/));
    expect(await why().findByRole("button", { name: "Write review comment" })).toBeInTheDocument();
    expect(why().queryByRole("button", { name: "View comment in Diff" })).not.toBeInTheDocument();
  });

  it("says why the comment could not be written, and stops it on Stop", async () => {
    vi.mocked(getTestDiagnoses).mockResolvedValue([makeDiagnosis()]);
    const pending = deferred<WrittenReviewComment>();
    vi.mocked(writeReviewComment)
      .mockRejectedValueOnce(apiError("rate_limited", "Claude Code is limiting requests; try again in a moment."))
      .mockReturnValueOnce(pending.promise);
    vi.mocked(cancelModelRun).mockImplementation(async () => pending.reject(apiError("cancelled", "Stopped.")));
    const user = await openFailing();

    await user.click(why().getByRole("button", { name: "Write review comment" }));
    expect(await why().findByRole("alert")).toHaveTextContent(
      "The review comment could not be written: Claude Code is limiting requests; try again in a moment.",
    );
    await user.click(why().getByRole("button", { name: "Write review comment" }));
    expect(why().queryByRole("alert")).not.toBeInTheDocument();
    await user.click(why().getByRole("button", { name: "Stop" }));
    const runId = vi.mocked(writeReviewComment).mock.calls[1][3];
    expect(cancelModelRun).toHaveBeenCalledWith(runId);
    expect(await why().findByRole("button", { name: "Write review comment" })).toBeInTheDocument();
    expect(why().queryByRole("alert")).not.toBeInTheDocument();
  });
});
