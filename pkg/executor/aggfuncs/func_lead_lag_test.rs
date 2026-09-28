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
// 块注释内保留 Go `TestLeadLag` / `TestMemLeadLag` 的用例矩阵与内存增量校验语义；
// 可执行部分直接驱动 `Lead`/`Lag`：验证 offset 越界时使用当前行 default 表达式的取值。
// 窗口函数（window function）按分区（partition）内行序取值，不改变结果集行数。

/*
//

// TestLeadLag 对应 Go 测试：覆盖 LAG/LEAD offset 与 default 参数的结果矩阵。
#[test]
pub fn test_lead_lag() {
    let zero = expression::NewZero();
    let one = expression::NewOne();
    let two = expression::Constant {
        Value: types::NewDatum(2),
        RetType: types::NewFieldType(mysql::TypeTiny),
    };
    let three = expression::Constant {
        Value: types::NewDatum(3),
        RetType: types::NewFieldType(mysql::TypeTiny),
    };
    let million = expression::Constant {
        Value: types::NewDatum(1_000_000),
        RetType: types::NewFieldType(mysql::TypeLong),
    };
    let default_arg = expression::Column {
        RetType: types::NewFieldType(mysql::TypeLonglong),
        Index: 0,
    };

    let num_rows = 3;
    let tests = vec![
        // lag(field0, N)：越界位置返回 NULL；offset 为 0 时返回当前行。
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![zero.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![one.clone()],
            0,
            num_rows,
            Nil,
            0,
            1,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![two.clone()],
            0,
            num_rows,
            Nil,
            Nil,
            0,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![three.clone()],
            0,
            num_rows,
            Nil,
            Nil,
            Nil,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![million.clone()],
            0,
            num_rows,
            Nil,
            Nil,
            Nil,
        ),
        // lag(field0, N, 1000000)：越界位置使用常量 default。
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![zero.clone(), million.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![one.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            0,
            1,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![two.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            1_000_000,
            0,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![three.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            1_000_000,
            1_000_000,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![million.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            1_000_000,
            1_000_000,
        ),
        // lag(field0, N, field0)：越界 default 来自当前行 field0。
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![zero.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![one.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            0,
            1,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![two.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            0,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![three.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![million.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        // lead(field0, N)：向后取值，越界位置返回 NULL。
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![zero.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![one.clone()],
            0,
            num_rows,
            1,
            2,
            Nil,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![two.clone()],
            0,
            num_rows,
            2,
            Nil,
            Nil,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![three.clone()],
            0,
            num_rows,
            Nil,
            Nil,
            Nil,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![million.clone()],
            0,
            num_rows,
            Nil,
            Nil,
            Nil,
        ),
        // lead(field0, N, 1000000)：向后越界使用常量 default。
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![zero.clone(), million.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![one.clone(), million.clone()],
            0,
            num_rows,
            1,
            2,
            1_000_000,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![two.clone(), million.clone()],
            0,
            num_rows,
            2,
            1_000_000,
            1_000_000,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![three.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            1_000_000,
            1_000_000,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![million.clone(), million.clone()],
            0,
            num_rows,
            1_000_000,
            1_000_000,
            1_000_000,
        ),
        // lead(field0, N, field0)：向后越界 default 来自当前行 field0。
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![zero.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![one.clone(), default_arg.clone()],
            0,
            num_rows,
            1,
            2,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![two.clone(), default_arg.clone()],
            0,
            num_rows,
            2,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![three.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
        buildWindowTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![million.clone(), default_arg.clone()],
            0,
            num_rows,
            0,
            1,
            2,
        ),
    ];

    for test in tests {
        testWindowFunc(test);
    }
}

// TestMemLeadLag 对应 Go 测试：LAG/LEAD 的 partial result 内存大小在各 offset 下保持一致。
#[test]
pub fn test_mem_lead_lag() {
    let zero = expression::NewZero();
    let one = expression::NewOne();
    let two = expression::Constant {
        Value: types::NewDatum(2),
        RetType: types::NewFieldType(mysql::TypeTiny),
    };
    let three = expression::Constant {
        Value: types::NewDatum(3),
        RetType: types::NewFieldType(mysql::TypeTiny),
    };
    let million = expression::Constant {
        Value: types::NewDatum(1_000_000),
        RetType: types::NewFieldType(mysql::TypeLong),
    };

    let num_rows = 3;
    let tests = vec![
        // lag(field0, N)
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![zero.clone()],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![one.clone()],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![two.clone()],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![three.clone()],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLag,
            mysql::TypeLonglong,
            vec![million.clone()],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        // lead(field0, N)
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![zero],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![one],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![two],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![three],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
        buildWindowMemTesterWithArgs(
            ast::WindowFuncLead,
            mysql::TypeLonglong,
            vec![million],
            0,
            num_rows,
            aggfuncs::DefPartialResult4LeadLagSize,
            rowMemDeltaGens,
        ),
    ];

    for test in tests {
        testWindowAggMemFunc(test);
    }
}
*/

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
