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

//! Go-equivalent tests for `precheck_test.go`.
//!
//! 该文件只验证 precheck builder 的“接线正确性”：
//! 给定一组基础依赖后，是否能为每个 `CheckItemID` 构造出对应 checker，
//! 并让返回对象报告正确的自身 ID。
//! 它相当于整个 precheck 接线层的一次轻量冒烟。

use crate::*;
use astersql_lightning_pkg_precheck as precheck;

/// Corresponds to Go `TestPrecheckBuilderBasic`.
///
/// Builds via `NewPreImportInfoGetter` + `NewPrecheckItemBuilder` +
/// `NewTargetInfoGetterImpl` (same wiring as `parity_test.rs`). Uses empty
/// `dbMetas` and `file:///` storage — no mock crate.
/// 因而这里不关注具体检查结果，只关心 builder 分发是否完整。
#[test]
fn test_precheck_builder_basic() {
    let mut cfg = config::Config::NewConfig();
    cfg.TikvImporter.Backend = config::BackendLocal.into();

    let target = NewTargetInfoGetterImpl(&cfg, sql::DB::new_memory(), None)
        .expect("NewTargetInfoGetterImpl");
    // 先组装一条最小可运行的 pre-info getter 链路。
    let pre_info_getter = NewPreImportInfoGetter(
        &cfg,
        vec![],
        storeapi::Storage::new("file:///"),
        target,
        None,
        None,
        vec![],
    )
    .expect("NewPreImportInfoGetter");

    let check_builder = NewPrecheckItemBuilder(&cfg, vec![], pre_info_getter, None, None, None);
    // 枚举这里支持的检查项，逐个确认 builder 能返回正确类型。
    // 只要某个枚举分支漏接，循环中的对应断言就会立刻失败。

    let check_item_ids = [
        precheck::CheckLargeDataFile,
        precheck::CheckSourcePermission,
        precheck::CheckTargetTableEmpty,
        precheck::CheckSourceSchemaValid,
        precheck::CheckCheckpoints,
        precheck::CheckCSVHeader,
        precheck::CheckTargetClusterSize,
        precheck::CheckTargetClusterEmptyRegion,
        precheck::CheckTargetClusterRegionDist,
        precheck::CheckTargetClusterVersion,
        precheck::CheckLocalDiskPlacement,
        precheck::CheckLocalTempKVDir,
    ];

    for check_item_id in check_item_ids {
        // 断言对象自身报告的 ID 与请求 ID 一致，证明分发没有串线。
        let checker = check_builder
            .BuildPrecheckItem(check_item_id)
            .expect("BuildPrecheckItem");
        assert_eq!(
            checker.GetCheckItemID(),
            check_item_id,
            "checker id mismatch for {check_item_id}"
        );
    }
}

/// Go `NewPrecheckItemBuilderFromConfig` opens the configured checkpoint
/// backend and propagates an unsupported-driver error instead of silently
/// replacing it with a null checkpoint database.
#[test]
fn test_precheck_builder_from_config_rejects_unknown_checkpoint_driver() {
    let mut cfg = config::Config::NewConfig();
    cfg.Checkpoint.Enable = true;
    cfg.Checkpoint.Driver = "unsupported".into();

    let err = NewPrecheckItemBuilderFromConfig(context::Background(), &cfg, None, vec![])
        .err()
        .expect("unknown checkpoint driver must fail");

    assert!(
        err.to_string().contains("unsupported"),
        "unexpected checkpoint error: {err}"
    );
}
