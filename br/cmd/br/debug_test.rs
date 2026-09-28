// Copyright 2026 AsterSQL.

//! Focused parity tests for `debug.rs` and `debug.go`.

use astersql_br_pkg_task::stubs::backuppb::BackupMeta;

use crate::debug::{NewDebugCommand, backup_meta_field};

#[test]
fn debug_command_tree_matches_go() {
    let debug = NewDebugCommand();
    assert_eq!(debug.Use, "debug <subcommand>");
    assert!(debug.Hidden);
    assert_eq!(debug.Aliases, vec!["validate"]);

    let uses: Vec<_> = debug
        .children
        .iter()
        .map(|command| command.Use.as_str())
        .collect();
    assert_eq!(
        uses,
        vec![
            "checksum",
            "backupmeta",
            "decode",
            "encode",
            "reset-pd-config-as-default",
            "search-log-backup",
        ]
    );
    assert!(
        debug
            .children
            .iter()
            .find(|c| c.Use == "checksum")
            .unwrap()
            .Hidden
    );
    assert!(
        debug
            .children
            .iter()
            .all(|c| c.Use != "decode" || c.no_args)
    );
    assert!(
        debug
            .children
            .iter()
            .all(|c| c.Use != "encode" || c.no_args)
    );
    assert!(
        debug
            .children
            .iter()
            .all(|c| c.Use != "reset-pd-config-as-default" || c.no_args)
    );
    assert!(
        debug
            .children
            .iter()
            .all(|c| c.Use != "search-log-backup" || c.no_args)
    );
}

#[test]
fn decode_field_supports_every_serialized_backup_meta_field_and_legacy_aliases() {
    let meta = BackupMeta {
        StartVersion: 11,
        EndVersion: 22,
        IsRawKv: true,
        ApiVersion: 7,
        ..Default::default()
    };

    assert_eq!(
        backup_meta_field(&meta, "start-version").as_deref(),
        Some("11")
    );
    assert_eq!(
        backup_meta_field(&meta, "end-version").as_deref(),
        Some("22")
    );
    assert_eq!(backup_meta_field(&meta, "IsRawKv").as_deref(), Some("true"));
    assert_eq!(backup_meta_field(&meta, "ApiVersion").as_deref(), Some("7"));
    assert_eq!(backup_meta_field(&meta, "missing"), None);
}
