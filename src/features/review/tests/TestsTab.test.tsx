import { act, render, screen, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import { PROGRESS_POLL_MS, RUN_POLL_MS } from "@/features/review/tests/use-tests";
import {
  cancelTests,
  getFileVersions,
  getQuality,
  getReviewState,
  getTestsProgress,
  getTestsState,
  listChangedFiles,
  type ReviewTarget,
  runTests,
  type TestFailure,
  type TestRun,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { answerReviewOpening, openReviewTab } from "@/test/review-screen";
import {
  FAILURE_OUTPUT,
  makeBigRun,
  makeFailingRun,
  makeQualityReport,
  makeTestCase,
  makeTestRun,
  makeTestsState,
} from "@/test/tests-fixtures";

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
const OLD = "a1b2c3d4e5f6a7b8";
const NEW = "e7f3b10c9d8e7f6a";
const WHOLE = { race: true, coverage: true };

const changeSet = makeChangeSet([
  makeChangedFile("internal/payment/repo.go"),
  makeChangedFile("internal/payment/repo_test.go"),
  makeChangedFile("internal/refund/handler.go", { status: "added" }),
  makeChangedFile("internal/refund/service.go", { status: "added" }),
  makeChangedFile("internal/refund/service_test.go", { status: "added" }),
  makeChangedFile("app/services/auth_service/service.go"),
  makeChangedFile("app/services/auth_service/password_policy.go", { status: "added" }),
  makeChangedFile("app/services/auth_service/auth_service_test.go"),
  makeChangedFile("app/controllers/edge_login_test.go", { status: "added" }),
]);

let bar: HTMLDivElement;

const tab = (name: RegExp) => within(screen.getByRole("tablist", { name: "Review views" })).getByRole("tab", { name });
const card = (name: string) => screen.getByRole("article", { name });
/** Fakes the clock, so the elapsed seconds can be moved on. */
function fakeClock() {
  vi.useFakeTimers({ toFake: ["Date", "setInterval", "clearInterval"] });
}

/**
 * Goes back to the real clock. jsdom runs animation frames on a setInterval of its own: one started while the clock
 * was fake has to run out first, or no animation frame would ever run again in this file (and Base UI menus, which
 * open on one, would never open).
 */
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
  return user;
}

async function openFailing(run: TestRun = makeFailingRun(), user = userEvent.setup()) {
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run }));
  await openTests(user);
  await screen.findByRole("heading", { level: 1 });
  return user;
}

async function openBrowser(user: ReturnType<typeof userEvent.setup>) {
  await user.click(screen.getByRole("button", { name: /^All tests/ }));
  return within(screen.getByRole("list", { name: "All tests" }));
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
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeTestRun() }));
  vi.mocked(getTestsProgress).mockResolvedValue(null);
  vi.mocked(cancelTests).mockResolvedValue(undefined);
  vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: makeQualityReport() });
});

afterEach(() => {
  bar.remove();
  realClock();
});

describe("Tests tab before a run", () => {
  it("only reads the last run, never runs anything on open, and says what running does", async () => {
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
    await openTests();

    expect(getTestsState).toHaveBeenCalledWith("p-1", OLD, NEW);
    expect(runTests).not.toHaveBeenCalled();
    expect(tab(/^Tests/)).toHaveAccessibleName("Tests");
    expect(await screen.findByRole("heading", { name: "The tests of this review have not run yet" })).toBeInTheDocument();
    expect(screen.getByText(/Running the tests executes this project's code on this machine,/)).toBeInTheDocument();
    expect(screen.getByText(/A run stops by itself after 10 minutes\./)).toBeInTheDocument();
    expect(screen.getByRole("checkbox", { name: "Race detector" })).toBeChecked();
    expect(screen.getByRole("checkbox", { name: "Measure coverage" })).toBeChecked();
  });

  it("offers the sandbox and a plain process, each saying what it means, with the project's choice preselected", async () => {
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ isolation: "process" }));
    await openTests();

    const choice = within(await screen.findByRole("group", { name: "Where the tests run" }));
    const sandbox = choice.getByRole("radio", { name: /^Sandbox/ });
    const plain = choice.getByRole("radio", { name: /^Plain process/ });
    expect(sandbox).toHaveAccessibleName(/No network\. Can write only inside its own copy of the branch\./);
    expect(plain).toHaveAccessibleName(/Runs as you, with everything you can reach\./);
    expect(plain).toBeChecked();
    expect(sandbox).not.toBeChecked();
  });

  it("runs every test where and with what was chosen", async () => {
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openTests();

    await user.click(await screen.findByRole("radio", { name: /^Plain process/ }));
    await user.click(screen.getByRole("checkbox", { name: "Race detector" }));
    await user.click(screen.getByRole("button", { name: "Run tests" }));

    expect(runTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW, "process", { race: false, coverage: true }, null);
    const status = screen.getByRole("status", { name: "Running the tests" });
    expect(within(status).getByRole("heading")).toHaveTextContent("Running every test as a plain process");
    expect(status).toHaveTextContent("0:00");
  });

  it("says calmly that the project's languages have no test runner yet", async () => {
    vi.mocked(getTestsState).mockResolvedValue(
      makeTestsState({ unavailable: { reason: "no_runner", languages: ["typescript", "python"] } }),
    );
    await openTests();

    expect(await screen.findByText("No test runner for TypeScript or Python yet.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Run tests" })).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();
  });

  it("shows a failure to read the last run and tries again", async () => {
    vi.mocked(getTestsState).mockRejectedValueOnce(apiError("unknown_project", "This project is no longer known."));
    const user = await openTests();

    expect(await screen.findByRole("alert")).toHaveTextContent("This project is no longer known.");
    await user.click(screen.getByRole("button", { name: "Try again" }));
    expect(await screen.findByRole("heading", { name: "1 test failed" })).toBeInTheDocument();
  });
});

describe("Tests tab while the whole suite runs", () => {
  it("shows the progress and the time, dims the previous results, and cancels", async () => {
    fakeClock();
    const pending = deferred<TestRun>();
    vi.mocked(runTests).mockReturnValue(pending.promise);
    vi.mocked(getTestsProgress).mockResolvedValue({
      packagesDone: 48,
      packagesTotal: 167,
      tests: 2611,
      failed: 0,
      current: "app/services/billing_service",
    });
    const user = await openFailing(makeFailingRun(), userEvent.setup({ advanceTimers: vi.advanceTimersByTime }));

    await user.click(screen.getByRole("button", { name: "Run all" }));
    expect(runTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW, "sandbox", WHOLE, null);
    const status = screen.getByRole("status", { name: "Running the tests" });
    expect(within(status).getByRole("heading")).toHaveTextContent("Running every test in the sandbox");
    expect(status).toHaveTextContent("previous run");
    expect(screen.getByTestId("test-run")).toHaveAttribute("data-faded", "true");
    for (const name of ["Run this test", "Run this package"]) {
      for (const button of screen.getAllByRole("button", { name })) {
        expect(button).toBeDisabled();
        expect(button).toHaveAttribute("title", "Wait for the current run to end");
      }
    }
    expect(screen.queryByRole("button", { name: "Run all" })).not.toBeInTheDocument();

    await act(async () => {
      vi.advanceTimersByTime(PROGRESS_POLL_MS);
    });
    expect(getTestsProgress).toHaveBeenCalledWith("p-1", NEW);
    const progress = within(status).getByRole("progressbar", { name: "Packages done" });
    expect(progress).toHaveAttribute("aria-valuenow", "48");
    expect(progress).toHaveAttribute("aria-valuemax", "167");
    expect(status).toHaveTextContent("48 of 167 packages");
    expect(status).toHaveTextContent("2611 tests so far, none failing");
    expect(status).toHaveTextContent("the last run took 2 min 5 s");
    expect(status).toHaveTextContent("now in app/services/billing_service");
    act(() => {
      vi.advanceTimersByTime(11_000);
    });
    expect(status).toHaveTextContent("0:12");

    await user.click(within(status).getByRole("button", { name: "Cancel" }));
    expect(cancelTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW);
    expect(within(status).getByRole("heading")).toHaveTextContent("Cancelling the run");

    realClock();
    pending.resolve(makeTestRun({ status: "cancelled", durationMs: 12_400, tests: [] }));
    expect(await screen.findByText("The last run was cancelled after 12 s. Nothing was reported.")).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Running the tests" })).not.toBeInTheDocument();
    expect(screen.getByRole("heading", { name: "Run the tests again" })).toBeInTheDocument();
    expect(screen.getByRole("radio", { name: /^Sandbox/ })).toBeChecked();
  });

  it("follows a run started elsewhere until it ends, without starting one", async () => {
    fakeClock();
    vi.mocked(getTestsState).mockResolvedValueOnce(makeTestsState({ running: true, isolation: "process" }));
    await openTests(userEvent.setup({ advanceTimers: vi.advanceTimersByTime }));

    const progress = await screen.findByRole("status", { name: "Running the tests" });
    expect(progress).toHaveTextContent("Running every test as a plain process");
    expect(screen.getByText("Started before this tab was opened. Its results show here when it ends.")).toBeInTheDocument();

    vi.mocked(getTestsState).mockResolvedValueOnce(makeTestsState({ running: true }));
    await act(async () => {
      vi.advanceTimersByTime(RUN_POLL_MS);
    });
    expect(screen.getByRole("status", { name: "Running the tests" })).toBeInTheDocument();

    vi.mocked(getTestsState).mockResolvedValueOnce(makeTestsState({ run: makeTestRun() }));
    await act(async () => {
      vi.advanceTimersByTime(RUN_POLL_MS);
    });

    expect(await screen.findByRole("heading", { name: "1 test failed" })).toBeInTheDocument();
    expect(screen.queryByRole("status", { name: "Running the tests" })).not.toBeInTheDocument();
    expect(runTests).not.toHaveBeenCalled();
    expect(tab(/^Tests/)).toHaveAccessibleName("Tests, 1 failing");
  });

  it("shows why a run could not start, and offers it again", async () => {
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
    vi.mocked(runTests).mockRejectedValueOnce(apiError("internal", "The project has no test runner."));
    const user = await openTests();

    await user.click(await screen.findByRole("button", { name: "Run tests" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("The project has no test runner.");
    expect(screen.getByRole("button", { name: "Run tests" })).toBeInTheDocument();
  });

  it("asks for the quality report again when a run completes", async () => {
    const user = userEvent.setup();
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    await openReviewTab(user, /^Quality/);
    await screen.findByRole("list", { name: "Functions over a limit" });
    expect(getQuality).toHaveBeenCalledTimes(1);

    vi.mocked(runTests).mockResolvedValue(makeTestRun({ startedAt: "2026-11-17T11:00:00Z" }));
    await user.click(tab(/^Tests/));
    await user.click(await screen.findByRole("button", { name: "Run all" }));

    await vi.waitFor(() => expect(getQuality).toHaveBeenCalledTimes(2));
  });
});

describe("Tests tab verdict", () => {
  it("answers at a glance what failed, with the numbers in a quiet line, and counts failures in the tab", async () => {
    await openFailing();

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("2 tests failed and 1 package did not build");
    expect(
      screen.getByText("In app/containers, app/controllers, and app/services/auth_service. Everything else passed."),
    ).toBeInTheDocument();
    const facts = within(screen.getByRole("list", { name: "Run numbers" }))
      .getAllByRole("listitem")
      .map((item) => item.textContent);
    expect(facts.slice(0, 7)).toEqual([
      "3 passing",
      "2 failing",
      "1 package did not build",
      "1 skipped",
      "1 added in this branch",
      "67% of changed lines covered",
      "2 min 5 s to run",
    ]);
    expect(facts[7]).toMatch(/, at e7f3b10$/);
    expect(facts[8]).toBe("in the sandbox, without network, with the race detector");
    // A test with failing subtests is one failure.
    expect(tab(/^Tests/)).toHaveAccessibleName("Tests, 3 failing");
  });

  it("opens the coverage of changed lines by file, each opening its diff", async () => {
    const user = await openFailing();

    const toggle = screen.getByRole("button", { name: "67% of changed lines covered" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const files = within(screen.getByRole("list", { name: "Coverage of changed lines, by file" }));
    expect(files.getAllByRole("listitem").map((item) => item.textContent)).toEqual([
      "internal/refund/handler.go82%",
      "internal/refund/service.go48%",
      "internal/payment/repo.go56%",
    ]);
    expect(files.getAllByRole("listitem")[1]).toHaveAttribute("title", "12 of 25 coverable lines run by a test");

    await user.click(files.getByRole("button", { name: "internal/refund/service.go" }));
    expect(await screen.findByTestId("diff-editor")).toHaveAttribute("data-path", "internal/refund/service.go");
  });

  it("says all passed, where, and offers no run of the failed ones", async () => {
    const passing = makeTestCase("TestValidate", { state: "untouched" });
    await openFailing(makeTestRun({ tests: [passing], coverage: null, raceDetector: false }));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("The only test passed");
    expect(screen.getByText("In 3 packages, without the race detector.")).toBeInTheDocument();
    expect(screen.getByRole("list", { name: "Run numbers" })).toHaveTextContent("— coverage not measured");
    expect(screen.queryByRole("button", { name: "Run only the failed ones" })).not.toBeInTheDocument();
    expect(screen.queryByRole("region", { name: /^Failures/ })).not.toBeInTheDocument();
    expect(tab(/^Tests/)).toHaveAccessibleName("Tests");
  });

  it("says what the last partial run covered and when", async () => {
    const now = new Date();
    const run = makeFailingRun({
      partial: {
        startedAt: new Date(now.getTime() - 3 * 60_000).toISOString(),
        durationMs: 4000,
        status: "completed",
        isolation: "sandbox",
        packages: 2,
        tests: 3,
      },
    });
    await openFailing(run);

    expect(
      screen.getByText("The last run, 3 min ago, ran only 3 tests in 2 packages. The rest is from the full run."),
    ).toBeInTheDocument();
  });

  it("runs only the failed tests and the packages that did not build", async () => {
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();

    await user.click(screen.getByRole("button", { name: "Run only the failed ones" }));

    expect(runTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW, "sandbox", WHOLE, {
      packages: [
        { package: "app/containers", tests: null },
        { package: "app/controllers", tests: [{ name: "TestSignUpFromEdge_CreatesAndLogsIn", subtests: null }] },
        {
          package: "app/services/auth_service",
          tests: [{ name: "TestAuthenticateAD_EveryRefusalIsTheGenericError", subtests: null }],
        },
      ],
    });
    // Inline, in the cards it runs; the rest waits.
    expect(within(card("TestSignUpFromEdge_CreatesAndLogsIn")).getByRole("status")).toHaveTextContent(
      "Running this test and its 2 subtests",
    );
    expect(within(card("app/containers")).getByRole("status")).toHaveTextContent("Running this package");
    expect(screen.queryByRole("status", { name: "Running the tests" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Run only the failed ones" })).toHaveAttribute(
      "title",
      "Wait for the current run to end",
    );
  });
});

describe("Tests tab run options", () => {
  it("chooses where and what the next run measures, from any button", async () => {
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();

    await user.click(screen.getByRole("button", { name: "Run options" }));
    const menu = within(await screen.findByRole("menu"));
    expect(menu.getByRole("menuitemradio", { name: /^In the sandbox\s*· last used/ })).toHaveAttribute("aria-checked", "true");
    expect(menu.getByRole("menuitemradio", { name: /^As a plain process/ })).toHaveAttribute("aria-checked", "false");
    expect(menu.getByRole("menuitemcheckbox", { name: /^Race detector/ })).toHaveAttribute("aria-checked", "true");
    expect(menu.getByText("They apply to the next run, from any button.")).toBeInTheDocument();

    await user.click(menu.getByRole("menuitemradio", { name: /^As a plain process/ }));
    await user.click(menu.getByRole("menuitemcheckbox", { name: /^Measure coverage/ }));
    expect(menu.getByRole("menuitemcheckbox", { name: /^Measure coverage/ })).toHaveAttribute("aria-checked", "false");
    await user.keyboard("{Escape}");
    await user.click(within(card("app/containers")).getByRole("button", { name: "Run this package" }));

    expect(runTests).toHaveBeenCalledExactlyOnceWith(
      "p-1",
      OLD,
      NEW,
      "process",
      { race: true, coverage: false },
      { packages: [{ package: "app/containers", tests: null }] },
    );
  });
});

describe("Tests tab failure cards", () => {
  it("summarises a data race: who wrote, who read, where their goroutines came from", async () => {
    const user = await openFailing();

    const race = within(card("TestSignUpFromEdge_CreatesAndLogsIn"));
    expect(race.getByText("data race")).toBeInTheDocument();
    expect(race.getByText("Two goroutines accessed the same memory without synchronisation")).toBeInTheDocument();
    const rows = race.getAllByRole("listitem").filter((row) => row.getAttribute("role") === "listitem" && !row.closest("ul"));
    expect(rows.map((row) => row.textContent)).toEqual([
      "Writeservice.go:661in (*Service).recordLastLogin, by a background goroutine created at password_policy.go:90",
      "Readedge_login_test.go:437by the test itself, before the write",
    ]);
    // The subtests, with their own status and time.
    const subtests = within(race.getByRole("list", { name: "Subtests" })).getAllByRole("listitem");
    expect(subtests.map((row) => row.textContent)).toEqual(["v1, the country of the edgeRun920 ms", "v2Run880 ms"]);
    expect(race.getByText("new")).toBeInTheDocument();
    expect(race.getByText("app/controllers")).toBeInTheDocument();

    await user.click(race.getByRole("button", { name: "service.go:661" }));
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "app/services/auth_service/service.go");
    expect(editor).toHaveAttribute("data-scroll-line", "661");
  });

  it("shows what an assertion expected and what came, and the full output on demand", async () => {
    const user = await openFailing();

    const assertion = within(card("TestAuthenticateAD_EveryRefusalIsTheGenericError"));
    expect(assertion.getByText("assertion")).toBeInTheDocument();
    expect(assertion.getByText("changed")).toBeInTheDocument();
    expect(assertion.getByText("Not equal")).toBeInTheDocument();
    expect(assertion.getByText(/in AD refuses,/)).toBeInTheDocument();
    expect(assertion.getAllByRole("term").map((term) => term.textContent)).toEqual(["Expected", "Received"]);
    expect(assertion.getByText('"credenciais inválidas"')).toHaveClass("text-add");
    expect(assertion.getByText('"ldap: invalid credentials (49)"')).toHaveClass("text-del");
    const subtests = within(assertion.getByRole("list", { name: "Subtests" })).getAllByRole("img");
    expect(subtests.map((mark) => mark.getAttribute("aria-label"))).toEqual(["failed", "passed", "passed"]);

    const toggle = assertion.getByRole("button", { name: "Show full output" });
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    await user.click(toggle);
    expect(assertion.getByRole("button", { name: "Hide full output" })).toHaveAttribute("aria-expanded", "true");
    expect(assertion.getByLabelText("Test output")).toHaveTextContent("Error: Not equal");
  });

  it("shows a package that did not build, with its errors, and why it cannot open in the Diff", async () => {
    await openFailing();

    const build = within(card("app/containers"));
    expect(build.getByText("did not build")).toBeInTheDocument();
    expect(build.getByText("Whole package: none of its tests ran")).toBeInTheDocument();
    expect(build.getAllByText("auth_container.go:47")[0]).toHaveAttribute("title", expect.stringMatching(/not changed in this review/));
    expect(build.getByText(/not enough arguments in call to auth_service\.NewService/)).toBeInTheDocument();
    expect(build.getByText("want (*sql.DB, *ldap.Client, auth_service.PasswordPolicy)")).toBeInTheDocument();
    expect(build.getByRole("button", { name: "Open in Diff" })).toBeDisabled();
    expect(build.getByRole("button", { name: "Open in Diff" })).toHaveAccessibleDescription(/not changed in this review/);
    expect(build.getByRole("button", { name: "Run this package" })).toHaveAttribute("title", "go test -race ./app/containers");
  });

  it("reads a panic, a timeout and an unknown failure", async () => {
    const failures: TestFailure[] = [
      {
        kind: "panic",
        message: "runtime error: index out of range [3] with length 3",
        frame: { function: "Validate", file: "internal/refund/validate.go", line: 30, test: false },
        test: "TestPanics",
      },
      {
        kind: "timeout",
        message: "test timed out after 10m0s",
        frame: { function: "TestHangs.func1", file: "internal/refund/service_test.go", line: 9, test: true },
        running: ["TestHangs"],
      },
      { kind: "other", lines: ["something odd", "it broke"] },
    ];
    const tests = ["TestPanics", "TestHangs", "TestOdd"].map((name, n) =>
      makeTestCase(name, { outcome: "fail", output: "boom", failure: failures[n] }),
    );
    await openFailing(makeTestRun({ tests }));

    const panic = within(card("TestPanics"));
    expect(panic.getByText("panic")).toBeInTheDocument();
    expect(panic.getByText("panic: runtime error: index out of range [3] with length 3")).toBeInTheDocument();
    expect(panic.getByText("Validate")).toBeInTheDocument();
    const timeout = within(card("TestHangs"));
    expect(timeout.getByText("timed out")).toBeInTheDocument();
    expect(timeout.getByText(/Still running:/)).toHaveTextContent("Still running: TestHangs");
    expect(timeout.getByRole("button", { name: "service_test.go:9" })).toBeInTheDocument();
    const odd = within(card("TestOdd"));
    expect(odd.getByText("failed")).toBeInTheDocument();
    expect(odd.getByText(/something odd\s+it broke/)).toBeInTheDocument();
  });

  it("opens a failing test in the Diff, and runs it or one of its subtests", async () => {
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeTestRun() }));
    const user = await openTests();

    const refund = within(await screen.findByRole("article", { name: "TestService_Request" }));
    expect(refund.getByRole("button", { name: "Run this test" })).toHaveAttribute(
      "title",
      "go test -race -run '^TestService_Request$' ./internal/refund",
    );
    const subtest = within(refund.getByRole("list", { name: "Subtests" })).getAllByRole("listitem")[0];
    expect(subtest).toHaveTextContent("gateway declines");
    expect(within(subtest).getByRole("button", { name: "Run" })).toHaveAttribute(
      "title",
      "go test -race -run '^TestService_Request$/^gateway_declines$' ./internal/refund",
    );
    await user.click(refund.getByRole("button", { name: "Show full output" }));
    expect(refund.getByLabelText("Test output").textContent).toBe(FAILURE_OUTPUT.replace("\n", ""));

    await user.click(within(subtest).getByRole("button", { name: "Run" }));
    expect(runTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW, "sandbox", WHOLE, {
      packages: [{ package: "internal/refund", tests: [{ name: "TestService_Request", subtests: ["gateway_declines"] }] }],
    });
    expect(within(subtest).queryByRole("img", { name: "failed" })).not.toBeInTheDocument();
    expect(refund.getByRole("status")).toHaveTextContent("Running 1 subtest of this test");
  });

  it("says the commands a Rust run reported, and a usual one where it reported none", async () => {
    const rust = (name: string, overrides: Parameters<typeof makeTestCase>[1]) =>
      makeTestCase(name, { container: "shapes", hierarchy: [name], file: "src/lib.rs", line: 5, ...overrides });
    vi.mocked(getTestsState).mockResolvedValue(
      makeTestsState({
        run: makeTestRun({
          language: "rust",
          tests: [
            rust("src/lib.rs - add (line 5)", {
              outcome: "fail",
              output: "assertion failed",
              command: "cargo test -p shapes --doc -- 'src/lib.rs - add (line 5)' --exact",
            }),
            rust("tests::area", { outcome: "fail", output: "assertion failed" }),
          ],
          summary: { passed: 0, failed: 2, skipped: 0, added: 2 },
          packages: [
            { container: "shapes", outcome: "fail", durationMs: 10, output: null, failure: null, command: null },
          ],
        }),
      }),
    );
    await openTests();

    // A doc test's name holds a "/" and is still one test, not a test with a subtest.
    const doc = within(await screen.findByRole("article", { name: "src/lib.rs - add (line 5)" }));
    expect(doc.queryByRole("list", { name: "Subtests" })).not.toBeInTheDocument();
    expect(doc.getByRole("button", { name: "Run this test" })).toHaveAttribute(
      "title",
      "cargo test -p shapes --doc -- 'src/lib.rs - add (line 5)' --exact",
    );
    expect(within(card("tests::area")).getByRole("button", { name: "Run this test" })).toHaveAttribute(
      "title",
      "cargo test -p shapes 'tests::area' -- --exact",
    );
  });

  it("opens the test in the Diff tab at its line", async () => {
    const user = await openFailing();

    await user.click(within(card("TestAuthenticateAD_EveryRefusalIsTheGenericError")).getByRole("button", { name: "Open in Diff" }));

    expect(tab(/^Diff/)).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "app/services/auth_service/auth_service_test.go");
    expect(editor).toHaveAttribute("data-scroll-line", "196");
  });
});

describe("Tests tab, all the tests", () => {
  it("is one line until opened, then lists packages, failures first, their tests only once opened", async () => {
    const user = await openFailing();

    const toggle = screen.getByRole("button", { name: /^All tests/ });
    expect(toggle).toHaveTextContent("All tests6 tests in 4 packages, 3 failures");
    expect(toggle).toHaveAttribute("aria-expanded", "false");
    expect(screen.queryByRole("list", { name: "All tests" })).not.toBeInTheDocument();

    const tree = await openBrowser(user);
    expect(toggle).toHaveAttribute("aria-expanded", "true");
    const rows = tree.getAllByRole("button", { name: /^Open / });
    expect(rows.map((row) => row.getAttribute("aria-label"))).toEqual([
      "Open app/containers",
      "Open app/controllers",
      "Open app/services/auth_service",
      "Open app/billing",
    ]);
    expect(tree.queryByText("TestLogin")).not.toBeInTheDocument();
    expect(tree.getAllByText("1 failing")[0].parentElement).toHaveTextContent("1 failing, 1 passing");
    expect(tree.getByText("1 new")).toBeInTheDocument();

    await user.click(tree.getByRole("button", { name: "Open app/controllers" }));
    expect(tree.getByRole("button", { name: "Close app/controllers" })).toHaveAttribute("aria-expanded", "true");
    expect(tree.getByText("TestLogin")).toBeInTheDocument();
    expect(tree.getByText("2 of 2 failing")).toBeInTheDocument();
    // A failing test opens its subtests.
    expect(tree.getByRole("button", { name: "Close the subtests" })).toBeInTheDocument();
    expect(tree.getByText("v1, the country of the edge")).toBeInTheDocument();

    await user.click(tree.getByRole("button", { name: "Open app/containers" }));
    expect(tree.getByText("No test ran: the package did not build.")).toBeInTheDocument();
  });

  it("searches and filters, keeping the selection", async () => {
    const user = await openFailing();
    const tree = await openBrowser(user);
    const chips = within(screen.getByRole("group", { name: "Filters" }));
    expect(chips.getAllByRole("button").map((chip) => chip.textContent)).toEqual([
      "Failing3",
      "Passing3",
      "Skipped1",
      "This branch2",
    ]);

    await user.click(
      within(card("TestSignUpFromEdge_CreatesAndLogsIn")).getByRole("checkbox", {
        name: "Select TestSignUpFromEdge_CreatesAndLogsIn",
      }),
    );
    await user.click(chips.getByRole("button", { name: /^Skipped/ }));
    expect(chips.getByRole("button", { name: /^Skipped/ })).toHaveAttribute("aria-pressed", "true");
    expect(tree.getAllByRole("button", { name: /^Open / }).map((row) => row.getAttribute("aria-label"))).toEqual([
      "Open app/services/auth_service",
    ]);
    await user.click(chips.getByRole("button", { name: /^Skipped/ }));

    await user.type(screen.getByRole("searchbox", { name: "Search test or package" }), "refresh");
    // A search that finds few packages opens them.
    expect(tree.getByText("TestRefreshToken_RotatesOnUse")).toBeInTheDocument();
    expect(tree.queryByText("TestAuthenticateAD_EveryRefusalIsTheGenericError")).not.toBeInTheDocument();
    expect(tree.queryByRole("button", { name: /app\/controllers/ })).not.toBeInTheDocument();

    await user.clear(screen.getByRole("searchbox", { name: "Search test or package" }));
    await user.type(screen.getByRole("searchbox", { name: "Search test or package" }), "nothing like it");
    expect(screen.getByText("No test matches.")).toBeInTheDocument();
    expect(screen.getByRole("region", { name: "Selected tests" })).toHaveTextContent("1 test selected");
  });

  it("stays light with 9,000 tests in 170 packages: only opened packages render their tests, a page at a time", async () => {
    const user = await openFailing(makeBigRun(170, 53));

    expect(screen.getByRole("heading", { level: 1 })).toHaveTextContent("All 9010 tests passed");
    const tree = await openBrowser(user);
    expect(tree.getAllByRole("button", { name: /^Open / })).toHaveLength(60);
    expect(tree.queryAllByRole("checkbox", { name: /^Select Test/ })).toHaveLength(0);
    await user.click(tree.getByRole("button", { name: "Show the other 110 packages" }));
    expect(tree.getAllByRole("button", { name: /^Open / })).toHaveLength(170);

    await user.click(tree.getByRole("button", { name: "Open pkg/p000" }));
    expect(tree.getAllByRole("checkbox", { name: /^Select Test/ })).toHaveLength(50);
    await user.click(tree.getByRole("button", { name: "Show the other 3 tests, all passing" }));
    expect(tree.getAllByRole("checkbox", { name: /^Select Test/ })).toHaveLength(53);
  });
});

describe("Tests tab selection", () => {
  it("keeps cards and tree in sync, with mixed parents, and counts by the rule", async () => {
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();
    const tree = await openBrowser(user);
    await user.click(tree.getByRole("button", { name: "Open app/services/auth_service" }));
    const auth = within(card("TestAuthenticateAD_EveryRefusalIsTheGenericError"));

    // One subtest, from the card: its test and package are mixed, in the card and in the tree.
    await user.click(auth.getByRole("checkbox", { name: "Select the subtest AD accepts" }));
    expect(auth.getByRole("checkbox", { name: "Select TestAuthenticateAD_EveryRefusalIsTheGenericError" })).toBePartiallyChecked();
    expect(tree.getByRole("checkbox", { name: "Select TestAuthenticateAD_EveryRefusalIsTheGenericError" })).toBePartiallyChecked();
    expect(tree.getByRole("checkbox", { name: "Select the package app/services/auth_service" })).toBePartiallyChecked();
    expect(tree.getAllByRole("checkbox", { name: "Select the subtest AD accepts" })[0]).toBeChecked();
    const selectionBar = screen.getByRole("region", { name: "Selected tests" });
    expect(selectionBar).toHaveTextContent("1 subtest selected");

    // The whole test, from the tree: it counts once.
    await user.click(tree.getByRole("checkbox", { name: "Select TestAuthenticateAD_EveryRefusalIsTheGenericError" }));
    expect(auth.getByRole("checkbox", { name: "Select TestAuthenticateAD_EveryRefusalIsTheGenericError" })).toBeChecked();
    expect(selectionBar).toHaveTextContent("1 test selected");

    // A package that did not build counts as a package; another test adds one.
    await user.click(within(card("app/containers")).getByRole("checkbox", { name: "Select the package app/containers" }));
    await user.click(tree.getByRole("checkbox", { name: "Select TestRefreshToken_RotatesOnUse" }));
    expect(selectionBar).toHaveTextContent("2 tests and 1 package selected");
    const run = within(selectionBar).getByRole("button", { name: "Run selected" });
    expect(run).toHaveAttribute(
      "title",
      [
        "Commands that run them again:",
        "go test -race ./app/containers",
        "go test -race -run '^(TestAuthenticateAD_EveryRefusalIsTheGenericError|TestRefreshToken_RotatesOnUse)$' ./app/services/auth_service",
      ].join("\n"),
    );

    await user.click(run);
    expect(runTests).toHaveBeenCalledExactlyOnceWith("p-1", OLD, NEW, "sandbox", WHOLE, {
      packages: [
        { package: "app/containers", tests: null },
        {
          package: "app/services/auth_service",
          tests: [
            { name: "TestAuthenticateAD_EveryRefusalIsTheGenericError", subtests: null },
            { name: "TestRefreshToken_RotatesOnUse", subtests: null },
          ],
        },
      ],
    });
    expect(selectionBar).toHaveTextContent("Running 2 tests and 1 package…");
    expect(within(selectionBar).getByRole("button", { name: "Cancel" })).toHaveFocus();
    expect(auth.getByRole("checkbox", { name: "Select TestAuthenticateAD_EveryRefusalIsTheGenericError" })).toBeDisabled();
  });

  it("selects what failed, runs a test with only some subtests on its own, and clears", async () => {
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openFailing();

    await user.click(screen.getByRole("button", { name: "Select all that failed" }));

    const selectionBar = screen.getByRole("region", { name: "Selected tests" });
    // The race test has both subtests failing, so it counts whole; of the assertion test only AD refuses failed.
    expect(selectionBar).toHaveTextContent("1 test, 1 subtest, and 1 package selected");
    expect(within(card("TestAuthenticateAD_EveryRefusalIsTheGenericError")).getByRole("checkbox", { name: "Select the subtest AD accepts" })).not.toBeChecked();
    expect(within(selectionBar).getByRole("button", { name: "Run selected" })).toHaveAttribute(
      "title",
      expect.stringContaining(
        "go test -race -run '^TestAuthenticateAD_EveryRefusalIsTheGenericError$/^AD_refuses$' ./app/services/auth_service",
      ),
    );

    await user.click(within(selectionBar).getByRole("button", { name: "Clear selection" }));
    expect(screen.queryByRole("region", { name: "Selected tests" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Select all that failed" })).toHaveFocus();
  });

  it("puts the focus back on the last box changed when the selection is cleared", async () => {
    const user = await openFailing();
    const box = within(card("TestSignUpFromEdge_CreatesAndLogsIn")).getByRole("checkbox", {
      name: "Select the subtest v2",
    });

    await user.click(box);
    await user.click(within(screen.getByRole("region", { name: "Selected tests" })).getByRole("button", { name: "Clear selection" }));

    expect(box).toHaveFocus();
    expect(box).not.toBeChecked();
  });
});

describe("Tests tab after a run that did not report", () => {
  it("shows a run the runner could not report, with its output and a way to run again", async () => {
    const run = makeTestRun({
      status: "failed",
      message: "go: cannot find main module",
      tests: [],
      summary: { passed: 0, failed: 0, skipped: 0, added: 0 },
    });
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run, isolation: "process" }));
    vi.mocked(runTests).mockReturnValue(new Promise(() => {}));
    const user = await openTests();

    expect(await screen.findByRole("alert")).toHaveTextContent("The test runner stopped without reporting.");
    expect(screen.getByLabelText("Runner output")).toHaveTextContent("go: cannot find main module");
    expect(screen.getByRole("radio", { name: /^Plain process/ })).toBeChecked();
    expect(tab(/^Tests/)).toHaveAccessibleName("Tests");

    await user.click(screen.getByRole("button", { name: "Run tests" }));
    expect(runTests).toHaveBeenCalledWith("p-1", OLD, NEW, "process", WHOLE, null);
  });

  it("says when a run hit the time limit", async () => {
    const run = makeTestRun({ status: "timed_out", durationMs: 600_000, tests: [] });
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run }));
    await openTests();

    expect(await screen.findByRole("alert")).toHaveTextContent(
      "The tests were stopped after 10 min, the time limit. Nothing was reported.",
    );
    expect(screen.getByRole("button", { name: "Run tests" })).toBeInTheDocument();
  });

  it("shows the runner's problems folded away", async () => {
    const user = await openFailing(makeFailingRun({ diagnostics: ["go: downloading failed, used the module cache"] }));

    await user.click(screen.getByText("1 problem while running the tests"));
    expect(screen.getByText(/used the module cache/)).toBeVisible();
  });
});
