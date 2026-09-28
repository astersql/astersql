// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
//
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// DDL 运行时端到端回归测试。
//
// 通过真实会话执行 DDL、DML 与管理语句，校验模式变更会同步反映到持久化行、
// Domain 元数据和后续会话中，并覆盖索引校验、区域散布及失败重试等生产路径。

use super::*;

#[test]
// USE 必须同步规划器读取的当前库，覆盖 WordPress 的未限定表聚合查询。
fn use_updates_planner_current_database_for_wordpress_queries() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database wordpress_runtime")
        .expect("create WordPress database");
    session
        .execute("use wordpress_runtime")
        .expect("select WordPress database");
    session
        .execute(
            "create table wp_comments (comment_ID bigint primary key, \
             comment_approved varchar(20) collate utf8mb4_unicode_520_ci, \
             comment_type varchar(20) collate utf8mb4_unicode_520_ci)",
        )
        .expect("create WordPress comments table");
    session
        .execute("insert into wp_comments values (1, '1', ''), (2, '0', 'note')")
        .expect("seed WordPress comments");

    let mut tables = session
        .execute("show tables like 'wp_comments'")
        .expect("filter WordPress table probe");
    assert_eq!(
        tables[0].Next().expect("read matched WordPress table"),
        Some(vec!["wp_comments".to_owned()])
    );
    assert_eq!(
        tables[0].Next().expect("finish WordPress table probe"),
        None
    );

    let mut result = session
        .execute(
            "select count(*) from wp_comments \
             where comment_approved = '1' and comment_type not in ('note')",
        )
        .expect("plan WordPress count in selected database");
    assert_eq!(
        result[0].Next().expect("read WordPress count"),
        Some(vec!["1".to_owned()])
    );

    session
        .execute(
            "create table wp_posts (ID bigint primary key, post_date datetime, \
             post_type varchar(20), post_status varchar(20))",
        )
        .expect("create WordPress posts table");
    session
        .execute("create table wp_term_relationships (object_id bigint, term_taxonomy_id bigint)")
        .expect("create WordPress term relationships table");
    session
        .execute(
            "insert into wp_posts values \
             (1, '2026-01-01 00:00:00', 'wp_global_styles', 'publish')",
        )
        .expect("seed WordPress posts");
    session
        .execute("insert into wp_term_relationships values (1, 2)")
        .expect("seed WordPress term relationships");
    let mut styles = session
        .execute(
            "select wp_posts.ID from wp_posts \
             left join wp_term_relationships \
             on (wp_posts.ID = wp_term_relationships.object_id) \
             where wp_term_relationships.term_taxonomy_id in (2) \
             and wp_posts.post_type = 'wp_global_styles' \
             and wp_posts.post_status = 'publish' \
             group by wp_posts.ID order by wp_posts.post_date desc limit 1",
        )
        .expect("order grouped WordPress styles by an unprojected column");
    assert_eq!(
        styles[0].Next().expect("read WordPress global style"),
        Some(vec!["1".to_owned()])
    );
}

#[test]
// 验证数据库的创建、选用和删除会驱动表数据及 Domain 元数据一致变化。
fn concrete_database_session_state_drives_ddl_dml_and_admin_check_table() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");

    session
        .execute("create database Company")
        .expect("create user database");
    assert!(
        session.execute("create database company").is_err(),
        "duplicate CREATE DATABASE without IF NOT EXISTS must fail"
    );
    session
        .execute("create database if not exists company")
        .expect("idempotent create database");
    session
        .execute("use COMPANY")
        .expect("select user database");
    session
        .execute("create table employee (id bigint primary key, name varchar(32))")
        .expect("create table in selected database");
    assert!(
        session
            .domain()
            .stats_table("company", "employee")
            .is_some()
    );

    session
        .execute("insert into employee (id, name) values (1, 'Ada')")
        .expect("insert through selected database");
    let mut rows = session
        .execute("select id, name from employee")
        .expect("select through selected database");
    assert_eq!(
        rows[0].Next().expect("read selected employee"),
        Some(vec!["1".to_owned(), "Ada".to_owned()])
    );
    session
        .execute("admin check table employee")
        .expect("check existing table");
    assert!(
        session
            .execute("admin check table missing_employee")
            .is_err(),
        "ADMIN CHECK TABLE must reject an absent table"
    );

    session
        .execute("drop database company")
        .expect("drop selected database");
    assert!(
        session
            .domain()
            .stats_table("company", "employee")
            .is_none()
    );
    assert!(session.execute("use company").is_err());
    assert!(session.execute("drop database company").is_err());
    session
        .execute("drop database if exists company")
        .expect("idempotent drop database");
}

#[test]
// 空数据库也必须跨会话可见，并允许由共享同一 Domain 的其他会话删除。
fn empty_database_is_visible_to_other_sessions_and_can_be_cleaned_up() {
    let (domain, creator) = crate::runtime::CreateAnalyzeSession().expect("creator session");
    creator
        .execute("create database empty_runtime_schema")
        .expect("create empty database");

    let observer = crate::runtime::ConcreteSession::new(domain);
    let mut databases = observer.execute("show databases").expect("show databases");
    let mut names = Vec::new();
    while let Some(row) = databases[0].Next().expect("read database row") {
        names.push(row[0].clone());
    }
    assert!(names.iter().any(|name| name == "empty_runtime_schema"));

    observer
        .execute("drop database empty_runtime_schema")
        .expect("drop empty database from another session");
    assert!(creator.execute("use empty_runtime_schema").is_err());
}

#[test]
// ALTER 校验必须读取既有数据，并正确维护外键、隐式索引和表达式索引元数据。
fn concrete_alter_index_and_foreign_key_paths_use_persisted_rows() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database ddl_runtime")
        .expect("create DDL database");
    session
        .execute("use ddl_runtime")
        .expect("select DDL database");
    session
        .execute("create table employee (id bigint auto_increment key, pid bigint)")
        .expect("create employee");
    session
        .execute("insert into employee (id) values (1),(2)")
        .expect("seed employees");
    session
        .execute("insert into employee (pid) select pid from employee")
        .expect("same-table INSERT SELECT");
    let mut count = session
        .execute("select count(*) from employee")
        .expect("count employees");
    assert_eq!(
        count[0].Next().expect("read employee count"),
        Some(vec!["4".to_owned()])
    );

    session
        .execute("update employee set pid=id")
        .expect("make self-reference valid");
    session
        .execute("alter table employee add foreign key fk_1(pid) references employee(id)")
        .expect("add checked foreign key");
    let table = session
        .domain()
        .stats_table("ddl_runtime", "employee")
        .expect("employee metadata")
        .1;
    assert_eq!(table.ForeignKeys.len(), 1);
    assert!(table.Indices.iter().any(|index| index.Name.L == "fk_1"));
    session
        .execute("alter table employee drop foreign key fk_1")
        .expect("drop foreign key metadata");
    session
        .execute("alter table employee drop index fk_1")
        .expect("drop auto-created index");
    session
        .execute("update employee set pid=0 where id=1")
        .expect("make self-reference invalid");
    let error = session
        .execute("alter table employee add foreign key fk_1(pid) references employee(id)")
        .err()
        .expect("invalid existing child row must reject the foreign key");
    assert!(error.to_string().contains("[ddl:1452]"));

    session
        .execute("create table duplicate_key (a bigint primary key, b int)")
        .expect("create unique-index fixture");
    session
        .execute("insert into duplicate_key values (1,7),(2,7)")
        .expect("seed duplicate values");
    let error = session
        .execute("alter table duplicate_key add unique index uk(b)")
        .err()
        .expect("existing duplicates must reject a unique index");
    assert!(
        error
            .to_string()
            .contains("Duplicate entry '7' for key 'duplicate_key.uk'")
    );

    session
        .execute("create table expression_key (j json, payload text)")
        .expect("create expression-index fixture");
    session
        .execute(
            "alter table expression_key add index idx_expr((cast(j as signed array)),payload(5))",
        )
        .expect("materialize expression index hidden column");
    session
        .execute("admin check table expression_key")
        .expect("check expression-index table");
    let table = session
        .domain()
        .stats_table("ddl_runtime", "expression_key")
        .expect("expression table metadata")
        .1;
    assert!(table.Columns.iter().any(|column| column.Hidden));
    assert!(table.Indices.iter().any(|index| index.Name.L == "idx_expr"));
}

#[test]
// CREATE INDEX 与 ADMIN CHECK INDEX 应走正式 ALTER 路径并查询已持久化的索引定义。
fn concrete_repeat_create_index_and_admin_check_index_use_production_paths() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database index_runtime")
        .expect("create index runtime database");
    session
        .execute("use index_runtime")
        .expect("select index runtime database");
    session
        .execute("create table t (id bigint primary key, payload varchar(32))")
        .expect("create index fixture");
    session
        .execute("insert into t values (1, repeat('ab', 2))")
        .expect("REPEAT in INSERT");
    session
        .execute("update t set payload=repeat(payload, 2) where id=1")
        .expect("REPEAT in UPDATE");
    let mut rows = session
        .execute("select payload from t where id=1")
        .expect("read repeated payload");
    assert_eq!(
        rows[0].Next().expect("payload row"),
        Some(vec!["abababab".to_owned()])
    );

    session
        .execute("create index idx_payload on t(payload)")
        .expect("CREATE INDEX through ALTER production path");
    session
        .execute("admin check index t idx_payload")
        .expect("ADMIN CHECK INDEX resolves the persisted index");
    let error = session
        .execute("admin check index t missing_idx")
        .err()
        .expect("ADMIN CHECK INDEX must reject an absent index");
    assert!(error.to_string().contains("unknown index missing_idx"));

    let table = session
        .domain()
        .stats_table("index_runtime", "t")
        .expect("index fixture metadata")
        .1;
    assert!(
        table
            .Indices
            .iter()
            .any(|index| index.Name.L == "idx_payload")
    );
}

#[test]
// 复合谓词的更新删除需精确命中行，匿名索引名则沿用 MySQL 的首列推导规则。
fn concrete_composite_predicates_and_anonymous_indexes_follow_mysql_ddl_dml() {
    let (_domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database composite_runtime")
        .expect("create composite database");
    session
        .execute("use composite_runtime")
        .expect("select composite database");
    session
        .execute("create table t (id1 varchar(16), id2 int, payload int, primary key(id1,id2))")
        .expect("create composite table");
    session
        .execute("insert into t values ('a',1,10),('a',2,20),('b',1,30)")
        .expect("seed composite rows");
    session
        .execute("update t set payload=99 where id1='a' and id2=2")
        .expect("update one composite row");
    let mut rows = session
        .execute("select id1,id2,payload from t order by id1,id2")
        .expect("read updated composite rows");
    assert_eq!(
        rows[0].Next().expect("first updated composite row"),
        Some(vec!["a".to_owned(), "1".to_owned(), "10".to_owned()])
    );
    assert_eq!(
        rows[0].Next().expect("second updated composite row"),
        Some(vec!["a".to_owned(), "2".to_owned(), "99".to_owned()])
    );
    assert_eq!(
        rows[0].Next().expect("third updated composite row"),
        Some(vec!["b".to_owned(), "1".to_owned(), "30".to_owned()])
    );
    assert_eq!(rows[0].Next().expect("end updated composite rows"), None);
    session
        .execute("delete from t where id1='a' and id2=1")
        .expect("delete one composite row");
    let mut rows = session
        .execute("select id1,id2,payload from t order by id1,id2")
        .expect("read composite rows");
    assert_eq!(
        rows[0].Next().expect("first composite row"),
        Some(vec!["a".to_owned(), "2".to_owned(), "99".to_owned()])
    );
    assert_eq!(
        rows[0].Next().expect("second composite row"),
        Some(vec!["b".to_owned(), "1".to_owned(), "30".to_owned()])
    );
    assert_eq!(rows[0].Next().expect("end composite rows"), None);

    session
        .execute("alter table t add index(payload)")
        .expect("derive anonymous index name from first column");
    let table = session
        .domain()
        .stats_table("composite_runtime", "t")
        .expect("composite table metadata")
        .1;
    assert!(table.Indices.iter().any(|index| index.Name.L == "payload"));
}

#[test]
// 校验会话级散布范围、全局变量继承，以及由表元数据推导出的区域拓扑。
fn scatter_scope_inherits_globally_and_show_regions_comes_from_table_metadata() {
    use std::sync::Mutex;

    struct SplitTableRegionReset(u32);

    // 测试会修改进程级开关，使用析构守卫确保离开作用域时恢复原值。
    impl Drop for SplitTableRegionReset {
        fn drop(&mut self) {
            astersql_ddl::EnableSplitTableRegion.store(self.0, Ordering::SeqCst);
        }
    }

    let previous = astersql_ddl::EnableSplitTableRegion.swap(1, Ordering::SeqCst);
    let _split_region_reset = SplitTableRegionReset(previous);

    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("create scatter runtime");
    crate::runtime::RegisterRuntimeTopology(
        &domain,
        vec![
            (1, "tikv-1:20160".to_owned()),
            (2, "tikv-2:20160".to_owned()),
            (3, "tikv-3:20160".to_owned()),
        ],
    );
    let observed = Arc::new(Mutex::new(Vec::new()));
    let callback_observed = Arc::clone(&observed);
    let _callback = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/preSplitAndScatter",
        move |scope| {
            // Failpoint 注册表为进程级共享，并发测试可能在守卫存活期间上报默认空范围；
            // 因此只记录本测试配置的 `table` 范围，避免把无关事件计入断言。
            if scope == "table" {
                callback_observed
                    .lock()
                    .expect("scatter callback lock poisoned")
                    .push(scope.to_owned());
            }
        },
    );

    session
        .execute("set @@session.tidb_scatter_region='table'")
        .expect("set session scatter");
    session
        .execute(
            "create table t(a int) shard_row_id_bits=10 pre_split_regions=3 \
             partition by range(a) (partition p0 values less than(10), \
             partition p1 values less than maxvalue)",
        )
        .expect("create pre-split partitioned table");
    assert_eq!(
        observed
            .lock()
            .expect("scatter callback lock poisoned")
            .as_slice(),
        ["table"]
    );

    let mut regions = session
        .execute("show table t regions")
        .expect("show derived regions")
        .remove(0);
    let mut rows = Vec::new();
    while let Some(row) = regions.Next().expect("read region row") {
        rows.push(row);
    }
    assert_eq!(rows.len(), 16);
    let mut leaders = rows
        .iter()
        .map(|row| row[4].clone())
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(leaders.len(), 3);
    leaders.clear();

    session
        .execute("set @@global.tidb_scatter_region='global'")
        .expect("set global scatter");
    let inherited = ConcreteSession::new(domain);
    let mut value = inherited
        .execute("select @@session.tidb_scatter_region")
        .expect("read inherited scatter")
        .remove(0);
    assert_eq!(
        value.Next().expect("read inherited scatter row"),
        Some(vec!["global".to_owned()])
    );
}

#[test]
// DDL 自版本写入遇到三次 etcd 注入故障后应逐次重试，并保持后续 DDL 可用。
fn ddl_self_version_update_retries_three_injected_etcd_failures() {
    use std::sync::atomic::AtomicUsize;

    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("create DDL retry runtime");
    let retries = Arc::new(AtomicUsize::new(0));
    let callback_retries = Arc::clone(&retries);
    let _callback = astersql_testkit_testfailpoint::enable_value_call(
        "github.com/pingcap/tidb/pkg/ddl/util/PutKVToEtcdError",
        move |event| {
            assert_eq!(event, "retry");
            callback_retries.fetch_add(1, Ordering::SeqCst);
        },
    );
    let _failure = astersql_testkit_testfailpoint::enable(
        "github.com/pingcap/tidb/pkg/ddl/util/PutKVToEtcdError",
        "3*return(true)",
    );

    session
        .execute("create table t(a int)")
        .expect("DDL must succeed after retrying etcd failures");
    assert_eq!(retries.load(Ordering::SeqCst), 3);
    session
        .execute("drop table t")
        .expect("subsequent DDL must still update self version");
}

#[test]
// 普通显式事务允许设置并清理由当前时间戳捕获的 TiDB 快照。
fn tidb_snapshot_can_be_set_in_a_normal_explicit_transaction() {
    let (_domain, session) =
        crate::runtime::CreateAnalyzeSession().expect("create snapshot runtime");
    session
        .execute("set @ts=@@tidb_current_ts")
        .expect("capture current timestamp");
    session.execute("begin").expect("begin normal transaction");
    session
        .execute("set @@tidb_snapshot=@ts")
        .expect("normal transaction must allow tidb_snapshot");
    session.execute("rollback").expect("rollback transaction");
    session
        .execute("set @@tidb_snapshot=''")
        .expect("clear snapshot");
}
#[test]
fn dxf_backend_alters_classic_table_mode_with_canonical_session() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database mode_runtime")
        .expect("create database");
    session
        .execute("create table mode_runtime.t (id bigint primary key)")
        .expect("create table");
    let schema_id = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("mode_runtime"))
        .expect("schema")
        .id;
    let table_id = domain.stats_table("mode_runtime", "t").expect("table").1.ID;
    let manager = session.ImportTaskManager().expect("DXF task manager");
    manager
        .WithNewTxn((), |se| {
            se.AlterTableModeForImport(schema_id, table_id)?;
            Ok(())
        })
        .expect("table mode import");
    assert_eq!(
        domain
            .stats_table("mode_runtime", "t")
            .expect("table after DDL")
            .1
            .Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
    let version = domain.info_schema().SchemaMetaVersion();
    manager
        .WithNewTxn((), |se| {
            se.AlterTableModeForImport(schema_id, table_id)?;
            Ok(())
        })
        .expect("same mode is a no-op");
    assert_eq!(domain.info_schema().SchemaMetaVersion(), version);
    manager
        .WithNewTxn((), |se| {
            se.AlterTableModeForNormal(schema_id, table_id)?;
            Ok(())
        })
        .expect("restore table mode after import");
    assert_eq!(
        domain
            .stats_table("mode_runtime", "t")
            .expect("table after restore")
            .1
            .Mode,
        astersql_meta_model::TableMode::TableModeNormal
    );
}

#[test]
fn dxf_backend_exposes_current_transaction_start_ts_for_import_stats() {
    let (_, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    let manager = session.ImportTaskManager().expect("DXF task manager");
    manager
        .WithNewTxn((), |se| {
            let timestamp = se.TxnStartTS()?;
            assert!(timestamp > 0);
            assert_eq!(se.TxnStartTS()?, timestamp);
            Ok(())
        })
        .expect("transaction start timestamp");
}

#[test]
fn import_scheduler_classic_table_empty_check_runs_on_canonical_session() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database empty_runtime")
        .expect("database");
    session
        .execute("create table empty_runtime.t (id bigint primary key)")
        .expect("table");
    let table = domain
        .info_schema()
        .ModelTableInfoByName(
            &astersql_infoschema::CiString::new("empty_runtime"),
            &astersql_infoschema::CiString::new("t"),
        )
        .expect("table metadata");
    let meta = astersql_dxf_importinto::TaskMeta {
        Plan: astersql_executor_importer::Plan {
            DBName: "empty_runtime".into(),
            TableInfo: Some(table),
            ..Default::default()
        },
        ..Default::default()
    };
    let manager = session.ImportTaskManager().expect("DXF task manager");
    let check = astersql_dxf_importinto::ProductionCheckImportTableEmpty(manager);
    check(&meta).expect("empty table");
    session
        .execute("insert into empty_runtime.t values (1)")
        .expect("insert row");
    assert!(
        check(&meta)
            .unwrap_err()
            .to_string()
            .contains("target table is not empty")
    );
}

#[test]
fn import_scheduler_stats_delta_runs_on_canonical_transaction() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database stats_import_runtime")
        .expect("database");
    session
        .execute("create table stats_import_runtime.t (id bigint primary key)")
        .expect("table");
    let table = domain
        .info_schema()
        .ModelTableInfoByName(
            &astersql_infoschema::CiString::new("stats_import_runtime"),
            &astersql_infoschema::CiString::new("t"),
        )
        .expect("table metadata");
    let meta = astersql_dxf_importinto::TaskMeta {
        Plan: astersql_executor_importer::Plan {
            TableInfo: Some(table),
            ..Default::default()
        },
        Summary: astersql_executor_importer::Summary {
            ImportedRows: 7,
            ..Default::default()
        },
        ..Default::default()
    };
    let manager = session.ImportTaskManager().expect("DXF task manager");
    manager
        .WithNewTxn((), |se| {
            astersql_dxf_importinto::FlushImportStatsProduction(&se, &meta)
                .map_err(|error| astersql_dxf_framework_storage::Error::new(error.to_string()))
        })
        .expect("stats delta");
}

#[test]
fn registered_import_scheduler_dispatches_durable_subtask_with_storage_adapter() {
    use astersql_dxf_framework_scheduler as scheduler;
    use astersql_dxf_framework_storage as storage;
    use astersql_dxf_importinto as importinto;
    use astersql_errors::{New, SharedError};
    use std::collections::HashMap;
    use std::sync::Arc;
    use std::time::Duration;

    struct Runtime;
    impl importinto::ImportSchedulerRuntime for Runtime {
        fn new_task_registration(
            &self,
            _: i64,
            _: Duration,
        ) -> Result<Box<dyn importinto::TaskRegistration>, SharedError> {
            Err(New("unused"))
        }
        fn switch_to_import_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
        fn switch_to_normal_mode(&self) -> Result<(), SharedError> {
            Ok(())
        }
    }
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database scheduler_runtime")
        .expect("database");
    session
        .execute("create table scheduler_runtime.t (id bigint primary key)")
        .expect("table");
    let table = domain
        .info_schema()
        .ModelTableInfoByName(
            &astersql_infoschema::CiString::new("scheduler_runtime"),
            &astersql_infoschema::CiString::new("t"),
        )
        .expect("table metadata");
    let manager = session.ImportTaskManager().expect("DXF task manager");
    manager
        .InitMeta((), "127.0.0.1:4000".into(), "background".into())
        .expect("managed node");
    let meta = importinto::TaskMeta {
        JobID: 41,
        Plan: astersql_executor_importer::Plan {
            DBName: "scheduler_runtime".into(),
            TableInfo: Some(table),
            ..Default::default()
        },
        EligibleInstances: vec![importinto::ServerInfo {
            ip: "127.0.0.1".into(),
            listening_port: 4000,
            ..Default::default()
        }],
        ChunkMap: HashMap::from([(
            1,
            vec![astersql_executor_importer::Chunk {
                Path: "data.csv".into(),
                FileSize: 10,
                EndOffset: 10,
                ..Default::default()
            }],
        )]),
        ..Default::default()
    };
    let task_id = manager
        .CreateTask(
            (),
            "scheduler-runtime-41".into(),
            storage::proto::ImportInto,
            String::new(),
            0,
            "background".into(),
            0,
            storage::proto::ExtraParams::default(),
            meta.Marshal().expect("task meta"),
        )
        .expect("create task");
    let adapter = scheduler::StorageTaskManagerAdapter::new(manager.clone());
    let task = scheduler::TaskManager::task_by_id(&adapter, task_id).expect("task snapshot");
    let encode = Arc::new(importinto::ConfiguredEncodeSortRuntime {
        ControllerServices: Arc::new(|| unreachable!()),
        ImporterService: Arc::new(|| unreachable!()),
        SharedImporterService: Arc::new(std::sync::OnceLock::new()),
        ObjectStore: Arc::new(astersql_objstore::azblob::MemoryStorage::default()),
        ObjectStoreFactory: None,
        LoggerFactory: Arc::new(|| unreachable!()),
        LocalEngines: None,
        Collector: None,
        WorkerFactory: None,
    });
    let services = Arc::new(importinto::ImportSchedulerServices::FromEncodeRuntime(
        encode,
        vec![],
        Arc::new(|_, _| importinto::planner::PlanCtx::default()),
    ));
    importinto::RegisterImportSchedulerFactoryWithServices(
        Arc::new(Runtime),
        manager.clone(),
        services,
    );
    let factory =
        scheduler::get_scheduler_factory(storage::proto::ImportInto).expect("registered factory");
    let param = scheduler::Param {
        task_manager: Arc::new(adapter),
        node_manager: Arc::new(scheduler::NodeManager::new()),
        slot_manager: Arc::new(scheduler::SlotManager::new()),
        server_id: "127.0.0.1:4000".into(),
        allocated_slots: true,
        node_resource: None,
    };
    let scheduler = factory(task, param);
    scheduler.init().expect("scheduler init");
    scheduler.schedule_once().expect("dispatch import step");
    assert_eq!(
        scheduler.task().base.step,
        astersql_dxf_framework_proto::ImportStepImport
    );
    let subtasks = manager
        .GetAllSubtasksByStepAndState(
            (),
            task_id,
            astersql_dxf_framework_proto::ImportStepImport,
            storage::proto::SubtaskStatePending,
        )
        .expect("subtasks")
        .unwrap_or_default();
    assert_eq!(subtasks.len(), 1);
    scheduler.close();
}

#[test]
fn importinto_submission_uses_canonical_sql_and_classic_ddl_backend() {
    let (domain, session) = crate::runtime::CreateAnalyzeSession().expect("canonical session");
    session
        .execute("create database submit_runtime")
        .expect("create database");
    session
        .execute("create table submit_runtime.t (id bigint primary key)")
        .expect("create table");
    let schema_id = domain
        .info_schema()
        .SchemaByName(&astersql_infoschema::CiString::new("submit_runtime"))
        .expect("schema")
        .id;
    let table = domain.table_by_name("submit_runtime", "t").expect("table");
    let manager = session.ImportTaskManager().expect("DXF task manager");
    manager
        .InitMeta((), "submit-host".into(), "background".into())
        .expect("managed node");
    let service = astersql_dxf_importinto::job::StorageTaskSubmissionService::WithManagers(
        false,
        std::sync::Arc::new(astersql_dxf_importinto::job::StorageSessionTableModeChanger),
        manager.clone(),
        manager,
        "scope".into(),
        true,
    );
    let mut plan = astersql_executor_importer::Plan {
        DBName: "submit_runtime".into(),
        DBID: schema_id,
        TableInfo: Some(table),
        ThreadCnt: 1,
        MaxNodeCnt: 1,
        User: "root@%".into(),
        ..Default::default()
    };
    let submitted = astersql_dxf_importinto::job::SubmitTask(&service, &mut plan, "import")
        .expect("submit IMPORT INTO job and DXF task");
    assert!(submitted.JobID > 0);
    assert!(submitted.TaskID > 0);
    assert_eq!(
        domain
            .stats_table("submit_runtime", "t")
            .expect("table mode")
            .1
            .Mode,
        astersql_meta_model::TableMode::TableModeImport
    );
}
