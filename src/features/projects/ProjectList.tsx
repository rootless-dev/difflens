import type { TFunction } from "i18next";
import { useTranslation } from "react-i18next";

import { ListPanel } from "@/components/page";
import { Badge } from "@/components/ui/badge";
import { useNow } from "@/hooks/use-now";
import type { Project } from "@/lib/api";
import { formatRelativeTime, languageName } from "@/lib/format";
import { cn } from "@/lib/utils";

interface ProjectListProps {
  projects: Project[];
  onOpen: (project: Project) => void;
}

export function ProjectList({ projects, onOpen }: ProjectListProps) {
  const { t } = useTranslation();
  const now = useNow();
  return (
    <ListPanel aria-label={t("projects.list.label")}>
      {projects.map((project) => (
        <li key={project.id}>
          <ProjectRow project={project} now={now} onOpen={() => onOpen(project)} />
        </li>
      ))}
    </ListPanel>
  );
}

function ProjectRow({ project, now, onOpen }: { project: Project; now: Date; onOpen: () => void }) {
  const { t } = useTranslation();
  const status = projectStatus(project, t);
  return (
    <button
      type="button"
      onClick={onOpen}
      className="grid w-full cursor-pointer grid-cols-[1fr_1.8fr_130px_180px_70px] items-center gap-4 px-4 py-[13px] text-left outline-none hover:bg-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring"
    >
      <span className={cn("min-w-0", !project.available && "opacity-55")}>
        <span className="block truncate font-medium">{project.name}</span>
        {project.lastModifiedAt && (
          <time dateTime={project.lastModifiedAt} className="block truncate text-xs text-faint">
            {t("projects.list.modified", { when: formatRelativeTime(project.lastModifiedAt, now) })}
          </time>
        )}
      </span>
      <span
        title={project.path}
        className={cn(
          "truncate font-mono text-xs text-muted-foreground",
          !project.available && "opacity-55",
        )}
      >
        {project.displayPath}
      </span>
      <span className={cn("flex flex-wrap gap-1.5", !project.available && "opacity-55")}>
        {project.languages.length === 0 ? (
          <span className="text-xs text-faint">{t("projects.list.noLanguage")}</span>
        ) : (
          project.languages.map((language) => (
            <Badge key={language}>{languageName(language)}</Badge>
          ))
        )}
      </span>
      <span className={status.className} title={status.title}>
        {status.text}
      </span>
      <span className="text-muted-foreground">{status.note}</span>
    </button>
  );
}

interface ProjectStatus {
  text: string;
  className: string;
  title?: string;
  note?: string;
}

function projectStatus(project: Project, t: TFunction): ProjectStatus {
  if (!project.available) {
    return {
      text: t("projects.list.unavailable"),
      className: "text-del",
      title: t("projects.list.unavailableTitle"),
    };
  }
  if (project.analysisSupport === "diff_only") {
    return {
      text: t("projects.list.languageNotSupported"),
      className: "text-muted-foreground",
      note: t("projects.list.diffOnly"),
    };
  }
  if (project.branchesAhead > 0) {
    return { text: t("projects.list.branchesToReview", { count: project.branchesAhead }), className: "text-mod" };
  }
  return { text: t("projects.list.nothingToReview"), className: "text-muted-foreground" };
}
