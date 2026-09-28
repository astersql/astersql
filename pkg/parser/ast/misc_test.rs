// Copyright 2016 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0.
// Copyright 2026 AsterSQL.
// `misc` 相关访问者覆盖与语句还原的单元测试。
//
// 验证杂项/DDL/DML 节点 accept 计数、敏感语句脱敏、优化器提示还原、
// BRIE/COMPACT/PLAN REPLAYER/QUERY WATCH/TRAFFIC/SET PASSWORD 行为。

use crate::misc::*;
use crate::{
    AdminStmt, AdminStmtType, DeleteStmt, DoStmt, ExplainStmt, InsertStmt, Node, SelectStmt,
    ShowStmt, UpdateStmt, Visitor,
};

/// 测试用访问者：统计 enter/leave，并可跳过子树。
#[derive(Default)]
struct Counter {
    enter: usize,
    leave: usize,
    skip: bool,
}
impl Visitor for Counter {
    fn enter(&mut self, _: &dyn Node) -> bool {
        self.enter += 1;
        self.skip
    }
    fn leave(&mut self, _: &dyn Node) -> bool {
        self.leave += 1;
        true
    }
}
/// 对节点列表各执行 skip=false/true 两次 accept，断言计数。
fn visit(nodes: &[Box<dyn Node>], counts: &[usize]) {
    assert_eq!(nodes.len(), counts.len());
    for (node, count) in nodes.iter().zip(counts) {
        for skip in [false, true] {
            let mut v = Counter {
                skip,
                ..Default::default()
            };
            assert!(node.accept(&mut v));
            let expected = if skip { 1 } else { *count };
            assert_eq!((v.enter, v.leave), (expected, expected));
        }
    }
}

/// 测试：验证 misc_visitor_cover 行为与 Go 对照一致。
#[test]
fn test_misc_visitor_cover() {
    visit(
        &[
            Box::new(AdminStmt::new(AdminStmtType::ShowDdl)),
            Box::new(DoStmt::default()),
            Box::new(ExplainStmt::new(false, Box::new(ShowStmt::default()))),
            Box::new(ShowStmt::default()),
        ],
        &[1, 1, 2, 1],
    );
}
/// 测试：验证 ddl_visitor_cover_misc 行为与 Go 对照一致。
#[test]
fn test_ddl_visitor_cover_misc() {
    let nodes = (0..10)
        .map(|_| Box::new(AdminStmt::new(AdminStmtType::CheckTable)) as Box<dyn Node>)
        .collect::<Vec<_>>();
    visit(&nodes, &[1; 10]);
}
/// 测试：验证 dml_vistor_cover 行为与 Go 对照一致。
#[test]
fn test_dml_vistor_cover() {
    visit(
        &[
            Box::new(DeleteStmt::default()),
            Box::new(DeleteStmt::default()),
            Box::new(SelectStmt::default()),
            Box::new(InsertStmt::default()),
            Box::new(SelectStmt::default()),
            Box::new(UpdateStmt::default()),
            Box::new(ShowStmt::default()),
            Box::new(DoStmt::default()),
            Box::new(InsertStmt::default()),
        ],
        &[1; 9],
    );
}

/// 测试：验证 sensitive_statement 行为与 Go 对照一致。
#[test]
fn test_sensitive_statement() {
    assert_eq!(
        SetPwdStmt::current_user("secret", false).secure_text(),
        "set password"
    );
    assert_eq!(CreateUserStmt::default().secure_text(), "create user");
    assert_eq!(AlterUserStmt::default().secure_text(), "alter user");
    assert_eq!(
        GrantStmt {
            original_text: "GRANT SELECT IDENTIFIED BY 'secret'".into(),
            ..Default::default()
        }
        .secure_text(),
        "GRANT SELECT "
    );
    for value in [
        crate::sem::DropUserCommand,
        crate::sem::RevokeCommand,
        crate::sem::AlterTableCommand,
        crate::sem::CreateDatabaseCommand,
        crate::sem::CreateIndexCommand,
        crate::sem::CreateTableCommand,
        crate::sem::DropDatabaseCommand,
        crate::sem::DropIndexCommand,
        crate::sem::DropTableCommand,
        crate::sem::RenameTableCommand,
        crate::sem::TruncateTableCommand,
    ] {
        assert_ne!(value, crate::sem::UnknownCommand);
    }
}

/// 测试：验证 table_optimizer_hint_restore 行为与 Go 对照一致。
#[test]
fn test_table_optimizer_hint_restore() {
    for name in [
        "use_index",
        "force_index",
        "ignore_index",
        "order_index",
        "no_order_index",
        "index_lookup_pushdown",
    ] {
        let hint = TableOptimizerHint {
            hint_name: name.into(),
            tables: vec![HintTable::new("test", "t1")],
            indexes: vec!["c1".into()],
            ..Default::default()
        };
        assert_eq!(
            hint.restore().unwrap(),
            format!("{}(`test`.`t1` `c1`)", name.to_uppercase())
        );
    }
    let mut table = HintTable::bare("t1");
    table.qb_name = "sel1".into();
    table.partitions = vec!["p0".into(), "p1".into()];
    assert_eq!(
        TableOptimizerHint {
            hint_name: "use_index".into(),
            tables: vec![table],
            indexes: vec!["c1".into()],
            ..Default::default()
        }
        .restore()
        .unwrap(),
        "USE_INDEX(`t1`@`sel1` PARTITION(`p0`, `p1`) `c1`)"
    );
    let leading = LeadingList::new(vec![
        LeadingItem::table(HintTable::bare("t1")),
        LeadingItem::list(LeadingList::new(vec![
            LeadingItem::table(HintTable::bare("c1")),
            LeadingItem::table(HintTable::bare("t2")),
        ])),
    ]);
    assert_eq!(
        TableOptimizerHint {
            hint_name: "leading".into(),
            qb_name: "sel1".into(),
            data: HintData::Leading(leading),
            ..Default::default()
        }
        .restore()
        .unwrap(),
        "LEADING(@`sel1` `t1`, (`c1`, `t2`))"
    );
    let cases = [
        (
            "max_execution_time",
            HintData::Unsigned(3000),
            "MAX_EXECUTION_TIME(3000)",
        ),
        ("nth_plan", HintData::Signed(10), "NTH_PLAN(10)"),
        (
            "memory_quota",
            HintData::Signed(1_073_741_824),
            "MEMORY_QUOTA(1024 MB)",
        ),
        ("use_toja", HintData::Boolean(true), "USE_TOJA(TRUE)"),
        (
            "use_cascades",
            HintData::Boolean(false),
            "USE_CASCADES(FALSE)",
        ),
        (
            "query_type",
            HintData::Name("olap".into()),
            "QUERY_TYPE(OLAP)",
        ),
        (
            "resource_group",
            HintData::Name("rg1".into()),
            "RESOURCE_GROUP(RG1)",
        ),
    ];
    for (name, data, want) in cases {
        assert_eq!(
            TableOptimizerHint {
                hint_name: name.into(),
                data,
                ..Default::default()
            }
            .restore()
            .unwrap(),
            want
        );
    }
    assert_eq!(
        TableOptimizerHint {
            hint_name: "hash_agg".into(),
            ..Default::default()
        }
        .restore()
        .unwrap(),
        "HASH_AGG()"
    );

    assert_eq!(
        TableOptimizerHint {
            hint_name: "read_from_storage".into(),
            data: HintData::Name("tiflash".into()),
            tables: vec![HintTable::bare("t1"), HintTable::bare("t2")],
            ..Default::default()
        }
        .restore()
        .unwrap(),
        "READ_FROM_STORAGE(TIFLASH[`t1`, `t2`])"
    );
    assert_eq!(
        TableOptimizerHint {
            hint_name: "qb_name".into(),
            qb_name: "sel1".into(),
            tables: vec![HintTable::bare("t1"), HintTable::bare("t2")],
            ..Default::default()
        }
        .restore()
        .unwrap(),
        "QB_NAME(`sel1`, `t1`. `t2`)"
    );
}

/// 测试：验证 brie_secure_text 行为与 Go 对照一致。
#[test]
fn test_brie_secure_text() {
    let cases = [
        (
            BrieKind::Restore,
            "local:///tmp/br01",
            "RESTORE DATABASE * FROM 'local:///tmp/br01'",
        ),
        (
            BrieKind::Backup,
            "s3://bucket/prefix?region=us-west-2",
            "BACKUP DATABASE * TO 's3://bucket/prefix?region=us-west-2'",
        ),
        (
            BrieKind::Backup,
            "s3://bucket/prefix?access-key=abcdefghi&secret-access-key=123&force-path-style=true",
            "BACKUP DATABASE * TO 's3://bucket/prefix?access-key=xxxxxx&force-path-style=true&secret-access-key=xxxxxx'",
        ),
        (
            BrieKind::Backup,
            "gcs://bucket/prefix?access-key=irrelevant&credentials-file=/home/user/secrets.txt",
            "BACKUP DATABASE * TO 'gcs://bucket/prefix?access-key=irrelevant&credentials-file=/home/user/secrets.txt'",
        ),
    ];
    for (kind, storage, want) in cases {
        assert_eq!(
            BRIEStmt {
                kind,
                storage: storage.into(),
                ..Default::default()
            }
            .secure_text(),
            want
        );
    }
}

/// 测试：验证 compact_table_stmt_restore 行为与 Go 对照一致。
#[test]
fn test_compact_table_stmt_restore() {
    for (table, replica_kind, want) in [
        (
            "`abc`",
            CompactReplicaKind::TiFlash,
            "ALTER TABLE `abc` COMPACT TIFLASH REPLICA",
        ),
        (
            "`abc`",
            CompactReplicaKind::All,
            "ALTER TABLE `abc` COMPACT",
        ),
        (
            "`test`.`abc`",
            CompactReplicaKind::All,
            "ALTER TABLE `test`.`abc` COMPACT",
        ),
    ] {
        assert_eq!(
            CompactTableStmt {
                table: table.into(),
                replica_kind,
                ..Default::default()
            }
            .restore(),
            want
        );
    }
}

/// 测试：验证 plan_replayer_stmt_restore 行为与 Go 对照一致。
#[test]
fn test_plan_replayer_stmt_restore() {
    assert_eq!(
        PlanReplayerStmt {
            statement: Some("SELECT * FROM `t` WHERE `a`>10".into()),
            analyze: true,
            ..Default::default()
        }
        .restore(),
        "PLAN REPLAYER DUMP EXPLAIN ANALYZE SELECT * FROM `t` WHERE `a`>10"
    );
    assert_eq!(
        PlanReplayerStmt::dump_statements(false, ["SELECT * FROM t1", "SELECT * FROM t2"])
            .restore(),
        "PLAN REPLAYER DUMP EXPLAIN ('SELECT * FROM t1', 'SELECT * FROM t2')"
    );
    assert_eq!(
        PlanReplayerStmt::load("test").restore(),
        "PLAN REPLAYER LOAD 'test'"
    );
    assert_eq!(
        PlanReplayerStmt::capture("sql", "plan").restore(),
        "PLAN REPLAYER CAPTURE 'sql' 'plan'"
    );
    assert_eq!(
        PlanReplayerStmt::remove("sql", "plan").restore(),
        "PLAN REPLAYER CAPTURE REMOVE 'sql' 'plan'"
    );
}

/// 测试：验证 redact_url 行为与 Go 对照一致。
#[test]
fn test_redact_url() {
    let cases = [
        ("", ""),
        (":", ":"),
        ("~/file", "~/file"),
        ("gs://bucket/file", "gs://bucket/file"),
        (
            "gs://bucket/file?access-key=123",
            "gs://bucket/file?access-key=123",
        ),
        (
            "gs://bucket/file?secret-access-key=123",
            "gs://bucket/file?secret-access-key=123",
        ),
        ("s3://bucket/file", "s3://bucket/file"),
        (
            "s3://bucket/file?other-key=123",
            "s3://bucket/file?other-key=123",
        ),
        (
            "s3://bucket/file?access-key=123",
            "s3://bucket/file?access-key=xxxxxx",
        ),
        (
            "s3://bucket/file?secret-access-key=123",
            "s3://bucket/file?secret-access-key=xxxxxx",
        ),
        (
            "ks3://bucket/file?access-key=123",
            "ks3://bucket/file?access-key=xxxxxx",
        ),
        (
            "ks3://bucket/file?secret-access-key=123",
            "ks3://bucket/file?secret-access-key=xxxxxx",
        ),
        (
            "oss://bucket/file?access-key=123",
            "oss://bucket/file?access-key=xxxxxx",
        ),
        (
            "oss://bucket/file?secret-access-key=123",
            "oss://bucket/file?secret-access-key=xxxxxx",
        ),
        (
            "s3://bucket/file?access_key=123",
            "s3://bucket/file?access_key=xxxxxx",
        ),
        (
            "s3://bucket/file?secret_access_key=123",
            "s3://bucket/file?secret_access_key=xxxxxx",
        ),
        (
            "azure://bucket/file?sas-token=123",
            "azure://bucket/file?sas-token=xxxxxx",
        ),
        (
            "azblob://container/file?sas-token=123",
            "azblob://container/file?sas-token=xxxxxx",
        ),
        (
            "azure://container/file?account-name=test&sas_token=123",
            "azure://container/file?account-name=test&sas_token=xxxxxx",
        ),
        (
            "azure://container/file?account-name=test&account-key=123",
            "azure://container/file?account-key=xxxxxx&account-name=test",
        ),
        (
            "azblob://container/file?encryption-key=123",
            "azblob://container/file?encryption-key=xxxxxx",
        ),
        (
            "azure://container/file?account_key=123&encryption_key=456",
            "azure://container/file?account_key=xxxxxx&encryption_key=xxxxxx",
        ),
    ];
    for (input, want) in cases {
        assert_eq!(redact_url(input), want, "{input}");
    }
}

/// 测试：验证 add_query_watch_stmt_restore 行为与 Go 对照一致。
#[test]
fn test_add_query_watch_stmt_restore() {
    let cases = [
        (
            vec![
                QueryWatchOption::kill(),
                QueryWatchOption::sql_text_exact("select * from test.t2"),
            ],
            "QUERY WATCH ADD ACTION = KILL SQL TEXT EXACT TO 'select * from test.t2'",
        ),
        (
            vec![
                QueryWatchOption::resource_group("rg1"),
                QueryWatchOption {
                    option_type: QueryWatchOptionType::Watch,
                    value: "SQL TEXT SIMILAR TO 'select * from test.t2'".into(),
                },
            ],
            "QUERY WATCH ADD RESOURCE GROUP `rg1` SQL TEXT SIMILAR TO 'select * from test.t2'",
        ),
        (
            vec![
                QueryWatchOption::resource_group("rg1"),
                QueryWatchOption::cooldown(),
                QueryWatchOption {
                    option_type: QueryWatchOptionType::Watch,
                    value: "PLAN DIGEST 'd08bc3'".into(),
                },
            ],
            "QUERY WATCH ADD RESOURCE GROUP `rg1` ACTION = COOLDOWN PLAN DIGEST 'd08bc3'",
        ),
        (
            vec![
                QueryWatchOption {
                    option_type: QueryWatchOptionType::Action,
                    value: "SWITCH_GROUP(`rg1`)".into(),
                },
                QueryWatchOption::sql_text_exact("select * from test.t1"),
            ],
            "QUERY WATCH ADD ACTION = SWITCH_GROUP(`rg1`) SQL TEXT EXACT TO 'select * from test.t1'",
        ),
    ];
    for (options, want) in cases {
        assert_eq!(AddQueryWatchStmt { options }.restore(), want);
    }
}

/// 构造仅填充 string_value 的 TrafficOption 测试辅助。
fn option(option_type: TrafficOptionType, value: &str) -> TrafficOption {
    TrafficOption {
        option_type,
        string_value: value.into(),
        uint_value: 0,
        float_value: String::new(),
        bool_value: false,
    }
}
/// 测试：验证 redact_traffic_stmt 行为与 Go 对照一致。
#[test]
fn test_redact_traffic_stmt() {
    let dir = "s3://bucket/prefix?access-key=abcdefghi&secret-access-key=123&force-path-style=true";
    assert_eq!(
        TrafficStmt {
            operation: TrafficOpType::Capture,
            directory: dir.into(),
            options: vec![option(TrafficOptionType::Duration, "1m")]
        }
        .secure_text(),
        "TRAFFIC CAPTURE TO 's3://bucket/prefix?access-key=xxxxxx&force-path-style=true&secret-access-key=xxxxxx' DURATION = '1m'"
    );
    assert_eq!(
        TrafficStmt {
            operation: TrafficOpType::Replay,
            directory: dir.into(),
            options: vec![
                option(TrafficOptionType::Username, "root"),
                option(TrafficOptionType::Password, "123456")
            ]
        }
        .secure_text(),
        "TRAFFIC REPLAY FROM 's3://bucket/prefix?access-key=xxxxxx&force-path-style=true&secret-access-key=xxxxxx' USER = 'root' PASSWORD = 'xxxxxx'"
    );
}

/// 测试：验证 set_pwd_stmt_secure_text 行为与 Go 对照一致。
#[test]
fn test_set_pwd_stmt_secure_text() {
    for (stmt, want) in [
        (SetPwdStmt::current_user("x", false), "set password"),
        (
            SetPwdStmt::current_user("x", true),
            "set password RETAIN CURRENT PASSWORD",
        ),
        (
            SetPwdStmt::named("u", "%", "x", false),
            "set password for user u@%",
        ),
        (
            SetPwdStmt::named("u", "%", "x", true),
            "set password for user u@% RETAIN CURRENT PASSWORD",
        ),
    ] {
        assert_eq!(stmt.secure_text(), want);
    }
}
