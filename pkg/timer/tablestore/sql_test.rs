// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

// Timer 表存储 SQL 层的单元测试。
//
// 覆盖定时器记录的增删改查 SQL 与参数顺序、条件树和扩展 JSON 的编码兼容性，
// 并通过按调用顺序校验的会话桩测试时区切换、资源清理、事务及通知器关闭语义。

use crate::{
    EtcdClient, EtcdNotifyEvent, NewEtcdNotifier, SqlArg, SqlResult, SqlRow, TableTimerStoreCore,
    buildCondCriteria, buildDeleteTimerSQL, buildInsertTimerSQL, buildSelectTimerSQL,
    buildUpdateCriteria, buildUpdateTimerSQL, decode_timer_ext, executeSQL, runInTxn,
};
use astersql_session_syssession as syssession;
use astersql_timer_api as api;
use std::any::Any;
use std::collections::VecDeque;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

// 以当前时间为锚点构造 Unix 秒，避免直接依赖具体时区表示。
fn timestamp(unix: i64) -> api::Timestamp {
    let now = api::now_timestamp();
    let current = now.timestamp();
    if unix >= current {
        now + Duration::from_secs((unix - current) as u64)
    } else {
        now - Duration::from_secs((current - unix) as u64)
    }
}

// 构造指定时区偏移的时间值，用于模拟数据库会话返回的时间戳。
fn timestamp_in_location(unix: i64, location: &str) -> api::Timestamp {
    let value = timestamp(unix);
    match api::parse_location(location).unwrap() {
        api::TimerLocation::Named(location) => value.with_timezone(&location).fixed_offset(),
        api::TimerLocation::Fixed(location) => value.with_timezone(&location),
    }
}

// SQL 使用 `%?` 作为内部绑定占位符；参数数量必须始终与之匹配。
fn placeholder_count(sql: &str) -> usize {
    sql.matches("%?").count()
}

fn arc_cond(cond: impl api::Cond + 'static) -> Arc<dyn api::Cond> {
    Arc::new(cond)
}

// 同时验证从空参数和已有参数开始拼接条件，防止构造器覆盖调用方参数。
fn assert_cond(
    cond: Option<Arc<dyn api::Cond>>,
    expected_criteria: &str,
    expected_args: Vec<SqlArg>,
) {
    assert_eq!(placeholder_count(expected_criteria), expected_args.len());
    let (criteria, args) = buildCondCriteria(cond.as_deref(), vec![]).unwrap();
    assert_eq!(criteria, expected_criteria);
    assert_eq!(args, expected_args);

    let prefix = vec![SqlArg::String("a".into()), SqlArg::String("b".into())];
    let (criteria, args) = buildCondCriteria(cond.as_deref(), prefix.clone()).unwrap();
    assert_eq!(criteria, expected_criteria);
    assert_eq!(args, [prefix, expected_args].concat());
}

#[test]
fn test_build_insert_timer_sql() {
    let now = api::now_timestamp();
    let sql_with_time = concat!(
        "INSERT INTO `db1`.`t1` (NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, ",
        "SCHED_POLICY_TYPE, SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, ",
        "EVENT_ID, EVENT_STATUS, EVENT_START, EVENT_DATA, SUMMARY_DATA, VERSION) ",
        "VALUES (%?, %?, %?, %?, %?, %?, %?, FROM_UNIXTIME(%?), %?, ",
        "JSON_MERGE_PATCH('{}', %?), %?, %?, FROM_UNIXTIME(%?), %?, %?, 1)"
    );
    let record = api::TimerRecord {
        TimerSpec: api::TimerSpec {
            Namespace: "n1".into(),
            Key: "k1".into(),
            Data: b"data1".to_vec(),
            TimeZone: "Asia/Shanghai".into(),
            SchedPolicyType: api::SchedEventInterval.into(),
            SchedPolicyExpr: "1h".into(),
            HookClass: "h1".into(),
            Watermark: Some(now),
            Enable: true,
            Tags: vec!["l1".into(), "l2".into()],
        },
        ManualRequest: api::ManualRequest {
            ManualRequestID: "req1".into(),
            ManualRequestTime: Some(timestamp(123)),
            ManualTimeout: Duration::from_secs(60),
            ManualProcessed: true,
            ManualEventID: "event1".into(),
        },
        EventExtra: api::EventExtra {
            EventManualRequestID: "req1".into(),
            EventWatermark: Some(timestamp(456)),
        },
        EventID: "e1".into(),
        EventStatus: api::SchedEventTrigger.into(),
        EventStart: Some(now + Duration::from_secs(1)),
        EventData: b"event1".to_vec(),
        SummaryData: b"summary1".to_vec(),
        ..api::TimerRecord::default()
    };
    let expected =
        vec![
        SqlArg::String("n1".into()),
        SqlArg::String("k1".into()),
        SqlArg::Bytes(b"data1".to_vec()),
        SqlArg::String("Asia/Shanghai".into()),
        SqlArg::String("INTERVAL".into()),
        SqlArg::String("1h".into()),
        SqlArg::String("h1".into()),
        SqlArg::I64(now.timestamp()),
        SqlArg::Bool(true),
        SqlArg::Json(concat!(
            "{\"tags\":[\"l1\",\"l2\"],",
            "\"manual\":{\"request_id\":\"req1\",\"request_time_unix\":123,\"timeout_sec\":60,",
            "\"processed\":true,\"event_id\":\"event1\"},",
            "\"event\":{\"manual_request_id\":\"req1\",\"watermark_unix\":456}}"
        ).into()),
        SqlArg::String("e1".into()),
        SqlArg::String("TRIGGER".into()),
        SqlArg::I64(now.timestamp() + 1),
        SqlArg::Bytes(b"event1".to_vec()),
        SqlArg::Bytes(b"summary1".to_vec()),
    ];
    let (sql, args) = buildInsertTimerSQL("db1", "t1", &record).unwrap();
    assert_eq!(sql, sql_with_time);
    assert_eq!(placeholder_count(&sql), args.len());
    assert_eq!(args, expected);

    let record = api::TimerRecord {
        TimerSpec: api::TimerSpec {
            Namespace: "n1".into(),
            Key: "k1".into(),
            SchedPolicyType: api::SchedEventInterval.into(),
            SchedPolicyExpr: "1h".into(),
            ..api::TimerSpec::default()
        },
        ..api::TimerRecord::default()
    };
    let (sql, args) = buildInsertTimerSQL("db1", "t1", &record).unwrap();
    assert_eq!(
        sql,
        concat!(
            "INSERT INTO `db1`.`t1` (NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, ",
            "SCHED_POLICY_TYPE, SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, ",
            "EVENT_ID, EVENT_STATUS, EVENT_START, EVENT_DATA, SUMMARY_DATA, VERSION) ",
            "VALUES (%?, %?, %?, %?, %?, %?, %?, %?, %?, ",
            "JSON_MERGE_PATCH('{}', %?), %?, %?, %?, %?, %?, 1)"
        )
    );
    assert_eq!(
        args,
        vec![
            SqlArg::String("n1".into()),
            SqlArg::String("k1".into()),
            SqlArg::Bytes(vec![]),
            SqlArg::String(String::new()),
            SqlArg::String("INTERVAL".into()),
            SqlArg::String("1h".into()),
            SqlArg::String(String::new()),
            SqlArg::Null,
            SqlArg::Bool(false),
            SqlArg::Json("{}".into()),
            SqlArg::String(String::new()),
            SqlArg::String("IDLE".into()),
            SqlArg::Null,
            SqlArg::Bytes(vec![]),
            SqlArg::Bytes(vec![]),
        ]
    );
}

#[test]
fn test_build_cond_criteria() {
    assert_cond(None, "1", vec![]);
    assert_cond(Some(arc_cond(api::TimerCond::default())), "1", vec![]);
    assert_cond(
        Some(arc_cond(api::TimerCond {
            ID: api::NewOptionalVal("1".into()),
            ..api::TimerCond::default()
        })),
        "ID = %?",
        vec![SqlArg::String("1".into())],
    );
    assert_cond(
        Some(arc_cond(api::TimerCond {
            Namespace: api::NewOptionalVal("ns1".into()),
            ..api::TimerCond::default()
        })),
        "NAMESPACE = %?",
        vec![SqlArg::String("ns1".into())],
    );
    assert_cond(
        Some(arc_cond(api::TimerCond {
            Key: api::NewOptionalVal("key1".into()),
            ..api::TimerCond::default()
        })),
        "TIMER_KEY = %?",
        vec![SqlArg::String("key1".into())],
    );
    assert_cond(
        Some(arc_cond(api::TimerCond {
            Key: api::NewOptionalVal("key1".into()),
            KeyPrefix: true,
            ..api::TimerCond::default()
        })),
        "TIMER_KEY LIKE %?",
        vec![SqlArg::String("key1%".into())],
    );
    let namespace_key = || api::TimerCond {
        Namespace: api::NewOptionalVal("ns1".into()),
        Key: api::NewOptionalVal("key1".into()),
        ..api::TimerCond::default()
    };
    assert_cond(
        Some(arc_cond(namespace_key())),
        "NAMESPACE = %? AND TIMER_KEY = %?",
        vec![SqlArg::String("ns1".into()), SqlArg::String("key1".into())],
    );
    assert_cond(
        Some(arc_cond(api::TimerCond {
            KeyPrefix: true,
            ..namespace_key()
        })),
        "NAMESPACE = %? AND TIMER_KEY LIKE %?",
        vec![SqlArg::String("ns1".into()), SqlArg::String("key1%".into())],
    );
    assert_cond(
        Some(arc_cond(api::TimerCond {
            Tags: api::NewOptionalVal(vec![]),
            ..api::TimerCond::default()
        })),
        "1",
        vec![],
    );
    for tags in [vec!["l1"], vec!["l1", "l2"]] {
        let tags = tags.into_iter().map(str::to_string).collect::<Vec<_>>();
        let json = format!(
            "[{}]",
            tags.iter()
                .map(|tag| format!("\"{tag}\""))
                .collect::<Vec<_>>()
                .join(",")
        );
        assert_cond(
            Some(arc_cond(api::TimerCond {
                Tags: api::NewOptionalVal(tags),
                ..api::TimerCond::default()
            })),
            "JSON_EXTRACT(TIMER_EXT, '$.tags') IS NOT NULL AND JSON_CONTAINS((TIMER_EXT->'$.tags'), %?)",
            vec![SqlArg::Json(json)],
        );
    }
    let id2 = || {
        arc_cond(api::TimerCond {
            ID: api::NewOptionalVal("2".into()),
            ..api::TimerCond::default()
        })
    };
    assert_cond(
        Some(arc_cond(api::And(vec![arc_cond(namespace_key()), id2()]))),
        "(NAMESPACE = %? AND TIMER_KEY = %?) AND (ID = %?)",
        vec![
            SqlArg::String("ns1".into()),
            SqlArg::String("key1".into()),
            SqlArg::String("2".into()),
        ],
    );
    assert_cond(
        Some(arc_cond(api::And(vec![
            arc_cond(api::TimerCond::default()),
            id2(),
        ]))),
        "1 AND (ID = %?)",
        vec![SqlArg::String("2".into())],
    );
    assert_cond(
        Some(arc_cond(api::And(vec![
            arc_cond(api::Not(arc_cond(api::TimerCond::default()))),
            id2(),
        ]))),
        "0 AND (ID = %?)",
        vec![SqlArg::String("2".into())],
    );
    let ns = || {
        arc_cond(api::TimerCond {
            Namespace: api::NewOptionalVal("ns1".into()),
            ..api::TimerCond::default()
        })
    };
    assert_cond(
        Some(arc_cond(api::And(vec![
            ns(),
            arc_cond(api::TimerCond::default()),
            id2(),
        ]))),
        "(NAMESPACE = %?) AND 1 AND (ID = %?)",
        vec![SqlArg::String("ns1".into()), SqlArg::String("2".into())],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(api::And(vec![
            arc_cond(namespace_key()),
            id2(),
        ]))))),
        "!((NAMESPACE = %? AND TIMER_KEY = %?) AND (ID = %?))",
        vec![
            SqlArg::String("ns1".into()),
            SqlArg::String("key1".into()),
            SqlArg::String("2".into()),
        ],
    );
    assert_cond(
        Some(arc_cond(api::Or(vec![arc_cond(namespace_key()), id2()]))),
        "(NAMESPACE = %? AND TIMER_KEY = %?) OR (ID = %?)",
        vec![
            SqlArg::String("ns1".into()),
            SqlArg::String("key1".into()),
            SqlArg::String("2".into()),
        ],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(api::Or(vec![
            arc_cond(namespace_key()),
            id2(),
        ]))))),
        "!((NAMESPACE = %? AND TIMER_KEY = %?) OR (ID = %?))",
        vec![
            SqlArg::String("ns1".into()),
            SqlArg::String("key1".into()),
            SqlArg::String("2".into()),
        ],
    );
    assert_cond(
        Some(arc_cond(api::Or(vec![
            arc_cond(api::TimerCond::default()),
            id2(),
        ]))),
        "1 OR (ID = %?)",
        vec![SqlArg::String("2".into())],
    );
    assert_cond(
        Some(arc_cond(api::Or(vec![
            ns(),
            arc_cond(api::TimerCond::default()),
            id2(),
        ]))),
        "(NAMESPACE = %?) OR 1 OR (ID = %?)",
        vec![SqlArg::String("ns1".into()), SqlArg::String("2".into())],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(api::TimerCond {
            ID: api::NewOptionalVal("3".into()),
            ..api::TimerCond::default()
        })))),
        "!(ID = %?)",
        vec![SqlArg::String("3".into())],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(api::TimerCond::default())))),
        "0",
        vec![],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(api::Not(arc_cond(
            api::TimerCond::default(),
        )))))),
        "1",
        vec![],
    );
    assert_cond(
        Some(arc_cond(api::Not(arc_cond(namespace_key())))),
        "!(NAMESPACE = %? AND TIMER_KEY = %?)",
        vec![SqlArg::String("ns1".into()), SqlArg::String("key1".into())],
    );
}

#[test]
fn test_build_select_timer_sql() {
    let prefix = concat!(
        "SELECT ID, NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, SCHED_POLICY_TYPE, ",
        "SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, EVENT_STATUS, EVENT_ID, ",
        "EVENT_DATA, EVENT_START, SUMMARY_DATA, CREATE_TIME, UPDATE_TIME, VERSION FROM `db1`.`t1`"
    );
    let cases: Vec<(Option<Arc<dyn api::Cond>>, &str, Vec<SqlArg>)> = vec![
        (None, " WHERE 1", vec![]),
        (
            Some(arc_cond(api::TimerCond {
                ID: api::NewOptionalVal("2".into()),
                ..api::TimerCond::default()
            })),
            " WHERE ID = %?",
            vec![SqlArg::String("2".into())],
        ),
        (
            Some(arc_cond(api::TimerCond {
                Namespace: api::NewOptionalVal("ns1".into()),
                Key: api::NewOptionalVal("key1".into()),
                ..api::TimerCond::default()
            })),
            " WHERE NAMESPACE = %? AND TIMER_KEY = %?",
            vec![SqlArg::String("ns1".into()), SqlArg::String("key1".into())],
        ),
        (
            Some(arc_cond(api::Or(vec![
                arc_cond(api::TimerCond {
                    ID: api::NewOptionalVal("3".into()),
                    ..api::TimerCond::default()
                }),
                arc_cond(api::TimerCond {
                    Namespace: api::NewOptionalVal("ns1".into()),
                    ..api::TimerCond::default()
                }),
            ]))),
            " WHERE (ID = %?) OR (NAMESPACE = %?)",
            vec![SqlArg::String("3".into()), SqlArg::String("ns1".into())],
        ),
    ];
    for (cond, suffix, expected_args) in cases {
        let (sql, args) = buildSelectTimerSQL("db1", "t1", cond.as_deref()).unwrap();
        assert_eq!(sql, format!("{prefix}{suffix}"));
        assert_eq!(placeholder_count(&sql), args.len());
        assert_eq!(args, expected_args);
    }
}

// 更新字段也必须追加到已有参数之后，顺序与生成的 SET 子句一致。
fn assert_update(update: &api::TimerUpdate, expected: &str, expected_args: Vec<SqlArg>) {
    assert_eq!(placeholder_count(expected), expected_args.len());
    let (criteria, args) = buildUpdateCriteria(update, vec![]).unwrap();
    assert_eq!(criteria, expected);
    assert_eq!(args, expected_args);

    let prefix = vec![
        SqlArg::I64(1),
        SqlArg::String("2".into()),
        SqlArg::String("3".into()),
    ];
    let (criteria, args) = buildUpdateCriteria(update, prefix.clone()).unwrap();
    assert_eq!(criteria, expected);
    assert_eq!(args, [prefix, expected_args].concat());
}

#[test]
fn test_build_update_criteria() {
    assert_update(
        &api::TimerUpdate::default(),
        "VERSION = VERSION + 1",
        vec![],
    );
    assert_update(
        &api::TimerUpdate {
            Enable: api::NewOptionalVal(true),
            ..api::TimerUpdate::default()
        },
        "ENABLE = %?, VERSION = VERSION + 1",
        vec![SqlArg::Bool(true)],
    );
    let now = api::now_timestamp();
    let full = api::TimerUpdate {
        Enable: api::NewOptionalVal(false),
        Tags: api::NewOptionalVal(vec!["l1".into(), "l2".into()]),
        TimeZone: api::NewOptionalVal("Asia/Shanghai".into()),
        SchedPolicyType: api::NewOptionalVal(api::SchedEventInterval.into()),
        SchedPolicyExpr: api::NewOptionalVal("1h".into()),
        ManualRequest: api::NewOptionalVal(api::ManualRequest {
            ManualRequestID: "req1".into(),
            ManualRequestTime: Some(timestamp(123)),
            ManualTimeout: Duration::from_secs(60),
            ManualProcessed: true,
            ManualEventID: "event1".into(),
        }),
        EventStatus: api::NewOptionalVal(api::SchedEventTrigger.into()),
        EventID: api::NewOptionalVal("event1".into()),
        EventData: api::NewOptionalVal(b"data1".to_vec()),
        EventStart: api::NewOptionalVal(Some(now)),
        EventExtra: api::NewOptionalVal(api::EventExtra {
            EventManualRequestID: "req2".into(),
            EventWatermark: Some(timestamp(456)),
        }),
        Watermark: api::NewOptionalVal(Some(now + Duration::from_secs(1))),
        SummaryData: api::NewOptionalVal(b"summary".to_vec()),
        CheckEventID: api::NewOptionalVal("ee".into()),
        CheckVersion: api::NewOptionalVal(1),
    };
    assert_update(
        &full,
        concat!(
            "ENABLE = %?, TIMEZONE = %?, SCHED_POLICY_TYPE = %?, SCHED_POLICY_EXPR = %?, ",
            "EVENT_STATUS = %?, EVENT_ID = %?, EVENT_DATA = %?, EVENT_START = FROM_UNIXTIME(%?), ",
            "WATERMARK = FROM_UNIXTIME(%?), SUMMARY_DATA = %?, ",
            "TIMER_EXT = JSON_MERGE_PATCH(TIMER_EXT, %?), VERSION = VERSION + 1"
        ),
        vec![
            SqlArg::Bool(false),
            SqlArg::String("Asia/Shanghai".into()),
            SqlArg::String("INTERVAL".into()),
            SqlArg::String("1h".into()),
            SqlArg::String("TRIGGER".into()),
            SqlArg::String("event1".into()),
            SqlArg::Bytes(b"data1".to_vec()),
            SqlArg::I64(now.timestamp()),
            SqlArg::I64(now.timestamp() + 1),
            SqlArg::Bytes(b"summary".to_vec()),
            SqlArg::Json(concat!(
                "{\"event\":{\"manual_request_id\":\"req2\",\"watermark_unix\":456},",
                "\"manual\":{\"request_id\":\"req1\",\"request_time_unix\":123,\"timeout_sec\":60,",
                "\"processed\":true,\"event_id\":\"event1\"},\"tags\":[\"l1\",\"l2\"]}"
            ).into()),
        ],
    );
    assert_update(
        &api::TimerUpdate {
            EventExtra: api::NewOptionalVal(api::EventExtra {
                EventManualRequestID: "req1".into(),
                ..api::EventExtra::default()
            }),
            ManualRequest: api::NewOptionalVal(api::ManualRequest {
                ManualRequestID: "req2".into(),
                ..api::ManualRequest::default()
            }),
            ..api::TimerUpdate::default()
        },
        "TIMER_EXT = JSON_MERGE_PATCH(TIMER_EXT, %?), VERSION = VERSION + 1",
        vec![SqlArg::Json(concat!(
            "{\"event\":{\"manual_request_id\":\"req1\",\"watermark_unix\":null},",
            "\"manual\":{\"request_id\":\"req2\",\"request_time_unix\":null,\"timeout_sec\":null,",
            "\"processed\":null,\"event_id\":null}}"
        ).into())],
    );
    assert_update(
        &api::TimerUpdate {
            EventExtra: api::NewOptionalVal(api::EventExtra {
                EventWatermark: Some(timestamp(123)),
                ..api::EventExtra::default()
            }),
            ManualRequest: api::NewOptionalVal(api::ManualRequest {
                ManualRequestTime: Some(timestamp(456)),
                ..api::ManualRequest::default()
            }),
            ..api::TimerUpdate::default()
        },
        "TIMER_EXT = JSON_MERGE_PATCH(TIMER_EXT, %?), VERSION = VERSION + 1",
        vec![SqlArg::Json(
            concat!(
                "{\"event\":{\"manual_request_id\":null,\"watermark_unix\":123},",
                "\"manual\":{\"request_id\":null,\"request_time_unix\":456,\"timeout_sec\":null,",
                "\"processed\":null,\"event_id\":null}}"
            )
            .into(),
        )],
    );
    assert_update(
        &api::TimerUpdate {
            TimeZone: api::NewOptionalVal(String::new()),
            SchedPolicyExpr: api::NewOptionalVal(String::new()),
            EventID: api::NewOptionalVal(String::new()),
            EventData: api::NewOptionalVal(vec![]),
            EventStart: api::NewOptionalVal(None),
            EventExtra: api::NewOptionalVal(api::EventExtra::default()),
            ManualRequest: api::NewOptionalVal(api::ManualRequest::default()),
            Watermark: api::NewOptionalVal(None),
            SummaryData: api::NewOptionalVal(vec![]),
            Tags: api::NewOptionalVal(vec![]),
            ..api::TimerUpdate::default()
        },
        concat!(
            "TIMEZONE = %?, SCHED_POLICY_EXPR = %?, EVENT_ID = %?, EVENT_DATA = %?, ",
            "EVENT_START = NULL, WATERMARK = NULL, SUMMARY_DATA = %?, ",
            "TIMER_EXT = JSON_MERGE_PATCH(TIMER_EXT, %?), VERSION = VERSION + 1"
        ),
        vec![
            SqlArg::String(String::new()),
            SqlArg::String(String::new()),
            SqlArg::String(String::new()),
            SqlArg::Bytes(vec![]),
            SqlArg::Bytes(vec![]),
            SqlArg::Json("{\"event\":null,\"manual\":null,\"tags\":null}".into()),
        ],
    );
    assert_update(
        &api::TimerUpdate {
            CheckEventID: api::NewOptionalVal("ee".into()),
            CheckVersion: api::NewOptionalVal(1),
            ..api::TimerUpdate::default()
        },
        "VERSION = VERSION + 1",
        vec![],
    );
}

#[test]
fn test_build_update_timer_sql() {
    let (sql, args) =
        buildUpdateTimerSQL("db1", "tbl1", "123", &api::TimerUpdate::default()).unwrap();
    assert_eq!(
        sql,
        "UPDATE `db1`.`tbl1` SET VERSION = VERSION + 1 WHERE ID = %?"
    );
    assert_eq!(args, vec![SqlArg::String("123".into())]);

    let update = api::TimerUpdate {
        SchedPolicyType: api::NewOptionalVal(api::SchedEventInterval.into()),
        SchedPolicyExpr: api::NewOptionalVal("1h".into()),
        ..api::TimerUpdate::default()
    };
    let (sql, args) = buildUpdateTimerSQL("db1", "tbl1", "123", &update).unwrap();
    assert_eq!(
        sql,
        "UPDATE `db1`.`tbl1` SET SCHED_POLICY_TYPE = %?, SCHED_POLICY_EXPR = %?, VERSION = VERSION + 1 WHERE ID = %?"
    );
    assert_eq!(
        args,
        vec![
            SqlArg::String("INTERVAL".into()),
            SqlArg::String("1h".into()),
            SqlArg::String("123".into()),
        ]
    );
}

#[test]
fn test_build_delete_timer_sql() {
    let (sql, args) = buildDeleteTimerSQL("db1", "tbl1", "123");
    assert_eq!(sql, "DELETE FROM `db1`.`tbl1` WHERE ID = %?");
    assert_eq!(args, vec![SqlArg::String("123".into())]);
}

#[test]
// 锁定 Go JSON 编码的转义和代理对兼容行为，并拒绝字段类型不匹配。
fn test_timer_ext_json_matches_go_encoding_edges() {
    let cond = api::TimerCond {
        Tags: api::NewOptionalVal(vec!["<&>\u{8}\u{c}\u{2028}\u{2029}".into()]),
        ..api::TimerCond::default()
    };
    let (_, args) = buildCondCriteria(Some(&cond), vec![]).unwrap();
    assert_eq!(
        args,
        vec![SqlArg::Json(
            r#"["\u003c\u0026\u003e\b\f\u2028\u2029"]"#.into()
        )]
    );

    let ext = decode_timer_ext(r#"{"tags":["\ud83d\ude00"]}"#).unwrap();
    assert_eq!(ext.tags, vec!["😀"]);
    let ext = decode_timer_ext(r#"{"tags":["\ud83d","\ude00"]}"#).unwrap();
    assert_eq!(ext.tags, vec!["�", "�"]);
    assert!(
        decode_timer_ext(r#"{"manual":{"request_id":1}}"#)
            .unwrap_err()
            .to_string()
            .contains("request_id")
    );
    for invalid in [r#"{"tags":["a",]}"#, r#"{"manual":{},}"#] {
        assert!(
            decode_timer_ext(invalid).is_err(),
            "Go encoding/json rejects trailing commas: {invalid}"
        );
    }
}

// 描述一次预期 SQL 调用的返回方式，错误与 panic 分开以覆盖不同清理路径。
enum MockOutcome {
    Rows(Vec<SqlRow>),
    Error(String),
    Panic(String),
}

// 会话桩以队列顺序消费调用，同时精确核对 SQL 文本及绑定参数。
struct MockCall {
    sql: String,
    args: Vec<SqlArg>,
    outcome: MockOutcome,
}

impl MockCall {
    fn ok(sql: &str) -> Self {
        Self {
            sql: sql.into(),
            args: vec![],
            outcome: MockOutcome::Rows(vec![]),
        }
    }

    fn rows(sql: &str, rows: Vec<SqlRow>) -> Self {
        Self {
            sql: sql.into(),
            args: vec![],
            outcome: MockOutcome::Rows(rows),
        }
    }

    fn error(sql: &str, error: &str) -> Self {
        Self {
            sql: sql.into(),
            args: vec![],
            outcome: MockOutcome::Error(error.into()),
        }
    }

    fn panic(sql: &str, value: &str) -> Self {
        Self {
            sql: sql.into(),
            args: vec![],
            outcome: MockOutcome::Panic(value.into()),
        }
    }

    fn with_args(mut self, args: Vec<SqlArg>) -> Self {
        self.args = args;
        self
    }
}

struct MockState {
    calls: VecDeque<MockCall>,
}

// 最小化的系统会话上下文；仅内部 SQL 执行参与这些测试。
struct MockSessionContext {
    state: Arc<Mutex<MockState>>,
}

impl syssession::SessionContext for MockSessionContext {
    fn close(&mut self) {}
    fn on_became_owner(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn on_resign_owner(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn has_pending_transaction(&self) -> bool {
        false
    }
    fn rollback_transaction(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn reset_state(&mut self) -> syssession::Result<()> {
        Ok(())
    }
    fn register_internal_session(&mut self) -> bool {
        true
    }
    fn unregister_internal_session(&mut self) {}
    fn execute(&mut self, _sql: &str) -> syssession::Result<Vec<syssession::RecordSet>> {
        Ok(vec![])
    }
    fn execute_internal(
        &mut self,
        sql: &str,
        args: &[syssession::SqlValue],
    ) -> syssession::Result<syssession::RecordSet> {
        let actual_args = args
            .iter()
            .map(|arg| {
                arg.downcast_ref::<SqlArg>()
                    .expect("timer SQL argument")
                    .clone()
            })
            .collect::<Vec<_>>();
        let call = self
            .state
            .lock()
            .expect("mock state lock")
            .calls
            .pop_front()
            .expect("unexpected SQL call");
        assert_eq!(sql, call.sql);
        assert_eq!(actual_args, call.args);
        match call.outcome {
            MockOutcome::Rows(rows) => Ok(Box::new(SqlResult { rows })),
            MockOutcome::Error(error) => Err(syssession::SessionError::new(error)),
            MockOutcome::Panic(value) => std::panic::panic_any(value),
        }
    }
    fn execute_statement(
        &mut self,
        _statement: &dyn Any,
    ) -> syssession::Result<syssession::RecordSet> {
        Ok(Box::new(()))
    }
    fn parse_with_params(
        &mut self,
        _sql: &str,
        _args: &[syssession::SqlValue],
    ) -> syssession::Result<syssession::Statement> {
        Ok(Box::new(()))
    }
    fn exec_restricted_statement(
        &mut self,
        _statement: &dyn Any,
    ) -> syssession::Result<Vec<syssession::Row>> {
        Ok(vec![])
    }
    fn exec_restricted_sql(
        &mut self,
        _sql: &str,
        _args: &[syssession::SqlValue],
    ) -> syssession::Result<Vec<syssession::Row>> {
        Ok(vec![])
    }
}

// 固定复用同一测试会话，并可在进入回调前注入连接池错误。
struct MockPool {
    session: syssession::Session,
    error: Mutex<Option<String>>,
}

impl syssession::Pool for MockPool {
    fn Get(&self) -> syssession::Result<syssession::Session> {
        Err(syssession::SessionError::new("mock pool Get is unused"))
    }
    fn Put(&self, _session: &syssession::Session) {}
    fn WithSession(
        &self,
        callback: &mut dyn FnMut(&syssession::Session) -> syssession::Result<()>,
    ) -> syssession::Result<()> {
        if let Some(error) = self.error.lock().expect("pool error lock").clone() {
            return Err(syssession::SessionError::new(error));
        }
        callback(&self.session)
    }
    fn WithForceBlockGCSession(
        &self,
        _cancellation: &syssession::CancellationToken,
        callback: &mut dyn FnMut(&syssession::Session) -> syssession::Result<()>,
    ) -> syssession::Result<()> {
        self.WithSession(callback)
    }
}

fn session_with_calls(calls: Vec<MockCall>) -> (syssession::Session, Arc<Mutex<MockState>>) {
    let state = Arc::new(Mutex::new(MockState {
        calls: calls.into(),
    }));
    let session = syssession::NewSessionForTest(Box::new(MockSessionContext {
        state: Arc::clone(&state),
    }))
    .unwrap();
    (session, state)
}

fn core_with_calls(
    calls: Vec<MockCall>,
) -> (TableTimerStoreCore, Arc<MockPool>, Arc<Mutex<MockState>>) {
    let (session, state) = session_with_calls(calls);
    let pool = Arc::new(MockPool {
        session,
        error: Mutex::new(None),
    });
    let core = TableTimerStoreCore {
        pool: pool.clone(),
        db_name: "db1".into(),
        table_name: "t1".into(),
        notifier: api::NewMemTimerWatchEventNotifier(),
    };
    (core, pool, state)
}

// `with_session` 进入业务回调前会回滚残留事务、保存时区并切换到 UTC。
fn init_calls() -> Vec<MockCall> {
    vec![
        MockCall::ok("ROLLBACK"),
        MockCall::rows(
            "SELECT @@time_zone",
            vec![SqlRow(vec![crate::SqlCell::String("tz1".into())])],
        ),
        MockCall::ok("SET @@time_zone='UTC'"),
    ]
}

// 回调结束后必须再次回滚，并恢复进入前记录的会话时区。
fn restore_calls() -> Vec<MockCall> {
    vec![
        MockCall::ok("ROLLBACK"),
        MockCall::ok("SET @@time_zone=%?").with_args(vec![SqlArg::String("tz1".into())]),
    ]
}

// 防止只验证返回值却遗漏应执行的清理 SQL。
fn assert_calls_consumed(state: &Arc<Mutex<MockState>>) {
    assert!(
        state.lock().expect("mock state lock").calls.is_empty(),
        "not all expected SQL calls were consumed"
    );
}

#[derive(Default)]
struct MockEtcdClient {
    watches: AtomicUsize,
}

impl EtcdClient for MockEtcdClient {
    fn grant(&self, _ttl_seconds: i64) -> Result<i64, String> {
        Ok(1)
    }

    fn keep_alive(&self, _lease_id: i64) -> Result<mpsc::Receiver<()>, String> {
        let (_sender, receiver) = mpsc::channel();
        Ok(receiver)
    }

    fn watch_prefix(&self, prefix: &str, ctx: &api::Context) -> api::WatchTimerChan {
        assert_eq!(prefix, "/tidb/timer/cluster/42/notify/");
        self.watches.fetch_add(1, Ordering::SeqCst);
        let notifier = api::NewMemTimerWatchEventNotifier();
        notifier.Watch(ctx)
    }

    fn put_events(
        &self,
        _key: &str,
        _events: &[EtcdNotifyEvent],
        _lease_id: i64,
        _timeout: Duration,
    ) -> Result<(), String> {
        Ok(())
    }
}

#[test]
// 关闭通知器既要断开现有监听，也不能为关闭后的请求创建新的 etcd watch。
fn test_etcd_notifier_close_cancels_watchers() {
    let client = Arc::new(MockEtcdClient::default());
    let notifier = NewEtcdNotifier(42, client.clone());
    let ctx = api::Context::background();
    let watcher = notifier.Watch(&ctx);
    assert_eq!(client.watches.load(Ordering::SeqCst), 1);

    notifier.Close();
    assert_eq!(
        format!(
            "{:?}",
            watcher
                .recv_timeout(Duration::from_millis(250))
                .unwrap_err()
        ),
        "Disconnected"
    );

    let late_watcher = notifier.Watch(&ctx);
    assert_eq!(
        format!(
            "{:?}",
            late_watcher
                .recv_timeout(Duration::from_millis(50))
                .unwrap_err()
        ),
        "Disconnected"
    );
    assert_eq!(client.watches.load(Ordering::SeqCst), 1);
}

#[test]
// 旧记录以 `TIDB` 标记时区，读取时应使用全局时区重新解释时间列。
fn test_list_uses_tidb_global_timezone_for_legacy_rows() {
    let select_sql = concat!(
        "SELECT ID, NAMESPACE, TIMER_KEY, TIMER_DATA, TIMEZONE, SCHED_POLICY_TYPE, ",
        "SCHED_POLICY_EXPR, HOOK_CLASS, WATERMARK, ENABLE, TIMER_EXT, EVENT_STATUS, EVENT_ID, ",
        "EVENT_DATA, EVENT_START, SUMMARY_DATA, CREATE_TIME, UPDATE_TIME, VERSION ",
        "FROM `db1`.`t1` WHERE 1"
    );
    let timer_row = SqlRow(vec![
        crate::SqlCell::U64(7),
        crate::SqlCell::String("ns".into()),
        crate::SqlCell::String("key".into()),
        crate::SqlCell::Null,
        crate::SqlCell::String("TIDB".into()),
        crate::SqlCell::String(api::SchedEventInterval.into()),
        crate::SqlCell::String("1h".into()),
        crate::SqlCell::String("hook".into()),
        crate::SqlCell::Timestamp(timestamp_in_location(123, "UTC")),
        crate::SqlCell::I64(1),
        crate::SqlCell::Json("{}".into()),
        crate::SqlCell::String(api::SchedEventIdle.into()),
        crate::SqlCell::String(String::new()),
        crate::SqlCell::Null,
        crate::SqlCell::Null,
        crate::SqlCell::Null,
        crate::SqlCell::Timestamp(timestamp_in_location(456, "UTC")),
        crate::SqlCell::Null,
        crate::SqlCell::U64(3),
    ]);
    let mut calls = init_calls();
    calls.push(MockCall::rows(
        "SELECT @@tidb_enable_index_merge",
        vec![SqlRow(vec![crate::SqlCell::String("OFF".into())])],
    ));
    calls.push(MockCall::ok("SET @@tidb_enable_index_merge=ON"));
    calls.push(MockCall::rows(select_sql, vec![timer_row]));
    calls.push(MockCall::rows(
        "SELECT @@global.time_zone",
        vec![SqlRow(vec![crate::SqlCell::String("Asia/Shanghai".into())])],
    ));
    calls.push(MockCall::ok("SET @@tidb_enable_index_merge=OFF"));
    calls.extend(restore_calls());
    let (core, _, state) = core_with_calls(calls);

    let timers = api::TimerStoreCore::List(&core, &api::Context::background(), None).unwrap();
    assert_eq!(timers.len(), 1);
    assert_eq!(timers[0].ID, "7");
    assert_eq!(timers[0].TimeZone, "TIDB");
    assert_eq!(
        timers[0].Location,
        Some(api::parse_location("Asia/Shanghai").unwrap())
    );
    assert_eq!(
        timers[0]
            .Watermark
            .as_ref()
            .unwrap()
            .offset()
            .local_minus_utc(),
        8 * 60 * 60
    );
    assert_eq!(
        timers[0]
            .CreateTime
            .as_ref()
            .unwrap()
            .offset()
            .local_minus_utc(),
        8 * 60 * 60
    );
    assert_calls_consumed(&state);
}

#[test]
// 覆盖初始化、回调及清理的错误与 panic；清理失败时会话必须禁止复用。
fn test_with_session() {
    let (core, pool, state) = core_with_calls(vec![]);
    *pool.error.lock().unwrap() = Some("mockErr".into());
    assert_eq!(
        core.with_session::<()>(|_| Ok(())).unwrap_err().to_string(),
        "mockErr"
    );
    assert_calls_consumed(&state);

    let (core, _, state) = core_with_calls(vec![MockCall::error("ROLLBACK", "mockErr1")]);
    assert_eq!(
        core.with_session::<()>(|_| Ok(())).unwrap_err().to_string(),
        "mockErr1"
    );
    assert_calls_consumed(&state);

    let (core, _, state) = core_with_calls(vec![
        MockCall::ok("ROLLBACK"),
        MockCall::error("SELECT @@time_zone", "mockErr2"),
    ]);
    assert_eq!(
        core.with_session::<()>(|_| Ok(())).unwrap_err().to_string(),
        "mockErr2"
    );
    assert_calls_consumed(&state);

    let (core, _, state) = core_with_calls(vec![MockCall::panic("ROLLBACK", "mockPanic")]);
    let panic = catch_unwind(AssertUnwindSafe(|| core.with_session::<()>(|_| Ok(()))));
    assert_eq!(
        *panic.unwrap_err().downcast::<String>().unwrap(),
        "mockPanic"
    );
    assert_calls_consumed(&state);

    let mut calls = init_calls();
    calls.extend(restore_calls());
    let (core, _, state) = core_with_calls(calls);
    assert_eq!(core.with_session(|_| Ok("value")).unwrap(), "value");
    assert_calls_consumed(&state);

    let mut calls = init_calls();
    calls.extend(restore_calls());
    let (core, _, state) = core_with_calls(calls);
    assert_eq!(
        core.with_session::<()>(|_| Err(api::TimerError::message("mockErr3")))
            .unwrap_err()
            .to_string(),
        "mockErr3"
    );
    assert_calls_consumed(&state);

    let mut calls = init_calls();
    calls.extend(restore_calls());
    let (core, _, state) = core_with_calls(calls);
    let panic = catch_unwind(AssertUnwindSafe(|| {
        core.with_session::<()>(|_| std::panic::panic_any("panic2".to_string()))
    }));
    assert_eq!(*panic.unwrap_err().downcast::<String>().unwrap(), "panic2");
    assert_calls_consumed(&state);

    let mut calls = init_calls();
    calls.push(MockCall::error("ROLLBACK", "ROLLBACK error"));
    let (core, pool, state) = core_with_calls(calls);
    assert!(core.with_session::<()>(|_| Ok(())).is_ok());
    assert!(pool.session.IsAvoidReuse());
    assert_calls_consumed(&state);

    let mut calls = init_calls();
    calls.push(MockCall::ok("ROLLBACK"));
    calls.push(
        MockCall::error("SET @@time_zone=%?", "SET tz error")
            .with_args(vec![SqlArg::String("tz1".into())]),
    );
    let (core, pool, state) = core_with_calls(calls);
    assert!(core.with_session::<()>(|_| Ok(())).is_ok());
    assert!(pool.session.IsAvoidReuse());
    assert_calls_consumed(&state);
}

#[test]
// 验证悲观事务的提交/回滚顺序，并确保回滚失败不覆盖原始业务错误。
fn test_run_in_txn() {
    let (session, state) = session_with_calls(vec![
        MockCall::ok("BEGIN PESSIMISTIC"),
        MockCall::ok("insert into t value(?)").with_args(vec![SqlArg::I64(1)]),
        MockCall::ok("COMMIT"),
    ]);
    runInTxn(&session, |session| {
        executeSQL(session, "insert into t value(?)", vec![SqlArg::I64(1)])?;
        Ok(())
    })
    .unwrap();
    assert_calls_consumed(&state);

    let (session, state) =
        session_with_calls(vec![MockCall::error("BEGIN PESSIMISTIC", "mockBeginErr")]);
    assert_eq!(
        runInTxn(&session, |_| Ok(())).unwrap_err().to_string(),
        "mockBeginErr"
    );
    assert_calls_consumed(&state);

    let (session, state) = session_with_calls(vec![
        MockCall::ok("BEGIN PESSIMISTIC"),
        MockCall::ok("ROLLBACK"),
    ]);
    assert_eq!(
        runInTxn::<()>(&session, |_| Err(api::TimerError::message("mockFuncErr")))
            .unwrap_err()
            .to_string(),
        "mockFuncErr"
    );
    assert_calls_consumed(&state);

    let (session, state) = session_with_calls(vec![
        MockCall::ok("BEGIN PESSIMISTIC"),
        MockCall::error("COMMIT", "commitErr"),
        MockCall::ok("ROLLBACK"),
    ]);
    assert_eq!(
        runInTxn(&session, |_| Ok(())).unwrap_err().to_string(),
        "commitErr"
    );
    assert_calls_consumed(&state);

    let (session, state) = session_with_calls(vec![
        MockCall::ok("BEGIN PESSIMISTIC"),
        MockCall::error("ROLLBACK", "rollbackErr"),
    ]);
    assert_eq!(
        runInTxn::<()>(&session, |_| Err(api::TimerError::message("mockFuncErr")))
            .unwrap_err()
            .to_string(),
        "mockFuncErr"
    );
    assert_calls_consumed(&state);
}
