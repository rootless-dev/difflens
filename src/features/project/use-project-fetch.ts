import { useCallback, useEffect, useRef, useState } from "react";

import { i18n } from "@/i18n";
import { fetchProject, type Project } from "@/lib/api";
import { errorMessage } from "@/lib/format";

/** A project last fetched longer ago than this is fetched when its page opens. */
export const AUTO_FETCH_AFTER_MS = 10 * 60_000;

/** How long the "branches updated" note stays after a successful fetch. */
export const FETCH_NOTE_MS = 6_000;

export type FetchState =
  | { status: "idle" }
  | { status: "running" }
  | { status: "done"; branchesUpdated: number }
  | { status: "error"; message: string };

export function shouldAutoFetch(project: Project, now: Date): boolean {
  if (!project.available || !project.hasRemote) {
    return false;
  }
  if (project.lastFetchedAt === null) {
    return true;
  }
  const last = new Date(project.lastFetchedAt).getTime();
  return Number.isNaN(last) || now.getTime() - last > AUTO_FETCH_AFTER_MS;
}

interface ProjectFetch {
  state: FetchState;
  fetch: () => void;
}

/**
 * Fetches the project's remotes on demand, and once when the page opens if the last fetch is
 * stale. `refresh` reloads the project afterwards; the page keeps showing local data meanwhile.
 */
export function useProjectFetch(project: Project, refresh: () => Promise<void>): ProjectFetch {
  const [state, setState] = useState<FetchState>({ status: "idle" });
  const running = useRef(false);
  const projectId = project.id;

  const fetch = useCallback(async () => {
    if (running.current) {
      return;
    }
    running.current = true;
    setState({ status: "running" });
    try {
      const result = await fetchProject(projectId);
      try {
        await refresh();
        setState({ status: "done", branchesUpdated: result.branchesUpdated });
      } catch (error) {
        setState({
          status: "error",
          message: i18n.t("project.fetch.reloadFailed", { message: errorMessage(error) }),
        });
      }
    } catch (error) {
      setState({ status: "error", message: errorMessage(error) });
    } finally {
      running.current = false;
    }
  }, [projectId, refresh]);

  // Decided once, with the project as the page opened it; later reloads never trigger it again.
  const autoChecked = useRef(false);
  useEffect(() => {
    if (autoChecked.current) {
      return;
    }
    autoChecked.current = true;
    if (shouldAutoFetch(project, new Date())) {
      void fetch();
    }
  }, [project, fetch]);

  useEffect(() => {
    if (state.status !== "done") {
      return;
    }
    const timer = setTimeout(() => setState({ status: "idle" }), FETCH_NOTE_MS);
    return () => clearTimeout(timer);
  }, [state]);

  return { state, fetch: () => void fetch() };
}
