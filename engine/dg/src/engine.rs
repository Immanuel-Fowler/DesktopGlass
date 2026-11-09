//! `dg engine` · `dg schema` (Phase 5).
use std::process::ExitCode;

pub fn run(args: &[String]) -> ExitCode {
    ExitCode::from(dg_engine::run(args) as u8)
}

pub fn schema(_args: &[String]) -> ExitCode {
    println!("{}", serde_json::to_string_pretty(&dg_proto::schema()).unwrap_or_default());
    ExitCode::SUCCESS
}
