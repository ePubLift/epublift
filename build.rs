//! Build metadata for `epublift -V`: exposes `EPUBLIFT_BUILD`, which is either
//! empty or SemVer build metadata pinning the exact source, `+<short-hash>`
//! with `.dirty` appended when the tree had uncommitted changes — so `-V`
//! prints e.g. `epublift 2.1.0+9abafee` (veripublica conventions CLI.md §3.1).
//! Source order for the hash:
//!   1. the `GIT_SHA` build env (set where there is no `.git`, e.g. Docker)
//!   2. `git rev-parse` (local builds and the release workflow's checkout)
//!   3. nothing: `-V` then prints the plain version, as a release is pinned by
//!      its tag anyway.
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    watch(".git/HEAD");
    watch(".git/index");
    if let Ok(head) = std::fs::read_to_string(".git/HEAD")
        && let Some(refname) = head.strip_prefix("ref:")
    {
        watch(&format!(".git/{}", refname.trim()));
    }
    println!("cargo:rustc-env=EPUBLIFT_BUILD={}", build_metadata());
}

fn watch(path: &str) {
    if Path::new(path).exists() {
        println!("cargo:rerun-if-changed={path}");
    }
}

fn build_metadata() -> String {
    if let Ok(sha) = std::env::var("GIT_SHA")
        && !sha.trim().is_empty()
    {
        // No working tree to inspect here, so no `.dirty` either.
        let short: String = sha.trim().chars().take(7).collect();
        return format!("+{short}");
    }
    let Some(hash) = git(&["rev-parse", "--short=7", "HEAD"]) else {
        return String::new();
    };
    let dirty = match git(&["status", "--porcelain"]) {
        Some(s) if !s.is_empty() => ".dirty",
        _ => "",
    };
    format!("+{hash}{dirty}")
}

fn git(args: &[&str]) -> Option<String> {
    let out = Command::new("git").args(args).output().ok()?;
    if !out.status.success() {
        return None;
    }
    Some(String::from_utf8(out.stdout).ok()?.trim().to_string())
}
