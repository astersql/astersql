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

// Import Into 协议类型（proto）相关单元测试与 Go 草稿存档。
//
// `_GO_PROTO_TEST_DRAFT` 保留从 Go 机械迁移的 `KVGroupConflictInfos` 冲突计数聚合测试；
// 可执行部分覆盖 data/index KV 组冲突计数合并，以及 `Checksum` 到 `KVChecksum` 的往返转换。
// KV 组（KV group）是按数据行与各索引划分的键值空间分区。

const _GO_PROTO_TEST_DRAFT: &str = r###"
// 这段逻辑只覆盖 KVGroupConflictInfos 的冲突计数聚合语义，不会调用真实 engineapi 或 global sort 组件。

// test_kv_group_conflict_infos_add_conflict_info 对应 Go 的 TestKVGroupConflictInfosAddConflictInfo。
#[test]
pub fn test_kv_group_conflict_infos_add_conflict_info() {
    let mut gci = KVGroupConflictInfos::default();

    // Count 为 0 时 Go 直接返回，不初始化 ConflictInfos map。
    gci.addDataConflictInfo(&engineapi::ConflictInfo { Count: 0, ..Default::default() });
    assert!(gci.ConflictInfos.is_none());

    // nil map 首次写入 data KV group 时应懒初始化，并保留传入 count。
    let info1 = engineapi::ConflictInfo { Count: 10, ..Default::default() };
    gci.addDataConflictInfo(&info1);
    assert!(gci.ConflictInfos.is_some());
    assert_eq!(gci.ConflictInfos.as_ref().unwrap().len(), 1);
    assert_eq!(gci.ConflictInfos.as_ref().unwrap()[globalsort::DataKVGroup].Count, 10);

    // 新 index group 使用 index id 字符串作为 key。
    let info2 = engineapi::ConflictInfo { Count: 20, ..Default::default() };
    gci.addIndexConflictInfo(1, &info2);
    assert_eq!(gci.ConflictInfos.as_ref().unwrap().len(), 2);
    assert_eq!(gci.ConflictInfos.as_ref().unwrap()["1"].Count, 20);

    // 已存在 data group 时合并计数；Go 的 addConflictInfo 还会合并其它冲突字段，保留计数断言。
    let info3 = engineapi::ConflictInfo { Count: 5, ..Default::default() };
    gci.addDataConflictInfo(&info3);
    assert_eq!(gci.ConflictInfos.as_ref().unwrap().len(), 2);
    assert_eq!(gci.ConflictInfos.as_ref().unwrap()[globalsort::DataKVGroup].Count, 15);

    // 再次传入 0 count 不改变已有 map。
    gci.addDataConflictInfo(&engineapi::ConflictInfo { Count: 0, ..Default::default() });
    assert_eq!(gci.ConflictInfos.as_ref().unwrap()[globalsort::DataKVGroup].Count, 15);
}
"###;

use crate::{Checksum, KVGroupConflictInfos, index_id_to_kv_group};
use astersql_ingestor_engineapi::ConflictInfo;

/// 验证 data 组冲突计数可累加、index 组按 index id 独立记账，以及 Checksum 字段往返一致。
#[test]
fn conflict_infos_merge_counts_and_checksum_round_trips() {
    let mut infos = KVGroupConflictInfos::default();
    infos.addDataConflictInfo(&ConflictInfo::default());
    assert!(infos.ConflictInfos.is_empty());
    // 首次写入 data 组冲突信息（Count=10）。
    infos.addDataConflictInfo(&ConflictInfo {
        Count: 10,
        ..ConflictInfo::default()
    });
    assert_eq!(infos.ConflictInfos.len(), 1);
    // index id=7 对应独立 KV 组，计数为 4。
    infos.addIndexConflictInfo(
        7,
        &ConflictInfo {
            Count: 4,
            ..ConflictInfo::default()
        },
    );
    assert_eq!(infos.ConflictInfos.len(), 2);
    // 再次写入 data 组，期望 Count 合并为 15。
    infos.addDataConflictInfo(&ConflictInfo {
        Count: 5,
        ..ConflictInfo::default()
    });
    assert_eq!(infos.ConflictInfos["data"].Count, 15);
    assert_eq!(infos.ConflictInfos[&index_id_to_kv_group(7)].Count, 4);
    infos.addDataConflictInfo(&ConflictInfo::default());
    assert_eq!(infos.ConflictInfos.len(), 2);
    assert_eq!(infos.ConflictInfos["data"].Count, 15);

    // Checksum 是可序列化的校验和摘要；ToKVChecksum 转为运行时 KVChecksum。
    let checksum = Checksum {
        Sum: 11,
        KVs: 12,
        Size: 13,
    };
    let kv = checksum.ToKVChecksum();
    assert_eq!(kv.Sum(), 11);
    assert_eq!(kv.SumKVS(), 12);
    assert_eq!(kv.SumSize(), 13);
}

#[test]
fn import_step_meta_decodes_persisted_chunks_and_result() {
    let mut meta = crate::ImportStepMeta::default();
    meta.ID = 3;
    meta.Chunks = vec![astersql_executor_importer::Chunk {
        Path: "file.csv".into(),
        FileSize: 19,
        ..Default::default()
    }];
    meta.Checksum.insert(
        7,
        crate::Checksum {
            Sum: 5,
            KVs: 2,
            Size: 9,
        },
    );
    meta.MaxIDs
        .insert(astersql_meta_autoid::AllocatorType::RowId, 42);
    let decoded = crate::ImportStepMeta::Unmarshal(&meta.Marshal().unwrap()).unwrap();
    assert_eq!(decoded.ID, 3);
    assert_eq!(decoded.Chunks[0].Path, "file.csv");
    assert_eq!(decoded.Chunks[0].FileSize, 19);
    assert_eq!(decoded.Checksum[&7].Sum, 5);
    assert_eq!(
        decoded.MaxIDs[&astersql_meta_autoid::AllocatorType::RowId],
        42
    );
}

#[test]
fn import_step_meta_rejects_go_int32_overflow_and_unknown_chunk_type() {
    assert!(
        crate::ImportStepMeta::Unmarshal(br#"{"ID":2147483648}"#)
            .err()
            .unwrap()
            .to_string()
            .contains("int32")
    );
    assert!(
        crate::ImportStepMeta::Unmarshal(br#"{"Chunks":[{"Type":99}]}"#)
            .err()
            .unwrap()
            .to_string()
            .contains("source type")
    );
}

#[test]
fn sorted_kv_meta_empty_summary_and_counter_overflow_match_go() {
    let empty = crate::new_sorted_kv_meta(&crate::WriterSummary {
        TotalSize: 8,
        TotalCnt: 9,
        ..Default::default()
    });
    assert_eq!(empty.TotalKVSize, 0);
    assert_eq!(empty.TotalKVCnt, 0);

    let mut meta = crate::SortedKVMeta {
        StartKey: vec![1],
        EndKey: vec![2],
        TotalKVSize: u64::MAX,
        TotalKVCnt: u64::MAX,
        ..Default::default()
    };
    meta.Merge(&crate::SortedKVMeta {
        StartKey: vec![1],
        EndKey: vec![3],
        TotalKVSize: 1,
        TotalKVCnt: 2,
        ..Default::default()
    });
    assert_eq!(meta.TotalKVSize, 0);
    assert_eq!(meta.TotalKVCnt, 1);
}
