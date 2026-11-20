mod cli;

use std::process::ExitCode;

fn main() -> ExitCode {
    let args = match cli::parse(std::env::args().skip(1)) {
        Ok(cli::ParseResult::Run(a)) => a,
        Ok(cli::ParseResult::Help) => {
            println!("{}", cli::USAGE);
            return ExitCode::SUCCESS;
        }
        Err(e) => {
            eprintln!("kagegami: {e}\n{}", cli::USAGE);
            return ExitCode::from(2);
        }
    };
    eprintln!("kagegami: {:?}", args);
    ExitCode::SUCCESS
}
