import { LoaderCircle } from "lucide-react";
import { lazy, type ReactNode, Suspense } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import type { DiffTarget } from "@/features/review/flow/flow-facts";
import { FlowToolbar } from "@/features/review/flow/FlowToolbar";
import type { FlowState } from "@/features/review/flow/use-flow";
import { useNow } from "@/hooks/use-now";
import { currentLanguage, i18n } from "@/i18n";
import type { ChangedFile, FlowOptions, FlowUnavailable } from "@/lib/api";
import { coreText } from "@/lib/core-text";
import { languageName } from "@/lib/format";

// React Flow and ELK are loaded with the first graph, so they stay out of the main bundle.
const FlowWorkspace = lazy(() => import("@/features/review/flow/FlowWorkspace"));

interface FlowViewProps {
  state: FlowState;
  /** What the graph is asked for with; changing it asks for a new graph. */
  options: FlowOptions;
  onOptionsChange: (options: FlowOptions) => void;
  /** Files of the review, to find the diff of a function. */
  files: readonly ChangedFile[];
  onRetry: () => void;
  onViewDiff: (target: DiffTarget) => void;
  onOpenDiff: () => void;
  /** A function to show, opened from another tab; a new object shows it again. */
  reveal?: Reveal | null;
}

/** A function to show in the graph, as its popover. */
export interface Reveal {
  functionId: string;
}

/** Content of the Flow tab: the left panel and the graph, or why there is no graph yet. */
export function FlowView({
  state,
  options,
  onOptionsChange,
  files,
  onRetry,
  onViewDiff,
  onOpenDiff,
  reveal = null,
}: FlowViewProps) {
  const { t } = useTranslation();
  if (state.status === "ready") {
    const { result, pending } = state;
    if (result.kind === "graph") {
      return (
        <Suspense
          fallback={
            <Shell>
              <p role="status" className="flex items-center gap-2">
                <Spinner />
                {t("flow.view.preparing")}
              </p>
            </Shell>
          }
        >
          <FlowWorkspace
            graph={result.graph}
            pending={pending}
            options={options}
            onOptionsChange={onOptionsChange}
            files={files}
            onViewDiff={onViewDiff}
            reveal={reveal}
          />
        </Suspense>
      );
    }
    return (
      <Shell toolbar={false}>
        <Unavailable reason={result} onRetry={onRetry} onOpenDiff={onOpenDiff} />
      </Shell>
    );
  }
  if (state.status === "error") {
    return (
      <Shell>
        <ErrorNote>{state.message}</ErrorNote>
        <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
          {t("common.actions.tryAgain")}
        </Button>
      </Shell>
    );
  }
  return (
    <Shell>
      {/* The first review of a language may wait for its analyzer to be prepared. */}
      <PreparingAnalyzers fallback={<Analysing startedAt={state.status === "loading" ? state.startedAt : null} />} />
    </Shell>
  );
}

/** The two columns of the tab with an empty left panel, for every state without a graph. */
function Shell({ children, toolbar = true }: { children: ReactNode; toolbar?: boolean }) {
  return (
    <>
      <aside className="border-r bg-card" />
      <div className="flex min-h-0 min-w-0 flex-col">
        {toolbar && <FlowToolbar />}
        <div className="max-w-[620px] px-4 py-4">{children}</div>
      </div>
    </>
  );
}

function Analysing({ startedAt }: { startedAt: number | null }) {
  const { t } = useTranslation();
  const now = useNow(1000);
  const seconds = startedAt === null ? null : Math.max(0, Math.floor((now.getTime() - startedAt) / 1000));
  return (
    <div role="status" aria-label={t("flow.view.analysingLabel")}>
      <p className="flex items-center gap-2">
        <Spinner />
        {t("flow.view.analysing")}
        {seconds !== null && <span className="font-mono text-xs text-muted-foreground">{seconds} s</span>}
      </p>
      <p className="mt-1.5 text-xs text-faint">{t("flow.view.analysingHint")}</p>
    </div>
  );
}

export function Spinner() {
  return <LoaderCircle aria-hidden="true" className="size-4 flex-none animate-spin text-muted-foreground" />;
}

/** "Go or Rust", in the language shown. */
export function anyOf(names: readonly string[]): string {
  return new Intl.ListFormat(currentLanguage(), { type: "disjunction" }).format(names);
}

export function noAnalyzerMessage(languages: readonly string[]): string {
  const names = languages.length > 0 ? anyOf(languages.map(languageName)) : i18n.t("flow.view.projectLanguages");
  return i18n.t("flow.view.noAnalyzer", { languages: names });
}

/** "Analysing the old side of this review failed." */
export function analysisFailedMessage(side: "old" | "new"): string {
  return i18n.t(`flow.view.analysisFailed.${side}`);
}

/** The heading of a review that could not be analysed: which side failed, or which analyzer could not be prepared. */
export function unavailableHeading(reason: Exclude<FlowUnavailable, { reason: "no_analyzer" }>): string {
  return reason.reason === "analyzer_unavailable"
    ? i18n.t("common.analyzer.unavailable", { language: reason.analyzer })
    : analysisFailedMessage(reason.side);
}

interface UnavailableProps {
  reason: FlowUnavailable;
  onRetry: () => void;
  onOpenDiff: () => void;
}

function Unavailable({ reason, onRetry, onOpenDiff }: UnavailableProps) {
  const { t } = useTranslation();
  if (reason.reason === "no_analyzer") {
    return (
      <>
        <p>{noAnalyzerMessage(reason.languages)}</p>
        <Button variant="secondary" size="sm" className="mt-2.5" onClick={onOpenDiff}>
          {t("flow.view.openDiff")}
        </Button>
      </>
    );
  }
  return (
    <>
      <ErrorNote>{unavailableHeading(reason)}</ErrorNote>
      {/* The analyzer's own output, as it wrote it. */}
      <pre className="mt-2 max-h-60 overflow-auto rounded-sm border bg-card px-3 py-2 font-mono text-xs whitespace-pre-wrap text-muted-foreground">
        {coreText(reason.message)}
      </pre>
      <Button variant="secondary" size="sm" className="mt-2.5" onClick={onRetry}>
        {t("common.actions.tryAgain")}
      </Button>
    </>
  );
}
