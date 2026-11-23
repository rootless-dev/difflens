import { ChevronDownIcon } from "lucide-react";
import type { KeyboardEvent, Ref } from "react";
import { useTranslation } from "react-i18next";

import {
  DropdownMenu,
  DropdownMenuCheckboxItem,
  DropdownMenuContent,
  DropdownMenuGroup,
  DropdownMenuItem,
  DropdownMenuLabel,
  DropdownMenuSeparator,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { Input } from "@/components/ui/input";
import { Select, SelectContent, SelectItem, SelectTrigger, SelectValue } from "@/components/ui/select";
import { type LanguageOption, NO_LANGUAGE, type ProjectSort } from "@/features/projects/project-filters";
import { languageName } from "@/lib/format";
import { cn } from "@/lib/utils";

interface ProjectToolbarProps {
  query: string;
  onQueryChange: (query: string) => void;
  searchRef: Ref<HTMLInputElement>;
  options: LanguageOption[];
  /** Chosen ids among `options`. */
  languages: string[];
  onLanguagesChange: (languages: string[]) => void;
  sort: ProjectSort;
  onSortChange: (sort: ProjectSort) => void;
}

/** Search by name, the language filter and the sort, in one row above the project list. */
export function ProjectToolbar({
  query,
  onQueryChange,
  searchRef,
  options,
  languages,
  onLanguagesChange,
  sort,
  onSortChange,
}: ProjectToolbarProps) {
  const { t } = useTranslation();
  const sorts: { value: ProjectSort; label: string }[] = [
    { value: "recent", label: t("projects.filters.sort.recent") },
    { value: "oldest", label: t("projects.filters.sort.oldest") },
  ];

  function onSearchKeyDown(event: KeyboardEvent<HTMLInputElement>) {
    if (event.key === "Escape" && query !== "") {
      event.preventDefault();
      onQueryChange("");
    }
  }

  return (
    <div className="mb-3 flex items-center gap-2.5">
      <Input
        ref={searchRef}
        type="search"
        aria-label={t("projects.filters.search")}
        aria-keyshortcuts="/ Meta+F Control+F"
        placeholder={t("projects.filters.searchPlaceholder")}
        value={query}
        onChange={(event) => onQueryChange(event.target.value)}
        onKeyDown={onSearchKeyDown}
        className="h-[35px] flex-1 rounded-[7px] border-border bg-card px-3 font-sans text-sm"
      />
      <LanguageFilter options={options} chosen={languages} onChange={onLanguagesChange} />
      <Select
        items={sorts}
        value={sort}
        onValueChange={(next) => {
          if (next !== null) onSortChange(next);
        }}
      >
        <SelectTrigger aria-label={t("projects.filters.sort.label")} className="h-[35px] min-w-52">
          <SelectValue />
        </SelectTrigger>
        <SelectContent align="end">
          {sorts.map((option) => (
            <SelectItem key={option.value} value={option.value}>
              {option.label}
            </SelectItem>
          ))}
        </SelectContent>
      </Select>
    </div>
  );
}

interface LanguageFilterProps {
  options: LanguageOption[];
  chosen: string[];
  onChange: (languages: string[]) => void;
}

/** "All languages ▾": a menu of the languages found, any number of them ticked, each with its project count. */
function LanguageFilter({ options, chosen, onChange }: LanguageFilterProps) {
  const { t } = useTranslation();
  const label = (id: string) => (id === NO_LANGUAGE ? t("projects.filters.languages.none") : languageName(id));
  const summary =
    chosen.length === 0
      ? t("projects.filters.languages.all")
      : chosen.length === 1
        ? label(chosen[0])
        : t("projects.filters.languages.several", { count: chosen.length });

  function toggle(id: string, checked: boolean) {
    onChange(checked ? [...chosen, id] : chosen.filter((language) => language !== id));
  }

  return (
    <DropdownMenu>
      <DropdownMenuTrigger
        aria-label={t("projects.filters.languages.trigger", { summary })}
        className={cn(
          "flex h-[35px] cursor-pointer items-center gap-1.5 rounded-[7px] border bg-card px-2.5 whitespace-nowrap outline-none hover:border-faint focus-visible:outline-2 focus-visible:outline-offset-1 focus-visible:outline-ring data-popup-open:border-faint",
          chosen.length > 0 && "border-primary/60",
        )}
      >
        {summary}
        <ChevronDownIcon aria-hidden="true" className="size-4 shrink-0 text-muted-foreground" />
      </DropdownMenuTrigger>
      <DropdownMenuContent align="end" className="min-w-56">
        <DropdownMenuGroup>
          <DropdownMenuLabel>{t("projects.filters.languages.title")}</DropdownMenuLabel>
          {options.map((option) => (
            <DropdownMenuCheckboxItem
              key={option.id}
              checked={chosen.includes(option.id)}
              closeOnClick={false}
              onCheckedChange={(checked) => toggle(option.id, checked)}
              label={label(option.id)}
              aria-label={t("projects.filters.languages.option", { language: label(option.id), count: option.count })}
            >
              <span className="flex-1">{label(option.id)}</span>
              <span aria-hidden="true" className="text-xs text-muted-foreground tabular-nums">
                {option.count}
              </span>
            </DropdownMenuCheckboxItem>
          ))}
        </DropdownMenuGroup>
        <DropdownMenuSeparator />
        <DropdownMenuItem disabled={chosen.length === 0} onClick={() => onChange([])}>
          {t("projects.filters.languages.showAll")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}
