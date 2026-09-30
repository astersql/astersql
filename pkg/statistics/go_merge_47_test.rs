// Copyright 2026 AsterSQL.

use crate::{DecodeColumnTopNValue, TopNMeta, topNMetaToDatum};

struct AnalyzeTopNDefaultRestore(u64);

impl Drop for AnalyzeTopNDefaultRestore {
    fn drop(&mut self) {
        vardef::AnalyzeDefaultNumTopN.Store(self.0);
    }
}

struct AnalyzeBucketDefaultRestore(u64);

impl Drop for AnalyzeBucketDefaultRestore {
    fn drop(&mut self) {
        vardef::AnalyzeDefaultNumBuckets.Store(self.0);
    }
}

#[test]
fn go_merge_47_bucket_pruning_uses_active_analyze_default() {
    let _restore = AnalyzeBucketDefaultRestore(vardef::AnalyzeDefaultNumBuckets.Load());
    let context = stmtctx::NewStmtCtx();
    let mut collector = crate::SampleCollector::New(20, 32);
    for value in 1_i64..=20 {
        collector
            .Collect(&context, types::NewIntDatum(value))
            .unwrap();
    }
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);
    vardef::AnalyzeDefaultNumBuckets.Store(20);
    let (default_hist, _) = crate::BuildHistAndTopN(
        &context,
        20,
        2,
        1,
        &mut collector.clone(),
        &field_type,
        true,
    )
    .unwrap();
    vardef::AnalyzeDefaultNumBuckets.Store(256);
    let (explicit_hist, _) =
        crate::BuildHistAndTopN(&context, 20, 2, 1, &mut collector, &field_type, true).unwrap();
    assert!(default_hist.Len() <= 9);
    assert!(explicit_hist.Len() > default_hist.Len());
}

#[test]
fn go_merge_47_top_n_pruning_uses_active_analyze_default() {
    let _restore = AnalyzeTopNDefaultRestore(vardef::AnalyzeDefaultNumTopN.Load());
    let context = stmtctx::NewStmtCtx();
    let mut collector = crate::SampleCollector::New(100, 16);
    for value in [1_i64, 2] {
        collector
            .Collect(&context, types::NewIntDatum(value))
            .unwrap();
    }
    collector.Count = 100;
    let field_type = types::NewFieldType(types::mysql::TypeLonglong);

    vardef::AnalyzeDefaultNumTopN.Store(2);
    let (_, pruned) =
        crate::BuildHistAndTopN(&context, 2, 2, 1, &mut collector.clone(), &field_type, true)
            .unwrap();
    vardef::AnalyzeDefaultNumTopN.Store(100);
    let (_, explicit) =
        crate::BuildHistAndTopN(&context, 2, 2, 1, &mut collector, &field_type, true).unwrap();
    assert_eq!(pruned.Num(), 1);
    assert_eq!(explicit.Num(), 2);
}

#[test]
fn go_merge_47_decode_duration_and_preserve_string_comparison_bytes() {
    let mut duration_type = types::NewFieldType(types::mysql::TypeDuration);
    duration_type.SetDecimal(0);
    let duration = 10 * 60 * 60 * 1_000_000_000_i64 + 30 * 60 * 1_000_000_000_i64;
    let encoded = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![types::NewIntDatum(duration)],
    )
    .unwrap();
    let decoded = DecodeColumnTopNValue(&encoded, &duration_type, codec::time::UTC).unwrap();
    assert_eq!(decoded.Kind(), types::KindMysqlDuration);
    assert_eq!(decoded.GetMysqlDuration().Duration, duration);
    assert_eq!(decoded.GetMysqlDuration().Fsp, 0);

    let string_type = types::NewFieldType(types::mysql::TypeVarchar);
    let bytes = vec![0, 255, 66];
    let encoded = codec::EncodeKey(
        codec::time::UTC,
        Vec::new(),
        vec![types::NewBytesDatum(bytes.clone())],
    )
    .unwrap();
    let decoded = DecodeColumnTopNValue(&encoded, &string_type, codec::time::UTC).unwrap();
    assert_eq!(decoded.Kind(), types::KindBytes);
    assert_eq!(decoded.GetBytes(), bytes);

    assert!(DecodeColumnTopNValue(&[0xff], &string_type, codec::time::UTC).is_err());

    let index_value = TopNMeta {
        Encoded: vec![0xff, 0x00],
        Count: 1,
    };
    let index_datum = topNMetaToDatum(&index_value, &string_type, true, codec::time::UTC).unwrap();
    assert_eq!(index_datum.GetBytes(), index_value.Encoded);
}

#[test]
fn go_merge_47_top_n_timestamp_respects_location() {
    let time_context = types::BasicTimeContext {
        flags: types::TimeFlags::default(),
        location: codec::time::UTC,
    };
    let instant = types::ParseTime(
        &time_context,
        "2016-06-23 11:30:45",
        types::mysql::TypeTimestamp,
        0,
    )
    .unwrap();
    let packed = instant.ToPackedUint().unwrap();
    let value = TopNMeta {
        Encoded: codec::EncodeKey(
            codec::time::UTC,
            Vec::new(),
            vec![types::NewUintDatum(packed)],
        )
        .unwrap(),
        Count: 1,
    };
    let field_type = types::NewFieldType(types::mysql::TypeTimestamp);
    let utc = topNMetaToDatum(&value, &field_type, false, codec::time::UTC).unwrap();
    let tokyo = topNMetaToDatum(&value, &field_type, false, chrono_tz::Asia::Tokyo).unwrap();
    assert_ne!(utc.GetMysqlTime().String(), tokyo.GetMysqlTime().String());
}
