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
