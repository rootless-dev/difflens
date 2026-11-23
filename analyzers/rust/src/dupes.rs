//! The `dupes` command: the duplicated code a review introduces
//! (dupes.schema.json), found the way the schema's description says every
//! analyzer finds it, so the languages agree.
//!
//! Files are the `.rs` files of the tree outside build output, dependencies'
//! and hidden folders, test files included, without generated files (the
//! `@generated` header `analyze` reports), files over 1 MiB and the files
//! the `exclude` of `--changed` names (a folder or a name ending it lists,
//! ignoring ASCII case). proc-macro2 tokenizes them: it drops comments and
//! turns doc comments into `#[doc = "…"]` attributes, dropped here as the
//! comments they are. Identifiers become ID, string, character and byte
//! literals STR, numbers NUM; keywords (`self`, `Self`, `crate`, `super`,
//! `true` and `false` included), lifetimes (`'ID`, `'static`, `'_`),
//! operators (`::`, `->`, `+=`… joined when proc-macro2 marks them joint)
//! and delimiters stay as they are. `use`, `extern crate` and `mod name;`
//! items, with their attributes and visibility, are left out and split the
//! file.
//!
//! Scopes: every token of a file under a folder named `tests` is test code.
//! In the other files, syn parses the same token stream, and every item
//! (module-level, nested, or an associated item of an impl or trait) that
//! an outer `#[cfg(test)]` or `#[cfg(all(test, …))]` applies to
//! (`index::is_cfg_test`) is test code, from that attribute's `#` through
//! the end of the item; a token belongs to it when its start position
//! (line and column, proc-macro2's span locations) lies in that range. A
//! file syn cannot parse is searched as production code only, with a
//! diagnostic. Every change between production and a test item, or between
//! two test items, starts a new split, so the production stream of a file
//! has a split where an item was taken out and each test item is a split of
//! its own.
//!
//! The search runs once per scope, production first, on the tokens of that
//! scope only, in the schema's steps: every window of `min_tokens` tokens
//! that touches an added line is hashed (a rolling hash), then every window
//! of every file; windows sharing a wanted hash are paired, each pair grows
//! into the longest equal run of its diagonal (once per run: seeds are
//! visited in order and those inside a run already found are skipped, so a
//! long clone is not re-extended from each of its windows), and the runs
//! whose new sides overlap become blocks, grouped into `copies_existing` and
//! `repeated_in_change`. Each scope keeps at most MAX_GROUPS groups.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Instant;

use proc_macro2::{Delimiter, LineColumn, Spacing, Span, TokenStream, TokenTree};
use syn::spanned::Spanned;
use syn::visit::{self, Visit};
use syn::{AttrStyle, Attribute, ImplItem, Item, TraitItem};

use crate::index::is_cfg_test;
use crate::protocol::{Changed, DupesRun, Duplicate, Exclude, LineRange};
use crate::source;

/// Files larger than this are left out.
const MAX_FILE_SIZE: u64 = 1 << 20;
/// A hash shared by more windows than this is boilerplate.
const MAX_BUCKET: usize = 64;
/// Groups reported at most, per scope.
const MAX_GROUPS: usize = 500;
/// The scopes, in the order their groups are reported.
const PRODUCTION: &str = "production";
const TEST: &str = "test";
/// Folders never searched, besides hidden ones: build output and
/// dependencies.
const SKIPPED_DIRS: &[&str] = &["target", "vendor", "node_modules"];
/// How many seeds are extended between two looks at the clock.
const SEEDS_PER_CLOCK_CHECK: usize = 1024;

/// Strict and reserved keywords, sorted (binary search). `r#match` is an
/// identifier, whatever it spells, and never matches.
const KEYWORDS: &[&str] = &[
    "Self", "_", "abstract", "as", "async", "await", "become", "box", "break", "const", "continue",
    "crate", "do", "dyn", "else", "enum", "extern", "false", "final", "fn", "for", "if", "impl",
    "in", "let", "loop", "macro", "match", "mod", "move", "mut", "override", "priv", "pub", "ref",
    "return", "self", "static", "struct", "super", "trait", "true", "try", "type", "typeof",
    "unsafe", "unsized", "use", "virtual", "where", "while", "yield",
];

/// Operators of more than one character, longest first.
const OPERATORS: &[&str] = &[
    "<<=", ">>=", "...", "..=", "::", "->", "=>", "==", "!=", "<=", ">=", "&&", "||", "+=", "-=",
    "*=", "/=", "%=", "^=", "&=", "|=", "<<", ">>", "..",
];

/// Every punctuation character proc-macro2 produces.
const PUNCTUATION: &str = "~!@#$%^&*-=+|;:,<.>/?'";

/// One normalized token: its text (ID, STR, NUM, a keyword, a lifetime, an
/// operator or a delimiter), the lines it starts and ends on, its split (no
/// clone spans two splits) and whether it is test code (step 2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Token {
    pub text: &'static str,
    pub start: u32,
    pub end: u32,
    pub split: u32,
    pub test: bool,
}

/// The normalized tokens of a Rust file (step 3), marked test (step 2): all
/// of them when `all_test` (a file under `tests/`), otherwise those of its
/// `#[cfg(test)]` items. The error says syn could not parse the file to
/// find those items: its tokens are then all production code.
fn tokenize_file(
    text: &str,
    all_test: bool,
) -> Result<(Vec<Token>, Option<syn::Error>), proc_macro2::LexError> {
    let stream = TokenStream::from_str(text)?;
    let (regions, error) = if all_test {
        (Vec::new(), None)
    } else {
        match syn::parse2::<syn::File>(stream.clone()) {
            Ok(file) => (test_regions(&file), None),
            Err(error) => (Vec::new(), Some(error)),
        }
    };
    let mut lexer = Lexer {
        tokens: Vec::new(),
        split: 0,
        all_test,
        regions,
        region: None,
    };
    lexer.stream(stream);
    Ok((lexer.tokens, error))
}

/// A position in a file: line (from 1) and column (from 0).
type Position = (usize, usize);

fn position(at: LineColumn) -> Position {
    (at.line, at.column)
}

/// The source ranges of the items an outer `#[cfg(test)]` applies to, from
/// the attribute's `#` to the end of the item (exclusive), in source
/// order; none holds another (a test item is not searched further).
fn test_regions(file: &syn::File) -> Vec<(Position, Position)> {
    let mut items = TestItems {
        regions: Vec::new(),
    };
    items.visit_file(file);
    items.regions
}

struct TestItems {
    regions: Vec<(Position, Position)>,
}

impl TestItems {
    /// Records the item when an outer `#[cfg(test)]` among `attrs` applies
    /// to it; true when it did.
    fn record(&mut self, attrs: &[Attribute], item: &impl Spanned) -> bool {
        let found = attrs.iter().find(|attr| {
            matches!(attr.style, AttrStyle::Outer) && is_cfg_test(std::slice::from_ref(*attr))
        });
        let Some(attr) = found else {
            return false;
        };
        let start = position(attr.pound_token.spans[0].start());
        self.regions.push((start, position(item.span().end())));
        true
    }
}

impl<'ast> Visit<'ast> for TestItems {
    fn visit_item(&mut self, item: &'ast Item) {
        if !self.record(item_attrs(item), item) {
            visit::visit_item(self, item);
        }
    }

    fn visit_impl_item(&mut self, item: &'ast ImplItem) {
        let attrs: &[Attribute] = match item {
            ImplItem::Const(inner) => &inner.attrs,
            ImplItem::Fn(inner) => &inner.attrs,
            ImplItem::Type(inner) => &inner.attrs,
            ImplItem::Macro(inner) => &inner.attrs,
            _ => &[],
        };
        if !self.record(attrs, item) {
            visit::visit_impl_item(self, item);
        }
    }

    fn visit_trait_item(&mut self, item: &'ast TraitItem) {
        let attrs: &[Attribute] = match item {
            TraitItem::Const(inner) => &inner.attrs,
            TraitItem::Fn(inner) => &inner.attrs,
            TraitItem::Type(inner) => &inner.attrs,
            TraitItem::Macro(inner) => &inner.attrs,
            _ => &[],
        };
        if !self.record(attrs, item) {
            visit::visit_trait_item(self, item);
        }
    }
}

fn item_attrs(item: &Item) -> &[Attribute] {
    match item {
        Item::Const(inner) => &inner.attrs,
        Item::Enum(inner) => &inner.attrs,
        Item::ExternCrate(inner) => &inner.attrs,
        Item::Fn(inner) => &inner.attrs,
        Item::ForeignMod(inner) => &inner.attrs,
        Item::Impl(inner) => &inner.attrs,
        Item::Macro(inner) => &inner.attrs,
        Item::Mod(inner) => &inner.attrs,
        Item::Static(inner) => &inner.attrs,
        Item::Struct(inner) => &inner.attrs,
        Item::Trait(inner) => &inner.attrs,
        Item::TraitAlias(inner) => &inner.attrs,
        Item::Type(inner) => &inner.attrs,
        Item::Union(inner) => &inner.attrs,
        Item::Use(inner) => &inner.attrs,
        _ => &[],
    }
}

struct Lexer {
    tokens: Vec<Token>,
    split: u32,
    /// Every token is test code (a file under `tests/`).
    all_test: bool,
    /// The test items' ranges (`test_regions`).
    regions: Vec<(Position, Position)>,
    /// The test item of the last token pushed, if any.
    region: Option<usize>,
}

impl Lexer {
    /// The test item holding the token starting at `at`, if any.
    fn region_of(&self, at: Position) -> Option<usize> {
        let after = self.regions.partition_point(|(start, _)| *start <= at);
        let index = after.checked_sub(1)?;
        (at < self.regions[index].1).then_some(index)
    }

    fn push(&mut self, text: &'static str, first: Span, last: Span) {
        let region = self.region_of(position(first.start()));
        // Leaving or entering a test item splits the file.
        if region != self.region {
            self.split += 1;
            self.region = region;
        }
        let start = source::line(first);
        self.tokens.push(Token {
            text,
            start,
            end: source::end_line(last).max(start),
            split: self.split,
            test: self.all_test || region.is_some(),
        });
    }

    /// The trees of one level (a file, or the inside of a group).
    fn stream(&mut self, stream: TokenStream) {
        let trees: Vec<TokenTree> = stream.into_iter().collect();
        let mut at = 0;
        while at < trees.len() {
            if let Some(next) = doc_attribute(&trees, at) {
                at = next;
            } else if let Some(next) = import_item(&trees, at) {
                self.split += 1;
                at = next;
            } else {
                at = self.tree(&trees, at);
            }
        }
    }

    /// Pushes the token at `at` (a group with its contents, a lifetime, an
    /// operator of several characters) and returns the index after it.
    fn tree(&mut self, trees: &[TokenTree], at: usize) -> usize {
        match &trees[at] {
            TokenTree::Group(group) => {
                let delimiters = match group.delimiter() {
                    Delimiter::Parenthesis => Some(("(", ")")),
                    Delimiter::Brace => Some(("{", "}")),
                    Delimiter::Bracket => Some(("[", "]")),
                    Delimiter::None => None,
                };
                if let Some((open, _)) = delimiters {
                    self.push(open, group.span_open(), group.span_open());
                }
                self.stream(group.stream());
                if let Some((_, close)) = delimiters {
                    self.push(close, group.span_close(), group.span_close());
                }
                at + 1
            }
            TokenTree::Ident(ident) => {
                self.push(identifier(&ident.to_string()), ident.span(), ident.span());
                at + 1
            }
            TokenTree::Literal(literal) => {
                self.push(
                    literal_kind(&literal.to_string()),
                    literal.span(),
                    literal.span(),
                );
                at + 1
            }
            TokenTree::Punct(punct) => {
                // proc-macro2 gives a lifetime as a joint `'` and an ident.
                if punct.as_char() == '\'' {
                    if let Some(TokenTree::Ident(name)) = trees.get(at + 1) {
                        self.push(lifetime(&name.to_string()), punct.span(), name.span());
                        return at + 2;
                    }
                }
                let (text, count) = operator(trees, at);
                self.push(text, punct.span(), trees[at + count - 1].span());
                at + count
            }
        }
    }
}

/// ID, or the keyword itself.
fn identifier(name: &str) -> &'static str {
    match KEYWORDS.binary_search(&name) {
        Ok(index) => KEYWORDS[index],
        Err(_) => "ID",
    }
}

/// `'static` and `'_` as they are, any other lifetime or label as `'ID`.
fn lifetime(name: &str) -> &'static str {
    match name {
        "static" => "'static",
        "_" => "'_",
        _ => "'ID",
    }
}

/// NUM for integer and float literals (they start with a digit), STR for
/// the others: strings, characters, bytes, byte and C strings, raw or not.
fn literal_kind(text: &str) -> &'static str {
    if text.starts_with(|c: char| c.is_ascii_digit()) {
        "NUM"
    } else {
        "STR"
    }
}

/// The operator starting at the punctuation `at` and how many characters
/// it takes: the longest of OPERATORS its joint characters spell, or the
/// character alone.
fn operator(trees: &[TokenTree], at: usize) -> (&'static str, usize) {
    let mut spelled = String::new();
    for tree in trees[at..].iter().take(3) {
        let TokenTree::Punct(punct) = tree else {
            break;
        };
        spelled.push(punct.as_char());
        if punct.spacing() == Spacing::Alone {
            break;
        }
    }
    for length in (2..=spelled.len()).rev() {
        if let Some(found) = OPERATORS.iter().find(|op| **op == &spelled[..length]) {
            return (found, length);
        }
    }
    let first = spelled.chars().next().unwrap_or(' ');
    match PUNCTUATION.find(first) {
        Some(index) => (&PUNCTUATION[index..index + 1], 1),
        None => ("PUNCT", 1),
    }
}

fn is_punct(tree: Option<&TokenTree>, char: char) -> bool {
    matches!(tree, Some(TokenTree::Punct(punct)) if punct.as_char() == char)
}

fn is_ident(tree: Option<&TokenTree>, name: &str) -> bool {
    matches!(tree, Some(TokenTree::Ident(ident)) if ident == name)
}

fn is_group(tree: Option<&TokenTree>, delimiter: Delimiter) -> bool {
    matches!(tree, Some(TokenTree::Group(group)) if group.delimiter() == delimiter)
}

/// The index after the `#[doc = "…"]` or `#![doc = "…"]` attribute at
/// `at`, which is what proc-macro2 makes of a doc comment.
fn doc_attribute(trees: &[TokenTree], at: usize) -> Option<usize> {
    if !is_punct(trees.get(at), '#') {
        return None;
    }
    let mut next = at + 1;
    if is_punct(trees.get(next), '!') {
        next += 1;
    }
    let Some(TokenTree::Group(group)) = trees.get(next) else {
        return None;
    };
    if group.delimiter() != Delimiter::Bracket {
        return None;
    }
    let inside: Vec<TokenTree> = group.stream().into_iter().collect();
    match inside.as_slice() {
        [TokenTree::Ident(name), TokenTree::Punct(equals), TokenTree::Literal(_)]
            if name == "doc" && equals.as_char() == '=' =>
        {
            Some(next + 1)
        }
        _ => None,
    }
}

/// The index after the `use`, `extern crate` or `mod name;` item starting
/// at `at`, its outer attributes and visibility included; None when no such
/// item starts there.
fn import_item(trees: &[TokenTree], at: usize) -> Option<usize> {
    let mut next = at;
    while is_punct(trees.get(next), '#') && is_group(trees.get(next + 1), Delimiter::Bracket) {
        next += 2;
    }
    if is_ident(trees.get(next), "pub") {
        next += 1;
        if is_group(trees.get(next), Delimiter::Parenthesis) {
            next += 1;
        }
    }
    let keyword = trees.get(next);
    let import = if is_ident(keyword, "use") {
        // `impl Trait + use<'a>` captures lifetimes: no import.
        !is_punct(trees.get(next + 1), '<')
    } else if is_ident(keyword, "extern") {
        is_ident(trees.get(next + 1), "crate")
    } else if is_ident(keyword, "mod") {
        matches!(trees.get(next + 1), Some(TokenTree::Ident(_)))
            && is_punct(trees.get(next + 2), ';')
    } else {
        false
    };
    if !import {
        return None;
    }
    let end = trees[next..]
        .iter()
        .position(|tree| is_punct(Some(tree), ';'))
        .map_or(trees.len(), |offset| next + offset + 1);
    Some(end)
}

/// One searched file: its tokens as interned ids, their lines and splits,
/// and how many of its lines are added up to each line.
struct File {
    rel: String,
    ids: Vec<u32>,
    starts: Vec<u32>,
    ends: Vec<u32>,
    splits: Vec<u32>,
    /// `added_upto[line]`: added lines among 1..=line (index 0 is 0).
    added_upto: Vec<u32>,
    /// `touching_upto[i]`: tokens among the first i that start on an added
    /// line.
    touching_upto: Vec<u32>,
}

impl File {
    fn new(rel: String, tokens: &[Token], added: &[[u32; 2]], interner: &mut Interner) -> Self {
        let last_line = tokens.iter().map(|token| token.end).max().unwrap_or(0) as usize;
        let mut is_added = vec![false; last_line + 1];
        for &[first, last] in added {
            let end = (last as usize).min(last_line);
            for flag in is_added.iter_mut().take(end + 1).skip(first as usize) {
                *flag = true;
            }
        }
        let added_upto = prefix_counts(is_added.iter().skip(1).copied());
        let touching_upto = prefix_counts(
            tokens
                .iter()
                .map(|token| is_added.get(token.start as usize) == Some(&true)),
        );
        Self {
            rel,
            ids: tokens.iter().map(|token| interner.id(token.text)).collect(),
            starts: tokens.iter().map(|token| token.start).collect(),
            ends: tokens.iter().map(|token| token.end).collect(),
            splits: tokens.iter().map(|token| token.split).collect(),
            added_upto,
            touching_upto,
        }
    }

    fn len(&self) -> usize {
        self.ids.len()
    }

    fn has_added(&self) -> bool {
        self.added_upto.last().is_some_and(|count| *count > 0)
    }

    /// Whether the `k` tokens at `at` are one window: inside one split.
    fn is_window(&self, at: usize, k: usize) -> bool {
        self.splits[at] == self.splits[at + k - 1]
    }

    /// Whether one of the `k` tokens at `at` starts on an added line.
    fn touches(&self, at: usize, k: usize) -> bool {
        self.touching_upto[at + k] > self.touching_upto[at]
    }

    /// How many of the lines `first..=last` are added.
    fn added_lines(&self, first: u32, last: u32) -> u32 {
        let upto = |line: u32| {
            let line = (line as usize).min(self.added_upto.len() - 1);
            self.added_upto[line]
        };
        upto(last) - upto(first.saturating_sub(1))
    }
}

/// `counts[i]`: how many of the first i flags are set.
fn prefix_counts(flags: impl Iterator<Item = bool>) -> Vec<u32> {
    let mut counts = vec![0];
    let mut count = 0;
    for flag in flags {
        count += u32::from(flag);
        counts.push(count);
    }
    counts
}

/// Token texts as small numbers, the same for every file of a search.
#[derive(Default)]
struct Interner {
    ids: HashMap<&'static str, u32>,
}

impl Interner {
    fn id(&mut self, text: &'static str) -> u32 {
        let next = self.ids.len() as u32 + 1;
        *self.ids.entry(text).or_insert(next)
    }
}

/// The `.rs` files of the tree (step 1) that `exclude` does not leave out,
/// sorted by relative path; links are not followed. Folders that cannot be
/// read are diagnostics.
fn source_files(
    dir: &Path,
    exclude: &Exclude,
    diagnostics: &mut Vec<String>,
) -> std::io::Result<Vec<(String, PathBuf)>> {
    let mut found = Vec::new();
    collect_files(dir, "", &mut found, diagnostics, true)?;
    found.retain(|(rel, _)| !exclude.excludes(rel));
    found.sort();
    Ok(found)
}

fn collect_files(
    folder: &Path,
    rel: &str,
    found: &mut Vec<(String, PathBuf)>,
    diagnostics: &mut Vec<String>,
    root: bool,
) -> std::io::Result<()> {
    let entries = match std::fs::read_dir(folder) {
        Ok(entries) => entries,
        Err(error) if root => return Err(error),
        Err(error) => {
            diagnostics.push(format!(
                "{rel} could not be read, so it is left out: {error}"
            ));
            return Ok(());
        }
    };
    for entry in entries.filter_map(Result::ok) {
        let name = entry.file_name().to_string_lossy().into_owned();
        let path = entry.path();
        let child = if rel.is_empty() {
            name.clone()
        } else {
            format!("{rel}/{name}")
        };
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_dir() {
            if !name.starts_with('.') && !SKIPPED_DIRS.contains(&name.as_str()) {
                collect_files(&path, &child, found, diagnostics, false)?;
            }
        } else if kind.is_file() && name.ends_with(".rs") {
            found.push((child, path));
        }
    }
    Ok(())
}

/// Whether the file at `rel` is under a folder named `tests`: all test code.
fn is_test_file(rel: &str) -> bool {
    rel.split('/')
        .rev()
        .skip(1)
        .any(|segment| segment == "tests")
}

/// The file's tokens, or None when it is left out (too large, generated);
/// Err is a diagnostic. A file syn cannot parse adds a diagnostic of its
/// own and gives its tokens, all production code.
fn load(
    rel: &str,
    path: &Path,
    diagnostics: &mut Vec<String>,
) -> Result<Option<Vec<Token>>, String> {
    let size = std::fs::metadata(path)
        .map_err(|error| format!("{rel} could not be read, so it is left out: {error}"))?
        .len();
    if size > MAX_FILE_SIZE {
        return Ok(None);
    }
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("{rel} could not be read, so it is left out: {error}"))?;
    if source::is_generated(&text) {
        return Ok(None);
    }
    let (tokens, unparsed) = tokenize_file(&text, is_test_file(rel)).map_err(|error| {
        format!(
            "{rel}:{} could not be tokenized, so it is left out: {error}",
            source::line(error.span())
        )
    })?;
    if let Some(error) = unparsed {
        diagnostics.push(format!(
            "{rel}:{} could not be parsed, so its #[cfg(test)] items are searched as production code: {error}",
            source::line(error.span())
        ));
    }
    Ok(Some(tokens))
}

/// The polynomial hash of every run of `k` ids (step 4), rolled: entry i
/// hashes `ids[i..i + k]`.
fn window_hashes(ids: &[u32], k: usize) -> Vec<u64> {
    const BASE: u64 = 0x0000_0100_0000_01b3;
    if k == 0 || ids.len() < k {
        return Vec::new();
    }
    let top = BASE.wrapping_pow(u32::try_from(k - 1).unwrap_or(u32::MAX));
    let mut hash = ids[..k].iter().fold(0u64, |hash, &id| {
        hash.wrapping_mul(BASE).wrapping_add(u64::from(id))
    });
    let mut hashes = Vec::with_capacity(ids.len() - k + 1);
    hashes.push(hash);
    for at in k..ids.len() {
        hash = hash
            .wrapping_sub(u64::from(ids[at - k]).wrapping_mul(top))
            .wrapping_mul(BASE)
            .wrapping_add(u64::from(ids[at]));
        hashes.push(hash);
    }
    hashes
}

/// Two windows of `k` tokens with the same hash, `a` before `b` (by file,
/// then position), at least one touching the change, not overlapping.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Seed {
    file_a: usize,
    at_a: usize,
    file_b: usize,
    at_b: usize,
}

/// Steps 5 and 6's candidates: the wanted hashes are those of the windows
/// touching the change; every window of every file with one of them joins
/// that hash's bucket; buckets of one window or more than MAX_BUCKET are
/// dropped; the others give their pairs, sorted.
fn seeds(files: &[File], k: usize) -> Vec<Seed> {
    let hashes: Vec<Vec<u64>> = files
        .iter()
        .map(|file| window_hashes(&file.ids, k))
        .collect();
    // The windows of a file, as (position, hash).
    let windows = |index: usize| {
        let file = &files[index];
        hashes[index]
            .iter()
            .enumerate()
            .filter(move |(at, _)| file.is_window(*at, k))
            .map(|(at, hash)| (at, *hash))
    };
    let mut wanted = HashSet::new();
    for (index, file) in files.iter().enumerate() {
        if file.has_added() {
            wanted.extend(
                windows(index)
                    .filter(|(at, _)| file.touches(*at, k))
                    .map(|(_, hash)| hash),
            );
        }
    }
    let mut entries: Vec<(u64, usize, usize)> = Vec::new();
    for index in 0..files.len() {
        entries.extend(
            windows(index)
                .filter(|(_, hash)| wanted.contains(hash))
                .map(|(at, hash)| (hash, index, at)),
        );
    }
    entries.sort_unstable();
    let mut seeds = Vec::new();
    for bucket in entries.chunk_by(|x, y| x.0 == y.0) {
        if bucket.len() < 2 || bucket.len() > MAX_BUCKET {
            continue;
        }
        for (position, &(_, file_a, at_a)) in bucket.iter().enumerate() {
            for &(_, file_b, at_b) in &bucket[position + 1..] {
                let overlapping = file_a == file_b && at_b < at_a + k;
                let touching = files[file_a].touches(at_a, k) || files[file_b].touches(at_b, k);
                if touching && !overlapping {
                    seeds.push(Seed {
                        file_a,
                        at_a,
                        file_b,
                        at_b,
                    });
                }
            }
        }
    }
    seeds.sort_unstable();
    seeds
}

/// Equal token runs: `length` tokens at `start_a` in `file_a` and at
/// `start_b` in `file_b`, `a` before `b`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct Pair {
    file_a: usize,
    start_a: usize,
    file_b: usize,
    start_b: usize,
    length: usize,
}

/// The longest run of equal tokens through the seed's windows, on their
/// diagonal and within the splits of both (overlap is not checked here).
fn equal_run(files: &[File], seed: Seed, k: usize) -> Pair {
    let (a, b) = (&files[seed.file_a], &files[seed.file_b]);
    let (split_a, split_b) = (a.splits[seed.at_a], b.splits[seed.at_b]);
    let same = |x: usize, y: usize| {
        a.ids[x] == b.ids[y] && a.splits[x] == split_a && b.splits[y] == split_b
    };
    let mut back = 0;
    while back < seed.at_a && back < seed.at_b && same(seed.at_a - back - 1, seed.at_b - back - 1) {
        back += 1;
    }
    let (start_a, start_b) = (seed.at_a - back, seed.at_b - back);
    let mut length = back + k;
    while start_a + length < a.len()
        && start_b + length < b.len()
        && same(start_a + length, start_b + length)
    {
        length += 1;
    }
    Pair {
        file_a: seed.file_a,
        start_a,
        file_b: seed.file_b,
        start_b,
        length,
    }
}

/// The maximal pairs of a run (step 6): the run itself when its sides do
/// not overlap. In one file, a run longer than the distance between its
/// sides is a periodic stretch (code repeated back to back): its maximal
/// pairs are the consecutive pieces as long as that distance, from its
/// start, the last one kept while it still has `k` tokens.
fn run_pairs(run: Pair, k: usize) -> Vec<Pair> {
    if run.file_a != run.file_b || run.start_b - run.start_a >= run.length {
        return vec![run];
    }
    let distance = run.start_b - run.start_a;
    let mut pairs = Vec::new();
    let mut offset = 0;
    while offset < run.length {
        let length = distance.min(run.length - offset);
        if length >= k {
            pairs.push(Pair {
                start_a: run.start_a + offset,
                start_b: run.start_b + offset,
                length,
                ..run
            });
        }
        offset += length;
    }
    pairs
}

/// Step 6: the maximal pairs of the seeds, once each. Seeds come sorted, so
/// those of one diagonal come in order, and a seed inside the last run of
/// its diagonal is skipped. The bool says the time ran out.
fn maximal_pairs(files: &[File], seeds: &[Seed], k: usize, deadline: Instant) -> (Vec<Pair>, bool) {
    let mut covered: HashMap<(usize, usize, usize), usize> = HashMap::new();
    let mut pairs = BTreeSet::new();
    for (count, &seed) in seeds.iter().enumerate() {
        if count % SEEDS_PER_CLOCK_CHECK == 0 && Instant::now() >= deadline {
            return (pairs.into_iter().collect(), true);
        }
        // The difference of the starts names the diagonal (it wraps when
        // `b` is earlier in a later file).
        let diagonal = (seed.file_a, seed.file_b, seed.at_b.wrapping_sub(seed.at_a));
        if covered.get(&diagonal).is_some_and(|&end| seed.at_a < end) {
            continue;
        }
        let (a, b) = (&files[seed.file_a], &files[seed.file_b]);
        if a.ids[seed.at_a..seed.at_a + k] != b.ids[seed.at_b..seed.at_b + k] {
            continue;
        }
        let run = equal_run(files, seed, k);
        covered.insert(diagonal, run.start_a + run.length);
        pairs.extend(run_pairs(run, k));
    }
    (pairs.into_iter().collect(), false)
}

/// One side of a maximal pair, as lines.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Side {
    file: usize,
    first: u32,
    last: u32,
    new: bool,
}

impl Side {
    fn of(files: &[File], file: usize, start: usize, length: usize) -> Self {
        let source = &files[file];
        let first = source.starts[start];
        let last = source.ends[start + length - 1].max(first);
        Self {
            file,
            first,
            last,
            new: is_new(source.added_lines(first, last), first, last),
        }
    }

    fn lines(self) -> u32 {
        self.last - self.first + 1
    }
}

/// Step 7: a side is new when at least half of the lines it spans are
/// added.
fn is_new(added: u32, first: u32, last: u32) -> bool {
    2 * u64::from(added) >= u64::from(last - first + 1)
}

/// Merges overlapping line ranges (inclusive) until none overlap; sorted.
fn merge_overlapping(mut ranges: Vec<(u32, u32)>) -> Vec<(u32, u32)> {
    ranges.sort_unstable();
    let mut merged: Vec<(u32, u32)> = Vec::new();
    for (first, last) in ranges {
        match merged.last_mut() {
            Some(previous) if first <= previous.1 => previous.1 = previous.1.max(last),
            _ => merged.push((first, last)),
        }
    }
    merged
}

/// A block (step 8): merged new sides of one file, with what it occurs as
/// elsewhere.
struct Block {
    file: usize,
    first: u32,
    last: u32,
    /// Existing occurrences: file and lines.
    existing: Vec<(usize, u32, u32)>,
    /// Most tokens of a pair with an existing occurrence.
    existing_tokens: usize,
}

/// Steps 7 to 10: the groups of the maximal pairs whose sides span at least
/// `min_lines` lines each, sorted by block, found in `scope`.
fn groups(files: &[File], pairs: &[Pair], min_lines: u32, scope: &str) -> Vec<Duplicate> {
    let sides: Vec<(Side, Side, usize)> = pairs
        .iter()
        .map(|pair| {
            (
                Side::of(files, pair.file_a, pair.start_a, pair.length),
                Side::of(files, pair.file_b, pair.start_b, pair.length),
                pair.length,
            )
        })
        .filter(|(a, b, _)| a.lines() >= min_lines && b.lines() >= min_lines && (a.new || b.new))
        .collect();

    let mut new_sides: BTreeMap<usize, Vec<(u32, u32)>> = BTreeMap::new();
    for (a, b, _) in &sides {
        for side in [a, b].into_iter().filter(|side| side.new) {
            new_sides
                .entry(side.file)
                .or_default()
                .push((side.first, side.last));
        }
    }
    let mut blocks = Vec::new();
    for (file, ranges) in new_sides {
        for (first, last) in merge_overlapping(ranges) {
            blocks.push(Block {
                file,
                first,
                last,
                existing: Vec::new(),
                existing_tokens: 0,
            });
        }
    }
    // Blocks are sorted by file, then line: the one holding a new side is
    // the last starting at or before it.
    let starts: Vec<(usize, u32)> = blocks
        .iter()
        .map(|block| (block.file, block.first))
        .collect();
    let block_of =
        |side: &Side| starts.partition_point(|start| *start <= (side.file, side.first)) - 1;

    let mut links: Vec<(usize, usize, usize)> = Vec::new();
    for (a, b, tokens) in &sides {
        match (a.new, b.new) {
            (true, true) => {
                let (x, y) = (block_of(a), block_of(b));
                if x != y {
                    links.push((x, y, *tokens));
                }
            }
            (true, false) | (false, true) => {
                let (new, existing) = if a.new { (a, b) } else { (b, a) };
                let block = block_of(new);
                blocks[block]
                    .existing
                    .push((existing.file, existing.first, existing.last));
                blocks[block].existing_tokens = blocks[block].existing_tokens.max(*tokens);
            }
            (false, false) => {}
        }
    }

    let range = |file: usize, first: u32, last: u32| LineRange {
        file: files[file].rel.clone(),
        start_line: first,
        end_line: last,
    };
    let mut duplicates = Vec::new();
    for block in blocks.iter().filter(|block| !block.existing.is_empty()) {
        let mut by_file: BTreeMap<usize, Vec<(u32, u32)>> = BTreeMap::new();
        for &(file, first, last) in &block.existing {
            by_file.entry(file).or_default().push((first, last));
        }
        let mut others: Vec<LineRange> = by_file
            .into_iter()
            .flat_map(|(file, ranges)| {
                merge_overlapping(ranges)
                    .into_iter()
                    .map(move |(first, last)| (file, first, last))
            })
            .map(|(file, first, last)| range(file, first, last))
            .collect();
        others.sort();
        duplicates.push(Duplicate {
            kind: "copies_existing".to_string(),
            scope: scope.to_string(),
            block: range(block.file, block.first, block.last),
            others,
            tokens: block.existing_tokens,
            lines: block.last - block.first + 1,
        });
    }
    for component in repeated_components(&blocks, &links) {
        let tokens = links
            .iter()
            .filter(|(x, y, _)| component.contains(x) && component.contains(y))
            .map(|(_, _, tokens)| *tokens)
            .max()
            .unwrap_or(0);
        let mut ranges: Vec<LineRange> = component
            .iter()
            .map(|&index| range(blocks[index].file, blocks[index].first, blocks[index].last))
            .collect();
        ranges.sort();
        let block = ranges.remove(0);
        duplicates.push(Duplicate {
            kind: "repeated_in_change".to_string(),
            scope: scope.to_string(),
            lines: block.end_line - block.start_line + 1,
            block,
            others: ranges,
            tokens,
        });
    }
    duplicates.sort_by(|x, y| x.block.cmp(&y.block).then_with(|| x.kind.cmp(&y.kind)));
    duplicates
}

/// The blocks without an existing occurrence that are occurrences of one
/// another, transitively (step 9), each set of two or more.
fn repeated_components(blocks: &[Block], links: &[(usize, usize, usize)]) -> Vec<BTreeSet<usize>> {
    let alone = |index: usize| blocks[index].existing.is_empty();
    let mut neighbours: HashMap<usize, Vec<usize>> = HashMap::new();
    for &(x, y, _) in links.iter().filter(|(x, y, _)| alone(*x) && alone(*y)) {
        neighbours.entry(x).or_default().push(y);
        neighbours.entry(y).or_default().push(x);
    }
    let mut seen = HashSet::new();
    let mut components = Vec::new();
    for start in 0..blocks.len() {
        if !neighbours.contains_key(&start) || !seen.insert(start) {
            continue;
        }
        let mut component = BTreeSet::from([start]);
        let mut pending = vec![start];
        while let Some(index) = pending.pop() {
            for &next in &neighbours[&index] {
                if seen.insert(next) {
                    component.insert(next);
                    pending.push(next);
                }
            }
        }
        components.push(component);
    }
    components
}

/// The duplicated code the review adds to the tree at `dir`
/// (dupes.schema.json), searched until `deadline`.
pub fn dupes(
    dir: &Path,
    changed: &Changed,
    min_tokens: usize,
    min_lines: u32,
    deadline: Instant,
) -> std::io::Result<DupesRun> {
    let mut run = DupesRun::new(min_tokens, min_lines);
    let mut added: HashMap<&str, Vec<[u32; 2]>> = HashMap::new();
    for file in &changed.files {
        let path = file.path.strip_prefix("./").unwrap_or(&file.path);
        added.entry(path).or_default().extend(&file.added);
    }
    let exclude = changed.exclude.clone().unwrap_or_default();
    let mut timed_out = false;
    // The files of each scope: the production and the test tokens of every
    // file, apart.
    let mut production = Vec::new();
    let mut test = Vec::new();
    let mut interner = Interner::default();
    for (rel, path) in source_files(dir, &exclude, &mut run.diagnostics)? {
        if Instant::now() >= deadline {
            timed_out = true;
            break;
        }
        match load(&rel, &path, &mut run.diagnostics) {
            Ok(Some(tokens)) => {
                let lines = added.get(rel.as_str()).map_or(&[][..], Vec::as_slice);
                let (test_tokens, production_tokens): (Vec<Token>, Vec<Token>) =
                    tokens.into_iter().partition(|token| token.test);
                if !production_tokens.is_empty() {
                    let file = File::new(rel.clone(), &production_tokens, lines, &mut interner);
                    production.push(file);
                }
                if !test_tokens.is_empty() {
                    test.push(File::new(rel, &test_tokens, lines, &mut interner));
                }
            }
            Ok(None) => {}
            Err(diagnostic) => run.diagnostics.push(diagnostic),
        }
    }
    for (scope, files) in [(PRODUCTION, production), (TEST, test)] {
        if timed_out {
            break;
        }
        if !files.iter().any(File::has_added) {
            continue;
        }
        let seeds = seeds(&files, min_tokens);
        let (pairs, out_of_time) = maximal_pairs(&files, &seeds, min_tokens, deadline);
        timed_out = out_of_time;
        let mut found = groups(&files, &pairs, min_lines, scope);
        if found.len() > MAX_GROUPS {
            run.diagnostics.push(format!(
                "{} groups of duplicated {scope} code were found; only the first {MAX_GROUPS} are reported",
                found.len()
            ));
            found.truncate(MAX_GROUPS);
        }
        run.duplicates.extend(found);
    }
    if timed_out {
        run.diagnostics.push(
            "the search for duplicated code ran out of time; only the groups found so far are reported"
                .to_string(),
        );
    }
    Ok(run)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use serde_json::{json, Value};

    use super::*;
    use crate::protocol::ChangedFile;
    use crate::testing::{schema, testdata, validate};

    /// The tokens of a file outside `tests/`.
    fn tokenize(text: &str) -> Result<Vec<Token>, proc_macro2::LexError> {
        tokenize_file(text, false).map(|(tokens, _)| tokens)
    }

    fn texts(code: &str) -> Vec<&'static str> {
        tokenize(code)
            .unwrap()
            .iter()
            .map(|token| token.text)
            .collect()
    }

    fn fixture_changed() -> Changed {
        let text = std::fs::read_to_string(testdata("dupes").join("changed.json")).unwrap();
        serde_json::from_str(&text).unwrap()
    }

    fn search(min_tokens: usize, min_lines: u32) -> Value {
        let deadline = Instant::now() + Duration::from_secs(60);
        let run = dupes(
            &testdata("dupes"),
            &fixture_changed(),
            min_tokens,
            min_lines,
            deadline,
        )
        .unwrap();
        serde_json::to_value(run).unwrap()
    }

    /// A file of `text` whose lines `added` are added, with its own
    /// interner (enough for one test).
    fn file_of(rel: &str, text: &str, added: &[[u32; 2]], interner: &mut Interner) -> File {
        File::new(rel.to_string(), &tokenize(text).unwrap(), added, interner)
    }

    #[test]
    fn identifiers_and_literals_are_normalized_and_keywords_kept() {
        assert_eq!(
            texts("fn add(self, n: u64) -> bool { r#match == 1_000 && 2.5e3 > 0x1f || true }"),
            [
                "fn", "ID", "(", "self", ",", "ID", ":", "ID", ")", "->", "ID", "{", "ID", "==",
                "NUM", "&&", "NUM", ">", "NUM", "||", "true", "}"
            ]
        );
        assert_eq!(
            texts(r##"("s", 'c', b'b', b"bytes", r#"raw"#, br"raw", c"c", -1)"##),
            [
                "(", "STR", ",", "STR", ",", "STR", ",", "STR", ",", "STR", ",", "STR", ",", "STR",
                ",", "-", "NUM", ")"
            ]
        );
        assert_eq!(
            texts("Self::new(crate::x, super::y)"),
            ["Self", "::", "ID", "(", "crate", "::", "ID", ",", "super", "::", "ID", ")"]
        );
    }

    #[test]
    fn lifetimes_are_one_token() {
        assert_eq!(
            texts("fn f<'a>(x: &'a str, y: &'static str, z: &'_ u8) { 'outer: loop {} }"),
            [
                "fn", "ID", "<", "'ID", ">", "(", "ID", ":", "&", "'ID", "ID", ",", "ID", ":", "&",
                "'static", "ID", ",", "ID", ":", "&", "'_", "ID", ")", "{", "'ID", ":", "loop",
                "{", "}", "}"
            ]
        );
    }

    #[test]
    fn joint_punctuation_makes_operators() {
        assert_eq!(
            texts("a += b; c..=d; e >>= 2; f != g; h = !i; j => k; Vec<Vec<u8>>; l.. ;"),
            [
                "ID", "+=", "ID", ";", "ID", "..=", "ID", ";", "ID", ">>=", "NUM", ";", "ID", "!=",
                "ID", ";", "ID", "=", "!", "ID", ";", "ID", "=>", "ID", ";", "ID", "<", "ID", "<",
                "ID", ">>", ";", "ID", "..", ";"
            ]
        );
    }

    #[test]
    fn doc_comments_are_dropped_like_other_comments() {
        let code = "//! Module docs.\n/// Item docs.\n/** Block docs. */\n// Plain.\nfn f() {\n    /* inside */ g();\n}\n";
        let tokens = tokenize(code).unwrap();
        assert_eq!(
            tokens.iter().map(|token| token.text).collect::<Vec<_>>(),
            ["fn", "ID", "(", ")", "{", "ID", "(", ")", ";", "}"]
        );
        assert_eq!((tokens[0].start, tokens[5].start, tokens[9].end), (5, 6, 7));
        // A doc attribute written by hand is not a comment.
        assert_eq!(texts("#[derive(Debug)] struct S;")[0], "#");
    }

    #[test]
    fn imports_are_left_out_and_split_the_file() {
        let code = "use a::b;\nfn f() {}\n#[cfg(test)]\npub(crate) use c::{d, e};\nextern crate g as h;\nmod i;\nmod j { fn k() {} }\nfn l() -> impl Sized + use<'a> {}\n";
        let tokens = tokenize(code).unwrap();
        assert_eq!(
            tokens.iter().map(|token| token.text).collect::<Vec<_>>(),
            [
                "fn", "ID", "(", ")", "{", "}", // fn f() {}
                "mod", "ID", "{", "fn", "ID", "(", ")", "{", "}", "}", // mod j { fn k() {} }
                "fn", "ID", "(", ")", "->", "impl", "ID", "+", "use", "<", "'ID", ">", "{", "}",
            ]
        );
        let splits: Vec<u32> = tokens.iter().map(|token| token.split).collect();
        assert_eq!(splits, [[1; 6].as_slice(), &[4; 24]].concat());
    }

    #[test]
    fn keywords_are_sorted_for_the_binary_search() {
        assert!(KEYWORDS.windows(2).all(|pair| pair[0] < pair[1]));
        assert_eq!(identifier("Self"), "Self");
        assert_eq!(identifier("yield"), "yield");
        assert_eq!(identifier("value"), "ID");
    }

    #[test]
    fn rolling_hashes_are_the_hashes_of_each_window() {
        let ids: Vec<u32> = (0..40).map(|n| n * 7 % 11 + 1).collect();
        let rolled = window_hashes(&ids, 5);
        assert_eq!(rolled.len(), 36);
        for (at, hash) in rolled.iter().enumerate() {
            assert_eq!(*hash, window_hashes(&ids[at..at + 5], 5)[0], "window {at}");
        }
        assert!(window_hashes(&ids[..4], 5).is_empty());
    }

    #[test]
    fn a_side_is_new_when_at_least_half_its_lines_are_added() {
        assert!(is_new(3, 1, 6));
        assert!(!is_new(2, 1, 6));
        assert!(is_new(1, 4, 4));
        assert!(!is_new(0, 4, 4));
        let mut interner = Interner::default();
        let added = [[2, 3], [9, 12]];
        let file = file_of("a.rs", "fn a() {\n\n\n\n}\n", &added, &mut interner);
        assert_eq!(file.added_lines(1, 5), 2);
        assert_eq!(file.added_lines(3, 5), 1);
        assert!(!file.touches(0, 4));
    }

    #[test]
    fn overlapping_blocks_merge_and_adjacent_ones_do_not() {
        assert_eq!(
            merge_overlapping(vec![(5, 9), (1, 3), (3, 4), (12, 14), (8, 10), (11, 11)]),
            [(1, 4), (5, 10), (11, 11), (12, 14)]
        );
    }

    #[test]
    fn a_run_repeated_back_to_back_splits_into_pieces() {
        let run = Pair {
            file_a: 0,
            start_a: 10,
            file_b: 0,
            start_b: 30,
            length: 50,
        };
        let pieces: Vec<(usize, usize, usize)> = run_pairs(run, 5)
            .iter()
            .map(|pair| (pair.start_a, pair.start_b, pair.length))
            .collect();
        assert_eq!(pieces, [(10, 30, 20), (30, 50, 20), (50, 70, 10)]);
        let across = Pair {
            file_b: 1,
            start_b: 2,
            ..run
        };
        assert_eq!(run_pairs(across, 5), [across]);
    }

    #[test]
    fn a_long_clone_is_one_maximal_pair() {
        // Lines of one shape with operators of a pseudo-random sequence, so
        // that no stretch of a file repeats inside it.
        let operators = ["+", "-", "*", "/", "%", "&", "|", "^"];
        let mut state: u32 = 7;
        let mut operator = || {
            state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            operators[(state >> 16) as usize % operators.len()]
        };
        let body: String = (0..40)
            .map(|_| {
                let (x, y, z) = (operator(), operator(), operator());
                format!("    let v = a {x} b {y} c {z} d;\n")
            })
            .collect();
        let existing = format!("fn old() {{\n{body}}}\n");
        let added = format!("struct S;\nfn new() {{\n{body}}}\n");
        let mut interner = Interner::default();
        let files = [
            file_of("a.rs", &added, &[[1, 43]], &mut interner),
            file_of("b.rs", &existing, &[], &mut interner),
        ];
        let seeds = seeds(&files, 20);
        assert!(seeds.len() > 400, "{}", seeds.len());
        let deadline = Instant::now() + Duration::from_secs(60);
        let (pairs, timed_out) = maximal_pairs(&files, &seeds, 20, deadline);
        assert!(!timed_out);
        assert_eq!(
            pairs,
            [Pair {
                file_a: 0,
                start_a: 3,
                file_b: 1,
                start_b: 0,
                length: 5 + 40 * 11 + 1,
            }]
        );
        let groups = groups(&files, &pairs, 6, PRODUCTION);
        assert_eq!(groups.len(), 1);
        let block = &groups[0].block;
        assert_eq!(
            (block.start_line, block.end_line, groups[0].lines),
            (2, 43, 42)
        );
    }

    #[test]
    fn the_fixture_reports_a_copy_and_a_repetition_and_nothing_else() {
        let document = search(50, 6);
        assert_eq!(
            document,
            json!({
                "protocol": 1,
                "language": "rust",
                "runner": { "name": "difflens-analyzer-rust", "version": "0.0.0" },
                "minTokens": 50,
                "minLines": 6,
                "duplicates": [
                    {
                        "kind": "copies_existing",
                        "scope": "production",
                        "block": { "file": "src/added.rs", "startLine": 13, "endLine": 25 },
                        "others": [{ "file": "src/helper.rs", "startLine": 4, "endLine": 15 }],
                        "tokens": 79,
                        "lines": 13
                    },
                    {
                        "kind": "repeated_in_change",
                        "scope": "production",
                        "block": { "file": "src/added.rs", "startLine": 42, "endLine": 49 },
                        "others": [
                            { "file": "src/added.rs", "startLine": 55, "endLine": 62 },
                            { "file": "src/added.rs", "startLine": 70, "endLine": 77 }
                        ],
                        "tokens": 60,
                        "lines": 8
                    }
                ],
                "diagnostics": []
            })
        );
        let dupes_schema = schema("dupes.schema.json");
        assert_eq!(
            validate(&document, &dupes_schema, &dupes_schema),
            Vec::<String>::new()
        );
        let changed: Value = serde_json::from_str(
            &std::fs::read_to_string(testdata("dupes").join("changed.json")).unwrap(),
        )
        .unwrap();
        assert_eq!(
            validate(&changed, &dupes_schema, &dupes_schema["$defs"]["changed"]),
            Vec::<String>::new()
        );
    }

    #[test]
    fn lower_minimums_report_the_near_miss() {
        let document = search(20, 3);
        let near_miss = json!({
            "kind": "copies_existing",
            "scope": "production",
            "block": { "file": "src/added.rs", "startLine": 34, "endLine": 39 },
            "others": [{ "file": "src/helper.rs", "startLine": 18, "endLine": 23 }],
            "tokens": 35,
            "lines": 6
        });
        let groups = document["duplicates"].as_array().unwrap();
        assert!(groups.contains(&near_miss), "{document}");
        assert_eq!(search(50, 14)["duplicates"].as_array().unwrap().len(), 0);
    }

    #[test]
    fn a_search_out_of_time_says_so() {
        let changed = fixture_changed();
        let run = dupes(&testdata("dupes"), &changed, 50, 6, Instant::now()).unwrap();
        assert!(run.duplicates.is_empty());
        let diagnostics = run.diagnostics;
        assert_eq!(diagnostics.len(), 1);
        assert!(
            diagnostics[0].contains("ran out of time"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn a_file_that_does_not_lex_is_a_diagnostic() {
        let folder = crate::testing::TempDir::new();
        let broken = "fn f() { \"unterminated }\n";
        std::fs::write(folder.0.join("bad.rs"), broken).unwrap();
        std::fs::create_dir_all(folder.0.join("target")).unwrap();
        std::fs::write(folder.0.join("target/out.rs"), broken).unwrap();
        let changed = Changed {
            protocol: 1,
            exclude: None,
            files: Vec::new(),
        };
        let deadline = Instant::now() + Duration::from_secs(60);
        let run = dupes(&folder.0, &changed, 50, 6, deadline).unwrap();
        let diagnostics = run.diagnostics;
        assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
        let expected = "bad.rs:1 could not be tokenized";
        assert!(diagnostics[0].starts_with(expected), "{diagnostics:?}");
    }

    /// A function of 12 lines and 78 tokens named `name`.
    fn shipping(name: &str) -> String {
        format!("{}{}", shipping_head(name), SHIPPING_TAIL)
    }

    /// The first 3 lines of `shipping` (42 tokens).
    fn shipping_head(name: &str) -> String {
        format!(
            "fn {name}(weight: u32, zone: u32) -> u32 {{\n    let base = weight * 3 + zone;\n    let surcharge = if zone > 4 {{ base / 2 }} else {{ base / 5 }};\n"
        )
    }

    /// The last 9 lines of `shipping` (36 tokens).
    const SHIPPING_TAIL: &str = "    let mut total = base + surcharge;\n    for step in 0..zone {\n        total += step * weight % 7;\n    }\n    if total > 1000 {\n        total -= 100;\n    }\n    total\n}\n";

    /// The search of a tree of `files` (path, text, whether every line is
    /// added), with `exclude`.
    fn search_tree(files: &[(&str, &str, bool)], exclude: Option<Exclude>) -> DupesRun {
        let folder = crate::testing::TempDir::new();
        let mut changed = Changed {
            protocol: 1,
            exclude,
            files: Vec::new(),
        };
        for (rel, text, added) in files {
            let path = folder.0.join(rel);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(&path, text).unwrap();
            if *added {
                changed.files.push(ChangedFile {
                    path: rel.to_string(),
                    added: vec![[1, 1000]],
                });
            }
        }
        let deadline = Instant::now() + Duration::from_secs(60);
        let run = dupes(&folder.0, &changed, 50, 6, deadline).unwrap();
        let document = serde_json::to_value(&run).unwrap();
        let dupes_schema = schema("dupes.schema.json");
        assert_eq!(
            validate(&document, &dupes_schema, &dupes_schema),
            Vec::<String>::new()
        );
        run
    }

    /// Each group as "kind scope block <- others", ranges as file:start-end.
    fn summary(run: &DupesRun) -> Vec<String> {
        let range =
            |range: &LineRange| format!("{}:{}-{}", range.file, range.start_line, range.end_line);
        run.duplicates
            .iter()
            .map(|group| {
                let others: Vec<String> = group.others.iter().map(range).collect();
                format!(
                    "{} {} {} <- {}",
                    group.kind,
                    group.scope,
                    range(&group.block),
                    others.join(" ")
                )
            })
            .collect()
    }

    #[test]
    fn cfg_test_items_are_test_code_and_split_the_file() {
        let code = "fn a() {}\n#[cfg(test)]\nmod t { fn b() {} }\nfn c() {}\n#[cfg(all(test, unix))] fn d() {}\n#[cfg(test)] fn e() {}\nimpl S {\n    #[allow(dead_code)]\n    #[cfg(test)]\n    fn f() {}\n    fn g() {}\n}\n#[cfg(not(test))] fn h() {}\nmod i {\n    #![cfg(test)]\n}\n";
        let tokens = tokenize(code).unwrap();
        let marks: Vec<(&str, bool, u32)> = tokens
            .iter()
            .map(|token| (token.text, token.test, token.split))
            .collect();
        let item = |test: bool, split: u32| {
            ["fn", "ID", "(", ")", "{", "}"].map(|text| (text, test, split))
        };
        let attribute = |test: bool, split: u32| {
            ["#", "[", "ID", "(", "ID", ")", "]"].map(|text| (text, test, split))
        };
        let mut expected = Vec::new();
        expected.extend(item(false, 0));
        expected.extend(attribute(true, 1));
        expected.extend(
            ["mod", "ID", "{"]
                .map(|text| (text, true, 1))
                .into_iter()
                .chain(item(true, 1))
                .chain([("}", true, 1)]),
        );
        expected.extend(item(false, 2));
        // `#[cfg(all(test, unix))] fn d() {}`, then `#[cfg(test)] fn e() {}`:
        // two test items, two splits.
        let all = [
            "#", "[", "ID", "(", "ID", "(", "ID", ",", "ID", ")", ")", "]",
        ];
        expected.extend(all.map(|text| (text, true, 3)));
        expected.extend(item(true, 3));
        expected.extend(attribute(true, 4));
        expected.extend(item(true, 4));
        // The impl: its `#[cfg(test)]` method, from that attribute on.
        let allow = ["impl", "ID", "{", "#", "[", "ID", "(", "ID", ")", "]"];
        expected.extend(allow.map(|text| (text, false, 5)));
        expected.extend(attribute(true, 6));
        expected.extend(item(true, 6));
        expected.extend(item(false, 7));
        expected.push(("}", false, 7));
        // `cfg(not(test))` and an inner `#![cfg(test)]` are not test items.
        let not = ["#", "[", "ID", "(", "ID", "(", "ID", ")", ")", "]"];
        expected.extend(not.map(|text| (text, false, 7)));
        expected.extend(item(false, 7));
        let inner = [
            "mod", "ID", "{", "#", "!", "[", "ID", "(", "ID", ")", "]", "}",
        ];
        expected.extend(inner.map(|text| (text, false, 7)));
        assert_eq!(marks, expected);
    }

    #[test]
    fn a_file_under_tests_is_all_test_code_and_one_syn_cannot_parse_all_production() {
        let code = "#[cfg(test)]\nmod t {}\nfn a() {}\n";
        let (tokens, error) = tokenize_file(code, true).unwrap();
        assert!(error.is_none());
        assert!(tokens.iter().all(|token| token.test && token.split == 0));
        assert!(is_test_file("tests/a.rs"));
        assert!(is_test_file("crates/acme/tests/common/mod.rs"));
        assert!(!is_test_file("src/tests.rs"));
        assert!(!is_test_file("src/tests_helper/a.rs"));
        // It lexes but is not Rust syntax.
        let (tokens, error) = tokenize_file("#[cfg(test)]\nmod t { fn }\n", false).unwrap();
        assert!(error.is_some());
        assert!(tokens.iter().all(|token| !token.test));
        let run = search_tree(
            &[("src/odd.rs", "#[cfg(test)]\nmod t { fn }\n", true)],
            None,
        );
        assert_eq!(run.diagnostics.len(), 1, "{:?}", run.diagnostics);
        let expected = "src/odd.rs:2 could not be parsed";
        assert!(
            run.diagnostics[0].starts_with(expected),
            "{:?}",
            run.diagnostics
        );
    }

    #[test]
    fn a_test_module_copying_production_code_of_its_file_is_no_duplicate() {
        let lib = format!(
            "{}\n#[cfg(test)]\nmod tests {{\n    use super::*;\n\n{}\n    #[test]\n    fn ships() {{\n        assert_eq!(calculate_shipping(1, 1), 4);\n    }}\n}}\n",
            shipping("calculate_shipping"),
            shipping("calculate_shipping_copy")
        );
        let run = search_tree(&[("src/lib.rs", &lib, true)], None);
        assert_eq!(summary(&run), Vec::<String>::new());
        assert_eq!(run.diagnostics, Vec::<String>::new());
    }

    #[test]
    fn test_code_copying_test_code_is_a_test_duplicate() {
        let lib = format!(
            "pub fn acme() -> u32 {{\n    1\n}}\n\n#[cfg(test)]\nmod tests_a {{\n{}}}\n\n#[cfg(all(test, feature = \"acme\"))]\nmod tests_b {{\n{}}}\n",
            shipping("first"),
            shipping("second")
        );
        let run = search_tree(&[("src/lib.rs", &lib, true)], None);
        assert_eq!(
            summary(&run),
            ["repeated_in_change test src/lib.rs:5-19 <- src/lib.rs:21-35"]
        );
        // Files under tests/: they pair with one another, never with the
        // production code they copy too; production groups come first.
        let run = search_tree(
            &[
                ("api/tests/one.rs", &shipping("one"), false),
                ("api/tests/two.rs", &shipping("two"), true),
                ("src/lib.rs", &shipping("calculate_shipping"), false),
                ("src/added.rs", &shipping("calculate_shipping_again"), true),
            ],
            None,
        );
        assert_eq!(
            summary(&run),
            [
                "copies_existing production src/added.rs:1-12 <- src/lib.rs:1-12",
                "copies_existing test api/tests/two.rs:1-12 <- api/tests/one.rs:1-12",
            ]
        );
        assert_eq!(run.duplicates[1].tokens, 78);
    }

    #[test]
    fn production_copies_are_found_around_a_test_module_but_not_across_it() {
        let lib = format!(
            "{}#[cfg(test)]\nmod tests {{\n    #[test]\n    fn ships() {{\n        assert_eq!(super::calculate_shipping(1, 1), 4);\n    }}\n}}\n{}",
            shipping("calculate_shipping"),
            shipping("calculate_shipping_again")
        );
        let mut files = vec![("src/lib.rs", lib.as_str(), true)];
        let run = search_tree(&files, None);
        assert_eq!(
            summary(&run),
            ["repeated_in_change production src/lib.rs:1-12 <- src/lib.rs:20-31"]
        );
        // Neither half reaches 50 tokens: joined across the test module they
        // would copy plain.rs.
        let split = format!(
            "{}    #[cfg(test)]\n    mod tests {{\n        fn probe() {{}}\n    }}\n{SHIPPING_TAIL}",
            shipping_head("calculate_shipping_split")
        );
        let tokens = tokenize(&split).unwrap();
        let production: Vec<u32> = tokens
            .iter()
            .filter(|token| !token.test)
            .map(|token| token.split)
            .collect();
        assert_eq!(production, [[0; 42].as_slice(), &[2; 36]].concat());
        let plain = shipping("calculate_shipping");
        files = vec![
            ("src/plain.rs", &plain, false),
            ("src/split.rs", &split, true),
        ];
        assert_eq!(summary(&search_tree(&files, None)), Vec::<String>::new());
        let joined = format!(
            "{}    // No test module in between.\n{SHIPPING_TAIL}",
            shipping_head("calculate_shipping_joined")
        );
        files[1] = ("src/split.rs", &joined, true);
        assert_eq!(summary(&search_tree(&files, None)).len(), 1);
    }

    #[test]
    fn excluded_folders_and_extensions_are_left_out() {
        let lib = shipping("calculate_shipping");
        let copy = shipping("calculate_shipping_copy");
        let files = [
            ("src/lib.rs", lib.as_str(), false),
            ("src/locales/en.rs", copy.as_str(), true),
            // Not under src/: the file system may ignore case.
            ("web/Locales/pt.rs", copy.as_str(), true),
            ("src/messages.i18n.rs", copy.as_str(), true),
        ];
        assert_eq!(summary(&search_tree(&files, None)).len(), 3);
        let exclude = Exclude {
            dirs: vec!["locales".to_string()],
            extensions: vec![".I18N.rs".to_string()],
        };
        assert_eq!(
            summary(&search_tree(&files, Some(exclude.clone()))),
            Vec::<String>::new()
        );
        assert!(exclude.excludes("LOCALES/a.rs"));
        assert!(exclude.excludes("a/b/locales/c/d.rs"));
        assert!(!exclude.excludes("src/locales.rs"));
        assert!(!exclude.excludes("src/locales_extra/a.rs"));
        assert!(exclude.excludes("src/MESSAGES.i18n.RS"));
        assert!(!Exclude::default().excludes("locales/a.rs"));
    }

    #[test]
    fn a_changed_file_with_exclude_matches_the_schema() {
        let changed = json!({
            "protocol": 1,
            "exclude": { "dirs": ["locales"], "extensions": [".po"] },
            "files": [{ "path": "src/lib.rs", "added": [[1, 2]] }]
        });
        let dupes_schema = schema("dupes.schema.json");
        assert_eq!(
            validate(&changed, &dupes_schema, &dupes_schema["$defs"]["changed"]),
            Vec::<String>::new()
        );
        let parsed: Changed = serde_json::from_value(changed).unwrap();
        assert_eq!(
            parsed.exclude,
            Some(Exclude {
                dirs: vec!["locales".to_string()],
                extensions: vec![".po".to_string()],
            })
        );
    }
}
