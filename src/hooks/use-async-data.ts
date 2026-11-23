import { useCallback, useEffect, useState } from "react";

import { errorMessage } from "@/lib/format";

export type AsyncState<T> =
  | { status: "loading" }
  | { status: "ready"; data: T }
  | { status: "error"; message: string };

interface AsyncData<T> {
  state: AsyncState<T>;
  reload: () => void;
  /** Replaces the loaded data without calling the loader again. Ignored unless the state is ready. */
  update: (change: (data: T) => T) => void;
}

/** Runs `load` on mount and whenever its identity changes; wrap it in useCallback. */
export function useAsyncData<T>(load: () => Promise<T>): AsyncData<T> {
  const [state, setState] = useState<AsyncState<T>>({ status: "loading" });
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let cancelled = false;
    setState({ status: "loading" });
    load().then(
      (data) => {
        if (!cancelled) setState({ status: "ready", data });
      },
      (error: unknown) => {
        if (!cancelled) setState({ status: "error", message: errorMessage(error) });
      },
    );
    return () => {
      cancelled = true;
    };
  }, [load, attempt]);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);
  const update = useCallback((change: (data: T) => T) => {
    setState((current) =>
      current.status === "ready" ? { status: "ready", data: change(current.data) } : current,
    );
  }, []);

  return { state, reload, update };
}
