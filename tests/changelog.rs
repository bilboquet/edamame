//! `CHANGELOG.md`'s trailing link-reference block matches its version headings.
//!
//! Each `## [x.y.z]` heading is a shortcut reference link, resolved on GitHub by a
//! `[x.y.z]: …/compare/vPREV...vX.Y.Z` definition at the foot of the file.  A heading with no
//! definition renders as literal `[x.y.z]` text — no error, nothing visibly broken — so the
//! block silently fell four releases behind.  This pins it: the footer must be exactly the
//! block the headings imply, in order, so a release that renames `[Unreleased]` without
//! updating it fails CI on the next push.
//!
//! Plain line matching, no Markdown parse, the same shapes `app::post_upgrade::changelog`
//! slices by.

const CHANGELOG_MD: &str = include_str!("../CHANGELOG.md");

/// The label a `## [label]` heading names, or `None` for any other line.
fn heading_label(line: &str) -> Option<&str> {
    let rest = line.strip_prefix("## [")?;
    Some(&rest[..rest.find(']')?])
}

/// A `[label]: url` link-reference definition line.
fn is_definition(line: &str) -> bool {
    line.starts_with('[') && line.contains("]:")
}

/// The footer the headings imply, newest first: `[Unreleased]` compares the newest tag with
/// `HEAD`, each release compares the release below it with its own tag, and the oldest links
/// to its tag's release page.
fn expected_definitions(labels: &[&str], repo: &str) -> Vec<String> {
    labels
        .iter()
        .enumerate()
        .map(|(i, &label)| {
            let to = if label == "Unreleased" {
                "HEAD".to_owned()
            } else {
                format!("v{label}")
            };
            match labels.get(i + 1) {
                Some(prev) => format!("[{label}]: {repo}/compare/v{prev}...{to}"),
                None => format!("[{label}]: {repo}/releases/tag/{to}"),
            }
        })
        .collect()
}

#[test]
fn the_link_reference_block_matches_the_version_headings() {
    let labels: Vec<&str> = CHANGELOG_MD.lines().filter_map(heading_label).collect();
    assert_eq!(
        labels.first(),
        Some(&"Unreleased"),
        "the changelog should open with an [Unreleased] section"
    );

    let actual: Vec<&str> = CHANGELOG_MD.lines().filter(|l| is_definition(l)).collect();
    let expected = expected_definitions(&labels, env!("CARGO_PKG_REPOSITORY"));
    assert_eq!(
        actual,
        expected,
        "CHANGELOG.md's link-reference block is out of step with its `## [x.y.z]` headings; \
         replace it with:\n\n{}\n",
        expected.join("\n"),
    );
}

#[test]
fn expected_definitions_chain_each_release_to_the_one_below() {
    let repo = "https://example.com/r";
    assert_eq!(
        expected_definitions(&["Unreleased", "0.2.0", "0.1.0"], repo),
        [
            "[Unreleased]: https://example.com/r/compare/v0.2.0...HEAD",
            "[0.2.0]: https://example.com/r/compare/v0.1.0...v0.2.0",
            "[0.1.0]: https://example.com/r/releases/tag/v0.1.0",
        ],
    );
}
