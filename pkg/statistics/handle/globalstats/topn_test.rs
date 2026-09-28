// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// 分区 TopN 合并的单元测试。
//
// 验证 `merge_partition_top_n`：相同编码值跨分区累加频次，
// 超出保留个数 `n` 的项作为 overflow 返回。

#[test]
/// 两个分区的 TopN 含重复键 `a` 时合并为 count=5，次高频 `b` 落入 overflow。
fn canonical_partition_topn_merge_sums_duplicates_and_returns_overflow() {
    let top_ns = vec![
        crate::TopN {
            values: vec![crate::TopNMeta {
                encoded: b"a".to_vec(),
                count: 2,
            }],
        },
        crate::TopN {
            values: vec![
                crate::TopNMeta {
                    encoded: b"a".to_vec(),
                    count: 3,
                },
                crate::TopNMeta {
                    encoded: b"b".to_vec(),
                    count: 1,
                },
            ],
        },
    ];
    let mut histograms = vec![crate::Histogram::default(), crate::Histogram::default()];
    let (top, overflow) = crate::merge_partition_top_n(
        &top_ns,
        1,
        &mut histograms,
        2,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(top.unwrap().values[0].count, 5);
    assert_eq!(overflow[0].encoded, b"b");
}

#[test]
/// 对应 Go 的无直方图场景：多个分区的三个 TopN 值合并后只保留前两个。
fn merge_partition_topn_without_histograms_keeps_total_frequency() {
    let top_ns = (0..10)
        .map(|_| crate::TopN {
            values: vec![
                crate::TopNMeta {
                    encoded: b"one".to_vec(),
                    count: 2,
                },
                crate::TopNMeta {
                    encoded: b"two".to_vec(),
                    count: 2,
                },
                crate::TopNMeta {
                    encoded: b"three".to_vec(),
                    count: 3,
                },
            ],
        })
        .collect::<Vec<_>>();
    let mut histograms = vec![crate::Histogram::default(); 10];
    let (top, overflow) = crate::merge_partition_top_n(
        &top_ns,
        2,
        &mut histograms,
        1,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(top.unwrap().total_count(), 50);
    assert_eq!(overflow.len(), 1);
    assert_eq!(overflow[0].encoded, b"two");
}

#[test]
/// 对应 Go 的有直方图场景：TopN 未覆盖的分区值从 histogram 补回并被移除。
fn merge_partition_topn_reads_and_removes_histogram_frequency() {
    let top_ns = vec![
        crate::TopN {
            values: vec![crate::TopNMeta {
                encoded: b"a".to_vec(),
                count: 2,
            }],
        },
        crate::TopN {
            values: vec![crate::TopNMeta {
                encoded: b"b".to_vec(),
                count: 3,
            }],
        },
    ];
    let mut histograms = vec![
        crate::Histogram {
            exact_counts: std::collections::HashMap::from([(b"b".to_vec(), 4.0)]),
            ..crate::Histogram::default()
        },
        crate::Histogram {
            exact_counts: std::collections::HashMap::from([(b"a".to_vec(), 5.0)]),
            ..crate::Histogram::default()
        },
    ];
    let (top, overflow) = crate::merge_partition_top_n(
        &top_ns,
        2,
        &mut histograms,
        2,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();
    let top = top.unwrap();
    assert_eq!(top.values[0].encoded, b"a");
    assert_eq!(top.values[0].count, 7);
    assert_eq!(top.values[1].encoded, b"b");
    assert_eq!(top.values[1].count, 7);
    assert!(overflow.is_empty());
    assert!(histograms.iter().all(|hist| hist.exact_counts.is_empty()));
}

#[test]
fn topn_merge_rejects_mismatched_histograms_and_cancellation() {
    let top_ns = vec![crate::TopN::default()];
    let mismatch = crate::merge_partition_top_n(
        &top_ns,
        1,
        &mut [],
        2,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap_err();
    assert!(mismatch.contains("mismatch"));

    let cancelled = std::sync::atomic::AtomicBool::new(true);
    let error = crate::merge_global_top_n_by_concurrency(
        &top_ns,
        1,
        &mut [crate::Histogram::default()],
        2,
        1,
        1,
        &cancelled,
    )
    .unwrap_err();
    assert_eq!(error, "query interrupted");
}

#[test]
fn merged_topn_is_encoded_sorted_after_frequency_selection() {
    let top_ns = vec![crate::TopN {
        values: vec![
            crate::TopNMeta {
                encoded: b"z".to_vec(),
                count: 10,
            },
            crate::TopNMeta {
                encoded: b"a".to_vec(),
                count: 5,
            },
            crate::TopNMeta {
                encoded: b"m".to_vec(),
                count: 1,
            },
        ],
    }];
    let mut histograms = vec![crate::Histogram::default()];

    let (top, overflow) = crate::merge_partition_top_n(
        &top_ns,
        2,
        &mut histograms,
        2,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    assert_eq!(
        top.unwrap()
            .values
            .iter()
            .map(|value| value.encoded.as_slice())
            .collect::<Vec<_>>(),
        vec![b"a".as_slice(), b"z".as_slice()]
    );
    assert_eq!(overflow[0].encoded, b"m");
}

#[test]
fn zero_limit_preserves_an_empty_topn_for_nonempty_candidates() {
    let top_ns = vec![crate::TopN {
        values: vec![crate::TopNMeta {
            encoded: b"a".to_vec(),
            count: 1,
        }],
    }];
    let mut histograms = vec![crate::Histogram::default()];

    let (top, overflow) = crate::merge_partition_top_n(
        &top_ns,
        0,
        &mut histograms,
        2,
        &std::sync::atomic::AtomicBool::new(false),
    )
    .unwrap();

    assert!(top.is_some());
    assert!(top.unwrap().values.is_empty());
    assert_eq!(overflow.len(), 1);
}
