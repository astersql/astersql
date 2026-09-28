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

// checkpoint 模块的单元测试。
//
// 本文件验证 DDL ingest（快速索引回填/数据摄入）流程中检查点（checkpoint）
// 机制的正确性。检查点用于记录索引回填任务已处理到的键位置（水位线，
// watermark），当 DDL 任务因节点重启、故障切换等原因中断后，可以从检查点
// 恢复而无需从头重做，避免重复扫描与写入。
//
// 核心概念：
// - `CheckpointManager`：检查点管理器，跟踪各个数据分块（chunk/task）的
//   读取与写入进度，并在满足条件时推进水位线、持久化检查点。
// - `CheckpointStorage` / `MemoryCheckpointStorage`：检查点存储抽象及其
//   内存实现，测试中用内存实现替代真实的系统表存储。
// - `ReorgCheckpoint`：检查点的持久化数据结构，记录本地/全局同步键、
//   已处理总键数、导入时间戳（import_ts）等信息。
// - 本地水位线（local_sync_key）与全局水位线（global_sync_key）：本地
//   水位线表示本节点本地引擎已写完的位置，全局水位线表示数据已导入到
//   分布式存储（TiKV）的位置；恢复时优先使用更靠前的本地水位线。

use std::sync::Arc;

use crate::checkpoint::{
    CheckpointManager, CheckpointStorage, MemoryCheckpointStorage, ReorgCheckpoint,
};

/// 验证：当一个分块被完整读取并写入后，推进水位线会把已导入的进度
/// 持久化到存储中（对应 Go 端 TestCheckpointManagerUpdateReorg）。
#[test]
fn checkpoint_persists_imported_watermark_in_memory_storage() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager =
        CheckpointManager::new(storage.clone(), b"09".to_vec(), 123_456, "node-1").unwrap();

    manager.add_chunk(0, b"19".to_vec());
    // Reader reports the last batch, then writer finishes the same rows (Go TestCheckpointManagerUpdateReorg).
    // 读取端上报最后一批数据（last_batch=true），随后写入端完成同样的行数，
    // 该分块即视为处理完毕，可以推进水位线。
    manager.update_chunk(0, 100, true);
    manager.finish_chunk(0, 100);
    assert_eq!(manager.total_key_count(), 100);
    manager.advance_watermark(true).unwrap();
    manager.close().unwrap();

    // 关闭后从存储加载检查点，确认本地/全局水位线、总键数、导入时间戳
    // 均已正确落盘。
    let checkpoint = storage.load_checkpoint().unwrap().unwrap();
    assert_eq!(checkpoint.local_sync_key, b"19");
    assert_eq!(checkpoint.global_sync_key, b"19");
    assert_eq!(checkpoint.local_key_count, 100);
    assert_eq!(checkpoint.global_key_count, 100);
    assert_eq!(checkpoint.import_ts, 123_456);
    assert_eq!(checkpoint.version, 1);
}

#[test]
fn checkpoint_persists_distinct_local_and_global_counts() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage.clone(), Vec::new(), 7, "node-1").unwrap();
    manager.add_chunk(0, b"09".to_vec());
    manager.update_chunk(0, 40, true);
    manager.finish_chunk(0, 40);
    manager.advance_watermark(false).unwrap();
    manager.close().unwrap();

    let checkpoint = storage.load_checkpoint().unwrap().unwrap();
    assert_eq!(checkpoint.local_key_count, 40);
    assert_eq!(checkpoint.global_key_count, 0);
}

#[test]
fn checkpoint_rejects_mismatched_physical_id_and_invalid_local_data() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    storage
        .save_checkpoint(&ReorgCheckpoint {
            version: 1,
            local_sync_key: b"29".to_vec(),
            local_key_count: 100,
            global_sync_key: b"19".to_vec(),
            global_key_count: 80,
            instance_addr: "node-1".to_owned(),
            physical_id: 42,
            import_ts: 123_456,
        })
        .unwrap();

    let mismatched = CheckpointManager::new_with_resume_options(
        storage.clone(),
        b"09".to_vec(),
        9,
        "node-1",
        7,
        true,
    )
    .unwrap();
    assert_eq!(mismatched.next_start_key(), b"09");
    assert_eq!(mismatched.import_ts(), 9);

    let no_local_data =
        CheckpointManager::new_with_resume_options(storage, Vec::new(), 0, "node-1", 42, false)
            .unwrap();
    assert!(no_local_data.is_key_processed(b"19"));
    assert!(!no_local_data.is_key_processed(b"29"));
    assert_eq!(no_local_data.next_start_key(), b"19");
    assert_eq!(no_local_data.total_key_count(), 0);
}

/// 验证：从已有检查点恢复时，管理器采用更靠前的本地水位线（local_sync_key）
/// 作为续跑起点，并恢复累计键数与导入时间戳。
#[test]
fn checkpoint_resumes_the_available_local_watermark() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    // 预先写入一个检查点：本地水位线（"29"）领先于全局水位线（"19"），
    // 模拟本地引擎已写到更远位置但尚未全部导入到远端存储的场景。
    storage
        .save_checkpoint(&ReorgCheckpoint {
            version: 1,
            local_sync_key: b"29".to_vec(),
            local_key_count: 100,
            global_sync_key: b"19".to_vec(),
            global_key_count: 100,
            instance_addr: "node-1".to_owned(),
            physical_id: 0,
            import_ts: 123_456,
        })
        .unwrap();

    // 新建管理器时会加载检查点；两个水位线之内的键都应被视为已处理，
    // 且下一个起始键取本地水位线 "29"。
    let manager = CheckpointManager::new(storage, Vec::new(), 0, "node-1").unwrap();
    assert!(manager.is_key_processed(b"19"));
    assert!(manager.is_key_processed(b"29"));
    assert_eq!(manager.next_start_key(), b"29");
    assert_eq!(manager.total_key_count(), 100);
    assert_eq!(manager.import_ts(), 123_456);
}

/// 验证：若读取端尚未上报最后一批（last_batch=false），即使写入端已完成
/// 相同行数，水位线也不能推进——防止漏掉尚未读完的数据。
#[test]
fn checkpoint_waits_for_the_last_reader_batch_before_advancing() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage, Vec::new(), 1, "node-1").unwrap();
    manager.add_chunk(0, b"09".to_vec());

    // last_batch=false 表示该分块还有后续批次未读取。
    manager.update_chunk(0, 100, false);
    manager.finish_chunk(0, 100);
    manager.advance_watermark(true).unwrap();

    // 分块未读完，键 "09" 不能视为已处理，起始键保持为空。
    assert!(!manager.is_key_processed(b"09"));
    assert_eq!(manager.next_start_key(), b"");
}

/// 验证：读取端上报了两批（共 100 行 + 末批 0 行），但写入端只完成了
/// 第一批，未完成的批次会阻止水位线推进。
#[test]
fn checkpoint_waits_for_every_finished_chunk_before_advancing() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage, Vec::new(), 1, "node-1").unwrap();
    manager.add_chunk(0, b"09".to_vec());

    // 读取端分两次上报：先 100 行，再以空批标记结束（last_batch=true）。
    manager.update_chunk(0, 100, false);
    manager.update_chunk(0, 0, true);
    // 写入端只完成了第一批的 100 行，末批尚未确认写完。
    manager.finish_chunk(0, 100);
    manager.advance_watermark(true).unwrap();

    // 仍有批次未被写入端确认，水位线不能推进。
    assert!(!manager.is_key_processed(b"09"));
    assert_eq!(manager.next_start_key(), b"");
}

/// 验证：多个任务乱序完成时，水位线只沿"从最小任务号开始的连续已完成
/// 前缀"推进——中间有未完成任务时不能跳跃，否则恢复时会漏数据。
#[test]
fn checkpoint_advances_only_the_contiguous_completed_task_prefix() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage, Vec::new(), 1, "node-1").unwrap();
    // 注册三个任务分块，结束键依次递增。
    for (task_id, end_key) in [(0, b"09"), (1, b"19"), (2, b"29")] {
        manager.add_chunk(task_id, end_key.to_vec());
    }

    // 先完成任务 2：因为任务 0、1 未完成，水位线不能推进到任何位置。
    manager.update_chunk(2, 100, true);
    manager.finish_chunk(2, 100);
    manager.advance_watermark(true).unwrap();
    assert!(!manager.is_key_processed(b"09"));
    assert!(!manager.is_key_processed(b"19"));
    assert!(!manager.is_key_processed(b"29"));

    // 再完成任务 0：连续前缀为 [0]，水位线推进到 "09"，任务 1、2 仍受阻。
    manager.update_chunk(0, 100, true);
    manager.finish_chunk(0, 100);
    manager.advance_watermark(true).unwrap();
    assert!(manager.is_key_processed(b"09"));
    assert!(!manager.is_key_processed(b"19"));
    assert!(!manager.is_key_processed(b"29"));

    // 最后完成任务 1：前缀 [0,1,2] 全部完成，水位线一次性推进到 "29"。
    manager.update_chunk(1, 100, true);
    manager.finish_chunk(1, 100);
    manager.advance_watermark(true).unwrap();
    assert!(manager.is_key_processed(b"29"));
    assert_eq!(manager.next_start_key(), b"29");
}

/// 验证：同一分块分多批读取与写入时，总键数只按读取端上报的行数累计
/// 一次，不会因写入端的多次 finish 而重复计数。
#[test]
fn checkpoint_counts_reader_rows_once_after_writer_completion() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage, Vec::new(), 1, "node-1").unwrap();
    manager.add_chunk(0, b"09".to_vec());
    // 读取端分两批上报（60 + 40），写入端也分两次确认完成。
    manager.update_chunk(0, 60, false);
    manager.update_chunk(0, 40, true);
    manager.finish_chunk(0, 60);
    manager.finish_chunk(0, 40);
    manager.advance_watermark(true).unwrap();

    // 总键数应为 60 + 40 = 100，而不是被重复累加。
    assert_eq!(manager.total_key_count(), 100);
}

/// 验证：对未注册（未 add_chunk）的任务调用 finish_chunk 会被安全忽略，
/// 不影响统计、不产生更新，也不会向存储写入检查点。
#[test]
fn checkpoint_ignores_finish_for_an_unknown_task() {
    let storage = Arc::new(MemoryCheckpointStorage::default());
    let mut manager = CheckpointManager::new(storage.clone(), Vec::new(), 1, "node-1").unwrap();
    // 任务 99 从未注册过，finish 调用应为无操作（no-op）。
    manager.finish_chunk(99, 100);
    assert_eq!(manager.total_key_count(), 0);
    assert!(manager.no_update());
    manager.advance_watermark(true).unwrap();
    assert_eq!(manager.next_start_key(), b"");
    assert_eq!(storage.load_checkpoint().unwrap(), None);
}
