// Copyright 2023 PingCAP, Inc.
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

// `syncutil` 迁移对齐单测：对照 Go 验证 deadlock / sync 两套锁表面。
//
// 覆盖 EnableDeadlock 常量、互斥串行化、持锁 panic 后可恢复、读写锁读写互斥等。

#[path = "mutex_deadlock.rs"]
mod mutex_deadlock;
#[path = "mutex_sync.rs"]
mod mutex_sync;

use std::sync::{Arc, Barrier};
use std::thread;
use std::time::Duration;

/// 断言两套变体的 EnableDeadlock 与 DEADLOCK_TIMEOUT 与 Go 一致，并调用 init。
#[test]
fn build_variants_expose_the_go_enable_deadlock_values() {
    assert!(!mutex_sync::EnableDeadlock);
    assert!(mutex_deadlock::EnableDeadlock);
    assert_eq!(mutex_deadlock::DEADLOCK_TIMEOUT, Duration::from_secs(20));
    mutex_deadlock::init();
}

/// 断言当前 crate 导出的锁表面与 feature 选择一致，且 lock/read 可用。
#[test]
fn selected_build_variant_exposes_one_unambiguous_lock_surface() {
    assert_eq!(super::EnableDeadlock, cfg!(feature = "deadlock"));

    let mutex = super::Mutex::new(13_u8);
    assert_eq!(*mutex.lock(), 13);

    let rwmutex = super::RWMutex::new(17_u8);
    assert_eq!(*rwmutex.read(), 17);
}

/// 多线程并发对 Mutex 累加，验证互斥串行化结果正确。
#[test]
fn mutex_serializes_parallel_updates() {
    let value = Arc::new(mutex_sync::Mutex::new(0_usize));
    let workers: Vec<_> = (0..8)
        .map(|_| {
            let value = Arc::clone(&value);
            thread::spawn(move || {
                for _ in 0..1_000 {
                    *value.lock() += 1;
                }
            })
        })
        .collect();

    for worker in workers {
        worker.join().unwrap();
    }
    assert_eq!(*value.lock(), 8_000);
}

/// 持锁线程 panic 后，其它线程仍能取得锁并看到已写入值（对齐 Go 语义）。
#[test]
fn mutex_remains_usable_after_a_holder_panics_like_go() {
    let value = Arc::new(mutex_sync::Mutex::new(7_u8));
    let panicking_value = Arc::clone(&value);
    assert!(
        thread::spawn(move || {
            let mut guard = panicking_value.lock();
            *guard = 9;
            panic!("intentional panic while holding the lock");
        })
        .join()
        .is_err()
    );

    assert_eq!(*value.lock(), 9);
}

/// 多读者可并发持有读锁，写锁在读者释放前 try_write 失败。
#[test]
fn rwmutex_allows_readers_and_blocks_a_writer() {
    let value = Arc::new(mutex_sync::RWMutex::new(11_u8));
    let readers_ready = Arc::new(Barrier::new(3));
    let release_readers = Arc::new(Barrier::new(3));
    let readers: Vec<_> = (0..2)
        .map(|_| {
            let value = Arc::clone(&value);
            let readers_ready = Arc::clone(&readers_ready);
            let release_readers = Arc::clone(&release_readers);
            thread::spawn(move || {
                let guard = value.read();
                readers_ready.wait();
                release_readers.wait();
                assert_eq!(*guard, 11);
            })
        })
        .collect();

    readers_ready.wait();
    assert!(value.try_write().is_none());
    release_readers.wait();
    for reader in readers {
        reader.join().unwrap();
    }
    *value.write() = 12;
    assert_eq!(*value.read(), 12);
}

/// deadlock 变体导出的 Mutex/RWMutex 与普通变体提供相同锁 API。
#[test]
fn deadlock_variant_uses_the_same_lock_surface() {
    let mutex = mutex_deadlock::Mutex::new(3_u8);
    assert_eq!(*mutex.lock(), 3);

    let rwmutex = mutex_deadlock::RWMutex::new(5_u8);
    assert_eq!(*rwmutex.read(), 5);
}
