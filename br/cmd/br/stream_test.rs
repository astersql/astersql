// Copyright 2026 AsterSQL.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use crate::stream::{install_stream_help, streamCommand};
use crate::stubs::Command;
use astersql_br_pkg_task::stubs::FlagValue;

#[test]
fn execution_errors_restore_usage_output_like_go_defer() {
    let mut command = Command {
        Use: "mystery".into(),
        SilenceUsage: true,
        ..Default::default()
    };
    astersql_br_pkg_task::DefineCommonFlags(command.PersistentFlags());
    astersql_br_pkg_task::DefineStreamCommonFlags(command.Flags());
    command
        .Flags()
        .Set("task-name", FlagValue::String("task".into()));

    let err = streamCommand(&mut command, "log mystery").unwrap_err();

    assert!(
        err.msg.contains("unknown stream command"),
        "unexpected error: {}",
        err.msg
    );
    assert!(!command.SilenceUsage);
}

#[test]
fn stream_help_hides_flags_then_delegates_to_the_original_help() {
    let called = Arc::new(AtomicBool::new(false));
    let observed = Arc::clone(&called);
    let mut command = Command::default();
    command.SetHelpFunc(Arc::new(move |_command, _args| {
        observed.store(true, Ordering::SeqCst);
    }));

    install_stream_help(&mut command);
    let help = command.HelpFunc();
    help(&mut command, &[]);

    assert!(called.load(Ordering::SeqCst));
}
