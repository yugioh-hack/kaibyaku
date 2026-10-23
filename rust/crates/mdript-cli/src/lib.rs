#[cfg(not(panic = "unwind"))]
compile_error!(
    "mdript-cli requires panic=unwind so per-image panic recovery can release worker reservations"
);

mod csv_log;
pub mod hash_cli;
pub mod learn;
mod paths;
pub mod process;
pub mod updater;
pub mod updater_hash;

pub use paths::validate_user_data;
