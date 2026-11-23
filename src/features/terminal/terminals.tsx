import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

import {
  attachTerminal,
  closeTerminal,
  listTerminals,
  openTerminal,
  resizeTerminal,
  type ReviewTarget,
  type TerminalEvent,
  type TerminalKind,
  type TerminalMessage,
  type TerminalSession,
  writeTerminal,
} from "@/lib/api";

/** What a mounted terminal view is given of its session: the bytes it prints and its events. */
export interface TerminalListener {
  output: (data: Uint8Array) => void;
  event: (event: TerminalEvent) => void;
}

/** The worktree a screen offers to create before its first session can open, and how creating it goes. */
export type CreateStep =
  | { kind: TerminalKind; phase: "ask" }
  | { kind: TerminalKind; phase: "creating"; excluded: boolean }
  | { kind: TerminalKind; phase: "failed"; message: string };

/** How one screen shows its terminal: kept per screen while the app runs, so going back finds it as it was. */
export interface PanelState {
  expanded: boolean;
  /** The session shown when expanded. */
  active: string | null;
  step: CreateStep | null;
}

const COLLAPSED: PanelState = { expanded: false, active: null, step: null };

export const MIN_HEIGHT = 140;
const DEFAULT_HEIGHT = 300;
const HEIGHT_KEY = "difflens.terminal.height";

interface TerminalsValue {
  /** Every session of the core, oldest first. */
  sessions: TerminalSession[];
  /** Sessions that printed while no view showed them. */
  activity: ReadonlySet<string>;
  open: (projectId: string, target: ReviewTarget | null, kind: TerminalKind) => Promise<TerminalSession>;
  close: (id: string) => Promise<void>;
  /** Shows a session in a view: its scrollback first, then what it prints. Returns the function that stops. */
  view: (id: string, listener: TerminalListener) => () => void;
  write: (id: string, data: string) => void;
  resize: (id: string, cols: number, rows: number) => void;
  panel: (screen: string) => PanelState;
  updatePanel: (screen: string, change: (panel: PanelState) => PanelState) => void;
  /** Height of the expanded panel in pixels, the same on every screen and remembered across runs. */
  height: number;
  setHeight: (height: number) => void;
}

const nothing = () => {};

/**
 * Outside a provider (a screen tested on its own) there are no sessions and nothing can be opened. Screens read the
 * sessions (the project screen counts those in each worktree) without needing the terminal to be there.
 */
const NO_TERMINALS: TerminalsValue = {
  sessions: [],
  activity: new Set(),
  open: () => Promise.reject(new Error("No terminal provider.")),
  close: () => Promise.resolve(),
  view: () => nothing,
  write: nothing,
  resize: nothing,
  panel: () => COLLAPSED,
  updatePanel: nothing,
  height: DEFAULT_HEIGHT,
  setHeight: nothing,
};

const TerminalsContext = createContext<TerminalsValue>(NO_TERMINALS);

export const useTerminals = () => useContext(TerminalsContext);

function readHeight(): number {
  try {
    const stored = Number(window.localStorage.getItem(HEIGHT_KEY));
    return Number.isFinite(stored) && stored >= MIN_HEIGHT ? stored : DEFAULT_HEIGHT;
  } catch {
    return DEFAULT_HEIGHT;
  }
}

/** Pending input of a session, sent one call at a time so the core writes it in the order it was typed. */
interface InputQueue {
  pending: string;
  sending: boolean;
}

/**
 * The terminal sessions of the app, for every screen. Sessions live in the core; this follows them through one
 * channel each, so a session keeps reporting (its new output, its foreground command, its exit) whatever screen is
 * shown, and tells a view mounting on it where its output goes. Each attach makes a new channel the current one; what
 * an older channel still delivers is dropped, so a view never gets the same bytes twice. After a reload of the window
 * the sessions are listed again and followed without their past.
 */
export function TerminalsProvider({ children }: { children: ReactNode }) {
  const [sessions, setSessions] = useState<TerminalSession[]>([]);
  const [activity, setActivity] = useState<ReadonlySet<string>>(() => new Set());
  const [panels, setPanels] = useState<Readonly<Record<string, PanelState>>>({});
  const [height, setStoredHeight] = useState(readHeight);
  const current = useRef(new Map<string, number>());
  const listeners = useRef(new Map<string, TerminalListener>());
  const inputs = useRef(new Map<string, InputQueue>());
  const tokens = useRef(0);

  const patch = useCallback((id: string, change: Partial<TerminalSession>) => {
    setSessions((list) => list.map((session) => (session.id === id ? { ...session, ...change } : session)));
  }, []);

  const unmark = useCallback((id: string) => {
    setActivity((marked) => {
      if (!marked.has(id)) return marked;
      const next = new Set(marked);
      next.delete(id);
      return next;
    });
  }, []);

  const deliver = useCallback(
    (id: string, token: number, message: TerminalMessage) => {
      if (current.current.get(id) !== token) return;
      const listener = listeners.current.get(id);
      if (message.kind === "output") {
        if (listener) listener.output(message.data);
        else setActivity((marked) => (marked.has(id) ? marked : new Set(marked).add(id)));
        return;
      }
      const { event } = message;
      if (event.type === "foreground") patch(id, { foreground: event.command });
      else patch(id, { ended: true, exitCode: event.code, foreground: null });
      listener?.event(event);
    },
    [patch],
  );

  /** A new channel for `id`, from now on the only one delivered. */
  const follow = useCallback(
    (id: string) => {
      const token = ++tokens.current;
      current.current.set(id, token);
      return (message: TerminalMessage) => deliver(id, token, message);
    },
    [deliver],
  );

  useEffect(() => {
    let cancelled = false;
    listTerminals().then(
      (listed) => {
        if (cancelled || listed.length === 0) return;
        setSessions((known) => [...listed.filter((session) => !known.some(({ id }) => id === session.id)), ...known]);
        for (const session of listed) {
          if (!current.current.has(session.id)) void attachTerminal(session.id, false, follow(session.id)).catch(nothing);
        }
      },
      // Without the core's list the app still works; sessions opened from now on are followed.
      nothing,
    );
    return () => {
      cancelled = true;
    };
  }, [follow]);

  const open = useCallback(
    async (projectId: string, target: ReviewTarget | null, kind: TerminalKind) => {
      // The channel can deliver before the call resolves with the session's id.
      const token = ++tokens.current;
      let id: string | null = null;
      const early: TerminalMessage[] = [];
      const session = await openTerminal(projectId, target, kind, (message) => {
        if (id === null) early.push(message);
        else deliver(id, token, message);
      });
      id = session.id;
      current.current.set(id, token);
      setSessions((list) => [...list, session]);
      for (const message of early) deliver(id, token, message);
      return session;
    },
    [deliver],
  );

  const close = useCallback(
    async (id: string) => {
      current.current.delete(id);
      listeners.current.delete(id);
      inputs.current.delete(id);
      setSessions((list) => list.filter((session) => session.id !== id));
      unmark(id);
      try {
        await closeTerminal(id);
      } catch {
        // It may still run: show what the core has.
        setSessions(await listTerminals().catch(() => []));
      }
    },
    [unmark],
  );

  const view = useCallback(
    (id: string, listener: TerminalListener) => {
      listeners.current.set(id, listener);
      unmark(id);
      void attachTerminal(id, true, follow(id)).then(
        ({ foreground, ended, exitCode }) => patch(id, { foreground, ended, exitCode }),
        nothing,
      );
      return () => {
        if (listeners.current.get(id) === listener) listeners.current.delete(id);
      };
    },
    [follow, patch, unmark],
  );

  const write = useCallback((id: string, data: string) => {
    const queue = inputs.current.get(id) ?? { pending: "", sending: false };
    inputs.current.set(id, queue);
    queue.pending += data;
    if (queue.sending) return;
    queue.sending = true;
    void (async () => {
      while (queue.pending) {
        const chunk = queue.pending;
        queue.pending = "";
        try {
          await writeTerminal(id, chunk);
        } catch {
          // The session is gone; what was typed into it goes with it.
          queue.pending = "";
        }
      }
      queue.sending = false;
    })();
  }, []);

  const resize = useCallback((id: string, cols: number, rows: number) => {
    void resizeTerminal(id, cols, rows).catch(nothing);
  }, []);

  const panel = useCallback((screen: string) => panels[screen] ?? COLLAPSED, [panels]);
  const updatePanel = useCallback((screen: string, change: (panel: PanelState) => PanelState) => {
    setPanels((all) => ({ ...all, [screen]: change(all[screen] ?? COLLAPSED) }));
  }, []);

  const setHeight = useCallback((next: number) => {
    setStoredHeight(next);
    try {
      window.localStorage.setItem(HEIGHT_KEY, String(next));
    } catch {
      // Storage can be unavailable; the height still applies for this run.
    }
  }, []);

  const value = useMemo<TerminalsValue>(
    () => ({ sessions, activity, open, close, view, write, resize, panel, updatePanel, height, setHeight }),
    [sessions, activity, open, close, view, write, resize, panel, updatePanel, height, setHeight],
  );
  return <TerminalsContext value={value}>{children}</TerminalsContext>;
}
