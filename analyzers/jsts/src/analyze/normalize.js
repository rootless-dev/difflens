// Sorts and deduplicates an analysis so that the same tree always gives the
// same bytes, as the Go analyzer's protocol.Document.Normalize does.

const cmp = (a, b) => (a < b ? -1 : a > b ? 1 : 0);

const EVIDENCE_SEPARATOR = "; ";

function compareCalls(a, b) {
  return (
    cmp(a.caller, b.caller) ||
    a.line - b.line ||
    cmp(a.callee, b.callee) ||
    cmp(a.relation, b.relation) ||
    cmp(a.kind, b.kind) ||
    cmp(a.resolution, b.resolution) ||
    cmp(a.evidence ?? "", b.evidence ?? "")
  );
}

function sameRelation(a, b) {
  if (a.relation === "call" || b.relation === "call") return compareCalls(a, b) === 0;
  return a.caller === b.caller && a.callee === b.callee && a.line === b.line && a.relation === b.relation;
}

function joinEvidence(joined, next) {
  if (!next || (joined ?? "").split(EVIDENCE_SEPARATOR).includes(next)) return joined;
  if (!joined) return next;
  return joined + EVIDENCE_SEPARATOR + next;
}

/**
 * Calls are duplicates when every field matches. Registrations and references are one per (caller, callee,
 * line, relation), their evidence joined. A call to a function also registered or referenced on the same line
 * keeps both, as they say different things.
 */
export function dedupeCalls(calls) {
  const sorted = [...calls].sort(compareCalls);
  const out = [];
  for (const c of sorted) {
    const last = out.at(-1);
    if (last && sameRelation(last, c)) {
      const evidence = joinEvidence(last.evidence, c.evidence);
      if (evidence) last.evidence = evidence;
      continue;
    }
    out.push({ ...c });
  }
  for (const c of out) if (!c.evidence) delete c.evidence;
  return out;
}

function compareEntryPoints(a, b) {
  return cmp(a.kind, b.kind) || cmp(a.label, b.label) || cmp(a.file ?? "", b.file ?? "") || (a.line ?? 0) - (b.line ?? 0);
}

/** The analysis sorted and deduplicated; an external that is a function of the tree is dropped. */
export function normalize({ functions, calls, externals, diagnostics }) {
  const byId = new Map();
  for (const f of functions) if (!byId.has(f.id)) byId.set(f.id, f);
  const outFunctions = [...byId.values()].sort((a, b) => cmp(a.id, b.id));
  for (const f of outFunctions) {
    if (!f.entryPoints) continue;
    const eps = [...f.entryPoints].sort(compareEntryPoints);
    f.entryPoints = eps.filter((ep, i) => i === 0 || compareEntryPoints(ep, eps[i - 1]) !== 0);
  }
  const extById = new Map();
  for (const e of externals) if (!byId.has(e.id) && !extById.has(e.id)) extById.set(e.id, e);
  const known = (id) => byId.has(id) || extById.has(id);
  const outCalls = dedupeCalls(calls.filter((c) => byId.has(c.caller) && known(c.callee) && c.line >= 1));
  const seenDiag = new Set();
  const outDiagnostics = [];
  for (const d of [...diagnostics].sort(
    (a, b) => cmp(a.file ?? "", b.file ?? "") || (a.line ?? 0) - (b.line ?? 0) || cmp(a.level, b.level) || cmp(a.message, b.message),
  )) {
    const key = JSON.stringify(d);
    if (seenDiag.has(key)) continue;
    seenDiag.add(key);
    outDiagnostics.push(d);
  }
  return {
    functions: outFunctions,
    calls: outCalls,
    externals: [...extById.values()].sort((a, b) => cmp(a.id, b.id)),
    diagnostics: outDiagnostics,
  };
}
