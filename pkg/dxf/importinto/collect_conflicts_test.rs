// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// collect-conflicts 步骤的单元测试与 Go 测试草稿存档。
//
// `_GO_COLLECT_CONFLICTS_TEST_DRAFT` 保留从 Go 机械迁移的端到端测试草稿；
// 可执行部分覆盖冲突结果回写以及文件名前缀的路径布局断言。

const _GO_COLLECT_CONFLICTS_TEST_DRAFT: &str = r###"
// 这里主要描述 collect-conflicts step 如何消费冲突 KV 元信息、回写 checksum 与冲突行文件信息。

// calcExpectedCollectConflictsChecksum 对应 Go 辅助函数：按测试 fixture 中的三行重复冲突数据
// 重新编码 KV，并汇总成 importinto.Checksum 供 collect-conflicts step 断言。
fn calc_expected_collect_conflicts_checksum(
    t: &mut testing::T,
    hdl_ctx: &conflictedKVHandleContext,
) -> importinto::Checksum {
    t.Helper();

    // Go 通过 EncodingConfig + LoadDataController 构造 dup-resolve 专用 encoder；
    // 只保留配置字段和调用顺序，不实际接线 importer。
    let encode_cfg = encode::EncodingConfig {
        Table: hdl_ctx.tbl.clone(),
        UseIdentityAutoRowID: true,
        ..Default::default()
    };
    let controller = importer::LoadDataController {
        ASTArgs: importer::ASTArgs {},
        Plan: importer::Plan {},
        Table: hdl_ctx.tbl.clone(),
        ..Default::default()
    };
    let local_encoder = importer::NewTableKVEncoderForDupResolve(&encode_cfg, &controller)
        .expect("Go require.NoError(t, err)");

    let mut sum = verification::NewKVChecksumWithKeyspace(hdl_ctx.store.GetCodec().GetKeyspace());
    for i in 0..3 {
        let dup_id = i + 1;
        let row = vec![
            types::NewDatum(dup_id),
            types::NewDatum(dup_id),
            types::NewDatum(dup_id),
        ];
        let dup_pairs = local_encoder
            .Encode(row, dup_id as i64)
            .expect("Go require.NoError(t, err2)");

        // Go 中每组编码出的 pairs 被重复 Update 三次，用来模拟 9 行冲突统计。
        for _ in 0..3 {
            sum.Update(dup_pairs.Pairs.clone());
        }
    }

    importinto::Checksum {
        Sum: sum.Sum(),
        KVs: sum.SumKVS(),
        Size: sum.SumSize(),
    }
}

// TestCollectConflictsStepExecutor 对应 Go 测试：验证 step executor 会把冲突 KV 元信息
// 处理成 checksum、冲突行数量和冲突行文件名列表。
#[test]
fn test_collect_conflicts_step_executor() {
    let mut t = testing::T::new();
    let hdl_ctx = prepareConflictedKVHandleContext(&mut t);
    let st_meta = importinto::CollectConflictsStepMeta {
        Infos: hdl_ctx.conflictedKVInfo.clone(),
        ..Default::default()
    };
    let bytes = json::Marshal(&st_meta).expect("Go require.NoError(t, err)");
    let mut st = proto::Subtask { Meta: bytes, ..Default::default() };

    let mut step_exe = importinto::NewCollectConflictsStepExecutor(
        &proto::TaskBase { RequiredSlots: 1, ..Default::default() },
        hdl_ctx.store.clone(),
        hdl_ctx.taskMeta.clone(),
        hdl_ctx.logger.clone(),
    );
    // runConflictedKVHandleStep 来自 conflict_resolution_test.go；这里保留跨测试辅助调用关系。
    runConflictedKVHandleStep(&mut t, &mut st, &mut step_exe);

    let mut out_st_meta = importinto::CollectConflictsStepMeta::default();
    json::Unmarshal(st.Meta.clone(), &mut out_st_meta).expect("Go require.NoError(t, json.Unmarshal)");
    let expected_sum = calc_expected_collect_conflicts_checksum(&mut t, &hdl_ctx);

    assert_eq!(expected_sum, out_st_meta.Checksum);
    assert_eq!(9, out_st_meta.ConflictedRowCount);
    // Go 注释指出并发运行会让输出文件数变化，因此只断言范围。
    assert!(out_st_meta.ConflictedRowFilenames.len() >= 2);
    assert!(out_st_meta.ConflictedRowFilenames.len() <= 9);
    assert!(!out_st_meta.ConflictedRowRecordingCapped);
    assert!(!out_st_meta.TooManyConflictsFromIndex);
}

// TestCollectConflictsStepExecutorFilesTruncated 对应 Go 测试：通过 failpoint 强制单线程
// 和很小的总文件大小限制，确认冲突行记录被截断并设置 capped 标记。
#[test]
fn test_collect_conflicts_step_executor_files_truncated() {
    let mut t = testing::T::new();
    let hdl_ctx = prepareConflictedKVHandleContext(&mut t);
    let st_meta = importinto::CollectConflictsStepMeta {
        Infos: hdl_ctx.conflictedKVInfo.clone(),
        ..Default::default()
    };
    let bytes = json::Marshal(&st_meta).expect("Go require.NoError(t, err)");
    let mut st = proto::Subtask { Meta: bytes, ..Default::default() };

    // 两个 failpoint 对应 Go 里的并发控制和 maxTotalConflictRowFileSize 覆盖。
    testfailpoint::Enable(
        &mut t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/forceHandleConflictsBySingleThread",
        "return(true)",
    );
    testfailpoint::EnableCall(
        &mut t,
        "github.com/pingcap/tidb/pkg/dxf/importinto/conflictedkv/mockTotalConflictRowFileSizeLimit",
        |limit_p: &mut i64| {
            *limit_p = 32;
        },
    );

    let mut step_exe = importinto::NewCollectConflictsStepExecutor(
        &proto::TaskBase { RequiredSlots: 1, ..Default::default() },
        hdl_ctx.store.clone(),
        hdl_ctx.taskMeta.clone(),
        hdl_ctx.logger.clone(),
    );
    runConflictedKVHandleStep(&mut t, &mut st, &mut step_exe);

    let mut out_st_meta = importinto::CollectConflictsStepMeta::default();
    json::Unmarshal(st.Meta.clone(), &mut out_st_meta).expect("Go require.NoError(t, json.Unmarshal)");
    assert!(out_st_meta.ConflictedRowRecordingCapped);
    assert!(out_st_meta.ConflictedRowCount > 0);
    assert!(!out_st_meta.ConflictedRowFilenames.is_empty());
}
"###;

use astersql_dxf_importinto_conflictedkv::NewCollectResult;
use astersql_lightning_verification::MakeKVChecksum;

use crate::collect_conflicts::applyCollectResult;
use crate::{CollectConflictsStepMeta, getConflictRowFilenamePrefix};

/// Go `onFinished` persists all five aggregate outputs; test the whole wire
/// state transition instead of leaving the corresponding Go assertions only
/// in the migration draft above.
#[test]
fn collect_result_is_fully_persisted_to_subtask_meta() {
    let mut result = NewCollectResult(b"keyspace");
    result.RowCount = 9;
    result.TotalFileSize = 123;
    result.RowRecordingCapped = true;
    result.Checksum = MakeKVChecksum(456, 9, 789);
    result.Filenames = vec!["conflicted-rows/1/a".into(), "conflicted-rows/1/b".into()];

    let mut meta = CollectConflictsStepMeta::default();
    applyCollectResult(&mut meta, &result, true);

    let checksum = meta.Checksum.expect("Go always writes a checksum");
    assert_eq!(checksum.Sum, 789);
    assert_eq!(checksum.KVs, 9);
    assert_eq!(checksum.Size, 456);
    assert_eq!(meta.ConflictedRowCount, 9);
    assert_eq!(meta.ConflictedRowFilenames, result.Filenames);
    assert!(meta.ConflictedRowRecordingCapped);
    assert!(meta.TooManyConflictsFromIndex);
}

/// 冲突行文件前缀必须落在 `conflicted-rows/` 下，避免随任务目录一并被清理。
#[test]
fn conflict_row_prefix_survives_task_directory_cleanup() {
    assert_eq!(
        getConflictRowFilenamePrefix(12, 34, "abcd"),
        "conflicted-rows/12/34-abcd"
    );
}
