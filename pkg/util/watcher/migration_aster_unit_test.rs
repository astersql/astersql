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

// Watcher 迁移对照单元测试。
//
// 验证 OpString、目录列举非递归、Remove 后不再跟踪，以及文件事件序列、
// Start/Close 状态机与 Go 侧行为一致。

use super::*;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::time::{Duration, Instant};

/// 在超时内等待指定路径上出现目标操作事件（忽略目录事件）。
fn wait_for(watcher: &Watcher, path: &std::path::Path, op: Op) -> Event {
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let event = watcher
            .Events
            .recv_timeout(remaining)
            .unwrap_or_else(|err| panic!("waiting for {op:?} on {}: {err}", path.display()));
        // 只接受精确路径 + 目标操作的文件事件。
        if !event.IsDirEvent() && event.Path == path && event.HasOps(&[op]) {
            return event;
        }
    }
}

/// 对照 Go：OpString 格式与空 Option 事件辅助函数行为。
#[test]
fn migration_op_string_and_nil_event_behavior_match_go() {
    assert_eq!(OpString(0), "");
    assert_eq!(OpString(Create | Modify | Move), "CREATE|MODIFY|MOVE");
    assert!(!IsDirEventOption(None));
    assert!(!HasOpsOption(None, &[Create]));
}

/// 列举仅一层、非递归；Remove 后 Start 不应再收到该目录下的事件。
#[test]
fn migration_listing_is_non_recursive_and_remove_stops_tracking() {
    let root = tempfile::tempdir().unwrap();
    let child = root.path().join("child.txt");
    let nested_dir = root.path().join("nested");
    let nested_child = nested_dir.join("hidden.txt");
    fs::write(&child, b"child").unwrap();
    fs::create_dir(&nested_dir).unwrap();
    fs::write(&nested_child, b"nested").unwrap();

    // listForName 只包含根、直接子文件与直接子目录，不含嵌套文件。
    let listed = listForName(root.path()).unwrap();
    assert!(listed.contains_key(root.path()));
    assert!(listed.contains_key(&child));
    assert!(listed.contains_key(&nested_dir));
    assert!(!listed.contains_key(&nested_child));

    // Add 后再 Remove，Start 后写入不应产生事件。
    let watcher = NewWatcher();
    watcher.Add(root.path()).unwrap();
    watcher.Remove(root.path()).unwrap();
    watcher.Start(Duration::from_millis(10)).unwrap();
    fs::write(root.path().join("after-remove.txt"), b"ignored").unwrap();
    assert!(
        watcher
            .Events
            .recv_timeout(Duration::from_millis(80))
            .is_err()
    );
    watcher.Close();
}

/// 完整文件事件序列：Create → Modify → Chmod → Rename → Remove → Create → Move。
#[test]
fn migration_watcher_reports_go_file_event_sequence() {
    let first = tempfile::tempdir().unwrap();
    let second = tempfile::tempdir().unwrap();
    let old_path = first.path().join("mysql-bin.000001");
    let new_path = first.path().join("mysql-bin.000002");
    let moved_path = second.path().join("mysql-bin.000001");

    let watcher = NewWatcher();
    watcher.Add(first.path()).unwrap();
    watcher.Add(second.path()).unwrap();
    watcher.Start(Duration::from_millis(10)).unwrap();

    fs::write(&old_path, b"").unwrap();
    wait_for(&watcher, &old_path, Create);

    OpenOptions::new()
        .write(true)
        .open(&old_path)
        .unwrap()
        .write_all(b"meaningless content")
        .unwrap();
    wait_for(&watcher, &old_path, Modify);

    // Unix 上通过改 mode 触发 Chmod 事件。
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&old_path).unwrap().permissions();
        permissions.set_mode(0o777);
        fs::set_permissions(&old_path, permissions).unwrap();
        wait_for(&watcher, &old_path, Chmod);
    }

    // 同目录重命名 → Rename；删除 → Remove。
    fs::rename(&old_path, &new_path).unwrap();
    wait_for(&watcher, &old_path, Rename);

    fs::remove_file(&new_path).unwrap();
    wait_for(&watcher, &new_path, Remove);

    // 重建后再跨监视目录移动 → Move。
    fs::write(&old_path, b"again").unwrap();
    wait_for(&watcher, &old_path, Create);

    fs::rename(&old_path, &moved_path).unwrap();
    wait_for(&watcher, &old_path, Move);
    watcher.Close();
}

/// Start 不可重复；Close 可幂等；关闭后 Add/Start 返回 Closed。
#[test]
fn migration_start_and_close_state_matches_go() {
    let watcher = NewWatcher();
    watcher.Start(Duration::from_millis(10)).unwrap();
    assert_eq!(
        watcher.Start(Duration::from_millis(10)),
        Err(WatcherError::Started)
    );
    watcher.Close();
    watcher.Close();
    assert_eq!(watcher.Add("missing"), Err(WatcherError::Closed));
    assert_eq!(
        watcher.Start(Duration::from_millis(10)),
        Err(WatcherError::Closed)
    );
}
