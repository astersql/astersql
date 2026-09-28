// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

//! Go-equivalent tests for `br/pkg/utils/storewatch/watching_test.go`.
//!
//! Production Rust abstracts PD/`conn.GetAllTiKVStoresWithRetry` behind
//! `StoreMeta::GetAllTiKVStores`; the sequential fixture therefore implements
//! that trait while preserving Go's pop-front sequence and error shape.
//!
//! 对照 Go 单测：注册 / 掉线 / 重启三条路径分别断言对应回调被调用。
//! 序列耗尽错误文案与 Go fixture 保持一致，避免误报「实现层」问题。
//! 每测只挂感兴趣的钩子，用 AtomicBool 观察是否触发，避免事件串耦合。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::{
    MakeCallback, New, Store, StoreMeta, StoreState, WithOnDisconnect, WithOnNewStoreRegistered,
    WithOnReboot,
};

/// SequentialReturningStoreMeta 对应 Go 测试 fixture：每次 GetAllStores / GetAllTiKVStores
/// 弹出一个预设 store 列表；序列耗尽时返回与 Go 相同的错误文案。
struct SequentialReturningStoreMeta {
    // 先进先出的多帧 store 列表；每帧对应一次 Step。
    sequence: Mutex<Vec<Vec<Store>>>,
}

/// 构造顺序吐帧的 StoreMeta 替身。
fn new_sequential_returning_store_meta(sequence: Vec<Vec<Store>>) -> SequentialReturningStoreMeta {
    SequentialReturningStoreMeta {
        sequence: Mutex::new(sequence),
    }
}

impl StoreMeta for SequentialReturningStoreMeta {
    fn GetAllTiKVStores(&self) -> Result<Vec<Store>, String> {
        let mut sequence = self.sequence.lock().expect("sequence lock");
        if sequence.is_empty() {
            // 与 Go 测试相同文案，便于跨语言对照失败信息。
            return Err("too many call to `GetAllStores` in test".to_string());
        }
        // Go 取 sequence[0] 后切掉头部；这里用 remove(0) 保留“每次前进一帧”的测试语义。
        Ok(sequence.remove(0))
    }
}

/// TestOnRegister：首次看到 Up store 时触发 OnNewStoreRegistered。
#[test]
fn test_on_register() {
    // A sequence of store state that we should believe the store is offline.
    // 单帧 Up：首见即注册，不触发掉线/重启。
    let seq = new_sequential_returning_store_meta(vec![vec![Store {
        Id: 1,
        State: StoreState::Up,
        StartTimestamp: 0,
    }]]);
    let callback_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&callback_called);
    // 仅挂注册钩子，掉线/重启钩子缺省为空。
    let callback = MakeCallback(vec![WithOnNewStoreRegistered(move |_s: &Store| {
        called.store(true, Ordering::SeqCst);
    })]);

    let mut watcher = New(seq, callback);
    // Step 成功且注册回调置位。
    assert!(watcher.Step().is_ok());
    assert!(callback_called.load(Ordering::SeqCst));
}

/// TestOnOffline：同一 store 从 Up 变为 Offline 时触发 OnDisconnect。
#[test]
fn test_on_offline() {
    // A sequence of store state that we should believe the store is offline.
    // 两帧：先注册，再 Up→Offline 触发掉线。
    let seq = new_sequential_returning_store_meta(vec![
        vec![Store {
            Id: 1,
            State: StoreState::Up,
            StartTimestamp: 0,
        }],
        vec![Store {
            Id: 1,
            State: StoreState::Offline,
            StartTimestamp: 0,
        }],
    ]);
    let callback_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&callback_called);
    let callback = MakeCallback(vec![WithOnDisconnect(move |_s: &Store| {
        called.store(true, Ordering::SeqCst);
    })]);

    let mut watcher = New(seq, callback);
    // 第一帧建立 lastStores，第二帧触发掉线。
    assert!(watcher.Step().is_ok());
    assert!(watcher.Step().is_ok());
    assert!(callback_called.load(Ordering::SeqCst));
}

/// TestOnReboot：store 重新变为 Up 且 StartTimestamp 变化时触发 OnReboot。
#[test]
fn test_on_reboot() {
    // A sequence of store state that we should believe the store is offline.
    // 三帧：注册 → 掉线 → 新时间戳上线，断言 reboot 回调。
    let seq = new_sequential_returning_store_meta(vec![
        vec![Store {
            Id: 1,
            State: StoreState::Up,
            StartTimestamp: 1,
        }],
        vec![Store {
            Id: 1,
            State: StoreState::Offline,
            StartTimestamp: 1,
        }],
        vec![Store {
            Id: 1,
            State: StoreState::Up,
            StartTimestamp: 2,
        }],
    ]);
    let callback_called = Arc::new(AtomicBool::new(false));
    let called = Arc::clone(&callback_called);
    let callback = MakeCallback(vec![WithOnReboot(move |_s: &Store| {
        called.store(true, Ordering::SeqCst);
    })]);

    let mut watcher = New(seq, callback);
    // 三帧走完后 reboot 钩子应已触发（时间戳 1→2）。
    assert!(watcher.Step().is_ok());
    assert!(watcher.Step().is_ok());
    assert!(watcher.Step().is_ok());
    assert!(callback_called.load(Ordering::SeqCst));
}
