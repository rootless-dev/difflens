import { type KeyboardEvent, useEffect, useId, useRef } from "react";
import { useTranslation } from "react-i18next";

import { Input } from "@/components/ui/input";
import { FILE_STATUS, fileStatusLabel, spokenFileStatus } from "@/features/review/file-status";
import { adjacentPath, type FileGroup, fileLabel, orderedPaths } from "@/features/review/file-tree";
import { i18n } from "@/i18n";
import type { ChangedFile, MarkStatus } from "@/lib/api";
import { cn } from "@/lib/utils";

const NO_MARKS: ReadonlyMap<string, MarkStatus> = new Map();

interface FileListProps {
  groups: FileGroup[];
  /** Whether to offer the filter box, for long lists. */
  filterable: boolean;
  query: string;
  onQueryChange: (query: string) => void;
  selectedPath: string | null;
  onSelect: (path: string) => void;
  /** Files marked as reviewed, by path. */
  marks?: ReadonlyMap<string, MarkStatus>;
  /** Said when there is no file at all to list. */
  emptyText?: string;
}

/** Changed files grouped by directory, as a listbox: arrows, Home and End move the selection. */
export function FileList({
  groups,
  filterable,
  query,
  onQueryChange,
  selectedPath,
  onSelect,
  marks = NO_MARKS,
  emptyText,
}: FileListProps) {
  const { t } = useTranslation();
  const list = useRef<HTMLDivElement>(null);
  const paths = orderedPaths(groups);
  // The one option reachable with Tab: the selected file, or the first one when it is filtered out.
  const tabStop = selectedPath !== null && paths.includes(selectedPath) ? selectedPath : paths[0];

  useEffect(() => {
    // Keeps the selection visible when it moves with j/k. jsdom has no scrollIntoView.
    optionFor(list.current, selectedPath)?.scrollIntoView?.({ block: "nearest" });
  }, [selectedPath]);

  function onKeyDown(event: KeyboardEvent) {
    const next =
      event.key === "ArrowDown"
        ? adjacentPath(paths, selectedPath, 1)
        : event.key === "ArrowUp"
          ? adjacentPath(paths, selectedPath, -1)
          : event.key === "Home"
            ? paths[0]
            : event.key === "End"
              ? paths[paths.length - 1]
              : undefined;
    if (next === undefined || next === null) return;
    event.preventDefault();
    onSelect(next);
    optionFor(list.current, next)?.focus();
  }

  return (
    <aside className="flex min-h-0 flex-col border-r bg-card">
      {filterable && (
        <div className="flex-none px-3 pt-3.5">
          <Input
            type="search"
            aria-label={t("review.fileList.filter")}
            placeholder={t("review.fileList.filter")}
            value={query}
            onChange={(event) => onQueryChange(event.target.value)}
            onKeyDown={(event) => {
              if (event.key === "Escape") onQueryChange("");
            }}
            className="bg-background font-sans text-sm"
          />
        </div>
      )}
      <div
        ref={list}
        role="listbox"
        aria-label={t("review.fileList.label")}
        onKeyDown={onKeyDown}
        className="min-h-0 flex-1 overflow-auto px-3 pt-3.5 pb-4"
      >
        {groups.map((group) => (
          <Group
            key={group.dir}
            group={group}
            selectedPath={selectedPath}
            tabStop={tabStop}
            onSelect={onSelect}
            marks={marks}
          />
        ))}
        {paths.length === 0 && (
          <p className="px-2 py-1.5 text-muted-foreground">
            {query.trim() ? t("review.fileList.noMatch") : (emptyText ?? t("review.fileList.empty"))}
          </p>
        )}
      </div>
    </aside>
  );
}

function optionFor(list: HTMLElement | null, path: string | null): HTMLElement | null {
  if (!list || path === null) return null;
  return [...list.querySelectorAll<HTMLElement>("[role=option]")].find((option) => option.dataset.path === path) ?? null;
}

interface GroupProps {
  group: FileGroup;
  selectedPath: string | null;
  tabStop: string | undefined;
  onSelect: (path: string) => void;
  marks: ReadonlyMap<string, MarkStatus>;
}

function Group({ group, selectedPath, tabStop, onSelect, marks }: GroupProps) {
  const { t } = useTranslation();
  const id = useId();
  const dir = group.dir || t("review.fileList.repositoryRoot");
  return (
    <div role="group" aria-labelledby={id} className="mb-2.5">
      <div id={id} className="mx-2 mb-1.5 truncate font-mono text-xs text-faint" title={dir}>
        {dir}
      </div>
      {group.files.map((file) => (
        <FileOption
          key={file.path}
          file={file}
          selected={file.path === selectedPath}
          tabbable={file.path === tabStop}
          onSelect={onSelect}
          mark={marks.get(file.path) ?? null}
        />
      ))}
    </div>
  );
}

/** "internal/x.go, modified, 14 added, 9 removed, reviewed", read by screen readers in place of the row. */
function describeFile(file: ChangedFile, mark: MarkStatus | null): string {
  const status = spokenFileStatus(file.status);
  const parts = [
    file.path,
    file.oldPath ? i18n.t("review.fileList.spoken.from", { status, oldPath: file.oldPath }) : status,
  ];
  if (file.binary) parts.push(i18n.t("review.fileList.spoken.binary"));
  else {
    parts.push(
      i18n.t("review.fileList.spoken.added", { count: file.additions }),
      i18n.t("review.fileList.spoken.removed", { count: file.deletions }),
    );
  }
  if (mark === "reviewed") parts.push(i18n.t("review.fileList.spoken.reviewed"));
  if (mark === "reopened") parts.push(i18n.t("review.fileList.spoken.reopened"));
  return parts.join(", ");
}

interface FileOptionProps {
  file: ChangedFile;
  selected: boolean;
  tabbable: boolean;
  onSelect: (path: string) => void;
  mark: MarkStatus | null;
}

function FileOption({ file, selected, tabbable, onSelect, mark }: FileOptionProps) {
  const status = FILE_STATUS[file.status];
  const { name, renamedFrom } = fileLabel(file);
  return (
    <div
      role="option"
      aria-selected={selected}
      aria-label={describeFile(file, mark)}
      data-path={file.path}
      tabIndex={tabbable ? 0 : -1}
      title={file.oldPath ? `${file.oldPath} → ${file.path}` : file.path}
      onClick={() => onSelect(file.path)}
      className={cn(
        "flex w-full cursor-pointer items-center gap-2 rounded-sm px-2 py-1.5 outline-none hover:bg-secondary focus-visible:outline-2 focus-visible:-outline-offset-2 focus-visible:outline-ring",
        selected && "bg-primary-soft hover:bg-primary-soft",
      )}
    >
      <span aria-hidden="true" className={cn("size-2 flex-none rounded-[2px]", status.dot)} />
      <span aria-hidden="true" title={fileStatusLabel(file.status)} className={cn("w-2.5 flex-none font-mono text-[11px]", status.text)}>
        {status.letter}
      </span>
      <span className="min-w-0 flex-1 truncate font-mono text-xs">
        {renamedFrom && (
          <>
            <span className="text-muted-foreground">{renamedFrom}</span>
            <span className="text-faint"> → </span>
          </>
        )}
        {name}
      </span>
      <FileMeta file={file} />
      <ReviewMark mark={mark} />
    </div>
  );
}

/** ✓ for a file reviewed as it is, "changed" for one that changed after it was marked (the prototype's `.mark`). */
function ReviewMark({ mark }: { mark: MarkStatus | null }) {
  const { t } = useTranslation();
  if (mark === "reviewed") {
    return (
      <span
        aria-hidden="true"
        title={t("review.fileList.reviewedMark")}
        className="w-3 flex-none text-center text-[11px] text-add"
      >
        ✓
      </span>
    );
  }
  if (mark === "reopened") {
    return (
      <span
        aria-hidden="true"
        title={t("review.fileList.reopenedTitle")}
        className="flex-none rounded-[9px] bg-primary-soft px-1.5 text-[11px] text-primary"
      >
        {t("review.fileList.reopenedMark")}
      </span>
    );
  }
  return null;
}

function FileMeta({ file }: { file: ChangedFile }) {
  const { t } = useTranslation();
  if (file.binary) {
    return <span className="flex-none text-xs text-faint">{t("review.fileList.binary")}</span>;
  }
  return (
    <span className="flex-none font-mono text-xs whitespace-nowrap text-faint">
      {file.additions > 0 && <span className="text-add">+{file.additions}</span>}
      {file.additions > 0 && file.deletions > 0 && " "}
      {file.deletions > 0 && <span className="text-del">−{file.deletions}</span>}
    </span>
  );
}
