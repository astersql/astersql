// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

use crate::execdetails::{NewRuntimeStatsColl, tipb};

#[test]
fn go_merge_22_tiflash_units_preserve_presence_and_overflow() {
    let mut stats = NewRuntimeStatsColl(None);
    let summary = tipb::ExecutorExecutionSummary {
        ExecutorId: "HashJoin_1".into(),
        NumProducedRows: Some(8),
        TiflashHashTableStats: Some(tipb::TiFlashHashTableStats {
            Size_: Some(1),
            SizeKind: tipb::TiFlashHashTableSizeKind::DistinctKeyCount,
        }),
        TiflashScanContext: Some(tipb::TiFlashScanContext {
            UserReadBytes: Some(20),
        }),
        TiflashNetworkSummary: Some(tipb::TiFlashNetworkSummary {
            InnerZoneSendBytes: Some(20),
            InterZoneSendBytes: Some(1),
        }),
        ..Default::default()
    };
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(summary.clone())]);
    let mut second = summary;
    second.TiflashHashTableStats.as_mut().unwrap().SizeKind =
        tipb::TiFlashHashTableSizeKind::BuildRowCount;
    second.TiflashHashTableStats.as_mut().unwrap().Size_ = Some(8);
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(second)]);
    let (units, found) = stats.GetTiFlashExecutionUnits(1);
    assert!(found);
    assert!(!units.Invalid);
    assert_eq!(
        (units.Rows, units.HashDistinctEntries, units.HashBuildRows),
        (16, 1, 8)
    );
    assert_eq!(
        (
            units.UserReadBytes,
            units.InnerZoneSendBytes,
            units.InterZoneSendBytes
        ),
        (40, 40, 2)
    );
    assert_eq!(units.Missing, 0);

    stats.RecordTiFlashExecutionSummaries(
        &[1],
        &[Some(tipb::ExecutorExecutionSummary {
            ExecutorId: "HashJoin_1".into(),
            ..Default::default()
        })],
    );
    let (units, _) = stats.GetTiFlashExecutionUnits(1);
    assert_ne!(units.Missing & 1, 0);
    assert_ne!(units.Observed & 1, 0);

    stats.RecordTiFlashExecutionSummaries(
        &[1],
        &[Some(tipb::ExecutorExecutionSummary {
            ExecutorId: "HashJoin_1".into(),
            NumProducedRows: Some(i64::MAX as u64),
            ..Default::default()
        })],
    );
    assert!(stats.GetTiFlashExecutionUnits(1).0.Invalid);
    let stats = NewRuntimeStatsColl(Some(stats));
    assert!(!stats.GetTiFlashExecutionUnits(1).1);
}

#[test]
fn go_merge_22_tiflash_units_filter_duplicates_and_columnar() {
    let mut stats = NewRuntimeStatsColl(None);
    let summary = tipb::ExecutorExecutionSummary {
        ExecutorId: "TableScan_1".into(),
        ColumnarScanContext: Some(tipb::ColumnarScanContext {
            UserReadBytes: Some(20),
            MvccInputBytes: Some(100),
        }),
        ..Default::default()
    };
    stats.RecordTiFlashExecutionSummaries(
        &[1],
        &[
            None,
            Some(tipb::ExecutorExecutionSummary {
                ExecutorId: "TableScan_2".into(),
                ..Default::default()
            }),
            Some(summary.clone()),
            Some(summary.clone()),
        ],
    );
    let (units, found) = stats.GetTiFlashExecutionUnits(1);
    assert!(found);
    assert!(units.Invalid);
    assert_eq!(units.UserReadBytes, 20);
    assert_ne!(units.Observed & (1 << 2), 0);
    assert!(!stats.GetTiFlashExecutionUnits(2).1);
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(summary)]);
    assert_eq!(stats.GetTiFlashExecutionUnits(1).0.UserReadBytes, 40);
}

#[test]
fn go_merge_22_tiflash_units_missing_zero_unknown_and_overflow() {
    let mut stats = NewRuntimeStatsColl(None);
    let empty = tipb::ExecutorExecutionSummary {
        ExecutorId: "TableScan_1".into(),
        ..Default::default()
    };
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(empty)]);
    let (units, found) = stats.GetTiFlashExecutionUnits(1);
    assert!(found);
    assert_eq!(units.Missing, 15);
    assert_eq!(units.Observed, 0);

    let zero = tipb::ExecutorExecutionSummary {
        ExecutorId: "TableScan_1".into(),
        NumProducedRows: Some(0),
        TiflashHashTableStats: Some(tipb::TiFlashHashTableStats {
            Size_: Some(9),
            SizeKind: tipb::TiFlashHashTableSizeKind::Unknown(99),
        }),
        ColumnarScanContext: Some(tipb::ColumnarScanContext {
            UserReadBytes: Some(0),
            MvccInputBytes: Some(100),
        }),
        TiflashNetworkSummary: Some(tipb::TiFlashNetworkSummary {
            InnerZoneSendBytes: Some(0),
            InterZoneSendBytes: Some(0),
        }),
        ..Default::default()
    };
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(zero)]);
    let (units, _) = stats.GetTiFlashExecutionUnits(1);
    assert!(!units.Invalid);
    assert_eq!(units.Observed, 1 | 4 | 8);
    assert_ne!(units.Missing & 2, 0);
    assert_eq!(units.UserReadBytes, 0);

    let mut stats = NewRuntimeStatsColl(None);
    let overflow = tipb::ExecutorExecutionSummary {
        ExecutorId: "TableScan_1".into(),
        ColumnarScanContext: Some(tipb::ColumnarScanContext {
            UserReadBytes: Some(u64::MAX),
            MvccInputBytes: Some(100),
        }),
        ..Default::default()
    };
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(overflow.clone())]);
    stats.RecordTiFlashExecutionSummaries(&[1], &[Some(overflow)]);
    let (units, _) = stats.GetTiFlashExecutionUnits(1);
    assert!(units.Invalid);
    assert_eq!(units.UserReadBytes, u64::MAX);
}

#[test]
fn go_merge_22_tiflash_units_reject_unrelated_ids_and_prefer_tiflash_scan() {
    let stats = NewRuntimeStatsColl(None);
    let invalid_ids = ["TableScan_0", "TableScan_bad", "TableScan_2"];
    for id in invalid_ids {
        stats.RecordTiFlashExecutionSummaries(
            &[1],
            &[Some(tipb::ExecutorExecutionSummary {
                ExecutorId: id.into(),
                NumProducedRows: Some(5),
                ..Default::default()
            })],
        );
    }
    assert!(!stats.GetTiFlashExecutionUnits(0).1);
    assert!(!stats.GetTiFlashExecutionUnits(1).1);
    assert!(!stats.GetTiFlashExecutionUnits(2).1);

    stats.RecordTiFlashExecutionSummaries(
        &[1],
        &[Some(tipb::ExecutorExecutionSummary {
            ExecutorId: "TableScan_1".into(),
            TiflashScanContext: Some(tipb::TiFlashScanContext {
                UserReadBytes: Some(7),
            }),
            ColumnarScanContext: Some(tipb::ColumnarScanContext {
                UserReadBytes: Some(20),
                MvccInputBytes: Some(100),
            }),
            TiflashNetworkSummary: Some(tipb::TiFlashNetworkSummary {
                InnerZoneSendBytes: Some(3),
                InterZoneSendBytes: None,
            }),
            ..Default::default()
        })],
    );
    let (units, found) = stats.GetTiFlashExecutionUnits(1);
    assert!(found);
    assert_eq!(units.UserReadBytes, 7);
    assert_eq!(units.InnerZoneSendBytes, 3);
    assert_eq!(units.InterZoneSendBytes, 0);
    assert_eq!(units.Observed & (1 << 3), 0);
    assert_ne!(units.Missing & (1 << 3), 0);
}
