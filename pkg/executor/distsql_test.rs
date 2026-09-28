// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
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

// DistSQL（分布式 SQL 下推）辅助函数的单元测试。
//
// 覆盖批大小计算、多 range 归并排序判定，以及 IndexLookUp 路径上
// 「期望 handle 集合相对已获得集合」的差集规则。

use crate::distsql::{
    ByItem, CalculateBatchSize, Datum, GetLackHandles, Handle, getDatumRow, needMergeSort,
};
use std::collections::BTreeSet;

/// 批大小、merge-sort 判定与缺失 handle 差集与读路径语义一致。
#[test]
fn distsql_batch_merge_and_missing_handle_rules_match_read_path() {
    // estimated 较大时按 2 次幂增长；estimated 小于初始批大小时仍保留 initial。
    assert_eq!(CalculateBatchSize(100, 32, 64), 64);
    assert_eq!(CalculateBatchSize(2, 32, 64), 32);
    // 有排序项且 range 数 > 1 才需要 merge sort。
    assert!(needMergeSort(
        &[ByItem {
            offset: 0,
            descending: false
        }],
        2
    ));
    assert!(!needMergeSort(&[], 2));
    let mut obtained = BTreeSet::from([Handle::Int(2)]);
    assert_eq!(
        GetLackHandles(&[Handle::Int(1), Handle::Int(2)], &mut obtained),
        vec![Handle::Int(1)]
    );
    assert!(obtained.is_empty());
}

/// Go consumes each obtained handle once, so duplicate index handles remain visible as missing.
#[test]
fn get_lack_handles_consumes_obtained_handles_and_reports_duplicates() {
    let mut obtained = BTreeSet::from([Handle::Int(1), Handle::Int(2)]);
    assert_eq!(
        GetLackHandles(
            &[Handle::Int(1), Handle::Int(1), Handle::Int(2)],
            &mut obtained,
        ),
        vec![Handle::Int(1)]
    );
    assert!(obtained.is_empty());
}

/// Go copies only columns that have matching field metadata.
#[test]
fn get_datum_row_stops_at_the_available_field_metadata() {
    let row = vec![Datum::Signed(1), Datum::Signed(2), Datum::Signed(3)];
    assert_eq!(getDatumRow(&row, &[0, 1]), row[..2]);
}
