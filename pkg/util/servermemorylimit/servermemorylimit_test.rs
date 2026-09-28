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

// servermemorylimit Go 对照测试：验证内存治理历史环形缓冲。
//
// 不发送 kill 信号，也不读取真实 runtime 内存；时间戳使用 Rust SystemTime。

// 本文件对照 pkg/util/servermemorylimit/servermemorylimit_test.go 验证内存治理历史环形缓冲。
// 不会发送 kill 信号，也不会读取真实 runtime 内存；时间戳使用 Rust SystemTime。
//

use std::time::SystemTime;

// TestMemoryUsageOpsHistory 对应 Go 测试：先写入 3 条历史，再写入到超过 50 条，
// 验证 GetRows 的输出列映射、环形淘汰顺序和 offsets 值。
#[test]
/// 对应 Go 测试：写入并环绕淘汰后校验 GetRows 列映射与 offsets。
fn test_memory_usage_ops_history() {
    let _guard = crate::migration_aster_unit_test::TEST_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    GlobalMemoryOpsHistoryManager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .init();
    let mut info = sessmgr::ProcessInfo::default();
    let gen_info = |info: &mut sessmgr::ProcessInfo, i: i32| {
        // Go 复用同一个 ProcessInfo 指针并反复覆盖字段，recordOne 会在当次调用中转成 Datum 行。
        info.ID = i as u64;
        info.DB = (2 * i).to_string();
        info.User = (3 * i).to_string();
        info.Host = (4 * i).to_string();
        info.Digest = (5 * i).to_string();
        info.Info = (6 * i).to_string();
    };

    for i in 0..3 {
        gen_info(&mut info, i);
        GlobalMemoryOpsHistoryManager.lock().unwrap().recordOne(
            &info,
            SystemTime::now(),
            i as u64,
            (2 * i) as u64,
        );
    }

    let checkResult = |datums: &[types::Datum], i: i32| {
        // 列序与 servermemorylimit.rs 的 GetRows 保持一致：OPS、MEMORY_LIMIT、MEMORY_CURRENT、PROCESSID 等。
        assert_eq!(datums[1].GetString(), "SessionKill");
        assert_eq!(datums[2].GetInt64(), i as i64);
        assert_eq!(datums[3].GetInt64(), (2 * i) as i64);
        assert_eq!(datums[4].GetInt64(), i as i64);
        assert_eq!(datums[7].GetString(), (4 * i).to_string());
        assert_eq!(datums[8].GetString(), (2 * i).to_string());
        assert_eq!(datums[9].GetString(), (3 * i).to_string());
        assert_eq!(datums[10].GetString(), (5 * i).to_string());
        assert_eq!(datums[11].GetString(), (6 * i).to_string());
    };

    let mut rows = GlobalMemoryOpsHistoryManager.lock().unwrap().GetRows();
    assert_eq!(3, rows.len());
    for i in 0..3 {
        checkResult(&rows[i as usize], i);
    }

    // Test evict
    // 继续写入 50 条后，固定容量环形缓冲会淘汰最早的 0、1、2 三条记录。
    for i in 3..53 {
        gen_info(&mut info, i);
        GlobalMemoryOpsHistoryManager.lock().unwrap().recordOne(
            &info,
            SystemTime::now(),
            i as u64,
            (2 * i) as u64,
        );
    }
    rows = GlobalMemoryOpsHistoryManager.lock().unwrap().GetRows();
    assert_eq!(50, rows.len());
    for i in 3..53 {
        checkResult(&rows[(i - 3) as usize], i);
    }
    // 写入 53 次、容量 50，下一次写入位置应回到下标 3。
    assert_eq!(GlobalMemoryOpsHistoryManager.lock().unwrap().offsets, 3);
    GlobalMemoryOpsHistoryManager
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .init();
}
