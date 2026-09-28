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

// ingest 内存分配失败相关的错误消息与错误结构。
//
// 当 MemRoot（内存配额根）无法为后端、引擎或写入器预留内存时，
// 用统一的字面量与 `IngestMemoryError` 携带任务 ID、索引 ID 及当前用量信息。

use crate::mem_root::MemRoot;

// Message const text. Keep this list in lockstep with message.go.
pub const LIT_ERR_ALLOC_MEM_FAIL: &str = "allocate memory failed";
pub const LIT_ERR_CREATE_DIR_FAIL: &str = "create ingest sort path error";
pub const LIT_ERR_STAT_DIR_FAIL: &str = "stat ingest sort path error";
pub const LIT_ERR_CREATE_BACKEND_FAIL: &str = "build ingest backend failed";
pub const LIT_ERR_GET_BACKEND_FAIL: &str = "cannot get ingest backend";
pub const LIT_ERR_CREATE_ENGINE_FAIL: &str = "build ingest engine failed";
pub const LIT_ERR_CREATE_CONTEXT_FAIL: &str = "build ingest writer context failed";
pub const LIT_ERR_GET_ENGINE_FAIL: &str = "can not get ingest engine info";
pub const LIT_ERR_GET_STORAGE_QUOTA: &str = "get storage quota error";
pub const LIT_ERR_CLOSE_ENGINE_ERR: &str = "close engine error";
pub const LIT_ERR_CLEAN_ENGINE_ERR: &str = "clean engine error";
pub const LIT_ERR_FLUSH_ENGINE_ERR: &str = "flush engine data err";
pub const LIT_ERR_INGEST_DATA_ERR: &str = "ingest data into storage error";
pub const LIT_ERR_REMOTE_DUP_EXIST_ERR: &str = "remote duplicate index key exist";
pub const LIT_ERR_EXCEED_CONCURRENCY: &str = "the concurrency is greater than ingest limit";
pub const LIT_ERR_CLOSE_WRITER_ERR: &str = "close writer error";
pub const LIT_ERR_READ_SORT_PATH: &str = "cannot read sort path";
pub const LIT_ERR_CLEAN_SORT_PATH: &str = "clean up temp dir failed";
pub const LIT_ERR_RESET_ENGINE_FAIL: &str = "reset engine failed";
pub const LIT_WARN_ENV_INIT_FAIL: &str = "initialize environment failed";
pub const LIT_WARN_CONFIG_ERROR: &str = "build config for backend failed";
pub const LIT_INFO_ENV_INIT_SUCC: &str = "init global ingest backend environment finished";
pub const LIT_INFO_SORT_DIR: &str = "the ingest sorted directory";
pub const LIT_INFO_CREATE_BACKEND: &str = "create one backend for an DDL job";
pub const LIT_INFO_CLOSE_BACKEND: &str = "close one backend for DDL job";
pub const LIT_INFO_OPEN_ENGINE: &str = "open an engine for index reorg task";
pub const LIT_INFO_CREATE_WRITE: &str = "create one local writer for index reorg task";
pub const LIT_INFO_CLOSE_ENGINE: &str = "flush all writer and get closed engine";
pub const LIT_INFO_REMOTE_DUP_CHECK: &str = "start remote duplicate checking";
pub const LIT_INFO_START_IMPORT: &str = "start to import data";
pub const LIT_INFO_CHG_MEM_SETTING: &str = "change memory setting for ingest";
pub const LIT_INFO_INIT_MEM_SETTING: &str = "initial memory setting for ingest";
pub const LIT_INFO_UNSAFE_IMPORT: &str = "do a partial import data into the storage";

/// ingest 内存分配失败时的结构化错误信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IngestMemoryError {
    /// 固定错误文案（对应上方 `LIT_*` 常量）。
    pub message: &'static str,
    /// 相关 DDL 任务 ID。
    pub job_id: i64,
    /// 相关索引 ID 列表。
    pub index_ids: Vec<i64>,
    /// 出错时 MemRoot 的当前用量（字节）。
    pub current_usage: i64,
    /// 出错时 MemRoot 的最大配额（字节）。
    pub max_quota: i64,
}

/// 构造“引擎内存分配失败”错误，并附带当前 MemRoot 用量快照。
pub fn engine_alloc_memory_failed(
    mem_root: &dyn MemRoot,
    job_id: i64,
    index_ids: &[i64],
) -> IngestMemoryError {
    IngestMemoryError {
        message: LIT_ERR_ALLOC_MEM_FAIL,
        job_id,
        index_ids: index_ids.to_vec(),
        current_usage: mem_root.current_usage(),
        max_quota: mem_root.max_memory_quota(),
    }
}
/// 构造“写入器内存分配失败”错误；`index_ids` 仅含单个索引 ID。
pub fn writer_alloc_memory_failed(
    mem_root: &dyn MemRoot,
    job_id: i64,
    index_id: i64,
) -> IngestMemoryError {
    IngestMemoryError {
        message: LIT_ERR_ALLOC_MEM_FAIL,
        job_id,
        index_ids: vec![index_id],
        current_usage: mem_root.current_usage(),
        max_quota: mem_root.max_memory_quota(),
    }
}
