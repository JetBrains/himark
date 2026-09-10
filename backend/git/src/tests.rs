use super::*;

fn repo() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical root");
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .arg("-c")
            .arg("core.fsmonitor=false")
            .arg("-C")
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["init", "-q"]);
    sh(&["config", "user.email", "test@example.com"]);
    sh(&["config", "user.name", "Test"]);
    std::fs::create_dir_all(root.join("src")).expect("mkdir");
    std::fs::write(root.join("README.md"), "# readme\n\nold body\n").unwrap();
    std::fs::write(root.join("src/lib.rs"), "fn main() {}\n").unwrap();
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "first commit"]);
    std::fs::write(root.join("README.md"), "# readme\n\nnew body\n").unwrap();
    std::fs::write(root.join("src/notes.md"), "note one\nnote two\n").unwrap();
    (dir, root)
}

#[test]
fn uncommitted_lists_tracked_edits_and_untracked_adds() {
    let (_dir, root) = repo();
    let top = toplevel(&root).expect("a repository");
    assert_eq!(top, root);
    let changes = uncommitted(&top);
    let paths: Vec<&str> = changes.iter().map(|c| c.rel_path.as_str()).collect();
    assert_eq!(paths, vec!["README.md", "src/notes.md"], "{changes:?}");
    assert_eq!(changes[0].kind, ChangeKind::Modified);
    assert_eq!((changes[0].added, changes[0].removed), (Some(1), Some(1)));
    assert_eq!(changes[1].kind, ChangeKind::Added);
    assert_eq!(changes[1].added, Some(2), "untracked counts its lines");
}

#[test]
fn show_answers_the_committed_text_and_refuses_the_untracked() {
    let (_dir, root) = repo();
    let sha = head(&root).expect("a born HEAD");
    assert_eq!(
        show(&root, &sha, "README.md").as_deref(),
        Some("# readme\n\nold body\n"),
        "the before side is HEAD's text"
    );
    assert!(
        show(&root, &sha, "src/notes.md").is_none(),
        "no before for untracked"
    );
    assert!(tracked_at(&root, &sha, "README.md"));
    assert!(!tracked_at(&root, &sha, "src/notes.md"));
}

#[test]
fn an_unborn_head_answers_only_untracked() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().canonicalize().expect("canonical");
    let sh = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git")
            .status
            .success());
    };
    sh(&["init", "-q"]);
    std::fs::write(root.join("fresh.md"), "hello\n").unwrap();
    assert!(head(&root).is_none(), "unborn");
    let changes = uncommitted(&root);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].kind, ChangeKind::Added);
}

#[test]
fn a_plain_folder_is_not_a_repository() {
    let dir = tempfile::tempdir().expect("tempdir");
    assert!(toplevel(dir.path()).is_none());
}

#[test]
fn the_signal_is_the_reflog_and_grows_on_commit() {
    let (_dir, root) = repo();
    let signal_path = signal(&root).expect("a signal");
    assert!(
        signal_path.ends_with("logs/HEAD"),
        "the reflog: {signal_path:?}"
    );
    let before = std::fs::metadata(&signal_path).expect("reflog").len();
    let sh = |args: &[&str]| {
        assert!(std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git")
            .status
            .success());
    };
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "second"]);
    let after = std::fs::metadata(&signal_path).expect("reflog").len();
    assert!(after > before, "a commit appends the reflog");
}

#[test]
fn the_log_windows_with_decorations_and_paging() {
    let (_dir, root) = repo();
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "second commit\n\nwith a body"]);

    let window = log(&root, 0, 10);
    assert_eq!(window.len(), 2);
    assert_eq!(window[0].summary, "second commit");
    assert_eq!(
        window[0].message.as_deref(),
        Some("second commit\n\nwith a body")
    );
    assert_eq!(window[0].parents.len(), 1);
    assert_eq!(window[0].parents[0], window[1].id);
    assert!(window[0]
        .decorations
        .iter()
        .any(|deco| deco.starts_with("HEAD -> ")));
    assert_eq!(window[1].summary, "first commit");
    assert_eq!(window[1].parents, Vec::<String>::new());
    assert_eq!(window[1].message, None, "a bare summary carries no body");
    assert_eq!(window[1].author_name, "Test");
    assert!(window[1].timestamp > 0);

    let older = log(&root, 1, 10);
    assert_eq!(older.len(), 1);
    assert_eq!(older[0].id, window[1].id);
    assert!(log(&root, 2, 10).is_empty());
}

#[test]
fn commit_changes_lists_one_commits_files_against_its_parent() {
    let (_dir, root) = repo();
    let sh = |args: &[&str]| {
        let output = std::process::Command::new("git")
            .args(["-c", "core.fsmonitor=false", "-C"])
            .arg(&root)
            .args(args)
            .output()
            .expect("git runs");
        assert!(output.status.success(), "git {args:?}: {output:?}");
    };
    sh(&["add", "."]);
    sh(&["commit", "-q", "-m", "second"]);
    let head = head(&root).expect("a born HEAD");

    let changes = commit_changes(&root, &head);
    let paths: Vec<&str> = changes.iter().map(|c| c.rel_path.as_str()).collect();
    assert_eq!(paths, vec!["README.md", "src/notes.md"], "{changes:?}");
    assert_eq!(changes[0].kind, ChangeKind::Modified);
    assert_eq!((changes[0].added, changes[0].removed), (Some(1), Some(1)));
    assert_eq!(changes[1].kind, ChangeKind::Added);
    assert_eq!(
        first_parent(&root, &head).as_deref(),
        Some(log(&root, 1, 1)[0].id.as_str())
    );

    let first = log(&root, 1, 1)[0].id.clone();
    let initial = commit_changes(&root, &first);
    assert_eq!(initial.len(), 2);
    assert!(initial
        .iter()
        .all(|change| change.kind == ChangeKind::Added));
    assert_eq!(first_parent(&root, &first), None);
}

#[test]
fn commit_all_commits_the_working_tree() {
    let (_dir, root) = repo();
    commit_all(&root, "from the wire").expect("the commit lands");
    assert!(uncommitted(&root).is_empty(), "nothing left uncommitted");
    let window = log(&root, 0, 1);
    assert_eq!(window[0].summary, "from the wire");

    assert!(commit_all(&root, "again").is_err());
}

#[test]
fn head_info_reads_the_branch_without_an_upstream() {
    let (_dir, root) = repo();
    let info = head_info(&root);
    assert!(info.branch.is_some(), "{info:?}");
    assert_eq!(info.upstream, None);
    assert_eq!((info.ahead, info.behind), (None, None));
    assert!(outgoing(&root).is_empty(), "no upstream, no outgoing set");
}
