use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Change {
    pub rel_path: String,
    pub kind: ChangeKind,

    pub added: Option<i64>,
    pub removed: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeKind {
    Modified,
    Added,
    Deleted,
}

fn git(repo: &Path, args: &[&str]) -> Option<String> {
    let output = std::process::Command::new("git")
        .arg("-c")
        .arg("core.fsmonitor=false")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

fn lines(text: String) -> Vec<String> {
    text.lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

pub fn toplevel(path: &Path) -> Option<PathBuf> {
    let top = git(path, &["rev-parse", "--show-toplevel"])?
        .trim()
        .to_owned();
    (!top.is_empty()).then(|| PathBuf::from(top))
}

pub fn head(toplevel: &Path) -> Option<String> {
    let sha = git(toplevel, &["rev-parse", "HEAD"])?.trim().to_owned();
    (!sha.is_empty()).then_some(sha)
}

pub fn uncommitted(toplevel: &Path) -> Vec<Change> {
    let mut changes: std::collections::BTreeMap<String, Change> = Default::default();
    if head(toplevel).is_some() {
        for line in lines(git(toplevel, &["diff", "HEAD", "--name-status"]).unwrap_or_default()) {
            let mut parts = line.split('\t');
            let Some(status) = parts.next() else { continue };
            match status.chars().next() {
                Some('R') | Some('C') => {
                    let (Some(old), Some(new)) = (parts.next(), parts.next()) else {
                        continue;
                    };
                    changes.insert(
                        old.to_owned(),
                        Change {
                            rel_path: old.to_owned(),
                            kind: ChangeKind::Deleted,
                            added: None,
                            removed: None,
                        },
                    );
                    changes.insert(
                        new.to_owned(),
                        Change {
                            rel_path: new.to_owned(),
                            kind: ChangeKind::Added,
                            added: None,
                            removed: None,
                        },
                    );
                }
                Some(kind) => {
                    let Some(path) = parts.next() else { continue };
                    let kind = match kind {
                        'A' => ChangeKind::Added,
                        'D' => ChangeKind::Deleted,
                        _ => ChangeKind::Modified,
                    };
                    changes.insert(
                        path.to_owned(),
                        Change {
                            rel_path: path.to_owned(),
                            kind,
                            added: None,
                            removed: None,
                        },
                    );
                }
                None => {}
            }
        }

        for line in lines(git(toplevel, &["diff", "HEAD", "--numstat"]).unwrap_or_default()) {
            let mut parts = line.split('\t');
            let (Some(added), Some(removed), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            if let Some(change) = changes.get_mut(path) {
                change.added = added.parse().ok();
                change.removed = removed.parse().ok();
            }
        }
    }
    for path in
        lines(git(toplevel, &["ls-files", "--others", "--exclude-standard"]).unwrap_or_default())
    {
        let added = std::fs::read_to_string(toplevel.join(&path))
            .ok()
            .map(|text| text.lines().count() as i64);
        changes.insert(
            path.clone(),
            Change {
                rel_path: path,
                kind: ChangeKind::Added,
                added,
                removed: Some(0),
            },
        );
    }
    changes.into_values().collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogCommit {
    pub id: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,

    pub timestamp: i64,

    pub decorations: Vec<String>,
    pub summary: String,

    pub message: Option<String>,
}

pub fn log(toplevel: &Path, skip: usize, limit: usize) -> Vec<LogCommit> {
    let count = format!("-n{limit}");
    let skip = format!("--skip={skip}");
    let output = git(
        toplevel,
        &[
            "log",
            &count,
            &skip,
            "--date=unix",

            "--format=%H%x1f%P%x1f%an%x1f%ae%x1f%ad%x1f%D%x1f%s%x1f%B%x1e",
        ],
    );
    let Some(output) = output else {
        return Vec::new();
    };
    output
        .split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches(['\n', '\r']);
            let mut fields = record.split('\u{1f}');
            let id = fields.next()?.trim().to_owned();
            if id.is_empty() {
                return None;
            }
            let parents: Vec<String> = fields
                .next()?
                .split_whitespace()
                .map(str::to_owned)
                .collect();
            let author_name = fields.next()?.to_owned();
            let author_email = fields.next()?.to_owned();
            let timestamp = fields.next()?.trim().parse().ok()?;
            let decorations: Vec<String> = fields
                .next()?
                .split(',')
                .map(str::trim)
                .filter(|deco| !deco.is_empty())
                .map(str::to_owned)
                .collect();
            let summary = fields.next()?.to_owned();
            let body = fields.next()?.trim_end().to_owned();
            let message = (body != summary && !body.is_empty()).then_some(body);
            Some(LogCommit {
                id,
                parents,
                author_name,
                author_email,
                timestamp,
                decorations,
                summary,
                message,
            })
        })
        .collect()
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HeadInfo {
    pub branch: Option<String>,
    pub upstream: Option<String>,
    pub ahead: Option<u32>,
    pub behind: Option<u32>,
}

pub fn head_info(toplevel: &Path) -> HeadInfo {
    let branch = git(toplevel, &["rev-parse", "--abbrev-ref", "HEAD"])
        .map(|out| out.trim().to_owned())
        .filter(|name| !name.is_empty() && name != "HEAD");
    let upstream = git(
        toplevel,
        &[
            "rev-parse",
            "--abbrev-ref",
            "--symbolic-full-name",
            "@{upstream}",
        ],
    )
    .map(|out| out.trim().to_owned())
    .filter(|name| !name.is_empty());
    let (ahead, behind) = match upstream.as_deref() {
        Some(_) => git(
            toplevel,
            &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
        )
        .and_then(|out| {
            let mut parts = out.split_whitespace();
            let behind: u32 = parts.next()?.parse().ok()?;
            let ahead: u32 = parts.next()?.parse().ok()?;
            Some((Some(ahead), Some(behind)))
        })
        .unwrap_or((None, None)),
        None => (None, None),
    };
    HeadInfo {
        branch,
        upstream,
        ahead,
        behind,
    }
}

pub fn outgoing(toplevel: &Path) -> Vec<String> {
    lines(git(toplevel, &["rev-list", "@{upstream}..HEAD"]).unwrap_or_default())
}

pub fn commit_changes(toplevel: &Path, id: &str) -> Vec<Change> {
    let mut changes: std::collections::BTreeMap<String, Change> = Default::default();
    for line in lines(
        git(
            toplevel,
            &[
                "diff-tree",
                "-r",
                "--root",
                "--no-commit-id",
                "--first-parent",
                "-M",
                "--name-status",
                id,
            ],
        )
        .unwrap_or_default(),
    ) {
        let mut parts = line.split('\t');
        let Some(status) = parts.next() else { continue };
        match status.chars().next() {
            Some('R') | Some('C') => {
                let (Some(old), Some(new)) = (parts.next(), parts.next()) else {
                    continue;
                };
                changes.insert(
                    old.to_owned(),
                    Change {
                        rel_path: old.to_owned(),
                        kind: ChangeKind::Deleted,
                        added: None,
                        removed: None,
                    },
                );
                changes.insert(
                    new.to_owned(),
                    Change {
                        rel_path: new.to_owned(),
                        kind: ChangeKind::Added,
                        added: None,
                        removed: None,
                    },
                );
            }
            Some(kind) => {
                let Some(path) = parts.next() else { continue };
                let kind = match kind {
                    'A' => ChangeKind::Added,
                    'D' => ChangeKind::Deleted,
                    _ => ChangeKind::Modified,
                };
                changes.insert(
                    path.to_owned(),
                    Change {
                        rel_path: path.to_owned(),
                        kind,
                        added: None,
                        removed: None,
                    },
                );
            }
            None => {}
        }
    }
    for line in lines(
        git(
            toplevel,
            &[
                "diff-tree",
                "-r",
                "--root",
                "--no-commit-id",
                "--first-parent",
                "-M",
                "--numstat",
                id,
            ],
        )
        .unwrap_or_default(),
    ) {
        let mut parts = line.split('\t');
        let (Some(added), Some(removed), Some(path)) = (parts.next(), parts.next(), parts.next())
        else {
            continue;
        };
        if let Some(change) = changes.get_mut(path) {
            change.added = added.parse().ok();
            change.removed = removed.parse().ok();
        }
    }
    changes.into_values().collect()
}

pub fn first_parent(toplevel: &Path, id: &str) -> Option<String> {
    git(toplevel, &["rev-parse", &format!("{id}^")]).map(|out| out.trim().to_owned())
}

pub fn commit_all(toplevel: &Path, message: &str) -> Result<(), String> {
    git(toplevel, &["add", "-A"]).ok_or("git add -A failed")?;
    git(
        toplevel,
        &["commit", "-m", message, "--no-verify", "--no-gpg-sign"],
    )
    .map(|_| ())
    .ok_or_else(|| "git commit failed (nothing to commit?)".to_owned())
}

pub fn show(toplevel: &Path, rev: &str, rel: &str) -> Option<String> {
    git(toplevel, &["show", &format!("{rev}:{rel}")])
}

pub fn tracked_at(toplevel: &Path, rev: &str, rel: &str) -> bool {
    git(toplevel, &["cat-file", "-e", &format!("{rev}:{rel}")]).is_some()
}

pub fn signal(toplevel: &Path) -> Option<PathBuf> {
    let git_dir = git(toplevel, &["rev-parse", "--absolute-git-dir"])?
        .trim()
        .to_owned();
    if git_dir.is_empty() {
        return None;
    }
    let log = Path::new(&git_dir).join("logs").join("HEAD");
    Some(match log.is_file() {
        true => log,
        false => PathBuf::from(git_dir),
    })
}

#[cfg(test)]
mod tests;
