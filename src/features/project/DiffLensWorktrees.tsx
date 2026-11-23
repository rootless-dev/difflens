import { useState } from "react";
import { Trans, useTranslation } from "react-i18next";

import { ErrorNote, ListPanel, SectionTitle } from "@/components/page";
import {
  AlertDialog,
  AlertDialogAction,
  AlertDialogCancel,
  AlertDialogContent,
  AlertDialogDescription,
  AlertDialogFooter,
  AlertDialogHeader,
  AlertDialogTitle,
} from "@/components/ui/alert-dialog";
import { Button } from "@/components/ui/button";
import { SessionIcon } from "@/features/terminal/icons";
import { useTerminals } from "@/features/terminal/terminals";
import { type ApiError, isApiError, removeWorktree, type ReviewTarget, type Worktree } from "@/lib/api";
import { errorMessage, formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

interface DiffLensWorktreesProps {
  projectId: string;
  /** Every worktree of the repository; only DiffLens's are listed. */
  worktrees: Worktree[];
  /** The review of a branch, to go and close its terminals; null when it has no base. */
  reviewOf: (branch: string) => ReviewTarget | null;
  onReview: (target: ReviewTarget) => void;
  /** Reloads the project once a worktree is gone. */
  onRemoved: () => Promise<void>;
}

/**
 * The worktrees DiffLens created for terminals (design/terminal-panel.html), shown only when there is one: each with
 * its branch and path, when it was created, whether it has uncommitted changes and how many terminals are open in it.
 * Removing a clean one is immediate; one with uncommitted changes asks first, naming what is lost; one with a terminal
 * open is refused until its terminals are closed. The branch and its commits are never touched.
 */
export function DiffLensWorktrees({ projectId, worktrees, reviewOf, onReview, onRemoved }: DiffLensWorktreesProps) {
  const { t } = useTranslation();
  const { sessions } = useTerminals();
  const ours = worktrees.filter((worktree) => worktree.folder.kind === "difflens");
  const [busy, setBusy] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<Worktree | null>(null);
  const [removing, setRemoving] = useState<string | null>(null);
  const [status, setStatus] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  if (ours.length === 0 && status === null) return null;

  const terminalsIn = (worktree: Worktree) => sessions.filter((session) => session.cwd === worktree.folder.path);
  const label = (worktree: Worktree) => worktree.branch ?? t("project.worktrees.detached");

  async function remove(worktree: Worktree, force: boolean) {
    setConfirming(null);
    setError(null);
    setRemoving(worktree.folder.path);
    try {
      await removeWorktree(projectId, worktree.folder.path, force);
      setBusy(null);
      setStatus(
        t(force ? "project.worktrees.removedDirty" : "project.worktrees.removed", { branch: label(worktree) }),
      );
      await onRemoved();
    } catch (cause) {
      const code = isApiError(cause) ? (cause as ApiError).code : null;
      if (code === "terminals_open") setBusy(worktree.folder.path);
      else if (code === "worktree_dirty") setConfirming(worktree);
      else setError(t("project.worktrees.removeFailed", { message: errorMessage(cause) }));
    } finally {
      setRemoving(null);
    }
  }

  function requestRemove(worktree: Worktree) {
    setStatus(null);
    if (terminalsIn(worktree).length > 0) setBusy(worktree.folder.path);
    else if (worktree.changedFiles) setConfirming(worktree);
    else void remove(worktree, false);
  }

  return (
    <section aria-labelledby="difflens-worktrees">
      <SectionTitle
        note={<Trans i18nKey="project.worktrees.note" components={{ code: <code /> }} />}
      >
        <span id="difflens-worktrees">{t("project.worktrees.title")}</span>
      </SectionTitle>
      {ours.length > 0 && (
        <ListPanel aria-labelledby="difflens-worktrees">
          {ours.map((worktree) => {
            const open = terminalsIn(worktree);
            const target = worktree.branch ? reviewOf(worktree.branch) : null;
            return (
              <li
                key={worktree.folder.path}
                className="grid grid-cols-[minmax(0,1fr)_110px_200px_150px_76px] items-center gap-x-3.5 gap-y-1 px-4 py-2.5 max-[1100px]:grid-cols-[minmax(0,1fr)_190px_76px]"
              >
                <span className="min-w-0">
                  <code className="block truncate font-medium">{label(worktree)}</code>
                  <small title={worktree.folder.path} className="block truncate font-mono text-[11.5px] text-faint">
                    {worktree.folder.relativePath}
                  </small>
                </span>
                <span className="text-xs whitespace-nowrap text-muted-foreground max-[1100px]:hidden">
                  {worktree.createdAt &&
                    t("project.worktrees.created", { when: formatRelativeTime(worktree.createdAt) })}
                </span>
                {worktree.changedFiles === null ? (
                  <span className="text-xs text-faint">{t("project.worktrees.unknownState")}</span>
                ) : worktree.changedFiles > 0 ? (
                  <span className="text-xs whitespace-nowrap text-mod">
                    {t("project.worktrees.dirty", { count: worktree.changedFiles })}
                  </span>
                ) : (
                  <span className="text-xs text-faint">{t("project.worktrees.clean")}</span>
                )}
                <span
                  title={open.map(({ name }) => name).join(", ") || undefined}
                  className={cn(
                    "flex items-center gap-1.5 text-xs whitespace-nowrap max-[1100px]:hidden",
                    open.length > 0 ? "text-primary" : "text-faint",
                  )}
                >
                  {open.length > 0 ? (
                    <>
                      <SessionIcon kind="shell" className="text-primary" />
                      {t("project.worktrees.terminals", { count: open.length })}
                    </>
                  ) : (
                    t("project.worktrees.noTerminals")
                  )}
                </span>
                <Button
                  variant="secondary"
                  size="sm"
                  className="justify-self-end"
                  aria-label={t("project.worktrees.removeLabel", { branch: label(worktree) })}
                  disabled={removing === worktree.folder.path}
                  onClick={() => requestRemove(worktree)}
                >
                  {removing === worktree.folder.path ? t("project.worktrees.removing") : t("project.worktrees.remove")}
                </Button>
                {busy === worktree.folder.path && (
                  <p
                    role="alert"
                    className="col-span-full mt-1.5 flex flex-wrap items-center gap-x-3 gap-y-1.5 rounded-[7px] bg-del-bg px-2.5 py-[7px] text-xs text-del"
                  >
                    <span className="text-foreground">
                      {t("project.worktrees.busy", { names: open.map(({ name }) => name).join(", ") })}
                    </span>
                    {target && (
                      <Button variant="link" size="inline" className="text-xs text-primary" onClick={() => onReview(target)}>
                        {t("project.worktrees.openReview")}
                      </Button>
                    )}
                  </p>
                )}
              </li>
            );
          })}
        </ListPanel>
      )}
      {status && (
        <p role="status" className="mt-2 text-xs text-faint">
          {status}
        </p>
      )}
      {error && <ErrorNote className="mt-2">{error}</ErrorNote>}
      <AlertDialog
        open={confirming !== null}
        onOpenChange={(open) => {
          if (!open) setConfirming(null);
        }}
      >
        <AlertDialogContent>
          {confirming && (
            <>
              <AlertDialogHeader>
                <AlertDialogTitle>{t("project.worktrees.dirtyTitle", { branch: label(confirming) })}</AlertDialogTitle>
                <AlertDialogDescription>
                  <span className="block">
                    <Trans
                      i18nKey="project.worktrees.dirtyText"
                      count={confirming.changedFiles ?? 0}
                      values={{ path: confirming.folder.relativePath, count: confirming.changedFiles ?? 0 }}
                      components={{ code: <code className="text-foreground" /> }}
                    />
                  </span>
                  {confirming.branch && (
                    <span className="mt-2 block">
                      <Trans
                        i18nKey="project.worktrees.dirtyBranch"
                        values={{ branch: confirming.branch }}
                        components={{ code: <code className="text-foreground" /> }}
                      />
                    </span>
                  )}
                </AlertDialogDescription>
              </AlertDialogHeader>
              <AlertDialogFooter>
                <AlertDialogCancel autoFocus>{t("common.actions.cancel")}</AlertDialogCancel>
                <AlertDialogAction variant="destructive" onClick={() => void remove(confirming, true)}>
                  {t("project.worktrees.removeAnyway")}
                </AlertDialogAction>
              </AlertDialogFooter>
            </>
          )}
        </AlertDialogContent>
      </AlertDialog>
    </section>
  );
}
