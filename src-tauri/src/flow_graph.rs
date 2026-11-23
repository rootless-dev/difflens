//! Builds the call graph of a review from the analyses of its two sides and
//! the lines the review changed. Pure: no git, no disk.
//!
//! - A function is matched across sides by id. Only on the new side: new;
//!   only on the old side: deleted; on both: modified when the review added
//!   lines inside its new range or removed lines inside its old range,
//!   untouched otherwise. Additions and deletions are those line counts.
//! - Test code (functions the analyzer flags `test`) is left out unless
//!   `include_tests`: its calls are dropped, so a test is neither a changed
//!   function nor a caller.
//! - Scope: the changed functions (generated ones excluded), nothing else by
//!   default. With `include_callers`, also their callers walked up (on the
//!   new side, on the old side for deleted functions) until an entry point
//!   or a function nobody calls. With `include_callees`, also their direct
//!   callees on either side (so a call the review removed shows): repository
//!   functions as they are, externals as one node per package,
//!   `external:<container>`.
//! - `hidden` counts what each option adds to what the other options draw,
//!   whether or not it is on.
//! - The changed functions are never cut. What the options add is capped at
//!   `max_nodes` nodes; beyond it the graph is `truncated` and keeps, for
//!   each changed function, the shortest path up to an entry point (or a
//!   function nobody calls) when callers are shown, then the direct callees
//!   when shown, then the nearest callers.
//! - Edges are the direct relations (call, registration, reference) between
//!   shown nodes on either side, one per (from, to, relation), at its first
//!   line. A relation that only passes through a function not shown is not
//!   an edge. Calls to several functions of one external package make one
//!   edge, of the strongest kind among them (always, then conditional, then
//!   async). `added`: only on the new side; `removed`: only on the old side;
//!   neither: kept by the review.
//! - Changed functions list every direct relation they have (`calls`),
//!   drawn or not, made of the same triples as the edges.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};

use crate::analysis::{Analysis, Call, Diagnostic, DiagnosticLevel, Function};
use crate::error::CoreText;
use crate::hunks::ChangedLines;
use crate::model::{
    AnalyzerIdentity, CallKind, CallResolution, EntryPoint, FlowCall, FlowEdge, FlowGraph,
    FlowHidden, FlowNode, FlowNodeState, FlowOptions, Relation,
};

/// Readability limit of the graph.
pub const MAX_NODES: usize = 300;

/// The two analyses of one language.
pub struct LanguageSides<'a> {
    pub old: &'a Analysis,
    pub new: &'a Analysis,
}

pub struct GraphRequest<'a> {
    /// At least one.
    pub languages: Vec<LanguageSides<'a>>,
    pub lines: &'a ChangedLines,
    pub old_sha: &'a str,
    pub new_sha: Option<&'a str>,
    pub analyzed_at: String,
    pub max_nodes: usize,
    pub options: FlowOptions,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Side {
    Old = 0,
    New = 1,
}

type NodeId = usize;

/// One function, or one external package, of one language.
struct Node<'a> {
    language: usize,
    id: Cow<'a, str>,
    old: Option<&'a Function>,
    new: Option<&'a Function>,
    /// The package of an external package node.
    package: Option<&'a str>,
    state: FlowNodeState,
    additions: u32,
    deletions: u32,
}

impl<'a> Node<'a> {
    fn function(language: usize, id: &'a str) -> Self {
        Self {
            language,
            id: Cow::Borrowed(id),
            old: None,
            new: None,
            package: None,
            state: FlowNodeState::External,
            additions: 0,
            deletions: 0,
        }
    }

    fn package(language: usize, container: &'a str) -> Self {
        Self {
            id: Cow::Owned(format!("external:{container}")),
            package: Some(container),
            ..Self::function(language, "")
        }
    }

    /// The new side, or the old one for a deleted function.
    fn current(&self) -> Option<&'a Function> {
        self.new.or(self.old)
    }

    fn on(&self, side: Side) -> Option<&'a Function> {
        match side {
            Side::Old => self.old,
            Side::New => self.new,
        }
    }

    fn is_changed(&self) -> bool {
        matches!(
            self.state,
            FlowNodeState::New | FlowNodeState::Modified | FlowNodeState::Deleted
        )
    }

    fn is_test(&self) -> bool {
        self.current().is_some_and(|function| function.test)
    }

    fn is_generated(&self) -> bool {
        self.current().is_some_and(|function| function.generated)
    }

    /// The side its callers are looked for on.
    fn walk_side(&self) -> Side {
        if self.state == FlowNodeState::Deleted {
            Side::Old
        } else {
            Side::New
        }
    }
}

/// A link of one relation from one node to another.
type Key = (NodeId, NodeId, Relation);

/// The relations of one side with one key, as one: the first, or for an
/// external package the first line and the strongest kind.
#[derive(Debug, Clone, Copy)]
struct Linked<'a> {
    line: u32,
    kind: CallKind,
    resolution: CallResolution,
    evidence: Option<&'a str>,
}

type LinkedCalls<'a> = HashMap<Key, Linked<'a>>;

/// Every key of either side, as (to, relation) grouped by `from`.
type Outgoing = HashMap<NodeId, Vec<(NodeId, Relation)>>;

/// Lower is stronger: a call made on every path says more than one made on
/// some, which says more than one started for later.
fn strength(kind: CallKind) -> u8 {
    match kind {
        CallKind::Always => 0,
        CallKind::Conditional => 1,
        CallKind::Async => 2,
    }
}

struct Graph<'a> {
    /// Language id of each `Node::language`.
    language_names: Vec<&'a str>,
    nodes: Vec<Node<'a>>,
    /// Calls of each side, indexed by `Side`, with both ends resolved.
    calls: [Vec<(NodeId, NodeId, &'a Call)>; 2],
    callers: [Vec<Vec<NodeId>>; 2],
    callees: [Vec<Vec<NodeId>>; 2],
}

impl<'a> Graph<'a> {
    fn new(languages: &[LanguageSides<'a>], lines: &ChangedLines, include_tests: bool) -> Self {
        let mut nodes: Vec<Node<'a>> = Vec::new();
        let mut index: Vec<HashMap<&'a str, NodeId>> = Vec::new();
        for (language, sides) in languages.iter().enumerate() {
            let mut ids: HashMap<&'a str, NodeId> = HashMap::new();
            for (side, analysis) in [(Side::New, sides.new), (Side::Old, sides.old)] {
                for function in &analysis.functions {
                    let node = *ids.entry(function.id.as_str()).or_insert_with(|| {
                        nodes.push(Node::function(language, &function.id));
                        nodes.len() - 1
                    });
                    let slot = match side {
                        Side::Old => &mut nodes[node].old,
                        Side::New => &mut nodes[node].new,
                    };
                    slot.get_or_insert(function);
                }
            }
            // Every external of one package resolves to the package's node.
            let mut packages: HashMap<&'a str, NodeId> = HashMap::new();
            for external in sides.new.externals.iter().chain(&sides.old.externals) {
                if ids.contains_key(external.id.as_str()) {
                    continue;
                }
                let node = *packages
                    .entry(external.container.as_str())
                    .or_insert_with(|| {
                        nodes.push(Node::package(language, &external.container));
                        nodes.len() - 1
                    });
                ids.insert(&external.id, node);
            }
            index.push(ids);
        }
        for node in &mut nodes {
            classify(node, lines);
        }

        let mut calls: [Vec<(NodeId, NodeId, &'a Call)>; 2] = [Vec::new(), Vec::new()];
        for (language, sides) in languages.iter().enumerate() {
            let ids = &index[language];
            for (side, analysis) in [(Side::Old, sides.old), (Side::New, sides.new)] {
                for call in &analysis.calls {
                    let (Some(&caller), Some(&callee)) =
                        (ids.get(call.caller.as_str()), ids.get(call.callee.as_str()))
                    else {
                        continue;
                    };
                    // Both ends must exist on the side that reports the call.
                    let callee_known =
                        nodes[callee].on(side).is_some() || nodes[callee].package.is_some();
                    if nodes[caller].on(side).is_none() || !callee_known {
                        continue;
                    }
                    if !include_tests && (nodes[caller].is_test() || nodes[callee].is_test()) {
                        continue;
                    }
                    calls[side as usize].push((caller, callee, call));
                }
            }
        }
        let mut callers = [vec![Vec::new(); nodes.len()], vec![Vec::new(); nodes.len()]];
        let mut callees = [vec![Vec::new(); nodes.len()], vec![Vec::new(); nodes.len()]];
        for side in [Side::Old, Side::New] {
            for &(caller, callee, _) in &calls[side as usize] {
                callers[side as usize][callee].push(caller);
                callees[side as usize][caller].push(callee);
            }
        }
        Self {
            language_names: languages
                .iter()
                .map(|sides| sides.new.language.as_str())
                .collect(),
            nodes,
            calls,
            callers,
            callees,
        }
    }

    fn callers_of(&self, node: NodeId, side: Side) -> &[NodeId] {
        &self.callers[side as usize][node]
    }

    fn is_entry(&self, node: NodeId, side: Side) -> bool {
        self.nodes[node]
            .on(side)
            .is_some_and(|function| !function.entry_points.is_empty())
    }

    /// Ends a walk up: an entry point or a function nobody calls.
    fn is_top(&self, node: NodeId, side: Side) -> bool {
        self.is_entry(node, side) || self.callers_of(node, side).is_empty()
    }

    fn sort_key(&self, node: NodeId) -> (usize, &str) {
        (self.nodes[node].language, &self.nodes[node].id)
    }

    /// Callers of `seeds` up to entry points, with their distance from the
    /// nearest seed, nearest first.
    fn callers_up(&self, seeds: &[NodeId]) -> Vec<NodeId> {
        let mut seen: HashSet<(NodeId, Side)> = HashSet::new();
        let mut queue = VecDeque::new();
        for &seed in seeds {
            let side = self.nodes[seed].walk_side();
            seen.insert((seed, side));
            queue.push_back((seed, side, 0u32));
        }
        let mut found: Vec<(u32, NodeId)> = Vec::new();
        while let Some((node, side, distance)) = queue.pop_front() {
            if distance > 0 {
                found.push((distance, node));
            }
            if self.is_entry(node, side) {
                continue;
            }
            for &caller in self.callers_of(node, side) {
                if seen.insert((caller, side)) {
                    queue.push_back((caller, side, distance + 1));
                }
            }
        }
        found.sort_by(|left, right| {
            left.0
                .cmp(&right.0)
                .then_with(|| self.sort_key(left.1).cmp(&self.sort_key(right.1)))
        });
        let mut unique = Vec::new();
        let mut added = HashSet::new();
        for (_, node) in found {
            if added.insert(node) {
                unique.push(node);
            }
        }
        unique
    }

    /// The shortest chain of callers from `seed` up to an entry point, or
    /// when none is reachable up to the nearest function nobody calls,
    /// nearest caller first. Empty when the seed is itself the top.
    fn shortest_path_up(&self, seed: NodeId) -> Vec<NodeId> {
        let side = self.nodes[seed].walk_side();
        if self.is_top(seed, side) {
            return Vec::new();
        }
        let mut parent: HashMap<NodeId, NodeId> = HashMap::new();
        let mut queue = VecDeque::from([seed]);
        let mut seen = HashSet::from([seed]);
        let mut nearest_root = None;
        while let Some(node) = queue.pop_front() {
            for &caller in self.callers_of(node, side) {
                if !seen.insert(caller) {
                    continue;
                }
                parent.insert(caller, node);
                if self.is_entry(caller, side) {
                    return path_from(caller, seed, &parent);
                }
                if self.callers_of(caller, side).is_empty() {
                    nearest_root.get_or_insert(caller);
                }
                queue.push_back(caller);
            }
        }
        nearest_root.map_or_else(Vec::new, |root| path_from(root, seed, &parent))
    }

    /// What `seeds` call on either side, so that a call the review removed
    /// from a modified function is found too.
    fn direct_callees(&self, seeds: &[NodeId]) -> Vec<NodeId> {
        let mut found = Vec::new();
        let mut seen = HashSet::new();
        for &seed in seeds {
            let mut callees = self.callees_of(seed);
            callees.sort_by(|left, right| self.sort_key(*left).cmp(&self.sort_key(*right)));
            for callee in callees {
                if seen.insert(callee) {
                    found.push(callee);
                }
            }
        }
        found
    }

    /// Distinct callees of `caller` on either side.
    fn callees_of(&self, caller: NodeId) -> Vec<NodeId> {
        let mut callees: Vec<NodeId> = [Side::New, Side::Old]
            .iter()
            .flat_map(|side| self.callees[*side as usize][caller].iter().copied())
            .collect();
        callees.sort_unstable();
        callees.dedup();
        callees
    }

    /// The relations of one side from `callers`, one per (from, to, relation).
    fn linked_calls(&self, side: Side, callers: &HashSet<NodeId>) -> LinkedCalls<'a> {
        let mut linked = LinkedCalls::new();
        for &(caller, callee, call) in &self.calls[side as usize] {
            if !callers.contains(&caller) {
                continue;
            }
            let this = Linked {
                line: call.line,
                kind: call.kind,
                resolution: call.resolution,
                evidence: call.evidence.as_deref(),
            };
            let slot = linked
                .entry((caller, callee, call.relation))
                .or_insert(this);
            let strongest = if strength(call.kind) < strength(slot.kind) {
                call.kind
            } else {
                slot.kind
            };
            if call.line < slot.line {
                *slot = this;
            }
            if self.nodes[callee].package.is_some() {
                slot.kind = strongest;
            }
        }
        linked
    }
}

/// The chain from `seed` (left out) up to `top`, nearest to the seed first.
fn path_from(top: NodeId, seed: NodeId, parent: &HashMap<NodeId, NodeId>) -> Vec<NodeId> {
    let mut path = vec![top];
    let mut current = top;
    while let Some(&below) = parent.get(&current) {
        if below == seed {
            break;
        }
        path.push(below);
        current = below;
    }
    path.reverse();
    path
}

fn classify(node: &mut Node<'_>, lines: &ChangedLines) {
    let added = |function: &Function| {
        lines.added_within(&function.file, function.start_line, function.end_line)
    };
    let removed = |function: &Function| {
        lines.removed_within(&function.file, function.start_line, function.end_line)
    };
    let (state, additions, deletions) = match (node.old, node.new) {
        (None, None) => (FlowNodeState::External, 0, 0),
        (None, Some(new)) => (FlowNodeState::New, added(new), 0),
        (Some(old), None) => (FlowNodeState::Deleted, 0, removed(old)),
        (Some(old), Some(new)) => {
            let (additions, deletions) = (added(new), removed(old));
            if additions > 0 || deletions > 0 {
                (FlowNodeState::Modified, additions, deletions)
            } else {
                (FlowNodeState::Untouched, 0, 0)
            }
        }
    };
    node.state = state;
    node.additions = additions;
    node.deletions = deletions;
}

/// Insertion-ordered set of nodes with a size limit.
struct Selection {
    order: Vec<NodeId>,
    members: HashSet<NodeId>,
}

impl Selection {
    fn new() -> Self {
        Self {
            order: Vec::new(),
            members: HashSet::new(),
        }
    }

    fn insert(&mut self, node: NodeId) {
        if self.members.insert(node) {
            self.order.push(node);
        }
    }

    /// Adds `nodes` in order while the selection is below `limit`.
    fn fill(&mut self, nodes: impl IntoIterator<Item = NodeId>, limit: usize) {
        for node in nodes {
            if self.order.len() >= limit {
                return;
            }
            self.insert(node);
        }
    }
}

fn count(n: usize) -> u32 {
    u32::try_from(n).unwrap_or(u32::MAX)
}

pub fn build(request: &GraphRequest<'_>) -> FlowGraph {
    let options = request.options;
    let graph = Graph::new(&request.languages, request.lines, options.include_tests);

    let changed: Vec<NodeId> = (0..graph.nodes.len())
        .filter(|&node| graph.nodes[node].is_changed() && !graph.nodes[node].is_generated())
        .collect();
    let changed_tests = changed
        .iter()
        .filter(|&&node| graph.nodes[node].is_test())
        .count();
    let mut seeds: Vec<NodeId> = changed
        .into_iter()
        .filter(|&node| options.include_tests || !graph.nodes[node].is_test())
        .collect();
    seeds.sort_by(|left, right| graph.sort_key(*left).cmp(&graph.sort_key(*right)));
    let changed_set: HashSet<NodeId> = seeds.iter().copied().collect();

    let callers: Vec<NodeId> = graph
        .callers_up(&seeds)
        .into_iter()
        .filter(|node| !changed_set.contains(node))
        .collect();
    let callees: Vec<NodeId> = graph
        .direct_callees(&seeds)
        .into_iter()
        .filter(|node| !changed_set.contains(node))
        .collect();
    let shown_callers: &[NodeId] = if options.include_callers {
        &callers
    } else {
        &[]
    };
    let shown_callees: &[NodeId] = if options.include_callees {
        &callees
    } else {
        &[]
    };
    let hidden = hidden_counts(
        &graph,
        changed_tests,
        (&callers, shown_callers),
        (&callees, shown_callees),
    );

    let mut scope = Selection::new();
    for &node in seeds.iter().chain(shown_callers).chain(shown_callees) {
        scope.insert(node);
    }
    // The changed functions are never cut: only what the options add is.
    let limit = seeds.len().saturating_add(request.max_nodes);
    let truncated = scope.order.len() > limit;
    let included = if truncated {
        let mut kept = Selection::new();
        for &seed in &seeds {
            kept.insert(seed);
        }
        if options.include_callers {
            for &seed in &seeds {
                kept.fill(graph.shortest_path_up(seed), limit);
            }
        }
        kept.fill(shown_callees.iter().copied(), limit);
        kept.fill(shown_callers.iter().copied(), limit);
        kept
    } else {
        scope
    };

    let display_ids = display_ids(&graph, &included.order);
    let new = graph.linked_calls(Side::New, &included.members);
    let old = graph.linked_calls(Side::Old, &included.members);
    let outgoing = outgoing(&new, &old);
    let edges = edges(&included.members, &display_ids, &outgoing, &new, &old);
    let entry_points = entry_points(&graph, &included, &seeds, &edges, &display_ids);
    let external_functions = external_functions(&graph, &included.members);

    let mut nodes: Vec<FlowNode> = included
        .order
        .iter()
        .map(|&node| {
            let calls = if graph.nodes[node].is_changed() {
                calls_of(
                    &graph,
                    node,
                    &included.members,
                    &display_ids,
                    &outgoing,
                    &new,
                    &old,
                )
            } else {
                Vec::new()
            };
            flow_node(
                &graph.nodes[node],
                display_ids[&node].clone(),
                calls,
                external_functions.get(&node).copied().unwrap_or(0),
            )
        })
        .collect();
    nodes.sort_by(|left, right| left.id.cmp(&right.id));
    let edges = edges.into_iter().map(|(_, _, edge)| edge).collect();

    let joined = |part: fn(&Analysis) -> &str| {
        request
            .languages
            .iter()
            .map(|sides| part(sides.new))
            .collect::<Vec<_>>()
            .join("+")
    };
    let mut diagnostics = Vec::new();
    for sides in &request.languages {
        diagnostics.extend(
            sides
                .old
                .diagnostics
                .iter()
                .map(|d| CoreText::raw(diagnostic("old", d))),
        );
        diagnostics.extend(
            sides
                .new
                .diagnostics
                .iter()
                .map(|d| CoreText::raw(diagnostic("new", d))),
        );
    }
    FlowGraph {
        language: joined(|analysis| &analysis.language),
        analyzer: AnalyzerIdentity {
            name: joined(|analysis| &analysis.analyzer.name),
            version: joined(|analysis| &analysis.analyzer.version),
        },
        old_sha: request.old_sha.to_string(),
        new_sha: request.new_sha.map(str::to_string),
        options,
        nodes,
        edges,
        hidden,
        entry_points,
        truncated,
        diagnostics,
        analyzed_at: request.analyzed_at.clone(),
    }
}

/// The node's id, prefixed with the language only when two languages
/// shown in the graph use the same id.
fn display_ids(graph: &Graph<'_>, included: &[NodeId]) -> HashMap<NodeId, String> {
    let mut uses: HashMap<&str, usize> = HashMap::new();
    for &node in included {
        *uses.entry(&graph.nodes[node].id).or_default() += 1;
    }
    included
        .iter()
        .map(|&node| {
            let id: &str = &graph.nodes[node].id;
            let shown = if uses[id] > 1 {
                format!("{}:{id}", graph.language_names[graph.nodes[node].language])
            } else {
                id.to_string()
            };
            (node, shown)
        })
        .collect()
}

/// What each option adds to the graph the other options draw: callers not
/// already drawn as callees, in-repository callees not already drawn as
/// callers, and external packages.
fn hidden_counts(
    graph: &Graph<'_>,
    changed_tests: usize,
    (callers, shown_callers): (&[NodeId], &[NodeId]),
    (callees, shown_callees): (&[NodeId], &[NodeId]),
) -> FlowHidden {
    let shown_callers: HashSet<NodeId> = shown_callers.iter().copied().collect();
    let shown_callees: HashSet<NodeId> = shown_callees.iter().copied().collect();
    let is_package = |node: &NodeId| graph.nodes[*node].package.is_some();
    FlowHidden {
        changed_tests: count(changed_tests),
        callers: count(
            callers
                .iter()
                .filter(|node| !shown_callees.contains(node))
                .count(),
        ),
        callees: count(
            callees
                .iter()
                .filter(|node| !is_package(node) && !shown_callers.contains(node))
                .count(),
        ),
        external_packages: count(callees.iter().filter(|node| is_package(node)).count()),
    }
}

/// The key's relation on the new side, else on the old side, with whether
/// the review added or removed it.
fn pair_call<'a>(
    key: Key,
    new: &LinkedCalls<'a>,
    old: &LinkedCalls<'a>,
) -> Option<(Linked<'a>, bool, bool)> {
    match (new.get(&key), old.get(&key)) {
        (Some(call), on_old) => Some((*call, on_old.is_none(), false)),
        (None, Some(call)) => Some((*call, false, true)),
        (None, None) => None,
    }
}

fn outgoing(new: &LinkedCalls<'_>, old: &LinkedCalls<'_>) -> Outgoing {
    let mut by_caller = Outgoing::new();
    let keys = new
        .keys()
        .chain(old.keys().filter(|key| !new.contains_key(key)));
    for &(caller, callee, relation) in keys {
        by_caller
            .entry(caller)
            .or_default()
            .push((callee, relation));
    }
    by_caller
}

/// One edge per (from, to, relation) between included nodes, ordered by
/// caller then position, with the pair kept for later walks. `new` and
/// `old` hold the relations of included callers only.
fn edges(
    included: &HashSet<NodeId>,
    display_ids: &HashMap<NodeId, String>,
    outgoing: &Outgoing,
    new: &LinkedCalls<'_>,
    old: &LinkedCalls<'_>,
) -> Vec<(NodeId, NodeId, FlowEdge)> {
    let mut callers: Vec<NodeId> = outgoing.keys().copied().collect();
    callers.sort_by(|left, right| display_ids[left].cmp(&display_ids[right]));
    let mut ordered = Vec::new();
    for caller in callers {
        let mut drawn: Vec<(NodeId, FlowEdge)> = outgoing[&caller]
            .iter()
            .filter(|(callee, _)| included.contains(callee))
            .filter_map(|&(callee, relation)| {
                let (call, added, removed) = pair_call((caller, callee, relation), new, old)?;
                Some((
                    callee,
                    FlowEdge {
                        from: display_ids[&caller].clone(),
                        to: display_ids[&callee].clone(),
                        position: 0,
                        line: call.line,
                        kind: call.kind,
                        resolution: call.resolution,
                        added,
                        removed,
                        relation,
                        evidence: call.evidence.map(str::to_string),
                    },
                ))
            })
            .collect();
        drawn.sort_by(|(_, left), (_, right)| {
            left.line
                .cmp(&right.line)
                .then_with(|| left.to.cmp(&right.to))
                .then_with(|| left.relation.cmp(&right.relation))
        });
        for (rank, (callee, mut edge)) in drawn.into_iter().enumerate() {
            edge.position = u32::try_from(rank + 1).unwrap_or(u32::MAX);
            ordered.push((caller, callee, edge));
        }
    }
    ordered
}

/// Every direct relation of the included `caller`, drawn or not, by line.
fn calls_of(
    graph: &Graph<'_>,
    caller: NodeId,
    included: &HashSet<NodeId>,
    display_ids: &HashMap<NodeId, String>,
    outgoing: &Outgoing,
    new: &LinkedCalls<'_>,
    old: &LinkedCalls<'_>,
) -> Vec<FlowCall> {
    let mut calls: Vec<FlowCall> = outgoing
        .get(&caller)
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|&(callee, relation)| {
            let (call, added, removed) = pair_call((caller, callee, relation), new, old)?;
            let node = &graph.nodes[callee];
            let qualified_name = match node.current() {
                Some(function) => function.qualified_name.clone(),
                None => node.package.unwrap_or_default().to_string(),
            };
            Some(FlowCall {
                callee_id: display_ids
                    .get(&callee)
                    .cloned()
                    .unwrap_or_else(|| node.id.to_string()),
                qualified_name,
                line: call.line,
                kind: call.kind,
                external: node.package.is_some(),
                shown: included.contains(&callee),
                added,
                removed,
                relation,
                evidence: call.evidence.map(str::to_string),
            })
        })
        .collect();
    calls.sort_by(|left, right| {
        left.line
            .cmp(&right.line)
            .then_with(|| left.callee_id.cmp(&right.callee_id))
            .then_with(|| left.relation.cmp(&right.relation))
    });
    calls
}

/// For each included package node, the distinct functions of it that
/// included nodes call on either side.
fn external_functions(graph: &Graph<'_>, included: &HashSet<NodeId>) -> HashMap<NodeId, u32> {
    let mut called: HashMap<NodeId, HashSet<&str>> = HashMap::new();
    for side in [Side::Old, Side::New] {
        for &(caller, callee, call) in &graph.calls[side as usize] {
            if graph.nodes[callee].package.is_some()
                && included.contains(&caller)
                && included.contains(&callee)
            {
                called.entry(callee).or_default().insert(&call.callee);
            }
        }
    }
    called
        .into_iter()
        .map(|(node, functions)| (node, count(functions.len())))
        .collect()
}

/// Included entry points from which a changed function can be reached along
/// the shown edges.
fn entry_points(
    graph: &Graph<'_>,
    included: &Selection,
    seeds: &[NodeId],
    edges: &[(NodeId, NodeId, FlowEdge)],
    display_ids: &HashMap<NodeId, String>,
) -> Vec<String> {
    let mut callers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (caller, callee, _) in edges {
        callers.entry(*callee).or_default().push(*caller);
    }
    let mut reaching: HashSet<NodeId> = seeds.iter().copied().collect();
    let mut queue: VecDeque<NodeId> = seeds.iter().copied().collect();
    while let Some(node) = queue.pop_front() {
        for &caller in callers.get(&node).map_or(&[][..], Vec::as_slice) {
            if reaching.insert(caller) {
                queue.push_back(caller);
            }
        }
    }
    let mut found: Vec<String> = included
        .order
        .iter()
        .filter(|&&node| reaching.contains(&node))
        .filter(|&&node| {
            graph.nodes[node]
                .current()
                .is_some_and(|function| !function.entry_points.is_empty())
        })
        .map(|node| display_ids[node].clone())
        .collect();
    found.sort();
    found
}

fn flow_node(
    node: &Node<'_>,
    id: String,
    calls: Vec<FlowCall>,
    external_functions: u32,
) -> FlowNode {
    if let Some(function) = node.current() {
        return FlowNode {
            id,
            name: function.name.clone(),
            qualified_name: function.qualified_name.clone(),
            container: function.container.clone(),
            state: node.state,
            file: Some(function.file.clone()),
            start_line: Some(function.start_line),
            end_line: Some(function.end_line),
            additions: node.additions,
            deletions: node.deletions,
            complexity: function.complexity,
            previous_complexity: node.old.and_then(|old| old.complexity),
            generated: function.generated,
            test: function.test,
            entry_points: function
                .entry_points
                .iter()
                .map(|entry| EntryPoint {
                    kind: entry.kind,
                    label: entry.label.clone(),
                })
                .collect(),
            calls,
            external_functions: 0,
        };
    }
    let package = node.package.unwrap_or_default().to_string();
    FlowNode {
        id,
        name: package.clone(),
        qualified_name: package.clone(),
        container: package,
        state: FlowNodeState::External,
        file: None,
        start_line: None,
        end_line: None,
        additions: 0,
        deletions: 0,
        complexity: None,
        previous_complexity: None,
        generated: false,
        test: false,
        entry_points: Vec::new(),
        calls,
        external_functions,
    }
}

/// "new: error: a/b.go:12: message"
fn diagnostic(side: &str, diagnostic: &Diagnostic) -> String {
    let level = match diagnostic.level {
        DiagnosticLevel::Warning => "warning",
        DiagnosticLevel::Error => "error",
    };
    let location = match (&diagnostic.file, diagnostic.line) {
        (Some(file), Some(line)) => format!("{file}:{line}: "),
        (Some(file), None) => format!("{file}: "),
        _ => String::new(),
    };
    format!("{side}: {level}: {location}{}", diagnostic.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::analysis::fixtures::{
        analysis, call, entry, external, function, in_test, reference, registration,
    };

    const CALLERS: FlowOptions = FlowOptions {
        include_tests: false,
        include_callers: true,
        include_callees: false,
    };
    const CALLEES: FlowOptions = FlowOptions {
        include_tests: false,
        include_callers: false,
        include_callees: true,
    };
    const CONTEXT: FlowOptions = FlowOptions {
        include_tests: false,
        include_callers: true,
        include_callees: true,
    };
    const TESTS: FlowOptions = FlowOptions {
        include_tests: true,
        include_callers: false,
        include_callees: false,
    };

    fn build_graph(old: &Analysis, new: &Analysis, lines: &ChangedLines, max: usize) -> FlowGraph {
        build_with(old, new, lines, max, FlowOptions::default())
    }

    fn build_with(
        old: &Analysis,
        new: &Analysis,
        lines: &ChangedLines,
        max: usize,
        options: FlowOptions,
    ) -> FlowGraph {
        build(&GraphRequest {
            languages: vec![LanguageSides { old, new }],
            lines,
            old_sha: "old",
            new_sha: Some("new"),
            analyzed_at: "2026-11-16T00:00:00Z".to_string(),
            max_nodes: max,
            options,
        })
    }

    fn node<'g>(graph: &'g FlowGraph, id: &str) -> &'g FlowNode {
        graph
            .nodes
            .iter()
            .find(|node| node.id == id)
            .unwrap_or_else(|| panic!("no node {id}: {:?}", ids(graph)))
    }

    fn ids(graph: &FlowGraph) -> Vec<&str> {
        graph.nodes.iter().map(|node| node.id.as_str()).collect()
    }

    /// (from, to, position, line, added, removed)
    fn edge_summary(graph: &FlowGraph) -> Vec<(&str, &str, u32, u32, bool, bool)> {
        graph
            .edges
            .iter()
            .map(|e| {
                (
                    e.from.as_str(),
                    e.to.as_str(),
                    e.position,
                    e.line,
                    e.added,
                    e.removed,
                )
            })
            .collect()
    }

    /// (callee, line, shown, added, removed)
    fn call_summary<'g>(graph: &'g FlowGraph, id: &str) -> Vec<(&'g str, u32, bool, bool, bool)> {
        node(graph, id)
            .calls
            .iter()
            .map(|c| (c.callee_id.as_str(), c.line, c.shown, c.added, c.removed))
            .collect()
    }

    /// (from, to, relation, evidence, added, removed)
    type RelationSummary<'g> = (&'g str, &'g str, Relation, Option<&'g str>, bool, bool);

    fn relation_summary(graph: &FlowGraph) -> Vec<RelationSummary<'_>> {
        graph
            .edges
            .iter()
            .map(|e| {
                (
                    e.from.as_str(),
                    e.to.as_str(),
                    e.relation,
                    e.evidence.as_deref(),
                    e.added,
                    e.removed,
                )
            })
            .collect()
    }

    fn hidden(
        changed_tests: u32,
        callers: u32,
        callees: u32,
        external_packages: u32,
    ) -> FlowHidden {
        FlowHidden {
            changed_tests,
            callers,
            callees,
            external_packages,
        }
    }

    #[test]
    fn states_and_line_counts_follow_the_changed_lines() {
        let mut old_c = function("p.C", "a.go", 20, 25);
        old_c.complexity = Some(4);
        let mut new_c = function("p.C", "a.go", 18, 22);
        new_c.complexity = Some(3);
        let old = analysis(
            vec![
                function("p.A", "a.go", 1, 5),
                function("p.B", "a.go", 7, 10),
                function("p.D", "a.go", 12, 14),
                old_c,
            ],
            vec![],
        );
        let new = analysis(
            vec![
                function("p.A", "a.go", 1, 6),
                function("p.B", "a.go", 8, 11),
                new_c,
                function("p.N", "new.go", 1, 4),
            ],
            vec![],
        );
        let lines = ChangedLines::default()
            .with_added("a.go", 3, 3)
            .with_removed("a.go", 12, 14)
            .with_removed("a.go", 22, 22)
            .with_added("new.go", 1, 4);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        let summary = |id: &str| {
            let n = node(&graph, id);
            (n.state, n.additions, n.deletions)
        };
        assert_eq!(summary("p.A"), (FlowNodeState::Modified, 1, 0));
        assert_eq!(summary("p.C"), (FlowNodeState::Modified, 0, 1));
        assert_eq!(summary("p.D"), (FlowNodeState::Deleted, 0, 3));
        assert_eq!(summary("p.N"), (FlowNodeState::New, 4, 0));
        assert!(
            graph.nodes.iter().all(|n| n.id != "p.B"),
            "untouched and unrelated"
        );
        let c = node(&graph, "p.C");
        assert_eq!((c.complexity, c.previous_complexity), (Some(3), Some(4)));
        assert_eq!((c.start_line, c.end_line), (Some(18), Some(22)));
        let d = node(&graph, "p.D");
        assert_eq!((d.file.as_deref(), d.start_line), (Some("a.go"), Some(12)));
        assert_eq!(node(&graph, "p.N").previous_complexity, None);
        assert!(!graph.truncated);
        assert_eq!(graph.options, FlowOptions::default());
    }

    #[test]
    fn a_renamed_file_counts_each_side_under_its_own_path() {
        let old = analysis(vec![function("p.F", "old.go", 5, 9)], vec![]);
        let new = analysis(vec![function("p.F", "new.go", 5, 10)], vec![]);
        let lines = ChangedLines::default()
            .with_removed("old.go", 6, 6)
            .with_added("new.go", 6, 7)
            // Lines of the other file at the same numbers do not count.
            .with_added("old.go", 5, 9);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        let f = node(&graph, "p.F");
        assert_eq!(
            (f.state, f.additions, f.deletions),
            (FlowNodeState::Modified, 2, 1)
        );
        assert_eq!(f.file.as_deref(), Some("new.go"));
    }

    fn walk_sides() -> (Analysis, Analysis, ChangedLines) {
        let functions = || {
            vec![
                function("p.main", "main.go", 1, 3),
                entry(function("p.Handle", "h.go", 1, 9), "POST /x"),
                function("p.Middle", "m.go", 1, 9),
                function("p.X", "x.go", 1, 9),
                function("p.Y", "y.go", 1, 9),
                function("p.DeepY", "y.go", 20, 29),
                function("p.Unrelated", "u.go", 1, 9),
            ]
        };
        let calls = vec![
            call("p.main", "p.Handle", 2),
            call("p.Handle", "p.Middle", 3),
            call("p.Middle", "p.X", 4),
            call("p.X", "p.Y", 5),
            call("p.X", "fmt.Println", 6),
            call("p.Y", "p.DeepY", 7),
            call("p.Unrelated", "p.Middle", 8),
        ];
        let old = analysis(functions(), calls.clone());
        let mut new = analysis(functions(), calls);
        new.externals.push(external("fmt.Println", "fmt"));
        let lines = ChangedLines::default().with_added("x.go", 2, 2);
        (old, new, lines)
    }

    #[test]
    fn the_default_scope_is_the_changed_functions_alone() {
        let (old, new, lines) = walk_sides();

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(ids(&graph), vec!["p.X"]);
        assert!(graph.edges.is_empty());
        assert!(graph.entry_points.is_empty());
        assert_eq!(graph.hidden, hidden(0, 3, 1, 1));
        assert!(!graph.truncated);
        assert_eq!(
            call_summary(&graph, "p.X"),
            vec![
                ("p.Y", 5, false, false, false),
                ("external:fmt", 6, false, false, false)
            ]
        );
    }

    #[test]
    fn callers_are_walked_to_entry_points_with_the_option() {
        let (old, new, lines) = walk_sides();

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLERS);

        assert_eq!(
            ids(&graph),
            vec!["p.Handle", "p.Middle", "p.Unrelated", "p.X"]
        );
        assert_eq!(graph.entry_points, vec!["p.Handle"]);
        assert_eq!(node(&graph, "p.Handle").entry_points[0].label, "POST /x");
        assert_eq!(graph.hidden, hidden(0, 3, 1, 1), "counted when on too");
        assert_eq!(graph.options, CALLERS);
        // Still listed, as not shown.
        assert_eq!(
            call_summary(&graph, "p.X"),
            vec![
                ("p.Y", 5, false, false, false),
                ("external:fmt", 6, false, false, false)
            ]
        );
        let println = &node(&graph, "p.X").calls[1];
        assert!(println.external);
        assert_eq!(println.qualified_name, "fmt");
        assert_eq!(node(&graph, "p.X").calls[0].qualified_name, "pkg.Y");
        assert!(node(&graph, "p.Middle").calls.is_empty(), "untouched");
    }

    #[test]
    fn callees_are_drawn_with_the_option() {
        let (old, new, lines) = walk_sides();

        let alone = build_with(&old, &new, &lines, MAX_NODES, CALLEES);
        assert_eq!(ids(&alone), vec!["external:fmt", "p.X", "p.Y"]);
        assert_eq!(alone.hidden, hidden(0, 3, 1, 1));
        assert_eq!(alone.options, CALLEES);

        let graph = build_with(&old, &new, &lines, MAX_NODES, CONTEXT);

        assert_eq!(
            ids(&graph),
            vec![
                "external:fmt",
                "p.Handle",
                "p.Middle",
                "p.Unrelated",
                "p.X",
                "p.Y"
            ]
        );
        let fmt = node(&graph, "external:fmt");
        assert_eq!(fmt.state, FlowNodeState::External);
        assert_eq!(
            (
                fmt.name.as_str(),
                fmt.container.as_str(),
                fmt.file.as_deref()
            ),
            ("fmt", "fmt", None)
        );
        assert_eq!(fmt.external_functions, 1);
        assert_eq!(graph.hidden, hidden(0, 3, 1, 1), "counted when on too");
        assert!(node(&graph, "p.X").calls.iter().all(|call| call.shown));
        assert_eq!(graph.options, CONTEXT);
    }

    #[test]
    fn a_function_both_caller_and_callee_is_counted_once_the_other_is_on() {
        let functions = || vec![function("p.A", "a.go", 1, 9), function("p.B", "b.go", 1, 9)];
        // A and B call each other; only A changed.
        let calls = vec![call("p.A", "p.B", 2), call("p.B", "p.A", 3)];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);
        let lines = ChangedLines::default().with_added("a.go", 4, 4);

        let default = build_graph(&old, &new, &lines, MAX_NODES);
        assert_eq!(default.hidden, hidden(0, 1, 1, 0));

        let callers = build_with(&old, &new, &lines, MAX_NODES, CALLERS);
        assert_eq!(ids(&callers), vec!["p.A", "p.B"]);
        assert_eq!(callers.hidden, hidden(0, 1, 0, 0), "B is already drawn");

        let callees = build_with(&old, &new, &lines, MAX_NODES, CALLEES);
        assert_eq!(callees.hidden, hidden(0, 0, 1, 0), "B is already drawn");
    }

    #[test]
    fn externals_are_grouped_per_package_with_one_edge_per_caller() {
        let functions = || {
            vec![
                function("p.A", "a.go", 1, 30),
                function("p.B", "b.go", 1, 9),
            ]
        };
        let mut async_query = call("p.A", "database/sql.(*DB).Query", 12);
        async_query.kind = CallKind::Async;
        let mut conditional_exec = call("p.A", "database/sql.(*DB).Exec", 15);
        conditional_exec.kind = CallKind::Conditional;
        let new_calls = vec![
            async_query,
            conditional_exec,
            call("p.A", "database/sql.Open", 20),
            call("p.A", "database/sql.Open", 22),
            call("p.A", "fmt.Println", 25),
            call("p.A", "p.B", 26),
            call("p.B", "database/sql.Open", 3),
        ];
        let mut old = analysis(functions(), vec![call("p.A", "database/sql.Open", 18)]);
        old.externals
            .push(external("database/sql.Open", "database/sql"));
        let mut new = analysis(functions(), new_calls);
        new.externals = vec![
            external("database/sql.(*DB).Query", "database/sql"),
            external("database/sql.(*DB).Exec", "database/sql"),
            external("database/sql.Open", "database/sql"),
            external("fmt.Println", "fmt"),
        ];
        let lines = ChangedLines::default().with_added("a.go", 12, 25);

        let default = build_graph(&old, &new, &lines, MAX_NODES);
        assert_eq!(ids(&default), vec!["p.A"]);
        assert_eq!(default.hidden, hidden(0, 0, 1, 2));

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLEES);

        assert_eq!(
            ids(&graph),
            vec!["external:database/sql", "external:fmt", "p.A", "p.B"]
        );
        assert_eq!(node(&graph, "external:database/sql").external_functions, 3);
        assert_eq!(node(&graph, "external:fmt").external_functions, 1);
        assert_eq!(
            edge_summary(&graph),
            vec![
                ("p.A", "external:database/sql", 1, 12, false, false),
                ("p.A", "external:fmt", 2, 25, true, false),
                ("p.A", "p.B", 3, 26, true, false),
                ("p.B", "external:database/sql", 1, 3, true, false),
            ]
        );
        assert_eq!(
            graph.edges[0].kind,
            CallKind::Always,
            "the strongest of async, conditional and always"
        );
        assert_eq!(
            call_summary(&graph, "p.A"),
            vec![
                ("external:database/sql", 12, true, false, false),
                ("external:fmt", 25, true, true, false),
                ("p.B", 26, true, true, false),
            ]
        );
    }

    #[test]
    fn calls_list_what_a_changed_function_calls_added_and_removed() {
        let functions = || {
            vec![
                function("p.A", "a.go", 1, 20),
                function("p.B", "b.go", 1, 5),
                function("p.C", "c.go", 1, 5),
                function("p.D", "d.go", 1, 5),
            ]
        };
        let old = analysis(
            functions(),
            vec![call("p.A", "p.B", 9), call("p.A", "p.D", 3)],
        );
        let new = analysis(
            functions(),
            vec![call("p.A", "p.B", 10), call("p.A", "p.C", 5)],
        );
        let lines = ChangedLines::default()
            .with_added("a.go", 5, 5)
            .with_added("c.go", 2, 2);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(ids(&graph), vec!["p.A", "p.C"]);
        assert_eq!(
            call_summary(&graph, "p.A"),
            vec![
                ("p.D", 3, false, false, true),
                ("p.C", 5, true, true, false),
                ("p.B", 10, false, false, false),
            ]
        );
        assert_eq!(graph.hidden, hidden(0, 0, 2, 0), "B and D; C is changed");
    }

    #[test]
    fn a_link_between_two_unchanged_functions_never_appears() {
        let functions = || {
            vec![
                function("p.A", "a.go", 1, 9),
                function("p.X", "x.go", 1, 9),
                function("p.Y", "y.go", 1, 9),
            ]
        };
        let calls = vec![call("p.X", "p.Y", 2), call("p.A", "p.X", 3)];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);
        let lines = ChangedLines::default().with_added("a.go", 4, 4);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(ids(&graph), vec!["p.A"]);
        assert!(graph.edges.is_empty());
        assert_eq!(
            call_summary(&graph, "p.A"),
            vec![("p.X", 3, false, false, false)]
        );
    }

    #[test]
    fn a_kept_relation_between_two_changed_functions_is_neither_added_nor_removed() {
        let old = analysis(
            vec![
                function("p.A", "a.go", 1, 20),
                function("p.B", "b.go", 1, 9),
                function("p.D", "d.go", 1, 9),
            ],
            vec![call("p.A", "p.B", 4), call("p.A", "p.D", 6)],
        );
        let new = analysis(
            vec![
                function("p.A", "a.go", 1, 20),
                function("p.B", "b.go", 1, 9),
                function("p.C", "c.go", 1, 9),
            ],
            vec![call("p.A", "p.B", 4), call("p.A", "p.C", 8)],
        );
        let lines = ChangedLines::default()
            .with_added("a.go", 8, 8)
            .with_removed("a.go", 6, 6)
            .with_added("b.go", 2, 2)
            .with_added("c.go", 1, 9)
            .with_removed("d.go", 1, 9);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(ids(&graph), vec!["p.A", "p.B", "p.C", "p.D"]);
        assert_eq!(
            edge_summary(&graph),
            vec![
                ("p.A", "p.B", 1, 4, false, false),
                ("p.A", "p.D", 2, 6, false, true),
                ("p.A", "p.C", 3, 8, true, false),
            ]
        );
    }

    #[test]
    fn registrations_and_references_are_links_of_their_own_with_their_evidence() {
        let functions = || {
            vec![
                function("p.Routes", "routes.go", 1, 20),
                function("p.Create", "create.go", 1, 9),
                function("p.Audit", "audit.go", 1, 9),
                function("p.Logger", "log.go", 1, 9),
            ]
        };
        let old = analysis(
            functions(),
            vec![registration("p.Routes", "p.Logger", 3, "middleware")],
        );
        let new = analysis(
            functions(),
            vec![
                registration("p.Routes", "p.Logger", 3, "middleware"),
                registration("p.Routes", "p.Create", 5, "route POST /refunds"),
                registration("p.Routes", "p.Create", 9, "route PUT /refunds"),
                call("p.Routes", "p.Create", 7),
                reference("p.Create", "p.Audit", 4),
            ],
        );
        let lines = ChangedLines::default()
            .with_added("routes.go", 5, 9)
            .with_added("create.go", 1, 9)
            .with_added("audit.go", 2, 2)
            .with_added("log.go", 2, 2);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(
            relation_summary(&graph),
            vec![
                (
                    "p.Create",
                    "p.Audit",
                    Relation::Reference,
                    None,
                    true,
                    false
                ),
                (
                    "p.Routes",
                    "p.Logger",
                    Relation::Registration,
                    Some("middleware"),
                    false,
                    false
                ),
                (
                    "p.Routes",
                    "p.Create",
                    Relation::Registration,
                    Some("route POST /refunds"),
                    true,
                    false
                ),
                ("p.Routes", "p.Create", Relation::Call, None, true, false),
            ]
        );
        assert_eq!(
            graph
                .edges
                .iter()
                .map(|edge| (edge.position, edge.line))
                .collect::<Vec<_>>(),
            vec![(1, 4), (1, 3), (2, 5), (3, 7)],
            "the first line of each relation"
        );
        let routes = &node(&graph, "p.Routes").calls;
        assert_eq!(
            routes
                .iter()
                .map(|c| (c.callee_id.as_str(), c.relation, c.evidence.as_deref()))
                .collect::<Vec<_>>(),
            vec![
                ("p.Logger", Relation::Registration, Some("middleware")),
                (
                    "p.Create",
                    Relation::Registration,
                    Some("route POST /refunds")
                ),
                ("p.Create", Relation::Call, None),
            ]
        );
    }

    #[test]
    fn a_link_through_an_unchanged_function_is_not_drawn() {
        let functions = || {
            vec![
                function("p.A", "a.go", 1, 9),
                function("p.U", "u.go", 1, 9),
                function("p.B", "b.go", 1, 9),
            ]
        };
        let calls = vec![call("p.A", "p.U", 2), call("p.U", "p.B", 3)];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);
        let lines = ChangedLines::default()
            .with_added("a.go", 4, 4)
            .with_added("b.go", 4, 4);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert_eq!(ids(&graph), vec!["p.A", "p.B"]);
        assert!(graph.edges.is_empty(), "{:?}", graph.edges);

        // Drawn once U is: as a callee of A.
        let around = build_with(&old, &new, &lines, MAX_NODES, CALLEES);
        assert_eq!(
            edge_summary(&around),
            vec![
                ("p.A", "p.U", 1, 2, false, false),
                ("p.U", "p.B", 1, 3, false, false),
            ]
        );
    }

    #[test]
    fn deleted_functions_are_walked_on_the_old_side() {
        let old = analysis(
            vec![
                entry(function("p.Route", "r.go", 1, 9), "GET /old"),
                function("p.Gone", "g.go", 1, 9),
                function("p.Inner", "i.go", 1, 9),
            ],
            vec![call("p.Route", "p.Gone", 3), call("p.Gone", "p.Inner", 4)],
        );
        let new = analysis(
            vec![
                entry(function("p.Route", "r.go", 1, 8), "GET /old"),
                function("p.Inner", "i.go", 1, 9),
            ],
            vec![],
        );
        let lines = ChangedLines::default()
            .with_removed("g.go", 1, 9)
            .with_removed("r.go", 3, 3);

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLEES);

        assert_eq!(ids(&graph), vec!["p.Gone", "p.Inner", "p.Route"]);
        assert_eq!(node(&graph, "p.Route").state, FlowNodeState::Modified);
        assert_eq!(
            edge_summary(&graph),
            vec![
                ("p.Gone", "p.Inner", 1, 4, false, true),
                ("p.Route", "p.Gone", 1, 3, false, true),
            ]
        );
        assert_eq!(graph.entry_points, vec!["p.Route"]);
        assert_eq!(
            call_summary(&graph, "p.Gone"),
            vec![("p.Inner", 4, true, false, true)]
        );
    }

    #[test]
    fn generated_functions_do_not_start_the_scope() {
        let mut generated = function("p.Gen", "gen.go", 1, 9);
        generated.generated = true;
        let old = analysis(vec![function("p.Caller", "c.go", 1, 9)], vec![]);
        let new = analysis(
            vec![generated, function("p.Caller", "c.go", 1, 9)],
            vec![call("p.Caller", "p.Gen", 2)],
        );
        let lines = ChangedLines::default().with_added("gen.go", 1, 9);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert!(graph.nodes.is_empty(), "{:?}", ids(&graph));
    }

    fn test_sides() -> (Analysis, Analysis, ChangedLines) {
        let functions = || {
            vec![
                entry(function("p.Route", "r.go", 1, 9), "GET /x"),
                function("p.Prod", "x.go", 1, 9),
                in_test(entry(function("p.TestProd", "x_test.go", 1, 9), "TestProd")),
                in_test(function("p.TestProd$1", "x_test.go", 3, 5)),
                in_test(function("p.helper", "x_test.go", 11, 15)),
                in_test(entry(function("p.TestOld", "old_test.go", 1, 9), "TestOld")),
            ]
        };
        let calls = vec![
            call("p.Route", "p.Prod", 2),
            call("p.TestProd", "p.TestProd$1", 2),
            call("p.TestProd$1", "p.helper", 4),
            call("p.helper", "p.Prod", 12),
            call("p.TestOld", "p.Prod", 3),
        ];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);
        let lines = ChangedLines::default()
            .with_added("x.go", 2, 2)
            .with_added("x_test.go", 1, 15);
        (old, new, lines)
    }

    #[test]
    fn tests_are_neither_changed_functions_nor_callers_by_default() {
        let (old, new, lines) = test_sides();

        let graph = build_graph(&old, &new, &lines, MAX_NODES);
        assert_eq!(ids(&graph), vec!["p.Prod"]);
        assert_eq!(graph.hidden, hidden(3, 1, 0, 0), "Route, not the tests");

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLERS);

        assert_eq!(ids(&graph), vec!["p.Prod", "p.Route"]);
        assert_eq!(graph.hidden, hidden(3, 1, 0, 0));
        assert_eq!(
            edge_summary(&graph),
            vec![("p.Route", "p.Prod", 1, 2, false, false)]
        );
        assert_eq!(graph.entry_points, vec!["p.Route"]);
    }

    #[test]
    fn tests_are_included_with_the_option() {
        let (old, new, lines) = test_sides();

        let changed = build_with(&old, &new, &lines, MAX_NODES, TESTS);
        assert_eq!(
            ids(&changed),
            vec!["p.Prod", "p.TestProd", "p.TestProd$1", "p.helper"]
        );
        assert_eq!(changed.hidden, hidden(3, 2, 0, 0), "Route and TestOld");
        assert_eq!(changed.entry_points, vec!["p.TestProd"]);
        assert_eq!(
            edge_summary(&changed),
            vec![
                ("p.TestProd", "p.TestProd$1", 1, 2, false, false),
                ("p.TestProd$1", "p.helper", 1, 4, false, false),
                ("p.helper", "p.Prod", 1, 12, false, false),
            ]
        );

        let graph = build_with(
            &old,
            &new,
            &lines,
            MAX_NODES,
            FlowOptions {
                include_callers: true,
                ..TESTS
            },
        );

        assert_eq!(
            ids(&graph),
            vec![
                "p.Prod",
                "p.Route",
                "p.TestOld",
                "p.TestProd",
                "p.TestProd$1",
                "p.helper"
            ]
        );
        assert!(node(&graph, "p.TestProd$1").test);
        assert!(!node(&graph, "p.Prod").test);
        assert_eq!(node(&graph, "p.TestOld").state, FlowNodeState::Untouched);
        assert_eq!(graph.hidden, hidden(3, 2, 0, 0), "counted when on too");
        assert_eq!(
            graph.entry_points,
            vec!["p.Route", "p.TestOld", "p.TestProd"]
        );
    }

    #[test]
    fn a_review_of_tests_only_has_no_node_by_default() {
        let (old, new, _) = test_sides();
        let lines = ChangedLines::default().with_added("x_test.go", 12, 12);

        let graph = build_graph(&old, &new, &lines, MAX_NODES);

        assert!(graph.nodes.is_empty(), "{:?}", ids(&graph));
        assert_eq!(graph.hidden.changed_tests, 1);
    }

    #[test]
    fn edges_are_added_removed_deduplicated_and_ranked_by_line() {
        let functions = || {
            vec![
                function("p.A", "a.go", 1, 20),
                function("p.B", "b.go", 1, 5),
                function("p.C", "c.go", 1, 5),
                function("p.D", "d.go", 1, 5),
            ]
        };
        let old = analysis(
            functions(),
            vec![call("p.A", "p.B", 9), call("p.A", "p.D", 3)],
        );
        let mut dynamic = call("p.A", "p.C", 8);
        dynamic.kind = CallKind::Async;
        dynamic.resolution = CallResolution::Dynamic;
        let new = analysis(
            functions(),
            vec![call("p.A", "p.B", 10), dynamic, call("p.A", "p.C", 5)],
        );
        let lines = ChangedLines::default().with_added("a.go", 5, 5);

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLEES);

        assert_eq!(
            edge_summary(&graph),
            vec![
                ("p.A", "p.D", 1, 3, false, true),
                ("p.A", "p.C", 2, 5, true, false),
                ("p.A", "p.B", 3, 10, false, false),
            ]
        );
        assert_eq!(
            graph.edges[1].kind,
            CallKind::Always,
            "the first call of the pair"
        );
    }

    #[test]
    fn entry_points_must_reach_a_changed_function() {
        let functions = || {
            vec![
                entry(function("p.Route", "r.go", 1, 9), "GET /a"),
                function("p.X", "x.go", 1, 9),
                entry(function("p.Callback", "cb.go", 1, 9), "payment.settled"),
            ]
        };
        let calls = vec![call("p.Route", "p.X", 2), call("p.X", "p.Callback", 3)];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);
        let lines = ChangedLines::default().with_added("x.go", 4, 4);

        let graph = build_with(&old, &new, &lines, MAX_NODES, CONTEXT);

        assert_eq!(ids(&graph), vec!["p.Callback", "p.Route", "p.X"]);
        assert_eq!(graph.entry_points, vec!["p.Route"]);
    }

    #[test]
    fn entry_points_are_the_drawn_changed_ones_by_default() {
        let functions = || {
            vec![
                entry(function("p.Route", "r.go", 1, 9), "GET /a"),
                function("p.X", "x.go", 1, 9),
            ]
        };
        let calls = vec![call("p.Route", "p.X", 2)];
        let old = analysis(functions(), calls.clone());
        let new = analysis(functions(), calls);

        let only_x = ChangedLines::default().with_added("x.go", 4, 4);
        assert!(build_graph(&old, &new, &only_x, MAX_NODES)
            .entry_points
            .is_empty());

        let both = only_x.with_added("r.go", 3, 3);
        let graph = build_graph(&old, &new, &both, MAX_NODES);
        assert_eq!(graph.entry_points, vec!["p.Route"]);
        assert_eq!(
            edge_summary(&graph),
            vec![("p.Route", "p.X", 1, 2, false, false)]
        );
    }

    #[test]
    fn a_large_scope_keeps_changed_functions_and_the_shortest_path_to_an_entry() {
        let mut functions = vec![
            function("p.X", "x.go", 1, 9),
            function("p.Y", "y.go", 1, 9),
            function("p.A1", "a.go", 1, 9),
            function("p.A2", "a.go", 10, 19),
            entry(function("p.Entry", "e.go", 1, 9), "GET /"),
        ];
        let mut calls = vec![
            call("p.X", "p.Y", 2),
            call("p.A1", "p.X", 2),
            call("p.A2", "p.A1", 12),
            call("p.Entry", "p.A2", 3),
        ];
        for n in 0..10 {
            let id = format!("p.Fan{n:02}");
            functions.push(function(&id, "fan.go", n * 10 + 1, n * 10 + 9));
            calls.push(call(&id, "p.X", n * 10 + 2));
        }
        let old = analysis(functions.clone(), calls.clone());
        let new = analysis(functions, calls);
        let lines = ChangedLines::default().with_added("x.go", 3, 3);

        let full = build_with(&old, &new, &lines, MAX_NODES, CONTEXT);
        assert_eq!(full.nodes.len(), 15);
        assert!(!full.truncated);

        // The limit counts what the options add (here 4), not the changed
        // function. Callees come before the other callers.
        let cut = build_with(&old, &new, &lines, 4, CONTEXT);
        assert!(cut.truncated);
        assert_eq!(ids(&cut), vec!["p.A1", "p.A2", "p.Entry", "p.X", "p.Y"]);
        assert_eq!(cut.entry_points, vec!["p.Entry"]);
        assert!(node(&cut, "p.X").calls[0].shown);

        let tiny = build_with(&old, &new, &lines, 1, CONTEXT);
        assert_eq!(ids(&tiny), vec!["p.A1", "p.X"]);
        assert!(!node(&tiny, "p.X").calls[0].shown, "Y was cut");

        // Without callees, the nearest callers fill the room.
        let cut = build_with(&old, &new, &lines, 4, CALLERS);
        assert!(cut.truncated);
        assert_eq!(ids(&cut), vec!["p.A1", "p.A2", "p.Entry", "p.Fan00", "p.X"]);

        // Callees alone are cut in their order, with no path up.
        let cut = build_with(&old, &new, &lines, 0, CALLEES);
        assert!(cut.truncated);
        assert_eq!(ids(&cut), vec!["p.X"]);
    }

    #[test]
    fn the_changed_functions_are_never_cut() {
        let functions: Vec<Function> = (1..=4)
            .map(|n| function(&format!("p.F{n}"), "f.go", n * 10, n * 10 + 5))
            .collect();
        let old = analysis(vec![], vec![]);
        let new = analysis(functions, vec![]);
        let lines = ChangedLines::default().with_added("f.go", 1, 100);

        let graph = build_graph(&old, &new, &lines, 0);

        assert!(!graph.truncated);
        assert_eq!(graph.nodes.len(), 4);
    }

    #[test]
    fn languages_are_merged_and_colliding_ids_are_prefixed() {
        let go_old = analysis(vec![], vec![]);
        let go_new = analysis(vec![function("main", "main.go", 1, 3)], vec![]);
        let mut py_old = analysis(vec![], vec![]);
        py_old.language = "python".to_string();
        let mut py_new = analysis(
            vec![
                function("main", "main.py", 1, 3),
                function("only.py", "o.py", 1, 2),
            ],
            vec![],
        );
        py_new.language = "python".to_string();
        py_new.diagnostics.push(Diagnostic {
            level: DiagnosticLevel::Error,
            message: "could not import".to_string(),
            file: Some("main.py".to_string()),
            line: Some(2),
        });
        let lines = ChangedLines::default()
            .with_added("main.go", 1, 3)
            .with_added("main.py", 1, 3)
            .with_added("o.py", 1, 2);

        let graph = build(&GraphRequest {
            languages: vec![
                LanguageSides {
                    old: &go_old,
                    new: &go_new,
                },
                LanguageSides {
                    old: &py_old,
                    new: &py_new,
                },
            ],
            lines: &lines,
            old_sha: "a",
            new_sha: None,
            analyzed_at: String::new(),
            max_nodes: MAX_NODES,
            options: FlowOptions::default(),
        });

        assert_eq!(ids(&graph), vec!["go:main", "only.py", "python:main"]);
        assert_eq!(graph.language, "go+python");
        assert_eq!(
            graph.diagnostics,
            vec!["new: error: main.py:2: could not import"]
        );
        assert_eq!(graph.new_sha, None);
    }

    #[test]
    fn calls_to_unknown_ids_are_ignored() {
        let old = analysis(vec![], vec![]);
        let new = analysis(
            vec![function("p.X", "x.go", 1, 9)],
            vec![call("p.X", "nowhere.Fn", 2), call("ghost.Fn", "p.X", 3)],
        );
        let lines = ChangedLines::default().with_added("x.go", 1, 9);

        let graph = build_with(&old, &new, &lines, MAX_NODES, CALLEES);

        assert_eq!(ids(&graph), vec!["p.X"]);
        assert!(graph.edges.is_empty());
        assert!(node(&graph, "p.X").calls.is_empty());
    }

    /// Builds the graph of a real review from files in the folder named by
    /// `DIFFLENS_GRAPH_DIR`: `old.json` and `new.json` printed by the Go
    /// analyzer, and `diff.patch` from `git diff -U0` between the two
    /// commits. Prints the size of the graph for each option set, and
    /// writes the default graph as `graph.json` in the same folder.
    /// Analyses printed before calls had a `relation` are read as calls.
    #[test]
    #[ignore = "needs analyses of a real repository"]
    fn sizes_of_a_real_review() {
        let folder = std::path::PathBuf::from(
            std::env::var_os("DIFFLENS_GRAPH_DIR").expect("DIFFLENS_GRAPH_DIR"),
        );
        let read = |name: &str| std::fs::read(folder.join(name)).unwrap();
        let analysis_of = |name: &str| {
            let mut value: serde_json::Value = serde_json::from_slice(&read(name)).unwrap();
            for call in value["calls"].as_array_mut().unwrap() {
                let call = call.as_object_mut().unwrap();
                if !call.contains_key("relation") {
                    call.insert("relation".to_string(), serde_json::json!("call"));
                }
            }
            crate::analysis::parse(value.to_string().as_bytes(), &["go"], name).unwrap()
        };
        let old = analysis_of("old.json");
        let new = analysis_of("new.json");
        let lines = crate::hunks::parse_patch(&read("diff.patch")).unwrap();
        for (label, options) in [
            ("default", FlowOptions::default()),
            ("includeTests", TESTS),
            ("includeCallers", CALLERS),
            ("includeCallees", CALLEES),
        ] {
            let graph = build_with(&old, &new, &lines, MAX_NODES, options);
            let count =
                |state: FlowNodeState| graph.nodes.iter().filter(|n| n.state == state).count();
            let files: HashSet<&str> = graph
                .nodes
                .iter()
                .filter_map(|n| n.file.as_deref())
                .collect();
            let packages: HashSet<&str> =
                graph.nodes.iter().map(|n| n.container.as_str()).collect();
            let edges = |added: bool, removed: bool| {
                graph
                    .edges
                    .iter()
                    .filter(|e| e.added == added && e.removed == removed)
                    .count()
            };
            println!(
                "{label}: {} nodes ({} new, {} modified, {} deleted, {} untouched, {} external packages) \
                 in {} files and {} packages, {} edges ({} added, {} preserved, {} removed), \
                 {} entry points, truncated {}, tests shown {}, hidden {:?}",
                graph.nodes.len(),
                count(FlowNodeState::New),
                count(FlowNodeState::Modified),
                count(FlowNodeState::Deleted),
                count(FlowNodeState::Untouched),
                count(FlowNodeState::External),
                files.len(),
                packages.len(),
                graph.edges.len(),
                edges(true, false),
                edges(false, false),
                edges(false, true),
                graph.entry_points.len(),
                graph.truncated,
                graph.nodes.iter().filter(|n| n.test).count(),
                graph.hidden,
            );
            if label == "default" {
                std::fs::write(
                    folder.join("graph.json"),
                    serde_json::to_vec(&graph).unwrap(),
                )
                .unwrap();
            }
        }
    }
}
