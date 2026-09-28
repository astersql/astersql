// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use astersql_br_pkg_restore_utils::GetRewriteRuleOfTable;

use crate::log_file_manager::LogDataFileInfo;
use crate::log_split_strategy::{NewLogSplitStrategy, SplitFileThresholdDefault};
use crate::stubs::Context;
use crate::stubs::checkpoint::{LogRestoreValueMarshaled, MemLogMetaManager};

fn rules() -> HashMap<i64, astersql_br_pkg_restore_utils::RewriteRules> {
    HashMap::from([(1, GetRewriteRuleOfTable(1, 100, HashMap::new(), false))])
}

fn file(table_id: i64, length: u64) -> LogDataFileInfo {
    LogDataFileInfo {
        StartKey: vec![1],
        EndKey: vec![2],
        Length: length,
        NumberOfEntries: 7,
        TableId: table_id,
        MetaDataGroupName: "group".into(),
        ..Default::default()
    }
}

#[test]
fn accumulates_only_files_above_threshold_and_splits_after_4096() {
    let ctx = Context::Background();
    let mut strategy = NewLogSplitStrategy(
        &ctx,
        false,
        None,
        rules(),
        Box::new(|_, _| {}),
        SplitFileThresholdDefault,
    )
    .unwrap();

    strategy.Accumulate(&file(1, SplitFileThresholdDefault));
    assert_eq!(strategy.base.AccumulateCount, 0);
    assert!(strategy.base.TableSplitter.is_empty());

    let large = file(1, SplitFileThresholdDefault + 1);
    for _ in 0..4096 {
        strategy.Accumulate(&large);
    }
    assert_eq!(strategy.base.AccumulateCount, 4096);
    assert!(!strategy.ShouldSplit());
    assert!(strategy.base.TableSplitter.contains_key(&1));

    strategy.Accumulate(&large);
    assert_eq!(strategy.base.AccumulateCount, 4097);
    assert!(strategy.ShouldSplit());
}

#[test]
fn skips_meta_unmapped_and_checkpoint_files_and_reports_checkpoint_progress() {
    let ctx = Context::Background();
    let manager = MemLogMetaManager::default();
    manager.data.lock().unwrap().push((
        "group".into(),
        LogRestoreValueMarshaled {
            Goff: 3,
            Foffs: HashMap::from([
                (999, vec![4]), // dropped downstream table: must be filtered out
                (100, vec![5]),
            ]),
        },
    ));
    let skipped_entries = Arc::new(AtomicU64::new(0));
    let skipped_bytes = Arc::new(AtomicU64::new(0));
    let entries = Arc::clone(&skipped_entries);
    let bytes = Arc::clone(&skipped_bytes);
    let mut strategy = NewLogSplitStrategy(
        &ctx,
        true,
        Some(&manager),
        rules(),
        Box::new(move |count, size| {
            entries.fetch_add(count, Ordering::SeqCst);
            bytes.fetch_add(size, Ordering::SeqCst);
        }),
        SplitFileThresholdDefault,
    )
    .unwrap();

    let mut meta = file(1, 11);
    meta.IsMeta = true;
    assert!(strategy.ShouldSkip(&meta));
    assert!(strategy.ShouldSkip(&file(2, 12)));

    let mut dropped_checkpoint = file(1, 13);
    dropped_checkpoint.OffsetInMetaGroup = 3;
    dropped_checkpoint.OffsetInMergedGroup = 4;
    assert!(!strategy.ShouldSkip(&dropped_checkpoint));

    let mut completed = file(1, 14);
    completed.OffsetInMetaGroup = 3;
    completed.OffsetInMergedGroup = 5;
    assert!(strategy.ShouldSkip(&completed));
    assert_eq!(skipped_entries.load(Ordering::SeqCst), 7);
    assert_eq!(skipped_bytes.load(Ordering::SeqCst), 14);
}

#[test]
fn checkpoint_requires_a_manager() {
    let ctx = Context::Background();
    let result = NewLogSplitStrategy(
        &ctx,
        true,
        None,
        rules(),
        Box::new(|_, _| {}),
        SplitFileThresholdDefault,
    );
    assert!(result.is_err());
}
