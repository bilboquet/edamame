//! Heading anchors against the ones GitHub renders, in-app and in the HTML export.
//!
//! The expected anchors were taken from GitHub's Markdown API, which renders repository files
//! with their heading links: `gh api markdown -f mode=markdown -F text=@doc.md`, reading each
//! `id="user-content-…"` in order.  To extend a case list, render the document it describes
//! that way and record what GitHub returns, never what edamame does.
//!
//! One known difference is left out: GitHub's slug includes a footnote reference's number
//! (`# h[^1] i` is `#h1-i`), which edamame leaves out for now (#77).

use edamame::config::Theme;
use edamame::document::ParsedDoc;
use edamame::export::{render_html, HtmlExportOptions};

/// Structural cases: separators, case, inline syntax that adds or hides text, line breaks
/// inside a heading, and duplicate numbering.
const STRUCTURAL: &str = r"# Hello, World!

# API v2 — Release Notes

# snake_case and kebab-case

#   spaces   here

# a	tab

# ΟΔΟΣ ΣΑΣ

# İstanbul

# Rust 🦀 tips

# 見出し、その「一」

# x² ① ★ Ⓐ

# A ![alt text](x.png) b

# c <span>in</span> d

# e \<b\> f

# g &amp; h &copy; i

# `co de` *em* __strong__ ~~del~~

# [link *text*](http://example.com) j

# $x+y$ k

# l <!-- note --> m

# p\* q \# r

# 1. s

# Echo

# Echo

# Echo 1

# Echo 1

# Echo

Foo
bar
===

baz\
qux
---
";

const STRUCTURAL_ANCHORS: &[&str] = &[
    "hello-world",
    "api-v2--release-notes",
    "snake_case-and-kebab-case",
    "spaces---here",
    "atab",
    "οδοσ-σασ",
    "i\u{307}stanbul",
    "rust--tips",
    "見出しその一",
    "x---ⓐ",
    "a--b",
    "c-in-d",
    "e-b-f",
    "g--h--i",
    "co-de-em-strong-del",
    "link-text-j",
    "xy-k",
    "l--m",
    "p-q--r",
    "1-s",
    "echo",
    "echo-1",
    "echo-1-1",
    "echo-1-2",
    "echo-2",
    "foobar",
    "bazqux",
];

/// Up to eight code points from every Unicode general category, spread evenly, plus the
/// edges the slug rules turn on; each is tested as the heading `# U+<code point> a<char>b`.
const CODE_POINTS: &str = include_str!("fixtures/github_anchors.tsv");

fn code_point_cases() -> (String, Vec<String>) {
    let mut doc = String::new();
    let mut anchors = Vec::new();
    for line in CODE_POINTS.lines().filter(|l| !l.starts_with('#')) {
        let (hex, anchor) = line.split_once('\t').expect("`code point<TAB>anchor`");
        let ch = u32::from_str_radix(hex, 16)
            .ok()
            .and_then(char::from_u32)
            .unwrap_or_else(|| panic!("bad code point {hex}"));
        doc.push_str(&format!("# U+{hex} a{ch}b\n\n"));
        anchors.push(anchor.to_owned());
    }
    (doc, anchors)
}

/// `heading_anchors` in document order.
fn in_app_anchors(md: &str) -> Vec<String> {
    let theme: &'static Theme = Box::leak(Box::new(Theme::default()));
    let parsed = ParsedDoc::build(md, theme, true, 80);
    let mut anchors: Vec<(usize, String)> = parsed
        .heading_anchors
        .iter()
        .map(|(slug, &line)| (line, slug.clone()))
        .collect();
    anchors.sort_unstable();
    anchors.into_iter().map(|(_, slug)| slug).collect()
}

/// Every heading `id` the export writes, in document order.
fn exported_anchors(md: &str) -> Vec<String> {
    let html = render_html(md, &HtmlExportOptions::default()).expect("export renders");
    html.split("<h")
        .skip(1)
        .filter(|tag| tag.starts_with(|c: char| ('1'..='6').contains(&c)))
        .filter_map(|tag| {
            let tag = &tag[..tag.find('>')?];
            let start = tag.find(" id=\"")? + 5;
            Some(tag[start..start + tag[start..].find('"')?].to_owned())
        })
        .collect()
}

/// Report every mismatch at once, so one run shows the whole drift.
fn assert_anchors(actual: &[String], expected: &[impl AsRef<str>], what: &str) {
    let mismatches: Vec<String> = actual
        .iter()
        .zip(expected)
        .filter(|(a, e)| a.as_str() != e.as_ref())
        .map(|(a, e)| format!("  expected {:?}, got {a:?}", e.as_ref()))
        .collect();
    assert!(
        mismatches.is_empty() && actual.len() == expected.len(),
        "{what}: {} of {} differ from GitHub ({} anchors, {} expected)\n{}",
        mismatches.len(),
        expected.len(),
        actual.len(),
        expected.len(),
        mismatches.join("\n"),
    );
}

#[test]
fn structural_cases_match_github_in_app() {
    assert_anchors(&in_app_anchors(STRUCTURAL), STRUCTURAL_ANCHORS, "in-app");
}

#[test]
fn structural_cases_match_github_in_the_export() {
    assert_anchors(&exported_anchors(STRUCTURAL), STRUCTURAL_ANCHORS, "export");
}

#[test]
fn every_category_matches_github_in_app() {
    let (doc, expected) = code_point_cases();
    assert_anchors(&in_app_anchors(&doc), &expected, "in-app");
}

#[test]
fn every_category_matches_github_in_the_export() {
    let (doc, expected) = code_point_cases();
    assert_anchors(&exported_anchors(&doc), &expected, "export");
}
