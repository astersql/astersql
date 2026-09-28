// Copyright 2026 AsterSQL.
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

// `TrackOpenMemStorage` 迁移单元测试。
//
// 校验成功打开后当前/累计计数、按偏移读回、关闭后当前计数归零；
// 以及打开失败时回滚 `Opened` 但保留 `TotalOpened` 递增。

use std::io::Read;
use std::sync::Arc;
use std::sync::atomic::{AtomicI32, Ordering};

use crate::TrackOpenMemStorage;
use objstore::storage::{Context as StorageContext, Storage};

/// 构造计数清零的追踪内存存储。
fn tracked_store() -> Arc<TrackOpenMemStorage> {
    Arc::new(TrackOpenMemStorage {
        MemStorage: Arc::new(objstore::memstore::NewMemStorage()),
        Opened: AtomicI32::new(0),
        TotalOpened: AtomicI32::new(0),
    })
}

/// 向底层 MemStorage 写入测试夹具文件。
fn write_file(store: &TrackOpenMemStorage, path: &str, contents: &[u8]) {
    store
        .MemStorage
        .WriteFile(&StorageContext::background(), path, contents)
        .expect("write fixture to memory storage");
}

#[test]
fn open_and_close_track_current_and_total_readers() {
    let store = tracked_store();
    write_file(&store, "data", b"abcdef");

    // StartOffset/EndOffset 半开区间 [1,4)，读出 "bcd"。
    let option = storeapi::ReaderOption {
        StartOffset: Some(1),
        EndOffset: Some(4),
        ..Default::default()
    };
    let mut reader = store
        .Open(&storeapi::Context::default(), "data", Some(&option))
        .expect("open tracked reader");

    assert_eq!(store.Opened.load(Ordering::SeqCst), 1);
    assert_eq!(store.TotalOpened.load(Ordering::SeqCst), 1);
    let mut contents = String::new();
    reader
        .read_to_string(&mut contents)
        .expect("read selected range");
    assert_eq!(contents, "bcd");
    // GetFileSize 返回完整对象长度，不受读区间裁剪影响。
    assert_eq!(reader.GetFileSize().expect("get full file size"), 6);

    reader.Close().expect("close tracked reader");
    // Close 成功后当前打开数归零；累计尝试数保持。
    assert_eq!(store.Opened.load(Ordering::SeqCst), 0);
    assert_eq!(store.TotalOpened.load(Ordering::SeqCst), 1);
}

#[test]
fn failed_open_rolls_back_current_count_but_keeps_total_attempts() {
    let store = tracked_store();

    let result = store.Open(&storeapi::Context::default(), "missing", None);

    // 文件不存在：Opened 回滚，TotalOpened 仍记一次尝试。
    assert!(result.is_err());
    assert_eq!(store.Opened.load(Ordering::SeqCst), 0);
    assert_eq!(store.TotalOpened.load(Ordering::SeqCst), 1);

    // 已取消的 Context 同样失败，再次累加 TotalOpened。
    let cancelled = storeapi::Context::default();
    cancelled.cancel();
    let result = store.Open(&cancelled, "missing", None);
    assert!(result.is_err());
    assert_eq!(store.Opened.load(Ordering::SeqCst), 0);
    assert_eq!(store.TotalOpened.load(Ordering::SeqCst), 2);
}

#[test]
fn cancelled_open_preserves_cancellation_error_classification() {
    let store = tracked_store();
    write_file(&store, "data", b"abcdef");
    let cancelled = storeapi::Context::default();
    cancelled.cancel();

    let error = store
        .Open(&cancelled, "data", None)
        .err()
        .expect("cancelled open must fail");

    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(store.Opened.load(Ordering::SeqCst), 0);
    assert_eq!(store.TotalOpened.load(Ordering::SeqCst), 1);
}
