// Copyright 2026 AsterSQL.

// Handle / RuntimeStatsBuilder 相关的 Aster 单元测试。
//
// 覆盖统计租约（lease）、强制 delta flush、ANALYZE 后元数据与历史版本、
// 时区敏感直方图边界编码、复合索引 TopN，以及 RuntimeStatsBuilder 的内存记账释放。

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use std::collections::HashMap;
use std::sync::Mutex;

use crate::{
    AnalyzeStatsStorage, Error, Handle, HandleBackend, ResetDumpStatsDeltaRatio,
    SetDumpStatsDeltaRatio, dump_stats_delta_ratio,
};

/// 构造带名称与字段类型的列元信息。
fn named_column(id: i64, name: &str, field_type: u8) -> astersql_meta_model::ColumnInfo {
    let mut column = astersql_meta_model::ColumnInfo::default();
    column.ID = id;
    column.Name.O = name.to_owned();
    column.Name.L = name.to_owned();
    column.FieldType.SetType(field_type);
    column
}

/// 构造含双列复合索引的表；`index_offset` 可注入非法 Offset 以测试错误路径。
fn composite_table(index_offset: isize) -> astersql_meta_model::TableInfo {
    let columns = vec![named_column(1, "a", 3), named_column(2, "b", 3)];
    let mut index = astersql_meta_model::IndexInfo::default();
    index.ID = 3;
    index.Name.O = "idx".to_owned();
    index.Name.L = "idx".to_owned();
    index.Columns = vec![
        astersql_meta_model::IndexColumn {
            Name: columns[0].Name.clone(),
            Offset: 0,
            ..Default::default()
        },
        astersql_meta_model::IndexColumn {
            Name: columns[1].Name.clone(),
            Offset: index_offset,
            ..Default::default()
        },
    ];
    astersql_meta_model::TableInfo {
        ID: 9,
        Columns: columns,
        Indices: vec![index],
        ..Default::default()
    }
}

/// 空实现的 HandleBackend：各钩子均为 no-op，用于观察 Handle 本身状态机。
#[derive(Default)]
struct RecordingBackend;

impl HandleBackend for RecordingBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }
    fn system_schema(&mut self, _database_id: i64) -> Result<bool, Error> {
        Ok(false)
    }
    fn reset_session_stats_list(&mut self) {}
    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), Error> {
        Ok(())
    }
    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

/// 记录 `dump_stats_delta` 调用次数的后端，用于断言强制 flush。
#[derive(Clone, Default)]
struct FlushRecordingBackend {
    /// 已执行的 delta flush 次数。
    flushes: Arc<AtomicUsize>,
}

impl HandleBackend for FlushRecordingBackend {
    fn memory_schema_id(&self, _database_id: i64) -> bool {
        false
    }
    fn system_schema(&mut self, _database_id: i64) -> Result<bool, Error> {
        Ok(false)
    }
    fn reset_session_stats_list(&mut self) {}
    fn dump_stats_delta(&mut self, _dump_all: bool) -> Result<(), Error> {
        self.flushes.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }
    fn start_usage_worker(&mut self) {}
    fn close_pool(&mut self) {}
    fn close_usage(&mut self) {}
    fn close_auto_analyze(&mut self) {}
}

/// 验证租约读写、强制 dump_stats_delta_to_kv，以及 dump 比例全局变量的设置/重置。
#[test]
fn handle_exposes_lease_and_forced_delta_flush() {
    let backend = FlushRecordingBackend::default();
    let flushes = Arc::clone(&backend.flushes);
    let mut handle = Handle::new(backend, false, false).expect("create stats handle");

    assert_eq!(handle.lease(), std::time::Duration::ZERO);
    handle.set_lease(std::time::Duration::from_millis(10));
    assert_eq!(handle.lease(), std::time::Duration::from_millis(10));
    handle
        .dump_stats_delta_to_kv(true)
        .expect("forced delta flush");
    assert_eq!(flushes.load(Ordering::Relaxed), 1);

    SetDumpStatsDeltaRatio(0.25);
    assert_eq!(dump_stats_delta_ratio(), 0.25);
    ResetDumpStatsDeltaRatio();
}

/// 验证注册表、记录 DML 增量、ANALYZE 发布后 realtime/modify/version 与历史 stats 一致。
#[test]
fn canonical_handle_context_observes_live_analyze_and_history_state() {
    let handle = Arc::new(Mutex::new(
        Handle::new(RecordingBackend::default(), false, true).expect("create stats handle"),
    ));
    let context = Arc::clone(&handle);
    handle
        .lock()
        .unwrap()
        .register_table_stats(42)
        .expect("register table in canonical handle");

    handle
        .lock()
        .unwrap()
        .record_table_mutation(42, 3, 3)
        .expect("record committed row delta");
    assert_eq!(
        context.lock().unwrap().stats_meta(42).unwrap().modify_count,
        3
    );

    let version = handle
        .lock()
        .unwrap()
        .analyze_table_stats(42)
        .expect("publish analyzed stats");
    let current = context
        .lock()
        .unwrap()
        .stats_meta(42)
        .cloned()
        .expect("current stats meta");
    assert_eq!(current.realtime_count, 3);
    assert_eq!(current.modify_count, 0);
    assert_eq!(current.version, version);
    assert_eq!(
        context
            .lock()
            .unwrap()
            .historical_stats(42)
            .last()
            .unwrap()
            .version,
        version
    );
}

/// 验证 RuntimeStatsBuilder：时区影响时间戳边界、文本排序规则、复合索引 TopN 形状与非法 Offset 报错。
#[test]
fn typed_builder_preserves_timezone_and_strict_composite_key_shape() {
    let mut timestamp = *astersql_types::field::NewFieldType(7);
    timestamp.SetCollate("utf8mb4_general_ci".to_owned());
    let rows = vec![vec![Some("2024-01-02 03:04:05".to_owned())]];
    let utc = astersql_statistics::RuntimeStatsBuilder::NewWithTimeZoneName("UTC").unwrap();
    let shanghai =
        astersql_statistics::RuntimeStatsBuilder::NewWithTimeZoneName("Asia/Shanghai").unwrap();
    let (utc_histogram, _) = utc
        .build_histogram(1, std::slice::from_ref(&timestamp), &rows, false, 0)
        .unwrap();
    let (shanghai_histogram, _) = shanghai
        .build_histogram(1, std::slice::from_ref(&timestamp), &rows, false, 0)
        .unwrap();
    assert_ne!(
        utc.encode_histogram_bound(&utc_histogram, 0, false)
            .unwrap(),
        shanghai
            .encode_histogram_bound(&shanghai_histogram, 0, false)
            .unwrap()
    );
    let mut text = *astersql_types::field::NewFieldType(15);
    text.SetCharset("utf8mb4".to_owned());
    text.SetCollate("utf8mb4_general_ci".to_owned());
    let (text_histogram, _) = utc
        .build_histogram(
            2,
            std::slice::from_ref(&text),
            &[vec![Some("A".to_owned())], vec![Some("a".to_owned())]],
            false,
            0,
        )
        .unwrap();
    assert_eq!(text_histogram.Tp.GetCollate(), "binary");
    assert_eq!(text_histogram.Tp.GetCharset(), "utf8mb4");

    let table = composite_table(1);
    let rows = [("1", "1"), ("1", "1"), ("1", "2"), ("1", "2")]
        .into_iter()
        .map(|(a, b)| {
            HashMap::from([
                ("a".to_owned(), Some(a.to_owned())),
                ("b".to_owned(), Some(b.to_owned())),
            ])
        })
        .collect::<Vec<_>>();
    let index_types = table.Indices[0]
        .Columns
        .iter()
        .map(|column| table.Columns[column.Offset as usize].FieldType.clone())
        .collect::<Vec<_>>();
    let index_rows = rows
        .iter()
        .map(|row| vec![row["a"].clone(), row["b"].clone()])
        .collect::<Vec<_>>();
    let (index_histogram, _) = utc
        .build_histogram(3, &index_types, &index_rows, true, 0)
        .unwrap();
    assert_eq!(
        index_histogram.Tp.GetType(),
        astersql_types::field::mysql::TypeBlob
    );
    assert_eq!(
        astersql_statistics::DecodeRuntimeStatsValue(
            &utc.encode_histogram_bound(&index_histogram, 0, true)
                .unwrap(),
            2,
        )
        .unwrap(),
        "(1, 1)"
    );
    let stats = crate::BuildRuntimeTableStats(9, &table, &rows, 1, 2).unwrap();
    let mut topn = stats.indexes[&3]
        .top_n
        .iter()
        .map(|(encoded, count)| {
            Ok((
                astersql_statistics::DecodeRuntimeStatsValue(encoded, 2)
                    .map_err(|error| error.to_string())?,
                *count,
            ))
        })
        .collect::<Result<Vec<_>, String>>()
        .unwrap();
    topn.sort();
    assert_eq!(
        topn,
        vec![("(1, 1)".to_owned(), 2), ("(1, 2)".to_owned(), 2)]
    );
    assert!(
        astersql_statistics::DecodeRuntimeStatsValue(&stats.indexes[&3].top_n[0].0, 3)
            .unwrap_err()
            .to_string()
            .contains("expected 3")
    );

    let invalid = composite_table(7);
    let error = crate::BuildRuntimeTableStats(9, &invalid, &rows, 1, 2).unwrap_err();
    assert!(error.contains("offset 7 exceeds table column count 2"));
}

/// 验证 build_histogram 成功与失败路径都将 memory_consumed 归零，且峰值大于 0。
#[test]
fn runtime_collector_releases_exact_input_memory_on_success_and_error() {
    let builder = astersql_statistics::RuntimeStatsBuilder::default();
    let text = *astersql_types::field::NewFieldType(15);
    let rows = (0..64)
        .map(|_| vec![Some("x".repeat(8 * 1024))])
        .collect::<Vec<_>>();
    assert_eq!(builder.memory_consumed(), 0);
    builder
        .build_histogram(1, std::slice::from_ref(&text), &rows, false, 0)
        .unwrap();
    assert_eq!(builder.memory_consumed(), 0);
    assert!(builder.max_memory_consumed() > 64 * 8 * 1024);

    let error_builder = astersql_statistics::RuntimeStatsBuilder::default();
    let integer = *astersql_types::field::NewFieldType(astersql_types::field::mysql::TypeLonglong);
    let error_rows = vec![
        vec![Some("1".to_owned())],
        vec![Some("not-an-integer".to_owned())],
    ];
    let error = error_builder
        .build_histogram(2, std::slice::from_ref(&integer), &error_rows, false, 0)
        .unwrap_err();
    assert!(!error.to_string().is_empty());
    assert_eq!(error_builder.memory_consumed(), 0);
    assert!(error_builder.max_memory_consumed() > 0);
}
