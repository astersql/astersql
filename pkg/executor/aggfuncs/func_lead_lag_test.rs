// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// LEAD/LAG 窗口函数测试。
//
// 测试直接驱动 `Lead`/`Lag`：验证 offset 越界时使用当前行 default 表达式的取值。
// 窗口函数（window function）按分区（partition）内行序取值，不改变结果集行数。


use crate::func_lead_lag::{Lag, Lead, LeadLagRow};
use crate::func_rank::DEF_ROW_SIZE;

fn collect_lead(offset: u64, rows: &[LeadLagRow<i64>]) -> Vec<Option<i64>> {
    let mut lead = Lead::new(offset);
    lead.update(rows.iter().cloned());
    (0..rows.len())
        .map(|_| lead.next_value().expect("one result per input row"))
        .collect()
}

fn collect_lag(offset: u64, rows: &[LeadLagRow<i64>]) -> Vec<Option<i64>> {
    let mut lag = Lag::new(offset);
    lag.update(rows.iter().cloned());
    (0..rows.len())
        .map(|_| lag.next_value().expect("one result per input row"))
        .collect()
}

#[test]
fn lead_and_lag_match_the_go_offset_and_default_matrix() {
    let values = [Some(0), Some(1), Some(2)];

    let null_default: Vec<_> = values
        .iter()
        .copied()
        .map(|value| LeadLagRow::new(value, None))
        .collect();
    let constant_default: Vec<_> = values
        .iter()
        .copied()
        .map(|value| LeadLagRow::new(value, Some(1_000_000)))
        .collect();
    let current_row_default: Vec<_> = values
        .iter()
        .copied()
        .map(|value| LeadLagRow::new(value, value))
        .collect();

    let offsets_and_null_results = [
        (0, vec![Some(0), Some(1), Some(2)]),
        (1, vec![None, Some(0), Some(1)]),
        (2, vec![None, None, Some(0)]),
        (3, vec![None, None, None]),
        (1_000_000, vec![None, None, None]),
    ];
    for (offset, expected_lag) in offsets_and_null_results {
        assert_eq!(collect_lag(offset, &null_default), expected_lag);
    }
    let offsets_and_null_results = [
        (0, vec![Some(0), Some(1), Some(2)]),
        (1, vec![Some(1), Some(2), None]),
        (2, vec![Some(2), None, None]),
        (3, vec![None, None, None]),
        (1_000_000, vec![None, None, None]),
    ];
    for (offset, expected_lead) in offsets_and_null_results {
        assert_eq!(collect_lead(offset, &null_default), expected_lead);
    }

    assert_eq!(
        collect_lag(2, &constant_default),
        vec![Some(1_000_000), Some(1_000_000), Some(0)]
    );
    assert_eq!(
        collect_lead(2, &constant_default),
        vec![Some(2), Some(1_000_000), Some(1_000_000)]
    );
    assert_eq!(
        collect_lag(2, &current_row_default),
        vec![Some(0), Some(1), Some(0)]
    );
    assert_eq!(
        collect_lead(2, &current_row_default),
        vec![Some(2), Some(1), Some(2)]
    );
}

#[test]
fn lead_and_lag_memory_accounting_reset_and_exhaustion_match_go() {
    let rows = [
        LeadLagRow::new(Some(10), None),
        LeadLagRow::new(None, Some(-2)),
        LeadLagRow::new(Some(30), None),
    ];

    let mut lead = Lead::new(1);
    assert_eq!(lead.update(rows[..1].iter().cloned()), DEF_ROW_SIZE);
    assert_eq!(lead.update(rows[1..].iter().cloned()), 2 * DEF_ROW_SIZE);
    assert_eq!(lead.next_value(), Some(None));
    assert_eq!(lead.next_value(), Some(Some(30)));
    assert_eq!(lead.next_value(), Some(None));
    assert_eq!(lead.next_value(), None);

    lead.reset();
    assert_eq!(lead.next_value(), None);
    assert_eq!(lead.update(rows.iter().cloned()), 3 * DEF_ROW_SIZE);
    assert_eq!(lead.next_value(), Some(None));

    let mut lag = Lag::new(1);
    assert_eq!(lag.update(rows.iter().cloned()), 3 * DEF_ROW_SIZE);
    assert_eq!(lag.next_value(), Some(None));
    lag.reset();
    assert_eq!(lag.next_value(), None);
}

/// 验证 LEAD/LAG 在 offset 指向分区外时，回退到当前行的 default 表达式结果。
///
/// 三行输入：值分别为 10/20/NULL，default 为 -1/-2/-3；offset=1。
/// LEAD：向前看 1 行，末行越界取 default=-3；中间行取到 NULL。
/// LAG：向后看 1 行，首行越界取 default=-1。
#[test]
fn lead_and_lag_use_current_row_default_outside_the_partition() {
    // 每行同时携带 value 与 default，模拟 TiDB 在当前行求值 default 表达式。
    let rows = [
        LeadLagRow::new(Some(10), Some(-1)),
        LeadLagRow::new(Some(20), Some(-2)),
        LeadLagRow::new(None, Some(-3)),
    ];
    // LEAD(offset=1)：第 0 行→20，第 1 行→NULL，第 2 行越界→default(-3)。
    let mut lead = Lead::new(1);
    lead.update(rows.clone());
    assert_eq!(lead.next_value(), Some(Some(20)));
    assert_eq!(lead.next_value(), Some(None));
    assert_eq!(lead.next_value(), Some(Some(-3)));

    // LAG(offset=1)：第 0 行越界→default(-1)，第 1 行→10，第 2 行→20。
    let mut lag = Lag::new(1);
    lag.update(rows);
    assert_eq!(lag.next_value(), Some(Some(-1)));
    assert_eq!(lag.next_value(), Some(Some(10)));
    assert_eq!(lag.next_value(), Some(Some(20)));
}
