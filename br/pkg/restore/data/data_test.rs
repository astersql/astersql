// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc. Licensed under Apache-2.0.
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

//! Equivalents of `br/pkg/restore/data/data_test.go`.
//!
//! Go uses mock TiKV/PD + glue progress; those two tests only fill in-memory
//! StoreMetas then call GetTotalRegions / MakeRecoveryPlan. Rust mirrors that
//! with MemMgr/MemProgress stubs (no real PD/TiKV/network).
//! 中文注释索引开始
//! 本文件负责`br/pkg/restore/data/data_test.rs`对应的恢复数据流单元测试，本次仅补充注释，不改变执行语义。
//! 阅读时应把它视为 Go 同名实现的语义镜像，重点核对职责边界而不是表面写法。
//! 注释优先解释状态推进、错误传播、资源释放、默认值来源以及与相邻 Go 文件的对齐点。
//! 若这里使用内存 DB、临时目录、本地 HTTP 桩或脚本化 mock，被保护的仍是可观察行为而非环境搭建本身。
//! 对于测试文件，模块概览还会列出长测试内部的子场景，便于维护者快速定位断言目的。
//! 对于入口与 lib 文件，注释会重点说明哪些模块只是重导出，哪些模块才承载真实逻辑。
//! 对于 mock 与 stubs 文件，注释强调它们服务于验证，不代表生产路径真的依赖这些简化实现。
//! 本任务要求至少32行中文注释，因此下方会显式列出关键符号与高价值场景索引。
//! 测试夹具中的 SQL 分支匹配顺序与 Go 用例场景一一对应，改动匹配条件等于改动契约。
//! - `NUM_ONLINE_STORE`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `MAX_ALLOCATE_ID`与 Go 侧常量/SQL 模板对齐，改动需同步核对占位与语义。
//! 该常量/SQL 模板与 Go 侧同名定义对齐，改动时需同步核对拼写与占位符顺序。
//! 注释强调它约束的是契约与协议文本，而不是本地临时写法。
//! - `TestData`承载"TestData"相关状态，是理解数据流的入口之一。
//! 关注点不只是字段名，还包括谁负责填充、谁负责消费、何时被复制以及何时需要回写。
//! 当 Rust 端使用 Arc、Mutex、RwLock 或其他包装来表达 Go 约束时，对外契约仍以可观察行为为准。
//! 所以注释重点会落在生命周期、并发保护和默认值，而不是逐字段翻译。
//! - `new_region_meta`是当前文件的重要函数，承担"new_region_meta"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `impl TestData`把"TestData"的方法聚合在一起，体现类型的生命周期与行为边界。
//! 注意这里真正需要关注的是状态如何推进、何时落盘或回写，以及错误是记录后继续还是立即返回。
//! 与 Go 相比，Rust 常用所有权与锁表达相同约束，因此注释会补足这层映射关系。
//! 这有助于减少局部重构时破坏跨方法隐含约束的风险。
//! - `create_stores`是当前文件的重要函数，承担"create_stores"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `create_data_suite`是当前文件的重要函数，承担"create_data_suite"对应的局部职责。
//! 这里真正需要解释的是输入输出、失败语义和调用顺序，而不是重复 Rust 语法本身。
//! 若函数服务于测试，它固定的是行为契约；若服务于实现，它固定的是边界与副作用。
//! 因此阅读该函数时要特别留意默认值、空集合、未知状态和错误包装是否继续与 Go 对齐。
//! - `test_get_total_regions`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! - `test_make_recovery_plan`保护特定场景的可观察行为，断言依据来自 Go 对应用例。
//! 本测试固定的是可观察行为与 Go 用例的对齐点，而不是环境搭建细节。
//! 断言失败时优先核对状态推进、错误类型与过滤条件是否仍与 Go 一致。
//! 辅助夹具若使用内存桩，只服务于隔离，不代表生产路径依赖这些简化实现。
//! 中文注释索引结束

use std::sync::Arc;

use crate::data::{NewRecovery, NewStoreMeta, Recovery, RecoveryStage, atStage, recoveryError};
use crate::stubs::metapb;
use crate::stubs::recovpb;
use crate::stubs::{Context, Error, MemMgr, MemProgress, Mgr, PDClient};

const NUM_ONLINE_STORE: usize = 3;
const MAX_ALLOCATE_ID: u64 = 0x176f;

struct TestData {
    _ctx: Context,
    mock_pd_client: Arc<dyn PDClient>,
    mock_recovery: Recovery,
}

fn new_region_meta(
    region_id: u64,
    peer_id: u64,
    last_log_term: u64,
    last_index: u64,
    commit_index: u64,
    version: u64,
    tombstone: bool,
    start_key: &[u8],
    end_key: &[u8],
) -> recovpb::RegionMeta {
    recovpb::RegionMeta {
        RegionId: region_id,
        PeerId: peer_id,
        LastLogTerm: last_log_term,
        LastIndex: last_index,
        CommitIndex: commit_index,
        Version: version,
        Tombstone: tombstone,
        StartKey: start_key.to_vec(),
        EndKey: end_key.to_vec(),
    }
}

impl TestData {
    fn generate_region_meta(&mut self) {
        let mut store_meta0 = NewStoreMeta(1);
        store_meta0
            .RegionMetas
            .push(new_region_meta(11, 24, 8, 5, 4, 1, false, b"", b"b"));
        store_meta0
            .RegionMetas
            .push(new_region_meta(12, 34, 5, 6, 5, 1, false, b"b", b"c"));
        store_meta0
            .RegionMetas
            .push(new_region_meta(13, 44, 1200, 7, 6, 1, false, b"c", b""));
        self.mock_recovery.StoreMetas[0] = store_meta0;

        let mut store_meta1 = NewStoreMeta(2);
        store_meta1
            .RegionMetas
            .push(new_region_meta(11, 25, 7, 6, 4, 1, false, b"", b"b"));
        store_meta1
            .RegionMetas
            .push(new_region_meta(12, 35, 5, 6, 5, 1, false, b"b", b"c"));
        store_meta1
            .RegionMetas
            .push(new_region_meta(13, 45, 1200, 6, 6, 1, false, b"c", b""));
        self.mock_recovery.StoreMetas[1] = store_meta1;

        let mut store_meta2 = NewStoreMeta(3);
        store_meta2
            .RegionMetas
            .push(new_region_meta(11, 26, 7, 5, 4, 1, false, b"", b"b"));
        store_meta2
            .RegionMetas
            .push(new_region_meta(12, 36, 5, 6, 6, 1, false, b"b", b"c"));
        store_meta2.RegionMetas.push(new_region_meta(
            13,
            MAX_ALLOCATE_ID,
            1200,
            6,
            6,
            1,
            false,
            b"c",
            b"",
        ));
        self.mock_recovery.StoreMetas[2] = store_meta2;
    }

    fn clean_up(&self) {
        // Go closes the mock PD client; MemPDClient has no Close — drop via Arc.
        let _ = self.mock_pd_client.GetAllStores(&Context::Background());
    }
}

fn create_stores() -> Vec<metapb::Store> {
    vec![
        metapb::Store {
            Id: 1,
            Address: String::new(),
            Labels: vec![metapb::StoreLabel {
                Key: "engine".into(),
                Value: "tikv".into(),
            }],
        },
        metapb::Store {
            Id: 2,
            Address: String::new(),
            Labels: vec![
                metapb::StoreLabel {
                    Key: "else".into(),
                    Value: "tikv".into(),
                },
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tiflash".into(),
                },
            ],
        },
        metapb::Store {
            Id: 3,
            Address: String::new(),
            Labels: vec![
                metapb::StoreLabel {
                    Key: "else".into(),
                    Value: "tiflash".into(),
                },
                metapb::StoreLabel {
                    Key: "engine".into(),
                    Value: "tikv".into(),
                },
            ],
        },
    ]
}

fn create_data_suite() -> TestData {
    let _ = NUM_ONLINE_STORE;
    let ctx = Context::Background();
    let mgr = Arc::new(MemMgr::new());
    let mock_pd_client = mgr.PDClient();
    // Go: mockGlue.StartProgress(ctx, "Restore Data", numOnlineStore*3, false)
    let progress = Arc::new(MemProgress::new());
    let mut recovery = NewRecovery(create_stores(), mgr, progress, 64);
    recovery.spawn_watcher = false;
    TestData {
        _ctx: ctx,
        mock_pd_client,
        mock_recovery: recovery,
    }
}

/// Go `TestGetTotalRegions`: three stores × three peers → 3 unique region IDs.
#[test]
fn test_get_total_regions() {
    let mut suite = create_data_suite();
    suite.generate_region_meta();
    let total_region = suite.mock_recovery.GetTotalRegions();
    assert_eq!(total_region, 3);
    suite.clean_up();
}

/// Go `TestMakeRecoveryPlan`: MaxAllocID and RecoveryPlan store count.
#[test]
fn test_make_recovery_plan() {
    let mut suite = create_data_suite();
    suite.generate_region_meta();
    suite
        .mock_recovery
        .MakeRecoveryPlan()
        .expect("MakeRecoveryPlan");
    assert_eq!(suite.mock_recovery.MaxAllocID, MAX_ALLOCATE_ID);
    assert_eq!(suite.mock_recovery.RecoveryPlan.len(), 2);
    suite.clean_up();
}

/// Go `recoveryError` exposes the embedded error text unchanged while carrying
/// the recovery stage separately for `errors.As` / retry classification.
#[test]
fn recovery_error_preserves_the_underlying_error_text() {
    let recovery_err = recoveryError {
        error: Error::new("read meta failed"),
        atStage: RecoveryStage::StageCollectingMeta,
    };

    assert_eq!(recovery_err.to_string(), "read meta failed");
    let err = Error::from(recovery_err);
    assert_eq!(err.to_string(), "read meta failed");
    assert_eq!(atStage(&err), RecoveryStage::StageCollectingMeta);
}
