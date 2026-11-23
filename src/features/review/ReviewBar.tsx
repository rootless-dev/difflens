import type { ReactNode } from "react";
import { Trans, useTranslation } from "react-i18next";

import { DiffStat } from "@/components/diff-stat";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { CardChip } from "@/features/review/card/CardChip";
import type { CardStatus } from "@/features/review/card/use-card";
import { PullRequestChip } from "@/features/review/pull-request/PullRequestChip";
import type { PullRequestStatus } from "@/features/review/pull-request/pull-request-status";
import { baseLabel, shortSha } from "@/features/review/review-target";
import type { AsyncState } from "@/hooks/use-async-data";
import { i18n } from "@/i18n";
import type { ChangeSet, ReviewTarget } from "@/lib/api";

interface ReviewBarProps {
  target: ReviewTarget;
  changeSet: AsyncState<ChangeSet>;
  onReload: () => void;
  /** The review's pull request, or why there is none. Not shown until the change set is read. */
  pullRequest?: PullRequestStatus;
  onOpenPreferences?: () => void;
  /** Approve and Request changes, next to the pull request. */
  verdicts?: ReactNode;
  /** The review's Jira card, shown before the pull request. */
  card?: CardStatus;
}

/**
 * Review part of the top bar: what is compared, its size, its pull request, the frozen sides and Reload.
 *
 * The bar sits in the top bar's size container. As it narrows, the parts that are said elsewhere or matter least go
 * first, so nothing overlaps: the frozen sides (Reload's title has them), reviewers' names (each mark's title has
 * them), the pull request's title (the link's title has it), the size (the progress bar under the tabs counts the
 * files), the compared chip (the breadcrumb names the branch), the reviewers and the pull request's state, then the
 * card (the branch name carries its key). The pull request link, the verdict buttons and Reload always stay. Each
 * breakpoint is where what is left fits, measured with the worst case (a card, five reviewers, "Changes requested").
 * The Portuguese labels of the parts that always stay ("Pedir alterações", "Alterações pedidas", "Recarregar") are
 * kept short so the same breakpoints hold in both languages; the compared chip reads "x → y" there.
 */
export function ReviewBar({
  target,
  changeSet,
  onReload,
  pullRequest,
  onOpenPreferences,
  verdicts,
  card,
}: ReviewBarProps) {
  const { t } = useTranslation();
  // The base the change set was read against (origin's branch, or the pull request's destination), once it is read.
  const base = baseLabel(target, changeSet.status === "ready" ? changeSet.data : null);
  return (
    <>
      <span
        data-testid="compared"
        title={target.kind === "branch" ? t("review.bar.comparedTitle", { branch: target.branch, base }) : undefined}
        className="flex min-w-0 items-center gap-1.5 rounded-sm border bg-secondary px-2.5 py-1 font-mono text-xs whitespace-nowrap @max-[52rem]:hidden"
      >
        {target.kind === "branch" ? (
          <Trans
            i18nKey="review.bar.comparedBranch"
            values={{ branch: target.branch, base }}
            components={{ name: <span className="truncate" />, faint: <span className="text-faint" /> }}
          />
        ) : (
          <Trans
            i18nKey="review.bar.comparedWorkingTree"
            components={{ plain: <span />, faint: <span className="text-faint" /> }}
          />
        )}
      </span>
      {changeSet.status === "loading" && (
        <Skeleton role="status" aria-label={t("review.bar.countingChanges")} className="h-3.5 w-28" />
      )}
      {changeSet.status === "ready" && <ChangeStat changeSet={changeSet.data} />}
      {changeSet.status === "ready" && card && <CardChip status={card} />}
      {changeSet.status === "ready" && pullRequest && (
        <PullRequestChip status={pullRequest} onOpenPreferences={onOpenPreferences} />
      )}
      {changeSet.status === "ready" && verdicts}
      <div className="flex-1" />
      {changeSet.status === "ready" && <Sides changeSet={changeSet.data} />}
      <Button
        variant="outline"
        size="sm"
        onClick={onReload}
        disabled={changeSet.status === "loading"}
        // Says what the sides chip would, once the bar is too narrow for it.
        title={changeSet.status === "ready" ? sidesTitle(changeSet.data) : undefined}
      >
        {t("review.bar.reload")}
      </Button>
    </>
  );
}

function ChangeStat({ changeSet }: { changeSet: ChangeSet }) {
  let additions = 0;
  let deletions = 0;
  for (const file of changeSet.files) {
    additions += file.additions;
    deletions += file.deletions;
  }
  return (
    <DiffStat
      filesChanged={changeSet.files.length}
      additions={additions}
      deletions={deletions}
      className="flex-none whitespace-nowrap @max-5xl:hidden"
    />
  );
}

function sidesTitle(changeSet: ChangeSet): string {
  const { oldSha, newSha, target } = changeSet;
  if (newSha === null) return i18n.t("review.bar.sides.titleWorkingTree", { oldSha });
  return target.kind === "branch"
    ? i18n.t("review.bar.sides.titleBranch", {
        newSha,
        oldSha,
        branch: target.branch,
        base: baseLabel(target, changeSet),
      })
    : i18n.t("review.bar.sides.titleCommits", { newSha, oldSha });
}

/** The commits the files were read from, frozen until Reload. */
function Sides({ changeSet }: { changeSet: ChangeSet }) {
  const { t } = useTranslation();
  const { newSha } = changeSet;
  return (
    <span
      title={sidesTitle(changeSet)}
      className="flex flex-none items-center gap-2 rounded-sm border px-2.5 py-[5px] text-xs whitespace-nowrap text-muted-foreground @max-[96rem]:hidden"
    >
      <i aria-hidden="true" className="size-2 rounded-full bg-add" />
      {newSha === null ? (
        t("review.bar.sides.workingTree")
      ) : (
        <span>
          <Trans i18nKey="review.bar.sides.at" values={{ sha: shortSha(newSha) }} components={{ code: <code /> }} />
        </span>
      )}
    </span>
  );
}
