//! Fake CUPS `cancel` command. See `fotoko_printer_sim::fake_cups`.
#![allow(clippy::print_stdout, clippy::print_stderr)]

use std::io::{stderr, stdin, stdout};
use std::process::ExitCode;

use fotoko_printer_sim::fake_cups::{Command, run};

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let code = run(
        Command::Cancel,
        &args,
        |key| std::env::var(key).ok(),
        &mut stdin().lock(),
        &mut stdout().lock(),
        &mut stderr().lock(),
    );
    ExitCode::from(u8::try_from(code).unwrap_or(1))
}
