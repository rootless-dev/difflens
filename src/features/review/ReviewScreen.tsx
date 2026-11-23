import { type ReactNode, use, useCallback, useEffect, useMemo, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { useFullAnalyses } from "@/features/full-analysis/full-analyses";
import { FullAnalysisOffer } from "@/features/full-analysis/FullAnalysisOffer";
import { FullAnalysisScreen } from "@/features/full-analysis/FullAnalysisScreen";
import { useCard } from "@/features/review/card/use-card";
import {
  ExplanationsContext,
  explanationSpans,
  useExplanations,
} from "@/features/review/explanations/use-explanations";
import type { ScrollTarget } from "@/features/review/diff-scroll";
import {
  DEFAULT_DIFF_OPTIONS,
  type DiffMode,
  type DiffOptions,
  type DiffScope,
  DiffToolbar,
  type SignalToggles,
} from "@/features/review/DiffToolbar";
import {
  FileDiff,
  type FileComments,
  type FileMark,
  type SignalDraftActions,
  UnchangedSince,
} from "@/features/review/FileDiff";
import { FileList } from "@/features/review/FileList";
import { adjacentPath, FILTER_THRESHOLD, filterFiles, groupFiles, orderedPaths } from "@/features/review/file-tree";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { FlowView, type Reveal } from "@/features/review/flow/FlowView";
import { type FunctionReview, FunctionReviewContext } from "@/features/review/flow/function-review";
import { DEFAULT_FLOW_OPTIONS, useFlow } from "@/features/review/flow/use-flow";
import { InsightsView } from "@/features/review/insights/InsightsView";
import { findingCounts, useInsights } from "@/features/review/insights/use-insights";
import { OverviewView } from "@/features/review/overview/OverviewView";
import { pullRequestText, useOverview } from "@/features/review/overview/use-overview";
import { type QualityDrafts, QualityView } from "@/features/review/quality/QualityView";
import { crossedCount, useQuality } from "@/features/review/quality/use-quality";
import { PublishButton } from "@/features/review/pull-request/PublishButton";
import { type PullRequestHandle, usePullRequest } from "@/features/review/pull-request/use-pull-request";
import { VerdictControls } from "@/features/review/pull-request/VerdictControls";
import { type CommentSide, isOpen, threadsOf } from "@/features/review/review-state/comments";
import { CommentsView } from "@/features/review/review-state/CommentsView";
import {
  type CommentActions,
  type CommentFocus,
  CommentFocusContext,
  PublishingContext,
} from "@/features/review/review-state/LineNotes";
import { ReviewProgress } from "@/features/review/review-state/ReviewProgress";
import {
  fileMarks,
  functionMarks,
  functionSpans,
  reviewedCount,
  sinceTitle,
  spanOf,
} from "@/features/review/review-state/review-status";
import { type ReviewStateHandle, useReviewState } from "@/features/review/review-state/use-review-state";
import { ReviewBar } from "@/features/review/ReviewBar";
import { baseLabel } from "@/features/review/review-target";
import { reviewSignals, type SignalsHandle, SignalsContext } from "@/features/review/signals/signals";
import { type FindingComments, useFindingComments } from "@/features/review/signals/use-finding-comments";
import { DiagnosesBanner, failureExplanation } from "@/features/review/tests/Diagnosis";
import { TestsView } from "@/features/review/tests/TestsView";
import { useTestDiagnoses } from "@/features/review/tests/use-diagnoses";
import { mutationRunOf, useMutation } from "@/features/review/tests/use-mutation";
import { failingCount, useTests } from "@/features/review/tests/use-tests";
import { useFileKeys } from "@/features/review/use-file-keys";
import {
  ENABLED_VIEWS,
  laterStep,
  type ReviewOpening,
  type ReviewView,
  viewLabel,
  VIEWS,
} from "@/features/review/views";
import { useAsyncData } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import {
  type ChangeSet,
  type FlowNode,
  type FlowOptions,
  type FunctionSpan,
  listChangedFiles,
  type MarkStatus,
  type ReviewComment,
  type ReviewTarget,
  type SignalDraft,
  type Verdict,
} from "@/lib/api";
import { readinessReason } from "@/lib/core-text";
import { cn } from "@/lib/utils";

const FIRST_VIEW: ReviewView = "Overview";

interface ReviewScreenProps {
  projectId: string;
  target: ReviewTarget;
  /** Top bar element the compare chip, stat and Reload are portalled into. Nothing is shown there while null. */
  barSlot: HTMLElement | null;
  /** Opens Preferences, offered when the pull request host is not connected. */
  onOpenPreferences?: () => void;
  /** The tab to open on, from a branch row's shortcuts; the Overview when absent. */
  opening?: ReviewOpening;
}

/** A file and line to show in the Diff tab, set when a function is opened from the Flow tab. */
interface Jump {
  path: string;
  scroll: ScrollTarget;
}

export function ReviewScreen({ projectId, target, barSlot, onOpenPreferences, opening }: ReviewScreenProps) {
  const { t } = useTranslation();
  const load = useCallback(() => listChangedFiles(projectId, target), [projectId, target]);
  const { state, reload } = useAsyncData(load);
  // A review opens on the Overview, the summary of the change, before its details, unless a shortcut named a tab.
  const [initialView] = useState<ReviewView>(() => opening?.view ?? FIRST_VIEW);
  const [view, setView] = useState<ReviewView>(initialView);
  // The flow is computed the first time its tab opens, or with the Overview, which shows it; never with a model.
  const [flowRequested, setFlowRequested] = useState(true);
  // Kept across Reload and across files, for as long as the screen is open.
  const [selectedPath, setSelectedPath] = useState<string | null>(null);
  const [jump, setJump] = useState<Jump | null>(null);
  const [query, setQuery] = useState("");
  const [options, setOptions] = useState<DiffOptions>(DEFAULT_DIFF_OPTIONS);
  // Files or Comments: like the diff options, kept for as long as the review is open, never saved.
  const [diffMode, setDiffMode] = useState<DiffMode>(() => opening?.diffMode ?? "files");
  // Like the diff options: kept for as long as the screen is open, never saved.
  const [flowOptions, setFlowOptions] = useState<FlowOptions>(DEFAULT_FLOW_OPTIONS);
  const changeSet = state.status === "ready" ? state.data : null;
  const flow = useFlow(projectId, changeSet, flowRequested, flowOptions);
  // The last test run is read (never run) as soon as the change set is there, so the Tests tab can show its count.
  // Each run that reports bumps testRuns, so the quality report, which takes coverage from it, is asked for again.
  const [testRuns, setTestRuns] = useState(0);
  const onRunCompleted = useCallback(() => setTestRuns((n) => n + 1), []);
  const tests = useTests(projectId, changeSet, onRunCompleted);
  // Mutation testing is read like the tests and asked again after each test run (its coverage picks the lines); it
  // only runs on a click in the Tests tab, and goes on while another tab is shown.
  const mutation = useMutation(projectId, changeSet, testRuns);
  // Like the flow, quality is measured the first time its tab or the Overview opens.
  const [qualityRequested, setQualityRequested] = useState(true);
  const quality = useQuality(projectId, changeSet, qualityRequested, testRuns);
  // Findings already written are read with the change set, for the tab's count; what Analyze would send is worked
  // out the first time the tab opens. The model is only asked when Analyze (or Retry) is clicked.
  const [insightsRequested, setInsightsRequested] = useState(initialView === "Insights");
  const insights = useInsights(projectId, changeSet, insightsRequested);
  const findings = findingCounts(insights.insights);
  // What the quality report, the last mutation run and the findings say of each function and file, worked out once
  // for the Overview, the Flow and the Diff; a mutation run that completes is in them at once.
  const qualityReport =
    quality.state.status === "ready" && quality.state.result.kind === "report" ? quality.state.result.report : null;
  const findingList = insights.insights?.findings ?? null;
  const mutationRun = mutationRunOf(mutation.panel);
  const signals = useMemo(
    () => reviewSignals(qualityReport, findingList, mutationRun),
    [qualityReport, findingList, mutationRun],
  );
  // Marks, comments and the last review. Functions are asked about once the flow graph says which changed.
  const graph = flow.state.status === "ready" && flow.state.result.kind === "graph" ? flow.state.result.graph : null;
  const spans = useMemo(() => (graph ? functionSpans(graph.nodes) : NO_SPANS), [graph]);
  const review = useReviewState(projectId, changeSet, spans);
  // A model finding's review comment, written by the model only on a click on Write comment in the Diff; the draft is
  // then read back with the review state.
  const findingComments = useFindingComments(projectId, changeSet, review.refresh, onOpenPreferences);
  // Looked for on open and on Reload; finding it brings its comments into the review state. Its description stays in
  // `pullRequest.status` for the model.
  const pullRequest = usePullRequest(projectId, changeSet, review.refresh);
  // The verdict only follows when every comment went through: a verdict without the comments that explain it would
  // read differently.
  const publishThenVerdict = async (verdict: Verdict) => {
    const result = await pullRequest.publish();
    if (result?.kind === "report" && result.report.failed.length === 0) await pullRequest.setVerdict(verdict, true);
  };
  // The card is read from Jira (only read) when the change set arrives, like the pull request; a key only in the pull
  // request's title waits for it.
  const card = useCard(changeSet, pullRequest.status);
  // The Overview reads its summary and the shape of the change the first time its tab opens; the model is only asked
  // when Summarize is clicked.
  const [overviewRequested, setOverviewRequested] = useState(true);
  const overview = useOverview(projectId, changeSet, overviewRequested, pullRequestText(pullRequest.status));
  const [reveal, setReveal] = useState<Reveal | null>(null);
  const marks = useMemo(() => fileMarks(review.state), [review.state]);
  const [scope, setScope] = useState<DiffScope>("branch");
  const functionReview = useFunctionReview(review);

  // A view is mounted the first time its tab opens and then kept mounted, only hidden, so going back to it is
  // instant and finds it as it was (the file and its scroll, the canvas and its zoom) instead of building it again.
  const [visited, setVisited] = useState<ReadonlySet<ReviewView>>(() => new Set([initialView]));
  const show = (next: ReviewView) => {
    setView(next);
    setVisited((current) => (current.has(next) ? current : new Set(current).add(next)));
  };

  // What a model wrote about each function, read when the Flow tab first opens; written only by an Explain button
  // (or a full analysis).
  const explainedSpans = useMemo(() => (graph ? explanationSpans(graph.nodes) : null), [graph]);
  const explanations = useExplanations(
    projectId,
    changeSet,
    explainedSpans,
    visited.has("Flow"),
    pullRequestText(pullRequest.status),
  );
  // A comment to bring into view in the Diff tab (View comment in Diff, or a row of the Comments view), until another
  // file or place is opened: a line comment in Files, at its line; a general one in Comments.
  const [focus, setFocus] = useState<CommentFocus | null>(null);
  const focusRequests = useRef(0);
  const viewComment = useCallback((comment: ReviewComment) => {
    if (comment.kind === "line") {
      // Shown in the branch's diff, where the comment was written.
      setScope("branch");
      setSelectedPath(comment.path);
      setJump({ path: comment.path, scroll: { line: comment.currentLine ?? comment.line, side: comment.side } });
      setDiffMode("files");
    } else {
      setDiffMode("comments");
    }
    setFocus({ commentId: comment.id, request: ++focusRequests.current });
    setView("Diff");
    setVisited((current) => (current.has("Diff") ? current : new Set(current).add("Diff")));
  }, []);
  // A draft written from a finding or a check, shown where it is. One just written may not be read back yet: a
  // general one is then found among the comments by its id.
  const latestComments = useRef<readonly ReviewComment[]>([]);
  useEffect(() => {
    latestComments.current = review.state?.comments ?? [];
  });
  const viewDraft = useCallback(
    (draft: SignalDraft) => {
      const comment = latestComments.current.find((candidate) => candidate.id === draft.commentId);
      if (comment) {
        viewComment(comment);
      } else if (draft.kind === "general") {
        setDiffMode("comments");
        setFocus({ commentId: draft.commentId, request: ++focusRequests.current });
      }
    },
    [viewComment],
  );
  // The Quality tab writes a draft on a function over a limit like the Diff writes one from a signal, and shows it
  // there.
  const qualitySignalDrafts = review.state?.signalDrafts;
  const qualityDrafts = useMemo<QualityDrafts | null>(() => {
    if (!qualitySignalDrafts) return null;
    const drafts = new Map(qualitySignalDrafts.map((draft) => [draft.signal, draft]));
    return {
      draftOf: (signal) => drafts.get(signal) ?? null,
      create: review.actions.draftFromSignal,
      view: viewDraft,
      busy: review.busy,
    };
  }, [qualitySignalDrafts, review.actions, review.busy, viewDraft]);
  // Why a failing test failed, as a model explained it: read when the Tests tab first opens on a run with failures,
  // and again after each run; written only by Explain the failure. A review comment written from one is a draft of the
  // review, shown in the Diff.
  const diagnosisComments = useMemo(
    () => ({ comments: review.state?.comments ?? null, onWritten: review.refresh, onView: viewComment }),
    [review.state, review.refresh, viewComment],
  );
  const diagnoses = useTestDiagnoses(projectId, changeSet, tests.panel, visited.has("Tests"), diagnosisComments);

  // A full analysis of this review, run in the core: while it runs (or once it ended with a failure or a cancel, until
  // See results) it covers the review and its tabs are disabled. When it ends, what it wrote is read again.
  const fullAnalyses = useFullAnalyses();
  const job = fullAnalyses.jobFor(projectId, target);
  const covering = job !== null && !(job.status === "finished" && job.outcome === "succeeded");
  const refreshResults = useRef<() => void>(() => {});
  useEffect(() => {
    refreshResults.current = () => {
      overview.retry();
      insights.refresh();
      tests.retry();
      mutation.retry();
      quality.retry();
      explanations.refresh();
    };
  });
  // Once per job that ended: its id and outcome do not change after that.
  const finishedId = job !== null && job.status === "finished" ? job.id : null;
  const succeeded = job !== null && job.outcome === "succeeded";
  const dismissJob = fullAnalyses.dismiss;
  useEffect(() => {
    if (finishedId === null) return;
    refreshResults.current();
    if (succeeded) {
      dismissJob(finishedId);
      setView(FIRST_VIEW);
    }
  }, [finishedId, succeeded, dismissJob]);
  const seeResults = () => {
    if (job === null) return;
    fullAnalyses.dismiss(job.id);
    openView("Overview");
  };

  const openView = (next: ReviewView) => {
    if (next === "Flow") setFlowRequested(true);
    if (next === "Quality") setQualityRequested(true);
    if (next === "Insights") setInsightsRequested(true);
    // The Overview shows what the flow and the quality report measured, so it asks for them too (never a model).
    if (next === "Overview") {
      setOverviewRequested(true);
      setFlowRequested(true);
      setQualityRequested(true);
    }
    show(next);
  };

  // Reload drops the flow, the quality report, the plan of an analysis and the overview; each is computed again for
  // the new change set when its tab (or the Overview, which shows the flow and quality) is next shown.
  const onReload = () => {
    reload();
    if (view !== "Flow" && view !== "Overview") setFlowRequested(false);
    if (view !== "Quality" && view !== "Overview") setQualityRequested(false);
    if (view !== "Insights") setInsightsRequested(false);
    if (view !== "Overview") setOverviewRequested(false);
  };

  const openFlow = (functionId: string) => {
    setReveal({ functionId });
    openView("Flow");
  };

  // The tabs the signals' sections link to, from the Flow's popovers and the Diff's blocks.
  const opener = useRef(openView);
  useEffect(() => {
    opener.current = openView;
  });
  const signalsHandle = useMemo<SignalsHandle>(
    () => ({
      signals,
      onOpenQuality: () => opener.current("Quality"),
      onOpenInsights: () => opener.current("Insights"),
      onOpenTests: () => opener.current("Tests"),
    }),
    [signals],
  );

  const showSince = () => {
    setScope("since");
    show("Diff");
  };

  const counts: Partial<Record<ReviewView, Count | null>> = {
    Tests: countOf(failingCount(tests.panel), (count) => t("review.views.counts.failing", { count })),
    Quality: countOf(crossedCount(quality.state), (count) => t("review.views.counts.limitsCrossed", { count })),
    Insights: countOf(findings?.total ?? null, (count) => t("review.views.counts.findings", { count }), "ai"),
  };

  const selectFile = useCallback((path: string) => {
    setSelectedPath(path);
    setJump(null);
    setFocus(null);
  }, []);

  const viewDiff = ({ path, line, side, reveal: revealed }: DiffTarget) => {
    setSelectedPath(path);
    setJump({ path, scroll: { line, side } });
    setFocus(null);
    // Going to a finding or a check shows its kind of block again, if it was hidden.
    if (revealed === "finding") setOptions((current) => ({ ...current, showFindings: true }));
    if (revealed === "check") setOptions((current) => ({ ...current, showChecks: true }));
    show("Diff");
  };

  return (
    <div className="flex min-h-0 flex-1 flex-col">
      {barSlot &&
        createPortal(
          <ReviewBar
            target={target}
            changeSet={state}
            onReload={onReload}
            pullRequest={pullRequest.status}
            card={card.status}
            onOpenPreferences={onOpenPreferences}
            verdicts={
              <VerdictControls
                status={pullRequest.status}
                unpublished={review.state?.summary.unpublished ?? 0}
                publishBlocked={
                  pullRequest.status.kind === "linked" && !pullRequest.status.publish.allowed
                    ? (readinessReason(pullRequest.status.publish) ?? t("review.screen.publishingNotPossible"))
                    : null
                }
                deciding={pullRequest.deciding}
                busy={pullRequest.publishing || review.busy}
                error={pullRequest.verdictError}
                onVerdict={(verdict, given) => void pullRequest.setVerdict(verdict, given)}
                onPublishThenVerdict={(verdict) => void publishThenVerdict(verdict)}
              />
            }
          />,
          barSlot,
        )}
      <Tabs value={view} onValueChange={(value) => openView(value as ReviewView)} className="min-h-0 flex-1">
        <div className="flex flex-none items-end border-b bg-card">
          <TabsList aria-label={t("review.views.label")} className="min-w-0 flex-1 border-b-0">
            {VIEWS.map((name) => (
              <TabsTrigger
                key={name}
                value={name}
                disabled={!ENABLED_VIEWS.has(name) || covering}
                title={
                  !ENABLED_VIEWS.has(name) ? laterStep() : covering ? t("fullAnalysis.screen.tabsLocked") : undefined
                }
                aria-label={counts[name] ? `${viewLabel(name)}, ${counts[name].label}` : undefined}
              >
                {viewLabel(name)}
                {counts[name] && <TabCount count={counts[name]} />}
              </TabsTrigger>
            ))}
          </TabsList>
          {/* Not shown as 0 of N while the review state is still being read. */}
          {changeSet && changeSet.files.length > 0 && (review.state !== null || review.error !== null) && (
            <ReviewProgress
              reviewed={reviewedCount(changeSet, marks)}
              total={changeSet.files.length}
              error={review.error}
              findings={findings}
            />
          )}
        </div>
        {/* What the tabs show, and over it the progress of a full analysis of this review. */}
        <div className="relative flex min-h-0 flex-1 flex-col">
          {covering && job && (
            <FullAnalysisScreen job={job} onCancel={() => fullAnalyses.cancel(job.id)} onSeeResults={seeResults} />
          )}
          <TabsContent value="Overview" keepMounted tabIndex={-1} className="flex min-h-0 flex-1 flex-col">
            {visited.has("Overview") &&
              (state.status === "error" ? (
                <ChangeSetError message={state.message} onRetry={reload} panel={false} />
              ) : (
                <OverviewView
                  handle={overview}
                  card={card}
                  pullRequest={pullRequest.status}
                  files={changeSet?.files ?? []}
                  review={review.state}
                  insights={insights.insights}
                  tests={tests.panel}
                  quality={quality.state}
                  flow={flow.state}
                  onViewDiff={viewDiff}
                  onOpenFlow={openFlow}
                  onOpenTests={() => openView("Tests")}
                  onOpenInsights={() => openView("Insights")}
                  signals={signals}
                  onShowSince={showSince}
                  onOpenPreferences={onOpenPreferences}
                  fullAnalysis={
                    <FullAnalysisOffer
                      projectId={projectId}
                      changeSet={changeSet}
                      pullRequest={pullRequestText(pullRequest.status)}
                      busy={overview.running !== null || insights.running !== null}
                      onOpenPreferences={onOpenPreferences}
                    />
                  }
                />
              ))}
          </TabsContent>
          <TabsContent value="Insights" keepMounted tabIndex={-1} className="flex min-h-0 flex-1 flex-col">
            {visited.has("Insights") &&
              (state.status === "error" ? (
                <ChangeSetError message={state.message} onRetry={reload} panel={false} />
              ) : (
                <InsightsView
                  handle={insights}
                  files={changeSet?.files ?? []}
                  onViewDiff={viewDiff}
                  onOpenPreferences={onOpenPreferences}
                />
              ))}
          </TabsContent>
          <TabsContent value="Tests" keepMounted tabIndex={-1} className="flex min-h-0 flex-1 flex-col">
            {visited.has("Tests") &&
              (state.status === "error" ? (
                <ChangeSetError message={state.message} onRetry={reload} panel={false} />
              ) : (
                <TestsView
                  panel={tests.panel}
                  files={changeSet?.files ?? []}
                  onRun={tests.run}
                  onCancel={tests.cancel}
                  onChoose={tests.choose}
                  onRetry={tests.retry}
                  onViewDiff={viewDiff}
                  renderExplanation={(failure) => failureExplanation(failure, diagnoses)}
                  renderFailuresBanner={(failures) => <DiagnosesBanner failures={failures} handle={diagnoses} />}
                  mutation={mutation}
                />
              ))}
          </TabsContent>
          <TabsContent value="Quality" keepMounted tabIndex={-1} className="flex min-h-0 flex-1 flex-col">
            {visited.has("Quality") &&
              (state.status === "error" ? (
                <ChangeSetError message={state.message} onRetry={reload} panel={false} />
              ) : (
                <QualityView
                  state={quality.state}
                  files={changeSet?.files ?? []}
                  drafts={qualityDrafts}
                  onRetry={quality.retry}
                  onViewDiff={viewDiff}
                  onOpenTests={() => openView("Tests")}
                  onOpenDiff={() => openView("Diff")}
                />
              ))}
          </TabsContent>
          <TabsContent
            value="Flow"
            keepMounted
            tabIndex={-1}
            className="grid min-h-0 flex-1 grid-cols-[232px_minmax(0,1fr)]"
          >
            {visited.has("Flow") &&
              (state.status === "error" ? (
                <ChangeSetError message={state.message} onRetry={reload} />
              ) : (
                <FunctionReviewContext value={functionReview}>
                  <ExplanationsContext value={explanations}>
                    <SignalsContext value={signalsHandle}>
                      <FlowView
                        state={flow.state}
                        options={flowOptions}
                        onOptionsChange={setFlowOptions}
                        files={changeSet?.files ?? []}
                        onRetry={flow.retry}
                        onViewDiff={viewDiff}
                        onOpenDiff={() => openView("Diff")}
                        reveal={reveal}
                      />
                    </SignalsContext>
                  </ExplanationsContext>
                </FunctionReviewContext>
              ))}
          </TabsContent>
          {/* Not a tab stop itself: the file list right inside is the next stop after the tabs. */}
          <TabsContent
            value="Diff"
            keepMounted
            tabIndex={-1}
            className="grid min-h-0 flex-1 grid-cols-[232px_minmax(0,1fr)]"
          >
            {state.status === "loading" && <LoadingReview />}
            {state.status === "error" && <ChangeSetError message={state.message} onRetry={reload} />}
            {state.status === "ready" && (
              <SignalsContext value={signalsHandle}>
                <LoadedReview
                  projectId={projectId}
                  active={view === "Diff"}
                  changeSet={state.data}
                  selectedPath={selectedPath}
                  onSelect={selectFile}
                  jump={jump}
                  query={query}
                  onQueryChange={setQuery}
                  options={options}
                  onOptionsChange={setOptions}
                  review={review}
                  marks={marks}
                  scope={scope}
                  onScopeChange={setScope}
                  pullRequest={pullRequest}
                  focus={focus}
                  mode={diffMode}
                  onModeChange={setDiffMode}
                  onViewComment={viewComment}
                  onViewDraft={viewDraft}
                  findingComments={findingComments}
                />
              </SignalsContext>
            )}
          </TabsContent>
        </div>
      </Tabs>
    </div>
  );
}

interface Count {
  value: number;
  /** What it counts, read after the tab's name: "3 failing". */
  label: string;
  /** "problem" for what failed or crossed a limit; "ai" for what a model wrote. */
  tone: "problem" | "ai";
}

/** A tab's count, or null when there is nothing to count yet or it is zero. */
function countOf(value: number | null, label: (value: number) => string, tone: Count["tone"] = "problem"): Count | null {
  return value === null || value === 0 ? null : { value, label: label(value), tone };
}

/** The pill after a tab's name, red or in the model's colour. Only the number is shown; the tab's label reads it. */
function TabCount({ count }: { count: Count }) {
  return (
    <span
      aria-hidden="true"
      className={cn(
        "ml-1.5 rounded-[9px] px-1.5 text-[11px]",
        count.tone === "ai" ? "bg-ai-bg text-ai" : "bg-del-bg text-del",
      )}
    >
      {count.value}
    </span>
  );
}

function ChangeSetError({ message, onRetry, panel = true }: { message: string; onRetry: () => void; panel?: boolean }) {
  const { t } = useTranslation();
  return (
    <>
      {panel && <aside className="border-r bg-card" />}
      <div className="px-4 py-4">
        <ErrorNote>{message}</ErrorNote>
        <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
          {t("common.actions.tryAgain")}
        </Button>
      </div>
    </>
  );
}

/** Function marks for the Flow tab, read from the review state and saved through it. */
function useFunctionReview({ state, actions, busy }: ReviewStateHandle): FunctionReview {
  const marks = useMemo(() => functionMarks(state), [state]);
  const loaded = state !== null;
  return useMemo(
    () => ({
      statusOf: (id: string) => marks.get(id) ?? null,
      toggle: loaded
        ? (node: FlowNode) => {
            const span = spanOf(node);
            // A reviewed function is unmarked; an unreviewed or reopened one is marked (again).
            if (span) void actions.setFunctionReviewed(span, marks.get(node.id) !== "reviewed");
          }
        : null,
      busy,
    }),
    [marks, loaded, actions, busy],
  );
}

interface LoadedReviewProps {
  projectId: string;
  /** The Diff tab is the one shown; its keys only act then. */
  active: boolean;
  changeSet: ChangeSet;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  jump: Jump | null;
  query: string;
  onQueryChange: (query: string) => void;
  options: DiffOptions;
  onOptionsChange: (options: DiffOptions) => void;
  review: ReviewStateHandle;
  marks: ReadonlyMap<string, MarkStatus>;
  scope: DiffScope;
  onScopeChange: (scope: DiffScope) => void;
  pullRequest: PullRequestHandle;
  /** A comment to bring into view, such as one opened from the Tests tab. */
  focus: CommentFocus | null;
  /** Files or Comments. */
  mode: DiffMode;
  onModeChange: (mode: DiffMode) => void;
  /** Opens a comment where it is: a line comment in Files at its line, a general one in Comments. */
  onViewComment: (comment: ReviewComment) => void;
  /** Opens a draft written from a finding or a check where it is. */
  onViewDraft: (draft: SignalDraft) => void;
  /** How the model writes a model finding's comment. */
  findingComments: FindingComments;
}

/** What the comments of the Diff tab read around them: whether there is a pull request, and a comment to show. */
function CommentContexts({
  publishing,
  focus,
  children,
}: {
  publishing: boolean;
  focus: CommentFocus | null;
  children: ReactNode;
}) {
  return (
    <PublishingContext value={publishing}>
      <CommentFocusContext value={focus}>{children}</CommentFocusContext>
    </PublishingContext>
  );
}

function LoadedReview({
  projectId,
  active,
  changeSet,
  selectedPath,
  onSelect,
  jump,
  query,
  onQueryChange,
  options,
  onOptionsChange,
  review,
  marks,
  scope,
  onScopeChange,
  pullRequest,
  focus,
  mode,
  onModeChange,
  onViewComment,
  onViewDraft,
  findingComments,
}: LoadedReviewProps) {
  const { t } = useTranslation();
  const { signals } = use(SignalsContext);
  const since = review.state?.since ?? null;
  // "Since last review" shows the files changed from the checkpoint to the head, each diffed between those two. When
  // it stops being available (a Reload that finds nothing new), the whole branch is shown again.
  const sinceShown = scope === "since" && since?.kind === "available" ? since : null;
  const shown = useMemo<ChangeSet>(
    () => (sinceShown ? { ...changeSet, oldSha: sinceShown.checkpointSha, files: sinceShown.files } : changeSet),
    [changeSet, sinceShown],
  );
  const { files } = shown;
  const filterable = files.length > FILTER_THRESHOLD;
  const groups = useMemo(() => groupFiles(filterable ? filterFiles(files, query) : files), [files, filterable, query]);
  const firstPath = useMemo(() => orderedPaths(groupFiles(files))[0] ?? null, [files]);
  const threads = useMemo(() => threadsOf(review.state?.comments ?? []), [review.state]);
  // The line comments follow the review's own file list, whatever the scope shown.
  const reviewOrder = useMemo(() => orderedPaths(groupFiles(changeSet.files)), [changeSet.files]);

  const inReview = (path: string) => changeSet.files.find((file) => file.path === path) ?? null;
  // The remembered file when the list still has it (it may be gone after Reload), else the first one. Since the last
  // review, a remembered file of the review with nothing new stays selected, to say so.
  const listed = files.find((file) => file.path === selectedPath);
  const unchangedSince = sinceShown && !listed && selectedPath !== null ? inReview(selectedPath) : null;
  const current = listed ?? (unchangedSince ? undefined : files.find((file) => file.path === firstPath));

  // Only over the files: in Comments, j and k would change a file that is not shown.
  useFileKeys((step) => {
    const next = adjacentPath(orderedPaths(groups), current?.path ?? null, step);
    if (next !== null) onSelect(next);
  }, active && mode === "files");
  // A file chosen in the list is shown, from either view.
  const selectFile = (path: string) => {
    onModeChange("files");
    onSelect(path);
  };

  /** The mark button of a file of the review; a file outside it (only in "Since last review") has none. */
  const markOf = (path: string): FileMark | null => {
    if (!inReview(path) || review.state === null) return null;
    const status = marks.get(path) ?? null;
    return {
      status,
      busy: review.busy,
      onToggle: () => void review.actions.setFileReviewed(path, status !== "reviewed"),
    };
  };

  const commentActions = useMemo<CommentActions>(
    () => ({
      add: (side, line, body) => (current ? review.actions.addComment(current.path, side, line, body) : Promise.resolve(false)),
      reply: review.actions.reply,
      edit: review.actions.edit,
      remove: review.actions.remove,
      restore: review.actions.restore,
      resolve: review.actions.resolve,
    }),
    [current, review.actions],
  );
  // Since the last review the old side is the checkpoint, not the review's own: only the new side takes comments.
  const sides = sinceShown ? NEW_SIDE_ONLY : BOTH_SIDES;
  const loaded = review.state !== null;
  const signalDraftList = review.state?.signalDrafts;
  const signalDrafts = useMemo<SignalDraftActions | undefined>(
    () =>
      signalDraftList
        ? {
            drafts: new Map(signalDraftList.map((draft) => [draft.signal, draft])),
            create: review.actions.draftFromSignal,
            view: onViewDraft,
            findings: findingComments,
          }
        : undefined,
    [signalDraftList, review.actions, onViewDraft, findingComments],
  );
  const comments = useMemo<FileComments | null>(
    () => (loaded ? { threads, sides, actions: commentActions, busy: review.busy, signalDrafts } : null),
    [loaded, threads, sides, commentActions, review.busy, signalDrafts],
  );
  // The signal toggles: each only when the review has something of its kind in the Diff.
  const currentSignals = current ? signals.files.get(current.path) : undefined;
  const toggles: SignalToggles = {
    findings: {
      review: [...signals.files.values()].some((file) => file.findings.length > 0 || file.dismissed.length > 0),
      inFile: currentSignals?.findings.length ?? 0,
    },
    // Duplicated code and mutants that survived are shown and hidden with the checks: all are measured on the changed
    // lines.
    checks: {
      review: signals.checks.length + signals.duplicates.length + signals.mutants.length > 0,
      inFile:
        (currentSignals?.checks.length ?? 0) +
        (currentSignals?.duplicates.length ?? 0) +
        (currentSignals?.mutants.length ?? 0),
    },
  };
  const commentsOf = (path: string) => (inReview(path) ? comments : null);

  const sinceTitleText = sinceTitle(since, review.state?.checkpoint?.at ?? null);

  return (
    <CommentContexts publishing={pullRequest.status.kind === "linked"} focus={focus}>
      <FileList
        groups={groups}
        filterable={filterable}
        query={query}
        onQueryChange={onQueryChange}
        selectedPath={current?.path ?? null}
        onSelect={selectFile}
        marks={marks}
        emptyText={sinceShown ? t("review.screen.noFileSince") : undefined}
      />
      <div className="flex min-h-0 min-w-0 flex-col">
        <DiffToolbar
          options={options}
          onChange={onOptionsChange}
          scope={{
            scope: sinceShown ? "since" : "branch",
            onChange: onScopeChange,
            sinceAvailable: since?.kind === "available",
            sinceTitle: sinceTitleText,
          }}
          mode={{ mode, onChange: onModeChange, conversations: loaded ? threads.filter(isOpen).length : null }}
          commentable={review.state !== null}
          signals={toggles}
          publish={
            <PublishButton
              status={pullRequest.status}
              summary={review.state?.summary ?? null}
              publishing={pullRequest.publishing}
              busy={review.busy}
              outcome={pullRequest.outcome}
              onPublish={() => void pullRequest.publish()}
            />
          }
        />
        {/* Both views stay mounted, one hidden, so each is found as it was left: the file and its scroll, a comment
            being written. */}
        {comments && (
          <div hidden={mode !== "comments"} className="flex min-h-0 flex-1 flex-col">
            <CommentsView
              threads={threads}
              order={reviewOrder}
              actions={comments.actions}
              onAdd={review.actions.addGeneralComment}
              busy={review.busy}
              onOpen={onViewComment}
            />
          </div>
        )}
        <div hidden={mode !== "files"} className="flex min-h-0 flex-1 flex-col">
          {current ? (
            <FileDiff
              key={current.path}
              projectId={projectId}
              changeSet={shown}
              file={current}
              options={options}
              scrollTo={jump?.path === current.path ? jump.scroll : null}
              mark={markOf(current.path)}
              comments={commentsOf(current.path)}
            />
          ) : unchangedSince ? (
            <UnchangedSince file={unchangedSince} mark={markOf(unchangedSince.path)} />
          ) : (
            <p className="px-4 py-4 text-muted-foreground">
              {sinceShown ? t("review.screen.nothingSince") : emptyMessage(changeSet)}
            </p>
          )}
        </div>
      </div>
    </CommentContexts>
  );
}

const BOTH_SIDES: ReadonlySet<CommentSide> = new Set(["old", "new"]);
const NEW_SIDE_ONLY: ReadonlySet<CommentSide> = new Set(["new"]);
const NO_SPANS: FunctionSpan[] = [];

function emptyMessage(changeSet: ChangeSet): string {
  const { target } = changeSet;
  return target.kind === "branch"
    ? i18n.t("review.screen.emptyBranch", { branch: target.branch, base: baseLabel(target, changeSet) })
    : i18n.t("review.screen.emptyWorkingTree");
}

function LoadingReview() {
  const { t } = useTranslation();
  return (
    <>
      <aside className="border-r bg-card px-5 py-4">
        <div role="status" aria-label={t("review.screen.loadingFiles")} className="grid gap-3">
          <Skeleton className="h-3 w-1/2" />
          <Skeleton className="h-3.5 w-full" />
          <Skeleton className="h-3.5 w-4/5" />
          <Skeleton className="h-3 w-2/5" />
          <Skeleton className="h-3.5 w-full" />
        </div>
      </aside>
      <div />
    </>
  );
}
