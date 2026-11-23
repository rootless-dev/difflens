import { describe, expect, it } from "vitest";

import {
  aggregateLinks,
  linkLabel,
  linkState,
  linkTitle,
  relationLabel,
  RELATION_LOOK,
  STATE_LOOK,
  stateSummary,
  typeSummary,
} from "@/features/review/flow/flow-links";
import { makeFlowEdge } from "@/test/flow-fixtures";

const identity = new Map(["a", "b", "c", "d"].map((id) => [id, id]));
const name = (id: string) => id.toUpperCase();

describe("link aggregation", () => {
  // service.go holds a and b, repo.go holds c and d.
  const files = new Map([
    ["a", "service.go"],
    ["b", "service.go"],
    ["c", "repo.go"],
    ["d", "repo.go"],
  ]);
  const groups = new Set(["service.go", "repo.go"]);
  const edges = [
    makeFlowEdge("a", "c", { line: 30, added: true }),
    makeFlowEdge("b", "c", { line: 12, added: true }),
    makeFlowEdge("a", "d", { line: 14 }),
    makeFlowEdge("b", "d", { line: 3 }),
    makeFlowEdge("a", "c", { line: 9, relation: "registration", evidence: "route POST /token" }),
    makeFlowEdge("b", "c", { line: 40, removed: true }),
    // Inside service.go: not a link once it is closed.
    makeFlowEdge("a", "b", { line: 20, added: true }),
  ];

  it("counts the relations between two groups per state and type, and calls it neither added nor removed", () => {
    const [link, ...rest] = aggregateLinks(edges, files, groups);
    expect(rest).toEqual([]);
    expect(link).toMatchObject({ id: "service.go->repo.go", from: "service.go", to: "repo.go", grouped: true });
    expect(link.counts).toEqual({
      added: { call: 2, registration: 0, reference: 0 },
      preserved: { call: 2, registration: 1, reference: 0 },
      removed: { call: 1, registration: 0, reference: 0 },
    });
    expect(link.state).toBe("mixed");
    expect(link.relation).toBe("call");
    expect(stateSummary(link)).toBe("2 added · 3 preserved · 1 removed");
    expect(typeSummary(link)).toBe("5 calls · 1 registration");
    expect(linkLabel(link)).toBe("2 added · 3 preserved · 1 removed · 1 registration");
    expect(link.relations.map((edge) => `${edge.from}:${edge.line}`)).toEqual(["a:9", "a:14", "a:30", "b:3", "b:12", "b:40"]);
  });

  it("keeps a link between two functions, a call to itself, and leaves out ends not drawn", () => {
    const links = aggregateLinks(
      [makeFlowEdge("a", "b", { added: true }), makeFlowEdge("a", "a"), makeFlowEdge("a", "gone")],
      identity,
      new Set(),
    );
    expect(links.map((link) => [link.id, link.grouped, link.state])).toEqual([
      ["a->b", false, "added"],
      ["a->a", false, "preserved"],
    ]);
  });

  it("gives a uniform link the state and type of its relations, else the strongest type present", () => {
    const [removed] = aggregateLinks([makeFlowEdge("a", "b", { removed: true, relation: "reference" })], identity, new Set());
    expect([removed.state, removed.relation]).toEqual(["removed", "reference"]);
    const [both] = aggregateLinks(
      [
        makeFlowEdge("a", "b", { relation: "reference" }),
        makeFlowEdge("a", "b", { relation: "registration", evidence: "middleware" }),
      ],
      identity,
      new Set(),
    );
    expect([both.state, both.relation]).toEqual(["preserved", "registration"]);
  });
});

describe("link labels", () => {
  it("names each type of relation, with the evidence of a registration", () => {
    expect(relationLabel({ relation: "call", evidence: null })).toBe("calls");
    expect(relationLabel({ relation: "registration", evidence: "route POST /token" })).toBe("registers route POST /token");
    expect(relationLabel({ relation: "registration", evidence: null })).toBe("registers");
    expect(relationLabel({ relation: "reference", evidence: null })).toBe("passes as value");
  });

  it("says what the relations between two functions are, each once", () => {
    const [link] = aggregateLinks(
      [
        makeFlowEdge("a", "b", { line: 4 }),
        makeFlowEdge("a", "b", { line: 9, relation: "registration", evidence: "route POST /token" }),
      ],
      identity,
      new Set(),
    );
    expect(linkLabel(link)).toBe("calls · registers route POST /token");
  });

  it("reads the state from the flags of a relation", () => {
    expect(linkState(makeFlowEdge("a", "b", { added: true }))).toBe("added");
    expect(linkState(makeFlowEdge("a", "b", { removed: true }))).toBe("removed");
    expect(linkState(makeFlowEdge("a", "b"))).toBe("preserved");
  });

  it("lists the relations in the tooltip, up to a limit", () => {
    const [link] = aggregateLinks(
      [
        makeFlowEdge("a", "c", { line: 3, added: true }),
        makeFlowEdge("b", "c", { line: 5, kind: "conditional", resolution: "dynamic" }),
        makeFlowEdge("b", "c", { line: 8, relation: "reference" }),
      ],
      new Map([
        ["a", "g"],
        ["b", "g"],
        ["c", "c"],
      ]),
      new Set(["g"]),
    );
    expect(linkTitle(link, name, 2)).toBe(
      [
        "1 added · 2 preserved (2 calls · 1 reference)",
        "A → C, line 3: calls, on every path, added.",
        "B → C, line 5: calls, only on some paths, resolved dynamically, preserved.",
        "and 1 more; select the line to list them.",
      ].join("\n"),
    );
  });
});

describe("link looks", () => {
  it("draws the type as the line style and never dashes a state", () => {
    expect(RELATION_LOOK.call.dash).toBeUndefined();
    expect(RELATION_LOOK.registration.dash).toBe("1.5 3.5");
    expect(RELATION_LOOK.reference.dash).toBe("1 6");
    expect(STATE_LOOK.added.stroke).toBe("stroke-primary");
    expect(STATE_LOOK.preserved.stroke).toBe("stroke-faint");
    expect(STATE_LOOK.removed.stroke).toBe("stroke-del");
    expect(Object.values(STATE_LOOK).every((look) => !("dash" in look))).toBe(true);
  });
});
