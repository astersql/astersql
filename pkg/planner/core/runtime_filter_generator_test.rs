// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// Runtime Filter 生成器单元测试。
//
// 覆盖 HashJoin→TableScan 过滤器生成、TiFlash Global 模式，以及连接类型/
// Fragment 归属判断与 Go 侧语义对齐。

use crate::{JoinType, PlanKind, PlanNode, RuntimeFilterGenerator, RuntimeFilterMode, StoreType};

/// 构造指定 ID、表名与存储类型的 TableScan 计划节点。
fn scan(id: i32, table: &str, store: StoreType) -> PlanNode {
    let mut plan = PlanNode::New(
        id,
        PlanKind::TableScan {
            table: table.to_owned(),
        },
        Vec::new(),
    );
    plan.store_type = store;
    plan
}

/// 验证 TiFlash HashJoin 仅为 probe Scan 生成 Local Runtime Filter。
#[test]
fn runtime_filter_generator_tracks_join_scan_and_mode() {
    let mut root = PlanNode::New(
        10,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("build.a".to_owned(), "probe.a".to_owned())],
        },
        vec![
            scan(11, "probe", StoreType::TiFlash),
            scan(12, "build", StoreType::TiFlash),
        ],
    );
    root.store_type = StoreType::TiFlash;
    let mut generator = RuntimeFilterGenerator::default();
    generator.GenerateRuntimeFilter(&root);
    assert_eq!(generator.filters.len(), 1);
    assert!(generator.filters.iter().all(|filter| {
        filter.source_join == 10
            && filter.mode == RuntimeFilterMode::Local
            && filter.build_expr == "build.a"
            && filter.probe_expr == "probe.a"
    }));
    assert_eq!(generator.filters[0].id, 0);
    assert_eq!(generator.filters[0].target_scan, 11);
}

/// 验证连接类型白名单与同 Fragment 判定与 Go 行为一致。
#[test]
fn runtime_filter_join_and_fragment_gates_match_go() {
    assert!(RuntimeFilterGenerator::matchRFJoinType(
        JoinType::InnerJoin,
        true
    ));
    assert!(RuntimeFilterGenerator::matchRFJoinType(
        JoinType::RightOuterJoin,
        true
    ));
    assert!(!RuntimeFilterGenerator::matchRFJoinType(
        JoinType::LeftOuterJoin,
        true
    ));
    assert!(RuntimeFilterGenerator::matchRFJoinType(
        JoinType::LeftOuterJoin,
        false
    ));
    assert!(!RuntimeFilterGenerator::matchRFJoinType(
        JoinType::RightOuterJoin,
        false
    ));
    assert!(RuntimeFilterGenerator::matchRFJoinType(
        JoinType::SemiJoin,
        true
    ));

    let target = scan(2, "b", StoreType::TiKV);
    let source = PlanNode::New(10, PlanKind::Projection, vec![target.clone()]);
    assert!(RuntimeFilterGenerator::belongsToSameFragment(
        &source, &target,
    ));

    let disconnected = scan(3, "c", StoreType::TiKV);
    assert!(!RuntimeFilterGenerator::belongsToSameFragment(
        &source,
        &disconnected,
    ));

    let exchange = PlanNode::New(
        11,
        PlanKind::ExchangeReceiver { task_ids: vec![] },
        vec![target.clone()],
    );
    assert!(!RuntimeFilterGenerator::belongsToSameFragment(
        &exchange, &target,
    ));
}

/// Go 仅在 TiFlash HashJoin 上构造 RF，并为每条等值条件分配连续 ID。
#[test]
fn runtime_filter_requires_tiflash_and_keeps_all_equal_conditions() {
    let tikv_join = PlanNode::New(
        20,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![("build.a".to_owned(), "probe.a".to_owned())],
        },
        vec![
            scan(21, "probe", StoreType::TiKV),
            scan(22, "build", StoreType::TiKV),
        ],
    );
    let mut generator = RuntimeFilterGenerator::default();
    generator.GenerateRuntimeFilter(&tikv_join);
    assert!(generator.filters.is_empty());

    let mut tiflash_join = PlanNode::New(
        30,
        PlanKind::HashJoin {
            inner_child: 1,
            equal_conditions: vec![
                ("build.a".to_owned(), "probe.a".to_owned()),
                ("build.b".to_owned(), "probe.b".to_owned()),
            ],
        },
        vec![
            scan(31, "probe", StoreType::TiFlash),
            scan(32, "build", StoreType::TiFlash),
        ],
    );
    tiflash_join.store_type = StoreType::TiFlash;
    generator.GenerateRuntimeFilter(&tiflash_join);
    assert_eq!(generator.filters.len(), 2);
    assert_eq!(generator.filters[0].id, 0);
    assert_eq!(generator.filters[1].id, 1);
    assert!(generator.filters.iter().all(|rf| rf.target_scan == 31));
}
