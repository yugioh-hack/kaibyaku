#[cfg(not(panic = "unwind"))]
compile_error!(
    "mdript-cli requires panic=unwind so per-image panic recovery can release worker reservations"
);

mod csv_log;
mod hash_cli;
mod learn;
mod paths;
mod process;
mod updater;
mod updater_hash;
use serde_json::json;
use std::process::ExitCode;
fn main() -> ExitCode {
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    let result = if args.first().is_some_and(|s| s == "identify") {
        process::run(args.into_iter().skip(1).collect())
    } else if args.first().is_some_and(|s| s == "learn") {
        learn::run(args.into_iter().skip(1).collect())
    } else if args.first().is_some_and(|s| s == "update-names") {
        updater::run_names(args.into_iter().skip(1).collect())
    } else if args.first().is_some_and(|s| s == "update-hashes") {
        updater_hash::run(args.into_iter().skip(1).collect())
    } else {
        hash_cli::run()
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{}", json!({"error":e}));
            ExitCode::FAILURE
        }
    }
}
