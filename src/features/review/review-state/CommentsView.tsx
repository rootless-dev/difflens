import { use, useId, useState } from "react";
import { useTranslation } from "react-i18next";

import { Button } from "@/components/ui/button";
import {
  authorOf,
  type Draft,
  generalThreads,
  lineThreadsByFile,
  type Thread,
} from "@/features/review/review-state/comments";
import { type CommentActions, Composer, PublishingContext, ThreadCard } from "@/features/review/review-state/LineNotes";
import { useNow } from "@/hooks/use-now";
import type { ReviewComment } from "@/lib/api";
import { cn } from "@/lib/utils";

interface CommentsViewProps {
  /** Every thread of the review: on the pull request as a whole and on lines. */
  threads: readonly Thread[];
  /** The review's files in the order the file list shows them, which the line comments follow. */
  order: readonly string[];
  actions: CommentActions;
  /** Writes a new general comment; resolves to whether it was saved. */
  onAdd: (body: string) => Promise<boolean>;
  /** A save is under way. */
  busy: boolean;
  /** Opens a line comment's file at its line, the thread focused. */
  onOpen: (comment: ReviewComment) => void;
}

/**
 * The Comments view of the Diff tab: the conversations on the pull request as a whole (from anyone, as the pull request
 * has them, and the user's drafts) with a composer for a new one, then every line conversation, by file, each a row
 * that opens its file at the line. All of them are published with Publish comments.
 */
export function CommentsView({ threads, order, actions, onAdd, busy, onOpen }: CommentsViewProps) {
  const { t } = useTranslation();
  return (
    <div role="region" aria-label={t("review.comments.view.label")} className="min-h-0 flex-1 overflow-y-auto pb-6">
      <GeneralComments threads={generalThreads(threads)} actions={actions} onAdd={onAdd} busy={busy} />
      <LineComments threads={threads} order={order} onOpen={onOpen} />
    </div>
  );
}

function GeneralComments({
  threads,
  actions,
  onAdd,
  busy,
}: {
  threads: readonly Thread[];
  actions: CommentActions;
  onAdd: (body: string) => Promise<boolean>;
  busy: boolean;
}) {
  const { t } = useTranslation();
  const now = useNow();
  const titleId = useId();
  // One thing is written at a time here, as in a file: a new comment, a reply or an edit.
  const [draft, setDraft] = useState<Draft | null>(null);
  const composing = draft?.kind === "general" ? draft : null;
  return (
    <section aria-labelledby={titleId} className="border-b px-4 pt-3 pb-2.5">
      <header className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        <h2 id={titleId} className="text-sm font-medium">
          {t("review.comments.general.title")}
        </h2>
        {threads.length > 0 && (
          <span className="text-xs text-muted-foreground">
            {t("review.comments.general.count", { count: threads.length })}
          </span>
        )}
        {!composing && (
          <Button
            variant="ghost"
            size="xs"
            className="ml-auto"
            title={t("review.comments.general.note")}
            onClick={() => setDraft({ kind: "general", text: "" })}
          >
            {t("review.comments.general.write")}
          </Button>
        )}
      </header>
      {threads.length === 0 && !composing && (
        <p className="mt-1 text-xs text-muted-foreground">{t("review.comments.general.none")}</p>
      )}
      {(threads.length > 0 || composing) && (
        <div className="-mx-4 mt-1">
          {threads.map((thread) => (
            <ThreadCard
              key={thread.root.id}
              thread={thread}
              now={now}
              draft={draft}
              onDraftChange={setDraft}
              actions={actions}
              busy={busy}
            />
          ))}
          {composing && (
            <div className="my-1.5 mr-4 ml-4 max-w-[640px] rounded-[8px] border bg-card px-3 py-2.5 text-sm">
              <Composer
                label={t("review.comments.general.writeLabel")}
                saveLabel={t("review.comments.addComment")}
                text={composing.text}
                busy={busy}
                onChange={(text) => setDraft({ kind: "general", text })}
                onCancel={() => setDraft(null)}
                onSave={async () => {
                  if (await onAdd(composing.text)) setDraft(null);
                }}
              />
            </div>
          )}
        </div>
      )}
    </section>
  );
}

const CHIP = "flex-none rounded-[9px] border px-[7px] text-[11px] text-muted-foreground";
/** Characters of a first comment a row shows, before CSS truncates it to the row. */
const EXCERPT_CHARS = 160;

/** The first line of a comment, its spaces run together, cut short. */
function excerptOf(body: string): string {
  const first = body.trim().split("\n")[0]?.replace(/\s+/g, " ") ?? "";
  return first.length > EXCERPT_CHARS ? `${first.slice(0, EXCERPT_CHARS).trimEnd()}…` : first;
}

function LineComments({
  threads,
  order,
  onOpen,
}: {
  threads: readonly Thread[];
  order: readonly string[];
  onOpen: (comment: ReviewComment) => void;
}) {
  const { t } = useTranslation();
  const titleId = useId();
  const groups = lineThreadsByFile(threads, order);
  const count = groups.reduce((total, group) => total + group.threads.length, 0);
  return (
    <section aria-labelledby={titleId} className="px-4 pt-3">
      <header className="flex flex-wrap items-center gap-x-2.5 gap-y-1">
        <h2 id={titleId} className="text-sm font-medium">
          {t("review.comments.view.lines")}
        </h2>
        {count > 0 && (
          <span className="text-xs text-muted-foreground">{t("review.comments.general.count", { count })}</span>
        )}
      </header>
      {groups.length === 0 && <p className="mt-1 text-xs text-muted-foreground">{t("review.comments.view.linesNone")}</p>}
      {groups.map((group) => (
        <FileComments key={group.path} path={group.path} threads={group.threads} onOpen={onOpen} />
      ))}
    </section>
  );
}

function FileComments({
  path,
  threads,
  onOpen,
}: {
  path: string;
  threads: readonly Thread[];
  onOpen: (comment: ReviewComment) => void;
}) {
  const headingId = useId();
  return (
    <div className="mt-2.5 max-w-[880px]">
      <h3 id={headingId} className="truncate font-mono text-xs text-muted-foreground">
        {path}
      </h3>
      <ul aria-labelledby={headingId} className="mt-1 overflow-hidden rounded-[8px] border bg-card">
        {threads.map((thread) => (
          <li key={thread.root.id} className="border-b last:border-b-0">
            <LineThreadRow thread={thread} onOpen={onOpen} />
          </li>
        ))}
      </ul>
    </div>
  );
}

/** One line conversation as a row: where, the start of its first comment, who, its state and its replies. */
function LineThreadRow({ thread, onOpen }: { thread: Thread; onOpen: (comment: ReviewComment) => void }) {
  const { t } = useTranslation();
  const publishing = use(PublishingContext);
  const { root, replies } = thread;
  const line = root.currentLine ?? root.line;
  // Its file (or side) left the review: there is no line to open.
  const placed = root.currentLine !== null;
  const deleted = root.pendingChange === "delete";
  return (
    <button
      type="button"
      disabled={!placed}
      title={placed ? t("review.comments.view.open") : t("review.comments.view.notInReview")}
      onClick={() => onOpen(root)}
      className={cn(
        "flex w-full cursor-pointer items-center gap-2.5 px-3 py-2 text-left text-sm outline-none hover:bg-secondary/60 focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring disabled:cursor-default disabled:opacity-60 disabled:hover:bg-transparent",
        root.resolved && "text-muted-foreground",
      )}
    >
      <span className="w-[92px] flex-none font-mono text-xs text-muted-foreground">
        {root.side === "old" ? t("review.comments.view.oldLine", { line }) : t("review.comments.view.line", { line })}
      </span>
      <span className={cn("min-w-0 flex-1 truncate", deleted && "line-through")}>{excerptOf(root.body)}</span>
      <span className="flex-none text-xs text-muted-foreground">{authorOf(root)}</span>
      {deleted && <span className={CHIP}>{t("review.comments.view.deleted")}</span>}
      {publishing && !deleted && (
        <span className={CHIP}>
          {root.state === "draft" ? t("review.comments.draft") : t("review.comments.view.published")}
        </span>
      )}
      {root.resolved && <span className={cn(CHIP, "text-add")}>{t("review.comments.resolved")}</span>}
      {root.outdated && (
        <span className="flex-none rounded-[9px] bg-primary-soft px-[7px] text-[11px] text-primary">
          {t("review.comments.view.outdated")}
        </span>
      )}
      {replies.length > 0 && (
        <span className="flex-none text-xs text-muted-foreground">
          {t("review.comments.view.replies", { count: replies.length })}
        </span>
      )}
    </button>
  );
}
