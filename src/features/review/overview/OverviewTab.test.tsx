import { render, screen, waitFor, within } from "@testing-library/react";
import userEvent from "@testing-library/user-event";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import type { DiffEditorProps } from "@/features/review/DiffEditor";
import { ReviewScreen } from "@/features/review/ReviewScreen";
import {
  cancelModelRun,
  findCard,
  getFileVersions,
  getFlow,
  getInsights,
  getMutationState,
  getOverview,
  getQuality,
  getReviewState,
  getTestsState,
  linkPullRequest,
  listChangedFiles,
  type Overview,
  type OverviewSummary,
  planOverview,
  type ReviewTarget,
  summarizeOverview,
} from "@/lib/api";
import { apiError, deferred, makeChangedFile, makeChangeSet, textSide } from "@/test/fixtures";
import { makeFlowGraph, makeFlowNode } from "@/test/flow-fixtures";
import { makeFinding, makeInsights, makeModelInUse, NO_INSIGHTS } from "@/test/insights-fixtures";
import { makeCard } from "@/test/jira-fixtures";
import { makeOverviewPlan } from "@/test/overview-fixtures";
import { makeLinked } from "@/test/pull-request-fixtures";
import { makeReviewState } from "@/test/review-fixtures";
import { changedFilesRead } from "@/test/review-screen";
import {
  makeDuplicate,
  makeLocatedCheck,
  makeMutationState,
  makeQualityReport,
  makeTestRun,
  makeTestsState,
} from "@/test/tests-fixtures";

vi.mock("@/lib/api", async (importOriginal) => ({
  ...(await importOriginal<typeof import("@/lib/api")>()),
  listChangedFiles: vi.fn(),
  getFileVersions: vi.fn(),
  getTestsState: vi.fn(),
  getMutationState: vi.fn(),
  getReviewState: vi.fn(),
  linkPullRequest: vi.fn(),
  getInsights: vi.fn(),
  getFlow: vi.fn(),
  getQuality: vi.fn(),
  findCard: vi.fn(),
  getOverview: vi.fn(),
  planOverview: vi.fn(),
  summarizeOverview: vi.fn(),
  cancelModelRun: vi.fn(),
}));

vi.mock("@tauri-apps/plugin-opener", () => ({ openUrl: vi.fn(() => Promise.resolve()) }));

vi.mock("@/features/review/DiffEditor", () => ({
  default: (props: DiffEditorProps) => (
    <div data-testid="diff-editor" data-path={props.path} data-scroll-line={props.scrollTo?.line ?? ""} />
  ),
}));

const BRANCH: ReviewTarget = { kind: "branch", branch: "feat/PAY-482-refunds", base: "main" };

const changeSet = makeChangeSet(
  [
    makeChangedFile("internal/refund/service.go", { status: "added" }),
    makeChangedFile("internal/refund/validate.go", { status: "added" }),
    makeChangedFile("internal/payment/repo.go"),
  ],
  { target: BRANCH },
);

const REVIEW = { target: BRANCH, oldSha: changeSet.oldSha, newSha: changeSet.newSha };

const NO_SUMMARY: Overview = {
  summary: null,
  shape: {
    filesAdded: 2,
    filesDeleted: 0,
    directoriesNew: ["internal/refund"],
    directoriesModified: ["internal/payment"],
    manifests: [],
    migrations: [{ path: "migrations/0003_refunds.sql", status: "added" }],
  },
  leftOut: { skipped: [{ path: "assets/logo.png", reason: "binary" }], truncated: [] },
};

function makeSummary(overrides: Partial<OverviewSummary> = {}): OverviewSummary {
  return {
    headSha: changeSet.newSha,
    provider: "claude_cli",
    providerName: "Claude Code",
    model: "claude-opus-5-5",
    createdAt: "2026-11-19T10:00:00Z",
    condensed: false,
    outdated: false,
    cardKey: "PAY-482",
    cardComments: 1,
    language: "en",
    translations: [],
    title: "Adds the refund flow",
    lead: "Support can refund a settled payment, in full or in part.",
    points: [
      "A new `POST /refunds` endpoint takes the payment and the amount.",
      "The request goes through `refund.Validate`, then the gateway pays it back.",
      "A payment that is not settled is refused.",
    ],
    attention: [],
    cardSummary: "Support wants to refund a settled payment, in full or in part.",
    criteria: [
      {
        criterion: "A payment that is not settled cannot be refunded",
        verdict: "met",
        reasoning: "Validate returns ErrNotSettled.",
        path: "internal/refund/validate.go",
        line: 13,
        located: true,
      },
      {
        criterion: "A partial refund is allowed up to the remaining balance",
        verdict: "partly_met",
        reasoning: "The balance check exists, but no test goes over the limit.",
        path: "internal/other.go",
        line: 3,
        located: false,
      },
      {
        criterion: "Finance is notified of each refund",
        verdict: "no_evidence",
        reasoning: "No event or outbound call in the changed files.",
        path: null,
        line: null,
        located: false,
      },
    ],
    ...overrides,
  };
}

let bar: HTMLDivElement;
const onOpenPreferences = vi.fn();

const tabs = () => within(screen.getByRole("tablist", { name: "Review views" }));
const section = (name: string) => within(screen.getByRole("region", { name: new RegExp(`^${name}`) }));

/** Opens a review, which shows the Overview first, and waits for its changed files. */
async function openOverview(user = userEvent.setup()) {
  render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} onOpenPreferences={onOpenPreferences} />);
  await changedFilesRead();
  return user;
}

beforeEach(() => {
  vi.resetAllMocks();
  bar = document.createElement("div");
  document.body.append(bar);
  vi.mocked(listChangedFiles).mockResolvedValue(changeSet);
  vi.mocked(getFileVersions).mockResolvedValue({ old: textSide("a\n"), new: textSide("b\n") });
  vi.mocked(getTestsState).mockResolvedValue(makeTestsState());
  vi.mocked(getMutationState).mockResolvedValue(makeMutationState());
  vi.mocked(getReviewState).mockResolvedValue(makeReviewState());
  vi.mocked(linkPullRequest).mockResolvedValue({ kind: "no_remote" });
  vi.mocked(getInsights).mockResolvedValue(NO_INSIGHTS);
  vi.mocked(getFlow).mockResolvedValue({
    kind: "graph",
    graph: makeFlowGraph(
      [
        makeFlowNode("refund.Handler", {
          state: "new",
          file: "internal/refund/handler.go",
          entryPoints: [{ kind: "http_route", label: "POST /refunds" }],
        }),
      ],
      [],
    ),
  });
  vi.mocked(getQuality).mockResolvedValue({ kind: "report", report: makeQualityReport() });
  vi.mocked(findCard).mockResolvedValue({ kind: "found", card: makeCard() });
  vi.mocked(getOverview).mockResolvedValue(NO_SUMMARY);
  vi.mocked(planOverview).mockResolvedValue(makeOverviewPlan());
  vi.mocked(cancelModelRun).mockResolvedValue(undefined);
});

afterEach(() => {
  bar.remove();
});

describe("the Overview tab", () => {
  it("shows the card in the top bar as soon as the review opens, read from the branch's key", async () => {
    render(<ReviewScreen projectId="p-1" target={BRANCH} barSlot={bar} />);
    const chip = await within(bar).findByRole("link", { name: "PAY-482" });
    expect(chip).toHaveAttribute("href", "https://acme.atlassian.net/browse/PAY-482");
    // The key is in the branch: no need to wait for the pull request's title.
    expect(findCard).toHaveBeenCalledExactlyOnceWith(BRANCH, null);
  });

  it("is the tab a review opens on, and reads the overview as it opens without asking a model", async () => {
    await openOverview();

    expect(tabs().getByRole("tab", { name: "Overview" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("heading", { level: 1, name: "What this change does" })).toBeVisible();
    expect(getOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);
    await waitFor(() => expect(planOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null));
    // What the Overview shows from the flow and the quality report is measured with it.
    expect(getFlow).toHaveBeenCalledOnce();
    expect(getQuality).toHaveBeenCalledOnce();
    expect(summarizeOverview).not.toHaveBeenCalled();
  });

  it("opens without asking a model: the card, the cost of Summarize, and everything measured", async () => {
    await openOverview();

    expect(await screen.findByRole("heading", { level: 1, name: "What this change does" })).toBeInTheDocument();
    expect(await screen.findByLabelText("What Summarize sends")).toHaveTextContent(
      "3 files and the card PAY-482 with 1 comment in one call, about 12k tokens in, read by Claude Code, model opus.",
    );
    expect(screen.getByRole("button", { name: "Summarize" })).toBeInTheDocument();
    expect(summarizeOverview).not.toHaveBeenCalled();
    expect(getOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW);
    expect(planOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null);

    const card = section("What the card asks");
    expect(card.getByRole("heading", { level: 3, name: "Allow refunds for settled payments" })).toBeInTheDocument();
    expect(card.getByText("Story · In Progress")).toBeInTheDocument();
    // The card's own text, as plain text.
    expect(card.getByText(/Support wants to refund a settled payment\./)).toBeInTheDocument();
    expect(section("Acceptance criteria").getByText(/Summarize to have the model find/)).toBeInTheDocument();

    const look = within(await screen.findByRole("list", { name: "Where to look first" }));
    expect(look.getAllByRole("listitem").map((item) => item.querySelector("strong")?.textContent)).toEqual([
      "Validate is over the complexity limit",
      "Service.Request is complex and not well tested",
    ]);
    expect(section("Left out of the analysis").getByText("assets/logo.png")).toBeInTheDocument();

    const measured = section("Measured in this branch");
    expect(measured.getByText("Tests have not run on this commit.")).toBeInTheDocument();
    // Any of the three limits now, the coverage of a function's changed lines included.
    expect(measured.getByText("Functions over a limit")).toBeInTheDocument();
    const shape = section("Shape of the change");
    expect(shape.getByText("Entry points").nextSibling).toHaveTextContent("1 new");
    expect(shape.getByText("Directories").nextSibling).toHaveTextContent("1 new, 1 modified");
    expect(shape.getByText("Database").nextSibling).toHaveTextContent("1 new migration");
    // No earlier review.
    expect(screen.queryByRole("region", { name: "Since your last review" })).not.toBeInTheDocument();
  });

  it("summarizes on click, then shows the title, the card's reading and each criterion's verdict", async () => {
    const user = await openOverview();
    const answer = deferred<Overview>();
    vi.mocked(summarizeOverview).mockReturnValueOnce(answer.promise);

    await user.click(await screen.findByRole("button", { name: "Summarize" }));
    expect(summarizeOverview).toHaveBeenCalledExactlyOnceWith("p-1", REVIEW, null, expect.stringMatching(/^overview-/));
    expect(await screen.findByText("Asking Claude Code, model opus")).toBeInTheDocument();
    answer.resolve({ ...NO_SUMMARY, summary: makeSummary() });

    expect(await screen.findByRole("heading", { level: 1, name: "Adds the refund flow" })).toBeInTheDocument();
    expect(screen.getAllByText("POST /refunds")[0].tagName).toBe("CODE");
    const card = section("What the card asks");
    expect(card.getByText("Support wants to refund a settled payment, in full or in part.")).toBeInTheDocument();
    expect(card.getByText(/Summarised from the card description and 1 comment\./)).toBeInTheDocument();

    const criteria = section("Acceptance criteria");
    expect(criteria.getByText("1 met, 1 partly met, 1 without evidence")).toBeInTheDocument();
    expect(
      criteria.getByText(/This shows it is absent from the diff, not that it is missing everywhere\./),
    ).toBeInTheDocument();
    // A place outside the change is shown without a link.
    expect(criteria.getByText("internal/other.go:3").tagName).toBe("CODE");

    await user.click(criteria.getByRole("button", { name: "internal/refund/validate.go:13" }));
    expect(tabs().getByRole("tab", { name: "Diff" })).toHaveAttribute("aria-selected", "true");
    const editor = await screen.findByTestId("diff-editor");
    expect(editor).toHaveAttribute("data-path", "internal/refund/validate.go");
    expect(editor).toHaveAttribute("data-scroll-line", "13");
  });

  it("shows the lead as a paragraph, how the change works as a list, and what needs attention apart", async () => {
    vi.mocked(getOverview).mockResolvedValue({
      ...NO_SUMMARY,
      summary: makeSummary({
        attention: ["The diff also carries the payouts work from `PAY-470`, which `main` does not have yet."],
      }),
    });
    await openOverview();

    const lead = await screen.findByText("Support can refund a settled payment, in full or in part.");
    expect(lead.closest("p")).not.toBeNull();
    const points = within(screen.getByRole("list", { name: "How the change works" })).getAllByRole("listitem");
    expect(points.map((point) => point.textContent)).toEqual([
      "A new POST /refunds endpoint takes the payment and the amount.",
      "The request goes through refund.Validate, then the gateway pays it back.",
      "A payment that is not settled is refused.",
    ]);
    expect(within(points[1]).getByText("refund.Validate").tagName).toBe("CODE");
    const attention = within(screen.getByRole("list", { name: "Before you read" })).getAllByRole("listitem");
    expect(attention).toHaveLength(1);
    expect(within(attention[0]).getByText("PAY-470").tagName).toBe("CODE");
  });

  it("shows a summary written before points as its one paragraph, with no list", async () => {
    vi.mocked(getOverview).mockResolvedValue({
      ...NO_SUMMARY,
      summary: makeSummary({
        lead: "A new `POST /refunds` endpoint backed by a new `refund` package, which validates and pays refunds.",
        points: [],
        attention: [],
      }),
    });
    await openOverview();

    expect(await screen.findByRole("heading", { level: 1, name: "Adds the refund flow" })).toBeInTheDocument();
    expect(screen.getByText("POST /refunds").closest("p")).toHaveTextContent(
      "A new POST /refunds endpoint backed by a new refund package, which validates and pays refunds.",
    );
    expect(screen.queryByRole("list", { name: "How the change works" })).not.toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Before you read" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Summarize again" })).toBeInTheDocument();
  });

  it("stops a summary on Cancel and says why it failed", async () => {
    const user = await openOverview();
    const answer = deferred<Overview>();
    vi.mocked(summarizeOverview).mockReturnValueOnce(answer.promise);
    await user.click(await screen.findByRole("button", { name: "Summarize" }));
    await user.click(screen.getByRole("button", { name: "Cancel" }));
    expect(cancelModelRun).toHaveBeenCalledWith(expect.stringMatching(/^overview-/));
    answer.reject(apiError("cancelled", "The model run was stopped."));
    expect(await screen.findByRole("alert")).toHaveTextContent("The model run was stopped.");
    expect(screen.getByRole("button", { name: "Summarize" })).toBeInTheDocument();
  });

  it("shows a summary of an older head as written for it, and summarizes again only on click", async () => {
    vi.mocked(getOverview).mockResolvedValue({
      ...NO_SUMMARY,
      summary: makeSummary({ outdated: true, headSha: "0ld0ld0ld0ld0ld0" }),
    });
    vi.mocked(summarizeOverview).mockResolvedValue({ ...NO_SUMMARY, summary: makeSummary({ title: "Adds refunds" }) });
    const user = await openOverview();

    expect(await screen.findByText(/the branch has moved since/)).toHaveTextContent("Written for 0ld0ld0");
    await user.click(screen.getByRole("button", { name: "Summarize again" }));
    expect(summarizeOverview).not.toHaveBeenCalled();
    await user.click(await screen.findByRole("button", { name: "Summarize again" }));
    expect(summarizeOverview).toHaveBeenCalledOnce();
    expect(await screen.findByRole("heading", { level: 1, name: "Adds refunds" })).toBeInTheDocument();
  });

  it("without a model says so, with a link to Preferences, and still shows what was measured", async () => {
    vi.mocked(planOverview).mockResolvedValue(
      makeOverviewPlan({ model: makeModelInUse({ ready: false, problem: "Choose a model in Preferences › Model." }) }),
    );
    vi.mocked(getTestsState).mockResolvedValue(makeTestsState({ run: makeTestRun() }));
    const user = await openOverview();

    expect(await screen.findByText(/No model is set up to write it\./)).toBeInTheDocument();
    await user.click(screen.getByRole("button", { name: "Open Preferences" }));
    expect(onOpenPreferences).toHaveBeenCalledOnce();
    expect(screen.queryByRole("button", { name: "Summarize" })).not.toBeInTheDocument();
    const measured = section("Measured in this branch");
    expect(measured.getByText("7 passing")).toBeInTheDocument();
    expect(measured.getByText("1 failing")).toBeInTheDocument();
    expect(measured.getByText("67%")).toBeInTheDocument();
  });

  it("says quietly when Jira is not connected, with a Connect link, and looks for the key in the pull request", async () => {
    const target: ReviewTarget = { kind: "branch", branch: "feat/refund-flow", base: "main" };
    vi.mocked(listChangedFiles).mockResolvedValue({ ...changeSet, target });
    vi.mocked(linkPullRequest).mockResolvedValue(makeLinked(null, { title: "PAY-482 Adds the refund flow" }));
    vi.mocked(findCard).mockResolvedValue({ kind: "not_connected", key: "PAY-482" });
    const user = userEvent.setup();
    render(<ReviewScreen projectId="p-1" target={target} barSlot={bar} onOpenPreferences={onOpenPreferences} />);
    await changedFilesRead();

    const card = section("What the card asks");
    expect(await card.findByText(/looks like a Jira card, but Jira is not connected\./)).toBeInTheDocument();
    expect(findCard).toHaveBeenCalledExactlyOnceWith(target, "PAY-482 Adds the refund flow");
    await waitFor(() =>
      expect(planOverview).toHaveBeenCalledWith("p-1", expect.anything(), {
        title: "PAY-482 Adds the refund flow",
        description: "Refunds settled payments.",
      }),
    );
    await user.click(card.getByRole("button", { name: "Connect" }));
    expect(onOpenPreferences).toHaveBeenCalledOnce();
    expect(screen.queryByRole("region", { name: /^Acceptance criteria/ })).not.toBeInTheDocument();
    expect(within(bar).getByText("PAY-482")).not.toHaveAttribute("href");
  });

  it("says when the branch holds no card key", async () => {
    vi.mocked(findCard).mockResolvedValue({ kind: "no_key", branch: "feat/PAY-482-refunds" });
    await openOverview();
    expect(await section("What the card asks").findByText(/No card key, such as/)).toBeInTheDocument();
  });

  it("tells what changed since the last review and shows only that in the Diff tab", async () => {
    const since = [makeChangedFile("internal/refund/service.go")];
    vi.mocked(getReviewState).mockResolvedValue(
      makeReviewState({
        checkpoint: { sha: "a41c9e2000000000", at: "2026-11-12T10:00:00Z" },
        since: { kind: "available", checkpointSha: "a41c9e2000000000", commits: 2, files: since },
        summary: { ...makeReviewState().summary, reopenedFiles: 1 },
      }),
    );
    vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings: [makeFinding("f", { recheck: true, verdict: "confirmed" })] }));
    const user = await openOverview();

    // Shown once the review state is read.
    const box = within(await screen.findByRole("region", { name: /^Since your last review/ }));
    expect(await box.findByText(/You reviewed on 12 Nov/)).toHaveTextContent(
      "You reviewed on 12 Nov at a41c9e2. 2 commits were added after that.",
    );
    expect(box.getByText("1 file changed: service.go")).toBeInTheDocument();
    expect(box.getByText("1 file you had marked as reviewed was reopened")).toBeInTheDocument();
    expect(box.getByText("1 finding you classified was touched")).toBeInTheDocument();

    await user.click(box.getByRole("button", { name: "Show only what changed" }));
    expect(tabs().getByRole("tab", { name: "Diff" })).toHaveAttribute("aria-selected", "true");
    expect(await screen.findByRole("button", { name: "Since last review" })).toHaveAttribute("aria-pressed", "true");
  });

  it("ranks duplicated code with the limits and counts it among what was measured", async () => {
    vi.mocked(getInsights).mockResolvedValue(NO_INSIGHTS);
    vi.mocked(getQuality).mockResolvedValue({
      kind: "report",
      report: makeQualityReport({ crossed: [], duplicates: [makeDuplicate()] }),
    });
    await openOverview();

    const look = within(await screen.findByRole("list", { name: "Where to look first" }));
    const row = await vi.waitFor(() => {
      const found = look
        .getAllByRole("listitem")
        .find((item) => item.textContent?.includes("Service.Request copies code that already exists"));
      expect(found).toBeDefined();
      return found!;
    });
    expect(row).toHaveTextContent("Copies internal/payment/money.go:40-47, 74 tokens, 8 lines.");
    expect(row).toHaveTextContent("Measuredduplicationinternal/refund/service.go:60-67");
    const measured = section("Measured in this branch");
    expect(measured.getByText("Duplicated code").nextSibling).toHaveTextContent(
      "1 blockcopies of at least 50 tokens and 6 lines",
    );
  });

  it("ranks the static checks among the rest, one row per function, and lists the findings left out below", async () => {
    const severities = ["high", "high", "medium", "medium", "medium", "medium", "low", "low", "low"] as const;
    vi.mocked(getInsights).mockResolvedValue(
      makeInsights({
        findings: severities.map((severity, index) =>
          makeFinding(`f-${index}`, { severity, title: `Finding ${index}` }),
        ),
      }),
    );
    vi.mocked(getQuality).mockResolvedValue({
      kind: "report",
      report: makeQualityReport({ checkFindings: [makeLocatedCheck()] }),
    });
    const user = await openOverview();

    const look = within(await screen.findByRole("list", { name: "Where to look first" }));
    await vi.waitFor(() =>
      expect(look.getAllByRole("listitem").map((item) => item.querySelector("strong")?.textContent)).toEqual([
        "Finding 0",
        "Finding 1",
        "Service.Request has a staticcheck finding and crosses 2 limits",
        "Validate is over the complexity limit",
        "Finding 2",
      ]),
    );
    const row = look.getAllByRole("listitem")[2];
    expect(row).toHaveTextContent(
      "Line 50: this value of err is never used. Complexity 11 and CRAP 34.2; the limits are 10 and 30.",
    );
    expect(within(row).getByText("this value of err is never used")).toHaveAttribute("lang", "en");
    expect(row).toHaveTextContent("MeasuredstaticcheckSA4006Warning2 limitsinternal/refund/service.go:44");
    expect(look.getAllByRole("listitem")[0]).toHaveTextContent("ModelHighinternal/refund/service.go:52");

    const suggested = section("The model suggests changing");
    expect(screen.getByRole("heading", { name: /^The model suggests changing/ })).toHaveTextContent(
      "The model suggests changingModel6 beyond the 3 above",
    );
    const rows = suggested.getAllByRole("button", { name: /Open in Diff$/ });
    expect(rows.map((button) => button.getAttribute("aria-label"))).toEqual([
      "Medium: Finding 3, internal/refund/service.go:52. Open in Diff",
      "Medium: Finding 4, internal/refund/service.go:52. Open in Diff",
      "Medium: Finding 5, internal/refund/service.go:52. Open in Diff",
      "Low: Finding 6, internal/refund/service.go:52. Open in Diff",
      "Low: Finding 7, internal/refund/service.go:52. Open in Diff",
    ]);
    expect(suggested.getByText("1 more only there")).toBeInTheDocument();
    const measured = section("Measured in this branch");
    expect(measured.getByText("Static checks").nextSibling).toHaveTextContent("1 findingstaticcheck 1");

    await user.click(rows[0]);
    expect(tabs().getByRole("tab", { name: "Diff" })).toHaveAttribute("aria-selected", "true");
    await user.click(tabs().getByRole("tab", { name: "Overview" }));
    await user.click(suggested.getByRole("button", { name: "See the 9 findings in Insights" }));
    expect(tabs().getByRole("tab", { name: /^Insights/ })).toHaveAttribute("aria-selected", "true");
  });

  it("has no list of suggestions when every finding is in where to look first", async () => {
    vi.mocked(getInsights).mockResolvedValue(makeInsights({ findings: [makeFinding("only", { title: "Only one" })] }));
    await openOverview();

    expect(await screen.findByText("Only one")).toBeInTheDocument();
    expect(screen.queryByRole("region", { name: /^The model suggests changing/ })).not.toBeInTheDocument();
  });
});
