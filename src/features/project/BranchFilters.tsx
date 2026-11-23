import { useTranslation } from "react-i18next";

import { Hint } from "@/components/page";
import { Segment, SegmentGroup } from "@/components/segment";
import { Input } from "@/components/ui/input";

interface BranchFiltersProps {
  query: string;
  onQueryChange: (query: string) => void;
  includeRemote: boolean;
  onIncludeRemoteChange: (includeRemote: boolean) => void;
}

export function BranchFilters({ query, onQueryChange, includeRemote, onIncludeRemoteChange }: BranchFiltersProps) {
  const { t } = useTranslation();
  return (
    <>
      <div className="mt-6 flex items-center gap-3">
        <Input
          type="search"
          aria-label={t("project.filters.label")}
          placeholder={t("project.filters.placeholder")}
          value={query}
          onChange={(event) => onQueryChange(event.target.value)}
          className="h-[35px] flex-1 rounded-[7px] border-border bg-card px-3 font-sans text-sm"
        />
        <SegmentGroup label={t("project.filters.scope")}>
          <Segment pressed={!includeRemote} onClick={() => onIncludeRemoteChange(false)}>
            {t("project.filters.local")}
          </Segment>
          <Segment pressed={includeRemote} onClick={() => onIncludeRemoteChange(true)}>
            {t("project.filters.remote")}
          </Segment>
        </SegmentGroup>
      </div>
      <Hint className="mt-2.5">{t("project.filters.hint")}</Hint>
    </>
  );
}
