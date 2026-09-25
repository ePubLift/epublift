//! `epublift -V` / `--version` (veripublica conventions CLI.md §3.1): print
//! `epublift <semver>` to stdout and exit 0. The semver may carry build
//! metadata (`+<short-hash>`, `.dirty`) when the build knew its source.

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_epublift"))
        .args(args)
        .output()
        .expect("run epublift");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn version_flags_print_the_tool_and_its_semver() {
    let want = format!("epublift {}", env!("CARGO_PKG_VERSION"));
    for flag in ["-V", "--version"] {
        let (code, stdout, stderr) = run(&[flag]);
        assert_eq!(code, Some(0), "{flag}");
        assert!(stderr.is_empty(), "{flag} wrote to stderr: {stderr}");
        let line = stdout.trim_end();
        assert!(line.starts_with(&want), "{flag}: {line:?}");
        // Anything after the version is SemVer build metadata, `+hash[.dirty]`.
        let rest = &line[want.len()..];
        assert!(
            rest.is_empty() || (rest.starts_with('+') && !rest.contains(' ')),
            "{flag}: unexpected suffix {rest:?}"
        );
    }
}
