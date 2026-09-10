use super::*;

fn location(kind: ResourceType, path: &[&str]) -> ResourceLocation {
    ResourceLocation::new(
        kind,
        Authority::new("local"),
        path.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
    )
}

#[test]
fn projections_read_the_tail_only() {
    let document = location(ResourceType::document(), &["Users", "x", "Design.MD"]);
    assert_eq!(document.name(), "Design.MD");
    assert_eq!(document.extension(), "md");
    assert_eq!(
        location(ResourceType::document(), &["no-dot"]).extension(),
        ""
    );

    let root = location(ResourceType::directory(), &[]);
    assert_eq!(root.name(), "local", "a rootless path shows the authority");
}

#[test]
fn a_child_extends_the_reified_path_within_the_authority() {
    let notes = location(ResourceType::directory(), &["Users", "x", "notes"]);
    let readme = notes.child(ResourceType::document(), "README.md");
    assert_eq!(readme.path(), &["Users", "x", "notes", "README.md"]);
    assert_eq!(readme.authority(), notes.authority());
    assert!(readme.kind().is_document());
    assert_ne!(readme, notes, "kind and path participate in identity");
}
