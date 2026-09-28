// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// infoschema 错误统计深拷贝安全性测试。
//
// 对应 Go `infoschema_test.go` 的 copy 语义：先构造一批增量并取出
// Global/User/Host 快照，再继续增量；断言快照不受后续写入影响，
// 而 live 统计反映最新状态。与 `errname_2_aster_unit_test` 共享全局
// 统计，故通过 `lock_stats_test` 串行化。

use crate::infoschema::{
    FlushStats, GlobalStats, HostStats, IncrementError, IncrementWarning, UserStats,
    incrementWithClock,
};
use std::time::{Duration, UNIX_EPOCH};

/// Go 首次增量先采集 LastSeen，再在 initCounters 中采集 FirstSeen。
#[test]
fn first_increment_uses_go_timestamp_sampling_order() {
    let _guard = crate::errname_2_aster_unit_test::lock_stats_test();
    FlushStats();

    let last_seen = UNIX_EPOCH + Duration::from_secs(1);
    let first_seen = UNIX_EPOCH + Duration::from_secs(2);
    let mut samples = [last_seen, first_seen].into_iter();
    incrementWithClock(123, "user", "host", false, || {
        samples.next().expect("exactly two timestamps are sampled")
    });
    let summary = GlobalStats().remove(&123).expect("counter must exist");

    assert_eq!(summary.FirstSeen, first_seen);
    assert_eq!(summary.LastSeen, last_seen);
    assert!(samples.next().is_none());
    FlushStats();
}

/// 验证 Global/User/Host 快照为深拷贝，且 live 统计随增量变化。
#[test]
fn test_copy_safety() {
    let _guard = crate::errname_2_aster_unit_test::lock_stats_test();
    FlushStats();
    // 第一批增量用于构造快照；Go 测试随后取得 Global/User/Host 三份 copy。
    IncrementError(123, "user", "host");
    IncrementError(321, "user2", "host2");
    IncrementWarning(123, "user", "host");
    IncrementWarning(999, "user", "host");
    IncrementWarning(222, "u", "h");

    let global_copy = GlobalStats();
    let user_copy = UserStats();
    let host_copy = HostStats();

    // 第二批增量发生在 copy 之后，用于验证 copy 不会随 stats 继续变化。
    IncrementError(123, "user", "host");
    IncrementError(999, "user2", "host2");
    IncrementError(123, "user3", "host");
    IncrementWarning(123, "user", "host");
    IncrementWarning(222, "u", "h");
    IncrementWarning(222, "a", "b");
    IncrementWarning(333, "c", "d");

    let global_live = GlobalStats();
    let user_live = UserStats();
    let host_live = HostStats();

    // global stats：全局 live stats 已累计 3 次 123 error，copy 仍停在第一次快照。
    assert_eq!(3, global_live[&123].ErrorCount);
    assert_eq!(1, global_copy[&123].ErrorCount);

    // user stats：Go 测试检查 live map 与 copy map 的用户数量和 user 维度计数。
    assert_eq!(6, user_live.len());
    assert_eq!(3, user_copy.len());
    assert_eq!(2, user_live["user"][&123].ErrorCount);
    assert_eq!(2, user_live["user"][&123].WarningCount);
    assert_eq!(1, user_copy["user"][&123].ErrorCount);
    assert_eq!(1, user_copy["user"][&123].WarningCount);

    // copy 中不应出现快照之后新增的 user3/a，但 live stats 中应存在。
    assert!(!user_copy.contains_key("user3"));
    assert!(user_live.contains_key("user3"));
    assert!(!user_copy.contains_key("a"));
    assert!(user_live.contains_key("a"));

    // host stats：先验证第二批增量后的 live/copy 长度，再额外新增 newhost。
    assert_eq!(5, host_live.len());
    assert_eq!(3, host_copy.len());

    IncrementError(123, "user3", "newhost");
    let host_live = HostStats();
    assert_eq!(6, host_live.len());
    assert_eq!(3, host_copy.len());

    // copy 中不应出现快照之后新增的 newhost/b，但 live stats 中应存在。
    assert!(!host_copy.contains_key("newhost"));
    assert!(host_live.contains_key("newhost"));
    assert!(!host_copy.contains_key("b"));
    assert!(host_live.contains_key("b"));

    FlushStats();
}
