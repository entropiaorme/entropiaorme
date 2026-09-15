//! Repository task runner: the CI guards, as a single binary.
//!
//! Replaces the original Python guard scripts. Each guard is a subcommand; every subcommand exits
//! non-zero on failure with a clear message, matching the pass/fail semantics
//! (and, where reasonable, the wording) of the Python script it ports.
//!
//! Usage:
//!
//! ```text
//! xtask ratify-check        --range <BASE>..<HEAD>
//! xtask authoring-lint      --range <BASE>..<HEAD>
//! xtask version-stamps
//! xtask mutation-floors     --outcomes <PATH> [--outcomes <PATH> ...]
//! xtask no-bare-setinterval [--warn-only]
//! xtask no-new-writable     [--warn-only]
//! xtask in-development      [--warn-only]
//! xtask id-order            [--warn-only]
//! xtask route-ceilings      [--warn-only]
//! xtask bump-version        <NEW_VERSION>
//! ```

mod authoring;
mod bump_version;
mod gen_ts;
mod git;
mod id_order;
mod in_development;
mod market_isolation;
mod mutation_floors;
mod no_bare_setinterval;
mod no_new_writable;
mod ratify;
mod route_ceilings;
mod version_stamps;
mod vocabulary;

use std::process::ExitCode;

const USAGE: &str = "\
xtask: repository CI guards

USAGE:
    xtask <subcommand> [options]

SUBCOMMANDS:
    ratify-check    --range <BASE>..<HEAD>   guard golden changes behind a recorded ratification verdict
    authoring-lint  --range <BASE>..<HEAD>   flag em dashes, US spellings, and stray references in what a change adds
    version-stamps                            assert the app version stamps agree across the tree
    mutation-floors --outcomes <PATH>...      enforce per-file cargo-mutants score floors (flag repeats to merge campaign shards)
    no-bare-setinterval [--warn-only]         forbid bare setInterval and the retired tracking event in the frontend
    no-new-writable [--warn-only]             forbid svelte/store imports outside the frozen legacy surface
    market-isolation [--warn-only]            forbid market-layer references on the accounting surface
    in-development [--warn-only]              keep the in-development register and its consumers in step
    id-order [--warn-only]                    require every id-ordered database read to name its allowed case
    route-ceilings [--warn-only]              hold tracked route files at or under their line ceilings
    bump-version <NEW_VERSION>                rewrite the app version stamps in lock-step
    gen-ts [--check]                          emit the TypeScript bindings for the typed IPC commands
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
        "no-new-writable" => no_new_writable::run(rest),
        "market-isolation" => market_isolation::run(rest),
        "in-development" => in_development::run(rest),
        "id-order" => id_order::run(rest),
        "route-ceilings" => route_ceilings::run(rest),
        "bump-version" => bump_version::run(rest),
        "gen-ts" => gen_ts::run(rest),
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
    let values = flag_values(args, flag)?;
    Ok(values.into_iter().next())
}

/// Pull every value of a repeatable flag (e.g. `--outcomes A --outcomes B`)
/// out of an argument list, in order. Empty when the flag is absent; `Err`
/// when any occurrence lacks a following value.
pub fn flag_values(args: &[String], flag: &str) -> Result<Vec<String>, String> {
    let mut values = Vec::new();
    let mut iter = args.iter();
    while let Some(arg) = iter.next() {
        if arg == flag {
            match iter.next() {
                Some(value) => values.push(value.clone()),
                None => return Err(format!("xtask: {flag} requires a value")),
            }
        } else if let Some(value) = arg.strip_prefix(&format!("{flag}=")) {
            values.push(value.to_string());
        }
    }
    Ok(values)
}
