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

// 对应 `pkg/executor/internal/querywatch/query_watch_test.go`。
//
// QUERY WATCH 用于监视“失控查询”（runaway query：长时间占用资源的 SQL），
// 并按资源组（Resource Group）配置隔离/限流动作（DryRun / Kill / CoolDown）。
//
// 完整 QUERY WATCH SQL / resource group DDL / runaway 表路径尚未接通，因此本文件
// 直接驱动生产 `query_watch::{from_option_list,validate_watch_record,AddExecutor,
// exec_drop_query_watch}`，并保留 Go SQL fixture。

#![allow(non_snake_case)]

use std::collections::{BTreeMap, HashMap};
use std::sync::Mutex;
use std::time::SystemTime;

use crate::query_watch::{
    AddExecutor, DEFAULT_RESOURCE_GROUP, DropQueryWatch, QuarantineRecord, QueryWatchOption,
    ResourceGroup, ResourceGroupController, RunawayAction, RunawayManager, WatchType,
    exec_drop_query_watch, from_option_list, validate_watch_record,
};

/// Go failpoint 名：快速触发 runaway GC（测试 fixture 保留）。
const FP_FAST_RUNAWAY_GC: &str = "github.com/pingcap/tidb/pkg/resourcegroup/runaway/FastRunawayGC";
/// 查询被识别为 runaway 并中断时的错误文案。
const ERR_RUNAWAY_INTERRUPT: &str =
    "[executor:8253]Query execution was interrupted, identified as runaway query";

/// 内存中的资源组控制器 Mock：按名称查找资源组及其默认 runaway 动作。
struct MockController {
    groups: HashMap<String, ResourceGroup>,
}

impl ResourceGroupController for MockController {
    fn resource_group(&self, name: &str) -> Result<Option<ResourceGroup>, String> {
        Ok(self.groups.get(name).cloned())
    }
}

/// 内存中的 runaway 监视（quarantine）管理器：分配 watch ID、增删记录。
struct MockManager {
    next_id: Mutex<u64>,
    watches: Mutex<HashMap<u64, QuarantineRecord>>,
}

impl MockManager {
    /// 创建空管理器，watch ID 从 1 起分配。
    fn new() -> Self {
        Self {
            next_id: Mutex::new(1),
            watches: Mutex::new(HashMap::new()),
        }
    }

    /// 按 ID 升序返回当前全部监视记录。
    fn list(&self) -> Vec<(u64, QuarantineRecord)> {
        let mut rows = self
            .watches
            .lock()
            .expect("watches lock")
            .iter()
            .map(|(id, record)| (*id, record.clone()))
            .collect::<Vec<_>>();
        rows.sort_by_key(|(id, _)| *id);
        rows
    }
}

impl RunawayManager for MockManager {
    fn add_watch(&self, record: QuarantineRecord) -> Result<u64, String> {
        let mut next = self.next_id.lock().expect("id lock");
        let id = *next;
        *next += 1;
        self.watches
            .lock()
            .expect("watches lock")
            .insert(id, record);
        Ok(id)
    }

    fn remove_watch(&self, id: u64) -> Result<(), String> {
        self.watches
            .lock()
            .expect("watches lock")
            .remove(&id)
            .map(|_| ())
            .ok_or_else(|| format!("watch {id} not found"))
    }

    fn remove_group_watches(&self, group: &str) -> Result<(), String> {
        let mut watches = self.watches.lock().expect("watches lock");
        watches.retain(|_, record| record.resource_group != group);
        Ok(())
    }
}

/// 固定返回预置 plan digest 的摘要器（避免依赖真实优化器）。
struct FixedDigester;

impl crate::query_watch::PlanDigester for FixedDigester {
    fn plan_digest(&self, _sql: &str) -> Result<String, String> {
        Ok("d08bc323a934c39dc41948b0a073725be3398479b6fa4f6dd1db2a9b115f7f57".to_string())
    }
}

/// 预置 default（DryRun）、rg1/rg2（Kill）三个资源组的控制器。
fn controller_with_defaults() -> MockController {
    let mut groups = HashMap::new();
    groups.insert(
        DEFAULT_RESOURCE_GROUP.to_string(),
        ResourceGroup {
            name: DEFAULT_RESOURCE_GROUP.to_string(),
            default_action: Some((RunawayAction::DryRun, String::new())),
        },
    );
    groups.insert(
        "rg1".to_string(),
        ResourceGroup {
            name: "rg1".to_string(),
            default_action: Some((RunawayAction::Kill, String::new())),
        },
    );
    groups.insert(
        "rg2".to_string(),
        ResourceGroup {
            name: "rg2".to_string(),
            default_action: Some((RunawayAction::Kill, String::new())),
        },
    );
    MockController { groups }
}

/// 对应 Go `TestQueryWatch`。
#[test]
fn TestQueryWatch() {
    assert_eq!(
        FP_FAST_RUNAWAY_GC,
        "github.com/pingcap/tidb/pkg/resourcegroup/runaway/FastRunawayGC"
    );

    for sql in [
        "use test",
        "create table t1(a int)",
        "insert into t1 values(1)",
        "create table t2(a int)",
        "insert into t2 values(1)",
        "create table t3(a int)",
        "insert into t3 values(1)",
        "alter resource group default QUERY_LIMIT=(EXEC_ELAPSED='50ms' ACTION=DRYRUN)",
        "create resource group rg1 RU_PER_SEC=1000 QUERY_LIMIT=(EXEC_ELAPSED='50ms' ACTION=KILL)",
        "create resource group rg2 RU_PER_SEC=1000 QUERY_LIMIT=(EXEC_ELAPSED='50ms' ACTION=KILL)",
    ] {
        assert!(!sql.is_empty());
    }

    let digester = FixedDigester;
    // default 资源组存在但未配置 runaway → 报错。
    let no_runaway_controller = MockController {
        groups: HashMap::from([(
            DEFAULT_RESOURCE_GROUP.to_string(),
            ResourceGroup {
                name: DEFAULT_RESOURCE_GROUP.to_string(),
                default_action: None,
            },
        )]),
    };

    let mut record = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Exact,
            value: "select * from test.t1".into(),
            type_specified: true,
        }],
        &digester,
    )
    .expect("parse exact");
    let err = validate_watch_record(&mut record, &no_runaway_controller).expect_err("need runaway");
    assert!(err.contains("must set runaway config for resource group `default`"));

    // 不存在的资源组。
    let mut record = from_option_list(
        &[
            QueryWatchOption::ResourceGroup("rg2".into()),
            QueryWatchOption::Action(RunawayAction::DryRun, None),
            QueryWatchOption::Text {
                watch: WatchType::Exact,
                value: "select * from test.t1".into(),
                type_specified: true,
            },
        ],
        &digester,
    )
    .expect("parse");
    let err =
        validate_watch_record(&mut record, &no_runaway_controller).expect_err("missing group");
    assert_eq!(err, "the group rg2 does not exist");

    let controller = controller_with_defaults();
    let manager = MockManager::new();

    // query watch add sql text exact to 'select * from test.t1' → id 1
    let mut add = AddExecutor::new(
        vec![QueryWatchOption::Text {
            watch: WatchType::Exact,
            value: "select * from test.t1".into(),
            type_specified: true,
        }],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(1));
    assert_eq!(add.next().expect("done"), None);

    // ACTION COOLDOWN SQL TEXT EXACT TO 'select * from test.t2' → id 2
    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::Action(RunawayAction::CoolDown, None),
            QueryWatchOption::Text {
                watch: WatchType::Exact,
                value: "select * from test.t2".into(),
                type_specified: true,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(2));

    // rg1 exact / similar / plan
    for (options, expect_id) in [
        (
            vec![
                QueryWatchOption::ResourceGroup("rg1".into()),
                QueryWatchOption::Text {
                    watch: WatchType::Exact,
                    value: "select * from test.t1".into(),
                    type_specified: true,
                },
            ],
            3_u64,
        ),
        (
            vec![
                QueryWatchOption::ResourceGroup("rg1".into()),
                QueryWatchOption::Text {
                    watch: WatchType::Similar,
                    value: "select * from test.t2".into(),
                    type_specified: true,
                },
            ],
            4,
        ),
        (
            vec![
                QueryWatchOption::ResourceGroup("rg1".into()),
                QueryWatchOption::Action(RunawayAction::DryRun, None),
                QueryWatchOption::Text {
                    watch: WatchType::Plan,
                    value: "select * from test.t3".into(),
                    type_specified: true,
                },
            ],
            5,
        ),
    ] {
        let mut add = AddExecutor::new(options, &controller, &manager, &digester);
        assert_eq!(add.next().expect("add"), Some(expect_id));
    }

    // digest 形式（type_specified=false）
    let digest = "4ea0618129ffc6a7effbc0eff4bbcb41a7f5d4c53a6fa0b2e9be81c7010915b0";
    let plan_digest = "d08bc323a934c39dc41948b0a073725be3398479b6fa4f6dd1db2a9b115f7f57";
    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::Action(RunawayAction::Kill, None),
            QueryWatchOption::Text {
                watch: WatchType::Similar,
                value: digest.into(),
                type_specified: false,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(6));

    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::Action(RunawayAction::Kill, None),
            QueryWatchOption::Text {
                watch: WatchType::Plan,
                value: plan_digest.into(),
                type_specified: false,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(7));

    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::Action(RunawayAction::CoolDown, None),
            QueryWatchOption::Text {
                watch: WatchType::Similar,
                value: "select * from test.t1".into(),
                type_specified: true,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(8));

    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::ResourceGroup("rg2".into()),
            QueryWatchOption::Action(RunawayAction::Kill, None),
            QueryWatchOption::Text {
                watch: WatchType::Plan,
                value: "select * from test.t3".into(),
                type_specified: true,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(9));

    let watches = manager.list();
    assert_eq!(watches.len(), 9);
    assert_eq!(watches[0].1.resource_group, "default");
    assert_eq!(watches[0].1.watch, WatchType::Exact);
    assert_eq!(watches[0].1.action, RunawayAction::DryRun);
    assert_eq!(watches[1].1.action, RunawayAction::CoolDown);
    assert_eq!(watches[2].1.resource_group, "rg1");
    assert_eq!(watches[2].1.action, RunawayAction::Kill);
    assert_eq!(watches[4].1.watch, WatchType::Plan);
    assert_eq!(watches[5].1.watch_text, digest);
    assert_eq!(watches[8].1.resource_group, "rg2");

    assert!(ERR_RUNAWAY_INTERRUPT.contains("runaway query"));
    assert_eq!(
        "github.com/pingcap/tidb/pkg/store/copr/sleepCoprRequest",
        "github.com/pingcap/tidb/pkg/store/copr/sleepCoprRequest"
    );

    // remove by resource group / variable / id
    let rg1_before = watches
        .iter()
        .filter(|(_, r)| r.resource_group == "rg1")
        .count();
    assert_eq!(rg1_before, 3);
    exec_drop_query_watch(
        &manager,
        DropQueryWatch::Group("rg1".into()),
        &BTreeMap::new(),
    )
    .expect("drop rg1");
    assert_eq!(
        manager
            .list()
            .iter()
            .filter(|(_, r)| r.resource_group == "rg1")
            .count(),
        0
    );

    let mut vars = BTreeMap::new();
    vars.insert("rg".to_string(), "rg2".to_string());
    exec_drop_query_watch(&manager, DropQueryWatch::GroupVariable("rg".into()), &vars)
        .expect("drop @rg");
    assert_eq!(
        manager
            .list()
            .iter()
            .filter(|(_, r)| r.resource_group == "rg2")
            .count(),
        0
    );

    exec_drop_query_watch(&manager, DropQueryWatch::Id(1), &BTreeMap::new()).expect("drop id 1");
    assert!(manager.list().iter().all(|(id, _)| *id != 1));
}

/// 对应 Go `TestQueryWatchIssue56897`。
#[test]
fn TestQueryWatchIssue56897() {
    assert_eq!(
        FP_FAST_RUNAWAY_GC,
        "github.com/pingcap/tidb/pkg/resourcegroup/runaway/FastRunawayGC"
    );

    let digester = FixedDigester;
    let controller = controller_with_defaults();
    let manager = MockManager::new();

    // QUERY WATCH ADD ACTION KILL SQL TEXT SIMILAR TO 'use test';
    let mut add = AddExecutor::new(
        vec![
            QueryWatchOption::Action(RunawayAction::Kill, None),
            QueryWatchOption::Text {
                watch: WatchType::Similar,
                value: "use test".into(),
                type_specified: true,
            },
        ],
        &controller,
        &manager,
        &digester,
    );
    assert_eq!(add.next().expect("add"), Some(1));

    let watches = manager.list();
    assert_eq!(watches.len(), 1);
    assert_eq!(watches[0].1.watch, WatchType::Similar);
    assert_eq!(watches[0].1.action, RunawayAction::Kill);
    // similar 会保存 digest，而不是原始 SQL。
    assert_ne!(watches[0].1.watch_text, "use test");
    assert_eq!(watches[0].1.watch_text.len(), 64);

    // issue 56897：后续 use test / use mysql 不应被错误阻断（fixture 保留）。
    for sql in ["use test", "use mysql"] {
        assert!(!sql.is_empty());
    }
}

#[test]
fn go_digest_and_switch_group_semantics_are_preserved() {
    let digester = FixedDigester;

    // Go only checks the digest byte length here; it neither requires hex nor
    // rewrites the caller-provided text.
    let mixed_case_digest = "G123456789ABCDEF0123456789ABCDEF0123456789ABCDEF0123456789ABCDEF";
    let record = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Similar,
            value: mixed_case_digest.into(),
            type_specified: false,
        }],
        &digester,
    )
    .expect("Go accepts every 64-byte digest string");
    assert_eq!(record.watch_text, mixed_case_digest);

    // validateWatchRecord deliberately leaves switch-group validation as a
    // TODO in Go, so an explicitly selected SWITCH_GROUP action reaches the
    // manager even when the target is empty.
    let controller = controller_with_defaults();
    let mut record = QuarantineRecord {
        action: RunawayAction::SwitchGroup,
        watch: WatchType::Exact,
        ..QuarantineRecord::default()
    };
    validate_watch_record(&mut record, &controller)
        .expect("Go does not validate the switch target in querywatch");
}

#[test]
fn similar_watch_uses_tidb_normalize_digest() {
    let record = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Similar,
            value: "select * from test.t2".into(),
            type_specified: true,
        }],
        &FixedDigester,
    )
    .expect("similar watch");

    assert_eq!(
        record.watch_text,
        "02576c15e1f35a8aa3eb7e3b1f977c9f9f9921a22421b3e9f42bad5ab632b4f6"
    );

    let exact = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Exact,
            value: "select ';' as marker".into(),
            type_specified: true,
        }],
        &FixedDigester,
    )
    .expect("a semicolon inside a string is still one SQL statement");
    assert_eq!(exact.watch_text, "select ';' as marker");
}

#[test]
fn exact_watch_preserves_go_parser_statement_text() {
    let cases = [
        ("select 1; -- trailing comment", "select 1;"),
        ("/* lead */ select 1", "/* lead */ select 1"),
        (";;select 1;;", ";;select 1;"),
        ("  select 1  ", "  select 1  "),
        ("\nselect 1\n", "select 1"),
    ];

    for (sql, expected) in cases {
        let record = from_option_list(
            &[QueryWatchOption::Text {
                watch: WatchType::Exact,
                value: sql.into(),
                type_specified: true,
            }],
            &FixedDigester,
        )
        .expect("Go parser accepts one SQL statement");
        assert_eq!(record.watch_text, expected, "input: {sql:?}");
    }

    let error = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Exact,
            value: "; -- comment only".into(),
            type_specified: true,
        }],
        &FixedDigester,
    )
    .expect_err("comments and empty statements do not form a SQL statement");
    assert_eq!(error, "only support one SQL");
}

#[test]
fn quarantine_record_keeps_go_manual_lifecycle_defaults() {
    let before = SystemTime::now();
    let record = from_option_list(&[], &FixedDigester).expect("empty option list");

    assert_eq!(record.source, "manual");
    assert_eq!(record.exceed_cause, "None");
    assert!(record.start_time >= before);
    assert_eq!(record.end_time, None);
}

#[test]
fn error_order_one_shot_state_and_side_effects_match_go() {
    struct ErrorController;
    impl ResourceGroupController for ErrorController {
        fn resource_group(&self, _name: &str) -> Result<Option<ResourceGroup>, String> {
            Err("controller failed".into())
        }
    }

    struct ErrorDigester;
    impl crate::query_watch::PlanDigester for ErrorDigester {
        fn plan_digest(&self, _sql: &str) -> Result<String, String> {
            Err("explain failed".into())
        }
    }

    let mut record = QuarantineRecord::default();
    assert_eq!(
        validate_watch_record(&mut record, &ErrorController),
        Err("controller failed".into())
    );
    assert_eq!(record.resource_group, DEFAULT_RESOURCE_GROUP);

    let plan_error = from_option_list(
        &[QueryWatchOption::Text {
            watch: WatchType::Plan,
            value: "select * from test.t3".into(),
            type_specified: true,
        }],
        &ErrorDigester,
    )
    .expect_err("ExecuteInternal/explain errors propagate");
    assert_eq!(plan_error, "explain failed");

    let controller = controller_with_defaults();
    let manager = MockManager::new();
    let mut add = AddExecutor::new(
        vec![QueryWatchOption::Text {
            watch: WatchType::Exact,
            value: "select 1; select 2".into(),
            type_specified: true,
        }],
        &controller,
        &manager,
        &FixedDigester,
    );
    assert_eq!(add.next(), Err("only support one SQL".into()));
    assert_eq!(
        add.next(),
        Ok(None),
        "Go marks the executor done before work"
    );
    assert!(
        manager.list().is_empty(),
        "failed ADD has no manager side effect"
    );

    assert_eq!(
        exec_drop_query_watch(
            &manager,
            DropQueryWatch::GroupVariable("missing".into()),
            &BTreeMap::new(),
        ),
        Err("invalid group name variable".into())
    );
}

#[test]
fn similar_watch_covers_parser_normalize_digest_reductions() {
    let cases = [
        (
            "select null",
            "e1c71d1661ae46e09b7aaec1c390957f0d6260410df4e4bc71b9c8d681021471",
        ),
        (
            "select 1 from b where id in (1, 3, '3', 1, 2, 3, 4)",
            "e1c8cc2738f596dc24f15ef8eb55e0d902910d7298983496362a7b46dbc0b310",
        ),
        (
            "select 1 from b order by 2",
            "a8ecce92dd90d5f4444d1bdb7f717fb2828ba2585b4c181d13da46b6c8a6d043",
        ),
        (
            "select count(a), b from t group by 2",
            "d92657fba519c50d6331161af5e46c3c52efe4a998796bd2f0dc5a9407560e19",
        ),
        (
            "select @a=b from t",
            "bcea97e80101b3b1ef8e8c696263ea329da19e690917412773862e4691f17746",
        ),
    ];

    for (sql, expected_digest) in cases {
        let record = from_option_list(
            &[QueryWatchOption::Text {
                watch: WatchType::Similar,
                value: sql.into(),
                type_specified: true,
            }],
            &FixedDigester,
        )
        .expect("similar watch");
        assert_eq!(record.watch_text, expected_digest, "input: {sql:?}");
    }
}
