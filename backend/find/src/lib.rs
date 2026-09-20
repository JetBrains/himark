// Copyright © 2026 JetBrains s.r.o.
// SPDX-License-Identifier: Apache-2.0

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

pub use himark_ahp_ext_types::{SearchKind, SearchTarget};

#[derive(Clone, Debug)]
pub struct SearchQuery {
    pub term: String,
    pub kind: SearchKind,
    pub case_sensitive: bool,
    pub target: SearchTarget,
}

#[derive(Debug)]
pub struct SearchHit {
    pub folder: usize,
    pub relative: PathBuf,
}

#[derive(Debug)]
pub struct Scan {
    pub hits: Vec<SearchHit>,
    pub truncated: bool,
}

const FUZZY_CONTENT_CAP: u64 = 2 * 1024 * 1024;

const BINARYISH: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "tiff", "ico", "icns", "heic", "pdf", "zip", "gz",
    "tgz", "tar", "xz", "7z", "rar", "jar", "class", "o", "a", "so", "dylib", "dll", "exe", "bin",
    "dmg", "ttf", "otf", "woff", "woff2", "mp3", "mp4", "m4a", "mov", "avi", "mkv", "wav", "flac",
    "ogg", "db", "sqlite",
];

pub fn scan(
    folders: &[PathBuf],
    query: &SearchQuery,
    limit: usize,
    cancel: &AtomicBool,
) -> Result<Scan, String> {
    if query.term.is_empty() || limit == 0 {
        return Ok(Scan {
            hits: Vec::new(),
            truncated: false,
        });
    }
    let content = content_matcher(query)?;
    let paths = path_matcher(query)?;
    let folded = fold(&query.term, query.case_sensitive);
    let hits = std::sync::Mutex::new(Vec::new());
    let truncated = AtomicBool::new(false);
    for (index, root) in folders.iter().enumerate() {
        if truncated.load(Ordering::Relaxed) {
            break;
        }
        let mut walk = ignore::WalkBuilder::new(root);

        walk.follow_links(false).require_git(false).threads(
            std::thread::available_parallelism()
                .map(|threads| threads.get())
                .unwrap_or(4)
                .min(12),
        );
        walk.build_parallel().run(|| {
            let content = &content;
            let paths = &paths;
            let folded = &folded;
            let hits = &hits;
            let truncated = &truncated;
            Box::new(move |entry| {
                use ignore::WalkState;
                if cancel.load(Ordering::Relaxed) {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }
                let path = entry.path();
                if !openable(path) {
                    return WalkState::Continue;
                }
                let relative = path.strip_prefix(root).unwrap_or(path);
                let hit = match query.target {
                    SearchTarget::Path => {
                        let haystack = relative.to_string_lossy();
                        match query.kind {
                            SearchKind::Text => {
                                fold(&haystack, query.case_sensitive).contains(folded)
                            }
                            SearchKind::Fuzzy => {
                                subsequence_match(&fold(&haystack, query.case_sensitive), folded)
                            }
                            SearchKind::Regex => paths
                                .as_ref()
                                .expect("built for path regex")
                                .is_match(&haystack),
                        }
                    }
                    SearchTarget::Content => match query.kind {
                        SearchKind::Fuzzy => fuzzy_contains(path, folded, query.case_sensitive),
                        SearchKind::Text | SearchKind::Regex => {
                            contains_match(content.as_ref().expect("built for content"), path)
                        }
                    },
                };
                if hit {
                    let mut held = hits.lock().expect("scan hits");
                    if held.len() >= limit {
                        truncated.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                    held.push(SearchHit {
                        folder: index,
                        relative: relative.to_path_buf(),
                    });
                    if held.len() >= limit {
                        truncated.store(true, Ordering::Relaxed);
                        return WalkState::Quit;
                    }
                }
                WalkState::Continue
            })
        });
    }
    let mut hits = hits.into_inner().expect("scan hits");
    hits.sort_by(|a, b| (a.folder, &a.relative).cmp(&(b.folder, &b.relative)));
    Ok(Scan {
        hits,
        truncated: truncated.load(Ordering::Relaxed),
    })
}

/// One match within a file, positioned and carrying its display
/// context — the wire shape of `himark_ahp_ext_types::Location`
/// minus the URI (docs/ahp/ahp-locations.md §2.1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineMatch {
    /// 0-based line.
    pub line: u32,
    /// 0-based byte column within the line.
    pub column: u32,
    /// Match length in bytes.
    pub length: u32,
    pub context: String,
    pub context_column_start: u32,
}

#[derive(Debug)]
pub struct FileMatches {
    pub folder: usize,
    pub relative: PathBuf,
    pub matches: Vec<LineMatch>,
}

/// Context windowing for huge lines: a whole line ships as-is up to
/// MAX_CONTEXT bytes; past that, a window starting shortly before
/// the match.
const MAX_CONTEXT: usize = 256;
const CONTEXT_MARGIN: usize = 32;

/// Validate a query without walking — the minting request refuses a
/// bad regex before a channel exists.
pub fn validate(query: &SearchQuery) -> Result<(), String> {
    content_matcher(query)?;
    path_matcher(query)?;
    Ok(())
}

/// The streaming content scan behind `searchLocations`
/// (docs/ahp/ahp-locations.md §3.1): walks like `scan`, but emits
/// every file's positioned matches through `emit` as the walk finds
/// them. The walk is parallel, so emission order is unspecified.
/// Answers whether the result was truncated — by `limit` (total
/// matches) or by `cancel`.
pub fn scan_locations(
    folders: &[PathBuf],
    query: &SearchQuery,
    limit: usize,
    cancel: &AtomicBool,
    emit: &(dyn Fn(FileMatches) + Sync),
) -> Result<bool, String> {
    match (query.target, query.kind) {
        (SearchTarget::Content, SearchKind::Text | SearchKind::Regex) => {}
        _ => return Err("locations search is a content text/regex search".to_owned()),
    }
    if query.term.is_empty() || limit == 0 {
        return Ok(false);
    }
    let matcher = content_matcher(query)?.expect("built for content");
    let counted = std::sync::atomic::AtomicUsize::new(0);
    let truncated = AtomicBool::new(false);
    for (index, root) in folders.iter().enumerate() {
        if truncated.load(Ordering::Relaxed) || cancel.load(Ordering::Relaxed) {
            truncated.store(true, Ordering::Relaxed);
            break;
        }
        let mut walk = ignore::WalkBuilder::new(root);
        walk.follow_links(false).require_git(false).threads(
            std::thread::available_parallelism()
                .map(|threads| threads.get())
                .unwrap_or(4)
                .min(12),
        );
        walk.build_parallel().run(|| {
            let matcher = &matcher;
            let counted = &counted;
            let truncated = &truncated;
            Box::new(move |entry| {
                use ignore::WalkState;
                if cancel.load(Ordering::Relaxed) {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else {
                    return WalkState::Continue;
                };
                if !entry.file_type().is_some_and(|kind| kind.is_file()) {
                    return WalkState::Continue;
                }
                let path = entry.path();
                if !openable(path) {
                    return WalkState::Continue;
                }
                let mut matches = located_matches(matcher, path);
                if matches.is_empty() {
                    return WalkState::Continue;
                }
                let before = counted.fetch_add(matches.len(), Ordering::Relaxed);
                if before >= limit {
                    truncated.store(true, Ordering::Relaxed);
                    return WalkState::Quit;
                }
                let cut = before + matches.len() > limit;
                if cut {
                    matches.truncate(limit - before);
                    truncated.store(true, Ordering::Relaxed);
                }
                emit(FileMatches {
                    folder: index,
                    relative: path.strip_prefix(root).unwrap_or(path).to_path_buf(),
                    matches,
                });
                match cut {
                    true => WalkState::Quit,
                    false => WalkState::Continue,
                }
            })
        });
    }
    Ok(truncated.load(Ordering::Relaxed))
}

/// Every positioned match in one file, in (line, column) order.
/// Undecodable lines do not match — the walk rule, kept per line so
/// the byte arithmetic between `column` and `context` stays exact.
fn located_matches(matcher: &grep_regex::RegexMatcher, path: &Path) -> Vec<LineMatch> {
    struct Collect<'a> {
        matcher: &'a grep_regex::RegexMatcher,
        out: &'a mut Vec<LineMatch>,
    }
    impl grep_searcher::Sink for Collect<'_> {
        type Error = std::io::Error;
        fn matched(
            &mut self,
            _searcher: &grep_searcher::Searcher,
            sunk: &grep_searcher::SinkMatch<'_>,
        ) -> Result<bool, Self::Error> {
            let Some(line_number) = sunk.line_number() else {
                return Ok(true);
            };
            let Ok(line) = std::str::from_utf8(trim_newline(sunk.bytes())) else {
                return Ok(true);
            };
            use grep_matcher::Matcher as _;
            let mut at = 0usize;
            while at <= line.len() {
                let found = match self.matcher.find_at(line.as_bytes(), at) {
                    Ok(Some(found)) => found,
                    _ => break,
                };
                let (context, context_column_start) = context_window(line, found.start());
                self.out.push(LineMatch {
                    line: line_number.saturating_sub(1) as u32,
                    column: found.start() as u32,
                    length: (found.end() - found.start()) as u32,
                    context,
                    context_column_start: context_column_start as u32,
                });
                // An empty match must still advance the scan.
                at = found.end().max(found.start() + 1);
            }
            Ok(true)
        }
    }
    let mut out = Vec::new();
    let _ = grep_searcher::Searcher::new().search_path(
        matcher,
        path,
        Collect {
            matcher,
            out: &mut out,
        },
    );
    out
}

fn trim_newline(bytes: &[u8]) -> &[u8] {
    let bytes = bytes.strip_suffix(b"\n").unwrap_or(bytes);
    bytes.strip_suffix(b"\r").unwrap_or(bytes)
}

/// The context slice a `Location` ships for a match at byte `start`
/// of `line`: the whole line up to MAX_CONTEXT bytes, else a window
/// beginning shortly before the match. Answers (context,
/// context_column_start). Shared by every locations producer so
/// windows look the same whatever produced them.
pub fn context_window(line: &str, start: usize) -> (String, usize) {
    if line.len() <= MAX_CONTEXT {
        return (line.to_owned(), 0);
    }
    let mut begin = start.saturating_sub(CONTEXT_MARGIN).min(line.len());
    while !line.is_char_boundary(begin) {
        begin -= 1;
    }
    let mut stop = (begin + MAX_CONTEXT).min(line.len());
    while !line.is_char_boundary(stop) {
        stop += 1;
    }
    (line[begin..stop].to_owned(), begin)
}

fn content_matcher(query: &SearchQuery) -> Result<Option<grep_regex::RegexMatcher>, String> {
    let pattern = match (query.target, query.kind) {
        (SearchTarget::Content, SearchKind::Text) => regex_escape(&query.term),
        (SearchTarget::Content, SearchKind::Regex) => query.term.clone(),
        _ => return Ok(None),
    };
    grep_regex::RegexMatcherBuilder::new()
        .case_insensitive(!query.case_sensitive)
        .build(&pattern)
        .map(Some)
        .map_err(|error| format!("invalid regular expression: {error}"))
}

fn path_matcher(query: &SearchQuery) -> Result<Option<regex::Regex>, String> {
    if !(query.target == SearchTarget::Path && query.kind == SearchKind::Regex) {
        return Ok(None);
    }
    regex::RegexBuilder::new(&query.term)
        .case_insensitive(!query.case_sensitive)
        .build()
        .map(Some)
        .map_err(|error| format!("invalid regular expression: {error}"))
}

fn fold(text: &str, case_sensitive: bool) -> String {
    match case_sensitive {
        true => text.to_owned(),
        false => text.to_lowercase(),
    }
}

fn openable(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_none_or(|extension| !BINARYISH.contains(&extension.to_lowercase().as_str()))
}

fn subsequence_match(haystack: &str, term: &str) -> bool {
    let mut chars = term.chars();
    let mut wanted = chars.next();
    for present in haystack.chars() {
        match wanted {
            Some(next) if next == present => wanted = chars.next(),
            Some(_) => {}
            None => break,
        }
    }
    wanted.is_none()
}

fn contains_match(matcher: &grep_regex::RegexMatcher, path: &Path) -> bool {
    struct FirstHit<'a>(&'a mut bool);
    impl grep_searcher::Sink for FirstHit<'_> {
        type Error = std::io::Error;
        fn matched(
            &mut self,
            _searcher: &grep_searcher::Searcher,
            _matched: &grep_searcher::SinkMatch<'_>,
        ) -> Result<bool, Self::Error> {
            *self.0 = true;
            Ok(false)
        }
    }
    let mut hit = false;
    let _ = grep_searcher::Searcher::new().search_path(matcher, path, FirstHit(&mut hit));
    hit
}

fn fuzzy_contains(path: &Path, folded_term: &str, case_sensitive: bool) -> bool {
    let small = std::fs::metadata(path).is_ok_and(|meta| meta.len() <= FUZZY_CONTENT_CAP);
    if !small {
        return false;
    }
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    text.lines()
        .any(|line| subsequence_match(&fold(line, case_sensitive), folded_term))
}

fn regex_escape(term: &str) -> String {
    let mut escaped = String::with_capacity(term.len() * 2);
    for ch in term.chars() {
        if !ch.is_alphanumeric() && !ch.is_whitespace() {
            escaped.push('\\');
        }
        escaped.push(ch);
    }
    escaped
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content_query(term: &str, kind: SearchKind) -> SearchQuery {
        SearchQuery {
            term: term.to_owned(),
            kind,
            case_sensitive: false,
            target: SearchTarget::Content,
        }
    }

    fn collect_locations(
        folders: &[PathBuf],
        query: &SearchQuery,
        limit: usize,
        cancel: &AtomicBool,
    ) -> (Vec<FileMatches>, bool) {
        let emitted = std::sync::Mutex::new(Vec::new());
        let truncated = scan_locations(folders, query, limit, cancel, &|batch| {
            emitted.lock().expect("emissions").push(batch);
        })
        .expect("scan");
        let mut emitted = emitted.into_inner().expect("emissions");
        emitted.sort_by(|a, b| (a.folder, &a.relative).cmp(&(b.folder, &b.relative)));
        (emitted, truncated)
    }

    #[test]
    fn locations_stream_per_file_with_positions() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("a.md"),
            "needle one needle\nplain\nneedle\n",
        )
        .expect("write");
        std::fs::write(dir.path().join("b.md"), "tail Needle\n").expect("write");
        std::fs::write(dir.path().join("c.md"), "nothing here\n").expect("write");
        let folders = vec![dir.path().to_path_buf()];

        let leash = AtomicBool::new(false);
        let (emitted, truncated) = collect_locations(
            &folders,
            &content_query("needle", SearchKind::Text),
            100,
            &leash,
        );
        assert!(!truncated);
        assert_eq!(emitted.len(), 2, "one emission per matched file");

        let a = &emitted[0];
        assert_eq!(a.relative, PathBuf::from("a.md"));
        let positions: Vec<(u32, u32, u32)> = a
            .matches
            .iter()
            .map(|found| (found.line, found.column, found.length))
            .collect();
        assert_eq!(positions, [(0, 0, 6), (0, 11, 6), (2, 0, 6)]);
        assert_eq!(a.matches[1].context, "needle one needle");
        assert_eq!(a.matches[1].context_column_start, 0);

        let b = &emitted[1];
        assert_eq!(b.matches.len(), 1, "case folds by default");
        assert_eq!(b.matches[0].column, 5);
    }

    #[test]
    fn huge_lines_ship_a_window() {
        let dir = tempfile::tempdir().expect("tempdir");
        let line = format!("{}needle{}", "x".repeat(500), "y".repeat(500));
        std::fs::write(dir.path().join("wide.md"), format!("{line}\n")).expect("write");
        let folders = vec![dir.path().to_path_buf()];

        let leash = AtomicBool::new(false);
        let (emitted, _) = collect_locations(
            &folders,
            &content_query("needle", SearchKind::Text),
            100,
            &leash,
        );
        let found = &emitted[0].matches[0];
        assert_eq!(found.column, 500);
        assert_eq!(found.context.len(), MAX_CONTEXT);
        assert_eq!(found.context_column_start as usize, 500 - CONTEXT_MARGIN);
        let inside = (found.column - found.context_column_start) as usize;
        assert_eq!(&found.context[inside..inside + 6], "needle");
    }

    #[test]
    fn the_limit_cuts_the_stream_and_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("many.md"), "hit\nhit\nhit\nhit\n").expect("write");
        let folders = vec![dir.path().to_path_buf()];

        let leash = AtomicBool::new(false);
        let (emitted, truncated) =
            collect_locations(&folders, &content_query("hit", SearchKind::Text), 2, &leash);
        assert!(truncated);
        let total: usize = emitted.iter().map(|batch| batch.matches.len()).sum();
        assert_eq!(total, 2);
    }

    #[test]
    fn locations_respect_the_leash_and_refuse_foreign_queries() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("hit.md"), "needle\n").expect("write");
        let folders = vec![dir.path().to_path_buf()];

        let leash = AtomicBool::new(true);
        let (emitted, truncated) = collect_locations(
            &folders,
            &content_query("needle", SearchKind::Text),
            100,
            &leash,
        );
        assert!(emitted.is_empty());
        assert!(truncated, "a cancelled scan reports the cut");

        let refused = scan_locations(
            &folders,
            &content_query("needle", SearchKind::Fuzzy),
            100,
            &AtomicBool::new(false),
            &|_| {},
        );
        assert!(refused.is_err(), "fuzzy is not a locations search");
        assert!(validate(&content_query("(", SearchKind::Regex)).is_err());
    }

    #[test]
    fn the_leash_stops_the_walk() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("hit.md"), "needle\n").expect("write");
        std::fs::write(dir.path().join("other.md"), "needle\n").expect("write");
        let folders = vec![dir.path().to_path_buf()];
        let query = SearchQuery {
            term: "needle".to_owned(),
            kind: SearchKind::Text,
            case_sensitive: false,
            target: SearchTarget::Content,
        };

        let leash = AtomicBool::new(false);
        let scan = scan(&folders, &query, 100, &leash).expect("scan");
        assert_eq!(scan.hits.len(), 2, "{:?}", scan.hits);
        assert!(!scan.truncated);

        leash.store(true, Ordering::Relaxed);
        let cut = super::scan(&folders, &query, 100, &leash).expect("scan");
        assert!(cut.hits.is_empty(), "{:?}", cut.hits);
        assert!(cut.truncated);
    }
}
