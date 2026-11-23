import { ScanSearch } from "lucide-react";
import { useEffect, useId, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { PreparingAnalyzers } from "@/components/preparing-analyzers";
import { Button } from "@/components/ui/button";
import { currentLabel, logText, logTime, stepName } from "@/features/full-analysis/log-text";
import type { FullAnalysisJob } from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

interface FullAnalysisScreenProps {
  job: FullAnalysisJob;
  onCancel: () => void;
  /** Gives the review back once the job has finished. */
  onSeeResults: () => void;
}

/**
 * What covers a review while its full analysis runs, like an installer: an animated icon, a progress bar with what
 * is in progress, a log that can be folded away, and Cancel. Once the job ends with a failure or a cancel, what
 * failed and the way back to the review.
 */
export function FullAnalysisScreen({ job, onCancel, onSeeResults }: FullAnalysisScreenProps) {
  const { t } = useTranslation();
  const titleId = useId();
  const finished = job.status === "finished";
  const failed = job.steps.filter((step) => step.status === "failed");
  return (
    <section
      aria-labelledby={titleId}
      className="absolute inset-0 z-20 flex flex-col items-center overflow-auto bg-background px-6 pt-[12vh] pb-8"
    >
      <div className="grid w-full max-w-[640px] justify-items-center gap-5">
        <ScanSearch
          aria-hidden="true"
          className={cn("size-12 text-primary", !finished && "animate-pulse")}
          strokeWidth={1.5}
        />
        <h1 id={titleId} className="text-lg font-semibold tracking-[-0.01em]">
          {finished
            ? job.outcome === "cancelled"
              ? t("fullAnalysis.screen.cancelledTitle")
              : t("fullAnalysis.screen.failedTitle")
            : t("fullAnalysis.title")}
        </h1>
        {!finished && <Progress job={job} />}
        {/* The flow, quality and test steps wait for an analyzer prepared on first need. */}
        {!finished && <PreparingAnalyzers className="w-full justify-items-center" />}
        {finished && (
          <div className="grid w-full gap-2">
            {job.outcome === "cancelled" ? (
              <p className="text-muted-foreground">{t("fullAnalysis.screen.cancelledBody")}</p>
            ) : (
              <>
                <p className="text-muted-foreground">{t("fullAnalysis.screen.failedBody")}</p>
                <ul aria-label={t("fullAnalysis.screen.failedBody")} className="grid gap-1.5">
                  {failed.map((step) => (
                    <li key={step.step}>
                      <ErrorNote>
                        <b className="font-medium">{stepName(step.step)}:</b>{" "}
                        {step.error ? errorMessage(step.error) : null}
                      </ErrorNote>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        )}
        <Log job={job} />
        {finished ? (
          <Button onClick={onSeeResults}>{t("fullAnalysis.screen.seeResults")}</Button>
        ) : (
          <Button variant="secondary" disabled={job.cancelling} onClick={onCancel}>
            {job.cancelling ? t("fullAnalysis.screen.cancelling") : t("fullAnalysis.screen.cancel")}
          </Button>
        )}
      </div>
    </section>
  );
}

function Progress({ job }: { job: FullAnalysisJob }) {
  const { t } = useTranslation();
  const label = currentLabel(job);
  return (
    <div className="grid w-full gap-1.5">
      <div
        role="progressbar"
        aria-label={t("fullAnalysis.screen.progressLabel")}
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={job.progress}
        aria-valuetext={`${job.progress}% · ${label}`}
        className="h-2 w-full overflow-hidden rounded-full bg-secondary"
      >
        <div
          className="h-full rounded-full bg-primary transition-[width] duration-300"
          style={{ width: `${job.progress}%` }}
        />
      </div>
      <p role="status" className="flex justify-between gap-3 text-xs text-muted-foreground">
        <span className="min-w-0 truncate">{label}</span>
        <span className="flex-none font-mono">{job.progress}%</span>
      </p>
      {job.status === "queued" && <p className="text-xs text-faint">{t("fullAnalysis.screen.queuedHint")}</p>}
    </div>
  );
}

/** The whole log, monospace and read-only, following its last line unless the reader scrolled up. */
function Log({ job }: { job: FullAnalysisJob }) {
  const { t } = useTranslation();
  const [open, setOpen] = useState(true);
  const box = useRef<HTMLDivElement>(null);
  const following = useRef(true);
  const id = useId();
  const lines = job.log.length;

  useEffect(() => {
    const element = box.current;
    if (element && following.current) element.scrollTop = element.scrollHeight;
  }, [lines, open]);

  return (
    <div className="grid w-full gap-1.5">
      <button
        type="button"
        aria-expanded={open}
        aria-controls={id}
        onClick={() => setOpen((current) => !current)}
        className="cursor-pointer justify-self-start rounded-[4px] text-xs text-muted-foreground outline-none hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
      >
        {open ? "▾" : "▸"} {t("fullAnalysis.screen.details")}
      </button>
      {open && (
        <div
          id={id}
          ref={box}
          role="log"
          aria-label={t("fullAnalysis.screen.log")}
          tabIndex={0}
          onScroll={(event) => {
            const element = event.currentTarget;
            following.current = element.scrollHeight - element.scrollTop - element.clientHeight < 24;
          }}
          className="h-56 overflow-auto rounded-[8px] border bg-card px-3 py-2 font-mono text-xs leading-[1.6] outline-none focus-visible:outline-2 focus-visible:outline-ring"
        >
          {job.log.map((line, index) => (
            <div key={index} className="flex gap-3 whitespace-pre-wrap">
              <span className="flex-none text-faint">{logTime(line.at)}</span>
              <span className="min-w-0 break-words">{logText(line)}</span>
            </div>
          ))}
        </div>
      )}
    </div>
  );
}
