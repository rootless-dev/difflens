import { LoaderCircle } from "lucide-react";
import { useTranslation } from "react-i18next";

import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { isActive, useFullAnalyses } from "@/features/full-analysis/full-analyses";
import { targetLabel } from "@/features/review/review-target";
import type { FullAnalysisJob } from "@/lib/api";

interface RunningAnalysesProps {
  /** Opens the review a full analysis is running for. */
  onOpen: (job: FullAnalysisJob) => void;
}

/**
 * The top bar's "2 analyses running", while any full analysis runs or waits: a menu of them, each with its progress
 * or "Queued", opening its review. Below 1440px of window only the spinner and the count show (the label is still
 * read and in the tooltip), so the review's part of the bar keeps the room its breakpoints were measured with.
 */
export function RunningAnalyses({ onOpen }: RunningAnalysesProps) {
  const { t } = useTranslation();
  const { jobs } = useFullAnalyses();
  const active = jobs.filter(isActive);
  if (active.length === 0) return null;
  const label = t("fullAnalysis.indicator.running", { count: active.length });
  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={label}
        title={label}
        className="inline-flex h-7 flex-none cursor-pointer items-center gap-1.5 rounded-[10px] border border-primary/40 px-2 text-xs whitespace-nowrap text-primary outline-none hover:border-primary focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring data-popup-open:border-primary"
      >
        <LoaderCircle aria-hidden="true" className="size-3.5 flex-none animate-spin" />
        <span aria-hidden="true" className="font-mono min-[1440px]:hidden">
          {active.length}
        </span>
        <span aria-hidden="true" className="max-[1439px]:hidden">
          {label}
        </span>
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="w-80">
        <DropdownMenuGroup>
          <DropdownMenuLabel>{t("fullAnalysis.indicator.title")}</DropdownMenuLabel>
          {active.map((job) => (
            <DropdownMenuItem
              key={job.id}
              aria-label={t("fullAnalysis.indicator.open", {
                review: `${job.projectName} / ${targetLabel(job.review.target)}`,
              })}
              onClick={() => onOpen(job)}
              className="grid grid-cols-[minmax(0,1fr)_auto] gap-x-3 gap-y-1 py-1.5"
            >
              <span className="min-w-0 truncate text-xs">
                <span className="text-muted-foreground">{job.projectName} / </span>
                <span className="font-mono">{targetLabel(job.review.target)}</span>
              </span>
              <span className="text-xs text-muted-foreground">
                {job.status === "queued"
                  ? t("fullAnalysis.indicator.queued")
                  : t("fullAnalysis.indicator.progress", { progress: job.progress })}
              </span>
              {job.status === "running" && (
                <span aria-hidden="true" className="col-span-2 h-1 overflow-hidden rounded-full bg-secondary">
                  <span className="block h-full rounded-full bg-primary" style={{ width: `${job.progress}%` }} />
                </span>
              )}
            </DropdownMenuItem>
          ))}
        </DropdownMenuGroup>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
