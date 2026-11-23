import { createContext, type KeyboardEvent, type ReactNode, use, useEffect, useRef, useState } from "react";
import { useTranslation } from "react-i18next";

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
import { authorOf, type CommentSide, type Draft, isOwn, type Thread } from "@/features/review/review-state/comments";
import { useNow } from "@/hooks/use-now";
import type { ReviewComment } from "@/lib/api";
import { commentPublishError } from "@/lib/core-text";
import { formatRelativeTime } from "@/lib/format";
import { cn } from "@/lib/utils";

/** What the notes of a file can do; each resolves to whether it was saved. */
export interface CommentActions {
  add: (side: CommentSide, line: number, body: string) => Promise<boolean>;
  reply: (rootId: string, body: string) => Promise<boolean>;
  edit: (commentId: string, body: string) => Promise<boolean>;
  remove: (commentId: string) => Promise<boolean>;
  /** Takes back the delete of a published comment that is not published yet. */
  restore: (commentId: string) => Promise<boolean>;
  resolve: (commentId: string, resolved: boolean) => Promise<boolean>;
}

/**
 * The review has a pull request its comments can be published to, so drafts are told apart from published comments.
 * Without one every comment simply lives in DiffLens, and nothing is said.
 */
export const PublishingContext = createContext(false);

/** A comment to bring into view and focus, such as one opened from the Tests tab; a new request each time. */
export interface CommentFocus {
  commentId: string;
  /** Tells two requests for the same comment apart. */
  request: number;
}

export const CommentFocusContext = createContext<CommentFocus | null>(null);

interface LineNotesProps {
  /** The line these notes are under. */
  side: CommentSide;
  line: number;
  threads: readonly Thread[];
  /** What is being written in this file, wherever it is; each part shows its own. */
  draft: Draft | null;
  onDraftChange: (draft: Draft | null) => void;
  actions: CommentActions;
  /** A save is under way. */
  busy: boolean;
}

// The prototype's `.cmt`: a card under the line, as wide as a comfortable read. It sits inside the editor's code
// column, so it sets its own font and wrapping.
const CARD =
  "my-1.5 mr-4 ml-4 max-w-[640px] rounded-[8px] border bg-card px-3 py-2.5 font-sans text-sm whitespace-normal text-foreground";
const ACTION =
  "cursor-pointer rounded-[3px] text-xs text-primary outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-45";
const TAG = "rounded-[9px] border px-[7px] text-[11px] text-muted-foreground";

/** The threads under one line of the diff, and the composer when a comment is being written on it. */
export function LineNotes({ side, line, threads, draft, onDraftChange, actions, busy }: LineNotesProps) {
  const { t } = useTranslation();
  const now = useNow();
  const composing = draft?.kind === "new" && draft.side === side && draft.line === line ? draft : null;
  return (
    <div>
      {threads.map((thread) => (
        <ThreadCard
          key={thread.root.id}
          thread={thread}
          now={now}
          draft={draft}
          onDraftChange={onDraftChange}
          actions={actions}
          busy={busy}
        />
      ))}
      {composing && (
        <div className={CARD}>
          <Composer
            label={t("review.comments.commentOnLine")}
            saveLabel={t("review.comments.addComment")}
            text={composing.text}
            busy={busy}
            onChange={(text) => onDraftChange({ ...composing, text })}
            onCancel={() => onDraftChange(null)}
            onSave={async () => {
              if (await actions.add(side, line, composing.text)) onDraftChange(null);
            }}
          />
        </div>
      )}
    </div>
  );
}

interface ThreadCardProps {
  thread: Thread;
  now: Date;
  draft: Draft | null;
  onDraftChange: (draft: Draft | null) => void;
  actions: CommentActions;
  busy: boolean;
}

/**
 * A first comment and its replies. Once resolved it folds to one line until shown again. Asked to (`CommentFocusContext`),
 * it scrolls into view, takes the focus and stays highlighted, unfolded, until another comment is asked for.
 */
export function ThreadCard({ thread, now, draft, onDraftChange, actions, busy }: ThreadCardProps) {
  const { t } = useTranslation();
  const { root, replies } = thread;
  const [shown, setShown] = useState(false);
  const replying = draft?.kind === "reply" && draft.rootId === root.id ? draft : null;
  const focus = use(CommentFocusContext);
  const focused =
    focus !== null && (focus.commentId === root.id || replies.some((reply) => reply.id === focus.commentId));
  const card = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!focused || focus === null) return;
    const element = card.current;
    // Not every environment scrolls (the tests' DOM has no layout).
    element?.scrollIntoView?.({ block: "center" });
    element?.focus({ preventScroll: true });
  }, [focused, focus]);

  if (root.pendingChange === "delete") {
    return (
      <div role="group" aria-label={t("review.comments.deletedGroup")} className={cn(CARD, "py-1.5 text-xs text-muted-foreground")}>
        <DeletedNote comment={root} actions={actions} busy={busy} />
      </div>
    );
  }

  if (root.resolved && !shown && !focused) {
    return (
      <div
        role="group"
        aria-label={t("review.comments.resolvedBy", { author: authorOf(root) })}
        className={cn(CARD, "flex items-center gap-2 py-1.5 text-xs text-muted-foreground")}
      >
        <span aria-hidden="true" className="text-add">
          ✓
        </span>
        <span className="flex-none">{t("review.comments.resolved")}</span>
        <span className="min-w-0 flex-1 truncate">
          {authorOf(root)}: {root.body}
        </span>
        <button type="button" className={ACTION} onClick={() => setShown(true)}>
          {t("review.comments.show")}
        </button>
        <button type="button" className={ACTION} disabled={busy} onClick={() => void actions.resolve(root.id, false)}>
          {t("review.comments.reopen")}
        </button>
      </div>
    );
  }

  return (
    <div
      ref={card}
      role="group"
      aria-label={t("review.comments.commentBy", { author: authorOf(root) })}
      tabIndex={focused ? -1 : undefined}
      data-focused={focused || undefined}
      className={cn(CARD, focused && "border-ai ring-2 ring-ai/35 outline-none")}
    >
      <CommentBody
        comment={root}
        now={now}
        outdated={root.outdated}
        draft={draft}
        onDraftChange={onDraftChange}
        actions={actions}
        busy={busy}
        threadActions={
          <>
            {root.resolved ? (
              <>
                <button type="button" className={ACTION} disabled={busy} onClick={() => void actions.resolve(root.id, false)}>
                  {t("review.comments.reopen")}
                </button>
                <button type="button" className={ACTION} onClick={() => setShown(false)}>
                  {t("review.comments.hide")}
                </button>
              </>
            ) : (
              <button type="button" className={ACTION} disabled={busy} onClick={() => void actions.resolve(root.id, true)}>
                {t("review.comments.resolve")}
              </button>
            )}
            {!replying && (
              <button
                type="button"
                className={ACTION}
                onClick={() => onDraftChange({ kind: "reply", rootId: root.id, text: "" })}
              >
                {t("review.comments.reply")}
              </button>
            )}
          </>
        }
      />
      {replies.map((reply) => (
        <div key={reply.id} className="mt-2.5 border-l-2 pl-3">
          <CommentBody
            comment={reply}
            now={now}
            outdated={false}
            draft={draft}
            onDraftChange={onDraftChange}
            actions={actions}
            busy={busy}
          />
        </div>
      ))}
      {replying && (
        <div className="mt-2.5">
          <Composer
            label={t("review.comments.replyLabel")}
            saveLabel={t("review.comments.reply")}
            text={replying.text}
            busy={busy}
            onChange={(text) => onDraftChange({ ...replying, text })}
            onCancel={() => onDraftChange(null)}
            onSave={async () => {
              if (await actions.reply(root.id, replying.text)) onDraftChange(null);
            }}
          />
        </div>
      )}
    </div>
  );
}

interface CommentBodyProps {
  comment: ReviewComment;
  now: Date;
  outdated: boolean;
  draft: Draft | null;
  onDraftChange: (draft: Draft | null) => void;
  actions: CommentActions;
  busy: boolean;
  /** Resolve and Reply, on a thread's first comment. */
  threadActions?: ReactNode;
}

function CommentBody({ comment, now, outdated, draft, onDraftChange, actions, busy, threadActions }: CommentBodyProps) {
  const { t } = useTranslation();
  const editing = draft?.kind === "edit" && draft.commentId === comment.id ? draft : null;
  const own = isOwn(comment);
  const publishing = use(PublishingContext);
  const [confirming, setConfirming] = useState(false);
  if (comment.pendingChange === "delete") {
    return (
      <div className="text-xs text-muted-foreground">
        <DeletedNote comment={comment} actions={actions} busy={busy} />
      </div>
    );
  }
  // A published comment leaves the pull request too, so that is asked first; a draft only ever lived here.
  const remove = () => (comment.state === "published" ? setConfirming(true) : void actions.remove(comment.id));
  const publishError = commentPublishError(comment);
  return (
    <>
      <div className="mb-[3px] flex flex-wrap items-center gap-x-2 gap-y-0.5 text-xs text-muted-foreground">
        <b className="font-medium text-foreground">{authorOf(comment)}</b>
        <span title={comment.createdAt}>{formatRelativeTime(comment.createdAt, now)}</span>
        {publishing && comment.state === "draft" && <span className={TAG}>{t("review.comments.draft")}</span>}
        {comment.pendingChange === "edit" && <span className={TAG}>{t("review.comments.editedNotPublished")}</span>}
        {outdated && (
          <em className="rounded-[9px] bg-primary-soft px-[7px] text-[11px] text-primary not-italic">
            {t("review.comments.codeChanged")}
          </em>
        )}
      </div>
      {publishing && outdated && comment.state === "draft" && (
        <p className="mb-1 text-xs text-muted-foreground">{t("review.comments.outdatedDraft")}</p>
      )}
      {publishError && (
        <p className="mb-1 text-xs text-destructive">{t("review.comments.notPublished", { reason: publishError })}</p>
      )}
      <AlertDialog open={confirming} onOpenChange={setConfirming}>
        <AlertDialogContent>
          <AlertDialogHeader>
            <AlertDialogTitle>{t("review.comments.deleteTitle")}</AlertDialogTitle>
            <AlertDialogDescription>{t("review.comments.deleteDescription")}</AlertDialogDescription>
          </AlertDialogHeader>
          <AlertDialogFooter>
            <AlertDialogCancel>{t("review.comments.keep")}</AlertDialogCancel>
            <AlertDialogAction
              variant="destructive"
              onClick={() => {
                setConfirming(false);
                void actions.remove(comment.id);
              }}
            >
              {t("review.comments.deleteComment")}
            </AlertDialogAction>
          </AlertDialogFooter>
        </AlertDialogContent>
      </AlertDialog>
      {editing ? (
        <Composer
          label={t("review.comments.editLabel")}
          saveLabel={t("common.actions.save")}
          text={editing.text}
          busy={busy}
          onChange={(text) => onDraftChange({ ...editing, text })}
          onCancel={() => onDraftChange(null)}
          onSave={async () => {
            if (await actions.edit(comment.id, editing.text)) onDraftChange(null);
          }}
        />
      ) : (
        <>
          <p className="break-words whitespace-pre-wrap">{comment.body}</p>
          <div className="mt-[7px] flex flex-wrap gap-x-3.5 gap-y-1">
            {threadActions}
            {own && (
              <>
                <button
                  type="button"
                  className={ACTION}
                  onClick={() => onDraftChange({ kind: "edit", commentId: comment.id, text: comment.body })}
                >
                  {t("review.comments.edit")}
                </button>
                <button type="button" className={ACTION} disabled={busy} onClick={remove}>
                  {t("review.comments.delete")}
                </button>
              </>
            )}
          </div>
        </>
      )}
    </>
  );
}

/** A published comment the user deleted here, until the next publish takes it off the pull request. */
function DeletedNote({ comment, actions, busy }: { comment: ReviewComment; actions: CommentActions; busy: boolean }) {
  const { t } = useTranslation();
  return (
    <div className="flex flex-wrap items-center gap-x-2 gap-y-0.5">
      <span className="min-w-0 flex-1">
        {comment.parentId === null ? t("review.comments.deletedComment") : t("review.comments.deletedReply")}
      </span>
      <button type="button" className={ACTION} disabled={busy} onClick={() => void actions.restore(comment.id)}>
        {t("review.comments.undo")}
      </button>
    </div>
  );
}

interface ComposerProps {
  /** Placeholder and accessible name of the text box. */
  label: string;
  saveLabel: string;
  text: string;
  busy: boolean;
  onChange: (text: string) => void;
  onSave: () => void;
  onCancel: () => void;
}

/** A text box with save and cancel. Escape cancels; Cmd+Enter (Ctrl+Enter elsewhere) saves. */
export function Composer({ label, saveLabel, text, busy, onChange, onSave, onCancel }: ComposerProps) {
  const { t } = useTranslation();
  const empty = text.trim() === "";
  function onKeyDown(event: KeyboardEvent<HTMLTextAreaElement>) {
    if (event.key === "Escape") {
      // Not also closing whatever else listens for Escape around it.
      event.preventDefault();
      event.stopPropagation();
      onCancel();
    } else if (event.key === "Enter" && (event.metaKey || event.ctrlKey)) {
      event.preventDefault();
      if (!empty && !busy) onSave();
    }
  }
  return (
    <>
      <textarea
        // It opens because the user asked to write, on the line they chose.
        autoFocus
        aria-label={label}
        placeholder={label}
        value={text}
        onChange={(event) => onChange(event.target.value)}
        onKeyDown={onKeyDown}
        className="block min-h-14 w-full resize-y rounded-[6px] border bg-background px-[9px] py-[7px] font-sans text-sm text-foreground outline-none placeholder:text-faint focus-visible:outline-2 focus-visible:-outline-offset-1 focus-visible:outline-ring"
      />
      <div className="mt-[7px] flex gap-2">
        <Button size="xs" disabled={empty || busy} onClick={onSave}>
          {saveLabel}
        </Button>
        <Button variant="ghost" size="xs" onClick={onCancel}>
          {t("common.actions.cancel")}
        </Button>
      </div>
    </>
  );
}
