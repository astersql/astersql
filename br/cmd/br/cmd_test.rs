// Copyright 2026 AsterSQL.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::cmd::{prepareStatusServer, registerStatusServerPreparer, timestampLogFileName};
use crate::stubs::{Command, StatusServerPreparer};

#[test]
fn timestamp_log_file_name_matches_go_layout() {
    let path = timestampLogFileName();
    let name = std::path::Path::new(&path)
        .file_name()
        .and_then(|name| name.to_str())
        .expect("timestamp log path must have a UTF-8 file name");

    assert_eq!(name.len(), "br.log.2006-01-02T15.04.05Z0700".len());
    assert!(name.starts_with("br.log."));
    for (index, expected) in [(11, '-'), (14, '-'), (17, 'T'), (20, '.'), (23, '.')] {
        assert_eq!(name.as_bytes()[index] as char, expected);
    }
    assert!(matches!(name.as_bytes()[26] as char, 'Z' | '+' | '-'));
}

#[test]
fn registering_status_preparer_again_replaces_the_previous_value() {
    let mut cmd = Command::default();
    let observed = Arc::new(AtomicUsize::new(0));

    for value in [1, 2] {
        let observed = Arc::clone(&observed);
        let preparer: StatusServerPreparer = Arc::new(move |_| {
            observed.store(value, Ordering::SeqCst);
            Ok(None)
        });
        registerStatusServerPreparer(&cmd, preparer);
    }

    prepareStatusServer(&mut cmd).unwrap();
    assert_eq!(observed.load(Ordering::SeqCst), 2);
}

#[test]
fn port_e0faf3fea2_default_and_explicit_redaction_flags() {
    use crate::cmd::{DefineCommonFlags, FlagRedactInfoLog, FlagRedactLog};
    use crate::stubs::effective_task_flags;
    use astersql_br_pkg_task::stubs::FlagValue;

    let mut cmd = Command::default();
    DefineCommonFlags(&mut cmd);
    let defaults = effective_task_flags(&cmd);
    assert!(defaults.GetBool(FlagRedactInfoLog).unwrap());
    assert!(!defaults.GetBool(FlagRedactLog).unwrap());

    for explicit in [false, true] {
        cmd.PersistentFlags()
            .Set(FlagRedactInfoLog, FlagValue::Bool(explicit));
        let flags = effective_task_flags(&cmd);
        assert_eq!(flags.GetBool(FlagRedactInfoLog).unwrap(), explicit);
        assert!(!flags.GetBool(FlagRedactLog).unwrap());
    }
}
