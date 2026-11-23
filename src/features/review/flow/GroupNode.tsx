import { Handle, type NodeProps, Position } from "@xyflow/react";
import { ChevronDown, ChevronRight, FileCode, Package } from "lucide-react";
import { createContext, useContext } from "react";
import { useTranslation } from "react-i18next";

import { ExplainButton } from "@/features/review/explanations/ExplainButton";
import { groupFunctionIds } from "@/features/review/explanations/explanation-text";
import { groupRunKey } from "@/features/review/explanations/use-explanations";
import { baseName } from "@/features/review/file-tree";
import type { Group } from "@/features/review/flow/flow-groups";
import {
  type ContainerFlowNode,
  type GroupFlowNode,
  groupName,
  isolatedNote,
} from "@/features/review/flow/flow-model";
import { SignalBadge } from "@/features/review/signals/signal-marks";
import { type Badge, filesBadge, type ReviewSignals, SignalsContext } from "@/features/review/signals/signals";
import { badgeParts } from "@/features/review/signals/signals-text";
import { i18n } from "@/i18n";
import { cn } from "@/lib/utils";

export interface GroupActions {
  /** Opens a closed group in place, or closes an open one. */
  toggle: (groupId: string) => void;
  /** Opens the popover of a file, or closes it when it is already open. */
  select: (groupId: string) => void;
}

export const GroupActionsContext = createContext<GroupActions>({ toggle: () => {}, select: () => {} });

// Links attach to these; they are invisible and cannot start a connection.
const HANDLE_STYLE = { opacity: 0, pointerEvents: "none" } as const;
// React Flow turns pointer events off on nodes that cannot be selected or dragged; the buttons take them back.
const BUTTON_STYLE = { pointerEvents: "auto" } as const;

/** "internal/refund, package, 3 files, 12 changed functions, 120 added, 30 removed". */
export function describeGroup(group: Group, isolated = false, badge: Badge | null = null): string {
  const parts = [groupName(group), i18n.t(`flow.groupKind.${group.kind}`)];
  if (group.kind === "package") parts.push(i18n.t("flow.files", { count: group.files.length }));
  parts.push(
    i18n.t("flow.changedFunctions", { count: group.changed }),
    i18n.t("flow.linesAdded", { count: group.additions }),
    i18n.t("flow.linesRemoved", { count: group.deletions }),
  );
  if (isolated) parts.push(isolatedNote().toLowerCase());
  if (badge) parts.push(...badgeParts(badge, true));
  return parts.join(", ");
}

/** What a closed group holds in all its files: the most severe finding, and the sum of each part. */
export function groupBadge(group: Group, signals: ReviewSignals): Badge | null {
  const paths = group.kind === "file" ? [group.path] : group.files.map((file) => file.path);
  return filesBadge(paths.map((path) => signals.files.get(path)));
}

function GroupIcon({ group, className }: { group: Group; className?: string }) {
  const Icon = group.kind === "package" ? Package : FileCode;
  return <Icon aria-hidden="true" className={cn("size-3.5 flex-none text-muted-foreground", className)} />;
}

/** Explains every function of the group in one model call. */
function GroupExplainButton({ group, className }: { group: Group; className?: string }) {
  const ids = groupFunctionIds(group);
  if (ids.length === 0) return null;
  return (
    <ExplainButton
      runKey={groupRunKey(group.id)}
      ids={ids}
      name={group.kind === "package" ? group.container : baseName(group.path) || group.path}
      group
      className={className}
    />
  );
}

function LineCounts({ group }: { group: Group }) {
  return (
    <span className="flex items-center gap-1.5 font-mono text-[11px]">
      <span className="text-add">+{group.additions}</span>
      <span className="text-del">−{group.deletions}</span>
    </span>
  );
}

/**
 * A closed package or file: its name, how many files and changed functions it holds, and the lines changed in
 * them. A package opens in place when pressed; a file opens its popover, and its chevron opens it in place.
 */
export function ClusterNode({ data }: NodeProps<GroupFlowNode>) {
  const { t } = useTranslation();
  const { toggle, select } = useContext(GroupActionsContext);
  const { signals } = useContext(SignalsContext);
  const { group, detail, selected, dimmed, isolated } = data;
  const name = groupName(group);
  const isFile = group.kind === "file";
  const badge = groupBadge(group, signals);

  return (
    <>
      <Handle type="target" position={Position.Left} isConnectable={false} style={HANDLE_STYLE} />
      <button
        type="button"
        data-group-id={group.id}
        data-kind={group.kind}
        data-dimmed={dimmed || undefined}
        aria-label={describeGroup(group, isolated, badge)}
        aria-haspopup={isFile ? "dialog" : undefined}
        aria-expanded={isFile ? selected : false}
        title={name}
        onClick={() => (isFile ? select(group.id) : toggle(group.id))}
        style={BUTTON_STYLE}
        className={cn(
          "relative flex h-full w-full cursor-pointer flex-col justify-center border bg-card py-1.5 pr-8 pl-3 text-left leading-[15px] outline-none focus-visible:outline-2 focus-visible:outline-offset-2 focus-visible:outline-ring",
          group.kind === "package" ? "rounded-[10px] border-[1.5px] shadow-[0_1px_0_var(--border)]" : "rounded-[7px]",
          selected && "border-primary shadow-[0_0_0_1px_var(--primary)]",
          dimmed && "opacity-30",
        )}
      >
        <span className="flex min-w-0 items-center gap-1.5">
          <GroupIcon group={group} />
          <span className="truncate font-mono text-[11.5px] font-medium">{name}</span>
        </span>
        <span className="truncate text-[11px] text-muted-foreground">{detail}</span>
        <LineCounts group={group} />
        {isolated && <span className="truncate text-[11px] text-faint italic">{t("flow.isolatedNote")}</span>}
        {!isFile && <ChevronRight aria-hidden="true" className="absolute top-2 right-2 size-4 text-muted-foreground" />}
        {badge && <SignalBadge badge={badge} />}
      </button>
      <GroupExplainButton group={group} className={cn("absolute right-1.5 bottom-1.5", dimmed && "opacity-30")} />
      {isFile && (
        <button
          type="button"
          aria-label={t("flow.group.showFunctionsOf", { name })}
          aria-expanded={false}
          title={t("flow.group.showFunctions")}
          onClick={() => toggle(group.id)}
          style={BUTTON_STYLE}
          className={cn(
            "absolute top-1.5 right-1.5 grid size-6 cursor-pointer place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring",
            dimmed && "opacity-30",
          )}
        >
          <ChevronRight aria-hidden="true" className="size-4" />
        </button>
      )}
      <Handle type="source" position={Position.Right} isConnectable={false} style={HANDLE_STYLE} />
    </>
  );
}

/** An open package or file: a frame around what it holds, with a header that closes it. */
export function ContainerNode({ data }: NodeProps<ContainerFlowNode>) {
  const { t } = useTranslation();
  const { toggle, select } = useContext(GroupActionsContext);
  const { group, detail, dimmed } = data;
  const name = groupName(group);

  return (
    <div
      data-container-id={group.id}
      data-dimmed={dimmed || undefined}
      className={cn(
        "h-full w-full border border-dashed",
        group.kind === "package" ? "rounded-[12px] bg-secondary/35" : "rounded-[9px] bg-card/70",
        dimmed && "opacity-30",
      )}
    >
      <div className="flex h-[34px] min-w-0 items-center gap-1.5 px-2 text-xs">
        <button
          type="button"
          aria-label={t("flow.group.closeName", { name })}
          aria-expanded={true}
          title={t("common.actions.close")}
          onClick={() => toggle(group.id)}
          style={BUTTON_STYLE}
          className="grid size-6 flex-none cursor-pointer place-items-center rounded-[5px] text-muted-foreground outline-none hover:bg-secondary hover:text-foreground focus-visible:outline-2 focus-visible:outline-ring"
        >
          <ChevronDown aria-hidden="true" className="size-4" />
        </button>
        <GroupIcon group={group} />
        {group.kind === "file" ? (
          <button
            type="button"
            data-group-id={group.id}
            aria-haspopup="dialog"
            title={name}
            onClick={() => select(group.id)}
            style={BUTTON_STYLE}
            className="min-w-0 cursor-pointer truncate rounded-[3px] font-mono font-medium outline-none hover:underline focus-visible:outline-2 focus-visible:outline-ring"
          >
            {name}
          </button>
        ) : (
          <span title={name} className="min-w-0 truncate font-mono font-medium">
            {name}
          </span>
        )}
        <span className="min-w-0 flex-none truncate text-faint">{detail}</span>
        <span className="ml-auto flex flex-none items-center gap-1.5">
          <LineCounts group={group} />
          <GroupExplainButton group={group} />
        </span>
      </div>
    </div>
  );
}
