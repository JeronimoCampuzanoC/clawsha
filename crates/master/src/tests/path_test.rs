use crate::error::MasterError;
use crate::path::{parse, split_parent};

#[test]
fn root_has_no_components() {
    assert_eq!(parse("/").unwrap(), Vec::<String>::new());
}

#[test]
fn nested_path_is_split_into_components() {
    assert_eq!(
        parse("/docs/2026/a.pdf").unwrap(),
        vec!["docs", "2026", "a.pdf"]
    );
}

#[test]
fn trailing_and_repeated_slashes_are_tolerated() {
    assert_eq!(parse("/docs/").unwrap(), vec!["docs"]);
    assert_eq!(parse("//docs///a").unwrap(), vec!["docs", "a"]);
}

#[test]
fn relative_paths_are_rejected() {
    for bad in ["docs/a", "", "a.pdf"] {
        assert!(
            matches!(parse(bad), Err(MasterError::InvalidPath(_))),
            "should reject {bad:?}"
        );
    }
}

#[test]
fn dot_segments_are_rejected() {
    for bad in ["/docs/../etc", "/./a", "/a/..", "/.."] {
        assert!(
            matches!(parse(bad), Err(MasterError::InvalidPath(_))),
            "should reject {bad:?}"
        );
    }
}

#[test]
fn control_characters_are_rejected() {
    assert!(parse("/docs/a\0b").is_err());
    assert!(parse("/docs/a\nb").is_err());
}

#[test]
fn very_long_names_are_rejected() {
    let long = format!("/{}", "x".repeat(256));
    assert!(parse(&long).is_err());
    let ok = format!("/{}", "x".repeat(255));
    assert!(parse(&ok).is_ok());
}

#[test]
fn split_parent_separates_the_last_component() {
    let (parent, name) = split_parent("/docs/2026/a.pdf").unwrap();
    assert_eq!(parent, vec!["docs", "2026"]);
    assert_eq!(name, "a.pdf");
}

#[test]
fn split_parent_of_top_level_entry_gives_empty_parent() {
    let (parent, name) = split_parent("/docs").unwrap();
    assert!(parent.is_empty());
    assert_eq!(name, "docs");
}

#[test]
fn root_cannot_be_split() {
    assert!(matches!(
        split_parent("/"),
        Err(MasterError::InvalidPath(_))
    ));
}
