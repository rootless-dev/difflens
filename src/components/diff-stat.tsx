import { useTranslation } from "react-i18next";

import { cn } from "@/lib/utils";

interface DiffStatProps {
  filesChanged: number;
  additions: number;
  deletions: number;
  className?: string;
}

export function DiffStat({ filesChanged, additions, deletions, className }: DiffStatProps) {
  const { t } = useTranslation();
  return (
    <span className={cn("font-mono text-xs text-muted-foreground", className)}>
      {t("app.diffStat.files", { count: filesChanged })} <span className="text-add">+{additions}</span>{" "}
      <span className="text-del">−{deletions}</span>
    </span>
  );
}
