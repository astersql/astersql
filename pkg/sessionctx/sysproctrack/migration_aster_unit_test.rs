// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 系统进程跟踪迁移期单元测试。
//
// 用 `MockProc` / `MockTracker` 对照 Go：同 ID 幂等 Track、异对象冲突、
// UnTrack 清零 ConnectionID、进程列表过滤，以及 Kill 仅作用于已跟踪进程。

#![allow(non_snake_case)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex, RwLock};

use anyhow::anyhow;
use sessmgr::ProcessInfo;
use variable::session::SessionVars;

use super::{TrackError, TrackProc, TrackProcRef, Tracker};

/// 测试用进程：持有会话变量与可选的进程 ID 快照。
struct MockProc {
    vars: Mutex<SessionVars>,
    process_id: RwLock<Option<u64>>,
}

impl MockProc {
    /// 构造 mock 进程；`process_id` 为 `ShowProcess` 返回的快照 ID。
    fn new(process_id: Option<u64>) -> Self {
        Self {
            vars: Mutex::new(SessionVars::new()),
            process_id: RwLock::new(process_id),
        }
    }
}

impl TrackProc for MockProc {
    fn GetSessionVars(&self) -> &Mutex<SessionVars> {
        &self.vars
    }

    fn ShowProcess(&self) -> Option<Arc<ProcessInfo>> {
        self.process_id.read().unwrap().map(|id| {
            let mut info = ProcessInfo::default();
            info.ID = id;
            Arc::new(info)
        })
    }
}

#[derive(Default)]
/// 测试用 Tracker：内存登记进程，并记录 Kill 调用过的 ID。
struct MockTracker {
    processes: RwLock<HashMap<u64, TrackProcRef>>,
    killed: Mutex<Vec<u64>>,
}

impl Tracker for MockTracker {
    fn Track(&self, id: u64, proc: TrackProcRef) -> Result<(), TrackError> {
        let mut processes = self.processes.write().unwrap();
        // 同一 ID 仅允许同一 Arc 对象幂等重入；不同对象则报「ID 已被占用」。
        if let Some(old_proc) = processes.get(&id) {
            if !Arc::ptr_eq(old_proc, &proc) {
                return Err(anyhow!("The ID is in use: {id}"));
            }
        }
        // 登记成功后把会话 ConnectionID 同步为跟踪 ID。
        proc.GetSessionVars().lock().unwrap().ConnectionID = id;
        processes.insert(id, proc);
        Ok(())
    }

    fn UnTrack(&self, id: u64) {
        if let Some(proc) = self.processes.write().unwrap().remove(&id) {
            proc.GetSessionVars().lock().unwrap().ConnectionID = 0;
        }
    }

    fn GetSysProcessList(&self) -> HashMap<u64, Arc<ProcessInfo>> {
        self.processes
            .read()
            .unwrap()
            .iter()
            // 跳过无快照或快照 ID 与登记 ID 不一致的进程。
            .filter_map(|(&id, proc)| {
                proc.ShowProcess()
                    .filter(|process| process.ID == id)
                    .map(|process| (id, process))
            })
            .collect()
    }

    fn KillSysProcess(&self, id: u64) {
        if self.processes.read().unwrap().contains_key(&id) {
            self.killed.lock().unwrap().push(id);
        }
    }
}

/// 验证 Track 幂等/冲突、ConnectionID 赋值与 UnTrack 清零。
#[test]
fn track_preserves_identity_and_connection_id_rules() {
    let tracker = MockTracker::default();
    let first: TrackProcRef = Arc::new(MockProc::new(Some(7)));
    let replacement: TrackProcRef = Arc::new(MockProc::new(Some(7)));

    tracker.Track(7, first.clone()).unwrap();
    tracker.Track(7, first.clone()).unwrap();
    assert_eq!(first.GetSessionVars().lock().unwrap().ConnectionID, 7);

    let error = tracker.Track(7, replacement).unwrap_err();
    assert_eq!(error.to_string(), "The ID is in use: 7");

    tracker.UnTrack(7);
    assert_eq!(first.GetSessionVars().lock().unwrap().ConnectionID, 0);
}

/// 验证进程列表跳过无快照与 ID 不匹配的条目。
#[test]
fn process_list_skips_nil_and_mismatched_snapshots() {
    let tracker = MockTracker::default();
    tracker.Track(1, Arc::new(MockProc::new(None))).unwrap();
    tracker.Track(2, Arc::new(MockProc::new(Some(99)))).unwrap();
    tracker.Track(3, Arc::new(MockProc::new(Some(3)))).unwrap();

    let processes = tracker.GetSysProcessList();
    assert_eq!(processes.len(), 1);
    assert_eq!(processes[&3].ID, 3);
}

/// 验证 Kill 只命中已跟踪进程，未跟踪 ID 被忽略。
#[test]
fn kill_only_targets_a_tracked_process() {
    let tracker = MockTracker::default();
    tracker
        .Track(11, Arc::new(MockProc::new(Some(11))))
        .unwrap();

    tracker.KillSysProcess(10);
    tracker.KillSysProcess(11);

    assert_eq!(*tracker.killed.lock().unwrap(), [11]);
}
