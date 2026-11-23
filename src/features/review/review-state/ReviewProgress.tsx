import { Trans, useTranslation } from "react-i18next";

interface ReviewProgressProps {
  reviewed: number;
  total: number;
  /** Why reading or saving the review state failed, if it did. */
  error: string | null;
  /** Findings of the Insights tab; nothing is said about them while there are none. */
  findings?: { classified: number; total: number } | null;
}

/**
 * "Reviewed 5 of 12 files", a thin bar and "3 of 5 findings classified", at the right of the tab row (the prototype's
 * `.progress`).
 */
export function ReviewProgress({ reviewed, total, error, findings = null }: ReviewProgressProps) {
  const { t } = useTranslation();
  const share = total === 0 ? 0 : Math.min(1, reviewed / total);
  const bold = <b className="font-medium text-foreground" />;
  return (
    <div className="flex min-w-0 flex-none items-center gap-2.5 px-4 pb-[9px] text-xs text-muted-foreground">
      {error && (
        <span role="alert" title={error} className="max-w-80 truncate text-destructive">
          {error}
        </span>
      )}
      <span>
        <Trans i18nKey="review.progress.reviewed" count={total} values={{ reviewed }} components={{ b: bold }} />
      </span>
      <div
        role="progressbar"
        aria-label={t("review.progress.label")}
        aria-valuemin={0}
        aria-valuemax={total}
        aria-valuenow={reviewed}
        className="h-[5px] w-[84px] overflow-hidden rounded-[3px] bg-border"
      >
        <i className="block h-full bg-add" style={{ width: `${share * 100}%` }} />
      </div>
      {findings && findings.total > 0 && (
        <span className="ml-2.5">
          <Trans
            i18nKey="review.progress.classified"
            count={findings.total}
            values={{ classified: findings.classified }}
            components={{ b: bold }}
          />
        </span>
      )}
    </div>
  );
}
