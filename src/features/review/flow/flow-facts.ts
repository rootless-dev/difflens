// Deterministic facts about one function, for the popover beside its node, and where its diff is.

import type { FileGroup } from "@/features/review/flow/flow-groups";
import { isChanged, nodeStateLabel } from "@/features/review/flow/flow-model";
import { i18n } from "@/i18n";
import type { ChangedFile, FlowEdge, FlowGraph, FlowNode } from "@/lib/api";

export interface Fact {
  label: string;
  value: string;
}

export interface NodeFacts {
  title: string;
  stateLabel: string;
  /** Lines added and removed in the function, for new, modified and deleted functions. */
  lines: { additions: number; deletions: number } | null;
  /** "internal/refund/service.go:44–71", or the name of an external package. */
  location: string;
  summary: string;
  facts: Fact[];
}

function summaryOf(node: FlowNode): string {
  return i18n.t(`flow.facts.summary.${node.state}`);
}

function locationOf(node: FlowNode): string {
  if (node.file === null) return node.container;
  if (node.startLine === null) return node.file;
  const end = node.endLine !== null && node.endLine !== node.startLine ? `–${node.endLine}` : "";
  return `${node.file}:${node.startLine}${end}`;
}

function complexityOf(node: FlowNode): string | null {
  if (node.complexity === null) return null;
  const { complexity, previousComplexity } = node;
  if (node.state === "modified" && previousComplexity !== null && previousComplexity !== complexity) {
    return `${previousComplexity} → ${complexity}`;
  }
  return node.state === "deleted" ? i18n.t("flow.facts.complexityOld", { complexity }) : String(complexity);
}

/** Distinct callers and callees drawn in the graph, counting calls this review removed apart. */
function neighbours(graph: FlowGraph, id: string) {
  const callers = new Set<string>();
  const callees = new Set<string>();
  const removedCallees = new Set<string>();
  for (const edge of graph.edges) {
    if (edge.to === id && !edge.removed) callers.add(edge.from);
    if (edge.from === id) (edge.removed ? removedCallees : callees).add(edge.to);
  }
  for (const callee of callees) removedCallees.delete(callee);
  return { callers: callers.size, callees: callees.size, removedCallees: removedCallees.size };
}

/**
 * The facts of a function. `withComplexity` false leaves the complexity out, for a popover that lists it among the
 * function's quality limits instead.
 */
export function nodeFacts(graph: FlowGraph, node: FlowNode, withComplexity = true): NodeFacts {
  const facts: Fact[] = [];
  const complexity = withComplexity ? complexityOf(node) : null;
  if (complexity !== null) facts.push({ label: i18n.t("flow.facts.cyclomaticComplexity"), value: complexity });
  if (node.entryPoints.length > 0) {
    facts.push({
      label: i18n.t("flow.facts.entryPoints", { count: node.entryPoints.length }),
      // Entry point labels ("POST /refunds") are the analyzer's.
      value: node.entryPoints.map((entry) => entry.label).join(", "),
    });
  }
  const { callers, callees, removedCallees } = neighbours(graph, node.id);
  facts.push({ label: i18n.t("flow.facts.callersShown"), value: String(callers) });
  if (node.state === "external") {
    facts.push({ label: i18n.t("flow.facts.functionsCalled"), value: String(node.externalFunctions) });
  }
  if (node.state !== "external") {
    facts.push({
      label: i18n.t("flow.facts.callsShown"),
      value:
        removedCallees > 0
          ? i18n.t("flow.facts.callsShownRemoved", { shown: callees, count: removedCallees })
          : String(callees),
    });
  }
  if (node.generated) facts.push({ label: i18n.t("flow.facts.generatedCode"), value: i18n.t("flow.facts.yes") });

  return {
    title: node.qualifiedName,
    stateLabel: nodeStateLabel(node.state),
    lines: isChanged(node) ? { additions: node.additions, deletions: node.deletions } : null,
    location: locationOf(node),
    summary: summaryOf(node),
    facts,
  };
}

/** Which file of the review to open, and where, to read a function's change. */
export interface DiffTarget {
  path: string;
  line: number;
  /** "old" for a deleted function: the line is on the old side of the file. */
  side: "old" | "new";
  /** The line holds a model finding or a static check to show: its kind of block is shown again if it was hidden. */
  reveal?: "finding" | "check";
}

/**
 * The changed file holding the function, matched on either side so renamed and deleted files are found.
 * Null for external packages and for functions in files this review does not change.
 */
export function diffTarget(node: FlowNode, files: readonly ChangedFile[]): DiffTarget | null {
  if (node.state === "external" || node.file === null) return null;
  const file = files.find((candidate) => candidate.path === node.file || candidate.oldPath === node.file);
  if (!file) return null;
  return { path: file.path, line: node.startLine ?? 1, side: node.state === "deleted" ? "old" : "new" };
}

/** Where a file group's change starts: its first changed function, else the file's first line. */
export function fileDiffTarget(file: FileGroup, files: readonly ChangedFile[]): DiffTarget | null {
  const first = [...file.functions]
    .filter(isChanged)
    .sort((a, b) => (a.startLine ?? 0) - (b.startLine ?? 0))[0];
  if (first) return diffTarget(first, files);
  const changed = files.find((candidate) => candidate.path === file.path || candidate.oldPath === file.path);
  return changed ? { path: changed.path, line: 1, side: "new" } : null;
}

/**
 * The line of a relation in its caller's file: on the old side when the review removed it (the line is the old
 * side's), on the new side otherwise.
 */
export function relationDiffTarget(edge: FlowEdge, graph: FlowGraph, files: readonly ChangedFile[]): DiffTarget | null {
  const caller = graph.nodes.find((node) => node.id === edge.from);
  const file = caller ? diffTarget(caller, files) : null;
  if (!file) return null;
  return { path: file.path, line: edge.line, side: edge.removed ? "old" : "new" };
}
