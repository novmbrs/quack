//! `quack`: see the library's documentation for what each command does.

use std::process::ExitCode;

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match quack::cli::parse(&args) {
        Ok(cli) => quack::app::run(cli),
        Err(problem) => {
            eprintln!("quack: {problem}\n\n{}", quack::cli::USAGE);
            ExitCode::from(2)
        }
    }
}
