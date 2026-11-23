// Links between the boxes drawn on the canvas: the graph's function-level relations merged per pair of boxes,
// with how many of them the review added, kept or removed, and of which type. Pure, but for the language its text
// is written in.

import { i18n } from "@/i18n";
import type { FlowEdge } from "@/lib/api";

export type Relation = FlowEdge["relation"];

/** What the review did to a relation: added it, kept it (both sides have it) or removed it. */
export type LinkState = "added" | "preserved" | "removed";

export const LINK_STATES: readonly LinkState[] = ["added", "preserved", "removed"];
export const RELATIONS: readonly Relation[] = ["call", "registration", "reference"];

export const linkState = (edge: FlowEdge): LinkState => (edge.added ? "added" : edge.removed ? "removed" : "preserved");

/** One line between two boxes, standing for every relation from a function of one to a function of the other. */
export interface Link {
  /** `${from}->${to}`. */
  id: string;
  /** Ids of the boxes it joins: functions, or closed files and packages. */
  from: string;
  to: string;
  /** The function-level relations it stands for, by caller then line. */
  relations: FlowEdge[];
  /** How many relations have each state and type. */
  counts: Record<LinkState, Record<Relation, number>>;
  /** An end is a group: the line stands for relations between several functions. */
  grouped: boolean;
  /** The state every relation has, or "mixed": a link is never "added" because only some of it is. */
  state: LinkState | "mixed";
  /** The type every relation has; when they differ, the first present of call, registration, reference. */
  relation: Relation;
}

const emptyCounts = (): Record<LinkState, Record<Relation, number>> => ({
  added: { call: 0, registration: 0, reference: 0 },
  preserved: { call: 0, registration: 0, reference: 0 },
  removed: { call: 0, registration: 0, reference: 0 },
});

export const linkId = (from: string, to: string) => `${from}->${to}`;

/**
 * Merges the relations whose two ends are drawn into one link per pair of boxes. `unitOf` gives the box drawn for
 * each function; a relation with an end not drawn is left out. A relation inside one group (both ends in the same
 * closed box) is not a link; a function calling itself is.
 */
export function aggregateLinks(
  edges: readonly FlowEdge[],
  unitOf: ReadonlyMap<string, string>,
  groups: ReadonlySet<string>,
): Link[] {
  const links = new Map<string, Link>();
  for (const edge of edges) {
    const from = unitOf.get(edge.from);
    const to = unitOf.get(edge.to);
    if (from === undefined || to === undefined) continue;
    if (from === to && groups.has(from)) continue;
    const id = linkId(from, to);
    let link = links.get(id);
    if (!link) {
      link = {
        id,
        from,
        to,
        relations: [],
        counts: emptyCounts(),
        grouped: groups.has(from) || groups.has(to),
        state: linkState(edge),
        relation: edge.relation,
      };
      links.set(id, link);
    }
    link.relations.push(edge);
    link.counts[linkState(edge)][edge.relation] += 1;
  }
  for (const link of links.values()) {
    link.relations.sort((a, b) => a.from.localeCompare(b.from) || a.line - b.line || a.to.localeCompare(b.to));
    const states = LINK_STATES.filter((state) => total(link.counts[state]) > 0);
    link.state = states.length === 1 ? states[0] : "mixed";
    link.relation = RELATIONS.find((relation) => relationTotal(link, relation) > 0) ?? "call";
  }
  return [...links.values()];
}

const total = (counts: Record<Relation, number>) => counts.call + counts.registration + counts.reference;

export const stateTotal = (link: Link, state: LinkState) => total(link.counts[state]);

export const relationTotal = (link: Link, relation: Relation) =>
  LINK_STATES.reduce((sum, state) => sum + link.counts[state][relation], 0);

/** One part of a link's label, with the state it counts, if it counts one (so it can be coloured as that state). */
export interface LabelPart {
  text: string;
  state: LinkState | null;
}

function stateParts(link: Link): LabelPart[] {
  return LINK_STATES.filter((state) => stateTotal(link, state) > 0).map((state) => ({
    text: i18n.t(`flow.link.stateCount.${state}`, { count: stateTotal(link, state) }),
    state,
  }));
}

/** "2 added · 3 preserved · 1 removed", leaving out the states with none. */
export function stateSummary(link: Link): string {
  return stateParts(link)
    .map((part) => part.text)
    .join(" · ");
}

const typeCount = (link: Link, relation: Relation) =>
  i18n.t(`flow.link.typeCount.${relation}`, { count: relationTotal(link, relation) });

/** "3 calls · 1 registration", leaving out the types with none. */
export function typeSummary(link: Link): string {
  return RELATIONS.filter((relation) => relationTotal(link, relation) > 0)
    .map((relation) => typeCount(link, relation))
    .join(" · ");
}

/** What one relation does, as the line's label says it: "calls", "registers route POST /token", "passes as value". */
export function relationLabel(edge: Pick<FlowEdge, "relation" | "evidence">): string {
  switch (edge.relation) {
    case "call":
      return i18n.t("flow.link.relation.call");
    case "registration":
      // The evidence ("route POST /token") is the analyzer's, shown as it gave it.
      return edge.evidence
        ? i18n.t("flow.link.relation.registrationOf", { evidence: edge.evidence })
        : i18n.t("flow.link.relation.registration");
    case "reference":
      return i18n.t("flow.link.relation.reference");
  }
}

/**
 * The parts of the label on a link. Between two functions they say what the relations are ("calls", or "calls ·
 * registers route POST /token"). Between groups they count them per state, adding the types other than calls when
 * present: "2 added · 3 preserved · 1 registration".
 */
export function linkLabelParts(link: Link): LabelPart[] {
  if (!link.grouped) {
    return [...new Set(link.relations.map(relationLabel))].map((text) => ({ text, state: null }));
  }
  const others = RELATIONS.filter((relation) => relation !== "call" && relationTotal(link, relation) > 0);
  return [...stateParts(link), ...others.map((relation) => ({ text: typeCount(link, relation), state: null }))];
}

/** The label on a link, its parts joined. */
export function linkLabel(link: Link): string {
  return linkLabelParts(link)
    .map((part) => part.text)
    .join(" · ");
}

export interface RelationLook {
  /** SVG stroke-dasharray, undefined for a solid line. */
  dash: string | undefined;
}

/** The type of a relation is its line style: calls solid, registrations dotted, references sparsely dotted. */
export const RELATION_LOOK: Record<Relation, RelationLook> = {
  call: { dash: undefined },
  registration: { dash: "1.5 3.5" },
  reference: { dash: "1 6" },
};

/** Phrase of the legend for a type of relation. */
export const relationMeaning = (relation: Relation) => i18n.t(`flow.legend.relation.${relation}`);

export interface StateLook {
  /** Stroke colour class of the line. */
  stroke: string;
  strokeWidth: number;
}

/**
 * The change state of a link is its colour, never its line style, so it reads with any type: added in the accent
 * colour, kept in a discreet grey, removed in red with a cross on its label. A link mixing states stays neutral and
 * its label gives the counts.
 */
export const STATE_LOOK: Record<LinkState | "mixed", StateLook> = {
  added: { stroke: "stroke-primary", strokeWidth: 1.8 },
  preserved: { stroke: "stroke-faint", strokeWidth: 1.4 },
  removed: { stroke: "stroke-del", strokeWidth: 1.4 },
  mixed: { stroke: "stroke-muted-foreground", strokeWidth: 1.6 },
};

/** Phrase of the legend and of screen readers for a link's state. */
export const stateMeaning = (state: LinkState | "mixed") => i18n.t(`flow.legend.state.${state}`);

/** The state of one relation, as a word: "added", "preserved", "removed". */
export const linkStateWord = (state: LinkState) => i18n.t(`flow.link.state.${state}`);

/** One line of a link's tooltip or list: "Handler.Create → Service.Request, line 33: calls, on every path, added." */
export function relationSentence(edge: FlowEdge, name: (id: string) => string): string {
  const relation = edge.relation === "call" ? i18n.t(`flow.link.callKind.${edge.kind}`) : relationLabel(edge);
  const what = edge.resolution === "dynamic" ? i18n.t("flow.link.resolvedDynamically", { what: relation }) : relation;
  return i18n.t("flow.link.sentence", {
    from: name(edge.from),
    to: name(edge.to),
    line: edge.line,
    what,
    state: linkStateWord(linkState(edge)),
  });
}

/** Tooltip of a link: what it counts, then each relation, at most `limit` of them. */
export function linkTitle(link: Link, name: (id: string) => string, limit = 8): string {
  const lines = [link.grouped ? `${stateSummary(link)} (${typeSummary(link)})` : stateSummary(link)];
  lines.push(...link.relations.slice(0, limit).map((edge) => relationSentence(edge, name)));
  if (link.relations.length > limit) lines.push(i18n.t("flow.link.more", { more: link.relations.length - limit }));
  return lines.join("\n");
}
