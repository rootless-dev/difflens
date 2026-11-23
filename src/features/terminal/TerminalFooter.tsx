import { ChevronDown, ChevronUp, Folder as FolderIcon, GitBranch, Plus, X } from "lucide-react";
import {
  type KeyboardEvent as ReactKeyboardEvent,
  type PointerEvent as ReactPointerEvent,
  type ReactNode,
  useCallback,
  useEffect,
  useRef,
  useState,
} from "react";
import { Trans, useTranslation } from "react-i18next";

import type { Route } from "@/app/route";
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
import {
  DropdownMenu,
  DropdownMenuContent,
  DropdownMenuItem,
  DropdownMenuShortcut,
  DropdownMenuTrigger,
} from "@/components/ui/dropdown-menu";
import { targetKey } from "@/features/review/review-target";
import { CreateWorktreeStep } from "@/features/terminal/CreateWorktreeStep";
import { SessionIcon } from "@/features/terminal/icons";
import { MIN_HEIGHT, type PanelState, useTerminals } from "@/features/terminal/terminals";
import {
  closingInterrupts,
  folderName,
  isTerminalShortcut,
  NEW_TERMINAL_KEYS,
  shortCommand,
  splitPath,
  tabLabel,
  TOGGLE_KEYS,
} from "@/features/terminal/terminal-text";
import { TerminalView } from "@/features/terminal/TerminalView";
import { useAsyncData } from "@/hooks/use-async-data";
import type { Theme } from "@/hooks/use-theme";
import {
  createWorktree,
  excludeDifflensFolder,
  getTerminalLocation,
  type ReviewTarget,
  type TerminalKind,
  type TerminalLocation,
  type TerminalSession,
} from "@/lib/api";
import { errorMessage } from "@/lib/format";
import { cn } from "@/lib/utils";

const PANEL_ID = "terminal-panel";
const HEIGHT_STEP = 24;
/** Room the expanded panel leaves to the top bar (46), the footer bar (32) and some of the screen above. */
const maxHeight = () => Math.max(160, window.innerHeight - 46 - 32 - 130);
const clampHeight = (height: number) => Math.round(Math.min(maxHeight(), Math.max(MIN_HEIGHT, height)));

interface TerminalFooterProps {
  projectId: string;
  /** The review shown; null on the project screen. */
  target: ReviewTarget | null;
  theme: Theme;
  /** Goes to the screen of sessions open elsewhere. */
  onNavigate: (route: Route) => void;
  /** Opens a session of this kind (or the worktree it needs) once the screen knows where: a branch's ⋮ menu. */
  request?: TerminalKind;
}

/**
 * The terminal of a screen (design/terminal-panel.html): a bar at its foot, always there like a status bar, with a tab
 * per session open where this screen's terminals open, the + for a new one, where that is, and how many sessions run
 * elsewhere. Expanded, the panel grows up from the bar and shows one session; its top edge sets its height. ⌃`
 * expands and collapses it, ⌃⇧` opens a new terminal. A review of a branch checked out nowhere first offers to create
 * its worktree; nothing is created before that click. Sessions belong to the core and outlive the screen.
 */
export function TerminalFooter({ projectId, target, theme, onNavigate, request }: TerminalFooterProps) {
  const { t } = useTranslation();
  const terminals = useTerminals();
  const screen = `${projectId}/${target ? targetKey(target) : "project"}`;
  const panel = terminals.panel(screen);
  const { updatePanel } = terminals;
  const setPanel = useCallback(
    (change: (panel: PanelState) => PanelState) => updatePanel(screen, change),
    [screen, updatePanel],
  );
  const key = target ? targetKey(target) : "project";
  // The identity of the target is its key; a new object for the same review asks nothing again.
  const targetRef = useRef(target);
  targetRef.current = target;
  const load = useCallback(() => getTerminalLocation(projectId, targetRef.current), [projectId, key]);
  const { state: locationState, update: updateLocation } = useAsyncData(load);
  const location = locationState.status === "ready" ? locationState.data : null;
  const [error, setError] = useState<string | null>(null);
  const [confirming, setConfirming] = useState<TerminalSession | null>(null);
  const [focusTarget, setFocusTarget] = useState<string | null>(null);

  const folder = location?.folder?.path ?? null;
  const here = terminals.sessions.filter((session) => folder !== null && session.cwd === folder);
  const others = terminals.sessions.filter((session) => session.cwd !== folder);
  const step = panel.step;
  const active = here.find((session) => session.id === panel.active) ?? here[0] ?? null;
  const anything = step !== null || here.length > 0;
  const expanded = panel.expanded && anything;

  async function startSession(kind: TerminalKind, where: TerminalLocation | null = location) {
    if (!where) return;
    if (!where.folder) {
      if (where.plan) setPanel((current) => ({ ...current, expanded: true, step: { kind, phase: "ask" } }));
      return;
    }
    setError(null);
    try {
      const session = await terminals.open(projectId, target, kind);
      setFocusTarget(session.id);
      setPanel((current) => ({ ...current, expanded: true, active: session.id, step: null }));
    } catch (cause) {
      setError(t("terminal.openFailed", { message: errorMessage(cause) }));
    }
  }

  function toggle() {
    if (expanded) {
      setPanel((current) => ({ ...current, expanded: false }));
    } else if (anything) {
      if (active) setFocusTarget(active.id);
      setPanel((current) => ({ ...current, expanded: true }));
    } else {
      void startSession("shell");
    }
  }

  function show(session: TerminalSession) {
    setFocusTarget(session.id);
    setPanel((current) => ({ ...current, expanded: true, active: session.id }));
  }

  async function close(session: TerminalSession) {
    setConfirming(null);
    const index = here.findIndex(({ id }) => id === session.id);
    const rest = here.filter(({ id }) => id !== session.id);
    const next = rest[index] ?? rest[index - 1] ?? null;
    // Closing the last tab leaves only the bar.
    setPanel((current) => ({
      ...current,
      active: current.active === session.id ? (next?.id ?? null) : current.active,
      expanded: rest.length > 0 && current.expanded,
    }));
    await terminals.close(session.id);
  }

  function requestClose(session: TerminalSession) {
    if (closingInterrupts(session)) setConfirming(session);
    else void close(session);
  }

  async function create() {
    if (!step || !location?.branch) return;
    const { kind } = step;
    const branch = location.branch;
    setPanel((current) => ({ ...current, step: { kind, phase: "creating", excluded: false } }));
    try {
      await excludeDifflensFolder(projectId);
      setPanel((current) => ({ ...current, step: { kind, phase: "creating", excluded: true } }));
      const created = await createWorktree(projectId, branch);
      updateLocation(() => created);
      await startSession(kind, created);
    } catch (cause) {
      setPanel((current) => ({ ...current, step: { kind, phase: "failed", message: errorMessage(cause) } }));
    }
  }

  // A request from a branch's ⋮ menu, once, when the screen knows where its terminals open.
  const requested = useRef(false);
  useEffect(() => {
    if (!request || requested.current || !location) return;
    requested.current = true;
    void startSession(request);
  });

  // ⌃` and ⌃⇧`, taken before anything else sees them, the terminal included.
  const shortcuts = useRef({ toggle, newShell: () => void startSession("shell") });
  shortcuts.current = { toggle, newShell: () => void startSession("shell") };
  const dialogOpen = confirming !== null;
  useEffect(() => {
    const onKey = (event: KeyboardEvent) => {
      if (!isTerminalShortcut(event) || dialogOpen) return;
      event.preventDefault();
      event.stopPropagation();
      if (event.shiftKey) shortcuts.current.newShell();
      else shortcuts.current.toggle();
    };
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [dialogOpen]);

  return (
    <>
      {expanded && (
        <Panel height={terminals.height} onHeight={terminals.setHeight}>
          {step && location?.plan && location.branch ? (
            <CreateWorktreeStep
              branch={location.branch}
              plan={location.plan}
              step={step}
              onCreate={() => void create()}
              onCancel={() => setPanel((current) => ({ ...current, step: null, expanded: false }))}
            />
          ) : (
            active && (
              <div role="tabpanel" aria-labelledby={`terminal-tab-${active.id}`} className="min-h-0 flex-1">
                <TerminalView
                  key={active.id}
                  session={active}
                  theme={theme}
                  autoFocus={focusTarget === active.id}
                />
              </div>
            )
          )}
        </Panel>
      )}
      <footer
        aria-label={t("terminal.region")}
        className="relative z-[7] flex h-8 flex-none items-stretch gap-0.5 border-t bg-card pr-2.5 pl-1 text-xs"
      >
        {anything && (
          <button
            type="button"
            aria-expanded={expanded}
            aria-controls={PANEL_ID}
            aria-label={expanded ? t("terminal.collapse") : t("terminal.expand")}
            title={t(expanded ? "terminal.collapseTitle" : "terminal.expandTitle", { keys: TOGGLE_KEYS })}
            onClick={toggle}
            className={ICON_BUTTON}
          >
            {expanded ? <ChevronDown className="size-3.5" /> : <ChevronUp className="size-3.5" />}
          </button>
        )}
        {anything && (
          <div role="tablist" aria-label={t("terminal.sessions")} className="flex min-w-0 overflow-x-auto [scrollbar-width:none]">
            {step ? (
              <PendingTab
                kind={step.kind}
                expanded={expanded}
                onCancel={() => setPanel((current) => ({ ...current, step: null, expanded: false }))}
              />
            ) : (
              here.map((session) => (
                <SessionTab
                  key={session.id}
                  session={session}
                  selected={session.id === active?.id}
                  shown={expanded && session.id === active?.id}
                  unseen={terminals.activity.has(session.id)}
                  onSelect={() => show(session)}
                  onClose={() => requestClose(session)}
                  onArrow={(direction) => {
                    const index = here.findIndex(({ id }) => id === session.id);
                    const next = here[(index + direction + here.length) % here.length];
                    show(next);
                    document.getElementById(`terminal-tab-${next.id}`)?.focus();
                  }}
                />
              ))
            )}
          </div>
        )}
        {!step && location && (location.folder || location.plan) && <AddMenu onNew={(kind) => void startSession(kind)} />}
        <span className="flex min-w-3 flex-1 items-center overflow-hidden px-2">
          {error && (
            <span role="alert" className="truncate text-destructive" title={error}>
              {error}
            </span>
          )}
        </span>
        <OthersChip sessions={others} onNavigate={onNavigate} />
        {location && <LocationLabel location={location} />}
        {locationState.status === "error" && (
          <span role="alert" className="self-center truncate text-destructive" title={locationState.message}>
            {locationState.message}
          </span>
        )}
      </footer>
      <CloseDialog
        session={confirming}
        onCancel={() => setConfirming(null)}
        onConfirm={(session) => void close(session)}
      />
    </>
  );
}

const ICON_BUTTON =
  "grid size-6 flex-none cursor-pointer place-items-center self-center rounded-md text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring aria-expanded:bg-secondary aria-expanded:text-foreground";

interface PanelProps {
  height: number;
  onHeight: (height: number) => void;
  children: ReactNode;
}

/** The expanded panel, above the bar; its top edge drags (or, focused, the arrows change) its height. */
function Panel({ height, onHeight, children }: PanelProps) {
  const { t } = useTranslation();
  const [dragging, setDragging] = useState(false);
  const drag = useRef<{ y: number; height: number } | null>(null);
  const shown = clampHeight(height);

  // Keeps the panel within the window when the window gets smaller.
  useEffect(() => {
    const onResize = () => {
      if (clampHeight(height) !== height) onHeight(clampHeight(height));
    };
    window.addEventListener("resize", onResize);
    return () => window.removeEventListener("resize", onResize);
  }, [height, onHeight]);

  function onPointerDown(event: ReactPointerEvent<HTMLDivElement>) {
    event.preventDefault();
    event.currentTarget.setPointerCapture?.(event.pointerId);
    drag.current = { y: event.clientY, height: shown };
    setDragging(true);
  }

  function onPointerMove(event: ReactPointerEvent<HTMLDivElement>) {
    if (drag.current) onHeight(clampHeight(drag.current.height + (drag.current.y - event.clientY)));
  }

  function onPointerUp() {
    drag.current = null;
    setDragging(false);
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    if (event.key !== "ArrowUp" && event.key !== "ArrowDown") return;
    event.preventDefault();
    onHeight(clampHeight(shown + (event.key === "ArrowUp" ? HEIGHT_STEP : -HEIGHT_STEP)));
  }

  return (
    <section
      id={PANEL_ID}
      aria-label={t("terminal.region")}
      style={{ height: shown }}
      className="relative flex min-h-0 flex-none flex-col border-t bg-background"
    >
      <div
        role="separator"
        aria-orientation="horizontal"
        aria-label={t("terminal.height")}
        aria-valuemin={MIN_HEIGHT}
        aria-valuemax={maxHeight()}
        aria-valuenow={shown}
        tabIndex={0}
        title={t("terminal.heightTitle")}
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onKeyDown={onKeyDown}
        data-dragging={dragging || undefined}
        className="group absolute inset-x-0 -top-1 z-[6] h-[7px] cursor-ns-resize touch-none outline-none"
      >
        <span className="absolute inset-x-0 top-[3px] h-px group-hover:top-0.5 group-hover:h-0.5 group-hover:bg-primary group-focus-visible:top-0.5 group-focus-visible:h-0.5 group-focus-visible:bg-primary group-data-dragging:top-0.5 group-data-dragging:h-0.5 group-data-dragging:bg-primary" />
        <span className="absolute top-px left-1/2 -ml-[18px] h-1 w-9 rounded-[3px] bg-border group-hover:bg-primary group-data-dragging:bg-primary" />
      </div>
      {children}
    </section>
  );
}

interface SessionTabProps {
  session: TerminalSession;
  /** The tab the panel shows when expanded. */
  selected: boolean;
  /** On screen now: selected and expanded. */
  shown: boolean;
  /** Printed something since it was last on screen. */
  unseen: boolean;
  onSelect: () => void;
  onClose: () => void;
  onArrow: (direction: 1 | -1) => void;
}

function SessionTab({ session, selected, shown, unseen, onSelect, onClose, onArrow }: SessionTabProps) {
  const { t } = useTranslation();
  const label = tabLabel(session, unseen && !shown);
  const running = session.kind === "shell" && !session.ended && session.foreground ? shortCommand(session.foreground) : "";
  return (
    <div
      role="presentation"
      className={cn(
        "group relative -mt-px flex items-center gap-px border-t-2 border-transparent pr-[3px] pl-px whitespace-nowrap text-muted-foreground hover:text-foreground",
        shown && "border-t-primary bg-background text-foreground",
        session.ended && "text-faint",
      )}
    >
      <button
        type="button"
        role="tab"
        id={`terminal-tab-${session.id}`}
        aria-selected={selected}
        aria-controls={PANEL_ID}
        tabIndex={selected ? 0 : -1}
        aria-label={label}
        title={label}
        onClick={onSelect}
        onKeyDown={(event) => {
          if (event.key === "ArrowRight" || event.key === "ArrowLeft") {
            event.preventDefault();
            onArrow(event.key === "ArrowRight" ? 1 : -1);
          }
        }}
        className="flex h-full cursor-pointer items-center gap-1.5 rounded-[5px] pr-[3px] pl-2 outline-none focus-visible:outline-2 focus-visible:-outline-offset-[3px] focus-visible:outline-ring"
      >
        <SessionIcon kind={session.kind} />
        <span>{session.name}</span>
        {running && <span className="font-mono text-[11.5px] text-faint">· {running}</span>}
        {unseen && !shown && <span aria-hidden="true" className="size-1.5 flex-none rounded-full bg-primary" />}
      </button>
      <button
        type="button"
        aria-label={t("terminal.tab.close", { name: session.name })}
        title={t("terminal.tab.close", { name: session.name })}
        onClick={onClose}
        className={cn(
          "grid size-[18px] cursor-pointer place-items-center rounded-[4px] text-faint opacity-0 outline-none group-hover:opacity-100 hover:bg-secondary hover:text-foreground focus-visible:opacity-100 focus-visible:outline-2 focus-visible:outline-ring",
          shown && "opacity-100",
        )}
      >
        <X aria-hidden="true" className="size-2.5" />
      </button>
    </div>
  );
}

/** The tab of the session waiting for its worktree; its × cancels. */
function PendingTab({ kind, expanded, onCancel }: { kind: TerminalKind; expanded: boolean; onCancel: () => void }) {
  const { t } = useTranslation();
  // The shell's own name is only known once it runs.
  const name = kind === "claude" ? "Claude Code" : t("terminal.region");
  return (
    <div
      role="presentation"
      className={cn(
        "relative -mt-px flex items-center gap-px border-t-2 border-transparent pr-[3px] pl-px whitespace-nowrap text-faint",
        expanded && "border-dashed border-t-primary bg-background",
      )}
    >
      <span
        role="tab"
        aria-selected="true"
        aria-controls={PANEL_ID}
        title={t("terminal.tab.pending", { name })}
        className="flex h-full items-center gap-1.5 pr-[3px] pl-2"
      >
        <SessionIcon kind={kind} />
        <span>{name}</span>
      </span>
      <button
        type="button"
        aria-label={t("terminal.tab.cancelPending", { name })}
        title={t("common.actions.cancel")}
        onClick={onCancel}
        className="grid size-[18px] cursor-pointer place-items-center rounded-[4px] text-faint outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
      >
        <X aria-hidden="true" className="size-2.5" />
      </button>
    </div>
  );
}

function AddMenu({ onNew }: { onNew: (kind: TerminalKind) => void }) {
  const { t } = useTranslation();
  return (
    <DropdownMenu>
      <DropdownMenuTrigger aria-label={t("terminal.add.label")} title={t("terminal.add.label")} className={cn(ICON_BUTTON, "ml-0.5")}>
        <Plus aria-hidden="true" className="size-3" />
      </DropdownMenuTrigger>
      <DropdownMenuContent side="top" align="start" className="min-w-[220px]">
        <DropdownMenuItem onClick={() => onNew("shell")}>
          <SessionIcon kind="shell" />
          {t("terminal.add.shell")}
          <DropdownMenuShortcut>{NEW_TERMINAL_KEYS}</DropdownMenuShortcut>
        </DropdownMenuItem>
        <DropdownMenuItem onClick={() => onNew("claude")}>
          <SessionIcon kind="claude" />
          {t("terminal.add.claude")}
        </DropdownMenuItem>
      </DropdownMenuContent>
    </DropdownMenu>
  );
}

/** Sessions open in other folders: how many where the newest one runs, a click away. Hidden on narrow windows. */
function OthersChip({ sessions, onNavigate }: { sessions: TerminalSession[]; onNavigate: (route: Route) => void }) {
  const { t } = useTranslation();
  const newest = sessions.at(-1);
  if (!newest) return null;
  const group = sessions.filter((session) => session.cwd === newest.cwd);
  const where = newest.branch ?? folderName(newest.cwd);
  const go = () =>
    onNavigate(
      newest.target
        ? { screen: "review", projectId: newest.projectId, projectName: newest.projectName, target: newest.target }
        : { screen: "project", projectId: newest.projectId, projectName: newest.projectName },
    );
  return (
    <button
      type="button"
      title={t("terminal.othersTitle", { names: group.map(({ name }) => name).join(", "), where })}
      onClick={go}
      className="mr-2.5 inline-flex flex-none cursor-pointer items-center gap-1.5 self-center rounded-[11px] border px-[9px] py-px whitespace-nowrap text-muted-foreground outline-none hover:border-faint hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring max-[1100px]:hidden"
    >
      <Trans
        i18nKey="terminal.others"
        count={group.length}
        values={{ count: group.length, where }}
        components={{ code: <code className="text-[11.5px] text-foreground" /> }}
      />
    </button>
  );
}

const TAG = "flex-none cursor-help rounded-[9px] px-2 text-[11px] whitespace-nowrap not-italic";

/** Where this screen's sessions open: the folder, its branch and what the folder is. */
function LocationLabel({ location }: { location: TerminalLocation }) {
  const { t } = useTranslation();
  const branch = location.branch;
  const branchPart = (
    <>
      <span aria-hidden="true" className="text-faint">
        ·
      </span>
      <GitBranch aria-label={t("terminal.location.branch")} className="size-3.5 flex-none" />
      {branch ? (
        <code className="text-[11.5px] whitespace-nowrap text-foreground">{branch}</code>
      ) : (
        <span className="text-faint">{t("terminal.location.detached")}</span>
      )}
    </>
  );
  if (!location.folder) {
    return (
      <span
        title={t("terminal.location.noneTitle", { branch })}
        className="flex min-w-0 flex-[0_1_auto] items-center gap-1.5 whitespace-nowrap text-muted-foreground"
      >
        <FolderIcon aria-label={t("terminal.location.folder")} className="size-3.5 flex-none" />
        <span className="text-faint italic">{t("terminal.location.none")}</span>
        {branchPart}
      </span>
    );
  }
  const { folder } = location;
  const [head, tail] = splitPath(folder);
  return (
    <span className="flex min-w-0 flex-[0_1_auto] items-center gap-1.5 whitespace-nowrap text-muted-foreground">
      <FolderIcon aria-label={t("terminal.location.folder")} className="size-3.5 flex-none" />
      <code title={folder.path} className="flex min-w-0 text-[11.5px] text-foreground">
        <span className="min-w-[2.5em] overflow-hidden text-ellipsis">{head}</span>
        <span className="flex-none">{tail}</span>
      </code>
      {branchPart}
      {folder.kind === "difflens" && (
        <em title={t("terminal.location.difflensTitle")} className={cn(TAG, "bg-primary-soft text-primary")}>
          {t("terminal.location.difflens")}
        </em>
      )}
      {folder.kind === "project" && (
        <em title={t("terminal.location.projectTitle")} className={cn(TAG, "bg-mod-bg text-mod")}>
          {t("terminal.location.project")}
        </em>
      )}
      {folder.kind === "worktree" && (
        <em title={t("terminal.location.worktreeTitle")} className={cn(TAG, "border text-muted-foreground")}>
          {t("terminal.location.worktree")}
        </em>
      )}
    </span>
  );
}

/** Asks before closing a session that would interrupt work: a command in the foreground, or Claude Code. */
function CloseDialog({
  session,
  onCancel,
  onConfirm,
}: {
  session: TerminalSession | null;
  onCancel: () => void;
  onConfirm: (session: TerminalSession) => void;
}) {
  const { t } = useTranslation();
  // Kept while the dialog closes, so its text does not change on the way out.
  const shown = useRef(session);
  if (session) shown.current = session;
  const subject = shown.current;
  const claude = subject?.kind === "claude";
  return (
    <AlertDialog
      open={session !== null}
      onOpenChange={(open) => {
        if (!open) onCancel();
      }}
    >
      <AlertDialogContent>
        <AlertDialogHeader>
          <AlertDialogTitle>{claude ? t("terminal.close.claudeTitle") : t("terminal.close.shellTitle")}</AlertDialogTitle>
          <AlertDialogDescription>
            {claude ? (
              <Trans i18nKey="terminal.close.claudeText" components={{ code: <code className="text-foreground" /> }} />
            ) : subject?.foreground ? (
              <Trans
                i18nKey="terminal.close.shellText"
                values={{ command: subject.foreground }}
                components={{ code: <code className="text-foreground" /> }}
              />
            ) : (
              t("terminal.close.shellTextUnknown")
            )}
          </AlertDialogDescription>
        </AlertDialogHeader>
        <AlertDialogFooter>
          <AlertDialogCancel autoFocus>{t("common.actions.cancel")}</AlertDialogCancel>
          <AlertDialogAction variant="destructive" onClick={() => subject && onConfirm(subject)}>
            {t("terminal.close.confirm")}
          </AlertDialogAction>
        </AlertDialogFooter>
      </AlertDialogContent>
    </AlertDialog>
  );
}
