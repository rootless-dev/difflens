import { type ReactNode, useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote, ListPanel, Page, PageTitle, PanelNote, SectionTitle } from "@/components/page";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { isActive, useFullAnalyses } from "@/features/full-analysis/full-analyses";
import { useJiraSite } from "@/features/jira/jira-links";
import { BranchFilters } from "@/features/project/BranchFilters";
import { BranchRow, ListAction, ListNote, WorkingTreeRow } from "@/features/project/BranchLists";
import { DiffLensWorktrees } from "@/features/project/DiffLensWorktrees";
import { ACTIVE_DAYS, countBranches, groupBranches, reviewableBranches } from "@/features/project/branches";
import type { CompareTarget } from "@/features/project/compare-queue";
import { FetchControl, FetchOutcome } from "@/features/project/FetchControl";
import { RemoveProject } from "@/features/project/RemoveProject";
import { useCompareQueue } from "@/features/project/use-compare-queue";
import { useProjectFetch } from "@/features/project/use-project-fetch";
import type { ReviewOpening } from "@/features/review/views";
import { useAsyncData } from "@/hooks/use-async-data";
import { useNow } from "@/hooks/use-now";
import {
  type Branch,
  compareWorkingTree,
  type FullAnalysisJob,
  getProject,
  type ProjectDetail,
  removeProject,
  type ReviewTarget,
  setBranchBase,
  type TerminalKind,
} from "@/lib/api";
import { languageName } from "@/lib/format";

interface ProjectScreenProps {
  projectId: string;
  /** Name known from the list, shown until the project itself has loaded. */
  projectName: string;
  onBack: () => void;
  /**
   * Opens the review screen on a branch or on the uncommitted changes, on `opening`'s tab when given, with a terminal
   * session of `terminal`'s kind opening in it when given.
   */
  onReview: (target: ReviewTarget, opening?: ReviewOpening, terminal?: TerminalKind) => void;
  /** Offered by a full analysis's confirmation when a model is not set up. */
  onOpenPreferences?: () => void;
}

export function ProjectScreen({ projectId, projectName, onBack, onReview, onOpenPreferences }: ProjectScreenProps) {
  const { t } = useTranslation();
  const load = useCallback(() => getProject(projectId), [projectId]);
  const { state, reload, update } = useAsyncData(load);

  const remove = useCallback(async () => {
    await removeProject(projectId);
    onBack();
  }, [projectId, onBack]);

  // Reloads the detail in place, keeping the page on screen while it loads.
  const refresh = useCallback(async () => {
    const detail = await getProject(projectId);
    update(() => detail);
  }, [projectId, update]);

  const replaceBranch = useCallback(
    (branch: Branch) =>
      update((detail) => ({
        ...detail,
        branches: detail.branches.map((current) => (current.name === branch.name ? branch : current)),
      })),
    [update],
  );

  return (
    <Page>
      <Button variant="link" size="inline" className="-mt-4 mb-3.5 -ml-0.5 flex" onClick={onBack}>
        {t("project.allProjects")}
      </Button>
      {state.status === "loading" && (
        <>
          <PageTitle>{projectName}</PageTitle>
          <div role="status" aria-label={t("project.loading")} className="mt-3 grid gap-4">
            <Skeleton className="h-4 w-1/2" />
            <Skeleton className="h-24 w-full rounded-[10px]" />
          </div>
        </>
      )}
      {state.status === "error" && (
        <>
          <PageTitle>{projectName}</PageTitle>
          <ErrorNote className="mt-3">{state.message}</ErrorNote>
          <Button variant="secondary" size="sm" className="mt-2.5" onClick={reload}>
            {t("common.actions.tryAgain")}
          </Button>
          <Footer>
            <RemoveProject projectName={projectName} onRemove={remove} />
          </Footer>
        </>
      )}
      {state.status === "ready" && (
        <LoadedProject
          detail={state.data}
          onRefresh={refresh}
          onBranchChanged={replaceBranch}
          onRemove={remove}
          onReview={onReview}
          onOpenPreferences={onOpenPreferences}
        />
      )}
    </Page>
  );
}

interface LoadedProjectProps {
  detail: ProjectDetail;
  onRefresh: () => Promise<void>;
  onBranchChanged: (branch: Branch) => void;
  onRemove: () => Promise<void>;
  onReview: (target: ReviewTarget, opening?: ReviewOpening, terminal?: TerminalKind) => void;
  onOpenPreferences?: () => void;
}

function LoadedProject({
  detail,
  onRefresh,
  onBranchChanged,
  onRemove,
  onReview,
  onOpenPreferences,
}: LoadedProjectProps) {
  const { t } = useTranslation();
  const { project, bases, workingTree } = detail;
  const now = useNow();
  const jiraSite = useJiraSite();
  const { jobFor } = useFullAnalyses();
  const [query, setQuery] = useState("");
  const [includeRemote, setIncludeRemote] = useState(false);
  const [showOlder, setShowOlder] = useState(false);
  const fetch = useProjectFetch(project, onRefresh);

  const filtering = query.trim() !== "";
  const groups = useMemo(
    () => groupBranches(detail, { query, includeRemote, now }),
    [detail, query, includeRemote, now],
  );
  const counts = useMemo(() => countBranches(reviewableBranches(detail)), [detail]);
  // A filter searches older branches too, without the extra click.
  const olderShown = filtering || showOlder;

  const rendered = [...groups.current, ...groups.active, ...(olderShown ? groups.older : [])];
  const compares = useCompareQueue(project.id, rendered.flatMap(compareTarget));

  const changeBase = (branch: Branch) => async (base: string | null) => {
    onBranchChanged(await setBranchBase(project.id, branch.name, base));
  };
  const localBranches = t("project.header.branches", { count: counts.local });
  const row = (branch: Branch, older = false) => {
    const target = compareTarget(branch)[0];
    // Ended ones too, until their review is opened: the row then says how it ended.
    const analysis = branch.base
      ? jobFor(project.id, { kind: "branch", branch: branch.name, base: branch.base.name })
      : null;
    return (
      <BranchRow
        key={branch.name}
        projectId={project.id}
        branch={branch}
        bases={bases}
        compare={target && compares.get(target)}
        older={older}
        onChangeBase={changeBase(branch)}
        onReview={onReview}
        worktrees={detail.worktrees}
        jiraSite={jiraSite}
        analysis={analysis}
        onOpenPreferences={onOpenPreferences}
      />
    );
  };
  const reviewOf = (name: string): ReviewTarget | null => {
    const base = detail.branches.find((branch) => branch.name === name)?.base;
    return base ? { kind: "branch", branch: name, base: base.name } : null;
  };

  return (
    <>
      <div className="flex items-start gap-4">
        <div className="min-w-0 flex-1">
          <PageTitle>{project.name}</PageTitle>
          <p className="mt-1 flex flex-wrap items-center gap-x-1.5 gap-y-1 text-muted-foreground">
            <code title={project.path}>{project.displayPath}</code>
            <span aria-hidden="true">·</span>
            <span>
              {project.languages.length === 0
                ? t("project.header.noLanguage")
                : project.languages.map(languageName).join(", ")}
            </span>
            <span aria-hidden="true">·</span>
            <span>
              {counts.remoteOnly > 0
                ? t("project.header.branchesWithRemoteOnly", { branches: localBranches, remoteOnly: counts.remoteOnly })
                : localBranches}
            </span>
            {project.analysisSupport === "diff_only" && (
              <Badge className="ml-1">{t("project.header.diffOnly")}</Badge>
            )}
          </p>
        </div>
        <FetchControl
          lastFetchedAt={project.lastFetchedAt}
          now={now}
          state={fetch.state}
          available={project.available}
          hasRemote={project.hasRemote}
          onFetch={fetch.fetch}
        />
        <Button variant="secondary" disabled title={t("project.header.settingsLater")}>
          {t("project.header.settings")}
        </Button>
      </div>
      <FetchOutcome state={fetch.state} />
      {!project.available && <ErrorNote className="mt-3">{t("project.header.folderGone")}</ErrorNote>}
      {project.available && project.analysisSupport === "diff_only" && (
        <p className="mt-3 text-muted-foreground">{t("project.header.diffOnlyNote")}</p>
      )}

      <BranchFilters
        query={query}
        onQueryChange={setQuery}
        includeRemote={includeRemote}
        onIncludeRemoteChange={setIncludeRemote}
      />

      <SectionTitle>{t("project.groups.checkedOut")}</SectionTitle>
      <ListPanel aria-label={t("project.groups.checkedOut")}>
        {groups.current.map((branch) => row(branch))}
        {groups.current.length === 0 && (
          <ListNote>
            {groups.hasCurrent ? t("project.groups.currentFiltered") : t("project.groups.noCurrent")}
          </ListNote>
        )}
        {workingTree.dirty && (
          <UncommittedChanges
            key={workingTree.filesChanged}
            projectId={project.id}
            filesChanged={workingTree.filesChanged}
            onReview={onReview}
          />
        )}
      </ListPanel>

      <SectionTitle note={t("project.groups.activeNote", { days: ACTIVE_DAYS })}>
        {t("project.groups.active")}
      </SectionTitle>
      <ListPanel aria-label={t("project.groups.active")}>
        {groups.active.map((branch) => row(branch))}
        {groups.active.length === 0 && (
          <ListNote>
            {filtering ? t("project.groups.activeFiltered") : t("project.groups.noActive", { days: ACTIVE_DAYS })}
          </ListNote>
        )}
      </ListPanel>

      <SectionTitle>{t("project.groups.older")}</SectionTitle>
      {groups.olderTotal === 0 ? (
        <PanelNote>{t("project.groups.noOlder")}</PanelNote>
      ) : (
        <ListPanel aria-label={t("project.groups.older")}>
          {olderShown && groups.older.map((branch) => row(branch, true))}
          {olderShown && groups.older.length === 0 && <ListNote>{t("project.groups.olderFiltered")}</ListNote>}
          {!filtering && (
            <ListAction aria-expanded={showOlder} onClick={() => setShowOlder(!showOlder)}>
              {showOlder
                ? t("project.groups.hideOlder")
                : t("project.groups.showOlder", { count: groups.olderTotal })}
            </ListAction>
          )}
        </ListPanel>
      )}

      <DiffLensWorktrees
        projectId={project.id}
        worktrees={detail.worktrees}
        reviewOf={reviewOf}
        onReview={(target) => onReview(target)}
        onRemoved={onRefresh}
      />

      <SectionTitle note={t("project.groups.integrationNote")}>{t("project.groups.integration")}</SectionTitle>
      <div className="flex flex-wrap items-center gap-2.5 rounded-[10px] border px-4 py-3 text-muted-foreground">
        {project.integrationBranches.map((name) => (
          <code key={name} className="rounded-[10px] border px-2 py-px text-foreground">
            {name}
          </code>
        ))}
        <Button
          variant="link"
          size="inline"
          disabled
          title={t("project.groups.settingsLater")}
          className="ml-auto text-primary"
        >
          {t("project.groups.changeInSettings")}
        </Button>
      </div>

      <Footer>
        <RemoveProject projectName={project.name} onRemove={onRemove} />
      </Footer>
    </>
  );
}

/** What to compare for a row: nothing without a base, or when nothing is ahead of it. */
function compareTarget(branch: Branch): CompareTarget[] {
  if (!branch.base || branch.ahead === 0) {
    return [];
  }
  return [
    {
      branch: branch.name,
      base: branch.base.name,
      baseSha: branch.base.sha,
      headSha: branch.head.sha,
    },
  ];
}

interface UncommittedChangesProps {
  projectId: string;
  filesChanged: number;
  onReview: (target: ReviewTarget) => void;
}

function UncommittedChanges({ projectId, filesChanged, onReview }: UncommittedChangesProps) {
  const load = useCallback(() => compareWorkingTree(projectId), [projectId]);
  const { state } = useAsyncData(load);
  const analysis = useActiveJob()(projectId, { kind: "working_tree" });
  return <WorkingTreeRow filesChanged={filesChanged} summary={state} onReview={onReview} analysis={analysis} />;
}

/** The full analysis running or waiting for a review of the project, if any. */
function useActiveJob(): (projectId: string, target: ReviewTarget) => FullAnalysisJob | null {
  const { jobFor } = useFullAnalyses();
  return (projectId, target) => {
    const job = jobFor(projectId, target);
    return job && isActive(job) ? job : null;
  };
}

function Footer({ children }: { children: ReactNode }) {
  return <div className="mt-10 border-t pt-3.5">{children}</div>;
}
