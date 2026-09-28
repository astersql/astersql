// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc. Licensed under Apache-2.0.

//! Focused parity tests for `compacted_file_strategy.go`.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_utils::GetRewriteRuleOfTable;

use crate::compacted_file_strategy::NewCompactedFileSplitStrategy;
use crate::ssts::{CompactedSSTs, CopiedSST, SSTs};
use crate::stubs::backuppb::{
    File, LogFileSubcompaction, LogFileSubcompactionMeta, RewrittenTableID,
};
use crate::stubs::tablecodec;

fn file(name: &str, table_id: i64, suffix: u8, kvs: u64, size: u64) -> File {
    let mut start_key = tablecodec::EncodeTablePrefix(table_id);
    start_key.push(suffix);
    let mut end_key = tablecodec::EncodeTablePrefix(table_id);
    end_key.push(suffix + 1);
    File {
        Name: name.into(),
        StartKey: start_key,
        EndKey: end_key,
        TotalKvs: kvs,
        Size_: size,
        ..Default::default()
    }
}

fn compacted(table_id: i64, files: Vec<File>) -> CompactedSSTs {
    CompactedSSTs::new(LogFileSubcompaction {
        Meta: LogFileSubcompactionMeta { TableId: table_id },
        SstOutputs: files,
    })
}

#[test]
fn should_split_uses_the_go_strict_threshold() {
    let mut strategy =
        NewCompactedFileSplitStrategy(HashMap::new(), HashSet::new(), Box::new(|_, _| {}));

    strategy.base.AccumulateCount = 4096 / 16;
    assert!(!strategy.ShouldSplit());
    strategy.base.AccumulateCount += 1;
    assert!(strategy.ShouldSplit());
}

#[test]
fn accumulate_counts_every_file_and_scales_non_empty_values() {
    let rules = HashMap::from([(7, GetRewriteRuleOfTable(7, 70, HashMap::new(), false))]);
    let mut strategy = NewCompactedFileSplitStrategy(rules, HashSet::new(), Box::new(|_, _| {}));
    let ssts = compacted(
        7,
        vec![
            file("empty", 7, 1, 0, 160),
            file("small", 7, 2, 15, 15),
            file("normal", 7, 3, 32, 160),
        ],
    );

    strategy.Accumulate(&ssts);

    assert_eq!(strategy.base.AccumulateCount, 3);
    let mut values = Vec::new();
    strategy.TableSplitter()[&7].Traverse(|valued| {
        if valued.Value.Number != 0 || valued.Value.Size != 0 {
            values.push(valued.Value);
        }
        true
    });
    assert!(
        values
            .iter()
            .any(|value| value.Number == 1 && value.Size == 1)
    );
    assert!(
        values
            .iter()
            .any(|value| value.Number == 2 && value.Size == 10)
    );
}

#[test]
fn should_skip_matches_no_rule_all_and_partial_checkpoint_cases() {
    let progress = Arc::new(Mutex::new(Vec::new()));
    let captured = progress.clone();
    let rules = HashMap::from([(7, GetRewriteRuleOfTable(7, 70, HashMap::new(), false))]);
    let mut strategy = NewCompactedFileSplitStrategy(
        rules,
        HashSet::from(["done-a".into(), "done-b".into()]),
        Box::new(move |kvs, size| captured.lock().unwrap().push((kvs, size))),
    );

    let mut no_rule = compacted(8, vec![file("done-a", 8, 1, 3, 30)]);
    assert!(strategy.ShouldSkip(&mut no_rule));
    assert!(progress.lock().unwrap().is_empty());

    let mut all_done = compacted(
        7,
        vec![file("done-a", 7, 1, 3, 30), file("done-b", 7, 2, 4, 40)],
    );
    assert!(strategy.ShouldSkip(&mut all_done));
    assert_eq!(all_done.GetSSTs().len(), 2);
    assert_eq!(*progress.lock().unwrap(), vec![(3, 30), (4, 40)]);

    let mut partial = compacted(
        7,
        vec![file("done-a", 7, 1, 5, 50), file("todo", 7, 2, 6, 60)],
    );
    assert!(!strategy.ShouldSkip(&mut partial));
    assert_eq!(partial.GetSSTs().len(), 1);
    assert_eq!(partial.GetSSTs()[0].Name, "todo");
    assert_eq!(progress.lock().unwrap().last().copied(), Some((5, 50)));
}

#[test]
fn rewritten_ssts_match_rules_by_logical_target_and_accumulate_there() {
    let rules = HashMap::from([(70, GetRewriteRuleOfTable(7, 70, HashMap::new(), false))]);
    let mut strategy = NewCompactedFileSplitStrategy(rules, HashSet::new(), Box::new(|_, _| {}));
    let mut ssts = CopiedSST::new(
        Some(file("copied", 7, 1, 32, 160)),
        RewrittenTableID {
            Upstream: 70,
            Downstream: 7,
        },
    );

    assert!(!strategy.ShouldSkip(&mut ssts));
    strategy.Accumulate(&ssts);
    assert!(strategy.TableSplitter().contains_key(&70));
    assert!(!strategy.TableSplitter().contains_key(&7));
}
