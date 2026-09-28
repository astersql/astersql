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

// 资源组 hint 在事务阶段传递的测试。
//
// 资源组（Resource Group）按 RU（Request Unit）配额限制 SQL/事务消耗。
// 本文件对照 Go `TestResourceGroupHintInTxn`：用 failpoint 校验
// `SetTxnResourceGroup` 在 Prewrite / Commit / PessimisticLock 请求上携带的组名，
// 并覆盖 SELECT 语句级 hint 解析与 mock store 上的建组/写入路径。

const _GO_DRAFT_ARCHIVE: &str = r################"
// 资源组 hint 在事务内外写入、悲观锁、prewrite/commit 与大小写场景下的期望资源组检查。

// TestResourceGroupHintInTxn 对应 Go 同名测试：通过 failpoint 检查 RESOURCE_GROUP hint 在事务阶段的传递。
#[test]
pub fn TestResourceGroupHintInTxn(t: testing::T) {
	let store = testkit::CreateMockStore(t);
	let tk = testkit::NewTestKit(t, store);

	tk.MustExec("create resource group rg1 ru_per_sec=1000");
	tk.MustExec("create resource group rg2 ru_per_sec=1000");
	tk.MustExec("use test;");
	tk.MustExec("create table t (id int primary key, val int)");

	require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker", r#"return("default")"#));
	// defer/cleanup 对应 Go 的资源收尾路径，只记录关闭顺序。
	defer!(|| {
		require::NoError(t, failpoint::Disable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker"));
	});
	tk.MustExec("insert into t values (1, 1);");
	require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker", r#"return("rg1")"#));
	tk.MustExec("insert /*+ RESOURCE_GROUP(rg1) */ into t values (2, 2);");
	tk.MustExec("BEGIN;");
	// for pessimistic lock the resource group should be rg1
	tk.MustExec("insert /*+ RESOURCE_GROUP(rg1) */ into t values (3, 3);");
	require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker", r#"return("rg2")"#));
	// for final prewrite/commit the resource group should be rg2
	tk.MustExec("update /*+ RESOURCE_GROUP(rg2) */ t set val = val + 1 where id = 3;");
	tk.MustExec("COMMIT;");

	tk.MustExec("SET @@autocommit=1;");
	require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker", r#"return("default")"#));
	tk.MustExec("insert /*+ RESOURCE_GROUP(not_exist_group) */ into t values (4, 4);");

	tk.MustExec("BEGIN;");
	// for pessimistic lock the resource group should be rg1
	tk.MustExec("insert /*+ RESOURCE_GROUP(unknown_1) */ into t values (5, 5);");
	// for final prewrite/commit the resource group should be rg2
	tk.MustExec("update /*+ RESOURCE_GROUP(unknown_2) */ t set val = val + 1 where id = 5;");
	tk.MustExec("COMMIT;");

	require::NoError(t, failpoint::Enable("github.com/pingcap/tidb/pkg/kv/TxnResourceGroupChecker", r#"return("rg1")"#));
	tk.MustExec("insert /*+ RESOURCE_GROUP(RG1) */ into t values (6, 6);");
	require::NoError(t, tk.ExecToErr("select /*+ RESOURCE_GROUP(RG1) */ * from t;"));
}
"################;

use std::any::Any;
use std::collections::HashMap;

use astersql_kv::{
    BatchGetOption, Context, EmptyIterator, Error, FairLockingController, GetOption, Getter,
    Iterator, Key, LockCtx, MemBuffer, Mutator, RPCInterceptor, RequestKind, ResourceGroupName,
    Retriever, RetrieverMutator, RpcInterceptor, RpcRequest, SetTxnResourceGroup, Snapshot,
    Transaction, ValueEntry, kvrpcpb, model, tikv,
};
use astersql_parser::Parser;
use astersql_session::hint_runtime::{SessionBindingCatalog, StartStatementHintsWithBindings};
use astersql_testkit::mockstore::CreateMockStoreAndDomain;
use astersql_testkit::{Rows, TestKit};
use astersql_testkit_testfailpoint::enable;

/// 轻量事务桩：只记录 `SetOption`/`GetOption`，供资源组 checker 注入 RPCInterceptor。
struct OptionTxn {
    opts: HashMap<i32, Box<dyn Any>>,
    checkpoint: tikv::MemDBCheckpoint,
}

impl Default for OptionTxn {
    fn default() -> Self {
        Self {
            opts: HashMap::new(),
            checkpoint: tikv::MemDBCheckpoint,
        }
    }
}

impl Getter for OptionTxn {
    fn Get(&self, _ctx: &Context, _k: Key, _options: &[GetOption]) -> Result<ValueEntry, Error> {
        Ok(ValueEntry::default())
    }
}

impl Retriever for OptionTxn {
    fn Iter(&self, _k: Key, _upper_bound: Option<Key>) -> Result<Box<dyn Iterator>, Error> {
        Ok(Box::new(EmptyIterator))
    }
    fn IterReverse(
        &self,
        _k: Option<Key>,
        _lower_bound: Option<Key>,
    ) -> Result<Box<dyn Iterator>, Error> {
        Ok(Box::new(EmptyIterator))
    }
}

impl Mutator for OptionTxn {
    fn Set(&mut self, _k: Key, _v: Vec<u8>) -> Result<(), Error> {
        Ok(())
    }
    fn Delete(&mut self, _k: Key) -> Result<(), Error> {
        Ok(())
    }
}

impl RetrieverMutator for OptionTxn {}

impl FairLockingController for OptionTxn {
    fn StartFairLocking(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn RetryFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn CancelFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn DoneFairLocking(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn IsInFairLockingMode(&self) -> bool {
        false
    }
}

impl Transaction for OptionTxn {
    fn Size(&self) -> usize {
        0
    }
    fn Mem(&self) -> u64 {
        0
    }
    fn SetMemoryFootprintChangeHook(&mut self, _hook: Box<dyn Fn(u64)>) {}
    fn MemHookSet(&self) -> bool {
        false
    }
    fn Len(&self) -> usize {
        0
    }
    fn Commit(&mut self, _ctx: &Context) -> Result<(), Error> {
        Ok(())
    }
    fn Rollback(&mut self) -> Result<(), Error> {
        Ok(())
    }
    fn String(&self) -> String {
        "option-txn".into()
    }
    fn LockKeys(
        &mut self,
        _ctx: &Context,
        _lock_ctx: &mut LockCtx,
        _keys: &[Key],
    ) -> Result<(), Error> {
        Ok(())
    }
    fn LockKeysFunc(
        &mut self,
        _ctx: &Context,
        _lock_ctx: &mut LockCtx,
        f: &mut dyn FnMut(),
        _keys: &[Key],
    ) -> Result<(), Error> {
        f();
        Ok(())
    }
    fn SetOption(&mut self, opt: i32, val: Option<Box<dyn Any>>) {
        match val {
            Some(value) => {
                self.opts.insert(opt, value);
            }
            None => {
                self.opts.remove(&opt);
            }
        }
    }
    fn GetOption(&self, opt: i32) -> Option<&dyn Any> {
        self.opts.get(&opt).map(|value| value.as_ref())
    }
    fn IsReadOnly(&self) -> bool {
        true
    }
    fn StartTS(&self) -> u64 {
        1
    }
    fn CommitTS(&self) -> u64 {
        0
    }
    fn Valid(&self) -> bool {
        true
    }
    fn GetMemBuffer(&self) -> &dyn MemBuffer {
        panic!("OptionTxn.GetMemBuffer unused")
    }
    fn GetSnapshot(&self) -> &dyn Snapshot {
        panic!("OptionTxn.GetSnapshot unused")
    }
    fn SetVars(&mut self, _vars: Box<dyn Any>) {}
    fn GetVars(&self) -> &dyn Any {
        &()
    }
    fn BatchGet(
        &self,
        _ctx: &Context,
        _keys: &[Key],
        _options: &[BatchGetOption],
    ) -> Result<HashMap<String, ValueEntry>, Error> {
        Ok(HashMap::new())
    }
    fn IsPessimistic(&self) -> bool {
        false
    }
    fn CacheTableInfo(&mut self, _id: i64, _info: model::TableInfo) {}
    fn GetTableInfo(&self, _id: i64) -> Option<&model::TableInfo> {
        None
    }
    fn SetDiskFullOpt(&mut self, _level: kvrpcpb::DiskFullOpt) {}
    fn ClearDiskFullOpt(&mut self) {}
    fn GetMemDBCheckpoint(&self) -> &tikv::MemDBCheckpoint {
        &self.checkpoint
    }
    fn RollbackMemDBToCheckpoint(&mut self, _checkpoint: &tikv::MemDBCheckpoint) {}
    fn IsPipelined(&self) -> bool {
        false
    }
    fn MayFlush(&mut self) -> Result<(), Error> {
        Ok(())
    }
}

/// 解析单条 SQL 为 AST 节点，供 hint 提取使用。
fn parse(sql: &str) -> Box<dyn astersql_parser_ast::Node> {
    Parser::default()
        .ParseSQL(sql, &[])
        .expect("parse statement")
        .0
        .into_iter()
        .next()
        .expect("one statement")
}

/// 取出事务上的 RPCInterceptor，并按 failpoint 期望校验请求资源组名。
fn invoke_checker(txn: &OptionTxn, kind: RequestKind, group: &str) {
    let interceptor = txn
        .GetOption(RPCInterceptor)
        .and_then(|value| value.downcast_ref::<RpcInterceptor>())
        .cloned()
        .expect("TxnResourceGroupChecker should install RPCInterceptor");
    let mut request = RpcRequest {
        Kind: kind,
        ResourceGroupName: group.to_owned(),
    };
    interceptor(&mut request).expect("resource group name should match failpoint expectation");
}

/// 校验 failpoint TxnResourceGroupChecker 在事务各阶段强制期望的资源组名。
// 对应 TestResourceGroupHintInTxn：failpoint TxnResourceGroupChecker 校验
// SetTxnResourceGroup 在 Prewrite/Commit/PessimisticLock 请求上携带的资源组名。
#[test]
fn txn_resource_group_checker_enforces_hint_names_across_txn_stages() {
    let mut txn = OptionTxn::default();

    let _default = enable("TxnResourceGroupChecker", "return(default)");
    SetTxnResourceGroup(&mut txn, "default".into());
    let name = txn
        .GetOption(ResourceGroupName)
        .and_then(|value| value.downcast_ref::<String>())
        .cloned()
        .expect("resource group option");
    assert_eq!(name, "default");
    invoke_checker(&txn, RequestKind::Prewrite, "default");
    drop(_default);

    let _rg1 = enable("TxnResourceGroupChecker", "return(rg1)");
    SetTxnResourceGroup(&mut txn, "rg1".into());
    invoke_checker(&txn, RequestKind::PessimisticLock, "rg1");
    drop(_rg1);

    let _rg2 = enable("TxnResourceGroupChecker", "return(rg2)");
    SetTxnResourceGroup(&mut txn, "rg2".into());
    invoke_checker(&txn, RequestKind::Commit, "rg2");
    drop(_rg2);

    // Go：未知资源组回退到 default；checker 仍按当前 failpoint 期望值校验。
    let _fallback = enable("TxnResourceGroupChecker", "return(default)");
    SetTxnResourceGroup(&mut txn, "default".into());
    invoke_checker(&txn, RequestKind::Prewrite, "default");
    drop(_fallback);

    // Go：RESOURCE_GROUP(RG1) 大小写不敏感，最终以小写 rg1 进入 checker。
    let _case = enable("TxnResourceGroupChecker", "return(rg1)");
    SetTxnResourceGroup(&mut txn, "rg1".into());
    invoke_checker(&txn, RequestKind::Prewrite, "rg1");
}

/// 校验 SELECT 语句级 RESOURCE_GROUP hint 解析结果可见。
// 对应 Go 测试中 RESOURCE_GROUP hint 的解析侧：SELECT 语句级 hint 必须带上
// HasResourceGroup / ResourceGroup（INSERT 的 ExtractTableHints 当前只放行 memory_quota）。
#[test]
fn resource_group_hint_is_visible_on_select_statements() {
    let mut variables = astersql_sessionctx_variable::session::SessionVars::default();
    variables.SetCurrentDB("test");
    let mut bindings = SessionBindingCatalog::New("test");

    for (sql, expected) in [
        ("SELECT /*+ RESOURCE_GROUP(rg1) */ * FROM t", "rg1"),
        ("SELECT /*+ RESOURCE_GROUP(RG1) */ * FROM t", "RG1"),
        (
            "SELECT /*+ RESOURCE_GROUP(rg2) */ id, val FROM t WHERE id = 3",
            "rg2",
        ),
    ] {
        let statement = parse(sql);
        let guard =
            StartStatementHintsWithBindings(&variables, statement.as_ref(), sql, &mut bindings);
        assert!(
            guard.QueryHints().HasResourceGroup,
            "expected RESOURCE_GROUP hint on {sql}"
        );
        assert_eq!(
            guard.QueryHints().ResourceGroup,
            expected,
            "resource group name on {sql}"
        );
        guard.Finish().expect("finish hint statement");
    }
}

/// 在 mock store 上执行 Go 测试的完整资源组 hint 事务场景。
// 资源组 DDL、建表、自动提交/显式事务、未知组回退以及 SELECT 语句级 hint
// 都必须真实执行；不能以忽略错误或统计行数替代 Go 的执行与结果断言。
#[test]
fn resource_group_hint_in_txn_matches_go_test() {
    let (store, domain) = CreateMockStoreAndDomain();
    let mut tk = TestKit::new(store);
    tk.MustExec("create resource group rg1 ru_per_sec=1000", Vec::new());
    tk.MustExec("create resource group rg2 ru_per_sec=1000", Vec::new());
    tk.MustExec("use test", Vec::new());
    tk.MustExec("create table t (id int primary key, val int)", Vec::new());

    let default = enable("TxnResourceGroupChecker", "return(default)");
    tk.MustExec("insert into t values (1, 1)", Vec::new());
    drop(default);

    let rg1 = enable("TxnResourceGroupChecker", "return(rg1)");
    tk.MustExec(
        "insert /*+ RESOURCE_GROUP(rg1) */ into t values (2, 2)",
        Vec::new(),
    );
    tk.MustExec("BEGIN", Vec::new());
    tk.MustExec(
        "insert /*+ RESOURCE_GROUP(rg1) */ into t values (3, 3)",
        Vec::new(),
    );
    drop(rg1);

    let rg2 = enable("TxnResourceGroupChecker", "return(rg2)");
    tk.MustExec(
        "update /*+ RESOURCE_GROUP(rg2) */ t set val = val + 1 where id = 3",
        Vec::new(),
    );
    tk.MustExec("COMMIT", Vec::new());
    drop(rg2);

    tk.MustExec("SET @@autocommit=1", Vec::new());
    let default = enable("TxnResourceGroupChecker", "return(default)");
    tk.MustExec(
        "insert /*+ RESOURCE_GROUP(not_exist_group) */ into t values (4, 4)",
        Vec::new(),
    );
    drop(default);

    let default = enable("TxnResourceGroupChecker", "return(default)");
    tk.MustExec("BEGIN", Vec::new());
    tk.MustExec(
        "insert /*+ RESOURCE_GROUP(unknown_1) */ into t values (5, 5)",
        Vec::new(),
    );
    tk.MustExec(
        "update /*+ RESOURCE_GROUP(unknown_2) */ t set val = val + 1 where id = 5",
        Vec::new(),
    );
    tk.MustExec("COMMIT", Vec::new());
    drop(default);

    let rg1 = enable("TxnResourceGroupChecker", "return(rg1)");
    tk.MustExec(
        "insert /*+ RESOURCE_GROUP(RG1) */ into t values (6, 6)",
        Vec::new(),
    );
    tk.MustQuery("select * from t order by id", Vec::new())
        .Check(Rows(&["1 1", "2 2", "3 4", "4 4", "5 6", "6 6"]));
    // Go's `ExecToErr` is only an execution-error probe; this SELECT must execute
    // without an execution error while its result set is discarded by TestKit.
    tk.MustExec("select /*+ RESOURCE_GROUP(RG1) */ * from t", Vec::new());
    drop(rg1);

    // Keep the domain alive for the duration of the session-backed TestKit, as in Go's
    // CreateMockStore/NewTestKit pair; this also makes the test fail if the table vanished.
    assert!(domain.table_by_name("test", "t").is_ok());
}
