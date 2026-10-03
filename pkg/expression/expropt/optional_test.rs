// Copyright 2026 AsterSQL.
// Copyright 2024 PingCAP, Inc.
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

// 可选属性注册表与各 Provider/Reader 的 Go 用例对齐测试。
//
// 按属性键顺序逐项：`assert_before_add` → 缺失报错 → `add` → `assert_after_add`
// → 直连 Provider / Reader 校验；最终键集合应为满集（IsFull）。

#![allow(non_snake_case)]

use std::any::Any;
use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::*;

/// 测试用窄 EvalContext，只持有可选属性表与可选时区名。
#[derive(Default)]
struct MockEvalCtx {
    props: OptionalEvalPropProviders,
    location: Option<String>,
}

impl OptionalEvalPropContext for MockEvalCtx {
    fn get_optional_prop_provider(
        &self,
        key: exprctx::OptionalEvalPropKey,
    ) -> Option<&dyn exprctx::OptionalEvalPropProvider> {
        self.props.get(key)
    }

    fn location_name(&self) -> Option<String> {
        self.location.clone()
    }
}

/// 入口：空表起步，依次跑完全部 Go 对齐用例。
#[test]
fn TestOptionalEvalPropProviders() {
    let mut ctx = MockEvalCtx::default();
    assert!(ctx.props.prop_key_set().IsEmpty());
    verify_all_go_cases(&mut ctx);
}

/// 断言 Reader 在未注册时返回缺失错误。
fn assert_missing<T>(result: anyhow::Result<T>) {
    let error = result.err().expect("reader without provider must fail");
    assert!(
        error.to_string().contains("not exists in EvalContext"),
        "unexpected error: {error}"
    );
}

/// 注册前：键集合为「低于当前键的前缀满集」，且 Provider/Reader 键声明一致。
fn assert_before_add(
    ctx: &MockEvalCtx,
    key: exprctx::OptionalEvalPropKey,
    provider: &dyn exprctx::OptionalEvalPropProvider,
    reader: &dyn RequireOptionalEvalProps,
) {
    assert_eq!(
        ctx.props.prop_key_set(),
        exprctx::OptionalEvalPropKeySet((1_u64 << key.0) - 1)
    );
    assert!(!ctx.props.prop_key_set().Contains(key));
    assert!(!ctx.props.contains(key));
    assert_eq!(provider.Desc().Key(), key);
    assert_eq!(
        provider.Desc().Key().AsPropKeySet(),
        reader.required_optional_eval_props()
    );
}

/// 注册后：键集合扩展为含当前键的前缀满集。
fn assert_after_add(ctx: &MockEvalCtx, key: exprctx::OptionalEvalPropKey) {
    assert_eq!(
        ctx.props.prop_key_set(),
        exprctx::OptionalEvalPropKeySet((1_u64 << (key.0 + 1)) - 1)
    );
    assert!(ctx.props.prop_key_set().Contains(key));
    assert!(ctx.props.contains(key));
    assert!(ctx.props.get(key).is_some());
}

/// 按 Go 用例顺序验证全部可选属性，最后断言键集合已满。
fn verify_all_go_cases(ctx: &mut MockEvalCtx) {
    verify_current_user(ctx);
    verify_session_vars(ctx);
    verify_session_context(ctx);
    verify_info_schema(ctx);
    verify_kv_store(ctx);
    verify_sql_executor(ctx);
    verify_sequence_operator(ctx);
    verify_advisory_lock(ctx);
    verify_ddl_owner(ctx);
    verify_privilege_checker(ctx);
    assert!(ctx.props.prop_key_set().IsFull());
}

/// 验证 CurrentUser：用户与活跃角色指针同一性。
fn verify_current_user(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropCurrentUser;
    let user = Arc::new(auth::UserIdentity {
        username: "u1".into(),
        hostname: "h1".into(),
        ..Default::default()
    });
    let roles = vec![
        Arc::new(auth::RoleIdentity {
            username: "u2".into(),
            hostname: "h2".into(),
        }),
        Arc::new(auth::RoleIdentity {
            username: "u3".into(),
            hostname: "h3".into(),
        }),
    ];
    let expected_user = Arc::clone(&user);
    let expected_roles = roles.clone();
    let provider = CurrentUserPropProvider::new(move || (Arc::clone(&user), roles.clone()));
    let reader = CurrentUserPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.current_user(ctx));
    assert_missing(reader.active_roles(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider = get_prop_provider::<CurrentUserPropProvider, _>(ctx, key).unwrap();
    let (provided_user, provided_roles) = provider.call();
    assert!(Arc::ptr_eq(&expected_user, &provided_user));
    assert!(Arc::ptr_eq(&expected_roles[0], &provided_roles[0]));
    assert!(Arc::ptr_eq(
        &expected_user,
        &reader.current_user(ctx).unwrap()
    ));
    let read_roles = reader.active_roles(ctx).unwrap();
    assert!(Arc::ptr_eq(&expected_roles[0], &read_roles[0]));
    assert!(Arc::ptr_eq(&expected_roles[1], &read_roles[1]));
}

/// 全局变量访问桩。
struct TestGlobalAccessor;

impl variable::GlobalVarAccessor for TestGlobalAccessor {
    fn get_global_sys_var(&self, _name: &str) -> Result<String, variable::VariableError> {
        Ok(String::new())
    }

    fn set_global_sys_var_only(
        &mut self,
        _ctx: &variable::Context,
        _name: &str,
        _value: &str,
        _update_local: bool,
    ) -> Result<(), variable::VariableError> {
        Ok(())
    }

    fn get_tidb_table_value(&self, _name: &str) -> Result<String, variable::VariableError> {
        Ok(String::new())
    }

    fn set_tidb_table_value(
        &mut self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), variable::VariableError> {
        Ok(())
    }
}

/// 验证 SessionVars：Reader 返回与注册相同的指针。
fn verify_session_vars(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropSessionVars;
    let vars = Arc::new(variable::SessionVars::new());
    let expected = Arc::clone(&vars);
    let provider = SessionVarsPropProvider::new(vars);
    let reader = SessionVarsPropReader;

    ctx.location = Some("+00:00".to_owned());
    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_session_vars(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let got = reader.get_session_vars(ctx).unwrap();
    assert!(std::ptr::eq(Arc::as_ptr(&expected), got));
}

/// 用 version 区分会话/Domain 快照的 InfoSchema 桩。
struct TestInfoSchema {
    version: i64,
}

impl infoschema::SchemaAndTable for TestInfoSchema {
    type Context = ();
    type Error = Infallible;

    fn AllSchemas(&self) -> Vec<Arc<infoschema::model::DBInfo>> {
        Vec::new()
    }

    fn SchemaTableInfos(
        &self,
        _ctx: &Self::Context,
        _schema: &infoschema::ast::CIStr,
    ) -> Result<Vec<Arc<infoschema::model::TableInfo>>, Self::Error> {
        Ok(Vec::new())
    }
}

impl infoschema::Misc for TestInfoSchema {
    fn PolicyByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::PolicyInfo>> {
        None
    }

    fn ResourceGroupByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::ResourceGroupInfo>> {
        None
    }

    fn MaskingPolicyByName(
        &self,
        _name: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::MaskingPolicyInfo>> {
        None
    }

    fn MaskingPolicyByTableColumn(
        &self,
        _table_id: i64,
        _column_id: i64,
    ) -> Option<Arc<infoschema::model::MaskingPolicyInfo>> {
        None
    }

    fn PlacementBundleByPhysicalTableID(
        &self,
        _id: i64,
    ) -> Option<Arc<infoschema::placement::Bundle>> {
        None
    }

    fn AllPlacementBundles(&self) -> Vec<Arc<infoschema::placement::Bundle>> {
        Vec::new()
    }

    fn AllPlacementPolicies(&self) -> Vec<Arc<infoschema::model::PolicyInfo>> {
        Vec::new()
    }

    fn ClonePlacementPolicies(&self) -> HashMap<String, Arc<infoschema::model::PolicyInfo>> {
        HashMap::new()
    }

    fn AllMaskingPolicies(&self) -> Vec<Arc<infoschema::model::MaskingPolicyInfo>> {
        Vec::new()
    }

    fn AllResourceGroups(&self) -> Vec<Arc<infoschema::model::ResourceGroupInfo>> {
        Vec::new()
    }

    fn CloneResourceGroups(&self) -> HashMap<String, Arc<infoschema::model::ResourceGroupInfo>> {
        HashMap::new()
    }

    fn HasTemporaryTable(&self) -> bool {
        false
    }
}

impl infoschema::MetaOnlyInfoSchema for TestInfoSchema {
    fn SchemaMetaVersion(&self) -> i64 {
        self.version
    }

    fn SchemaByName(
        &self,
        _schema: &infoschema::ast::CIStr,
    ) -> Option<Arc<infoschema::model::DBInfo>> {
        None
    }

    fn SchemaExists(&self, _schema: &infoschema::ast::CIStr) -> bool {
        false
    }

    fn TableInfoByName(
        &self,
        _schema: &infoschema::ast::CIStr,
        _table: &infoschema::ast::CIStr,
    ) -> Result<Arc<infoschema::model::TableInfo>, Self::Error> {
        unreachable!("unused by expropt test")
    }

    fn TableInfoByID(&self, _id: i64) -> Option<Arc<infoschema::model::TableInfo>> {
        None
    }

    fn FindTableInfoByPartitionID(
        &self,
        _partition_id: i64,
    ) -> Option<(
        Arc<infoschema::model::TableInfo>,
        Arc<infoschema::model::DBInfo>,
        Arc<infoschema::model::PartitionDefinition>,
    )> {
        None
    }

    fn TableExists(
        &self,
        _schema: &infoschema::ast::CIStr,
        _table: &infoschema::ast::CIStr,
    ) -> bool {
        false
    }

    fn SchemaByID(&self, _id: i64) -> Option<Arc<infoschema::model::DBInfo>> {
        None
    }

    fn AllSchemaNames(&self) -> Vec<infoschema::ast::CIStr> {
        Vec::new()
    }

    fn SchemaSimpleTableInfos(
        &self,
        _ctx: &Self::Context,
        _schema: &infoschema::ast::CIStr,
    ) -> Result<Vec<Arc<infoschema::model::TableNameInfo>>, Self::Error> {
        Ok(Vec::new())
    }

    fn ListTablesWithSpecialAttribute(
        &self,
        _filter: infoschema::SpecialAttributeFilter,
    ) -> Vec<infoschema::TableInfoResult> {
        Vec::new()
    }

    fn GetTableReferredForeignKeys(
        &self,
        _schema: &str,
        _table: &str,
    ) -> Vec<Arc<infoschema::model::ReferredFKInfo>> {
        Vec::new()
    }
}

/// 验证 InfoSchema：`is_domain` 选择会话或 Domain 快照。
fn verify_info_schema(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropInfoSchema;
    let session_schema = Arc::new(TestInfoSchema { version: 11 });
    let domain_schema = Arc::new(TestInfoSchema { version: 22 });
    let expected_session = Arc::clone(&session_schema);
    let expected_domain = Arc::clone(&domain_schema);
    let provider = InfoSchemaPropProvider::new(move |is_domain| {
        if is_domain {
            Arc::clone(&domain_schema)
        } else {
            Arc::clone(&session_schema)
        }
    });
    let reader = InfoSchemaPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_session_info_schema::<TestInfoSchema, _>(ctx));
    assert_missing(reader.get_latest_info_schema::<TestInfoSchema, _>(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider =
        get_prop_provider::<InfoSchemaPropProvider<TestInfoSchema>, _>(ctx, key).unwrap();
    assert!(Arc::ptr_eq(&expected_domain, &provider.call(true)));
    assert!(Arc::ptr_eq(&expected_session, &provider.call(false)));
    assert!(Arc::ptr_eq(
        &expected_domain,
        &reader
            .get_latest_info_schema::<TestInfoSchema, _>(ctx)
            .unwrap()
    ));
    assert!(Arc::ptr_eq(
        &expected_session,
        &reader
            .get_session_info_schema::<TestInfoSchema, _>(ctx)
            .unwrap()
    ));
}

/// 占位 KV Store 类型。
struct TestKVStore;

/// 验证 KV Store：Provider 与 Reader 返回同一 Arc。
fn verify_kv_store(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropKVStore;
    let store = Arc::new(TestKVStore);
    let expected = Arc::clone(&store);
    let provider = KVStorePropProvider::new(move || Arc::clone(&store));
    let reader = KVStorePropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_kv_store::<TestKVStore, _>(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider = get_prop_provider::<KVStorePropProvider<TestKVStore>, _>(ctx, key).unwrap();
    assert!(Arc::ptr_eq(&expected, &provider.call()));
    assert!(Arc::ptr_eq(
        &expected,
        &reader.get_kv_store::<TestKVStore, _>(ctx).unwrap()
    ));
}

/// 受限 SQL 执行器桩。
#[derive(Debug)]
struct TestSQLExecutor;

impl SQLExecutor for TestSQLExecutor {
    type Context = ();
    type OptionFuncAlias = ();
    type Row = ();
    type ResultField = ();

    fn exec_restricted_sql(
        &self,
        _ctx: &Self::Context,
        _opts: &[Self::OptionFuncAlias],
        _sql: &str,
        _args: &[Box<dyn Any>],
    ) -> anyhow::Result<(Vec<Self::Row>, Vec<Self::ResultField>)> {
        Ok((Vec::new(), Vec::new()))
    }
}

/// 验证 SQLExecutor：成功返回与错误注入均可经 Provider/Reader 透传。
fn verify_sql_executor(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropSQLExecutor;
    let executor = Arc::new(TestSQLExecutor);
    let expected = Arc::clone(&executor);
    let mock_error = Arc::new(Mutex::new(None::<&'static str>));
    let provider_error = Arc::clone(&mock_error);
    let provider = SQLExecutorPropProvider::new(move || {
        if let Some(message) = *provider_error.lock().unwrap() {
            Err(anyhow::anyhow!(message))
        } else {
            Ok(Arc::clone(&executor))
        }
    });
    let reader = SQLExecutorPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_sql_executor::<TestSQLExecutor, _>(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider =
        get_prop_provider::<SQLExecutorPropProvider<TestSQLExecutor>, _>(ctx, key).unwrap();
    assert!(Arc::ptr_eq(&expected, &provider.call().unwrap()));
    *mock_error.lock().unwrap() = Some("mockErr1");
    assert_eq!(provider.call().unwrap_err().to_string(), "mockErr1");
    *mock_error.lock().unwrap() = None;
    assert!(Arc::ptr_eq(
        &expected,
        &reader.get_sql_executor::<TestSQLExecutor, _>(ctx).unwrap()
    ));
    *mock_error.lock().unwrap() = Some("mockErr2");
    assert_eq!(
        reader
            .get_sql_executor::<TestSQLExecutor, _>(ctx)
            .unwrap_err()
            .to_string(),
        "mockErr2"
    );
}

/// 共享可变计数器的序列算子桩。
struct TestSequenceOperator {
    value: Arc<Mutex<i64>>,
}

impl SequenceOperator for TestSequenceOperator {
    fn get_sequence_id(&self) -> i64 {
        *self.value.lock().unwrap()
    }

    fn get_sequence_next_val(&mut self) -> anyhow::Result<i64> {
        let mut value = self.value.lock().unwrap();
        *value += 1;
        Ok(*value)
    }

    fn set_sequence_val(&mut self, new_val: i64) -> anyhow::Result<(i64, bool)> {
        let mut value = self.value.lock().unwrap();
        let already_under_base = new_val < *value;
        *value = (*value).max(new_val);
        Ok((*value, already_under_base))
    }
}

/// 验证 Sequence：转发 db/name，并透传 Provider 错误。
fn verify_sequence_operator(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropSequenceOperator;
    let shared_value = Arc::new(Mutex::new(41));
    let mock_error = Arc::new(Mutex::new(None::<&'static str>));
    let provider_value = Arc::clone(&shared_value);
    let provider_error = Arc::clone(&mock_error);
    let provider = SequenceOperatorProvider::new(move |db, name| {
        assert_eq!((db, name), ("db1", "name1"));
        if let Some(message) = *provider_error.lock().unwrap() {
            Err(anyhow::anyhow!(message))
        } else {
            Ok(Box::new(TestSequenceOperator {
                value: Arc::clone(&provider_value),
            }) as Box<dyn SequenceOperator>)
        }
    });
    let reader = SequenceOperatorPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_sequence_operator(ctx, "db1", "name1"));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider = get_prop_provider::<SequenceOperatorProvider, _>(ctx, key).unwrap();
    assert_eq!(provider.call("db1", "name1").unwrap().get_sequence_id(), 41);
    *mock_error.lock().unwrap() = Some("mockErr1");
    assert_eq!(
        provider.call("db1", "name1").err().unwrap().to_string(),
        "mockErr1"
    );
    *mock_error.lock().unwrap() = None;
    let mut operator = reader.get_sequence_operator(ctx, "db1", "name1").unwrap();
    assert_eq!(operator.get_sequence_next_val().unwrap(), 42);
    *mock_error.lock().unwrap() = Some("mockErr2");
    assert_eq!(
        reader
            .get_sequence_operator(ctx, "db1", "name1")
            .err()
            .unwrap()
            .to_string(),
        "mockErr2"
    );
}

/// 内存顾问锁实现。
#[derive(Default)]
struct TestAdvisoryLocks {
    locks: Mutex<Vec<String>>,
}

impl AdvisoryLockContext for TestAdvisoryLocks {
    fn get_advisory_lock(&self, name: &str, _timeout: i64) -> anyhow::Result<()> {
        self.locks.lock().unwrap().push(name.to_owned());
        Ok(())
    }

    fn is_used_advisory_lock(&self, name: &str) -> u64 {
        u64::from(self.locks.lock().unwrap().iter().any(|lock| lock == name))
    }

    fn release_advisory_lock(&self, name: &str) -> bool {
        let mut locks = self.locks.lock().unwrap();
        let Some(index) = locks.iter().position(|lock| lock == name) else {
            return false;
        };
        locks.remove(index);
        true
    }

    fn release_all_advisory_locks(&self) -> i32 {
        let mut locks = self.locks.lock().unwrap();
        let count = locks.len() as i32;
        locks.clear();
        count
    }
}

/// 验证顾问锁：直连 Provider 与 Reader 返回同一对象，并转发加解锁。
fn verify_advisory_lock(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropAdvisoryLock;
    let locks = Arc::new(TestAdvisoryLocks::default());
    let provider = AdvisoryLockPropProvider::new(Arc::clone(&locks));
    let reader = AdvisoryLockPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.advisory_lock_ctx(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let direct = get_prop_provider::<AdvisoryLockPropProvider, _>(ctx, key).unwrap();
    let read = reader.advisory_lock_ctx(ctx).unwrap();
    assert!(std::ptr::eq(direct, read));
    read.get_advisory_lock("lock1", 10).unwrap();
    assert_eq!(locks.is_used_advisory_lock("lock1"), 1);
    assert!(read.release_advisory_lock("lock1"));
}

/// 验证 DDL Owner：原子开关驱动 Provider/Reader 布尔结果。
fn verify_ddl_owner(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropDDLOwnerInfo;
    let is_owner = Arc::new(AtomicBool::new(false));
    let provider_state = Arc::clone(&is_owner);
    let provider = DDLOwnerInfoProvider::new(move || provider_state.load(Ordering::SeqCst));
    let reader = DDLOwnerPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.is_ddl_owner(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider = get_prop_provider::<DDLOwnerInfoProvider, _>(ctx, key).unwrap();
    is_owner.store(true, Ordering::SeqCst);
    assert!(provider.call());
    is_owner.store(false, Ordering::SeqCst);
    assert!(!provider.call());
    is_owner.store(true, Ordering::SeqCst);
    assert!(reader.is_ddl_owner(ctx).unwrap());
    is_owner.store(false, Ordering::SeqCst);
    assert!(!reader.is_ddl_owner(ctx).unwrap());
}

/// 权限检查桩。
struct TestPrivilegeChecker;

impl PrivilegeChecker for TestPrivilegeChecker {
    fn request_verification(
        &self,
        db: &str,
        table: &str,
        column: &str,
        _privilege: mysql::PrivilegeType,
    ) -> bool {
        (db, table, column) == ("db1", "table1", "column1")
    }

    fn request_dynamic_verification(&self, privilege_name: &str, grantable: bool) -> bool {
        privilege_name == "BACKUP_ADMIN" && grantable
    }
}

/// 验证权限 Checker：返回同一实例并转发两类校验。
fn verify_privilege_checker(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropPrivilegeChecker;
    let checker: Arc<dyn PrivilegeChecker> = Arc::new(TestPrivilegeChecker);
    let expected = Arc::clone(&checker);
    let provider = PrivilegeCheckerProvider::new(move || Arc::clone(&checker));
    let reader = PrivilegeCheckerPropReader;

    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_privilege_checker(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);

    let provider = get_prop_provider::<PrivilegeCheckerProvider, _>(ctx, key).unwrap();
    assert!(Arc::ptr_eq(&expected, &provider.call()));
    let got = reader.get_privilege_checker(ctx).unwrap();
    assert!(Arc::ptr_eq(&expected, &got));
    assert!(got.request_verification("db1", "table1", "column1", mysql::PrivilegeType(1)));
    assert!(got.request_dynamic_verification("BACKUP_ADMIN", true));
}

struct EmbeddingSession;
impl SessionContext for EmbeddingSession {
    fn embedding_runtime(&self) -> Option<Arc<inference::EmbedFn>> {
        None
    }
    fn embedding_cancellation(&self) -> Option<String> {
        None
    }
}
fn verify_session_context(ctx: &mut MockEvalCtx) {
    let key = exprctx::OptPropSessionContext;
    let provider = SessionContextPropProvider::new(Arc::new(EmbeddingSession));
    let reader = SessionContextPropReader;
    assert_before_add(ctx, key, &provider, &reader);
    assert_missing(reader.get_session_context(ctx));
    ctx.props.add(Box::new(provider));
    assert_after_add(ctx, key);
    assert!(
        reader
            .get_session_context(ctx)
            .unwrap()
            .embedding_runtime()
            .is_none()
    );
}
