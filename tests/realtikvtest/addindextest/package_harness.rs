// Copyright 2026 AsterSQL.

use astersql_tests_realtikvtest::stubs::config;
use astersql_tests_realtikvtest::{RunTestMainWith, SetupTestMain, UpdateTiDBConfig};
use astersql_tests_realtikvtest_addindextest::{FULL_MODE, parse_full_mode};
use libtest_mimic::{Arguments, Trial};
use std::sync::atomic::Ordering;

/// Restore package and common configuration after a case clears mutable globals.
pub fn configure() {
    config::UpdateGlobal(|conf| conf.Store = config::StoreTypeTiKV.to_string());
    UpdateTiDBConfig();
    SetupTestMain();
}

/// Each Cargo target has its own process, so each must execute Go's TestMain.
pub fn run(cases: &[(&'static str, fn())]) -> std::process::ExitCode {
    let (full_mode, args) = match parse_full_mode(std::env::args().skip(1)) {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("{error}");
            return std::process::ExitCode::from(2);
        }
    };
    FULL_MODE.store(full_mode, Ordering::SeqCst);
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        println!("  -full-mode[=true|false]  Run all add-index cases (default: false)");
    }
    let args = Arguments::from_iter(std::iter::once("addindextest".to_string()).chain(args));
    config::UpdateGlobal(|conf| conf.Store = config::StoreTypeTiKV.to_string());
    UpdateTiDBConfig();
    let tests = cases
        .iter()
        .map(|&(name, test)| {
            Trial::test(name, move || {
                test();
                Ok(())
            })
        })
        .collect();
    let code = RunTestMainWith(|| {
        if libtest_mimic::run(&args, tests).has_failed() {
            1
        } else {
            0
        }
    });
    std::process::ExitCode::from(code as u8)
}
