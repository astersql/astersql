// Copyright 2026 AsterSQL.

use std::{env, path::Path, process::ExitCode};

use astersql_parsergen::{check_generated_outputs, write_generated_outputs};

const HELP: &str = "\
Generate or check the committed AsterSQL parser tables.

Usage: astersql-parsergen <COMMAND>

Commands:
  generate  Write parser tables to pkg/parser/generated
  check     Check committed parser tables without writing files
  help      Print this help

Options:
  -h, --help  Print help
";

fn main() -> ExitCode {
    let mut arguments = env::args().skip(1);
    let command = arguments.next();
    if arguments.next().is_some() {
        eprintln!("error: expected exactly one command\n\n{HELP}");
        return ExitCode::from(2);
    }

    let parser_root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("parsergen must be located below pkg/parser");
    match command.as_deref() {
        Some("generate") => match write_generated_outputs(parser_root) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Some("check") => match check_generated_outputs(parser_root) {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("error: {error}");
                ExitCode::FAILURE
            }
        },
        Some("help" | "-h" | "--help") => {
            print!("{HELP}");
            ExitCode::SUCCESS
        }
        Some(command) => {
            eprintln!("error: unknown command `{command}`\n\n{HELP}");
            ExitCode::from(2)
        }
        None => {
            eprintln!("error: missing command\n\n{HELP}");
            ExitCode::from(2)
        }
    }
}
