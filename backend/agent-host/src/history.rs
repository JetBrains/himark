use std::path::{Path, PathBuf};

use himark_ahp_ext_types::history::{
    Commit, CommitAuthor, CommitRef, HistoryHead, HistoryState, HistoryStatus,
};

pub(crate) const CHANNEL_PREFIX: &str = "hihost-history:/";

pub(crate) const WINDOW: usize = 100;

pub(crate) fn catalog(directories: &[String]) -> Vec<ahp_types::state::Changeset> {
    directories
        .iter()
        .filter_map(|uri| crate::uris::file_path_str(uri))
        .map(|abs| ahp_types::state::Changeset {
            label: "History".to_owned(),
            uri_template: format!("{CHANNEL_PREFIX}{abs}"),
            description: Some(abs.to_owned()),
            change_kind: himark_ahp_ext_types::history::HISTORY_CHANGE_KIND.to_owned(),
            capabilities: None,
        })
        .collect()
}

pub(crate) fn channel_folder(channel: &str) -> Option<PathBuf> {
    let path = channel.strip_prefix(CHANNEL_PREFIX)?;
    path.starts_with('/').then(|| PathBuf::from(path))
}

pub(crate) enum Computed {
    NotRepository,
    Window(HistoryState),
}

pub(crate) fn compute(folder: &Path, limit: usize) -> Computed {
    let Some(toplevel) = higit::toplevel(folder) else {
        return Computed::NotRepository;
    };
    let info = higit::head_info(&toplevel);
    let outgoing: std::collections::HashSet<String> =
        higit::outgoing(&toplevel).into_iter().collect();
    let (commits, more) = window(folder, &toplevel, &outgoing, 0, limit);
    Computed::Window(HistoryState {
        status: HistoryStatus::Ready,
        error: None,
        head: HistoryHead {
            branch: info.branch,
            upstream: info.upstream,
            ahead: info.ahead,
            behind: info.behind,
        },
        commits,
        more,
    })
}

pub(crate) fn compute_slice(
    folder: &Path,
    skip: usize,
    limit: usize,
) -> Option<(Vec<Commit>, Option<String>)> {
    let toplevel = higit::toplevel(folder)?;
    let outgoing: std::collections::HashSet<String> =
        higit::outgoing(&toplevel).into_iter().collect();
    Some(window(folder, &toplevel, &outgoing, skip, limit))
}

fn window(
    folder: &Path,
    toplevel: &Path,
    outgoing: &std::collections::HashSet<String>,
    skip: usize,
    limit: usize,
) -> (Vec<Commit>, Option<String>) {
    let log = higit::log(toplevel, skip, limit);
    let full = log.len() == limit;
    let commits: Vec<Commit> = log
        .into_iter()
        .map(|entry| commit_of(folder, entry, outgoing))
        .collect();

    let more = full.then(|| (skip + limit).to_string());
    (commits, more)
}

fn commit_of(
    folder: &Path,
    entry: higit::LogCommit,
    outgoing: &std::collections::HashSet<String>,
) -> Commit {
    let refs = entry
        .decorations
        .iter()
        .filter_map(|deco| {
            let deco = deco.trim();
            if let Some(target) = deco.strip_prefix("HEAD -> ") {
                return Some(CommitRef {
                    name: target.to_owned(),
                    kind: "branch".to_owned(),
                });
            }
            if deco == "HEAD" {
                return Some(CommitRef {
                    name: deco.to_owned(),
                    kind: "head".to_owned(),
                });
            }
            if let Some(tag) = deco.strip_prefix("tag: ") {
                return Some(CommitRef {
                    name: tag.to_owned(),
                    kind: "tag".to_owned(),
                });
            }
            (!deco.is_empty()).then(|| CommitRef {
                name: deco.to_owned(),
                kind: match deco.contains('/') {
                    true => "remote".to_owned(),
                    false => "branch".to_owned(),
                },
            })
        })
        .collect();
    let changeset = format!(
        "{}{}?commit={}",
        crate::changes::CHANNEL_PREFIX,
        folder.display(),
        entry.id
    );
    Commit {
        outgoing: outgoing.contains(&entry.id),
        parents: entry.parents,
        summary: entry.summary,
        message: entry.message,
        author: CommitAuthor {
            name: entry.author_name,
            email: (!entry.author_email.is_empty()).then_some(entry.author_email),
            timestamp: entry.timestamp,
        },
        refs,
        changeset,
        id: entry.id,
    }
}
