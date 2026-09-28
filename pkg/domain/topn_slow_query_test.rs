// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

use crate::topn_slow_query::{ExecDetails, ShowSlowKind, SlowQueryInfo, TopNSlowQueries};
use std::time::{Duration, SystemTime};

fn query_at(sql: &str, start: SystemTime, duration: Duration, internal: bool) -> SlowQueryInfo {
    SlowQueryInfo {
        sql: sql.into(),
        start,
        duration,
        detail: ExecDetails::default(),
        connection_id: 1,
        session_alias: String::new(),
        transaction_ts: 0,
        user: "root".into(),
        database: "test".into(),
        table_ids: String::new(),
        index_names: String::new(),
        digest: sql.into(),
        internal,
        success: true,
    }
}

fn query(sql: &str, millis: u64, internal: bool) -> SlowQueryInfo {
    query_at(
        sql,
        SystemTime::now(),
        Duration::from_millis(millis),
        internal,
    )
}

fn durations(top: &TopNSlowQueries, count: usize, kind: ShowSlowKind) -> Vec<Duration> {
    top.query_top(count, kind)
        .iter()
        .map(|info| info.duration)
        .collect()
}

#[test]
fn top_n_matches_go_heap_replacement_rules() {
    let top = TopNSlowQueries::new(10, Duration::ZERO, 10);
    for millis in (300..=1_200).step_by(100) {
        assert!(top.append(query("", millis, false)));
    }
    assert_eq!(
        durations(&top, 10, ShowSlowKind::Default),
        (300..=1_200)
            .rev()
            .step_by(100)
            .map(Duration::from_millis)
            .collect::<Vec<_>>()
    );

    for millis in (1_300..=2_100).step_by(100) {
        assert!(top.append(query("", millis, false)));
    }
    assert!(top.append(query("", 1_500, false)));
    assert_eq!(
        durations(&top, 10, ShowSlowKind::Default),
        vec![
            2_100, 2_000, 1_900, 1_800, 1_700, 1_600, 1_500, 1_500, 1_400, 1_300,
        ]
        .into_iter()
        .map(Duration::from_millis)
        .collect::<Vec<_>>()
    );

    assert!(top.append(query("", 1_200, false)));
    assert!(top.append(query("", 666, false)));
    assert_eq!(
        durations(&top, 10, ShowSlowKind::Default).last().copied(),
        Some(Duration::from_millis(1_300))
    );
}

#[test]
fn remove_expired_matches_go_strict_after_boundary() {
    let now = SystemTime::now();
    let top = TopNSlowQueries::new(6, Duration::from_secs(3), 10);
    for (offset, nanos) in [(0, 6), (1, 5), (2, 4), (3, 3), (4, 2)] {
        assert!(top.append(query_at(
            "",
            now + Duration::from_secs(offset),
            Duration::from_nanos(nanos),
            false,
        )));
    }

    top.remove_expired(now + Duration::from_secs(5));
    assert_eq!(
        durations(&top, 6, ShowSlowKind::Default),
        vec![Duration::from_nanos(3), Duration::from_nanos(2)]
    );

    for (offset, nanos) in [(3, 3), (4, 2), (5, 1), (6, 0)] {
        assert!(top.append(query_at(
            "",
            now + Duration::from_secs(offset),
            Duration::from_nanos(nanos),
            false,
        )));
    }
    top.remove_expired(now + Duration::from_secs(6));
    assert_eq!(
        durations(&top, 6, ShowSlowKind::Default),
        vec![
            Duration::from_nanos(2),
            Duration::from_nanos(2),
            Duration::from_nanos(1),
            Duration::ZERO,
        ]
    );
}

#[test]
fn recent_queue_matches_go_order_and_capacity() {
    let top = TopNSlowQueries::new(10, Duration::from_secs(60), 5);
    for sql in ["aaa", "bbb", "ccc"] {
        assert!(top.append(query(sql, 0, false)));
    }
    assert_eq!(
        top.query_recent(6)
            .iter()
            .map(|info| info.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["ccc", "bbb", "aaa"]
    );
    for sql in ["ddd", "eee", "fff", "ggg"] {
        assert!(top.append(query(sql, 0, false)));
    }
    assert_eq!(
        top.query_recent(6)
            .iter()
            .map(|info| info.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["ggg", "fff", "eee", "ddd", "ccc"]
    );
    assert_eq!(
        top.query_all()
            .iter()
            .map(|info| info.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["ccc", "ddd", "eee", "fff", "ggg"]
    );
}

/// 用户与内部 Top 分离；recent 为新→旧；close 后 append 失败。
#[test]
fn canonical_topn_slow_queries_separates_internal_and_recent_then_closes() {
    // top_n=2、队列容量=2：第三条用户慢查询会挤出 recent 中最旧项。
    let top = TopNSlowQueries::new(2, Duration::from_secs(60), 2);
    assert!(top.append(query("fast", 1, false)));
    assert!(top.append(query("slow", 20, false)));
    assert!(top.append(query("internal", 30, true)));
    // recent 最新两条应为 internal、slow；用户 Top1 为 slow，内部 Top1 为 internal。
    assert_eq!(
        top.query_recent(2)
            .iter()
            .map(|q| q.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["internal", "slow"]
    );
    assert_eq!(top.query_top(1, ShowSlowKind::Default)[0].sql, "slow");
    assert_eq!(top.query_top(1, ShowSlowKind::Internal)[0].sql, "internal");
    assert_eq!(
        top.query_top(2, ShowSlowKind::All)
            .iter()
            .map(|q| q.sql.as_str())
            .collect::<Vec<_>>(),
        vec!["internal", "slow"]
    );
    // 关闭后不应再接受新慢查询。
    top.close();
    assert!(!top.append(query("late", 40, false)));
}
