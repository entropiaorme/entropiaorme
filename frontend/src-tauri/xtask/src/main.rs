//! Repository task runner: the CI guards, as a single binary.
//!
//! Replaces the Python guard scripts that previously lived under
//! `backend/scripts/`. Each guard is a subcommand; every subcommand exits
//! non-zero on failure with a clear message, matching the pass/fail semantics
//! (and, where reasonable, the wording) of the Python script it ports.
//!
//! Usage:
//!   xtask ratify-check        --range <BASE>..<HEAD>
//!   xtask authoring-lint      --range <BASE>..<HEAD>
//!   xtask version-stamps
//!   xtask mutation-floors     --outcomes <PATH>
//!   xtask no-bare-setinterval [--warn-only]
//!   xtask bump-version        <NEW_VERSION>

mod authoring;
mod bump_version;
mod git;
mod mutation_floors;
mod no_bare_setinterval;
mod ratify;
mod version_stamps;

use std::process::ExitCode;

const USAGE: &str = "\
xtask: repository CI guards

USAGE:
    xtask <subcommand> [options]

SUBCOMMANDS:
    ratify-check    --range <BASE>..<HEAD>   guard golden changes behind a recorded ratification verdict
    authoring-lint  --range <BASE>..<HEAD>   flag em dashes and US spellings on newly added lines
    version-stamps                            assert the app version stamps agree across the tree
    mutation-floors --outcomes <PATH>         enforce per-file cargo-mutants score floors
    no-bare-setinterval [--warn-only]         forbid bare setInterval and the retired tracking event in the frontend
    bump-version <NEW_VERSION>                rewrite the app version stamps in lock-step
";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(subcommand) = args.first() else {
        eprint!("{USAGE}");
        return ExitCode::FAILURE;
    };
    let rest = &args[1..];

    let result: Result<i32, String> = match subcommand.as_str() {
        "ratify-check" => ratify::run(rest),
        "authoring-lint" => authoring::run(rest),
        "version-stamps" => version_stamps::run(rest),
        "mutation-floors" => mutation_floors::run(rest),
        "no-bare-setinterval" => no_bare_setinterval::run(rest),
        "bump-version" => bump_version::run(rest),
        "-h" | "--help" | "help" => {
            print!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        other => {
            eprintln!("xtask: unknown subcommand {other:?}\n");
            eprint!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };

    match result {
        Ok(code) => ExitCode::from(code as u8),
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Pull the value following a flag (e.g. `--range A..B`) out of an argument list.
///
/// Returns `Ok(None)` when the flag is absent, `Ok(Some(value))` when present
/// with a value, and `Err` when present without a following value. Kept here so
/// every subcommand parses its flags the same way without a CLI dependency.
pub fn flag_value(args: &[String], flag: &str) -> Result<Option<String>, String> {
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            return match iter.next() {
                Some(value) => Ok(Some(value.clone())),
                None => Err(format!("xtask: {flag} requires a value")),
            };
        }
        if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            return Ok(Some(value.to_string()));
        }
    }
    Ok(None)
}
