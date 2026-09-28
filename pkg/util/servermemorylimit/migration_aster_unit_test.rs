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

// servermemorylimit 迁移回归测试：历史环形缓冲与 kill 路径。
//
// 用假 ProcessInfoProvider 与可控 Tracker，验证超限 kill、低于会话最小值
// 清理 top tracker，以及超时强制 FinishResultSet 等与 Go 对齐的行为。

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};

use crate::{memory, sessmgr, sqlkiller, types};

/// 串行化共享全局状态的测试互斥锁。
pub(crate) static TEST_LOCK: Mutex<()> = Mutex::new(());

/// 可注入的假进程信息提供者，供 kill 路径单测使用。
#[derive(Default)]
struct FakeProcessInfoProvider {
    info: Mutex<Option<Arc<sessmgr::ProcessInfo>>>,
}

impl ProcessInfoProvider for FakeProcessInfoProvider {
    fn get_process_info(&self, id: u64) -> Option<Arc<sessmgr::ProcessInfo>> {
        self.info
            .lock()
            .expect("fake process-info lock poisoned")
            .as_ref()
            .filter(|info| info.ID == id)
            .cloned()
    }
}

/// 重置包级全局观测指标与 top tracker，避免用例互相污染。
fn reset_globals() {
    MemoryMaxUsed.Store(0);
    SessionKillLast.Store(SystemTime::UNIX_EPOCH);
    SessionKillTotal.Store(0);
    IsKilling.Store(false);
    memory::ServerMemoryLimitSessMinSize.Store(128 << 20);
    memory::MemUsageTop1Tracker.store(ptr::null_mut(), std::sync::atomic::Ordering::SeqCst);
    GlobalMemoryOpsHistoryManager
        .lock()
        .expect("history lock poisoned")
        .init();
}

/// 校验历史行若干列是否与构造时的 `i` 缩放字段一致。
fn assert_history_row(datums: &[types::Datum], i: i32) {
    assert_eq!(datums[1].GetString(), "SessionKill");
    assert_eq!(datums[2].GetInt64(), i as i64);
    assert_eq!(datums[3].GetInt64(), (2 * i) as i64);
    assert_eq!(datums[4].GetUint64(), i as u64);
    assert_eq!(datums[7].GetString(), (4 * i).to_string());
    assert_eq!(datums[8].GetString(), (2 * i).to_string());
    assert_eq!(datums[9].GetString(), (3 * i).to_string());
    assert_eq!(datums[10].GetString(), (5 * i).to_string());
    assert_eq!(datums[11].GetString(), (6 * i).to_string());
}

/// 写入超过容量后，环形缓冲只保留最近 50 条且 offsets 正确。
#[test]
fn memory_usage_ops_history_matches_go_ring_and_columns() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    let mut manager = memoryOpsHistoryManager::default();
    let mut info = sessmgr::ProcessInfo::default();

    for i in 0..53 {
        info.ID = i as u64;
        info.DB = (2 * i).to_string();
        info.User = (3 * i).to_string();
        info.Host = (4 * i).to_string();
        info.Digest = (5 * i).to_string();
        info.Info = (6 * i).to_string();
        manager.recordOne(
            &info,
            SystemTime::UNIX_EPOCH + Duration::from_secs(i as u64 + 1),
            i as u64,
            (2 * i) as u64,
        );
    }

    let rows = manager.GetRows();
    assert_eq!(rows.len(), 50);
    for i in 3..53 {
        assert_history_row(&rows[(i - 3) as usize], i);
    }
    assert_eq!(manager.offsets, 3);
}

/// 超限会话收到 kill 并记入历史；进程信息消失后状态复位。
#[test]
fn over_limit_session_receives_kill_and_history_then_resets_when_gone() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    reset_globals();
    memory::ServerMemoryLimitSessMinSize.Store(1);

    let mut tracker = memory::NewTracker(1, -1);
    tracker.SessionID.Store(42);
    tracker.Killer = Some(Box::new(sqlkiller::SQLKiller::default()));
    tracker.Consume(16);
    let tracker: Arc<memory::Tracker> = Arc::from(tracker);
    let tracker_ptr = Arc::as_ptr(&tracker) as *mut memory::Tracker;
    memory::MemUsageTop1Tracker.store(tracker_ptr, std::sync::atomic::Ordering::SeqCst);

    let mut info = sessmgr::ProcessInfo::default();
    info.ID = 42;
    info.Time = SystemTime::now();
    info.Info = "select * from t".to_owned();
    info.Digest = "digest".to_owned();
    info.MemTracker = Some(tracker.clone());
    let provider = FakeProcessInfoProvider {
        info: Mutex::new(Some(Arc::new(info))),
    };
    let mut state = sessionToBeKilled::default();

    killSessIfNeeded(&mut state, 1, &provider);

    assert!(state.isKilling);
    assert_eq!(state.sessionID, 42);
    assert_eq!(
        tracker.Killer.as_ref().unwrap().GetKillSignal(),
        sqlkiller::ServerMemoryExceeded
    );
    assert!(IsKilling.Load());
    assert_eq!(SessionKillTotal.Load(), 1);
    assert_eq!(
        GlobalMemoryOpsHistoryManager
            .lock()
            .expect("history lock poisoned")
            .GetRows()
            .len(),
        1
    );

    *provider
        .info
        .lock()
        .expect("fake process-info lock poisoned") = None;
    killSessIfNeeded(&mut state, 1, &provider);
    assert!(!state.isKilling);
    assert!(!IsKilling.Load());

    reset_globals();
}

/// 内存低于会话最小值时清空 top tracker，且不发起 kill。
#[test]
fn below_session_minimum_clears_top_tracker_without_killing() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    reset_globals();
    memory::ServerMemoryLimitSessMinSize.Store(8);

    let tracker = memory::NewTracker(1, -1);
    tracker.SessionID.Store(7);
    tracker.Consume(4);
    let tracker: Arc<memory::Tracker> = Arc::from(tracker);
    let tracker_ptr = Arc::as_ptr(&tracker) as *mut memory::Tracker;
    memory::MemUsageTop1Tracker.store(tracker_ptr, std::sync::atomic::Ordering::SeqCst);
    let provider = FakeProcessInfoProvider::default();
    let mut state = sessionToBeKilled::default();

    killSessIfNeeded(&mut state, 1, &provider);

    assert!(
        memory::MemUsageTop1Tracker
            .load(std::sync::atomic::Ordering::SeqCst)
            .is_null()
    );
    assert!(!state.isKilling);
    assert_eq!(SessionKillTotal.Load(), 0);

    reset_globals();
}

/// 同一 SQL 卡在 kill 超过约 60 秒时，强制调用 FinishResultSet。
#[test]
fn same_sql_stuck_for_sixty_seconds_forces_result_set_finish() {
    let _guard = TEST_LOCK.lock().expect("test lock poisoned");
    reset_globals();

    let finished = Arc::new(AtomicBool::new(false));
    let finished_by_callback = finished.clone();
    let mut tracker = memory::NewTracker(1, -1);
    let killer = sqlkiller::SQLKiller::default();
    killer.SetFinishFunc(Box::new(move || {
        finished_by_callback.store(true, Ordering::SeqCst);
    }));
    tracker.Killer = Some(Box::new(killer));
    let tracker: Arc<memory::Tracker> = Arc::from(tracker);
    let tracker_ptr = Arc::as_ptr(&tracker) as *mut memory::Tracker;

    let sql_start = SystemTime::now() - Duration::from_secs(120);
    let mut info = sessmgr::ProcessInfo::default();
    info.ID = 99;
    info.Time = sql_start;
    info.MemTracker = Some(tracker.clone());
    let provider = FakeProcessInfoProvider {
        info: Mutex::new(Some(Arc::new(info))),
    };
    let now = SystemTime::now();
    let mut state = sessionToBeKilled {
        isKilling: true,
        sqlStartTime: Some(sql_start),
        sessionID: 99,
        sessionTracker: tracker_ptr,
        killStartTime: Some(now - Duration::from_secs(61)),
        lastLogTime: Some(now - Duration::from_secs(6)),
    };

    killSessIfNeeded(&mut state, 0, &provider);

    assert!(finished.load(Ordering::SeqCst));
    assert!(!state.isKilling);
    reset_globals();
}
