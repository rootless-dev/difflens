import { open } from "@tauri-apps/plugin-dialog";
import { useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote, Page, PageTitle, PanelNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { CloneBox } from "@/features/projects/CloneBox";
import { ProjectList } from "@/features/projects/ProjectList";
import { ProjectToolbar } from "@/features/projects/ProjectToolbar";
import {
  languageOptions,
  matchesLanguages,
  matchesQuery,
  sortProjects,
  useProjectFilters,
} from "@/features/projects/project-filters";
import { useAsyncData } from "@/hooks/use-async-data";
import { addLocalProject, cloneProject, listProjects, type Project } from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { isTyping } from "@/lib/utils";

interface ProjectsScreenProps {
  onOpenProject: (project: Project) => void;
}

const pickFolder = () => open({ directory: true });

export function ProjectsScreen({ onOpenProject }: ProjectsScreenProps) {
  const { t } = useTranslation();
  const { state, reload, update } = useAsyncData(listProjects);
  const [adding, setAdding] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  const [cloneOpen, setCloneOpen] = useState(false);
  const [cloning, setCloning] = useState(false);
  const [cloneError, setCloneError] = useState<string | null>(null);

  const prepend = (project: Project) => update((projects) => [project, ...projects]);

  async function addLocalFolder() {
    setAddError(null);
    try {
      const path = await pickFolder();
      if (path === null) {
        return;
      }
      setAdding(true);
      prepend(await addLocalProject(path));
    } catch (error) {
      setAddError(errorMessage(error));
    } finally {
      setAdding(false);
    }
  }

  async function clone(url: string, destination: string) {
    setCloneError(null);
    setCloning(true);
    try {
      prepend(await cloneProject(url, destination));
      setCloneOpen(false);
    } catch (error) {
      setCloneError(errorMessage(error));
    } finally {
      setCloning(false);
    }
  }

  function closeCloneBox() {
    setCloneOpen(false);
    setCloneError(null);
  }

  const listReady = state.status === "ready";

  return (
    <Page>
      <PageTitle>{t("projects.title")}</PageTitle>
      <p className="mt-1 mb-[22px] text-muted-foreground">{t("projects.description")}</p>
      <div className="mb-[22px]">
        <div className="flex gap-2.5">
          <Button onClick={addLocalFolder} disabled={adding || !listReady}>
            {adding ? t("projects.adding") : t("projects.addLocalFolder")}
          </Button>
          <Button
            variant="secondary"
            aria-expanded={cloneOpen}
            onClick={() => (cloneOpen ? closeCloneBox() : setCloneOpen(true))}
            disabled={cloning || !listReady}
          >
            {t("projects.cloneRepository")}
          </Button>
        </div>
        {addError && <ErrorNote className="mt-2.5">{addError}</ErrorNote>}
      </div>
      {cloneOpen && (
        <CloneBox
          pending={cloning}
          error={cloneError}
          onChooseFolder={pickFolder}
          onSubmit={clone}
          onCancel={closeCloneBox}
        />
      )}
      {state.status === "loading" && <ProjectListSkeleton />}
      {state.status === "error" && (
        <div className="rounded-[10px] border bg-card px-4 py-[13px]">
          <ErrorNote>{state.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={reload}>
            {t("common.actions.tryAgain")}
          </Button>
        </div>
      )}
      {state.status === "ready" &&
        (state.data.length === 0 ? (
          <PanelNote>
            <strong className="block font-medium text-foreground">{t("projects.empty.title")}</strong>
            {t("projects.empty.body")}
          </PanelNote>
        ) : (
          <ProjectBrowser projects={state.data} onOpen={onOpenProject} />
        ))}
    </Page>
  );
}

/**
 * The list with its search, language filter and sort. The sort and the languages are kept while the app runs; the
 * search starts empty each time. "/" or ⌘F (Ctrl+F) moves to the search unless another field is being typed in.
 */
function ProjectBrowser({ projects, onOpen }: { projects: Project[]; onOpen: (project: Project) => void }) {
  const { t } = useTranslation();
  const [query, setQuery] = useState("");
  const [filters, updateFilters] = useProjectFilters();
  const searchRef = useRef<HTMLInputElement>(null);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      const search = searchRef.current;
      if (event.defaultPrevented || event.altKey || search === null) return;
      const slash = event.key === "/" && !event.metaKey && !event.ctrlKey;
      const find = (event.metaKey || event.ctrlKey) && !event.shiftKey && event.key.toLowerCase() === "f";
      if (!slash && !find) return;
      // A field keeps the keys typed in it; ⌘F in the search itself selects what is there.
      if (isTyping(event.target) && (slash || event.target !== search)) return;
      event.preventDefault();
      search.focus();
      search.select();
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const options = languageOptions(projects);
  // A language remembered from before may have left with its last project.
  const languages = filters.languages.filter((id) => options.some((option) => option.id === id));
  const visible = sortProjects(
    projects.filter((project) => matchesQuery(project, query) && matchesLanguages(project, languages)),
    filters.sort,
  );

  function clearFilters() {
    setQuery("");
    updateFilters({ languages: [] });
    searchRef.current?.focus();
  }

  return (
    <>
      <ProjectToolbar
        query={query}
        onQueryChange={setQuery}
        searchRef={searchRef}
        options={options}
        languages={languages}
        onLanguagesChange={(next) => updateFilters({ languages: next })}
        sort={filters.sort}
        onSortChange={(sort) => updateFilters({ sort })}
      />
      {visible.length === 0 ? (
        <PanelNote>
          <strong className="block font-medium text-foreground">{t("projects.filters.noMatch")}</strong>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={clearFilters}>
            {t("projects.filters.clear")}
          </Button>
        </PanelNote>
      ) : (
        <ProjectList projects={visible} onOpen={onOpen} />
      )}
    </>
  );
}

function ProjectListSkeleton() {
  const { t } = useTranslation();
  return (
    <div
      role="status"
      aria-label={t("projects.loading")}
      className="grid gap-4 rounded-[10px] border bg-card px-4 py-4"
    >
      <Skeleton className="h-4 w-2/3" />
      <Skeleton className="h-4 w-1/2" />
      <Skeleton className="h-4 w-3/5" />
    </div>
  );
}
