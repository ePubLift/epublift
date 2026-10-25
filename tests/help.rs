//! A help request short-circuits (veripublica conventions CLI.md §5): `-h` /
//! `--help` anywhere on the line prints help and exits 0, even when the rest
//! of the line is malformed. After `--` it is an operand, not a help request.

use std::process::Command;

fn run(args: &[&str]) -> (Option<i32>, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_epublift"))
        .args(args)
        .output()
        .expect("run epublift");
    (
        out.status.code(),
        String::from_utf8_lossy(&out.stdout).into_owned(),
    )
}

#[test]
fn help_wins_over_an_unknown_option() {
    for (bad, good) in [
        (&["--zzz-nope", "--help"][..], &["--help"][..]),
        (&["--zzz-nope", "-h"], &["-h"]),
        (
            &["meta", "enrich", "--zzz-nope", "--help"],
            &["meta", "enrich", "--help"],
        ),
        (&["meta", "--zzz-nope", "-h"], &["meta", "-h"]),
        (&["archive", "--help", "--zzz-nope"], &["archive", "--help"]),
    ] {
        let (code, stdout) = run(bad);
        assert_eq!(code, Some(0), "{bad:?}");
        assert_eq!(stdout, run(good).1, "{bad:?} printed other help");
    }
}

#[test]
fn help_after_double_dash_is_not_a_help_request() {
    let (code, _) = run(&["--zzz-nope", "--", "--help"]);
    assert_eq!(code, Some(2));
}
