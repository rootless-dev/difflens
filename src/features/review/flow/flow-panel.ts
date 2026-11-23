// What the left panel of the Flow tab lists: the entry points that reach a change, and the packages changed.

import { isChanged } from "@/features/review/flow/flow-model";
import { i18n } from "@/i18n";
import type { EntryPointKind, FlowGraph, FlowNodeState } from "@/lib/api";

export interface EntryRow {
  key: string;
  nodeId: string;
  label: string;
  state: FlowNodeState;
  /** "new", the kind when the label alone does not say it ("consumer"), or both. */
  meta: string;
}

// Routes need no word: "POST /refunds" already reads as one.
function kindWord(kind: EntryPointKind): string {
  return kind === "http_route" ? "" : i18n.t(`flow.sidebar.entryKind.${kind}`);
}

/** One row per entry point of every entry function, in the analyzer's order. */
export function entryRows(graph: FlowGraph): EntryRow[] {
  const byId = new Map(graph.nodes.map((node) => [node.id, node]));
  return graph.entryPoints.flatMap((id) => {
    const node = byId.get(id);
    if (!node) return [];
    const entries = node.entryPoints.length > 0 ? node.entryPoints : [{ kind: "main" as const, label: node.name }];
    return entries.map((entry, index) => ({
      key: `${id}#${index}`,
      nodeId: id,
      label: entry.label,
      state: node.state,
      meta: [node.state === "new" ? i18n.t("flow.sidebar.new") : "", kindWord(entry.kind)].filter(Boolean).join(" · "),
    }));
  });
}

export interface PackageRow {
  container: string;
  /** New, modified and deleted functions in it. */
  changed: number;
}

/** Packages with at least one changed function, most changed first, then by name. */
export function packageRows(graph: FlowGraph): PackageRow[] {
  const counts = new Map<string, number>();
  for (const node of graph.nodes) {
    if (isChanged(node)) counts.set(node.container, (counts.get(node.container) ?? 0) + 1);
  }
  return [...counts.entries()]
    .map(([container, changed]) => ({ container, changed }))
    .sort((a, b) => b.changed - a.changed || a.container.localeCompare(b.container));
}
