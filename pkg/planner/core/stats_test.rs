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

// 计划器统计信息推导相关单元测试。
//
// 覆盖 Selection/Limit/Join/Agg 的递归行数推导，以及 DataSource 过滤选择率
// 与访问路径属性汇总。

use crate::{
    DataSourceStats, PlanKind, PlanNode, RecursiveDeriveStats4Test, StatsAccessPath,
    deriveStatsByFilter, getGeneralAttributesFromPaths, pruneEstimateRange,
};

/// 验证过滤、Limit、聚合的行数推导链路。
#[test]
fn recursive_stats_derivation_covers_filter_limit_join_and_aggregation() {
    let mut scan = PlanNode::New(
        1,
        PlanKind::TableScan {
            table: "t".to_owned(),
        },
        Vec::new(),
    );
    scan.estimated_rows = 1_000.0;
    let selection = PlanNode::New(
        2,
        PlanKind::Selection {
            conditions: vec!["a>1".to_owned(), "b<9".to_owned()],
        },
        vec![scan],
    );
    let mut limit = PlanNode::New(
        3,
        PlanKind::Limit {
            offset: 10,
            count: 50,
        },
        vec![selection],
    );
    let (stats, changed) = RecursiveDeriveStats4Test(&mut limit);
    assert!(changed);
    assert_eq!(stats.row_count, 50.0);
    assert!((limit.children[0].estimated_rows - 640.0).abs() < 1e-9);

    let mut aggregate = PlanNode::New(4, PlanKind::HashAgg, vec![limit]);
    let (stats, _) = RecursiveDeriveStats4Test(&mut aggregate);
    assert!((stats.row_count - 50.0_f64.sqrt()).abs() < 1e-12);
}

/// 验证过滤选择率、路径最小行数与 range 截断行为。
#[test]
fn datasource_filter_and_path_attributes_use_real_access_paths() {
    let mut source = DataSourceStats {
        table_rows: 10_000.0,
        ..Default::default()
    };
    let stats = deriveStatsByFilter(&mut source, 3);
    assert!((stats.row_count - 5_120.0).abs() < 1e-9);
    assert!((source.selectivity - 0.512).abs() < 1e-12);

    let paths = vec![
        StatsAccessPath {
            count_after_access: 500.0,
            count_after_index: 400.0,
            table_path: true,
            ..Default::default()
        },
        StatsAccessPath {
            count_after_access: 250.0,
            count_after_index: 100.0,
            forced: true,
            ..Default::default()
        },
    ];
    assert_eq!(getGeneralAttributesFromPaths(&paths, 1_000.0), (0.1, true));
    assert_eq!(getGeneralAttributesFromPaths(&[], 1_000.0), (1.0, false));
    assert_eq!(getGeneralAttributesFromPaths(&paths, 0.0), (1.0, true));
    let index_merge = StatsAccessPath {
        partial_index_paths: vec![StatsAccessPath::default()],
        count_after_access: 200.0,
        count_after_index: 10.0,
        ..Default::default()
    };
    assert_eq!(
        getGeneralAttributesFromPaths(&[index_merge], 1_000.0),
        (0.2, false)
    );
    assert_eq!(
        pruneEstimateRange(&[vec!["a".into(), "b".into()], vec!["c".into()]], 1),
        vec![vec!["a".to_owned()], vec!["c".to_owned()]]
    );
}
