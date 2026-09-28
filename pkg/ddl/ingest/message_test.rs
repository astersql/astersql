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

use crate::mem_root::{MemRoot, MemRootImpl};
use crate::message::*;

#[test]
fn message_constants_match_go() {
    assert_eq!(LIT_ERR_ALLOC_MEM_FAIL, "allocate memory failed");
    assert_eq!(LIT_ERR_CREATE_DIR_FAIL, "create ingest sort path error");
    assert_eq!(LIT_ERR_STAT_DIR_FAIL, "stat ingest sort path error");
    assert_eq!(LIT_ERR_CREATE_BACKEND_FAIL, "build ingest backend failed");
    assert_eq!(LIT_ERR_GET_BACKEND_FAIL, "cannot get ingest backend");
    assert_eq!(LIT_ERR_CREATE_ENGINE_FAIL, "build ingest engine failed");
    assert_eq!(
        LIT_ERR_CREATE_CONTEXT_FAIL,
        "build ingest writer context failed"
    );
    assert_eq!(LIT_ERR_GET_ENGINE_FAIL, "can not get ingest engine info");
    assert_eq!(LIT_ERR_GET_STORAGE_QUOTA, "get storage quota error");
    assert_eq!(LIT_ERR_CLOSE_ENGINE_ERR, "close engine error");
    assert_eq!(LIT_ERR_CLEAN_ENGINE_ERR, "clean engine error");
    assert_eq!(LIT_ERR_FLUSH_ENGINE_ERR, "flush engine data err");
    assert_eq!(LIT_ERR_INGEST_DATA_ERR, "ingest data into storage error");
    assert_eq!(
        LIT_ERR_REMOTE_DUP_EXIST_ERR,
        "remote duplicate index key exist"
    );
    assert_eq!(
        LIT_ERR_EXCEED_CONCURRENCY,
        "the concurrency is greater than ingest limit"
    );
    assert_eq!(LIT_ERR_CLOSE_WRITER_ERR, "close writer error");
    assert_eq!(LIT_ERR_READ_SORT_PATH, "cannot read sort path");
    assert_eq!(LIT_ERR_CLEAN_SORT_PATH, "clean up temp dir failed");
    assert_eq!(LIT_ERR_RESET_ENGINE_FAIL, "reset engine failed");
    assert_eq!(LIT_WARN_ENV_INIT_FAIL, "initialize environment failed");
    assert_eq!(LIT_WARN_CONFIG_ERROR, "build config for backend failed");
    assert_eq!(
        LIT_INFO_ENV_INIT_SUCC,
        "init global ingest backend environment finished"
    );
    assert_eq!(LIT_INFO_SORT_DIR, "the ingest sorted directory");
    assert_eq!(LIT_INFO_CREATE_BACKEND, "create one backend for an DDL job");
    assert_eq!(LIT_INFO_CLOSE_BACKEND, "close one backend for DDL job");
    assert_eq!(LIT_INFO_OPEN_ENGINE, "open an engine for index reorg task");
    assert_eq!(
        LIT_INFO_CREATE_WRITE,
        "create one local writer for index reorg task"
    );
    assert_eq!(
        LIT_INFO_CLOSE_ENGINE,
        "flush all writer and get closed engine"
    );
    assert_eq!(LIT_INFO_REMOTE_DUP_CHECK, "start remote duplicate checking");
    assert_eq!(LIT_INFO_START_IMPORT, "start to import data");
    assert_eq!(LIT_INFO_CHG_MEM_SETTING, "change memory setting for ingest");
    assert_eq!(
        LIT_INFO_INIT_MEM_SETTING,
        "initial memory setting for ingest"
    );
    assert_eq!(
        LIT_INFO_UNSAFE_IMPORT,
        "do a partial import data into the storage"
    );
}

#[test]
fn allocation_failures_capture_go_log_fields() {
    let mem_root = MemRootImpl::new(1_024);
    mem_root.consume(256);

    let engine = engine_alloc_memory_failed(&mem_root, 7, &[11, 13]);
    assert_eq!(engine.message, LIT_ERR_ALLOC_MEM_FAIL);
    assert_eq!(engine.job_id, 7);
    assert_eq!(engine.index_ids, vec![11, 13]);
    assert_eq!((engine.current_usage, engine.max_quota), (256, 1_024));

    let writer = writer_alloc_memory_failed(&mem_root, 8, 17);
    assert_eq!(writer.message, LIT_ERR_ALLOC_MEM_FAIL);
    assert_eq!(writer.job_id, 8);
    assert_eq!(writer.index_ids, vec![17]);
    assert_eq!((writer.current_usage, writer.max_quota), (256, 1_024));
}
