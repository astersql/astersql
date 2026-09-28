// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
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

// `misc` 模块的 Aster 迁移单元测试。
//
// 覆盖 URL 脱敏、PLAN REPLAYER/LEADING/QUERY WATCH 还原、事务与权限语句、
// BRIE/流量安全文本，以及 SensitiveStatement 显式契约。

use crate::misc::*;

/// redacturlmatchesgoschemeandkeyrules。
#[test]
fn redact_url_matches_go_scheme_and_key_rules() {
    let cases = [
        ("", ""),
        ("~/file", "~/file"),
        ("s3://bucket/file", "s3://bucket/file"),
        (
            "gs://bucket/file?access-key=123",
            "gs://bucket/file?access-key=123",
        ),
        (
            "s3://bucket/file?access-key=123",
            "s3://bucket/file?access-key=xxxxxx",
        ),
        (
            "s3://bucket/file?secret_access_key=123",
            "s3://bucket/file?secret_access_key=xxxxxx",
        ),
        (
            "ks3://bucket/file?session-token=123",
            "ks3://bucket/file?session-token=xxxxxx",
        ),
        (
            "azure://container/file?account-name=test&account-key=123",
            "azure://container/file?account-key=xxxxxx&account-name=test",
        ),
        (
            "azblob://container/file?encryption_key=123",
            "azblob://container/file?encryption_key=xxxxxx",
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(redact_url(input), expected, "{input}");
    }
}

/// planreplayerrestorepreservesgobranchpriority。
#[test]
fn plan_replayer_restore_preserves_go_branch_priority() {
    assert_eq!(
        PlanReplayerStmt::load("x.zip").restore(),
        "PLAN REPLAYER LOAD 'x.zip'"
    );
    assert_eq!(
        PlanReplayerStmt::capture("sql", "plan").restore(),
        "PLAN REPLAYER CAPTURE 'sql' 'plan'"
    );
    assert_eq!(
        PlanReplayerStmt::remove("sql", "plan").restore(),
        "PLAN REPLAYER CAPTURE REMOVE 'sql' 'plan'"
    );
    assert_eq!(
        PlanReplayerStmt::dump_statements(true, ["select 1", "select 'x'"]).restore(),
        "PLAN REPLAYER DUMP EXPLAIN ANALYZE ('select 1', 'select \\'x\\'')"
    );
    assert_eq!(
        PlanReplayerStmt::dump_slow_query().restore(),
        "PLAN REPLAYER DUMP EXPLAIN SLOW QUERY"
    );
}

/// leadinglistflattensdepthfirstandrestoresqueryblock。
#[test]
fn leading_list_flattens_depth_first_and_restores_query_block() {
    let list = LeadingList::new(vec![
        LeadingItem::table(HintTable::new("test", "a")),
        LeadingItem::list(LeadingList::new(vec![
            LeadingItem::table(HintTable::bare("b")),
            LeadingItem::table(HintTable::bare("c")),
        ])),
    ]);
    let flattened = list.flatten();
    let names: Vec<_> = flattened
        .iter()
        .map(|table| table.table_name.as_str())
        .collect();
    assert_eq!(names, ["a", "b", "c"]);
    assert_eq!(
        list.restore_with_qb(Some("sel_1"), false),
        "@`sel_1` `test`.`a`, (`b`, `c`)"
    );

    let hint = TableOptimizerHint {
        hint_name: "leading".into(),
        qb_name: "sel_1".into(),
        data: HintData::Leading(list),
        ..TableOptimizerHint::default()
    };
    assert_eq!(
        hint.restore().unwrap(),
        "LEADING(@`sel_1` `test`.`a`, (`b`, `c`))"
    );
}

/// querywatchduplicatecheckandsecurepasswordmatchgo。
#[test]
fn query_watch_duplicate_check_and_secure_password_match_go() {
    let options = vec![
        QueryWatchOption::resource_group("rg1"),
        QueryWatchOption::kill(),
    ];
    assert!(!check_query_watch_append(
        &options,
        &QueryWatchOption::cooldown()
    ));
    assert!(check_query_watch_append(
        &options,
        &QueryWatchOption::sql_text_exact("select 1")
    ));
    assert_eq!(
        SetPwdStmt::current_user("secret", false).secure_text(),
        "set password"
    );
    assert_eq!(
        SetPwdStmt::named("u", "%", "secret", true).secure_text(),
        "set password for user u@% RETAIN CURRENT PASSWORD"
    );
}

/// roleandaccountrestorematchgoordering。
#[test]
fn role_and_account_restore_match_go_ordering() {
    let role = Identity::new("r", "%");
    let user = Identity::new("u", "localhost");
    assert_eq!(
        SetRoleStmt {
            option: SetRoleStmtType::AllExcept,
            roles: vec![role.clone()]
        }
        .restore(),
        "SET ROLE ALL EXCEPT 'r'@'%'"
    );
    assert_eq!(
        SetDefaultRoleStmt {
            option: SetRoleStmtType::Regular,
            roles: vec![role],
            users: vec![user.clone()]
        }
        .restore(),
        "SET DEFAULT ROLE 'r'@'%' TO 'u'@'localhost'"
    );

    let statement = CreateUserStmt {
        if_not_exists: true,
        specs: vec![UserSpec {
            user,
            auth: Some(AuthOption {
                by_auth_string: true,
                auth_string: "secret".into(),
                ..AuthOption::default()
            }),
            dual_password: DualPasswordOptionType::RetainCurrent,
            ..UserSpec::default()
        }],
        tls_options: vec![AuthTokenOrTlsOption {
            option_type: AuthTokenOrTlsOptionType::Ssl,
            value: String::new(),
        }],
        resource_options: vec![ResourceOption {
            option_type: ResourceOptionType::MaxQueriesPerHour,
            count: 10,
        }],
        password_options: vec![PasswordOrLockOption {
            option_type: PasswordOrLockOptionType::Lock,
            count: 0,
        }],
        ..CreateUserStmt::default()
    };
    assert_eq!(
        statement.restore().unwrap(),
        "CREATE USER IF NOT EXISTS 'u'@'localhost' IDENTIFIED BY 'secret' RETAIN CURRENT PASSWORD REQUIRE SSL WITH MAX_QUERIES_PER_HOUR 10 ACCOUNT LOCK"
    );
    assert_eq!(
        statement.secure_text(),
        "create user {'u'@'localhost' password = *** RETAIN CURRENT PASSWORD}"
    );
}

/// transactionprepareandflushrestorematchgo。
#[test]
fn transaction_prepare_and_flush_restore_match_go() {
    assert_eq!(
        PrepareStmt {
            name: "s".into(),
            sql_text: "select 1".into(),
            sql_var: None
        }
        .restore()
        .unwrap(),
        "PREPARE `s` FROM 'select 1'"
    );
    assert_eq!(
        BeginStmt {
            read_only: true,
            as_of: Some("AS OF TIMESTAMP '2020-01-01'".into()),
            ..BeginStmt::default()
        }
        .restore(),
        "START TRANSACTION READ ONLY AS OF TIMESTAMP '2020-01-01'"
    );
    assert_eq!(
        RollbackStmt {
            completion_type: CompletionType::Chain,
            savepoint_name: "sp".into()
        }
        .restore(),
        "ROLLBACK TO sp AND CHAIN"
    );
    assert_eq!(
        FlushStmt {
            statement_type: FlushStmtType::Logs,
            no_write_to_binlog: false,
            log_type: LogType::Slow,
            tables: vec![],
            read_lock: false,
            plugins: vec![],
            is_cluster: false,
            flush_objects: vec![]
        }
        .restore()
        .unwrap(),
        "FLUSH SLOW LOGS"
    );
}

/// admingrantandbrierestorematchgo。
#[test]
fn admin_grant_and_brie_restore_match_go() {
    let admin = AdminStmt {
        statement_type: AdminStmtType::ShowDdlJobQueriesWithRange,
        index: String::new(),
        tables: vec![],
        job_ids: vec![],
        job_number: 0,
        handle_ranges: vec![],
        show_slow: None,
        plugins: vec![],
        where_clause: None,
        scope: StatementScope::None,
        limit: LimitSimple {
            count: 10,
            offset: 5,
        },
        bdr_role: BdrRole::None,
        alter_job_options: vec![],
    };
    assert_eq!(
        admin.restore().unwrap(),
        "ADMIN SHOW DDL JOB QUERIES LIMIT 5, 10"
    );
    let grant = GrantStmt {
        privileges: vec![PrivElem {
            privilege: "select".into(),
            ..PrivElem::default()
        }],
        object_type: ObjectTypeType::Table,
        level: GrantLevel {
            level: GrantLevelType::Table,
            db_name: "test".into(),
            table_name: "t".into(),
        },
        users: vec![UserSpec {
            user: Identity::new("u", "%"),
            ..UserSpec::default()
        }],
        with_grant: true,
        ..GrantStmt::default()
    };
    assert_eq!(
        grant.restore().unwrap(),
        "GRANT SELECT ON TABLE `test`.`t` TO 'u'@'%' WITH GRANT OPTION"
    );
    let backup = BRIEStmt {
        kind: BrieKind::Backup,
        tables: vec!["`test`.`t`".into()],
        storage: "s3://bucket/file?access-key=secret".into(),
        ..BRIEStmt::default()
    };
    assert_eq!(
        backup.secure_text(),
        "BACKUP TABLE `test`.`t` TO 's3://bucket/file?access-key=xxxxxx'"
    );
}

/// hintsresourceandvisitorareexecutable。
#[test]
fn hints_resource_and_visitor_are_executable() {
    let hint = TableOptimizerHint {
        hint_name: "use_index".into(),
        tables: vec![HintTable::new("test", "t")],
        indexes: vec!["idx".into()],
        ..TableOptimizerHint::default()
    };
    assert_eq!(hint.restore().unwrap(), "USE_INDEX(`test`.`t` `idx`)");
    assert_eq!(
        CalibrateResourceStmt {
            resource_type: CalibrateResourceType::Tpcc,
            options: vec![]
        }
        .restore(),
        "CALIBRATE RESOURCE WORKLOAD TPCC"
    );
    struct Recorder(Vec<&'static str>);
    impl MiscVisitor for Recorder {
        fn enter(&mut self, name: &'static str) -> bool {
            self.0.push(name);
            false
        }
        fn leave(&mut self, name: &'static str) -> bool {
            self.0.push(name);
            true
        }
    }
    let mut stmt = ShutdownStmt;
    let mut recorder = Recorder(Vec::new());
    assert!(stmt.accept(&mut recorder));
    assert_eq!(recorder.0.len(), 2);
}

/// trafficandquerywatchsecurerestorematchgo。
#[test]
fn traffic_and_query_watch_secure_restore_match_go() {
    let traffic = TrafficStmt {
        operation: TrafficOpType::Replay,
        directory:
            "s3://bucket/prefix?access-key=abcdef&secret-access-key=123&force-path-style=true"
                .into(),
        options: vec![
            TrafficOption {
                option_type: TrafficOptionType::Username,
                string_value: "root".into(),
                uint_value: 0,
                float_value: String::new(),
                bool_value: false,
            },
            TrafficOption {
                option_type: TrafficOptionType::Password,
                string_value: "123456".into(),
                uint_value: 0,
                float_value: String::new(),
                bool_value: false,
            },
        ],
    };
    assert_eq!(
        traffic.secure_text(),
        "TRAFFIC REPLAY FROM 's3://bucket/prefix?access-key=xxxxxx&force-path-style=true&secret-access-key=xxxxxx' USER = 'root' PASSWORD = 'xxxxxx'"
    );
    let watch = AddQueryWatchStmt {
        options: vec![
            QueryWatchOption::resource_group("rg1"),
            QueryWatchOption::kill(),
            QueryWatchOption::sql_text_exact("select * from t"),
        ],
    };
    assert_eq!(
        watch.restore(),
        "QUERY WATCH ADD RESOURCE GROUP `rg1` ACTION = KILL SQL TEXT EXACT TO 'select * from t'"
    );
}

/// planreplayerfulldumpandcompactrestorematchgo。
#[test]
fn plan_replayer_full_dump_and_compact_restore_match_go() {
    let plan = PlanReplayerStmt {
        statement: Some("SELECT * FROM `t` WHERE `a`>10".into()),
        analyze: true,
        historical_stats: Some("AS OF TIMESTAMP 12345".into()),
        ..PlanReplayerStmt::default()
    };
    assert_eq!(
        plan.restore(),
        "PLAN REPLAYER DUMP WITH STATS AS OF TIMESTAMP 12345 EXPLAIN ANALYZE SELECT * FROM `t` WHERE `a`>10"
    );
    assert_eq!(
        CompactTableStmt {
            table: "`test`.`abc`".into(),
            partitions: vec![],
            replica_kind: CompactReplicaKind::TiFlash
        }
        .restore(),
        "ALTER TABLE `test`.`abc` COMPACT TIFLASH REPLICA"
    );
}

/// sensitivestatementcontractisexplicit。
#[test]
fn sensitive_statement_contract_is_explicit() {
    fn secure(value: &dyn SensitiveStatement) -> String {
        value.secure_sql()
    }
    assert_eq!(
        secure(&SetPwdStmt::current_user("secret", false)),
        "set password"
    );
    assert!(secure(&CreateUserStmt::default()).starts_with("create user"));
}
