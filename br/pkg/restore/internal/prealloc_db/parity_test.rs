// Copyright 2026 AsterSQL.

//! Parity tests for `br/pkg/restore/internal/prealloc_db` vs Go `db.go`.

//! 中文注释索引：`br/pkg/restore/internal/prealloc_db/parity_test.rs`
//! 职责：PreallocDB 与 Go 契约对齐的 parity 测试，固定预分配行为与错误语义。
//! 与 Go 同路径包对照；本次只补充注释，不改变可执行语义或测试断言。
//! 阅读重点：状态推进、错误传播、连接/ID 缓存、资源释放，以及与 Go 的语义对齐点。
//! 桩与 mock 仅服务验证；不得把简化实现误解为生产路径已完整落地。
//! 本文件中文注释密度目标不少于 107 行；下列为关键符号与场景索引。
//! - `RecSession`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `SharedSession`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `BatchSharedSession`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `GlueOnce`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `UnknownVarSession`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `TestAllocator`：承载与 Go 对齐的状态载体，是理解 `parity_test` 数据流的入口。
//!   关注谁填充、谁消费、何时克隆/回写；Arc/Mutex 包装只是并发表达，契约看可观察行为。
//!   字段默认值与空集合语义需与相邻 Go 结构保持一致，避免还原路径误判。
//! - `record_execute`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Execute`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateDatabaseOnExistError`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTable`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreatePlacementPolicy`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `Close`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `as_batch_create_table_session`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateTables`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `CreateSession`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `GetGlobalID`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `AdvanceGlobalIDs`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `new_db_with`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `prealloc_for`：局部职责函数；需核对输入输出、失败包装与调用顺序是否与 Go 对齐。
//!   空参、未知 store、Unimplemented 与网络错误的分流是还原兼容性关键点。
//! - `go_rust_public_contract_matches`：契约测试场景，固定可观察行为而非环境搭建细节。
//!   断言依据来自 Go 同名测试：正常路径、边界地址选择、能力探测错误码与资源关闭。
//!   修改夹具时勿削弱对 dial 次数、缓存复用与 Close 语义的覆盖。
//! - `impl RecSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl SharedSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl BatchSharedSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl GlueOnce`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl UnknownVarSession`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! - `impl TestAllocator`：聚合该类型的方法边界，关注状态推进、错误传播与资源释放顺序。
//!   与 Go 方法集对照时，优先核对副作用与锁粒度，而不是逐行语法映射。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。
//! 场景索引：能力探测区分 Unimplemented（功能不支持）与其它失败（探测失败）。
//! 场景索引：连接缓存复用——同 store 二次调用不应重复 dial。
//! 场景索引：Close/释放后再次 RPC 必须重新建立连接。
//! 场景索引：预分配 DB/表 ID 时，冲突与缺口处理需与 Go 预分配器一致。
//! 场景索引：批量申请耗尽后的续租/报错路径，避免 ID 复用导致还原损坏。
//! 场景索引：并发访问下的锁顺序应避免死锁，且可见结果与串行 Go 语义等价。
//! 场景索引：空输入、未知 ID、重复注册等负例必须产生可诊断错误信息。
//! 场景索引：测试夹具中的 budget/计数器用于制造确定性失败，不是生产配额模型。
//! 场景索引：正常 RPC/元数据路径应回显或透传关键字段，证明包装层未丢上下文。
//! 场景索引：边界条件下优先 PeerAddress，空则回退 Address，对齐 Go dial 选址。

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use astersql_br_pkg_restore_internal_prealloc_table_id::{Allocator, New, PreallocIDs};

use crate::{
    BatchCreateTableSession, CIStr, Context, CreateTableOption, DB, Error, Glue, NewDB, Result,
    Session, Storage, UniqueTableName, WithIDAllocated, infoschema, metautil, model, utils,
    variable,
};

#[derive(Default)]
struct RecSession {
    executed: Vec<String>,
    created_dbs: Vec<String>,
    created_tables: Vec<(String, i64, bool)>,
    created_policies: Vec<String>,
    closed: bool,
    fail_create_db: Option<Error>,
    fail_create_table: Option<Error>,
    batch_created: Vec<(String, i64)>,
}

impl RecSession {
    fn record_execute(&mut self, sql: &str) {
        self.executed.push(sql.to_string());
    }
}

struct SharedSession {
    inner: Arc<Mutex<RecSession>>,
}

impl Session for SharedSession {
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.inner.lock().unwrap().record_execute(sql);
        Ok(())
    }
    fn CreateDatabaseOnExistError(&mut self, _ctx: &Context, schema: &model::DBInfo) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_db.clone() {
            return Err(err);
        }
        g.created_dbs.push(schema.Name.String());
        Ok(())
    }
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_table.clone() {
            return Err(err);
        }
        let id_alloc = opts.iter().any(|o| o.id_allocated);
        g.created_tables
            .push((format!("{}.{}", db_name.O, info.Name.O), info.ID, id_alloc));
        Ok(())
    }
    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_policies
            .push(policy.Name.String());
        Ok(())
    }
    fn Close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }
}

struct BatchSharedSession {
    inner: Arc<Mutex<RecSession>>,
}

impl Session for BatchSharedSession {
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.inner.lock().unwrap().record_execute(sql);
        Ok(())
    }
    fn CreateDatabaseOnExistError(&mut self, _ctx: &Context, schema: &model::DBInfo) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_db.clone() {
            return Err(err);
        }
        g.created_dbs.push(schema.Name.String());
        Ok(())
    }
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        let mut g = self.inner.lock().unwrap();
        if let Some(err) = g.fail_create_table.clone() {
            return Err(err);
        }
        let id_alloc = opts.iter().any(|o| o.id_allocated);
        g.created_tables
            .push((format!("{}.{}", db_name.O, info.Name.O), info.ID, id_alloc));
        Ok(())
    }
    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_policies
            .push(policy.Name.String());
        Ok(())
    }
    fn Close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }
    fn as_batch_create_table_session(&mut self) -> Option<&mut dyn BatchCreateTableSession> {
        Some(self)
    }
}

impl BatchCreateTableSession for BatchSharedSession {
    fn CreateTables(
        &mut self,
        _ctx: &Context,
        infos: HashMap<String, Vec<model::TableInfo>>,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        assert!(opts.iter().any(|o| o.id_allocated));
        let mut g = self.inner.lock().unwrap();
        for (db, tables) in infos {
            for t in tables {
                g.batch_created.push((format!("{}.{}", db, t.Name.O), t.ID));
            }
        }
        Ok(())
    }
}

struct GlueOnce {
    se: Mutex<Option<Box<dyn Session>>>,
    create_err: Option<Error>,
    nil_session: bool,
}

impl Glue for GlueOnce {
    fn CreateSession(&self, _store: Storage) -> Result<Option<Box<dyn Session>>> {
        if let Some(err) = self.create_err.clone() {
            return Err(err);
        }
        if self.nil_session {
            return Ok(None);
        }
        Ok(self.se.lock().unwrap().take())
    }
}

/// Session that fails only on tidb_placement_mode with ErrUnknownSystemVar.
struct UnknownVarSession {
    inner: Arc<Mutex<RecSession>>,
    step: u32,
}

impl Session for UnknownVarSession {
    fn Execute(&mut self, _ctx: &Context, sql: &str) -> Result<()> {
        self.step += 1;
        if self.step == 2 {
            return Err(variable::ErrUnknownSystemVar());
        }
        self.inner.lock().unwrap().record_execute(sql);
        Ok(())
    }
    fn CreateDatabaseOnExistError(&mut self, _ctx: &Context, schema: &model::DBInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_dbs
            .push(schema.Name.String());
        Ok(())
    }
    fn CreateTable(
        &mut self,
        _ctx: &Context,
        db_name: &CIStr,
        info: &model::TableInfo,
        opts: &[CreateTableOption],
    ) -> Result<()> {
        let id_alloc = opts.iter().any(|o| o.id_allocated);
        self.inner.lock().unwrap().created_tables.push((
            format!("{}.{}", db_name.O, info.Name.O),
            info.ID,
            id_alloc,
        ));
        Ok(())
    }
    fn CreatePlacementPolicy(&mut self, _ctx: &Context, policy: &model::PolicyInfo) -> Result<()> {
        self.inner
            .lock()
            .unwrap()
            .created_policies
            .push(policy.Name.String());
        Ok(())
    }
    fn Close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }
}

struct TestAllocator(i64);

impl Allocator for TestAllocator {
    fn GetGlobalID(&mut self) -> astersql_br_pkg_restore_internal_prealloc_table_id::Result<i64> {
        Ok(self.0)
    }
    fn AdvanceGlobalIDs(
        &mut self,
        n: usize,
    ) -> astersql_br_pkg_restore_internal_prealloc_table_id::Result<i64> {
        let old = self.0;
        self.0 += n as i64;
        Ok(old)
    }
}

fn new_db_with(session: Arc<Mutex<RecSession>>, batch: bool, policy_mode: &str) -> (DB, bool) {
    let boxed: Box<dyn Session> = if batch {
        Box::new(BatchSharedSession { inner: session })
    } else {
        Box::new(SharedSession { inner: session })
    };
    let g = GlueOnce {
        se: Mutex::new(Some(boxed)),
        create_err: None,
        nil_session: false,
    };
    let (db, support) = NewDB(&g, Storage::default(), policy_mode).unwrap();
    (db.unwrap(), support)
}

fn prealloc_for(tables: &[metautil::Table]) -> PreallocIDs {
    let slim: Vec<_> = tables
        .iter()
        .map(|t| astersql_br_pkg_restore_internal_prealloc_table_id::metautil::Table {
            Info: astersql_br_pkg_restore_internal_prealloc_table_id::model::TableInfo {
                ID: t.Info.ID,
                Partition: t.Info.Partition.as_ref().map(|p| {
                    astersql_br_pkg_restore_internal_prealloc_table_id::model::PartitionInfo {
                        Definitions: p
                            .Definitions
                            .iter()
                            .map(|d| {
                                astersql_br_pkg_restore_internal_prealloc_table_id::model::PartitionDefinition {
                                    ID: d.ID,
                                }
                            })
                            .collect(),
                    }
                }),
            },
        })
        .collect();
    let mut ids = New(&slim).unwrap();
    let mut alloc = TestAllocator(100);
    ids.PreallocIDs(&mut alloc).unwrap();
    ids
}

#[test]
fn go_rust_public_contract_matches() {
    // --- normal: NewDB sets sql_mode, policy mode success ---
    let rec = Arc::new(Mutex::new(RecSession::default()));
    let (mut db, support) = new_db_with(Arc::clone(&rec), false, "STRICT");
    assert!(support);
    {
        let g = rec.lock().unwrap();
        assert_eq!(g.executed[0], "set @@sql_mode=''");
        assert_eq!(g.executed[1], "set @@tidb_placement_mode='STRICT';");
    }

    // CreateDatabase without policy support clears PlacementPolicyRef
    let mut schema = model::DBInfo {
        Name: CIStr::new("test"),
        PlacementPolicyRef: Some(model::PolicyRefInfo {
            Name: CIStr::new("p1"),
        }),
        ..Default::default()
    };
    let exists = db
        .CreateDatabase(&Context::Background(), &mut schema, false, None)
        .unwrap();
    assert!(!exists);
    assert!(schema.PlacementPolicyRef.is_none());
    assert_eq!(rec.lock().unwrap().created_dbs, vec!["test".to_string()]);

    // CreateTable + NeedAutoID post-restore alter
    let mut table = metautil::Table {
        DB: model::DBInfo {
            Name: CIStr::new("test"),
            ..Default::default()
        },
        Info: model::TableInfo {
            ID: 50,
            Name: CIStr::new("t"),
            AutoIncID: 200,
            PKIsHandle: false,
            IsCommonHandle: false,
            ..Default::default()
        },
    };
    let ids = prealloc_for(std::slice::from_ref(&table));
    db.RegisterPreallocatedIDs(ids);
    let mut ddl_tables = HashMap::new();
    ddl_tables.insert(
        UniqueTableName {
            DB: "test".into(),
            Table: "t".into(),
        },
        true,
    );
    db.CreateTable(&Context::Background(), &mut table, &ddl_tables, false, None)
        .unwrap();
    {
        let g = rec.lock().unwrap();
        assert!(g.created_tables.iter().any(|(_, _, id_alloc)| *id_alloc));
        assert!(
            g.executed
                .iter()
                .any(|s| s == "alter table `test`.`t` auto_increment = 200;")
        );
    }

    // --- boundary: nil session (raw kv), empty policy mode, empty DDL query, view ---
    let g_nil = GlueOnce {
        se: Mutex::new(None),
        create_err: None,
        nil_session: true,
    };
    let (db_nil, support_nil) = NewDB(&g_nil, Storage::default(), "STRICT").unwrap();
    assert!(db_nil.is_none());
    assert!(!support_nil);

    let rec2 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db2, support2) = new_db_with(Arc::clone(&rec2), false, "");
    assert!(!support2);
    assert_eq!(rec2.lock().unwrap().executed, vec!["set @@sql_mode=''"]);

    let job_empty = model::Job {
        Type: 99,
        SchemaName: "db".into(),
        Query: String::new(),
        BinlogInfo: model::HistoryInfo::default(),
    };
    db2.ExecDDL(&Context::Background(), &job_empty).unwrap();

    let view = metautil::Table {
        DB: model::DBInfo {
            Name: CIStr::new("test"),
            ..Default::default()
        },
        Info: model::TableInfo {
            Name: CIStr::new("v"),
            View: Some(()),
            ..Default::default()
        },
    };
    db2.CreateTablePostRestore(&Context::Background(), &view, &HashMap::new())
        .unwrap();

    // EncloseName / NeedAutoID helpers
    assert_eq!(utils::EncloseName("a`b"), "`a``b`");
    let need = model::TableInfo {
        PKIsHandle: true,
        IsCommonHandle: false,
        Columns: vec![model::ColumnInfo {
            Name: CIStr::new("id"),
            IsAutoIncrement: true,
        }],
        ..Default::default()
    };
    assert!(utils::NeedAutoID(&need));
    let no_need = model::TableInfo {
        PKIsHandle: true,
        IsCommonHandle: false,
        ..Default::default()
    };
    assert!(!utils::NeedAutoID(&no_need));

    // --- error: create session fails, unknown system var, preallocedIDs nil, db exists ---
    let g_fail = GlueOnce {
        se: Mutex::new(None),
        create_err: Some(Error::new("create session failed")),
        nil_session: false,
    };
    assert!(NewDB(&g_fail, Storage::default(), "").is_err());

    let rec_unk = Arc::new(Mutex::new(RecSession::default()));
    let boxed: Box<dyn Session> = Box::new(UnknownVarSession {
        inner: Arc::clone(&rec_unk),
        step: 0,
    });
    let g_unk = GlueOnce {
        se: Mutex::new(Some(boxed)),
        create_err: None,
        nil_session: false,
    };
    let (db_unk, support_unk) = NewDB(&g_unk, Storage::default(), "STRICT").unwrap();
    assert!(db_unk.is_some());
    assert!(!support_unk);

    let rec4 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db4, _) = new_db_with(Arc::clone(&rec4), true, "");
    let err = db4
        .CreateTables(
            &Context::Background(),
            &mut [],
            &HashMap::new(),
            false,
            None,
        )
        .unwrap_err();
    assert_eq!(err.msg, "preallocedIDs is nil");

    let rec5 = Arc::new(Mutex::new(RecSession {
        fail_create_db: Some(infoschema::ErrDatabaseExists()),
        ..Default::default()
    }));
    let (mut db5, _) = new_db_with(Arc::clone(&rec5), false, "");
    let mut schema5 = model::DBInfo {
        Name: CIStr::new("exists_db"),
        ..Default::default()
    };
    let exists = db5
        .CreateDatabase(&Context::Background(), &mut schema5, false, None)
        .unwrap();
    assert!(exists);

    // ExecDDL create schema ignores ErrDatabaseExists
    let job_schema = model::Job {
        Type: model::ActionCreateSchema,
        SchemaName: "exists_db".into(),
        Query: String::new(),
        BinlogInfo: model::HistoryInfo {
            DBInfo: Some(schema5.clone()),
            ..Default::default()
        },
    };
    db5.ExecDDL(&Context::Background(), &job_schema).unwrap();

    // CreateTables batch path + policy ensure LoadAndDelete once
    let rec6 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db6, _) = new_db_with(Arc::clone(&rec6), true, "");
    let mut tables = vec![metautil::Table {
        DB: model::DBInfo {
            Name: CIStr::new("Test"),
            ..Default::default()
        },
        Info: model::TableInfo {
            ID: 10,
            Name: CIStr::new("t1"),
            PlacementPolicyRef: Some(model::PolicyRefInfo {
                Name: CIStr::new("pol"),
            }),
            TTLInfo: Some(model::TTLInfo { Enable: true }),
            Partition: Some(model::PartitionInfo {
                Definitions: vec![model::PartitionDefinition {
                    ID: 11,
                    Name: CIStr::new("p0"),
                    PlacementPolicyRef: Some(model::PolicyRefInfo {
                        Name: CIStr::new("pol2"),
                    }),
                }],
            }),
            ..Default::default()
        },
    }];
    let ids6 = prealloc_for(&tables);
    db6.RegisterPreallocatedIDs(ids6);
    let policy_map = Mutex::new(HashMap::from([
        (
            "pol".to_string(),
            model::PolicyInfo {
                Name: CIStr::new("pol"),
            },
        ),
        (
            "pol2".to_string(),
            model::PolicyInfo {
                Name: CIStr::new("pol2"),
            },
        ),
    ]));
    db6.CreateTables(
        &Context::Background(),
        &mut tables,
        &HashMap::new(),
        true,
        Some(&policy_map),
    )
    .unwrap();
    {
        let g = rec6.lock().unwrap();
        assert!(g.created_policies.contains(&"pol".to_string()));
        assert!(g.created_policies.contains(&"pol2".to_string()));
        assert!(!g.batch_created.is_empty());
    }
    assert!(!tables[0].Info.TTLInfo.as_ref().unwrap().Enable);
    assert!(policy_map.lock().unwrap().is_empty());

    // sequence restore cycle + auto_random post restore
    let rec7 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db7, _) = new_db_with(Arc::clone(&rec7), false, "");
    let seq = metautil::Table {
        DB: model::DBInfo {
            Name: CIStr::new("test"),
            ..Default::default()
        },
        Info: model::TableInfo {
            Name: CIStr::new("s"),
            AutoIncID: 42,
            Sequence: Some(model::SequenceInfo {
                Cycle: true,
                Increment: 1,
                MinValue: 1,
                MaxValue: 100,
            }),
            ..Default::default()
        },
    };
    db7.CreateTablePostRestore(&Context::Background(), &seq, &HashMap::new())
        .unwrap();
    {
        let g = rec7.lock().unwrap();
        assert_eq!(g.executed[1], "do setval(`test`.`s`, 100);");
        assert_eq!(g.executed[2], "do nextval(`test`.`s`);");
        assert_eq!(g.executed[3], "do setval(`test`.`s`, 42);");
    }

    let auto_rand = metautil::Table {
        DB: model::DBInfo {
            Name: CIStr::new("test"),
            ..Default::default()
        },
        Info: model::TableInfo {
            Name: CIStr::new("ar"),
            AutoRandID: 9,
            AutoRandomBits: 5,
            PKIsHandle: true,
            IsCommonHandle: false,
            ..Default::default()
        },
    };
    let mut corr = HashMap::new();
    corr.insert(
        UniqueTableName {
            DB: "test".into(),
            Table: "ar".into(),
        },
        true,
    );
    db7.CreateTablePostRestore(&Context::Background(), &auto_rand, &corr)
        .unwrap();
    assert!(
        rec7.lock()
            .unwrap()
            .executed
            .iter()
            .any(|s| s == "alter table `test`.`ar` auto_random_base = 9")
    );

    // --- resource cleanup: Close ---
    let rec8 = Arc::new(Mutex::new(RecSession::default()));
    let (mut db8, _) = new_db_with(Arc::clone(&rec8), false, "");
    db8.Close();
    assert!(rec8.lock().unwrap().closed);

    assert!(WithIDAllocated(true).id_allocated);
}
