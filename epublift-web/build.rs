//! Capture the short git commit SHA at build time, exposed as the `GIT_SHA`
//! compile-time env (read with `env!("GIT_SHA")`). Source order:
//!   1. the `GIT_SHA` build env (set by Docker/CI where there is no `.git`)
//!   2. `git rev-parse HEAD` (local + the release workflow, which have `.git`)
//!   3. empty (the footer then shows just the version, no commit link)
use std::path::Path;
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-env-changed=GIT_SHA");
    // Rebuild when HEAD moves (local builds); harmless if `.git` is absent.
    if Path::new("../.git/HEAD").exists() {
        println!("cargo:rerun-if-changed=../.git/HEAD");
    }

    let sha = std::env::var("GIT_SHA")
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            Command::new("git")
                .args(["rev-parse", "HEAD"])
                .output()
                .ok()
                .filter(|o| o.status.success())
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        })
        .unwrap_or_default();

    let short: String = sha.chars().take(7).collect();
    println!("cargo:rustc-env=GIT_SHA={short}");
}
