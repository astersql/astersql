// Copyright 2026 AsterSQL.

//! 流式备份元文件名解析契约：legacy/tagged 格式与 shift-ts 状态。
//! 与 Go backupmetas 公开 API 向量保持一致。

use crate::{ParseName, ShiftTSStatus, TryParseTaggedBackupMetaFileName};

fn tagged(suffix: &str) -> String {
    format!("000000000000000a000000000000000b-{suffix}")
}

#[test]
fn go_rust_public_contract_matches() {
    // legacy：四段十六进制 TS，HasDDLFiles 为真。
    let legacy = "0000000000000001-0000000000000002-0000000000000003-0000000000000004";
    let p = ParseName(legacy).unwrap();
    assert_eq!(p.FlushTS, 1);
    assert_eq!(p.MinBeginTsInDefaultCf, 2);
    assert_eq!(p.MinTS, 3);
    assert_eq!(p.MaxTS, 4);
    assert!(p.HasDDLFiles());

    // tagged：后缀标签连续拼接，无 '-' 分隔 tag 段。
    let tagged =
        "000000000000000a000000000000000b-d000000000000000cl000000000000000du000000000000000e";
    let t = ParseName(tagged).unwrap();
    assert_eq!(t.FlushTS, 0xa);
    assert_eq!(t.StoreID, 0xb);
    assert_eq!(t.MinBeginTsInDefaultCf, 0xc);
    assert_eq!(t.MinTS, 0xd);
    assert_eq!(t.MaxTS, 0xe);
    assert_eq!(TryParseTaggedBackupMetaFileName(tagged).unwrap(), t);

    // shift ts：窗口命中则返回 MinBeginTs；否则 NotFound。
    let (ts, st) = t.CalculateShiftTS(0xd, 0xe);
    assert_eq!(st, ShiftTSStatus::ShiftTSFound);
    assert_eq!(ts, 0xc);
    let (_, st2) = t.CalculateShiftTS(0x100, 0x200);
    assert_eq!(st2, ShiftTSStatus::ShiftTSNotFound);

    // errors：非法名失败；legacy 不能当 tagged 解析。
    assert!(ParseName("bad").is_err());
    assert!(TryParseTaggedBackupMetaFileName(legacy).is_err());
}

#[test]
fn shift_ts_matches_go_window_and_invalid_stats_branches() {
    let parsed = ParseName(&tagged(
        "d0000000000000002l0000000000000003u0000000000000004",
    ))
    .unwrap();
    assert_eq!(
        parsed.CalculateShiftTS(4, 4),
        (2, ShiftTSStatus::ShiftTSFound)
    );
    assert_eq!(
        parsed.CalculateShiftTS(5, 5),
        (0, ShiftTSStatus::ShiftTSNotFound)
    );
    assert_eq!(
        parsed.CalculateShiftTS(1, 2),
        (0, ShiftTSStatus::ShiftTSNotFound)
    );

    let zero_begin = ParseName(&tagged(
        "d0000000000000000l0000000000000003u0000000000000004",
    ))
    .unwrap();
    assert_eq!(
        zero_begin.CalculateShiftTS(3, 4),
        (0, ShiftTSStatus::ShiftTSInvalidStats)
    );
    let begin_after_min = ParseName(&tagged(
        "d0000000000000004l0000000000000003u0000000000000004",
    ))
    .unwrap();
    assert_eq!(
        begin_after_min.CalculateShiftTS(3, 4),
        (0, ShiftTSStatus::ShiftTSInvalidStats)
    );
}

#[test]
fn ddl_flag_presence_and_bits_match_go() {
    let without_flags = ParseName(&tagged(
        "d0000000000000002l0000000000000003u0000000000000004",
    ))
    .unwrap();
    assert!(!without_flags.HasFlags);
    assert!(without_flags.HasDDLFiles());

    let zero_flags = ParseName(&tagged(
        "d0000000000000002l0000000000000003u0000000000000004p0000000000000000",
    ))
    .unwrap();
    assert!(zero_flags.HasFlags);
    assert!(zero_flags.HasDDLFiles());

    let no_ddl = ParseName(&tagged(
        "d0000000000000002l0000000000000003u0000000000000004p0000000000000001",
    ))
    .unwrap();
    assert!(!no_ddl.HasDDLFiles());
}

#[test]
fn tagged_validation_and_forward_compatibility_match_go() {
    let duplicate = tagged("d0000000000000002d0000000000000002l0000000000000003u0000000000000004");
    assert!(
        ParseName(&duplicate)
            .unwrap_err()
            .contains("duplicate suffix tag")
    );

    let missing_required = tagged("d0000000000000002l0000000000000003");
    assert!(
        ParseName(&missing_required)
            .unwrap_err()
            .contains("missing 'u' tag")
    );

    let with_unknown =
        tagged("A00000000000000ffd0000000000000002l0000000000000003u0000000000000004");
    let parsed = ParseName(&with_unknown).unwrap();
    assert_eq!(parsed.FlushTS, 0xa);
    assert_eq!(parsed.StoreID, 0xb);
    assert_eq!(parsed.MinTS, 3);

    for invalid in [
        "00000000000000010000000000000002-",
        "00000000000000010000000000000002-d000000000000000g",
        "00000000000000010000000000000002-_0000000000000001",
    ] {
        assert!(ParseName(invalid).is_err(), "unexpectedly parsed {invalid}");
    }
}

#[test]
fn empty_flag_requires_presence_and_preserves_ddl_bits() {
    for flags in [0u64, 1, 2, 3, 4, u64::MAX] {
        let parsed = ParseName(&tagged(&format!(
            "d0000000000000000l0000000000000000u0000000000000000p{flags:016X}"
        )))
        .unwrap();
        assert_eq!(parsed.IsEmpty(), flags & 2 != 0);
        assert_eq!(parsed.HasDDLFiles(), flags & 1 == 0);
    }
    assert!(
        !crate::ParsedName {
            Flags: 2,
            HasFlags: false,
            ..Default::default()
        }
        .IsEmpty()
    );
    assert!(
        !ParseName("0000000000000001-0000000000000002-0000000000000003-0000000000000004")
            .unwrap()
            .IsEmpty()
    );
}
