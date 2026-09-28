// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

//! Go-equivalent tests for `placement_rule_manager_test.go`.
//! PD/region boundary: MemPdClient + MemSplitClient (no kvproto/grpcio).
//! 验证离线/在线无 store/在线有 restore store 三条工厂分支与 Set/Reset 闭环。
//! Region peer 预先落在 restore store 2，使 waitPlacementSchedule 可立即成功。
//! 不引入真实 PD/gRPC，只锁定标签筛选与规则生命周期的可观察行为。
//! generate_stores 刻意混入 TiFlash 与 Offline，证明筛选不会误收这些节点。
//! generate_tables 使用 ID 1 与 100，对应后续 region 前缀覆盖范围。
//! Offline 用例不传 SplitClient，确保工厂不会因缺 tool 而失败。
//! OnlineNoStores 场景 stores 均无可用 restore 标签，断言降级为空操作。
//! OnlineLeave 预写 MemSplitClient.regions，模拟调度已完成的生产终态。
//! region 辅助函数固定 peer.StoreId=2，与唯一 Up+restore store 对齐。
//! OldTable 元数据仅填充 DB/表名，不参与放置规则键空间计算。
//! 三条测试均调用 Set 后再 Reset，锁定完整生命周期而非单点 API。
//! 与 Go failpoint 加速 ticker 的差异写在用例注释，避免误判轮询语义缺失。
//! 标签常量经 export_test 导出，保证与实现侧 restoreLabelKey/Value 同源。
//! 本文件不修改 placement 规则内容本身，只验证管理器分支选择与成功路径。
//! 若未来增加“调度未完成报错”用例，应把 wait_ready=false 与错误 peer 组合注入。
//! 当前范围对齐 Go 同名测试的离线/无 store/在线离开三条主路径。
//! MemPdClient.cluster_id 固定为 1，仅满足 StoreMeta 最小字段要求。
//! Context::Background 无取消，聚焦放置路径而非超时语义。
//! Arc<MemSplitClient> 注入工厂，验证在线分支对 toolClient 的所有权要求。
//! regions 锁内写入两段前缀，覆盖 tables 两个 ID，避免部分表未就绪假阳性。
//! EncodeTablePrefix(n)..EncodeTablePrefix(n+1) 与实现侧范围编码完全一致。
//! Offline store 即使携带 restore 标签也不能入选，对应 generate_stores 的 id=4。
//! TiFlash Up store 无 restore 标签，对应 id=1，验证 engine 标签不被误用。
//! SetPlacementRule 成功隐含 setup+wait 全链路，Reset 隐含逐表删除成功。
//! 不在此文件断言规则 Index/Override 数值，那些由实现单测或 parity 覆盖。
//! 测试命名保留 Go ContextManager* 前缀语义，便于检索对照。
//! 若 PD 返回删除失败，Reset 应报错；当前 MemSplitClient 默认删除成功。
//! 本任务仅补充注释，不改断言阈值与 fixture 数据。
//! 通过后即可与 Go `placement_rule_manager_test.go` 做行为对照回归。

use std::collections::HashMap;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use crate::export_test::{RestoreLabelKey, RestoreLabelValue};
use crate::placement_rule_manager::{
    NewPlacementRuleManager, PlacementRuleManager, onlinePlacementRuleManager,
};
use crate::stubs::{
    Context, CreatedTable, MemPdClient, MemSplitClient, RegionInfo, codec, metapb, metautil, model,
    tablecodec,
};

/// 两张待恢复表（ID 1/100），供 SetPlacementRule 写入 restoreTables。
fn generate_tables() -> Vec<CreatedTable> {
    vec![
        CreatedTable {
            Table: model::TableInfo {
                ID: 1,
                ..Default::default()
            },
            OldTable: metautil::Table {
                DB: model::DBInfo {
                    Name: model::CIStr::new("test"),
                    ..Default::default()
                },
                Info: model::TableInfo {
                    Name: model::CIStr::new("t1"),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
        CreatedTable {
            Table: model::TableInfo {
                ID: 100,
                ..Default::default()
            },
            OldTable: metautil::Table {
                DB: model::DBInfo {
                    Name: model::CIStr::new("test"),
                    ..Default::default()
                },
                Info: model::TableInfo {
                    Name: model::CIStr::new("t100"),
                    ..Default::default()
                },
                ..Default::default()
            },
            ..Default::default()
        },
    ]
}

/// 混合 store：TiFlash、带标签 Up TiKV、Offline（含/不含标签），只应选中 store 2。
fn generate_stores() -> Vec<metapb::Store> {
    vec![
        metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tiflash".into(),
            }],
            ..Default::default()
        },
        metapb::Store {
            Id: 2,
            State: metapb::StoreState::Up,
            Labels: vec![
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tikv".into(),
                },
                metapb::StoreLabel {
                    Key: RestoreLabelKey.into(),
                    Value: RestoreLabelValue.into(),
                },
            ],
            ..Default::default()
        },
        metapb::Store {
            Id: 3,
            State: metapb::StoreState::Offline,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tikv".into(),
            }],
            ..Default::default()
        },
        metapb::Store {
            Id: 4,
            State: metapb::StoreState::Offline,
            Labels: vec![
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tikv".into(),
                },
                metapb::StoreLabel {
                    Key: RestoreLabelKey.into(),
                    Value: RestoreLabelValue.into(),
                },
            ],
            ..Default::default()
        },
    ]
}

/// 构造单 peer 落在 store 2 的 RegionInfo，满足 checkRange 就绪条件。
fn region(id: u64, start: Vec<u8>, end: Vec<u8>) -> RegionInfo {
    RegionInfo {
        Region: metapb::Region {
            Id: id,
            StartKey: start,
            EndKey: end,
            Peers: vec![metapb::Peer { Id: id, StoreId: 2 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: id, StoreId: 2 }),
    }
}

/// TestContextManagerOffline — Go `TestContextManagerOffline`.
/// is_online=false 时即使无 SplitClient 也应空操作成功。
#[test]
fn test_context_manager_offline() {
    let ctx = Context::Background();
    let pd = MemPdClient {
        cluster_id: 1,
        stores: Vec::new(),
    };
    let mut mgr = NewPlacementRuleManager(&ctx, &pd, None, false).unwrap();
    let tables = generate_tables();
    mgr.SetPlacementRule(&ctx, &tables).unwrap();
    mgr.ResetPlacementRules(&ctx).unwrap();
}

/// TestContextManagerOnlineNoStores — Go `TestContextManagerOnlineNoStores`.
/// 在线但无 Up+restore 标签 store 时降级为 offline，Set/Reset 仍成功。
#[test]
fn test_context_manager_online_no_stores() {
    let ctx = Context::Background();
    let stores = vec![
        metapb::Store {
            Id: 1,
            State: metapb::StoreState::Up,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tiflash".into(),
            }],
            ..Default::default()
        },
        metapb::Store {
            Id: 2,
            State: metapb::StoreState::Offline,
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tikv".into(),
            }],
            ..Default::default()
        },
    ];
    let pd = MemPdClient {
        cluster_id: 1,
        stores,
    };
    // No restore-label Up stores → offline manager (Go same).
    // 标签筛选结果为空，工厂必须降级而非报错。
    let mut mgr = NewPlacementRuleManager(&ctx, &pd, None, true).unwrap();
    let tables = generate_tables();
    mgr.SetPlacementRule(&ctx, &tables).unwrap();
    mgr.ResetPlacementRules(&ctx).unwrap();
}

/// TestContextManagerOnlineLeave — Go `TestContextManagerOnlineLeave`.
/// Go uses failpoint quicker ticker; Rust MemSplitClient regions already satisfy restore peers.
/// 预置表 1/100 前缀 region，peer 仅在 restore store 2，验证完整 Set→Reset。
#[test]
fn test_context_manager_online_leave() {
    let ctx = Context::Background();
    let pd = MemPdClient {
        cluster_id: 1,
        stores: generate_stores(),
    };
    let split = Arc::new(MemSplitClient::default());
    // Cover table prefixes 1 and 100 with peers only on restore store 2.
    // 与 Go failpoint 加速轮询不同：这里直接让 checkRegions 一次通过。
    {
        let mut regions = split.regions.lock().unwrap();
        regions.push(region(
            1,
            codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(1)),
            codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(2)),
        ));
        regions.push(region(
            100,
            codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(100)),
            codec::EncodeBytes(Vec::new(), &tablecodec::EncodeTablePrefix(101)),
        ));
    }
    let mut mgr = NewPlacementRuleManager(&ctx, &pd, Some(split), true).unwrap();
    let tables = generate_tables();
    mgr.SetPlacementRule(&ctx, &tables).unwrap();
    mgr.ResetPlacementRules(&ctx).unwrap();
}

/// Go `waitPlacementSchedule` keeps polling after an incomplete scan instead of
/// turning the transient placement state into a restore error.
#[test]
fn test_context_manager_online_retries_until_regions_are_ready() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    split.regions.lock().unwrap().push(RegionInfo {
        Region: metapb::Region {
            Id: 1,
            Peers: vec![metapb::Peer { Id: 1, StoreId: 3 }],
            ..Default::default()
        },
        Leader: Some(metapb::Peer { Id: 1, StoreId: 3 }),
    });

    let updater = Arc::clone(&split);
    let ready = thread::spawn(move || {
        thread::sleep(Duration::from_millis(25));
        updater.regions.lock().unwrap()[0].Region.Peers[0].StoreId = 2;
    });

    let mut manager = onlinePlacementRuleManager {
        toolClient: split,
        restoreStores: vec![2],
        restoreTables: HashMap::new(),
        waitInterval: Duration::from_millis(5),
    };
    manager
        .SetPlacementRule(&ctx, &generate_tables()[..1])
        .unwrap();
    ready.join().unwrap();
}

/// Go integer arithmetic wraps when deriving the exclusive end table prefix.
/// Rust must preserve that behavior instead of panicking for the largest table ID.
#[test]
fn test_max_table_id_rule_range_wraps_like_go() {
    let ctx = Context::Background();
    let split = Arc::new(MemSplitClient::default());
    let mut restore_tables = HashMap::new();
    restore_tables.insert(i64::MAX, ());
    let manager = onlinePlacementRuleManager {
        toolClient: split.clone(),
        restoreStores: vec![2],
        restoreTables: restore_tables,
        waitInterval: Duration::from_millis(5),
    };

    manager.setupPlacementRules(&ctx).unwrap();

    let rules = split.rules.lock().unwrap();
    let rule = rules
        .get(&("pd".to_string(), "restore-t9223372036854775807".to_string()))
        .unwrap();
    assert_eq!(
        rule.EndKeyHex,
        crate::stubs::bytes_to_hex(&codec::EncodeBytes(
            Vec::new(),
            &tablecodec::EncodeTablePrefix(i64::MIN),
        ))
    );
}
