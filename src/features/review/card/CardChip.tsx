import { useTranslation } from "react-i18next";

import { ExternalLink } from "@/features/jira/jira-links";
import type { CardStatus } from "@/features/review/card/use-card";
import { i18n } from "@/i18n";

/**
 * The review's Jira card in the top bar (the prototype's `.ext` "PAY-482" link), before the pull request. A key that
 * cannot be opened (Jira not connected, no such card, a failed read) is shown quietly with the reason in its title;
 * without a key nothing is shown.
 */
export function CardChip({ status }: { status: CardStatus }) {
  const { t } = useTranslation();
  if (status.kind === "found") {
    const { card } = status;
    return (
      // Dropped from a very narrow bar, as ReviewBar lays out; the breadcrumb's branch name carries the key.
      <ExternalLink href={card.url} title={t("review.card.open", { summary: card.summary })} className="@max-xl:hidden">
        {card.key}
      </ExternalLink>
    );
  }
  const quiet = quietKey(status);
  if (quiet === null) return null;
  return (
    <span title={quiet.why} className="flex-none font-mono text-xs whitespace-nowrap text-muted-foreground @max-xl:hidden">
      {quiet.key}
    </span>
  );
}

function quietKey(status: CardStatus): { key: string; why: string } | null {
  switch (status.kind) {
    case "not_connected":
      return { key: status.key, why: i18n.t("review.card.notConnected") };
    case "not_found":
      return { key: status.key, why: i18n.t("review.card.notFound", { key: status.key }) };
    case "error":
      return status.key === null
        ? null
        : { key: status.key, why: i18n.t("review.card.unreadable", { message: status.message }) };
    default:
      return null;
  }
}
