//! Integration tests for the `epublift check` subcommand (the epubveri
//! validation wiring). These exercise OUR surface — argument parsing, the exit
//! code, and the shared veripublica envelope's skeleton — without pinning to
//! epubveri's exact rule set (which is pre-1.0 and evolves), so they stay robust
//! across validator upgrades.
#![cfg(feature = "validate")]

use std::process::Command;

/// The `epublift` binary built for this test run (with `--features validate`).
fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_epublift")
}

/// Write `bytes` to a temp file with the given name and return its path.
fn tmp_file(name: &str, bytes: &[u8]) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("epublift-check-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn malformed_epub_is_a_verdict_with_exit_1() {
    // Not a ZIP at all. epubveri can still name the defect (a fatal finding), so
    // this is a *verdict*, not an unprocessable input: exit 1, never 2.
    let path = tmp_file("not-a-real.epub", b"this is plainly not an epub");
    let out = Command::new(bin())
        .arg("check")
        .arg(&path)
        .output()
        .expect("run epublift check");

    assert_eq!(out.status.code(), Some(1), "invalid EPUB should exit 1");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("FAIL"),
        "human report should mark the file as failing; got:\n{stdout}"
    );
}

#[test]
fn fatal_findings_are_counted_in_the_human_summary() {
    // epubveri counts fatals apart from errors, so a book stopped dead by a
    // corrupt container would read "FAIL (0 errors, 0 warnings)" if we only ever
    // printed errors/warnings — a failure that names nothing. Guard that.
    let path = tmp_file("fatal-count.epub", b"not an epub either");
    let out = Command::new(bin())
        .arg("check")
        .arg(&path)
        .output()
        .expect("run epublift check");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("1 fatal"),
        "a fatal finding must be named in the summary line; got:\n{stdout}"
    );
    // The human report shouts the severity; only the JSON carries the lowercase
    // wire value. Reading the envelope's `severity` straight through would
    // silently downcase this.
    assert!(
        stdout.contains("FATAL "),
        "human report should print severity uppercase; got:\n{stdout}"
    );
}

#[test]
fn json_is_the_shared_envelope_and_flags_invalid() {
    let path = tmp_file("bad-for-json.epub", b"still not an epub");
    let out = Command::new(bin())
        .args(["check", "--json"])
        .arg(&path)
        .output()
        .expect("run epublift check --json");

    // Non-zero exit even in JSON mode.
    assert_eq!(out.status.code(), Some(1));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON object");

    // The veripublica envelope skeleton (FORMATS.md §1.1): exactly one object,
    // naming the tool that produced it and the convention it conforms to.
    assert_eq!(v["tool"], "epublift");
    assert_eq!(v["convention"], "0.6");
    assert!(v["tool_version"].is_string());
    // `status` mirrors the exit code: a graded book with fatal findings is
    // "problems" (exit 1), never "error" (which means no report was possible).
    assert_eq!(v["status"], "problems");

    // One self-contained input object per path, in command-line order.
    let inputs = v["inputs"].as_array().expect("inputs is an array");
    assert_eq!(inputs.len(), 1);
    assert_eq!(inputs[0]["status"], "problems");
    assert!(
        inputs[0]["path"]
            .as_str()
            .unwrap()
            .contains("bad-for-json.epub"),
        "the path we passed must round-trip into the report"
    );
    // Findings carry the shared item fields a consumer relies on.
    let items = inputs[0]["items"].as_array().expect("items is an array");
    assert!(!items.is_empty(), "a malformed EPUB must produce a finding");
    assert_eq!(items[0]["type"], "finding");
    assert!(items[0]["code"].is_string());
    // As in epubcheck, a file that isn't a ZIP draws PKG-003 (error) and then
    // PKG-008 (fatal); the fatal one is what stops the book.
    assert!(
        items
            .iter()
            .any(|i| i["code"] == "PKG-008" && i["severity"] == "fatal"),
        "expected a fatal PKG-008 among: {items:?}"
    );
    // Every summary counter is present, zeros included (FORMATS §1.4), so an
    // absent key never stands in for "none"; and nothing was filtered.
    let summary = &inputs[0]["summary"];
    for key in ["fatal", "error", "warning", "info", "usage"] {
        assert!(summary[key].is_u64(), "summary.{key} missing: {summary}");
    }
    assert!(summary.get("suppressed").is_none());
}

#[test]
fn unreadable_input_exits_2_not_1() {
    let out = Command::new(bin())
        .arg("check")
        .arg("/no/such/path/definitely-missing.epub")
        .output()
        .expect("run epublift check on a missing file");

    // No report was possible for this input, so it is not a verdict: exit 2
    // ("the tool could not do its job"), which a script can tell apart from
    // exit 1 ("the book is bad"). The process must not panic either.
    assert_eq!(out.status.code(), Some(2));
}

#[test]
fn every_input_is_processed_even_after_an_unreadable_one() {
    // A batch must not stop at the first unreadable input: the reports for the
    // rest still appear, and the run exits 2 because one could not be processed.
    let bad = tmp_file("batch-graded.epub", b"not an epub");
    let out = Command::new(bin())
        .args(["check", "--json"])
        .arg("/no/such/path/missing-first.epub")
        .arg(&bad)
        .output()
        .expect("run epublift check on a mixed batch");

    assert_eq!(out.status.code(), Some(2));
    let stdout = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(&stdout).expect("stdout is one JSON object");
    assert_eq!(v["status"], "error");

    let inputs = v["inputs"].as_array().expect("inputs is an array");
    assert_eq!(inputs.len(), 2, "both inputs must be reported");
    // Command-line order is preserved, and the unreadable one says why.
    assert_eq!(inputs[0]["status"], "error");
    assert!(inputs[0]["error"].is_string());
    // The graded book still got its verdict despite the earlier failure.
    assert_eq!(inputs[1]["status"], "problems");
    assert!(!inputs[1]["items"].as_array().unwrap().is_empty());
}

/// Run `epublift check <args> <path>` and return (exit code, stdout, stderr).
fn check_with(args: &[&str], path: &std::path::Path) -> (Option<i32>, String, String) {
    let out = Command::new(bin())
        .arg("check")
        .args(args)
        .arg(path)
        .output()
        .expect("run epublift check");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn format_json_is_the_envelope_and_json_is_its_alias() {
    // veripublica conventions CLI.md §3.6: `--format`, `human` by default; `--json`
    // stays as the older spelling of `--format json` and prints the same bytes.
    let path = tmp_file("format-json.epub", b"not an epub");
    let (code, human, _) = check_with(&[], &path);
    let (_, explicit, _) = check_with(&["--format", "human"], &path);
    assert_eq!(code, Some(1));
    assert_eq!(human, explicit, "human is the default format");

    let (code, json, _) = check_with(&["--format", "json"], &path);
    assert_eq!(code, Some(1));
    let v: serde_json::Value = serde_json::from_str(&json).expect("one JSON object");
    assert_eq!(v["tool"], "epublift");
    let (_, alias, _) = check_with(&["--json"], &path);
    assert_eq!(
        alias, json,
        "--json must print exactly what --format json prints"
    );
}

#[test]
fn format_usage_errors_exit_2() {
    let path = tmp_file("format-usage.epub", b"not an epub");
    // An unsupported value names the supported ones (CLI.md §3.5).
    let (code, _, err) = check_with(&["--format", "xml"], &path);
    assert_eq!(code, Some(2));
    assert!(err.contains("human") && err.contains("json"), "got: {err}");
    // `--format` is single-valued, even when both values agree (CLI.md §3.4).
    let (code, _, _) = check_with(&["--format", "json", "--format", "json"], &path);
    assert_eq!(code, Some(2));
    // `--json` is `--format json`, so giving both is `--format` twice.
    let (code, _, _) = check_with(&["--json", "--format", "human"], &path);
    assert_eq!(code, Some(2));
    let (code, _, _) = check_with(&["--json", "--format", "json"], &path);
    assert_eq!(code, Some(2));
}
