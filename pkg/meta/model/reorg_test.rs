// Copyright 2026 AsterSQL.

use crate::group_2::serde_json;
use crate::group_3::{BackfillMeta, DDLReorgMeta};

#[test]
fn reorg_meta_decodes_go_zero_value_and_null_maps() {
    let empty: DDLReorgMeta = serde_json::from_str("{}").expect("decode Go zero value");
    assert!(empty.Warnings.is_empty());
    assert!(empty.WarningsCount.is_empty());

    let with_null_maps: DDLReorgMeta =
        serde_json::from_str(r#"{"warnings":null,"warnings_count":null}"#)
            .expect("decode Go nil maps");
    assert!(with_null_maps.Warnings.is_empty());
    assert!(with_null_maps.WarningsCount.is_empty());
}

#[test]
fn backfill_meta_decodes_go_zero_value_and_null_collections() {
    let mut meta = BackfillMeta::default();
    meta.Decode(b"{}").expect("decode Go zero value");
    assert!(meta.Warnings.is_empty());
    assert!(meta.StartKey.is_empty());

    meta.Decode(
        br#"{"warnings":null,"warnings_count":null,"start_key":null,"end_key":null,"curr_key":null}"#,
    )
    .expect("decode Go nil maps and slices");
    assert!(meta.Warnings.is_empty());
    assert!(meta.WarningsCount.is_empty());
    assert!(meta.StartKey.is_empty());
    assert!(meta.EndKey.is_empty());
    assert!(meta.CurrKey.is_empty());
}

#[test]
fn backfill_meta_uses_go_json_names_and_byte_encoding() {
    let meta = BackfillMeta {
        IsUnique: true,
        StartKey: vec![1, 2, 3],
        EndKey: b"end".to_vec(),
        CurrKey: b"current".to_vec(),
        ..Default::default()
    };
    let encoded = meta.Encode().expect("encode backfill metadata");
    let json: serde_json::Value = serde_json::from_slice(&encoded).expect("parse encoded JSON");
    assert_eq!(json["is_unique"], true);
    assert_eq!(json["start_key"], "AQID");
    assert_eq!(json["end_key"], "ZW5k");
    assert_eq!(json["curr_key"], "Y3VycmVudA==");
    assert!(json.get("IsUnique").is_none());
}
