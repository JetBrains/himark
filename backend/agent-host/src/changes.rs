use std::path::{Path, PathBuf};

use ahp_types::state::{ChangesetFile, FileEdit};

pub(crate) const CHANNEL_PREFIX: &str = "hihost-changes:/";

pub(crate) fn catalog(directories: &[String]) -> Vec<ahp_types::state::Changeset> {
    let mut entries: Vec<ahp_types::state::Changeset> = directories
        .iter()
        .filter_map(|uri| crate::uris::file_path_str(uri))
        .map(|abs| ahp_types::state::Changeset {
            label: "Uncommitted Changes".to_owned(),
            uri_template: format!("{CHANNEL_PREFIX}{abs}"),
            description: Some(abs.to_owned()),
            change_kind: "uncommitted".to_owned(),
            capabilities: None,
        })
        .collect();
    entries.extend(crate::history::catalog(directories));
    entries
}

pub(crate) const REF_PREFIX: &str = "hihost-git:/";

pub(crate) fn channel_folder(channel: &str) -> Option<PathBuf> {
    let path = channel.strip_prefix(CHANNEL_PREFIX)?;
    (path.starts_with('/') && !path.contains('?')).then(|| PathBuf::from(path))
}

pub(crate) fn channel_commit(channel: &str) -> Option<(PathBuf, String)> {
    let path = channel.strip_prefix(CHANNEL_PREFIX)?;
    let (folder, commit) = path.split_once("?commit=")?;
    (folder.starts_with('/') && !commit.is_empty())
        .then(|| (PathBuf::from(folder), commit.to_owned()))
}

pub(crate) fn mint_ref(sha: &str, toplevel: &Path, rel: &str) -> String {
    format!("{REF_PREFIX}{sha}\u{1f}{}\u{1f}{rel}", toplevel.display())
}

pub(crate) fn parse_ref(uri: &str) -> Option<(String, PathBuf, String)> {
    let body = uri.strip_prefix(REF_PREFIX)?;
    let mut parts = body.splitn(3, '\u{1f}');
    let sha = parts.next()?.to_owned();
    let toplevel = PathBuf::from(parts.next()?);
    let rel = parts.next()?.to_owned();
    Some((sha, toplevel, rel))
}

pub(crate) enum Computed {
    NotRepository,
    Files(Vec<ChangesetFile>),
}

pub(crate) fn compute_commit(folder: &Path, commit: &str) -> Computed {
    let Some(toplevel) = higit::toplevel(folder) else {
        return Computed::NotRepository;
    };
    let parent = higit::first_parent(&toplevel, commit);
    let mut files = Vec::new();
    for change in higit::commit_changes(&toplevel, commit) {
        let abs = toplevel.join(&change.rel_path);
        if !abs.starts_with(folder) {
            continue;
        }
        let file_uri = crate::uris::file_uri(&abs);
        let before = match (&parent, change.kind) {
            (Some(parent), higit::ChangeKind::Modified | higit::ChangeKind::Deleted) => {
                Some(serde_json::json!({
                    "uri": file_uri,
                    "content": { "uri": mint_ref(parent, &toplevel, &change.rel_path) },
                }))
            }
            _ => None,
        };
        let after = match change.kind {
            higit::ChangeKind::Deleted => None,
            _ => Some(serde_json::json!({
                "uri": file_uri,
                "content": { "uri": mint_ref(commit, &toplevel, &change.rel_path) },
            })),
        };
        files.push(ChangesetFile {
            id: file_uri,
            edit: FileEdit {
                before,
                after,
                diff: Some(serde_json::json!({
                    "added": change.added,
                    "removed": change.removed,
                })),
            },
            reviewed: None,
            meta: None,
        });
    }
    Computed::Files(files)
}

pub(crate) fn compute(folder: &Path) -> Computed {
    let Some(toplevel) = higit::toplevel(folder) else {
        return Computed::NotRepository;
    };
    let head = higit::head(&toplevel);
    let mut files = Vec::new();
    for change in higit::uncommitted(&toplevel) {
        let abs = toplevel.join(&change.rel_path);
        if !abs.starts_with(folder) {
            continue;
        }
        let file_uri = crate::uris::file_uri(&abs);
        let before = match (&head, change.kind) {
            (Some(sha), higit::ChangeKind::Modified | higit::ChangeKind::Deleted) => {
                Some(serde_json::json!({
                    "uri": file_uri,
                    "content": { "uri": mint_ref(sha, &toplevel, &change.rel_path) },
                }))
            }

            _ => None,
        };
        let after = match change.kind {
            higit::ChangeKind::Deleted => None,
            _ => Some(serde_json::json!({ "uri": file_uri })),
        };
        files.push(ChangesetFile {
            id: file_uri,
            edit: FileEdit {
                before,
                after,
                diff: Some(serde_json::json!({
                    "added": change.added,
                    "removed": change.removed,
                })),
            },
            reviewed: None,
            meta: None,
        });
    }
    Computed::Files(files)
}
