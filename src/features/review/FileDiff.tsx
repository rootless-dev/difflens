import { lazy, Suspense, use, useCallback, useMemo, useState } from "react";
import { useTranslation } from "react-i18next";

import { ErrorNote } from "@/components/page";
import { Button } from "@/components/ui/button";
import { Skeleton } from "@/components/ui/skeleton";
import { diffContent } from "@/features/review/diff-content";
import type { ScrollTarget } from "@/features/review/diff-scroll";
import type { LineAnnotation } from "@/features/review/DiffEditor";
import type { DiffOptions } from "@/features/review/DiffToolbar";
import { FILE_STATUS, fileStatusLabel } from "@/features/review/file-status";
import {
  type CommentSide,
  type Draft,
  lineKey,
  parseLineKey,
  type Thread,
  threadsByLine,
} from "@/features/review/review-state/comments";
import { type CommentActions, LineNotes } from "@/features/review/review-state/LineNotes";
import { fileButtonLabel } from "@/features/review/review-state/review-status";
import { fileSignalLayout } from "@/features/review/signals/diff-signals";
import {
  CheckBlock,
  DismissedBlock,
  type DraftControl,
  DuplicateNote,
  FileSignalSummary,
  FindingBlock,
  FunctionMarkers,
  LimitPanel,
  MutantNote,
} from "@/features/review/signals/DiffSignals";
import { type FileSignals, SignalsContext } from "@/features/review/signals/signals";
import type { FindingComments } from "@/features/review/signals/use-finding-comments";
import { useAsyncData } from "@/hooks/use-async-data";
import {
  type ChangedFile,
  type ChangeSet,
  getFileVersions,
  type MarkStatus,
  type SignalDraft,
  type SignalDraftRequest,
} from "@/lib/api";
import { cn } from "@/lib/utils";

const DiffEditor = lazy(() => import("@/features/review/DiffEditor"));

/** The key of the marker drawn at the end of a function's first line. */
const INLINE = "inline:";

/** The file's "Mark as reviewed" button: its status, and what a press does. */
export interface FileMark {
  status: MarkStatus | null;
  onToggle: () => void;
  busy: boolean;
}

/** The comment threads of the review and how to write more, for the files that can be commented. */
export interface FileComments {
  threads: readonly Thread[];
  /** Sides whose lines take comments: both, or only the new side when the old one is not the review's own. */
  sides: ReadonlySet<CommentSide>;
  actions: CommentActions;
  busy: boolean;
  /** The drafts written from a finding or a check, and how to write and show one; absent where none can be. */
  signalDrafts?: SignalDraftActions;
}

export interface SignalDraftActions {
  /** By signal key. */
  drafts: ReadonlyMap<string, SignalDraft>;
  create: (request: SignalDraftRequest) => Promise<SignalDraft | null>;
  /** Brings a draft into view: on its line, or among the general comments. */
  view: (draft: SignalDraft) => void;
  /** How the model writes a model finding's comment; without it a finding offers no draft. */
  findings?: FindingComments;
}

interface FileDiffProps {
  projectId: string;
  changeSet: ChangeSet;
  file: ChangedFile;
  options: DiffOptions;
  /** Line to bring into view, such as the start of a function opened from the Flow tab. */
  scrollTo?: ScrollTarget | null;
  /** null when the file cannot be marked. */
  mark?: FileMark | null;
  /** null when the file cannot be commented. */
  comments?: FileComments | null;
}

/** Loads both sides of one file and shows them as a diff. Key it by path to start fresh per file. */
export function FileDiff({ projectId, changeSet, file, options, scrollTo = null, mark = null, comments = null }: FileDiffProps) {
  const { t } = useTranslation();
  const { oldSha, newSha } = changeSet;
  const load = useCallback(
    () => getFileVersions(projectId, oldSha, newSha, file.path, file.oldPath),
    [projectId, oldSha, newSha, file.path, file.oldPath],
  );
  const { state, reload } = useAsyncData(load);
  const { signals } = use(SignalsContext);

  return (
    <section aria-label={t("review.fileDiff.label", { path: file.path })} className="flex min-h-0 flex-1 flex-col">
      <FileHeader file={file} mark={mark} signals={signals.files.get(file.path)} />
      <div className="min-h-0 flex-1">
        {state.status === "loading" && <DiffSkeleton label={t("review.fileDiff.loadingFile")} />}
        {state.status === "error" && (
          <div className="px-4 py-4">
            <ErrorNote>{state.message}</ErrorNote>
            <Button variant="secondary" size="sm" className="mt-2.5" onClick={reload}>
              {t("common.actions.tryAgain")}
            </Button>
          </div>
        )}
        {state.status === "ready" && (
          <Content
            file={file}
            content={diffContent(state.data)}
            options={options}
            scrollTo={scrollTo}
            comments={comments}
          />
        )}
      </div>
    </section>
  );
}

/** A file shown in "Since last review" that has nothing new since then. */
export function UnchangedSince({ file, mark = null }: { file: ChangedFile; mark?: FileMark | null }) {
  const { t } = useTranslation();
  return (
    <section aria-label={t("review.fileDiff.label", { path: file.path })} className="flex min-h-0 flex-1 flex-col">
      <FileHeader file={file} mark={mark} />
      <p className="px-4 py-4 text-muted-foreground">{t("review.fileDiff.unchangedSince")}</p>
    </section>
  );
}

function Content({
  file,
  content,
  options,
  scrollTo,
  comments,
}: {
  file: ChangedFile;
  content: ReturnType<typeof diffContent>;
  options: DiffOptions;
  scrollTo: ScrollTarget | null;
  comments: FileComments | null;
}) {
  const { t } = useTranslation();
  // One thing is written at a time in a file: a comment, a reply or an edit. It lives here rather than in the
  // composer, so it survives the editor drawing the composer anew when it scrolls back into view.
  const [draft, setDraft] = useState<Draft | null>(null);
  const byLine = useMemo(
    () => (comments ? threadsByLine(comments.threads, file.path, comments.sides) : new Map<string, Thread[]>()),
    [comments, file.path],
  );
  // The file's findings, checks and limits, on the new side, the kinds hidden by the toolbar left out. Like the
  // draft, which limit panels are open and which findings are folded live here.
  const handle = use(SignalsContext);
  const fileSignals = handle.signals.files.get(file.path);
  const { showFindings, showChecks } = options;
  const layout = useMemo(
    () => fileSignalLayout(fileSignals, { findings: showFindings, checks: showChecks }),
    [fileSignals, showFindings, showChecks],
  );
  const [panels, setPanels] = useState<ReadonlySet<number>>(() => new Set());
  const [folded, setFolded] = useState<ReadonlySet<string>>(() => new Set());
  const annotations = useMemo<LineAnnotation[]>(() => {
    const keys = new Set(byLine.keys());
    if (draft?.kind === "new") keys.add(lineKey(draft.side, draft.line));
    for (const line of layout.under.keys()) keys.add(lineKey("new", line));
    for (const line of panels) if (layout.starts.has(line)) keys.add(lineKey("new", line));
    const blocks: LineAnnotation[] = [...keys].map((key) => ({ key, ...parseLineKey(key) }));
    const inline = [...layout.starts.keys()].map((line) => ({ key: `${INLINE}${line}`, side: "new" as const, line, inline: true }));
    return [...blocks, ...inline];
  }, [byLine, draft, layout, panels]);

  if (content.kind !== "text") {
    return <p className="px-4 py-4 text-muted-foreground">{content.message}</p>;
  }
  const signalDrafts = comments?.signalDrafts;
  const draftControl: DraftControl | null =
    comments && signalDrafts
      ? {
          draftOf: (signal) => signalDrafts.drafts.get(signal) ?? null,
          create: (request) =>
            void signalDrafts.create(request).then((written) => {
              if (written === null) return;
              // A draft on a line opens to be edited, where it is; a general one is shown among the comments.
              if (written.kind === "line") setDraft({ kind: "edit", commentId: written.commentId, text: request.body });
              else signalDrafts.view(written);
            }),
          view: signalDrafts.view,
          busy: comments.busy,
          finding: signalDrafts.findings ?? null,
        }
      : null;
  const toggle = <T,>(set: ReadonlySet<T>, value: T): ReadonlySet<T> => {
    const next = new Set(set);
    if (!next.delete(value)) next.add(value);
    return next;
  };
  const renderSignals = (line: number) => {
    const fn = panels.has(line) ? layout.starts.get(line) : undefined;
    const under = layout.under.get(line);
    return (
      <>
        {fn && handle.signals.limits && (
          <LimitPanel signals={fn} limits={handle.signals.limits} onOpenQuality={handle.onOpenQuality} />
        )}
        {under?.findings.map((finding) => (
          <FindingBlock
            key={finding.id}
            finding={finding}
            open={!folded.has(finding.id)}
            onToggle={() => setFolded((current) => toggle(current, finding.id))}
            draft={draftControl}
          />
        ))}
        {under?.dismissed.map((finding) => <DismissedBlock key={finding.id} finding={finding} />)}
        {under?.checks.map((check) => (
          <CheckBlock key={`${check.tool}:${check.rule}:${check.message}`} check={check} draft={draftControl} />
        ))}
        {under?.duplicates.map((duplicate) => (
          <DuplicateNote
            key={`${duplicate.kind}:${duplicate.block.startLine}:${duplicate.block.endLine}`}
            duplicate={duplicate}
            draft={draftControl}
          />
        ))}
        {under?.mutants.map((mutant) => <MutantNote key={mutant.id} mutant={mutant} draft={draftControl} />)}
      </>
    );
  };
  const onLineClick = comments
    ? (side: CommentSide, line: number) => {
        if (!comments.sides.has(side)) return;
        // Moving to another line keeps what was typed.
        setDraft((current) => ({ kind: "new", side, line, text: current?.kind === "new" ? current.text : "" }));
      }
    : undefined;
  const renderAnnotation = (key: string) => {
    if (key.startsWith(INLINE)) {
      const line = Number(key.slice(INLINE.length));
      const fn = layout.starts.get(line);
      return fn ? (
        <FunctionMarkers
          signals={fn}
          limits={handle.signals.limits}
          expanded={panels.has(line)}
          onToggle={() => setPanels((current) => toggle(current, line))}
        />
      ) : null;
    }
    const { side, line } = parseLineKey(key);
    return (
      <>
        {side === "new" && renderSignals(line)}
        {comments && (
          <LineNotes
            side={side}
            line={line}
            threads={byLine.get(key) ?? []}
            draft={draft}
            onDraftChange={setDraft}
            actions={comments.actions}
            busy={comments.busy}
          />
        )}
      </>
    );
  };
  return (
    <Suspense fallback={<DiffSkeleton label={t("review.fileDiff.loadingEditor")} />}>
      <DiffEditor
        oldText={content.oldText}
        newText={content.newText}
        language={file.language}
        path={file.path}
        layout={options.layout}
        collapseUnchanged={options.changesOnly}
        scrollTo={scrollTo}
        annotations={annotations}
        renderAnnotation={renderAnnotation}
        lane={layout.lane}
        onLineClick={onLineClick}
      />
    </Suspense>
  );
}

/** Stays on top while the diff below scrolls (the prototype's sticky `.file`). */
function FileHeader({
  file,
  mark,
  signals,
}: {
  file: ChangedFile;
  mark: FileMark | null;
  signals?: FileSignals;
}) {
  const status = FILE_STATUS[file.status];
  return (
    <div className="flex flex-none items-center gap-2.5 border-b bg-background px-4 py-2">
      <span aria-hidden="true" className={cn("size-2 flex-none rounded-[2px]", status.dot)} />
      <h2 className="min-w-0 truncate font-mono text-xs font-normal">
        {file.oldPath && (
          <>
            <span className="text-muted-foreground">{file.oldPath}</span>
            <span className="text-faint"> → </span>
          </>
        )}
        {file.path}
      </h2>
      <span className={cn("text-xs", status.text)}>{fileStatusLabel(file.status)}</span>
      {!file.binary && (
        <span className="font-mono text-xs whitespace-nowrap">
          <span className="text-add">+{file.additions}</span> <span className="text-del">−{file.deletions}</span>
        </span>
      )}
      <FileSignalSummary file={signals} />
      {mark && (
        <Button
          variant="outline"
          size="xs"
          disabled={mark.busy}
          onClick={mark.onToggle}
          className={cn(
            "ml-auto text-xs",
            mark.status === "reviewed" && "border-add text-add not-disabled:hover:border-add not-disabled:hover:text-add",
            mark.status === "reopened" &&
              "border-primary text-primary not-disabled:hover:border-primary not-disabled:hover:text-primary",
          )}
        >
          {fileButtonLabel(mark.status)}
        </Button>
      )}
    </div>
  );
}

function DiffSkeleton({ label }: { label: string }) {
  return (
    <div role="status" aria-label={label} className="grid gap-2 px-4 py-4">
      <Skeleton className="h-3.5 w-2/3" />
      <Skeleton className="h-3.5 w-1/2" />
      <Skeleton className="h-3.5 w-3/5" />
    </div>
  );
}
