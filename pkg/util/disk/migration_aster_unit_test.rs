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

// Aster 迁移补充单测：临时目录并发初始化与 Tracker 别名行为。
//
// 覆盖 `CheckAndInitTempDir` 并发安全、异步清理保留 `_dir.lock`/`record`，
// 以及 `CheckAndCreateDir`/`NewTracker`/`NewGlobalTracker` 与 Go 语义对齐。

use super::*;
use astersql_config::{get_global_config, restore_func, update_global};
use std::fs;
use std::sync::{Arc, Barrier};
use std::thread;
use std::time::{Duration, Instant};

/// 将全局 `temp_storage_path` 临时改到 `path`，Drop 时 `CleanUp` 并恢复配置。
fn use_temp_storage(path: &std::path::Path) -> impl Drop {
    let restore = restore_func();
    update_global(|config| config.temp_storage_path = path.to_string_lossy().into_owned());
    struct Restore<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for Restore<F> {
        fn drop(&mut self) {
            CleanUp();
            (self.0.take().unwrap())();
        }
    }
    Restore(Some(restore))
}

/// 并发调用初始化后，清理陈旧条目但保留 record 与锁文件。
#[test]
#[serial_test::serial(temp_dir)]
fn temp_dir_recreates_concurrently_and_preserves_reserved_entries() {
    let root = tempfile::tempdir().unwrap();
    let path = root.path().join("tmp-storage");
    let _restore = use_temp_storage(&path);

    // 多线程同时 CheckAndInitTempDir，验证 singleflight 式互斥仍能成功建目录。
    let workers = 10;
    let barrier = Arc::new(Barrier::new(workers));
    let mut joins = Vec::new();
    for _ in 0..workers {
        let barrier = Arc::clone(&barrier);
        joins.push(thread::spawn(move || {
            barrier.wait();
            CheckAndInitTempDir()
        }));
    }
    for join in joins {
        join.join().unwrap().unwrap();
    }
    assert!(path.is_dir());

    // 写入保留目录 record 与陈旧条目，再 InitializeTempDir，等待异步删除完成。
    fs::create_dir(path.join("record")).unwrap();
    fs::write(path.join("stale-a"), b"a").unwrap();
    fs::create_dir(path.join("stale-b")).unwrap();
    CleanUp();
    InitializeTempDir().unwrap();

    let deadline = Instant::now() + Duration::from_secs(2);
    while (path.join("stale-a").exists() || path.join("stale-b").exists())
        && Instant::now() < deadline
    {
        thread::sleep(Duration::from_millis(10));
    }
    assert!(!path.join("stale-a").exists());
    assert!(!path.join("stale-b").exists());
    assert!(path.join("record").is_dir());
    assert!(path.join("_dir.lock").is_file());
    assert_eq!(
        get_global_config().temp_storage_path,
        path.to_string_lossy()
    );
}

/// 幂等建目录，以及磁盘 Tracker 构造函数别名与 Go 一致。
#[test]
fn check_and_create_dir_and_tracker_aliases_match_go_behavior() {
    let root = tempfile::tempdir().unwrap();
    let nested = root.path().join("one/two");
    CheckAndCreateDir(&nested).unwrap();
    CheckAndCreateDir(&nested).unwrap();
    assert!(nested.is_dir());

    // Go 仅在 os.Stat 返回错误时创建；已存在的普通文件同样直接成功。
    let existing_file = root.path().join("existing-file");
    fs::write(&existing_file, b"x").unwrap();
    CheckAndCreateDir(&existing_file).unwrap();

    // NewTracker(label, bytesLimit)；NewGlobalTracker 在 limit=0 时字节上限为 -1（无上限）。
    let tracker: Box<Tracker> = NewTracker(7, 128);
    assert_eq!(tracker.Label(), 7);
    assert_eq!(tracker.GetBytesLimit(), 128);

    let global: Box<Tracker> = NewGlobalTracker(8, 0);
    assert_eq!(global.Label(), 8);
    assert_eq!(global.GetBytesLimit(), -1);
}
