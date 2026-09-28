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

// Aster 迁移单元测试：逐项核对各可选属性 Reader/Provider 与 Go 行为对齐。
//
// 覆盖缺失属性报错、注册后原样返回、Provider 错误透传、类型不匹配拒绝强转，
// 以及 InfoSchema 会话/Domain 双路径选择等契约。

use std::collections::HashMap;
use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use crate::*;

/// 仅实现可选属性上下文的测试用 EvalContext。
#[derive(Default)]
struct TestEvalContext {
    props: OptionalEvalPropProviders,
    location: Option<String>,
}

impl OptionalEvalPropContext for TestEvalContext {
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

/// 断言 Reader 在属性未注册时返回「not exists in EvalContext」错误。
fn assert_missing<T>(result: anyhow::Result<T>) {
    let error = result.err().expect("reader without provider must fail");
    assert!(
        error.to_string().contains("not exists in EvalContext"),
        "unexpected error: {error}"
    );
}

/// 占位 KV Store 类型，仅用于类型参数。
struct TestKVStore;

/// 受限 SQL 执行器桩，恒返回空结果。
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
        _args: &[Box<dyn std::any::Any>],
    ) -> anyhow::Result<(Vec<Self::Row>, Vec<Self::ResultField>)> {
        Ok((Vec::new(), Vec::new()))
    }
}

/// 空注册表时：键集合为空、越界键不可见、各 Reader 均报缺失。
#[test]
fn registry_and_missing_reader_paths_match_go() {
    let context = TestEvalContext::default();
    assert!(context.props.prop_key_set().IsEmpty());

    let invalid = exprctx::OptionalEvalPropKey(exprctx::OPT_PROPS_CNT);
    assert!(!context.props.contains(invalid));
    assert!(context.props.get(invalid).is_none());

    assert_missing(CurrentUserPropReader.current_user(&context));
    assert_missing(CurrentUserPropReader.active_roles(&context));
    assert_missing(DDLOwnerPropReader.is_ddl_owner(&context));
    assert_missing(KVStorePropReader.get_kv_store::<TestKVStore, _>(&context));
    assert_missing(SQLExecutorPropReader.get_sql_executor::<TestSQLExecutor, _>(&context));
    assert_missing(SequenceOperatorPropReader.get_sequence_operator(&context, "db1", "name1"));
    assert_missing(AdvisoryLockPropReader.advisory_lock_ctx(&context));
    assert_missing(SessionVarsPropReader.get_session_vars(&context));
    assert_missing(PrivilegeCheckerPropReader.get_privilege_checker(&context));
}

/// CurrentUser / DDLOwner 注册后，Reader 返回同一对象且键集合正确。
#[test]
fn current_user_and_ddl_owner_readers_preserve_provider_values() {
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

    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(CurrentUserPropProvider::new(move || {
            (Arc::clone(&user), roles.clone())
        })));
    context
        .props
        .add(Box::new(DDLOwnerInfoProvider::new(|| true)));

    assert_eq!(
        context.props.prop_key_set(),
        exprctx::OptPropCurrentUser
            .AsPropKeySet()
            .Add(exprctx::OptPropDDLOwnerInfo)
    );
    let read_user = CurrentUserPropReader.current_user(&context).unwrap();
    let read_roles = CurrentUserPropReader.active_roles(&context).unwrap();
    assert!(Arc::ptr_eq(&expected_user, &read_user));
    assert!(Arc::ptr_eq(&expected_roles[0], &read_roles[0]));
    assert!(DDLOwnerPropReader.is_ddl_owner(&context).unwrap());
    assert_eq!(
        CurrentUserPropReader.required_optional_eval_props(),
        exprctx::OptPropCurrentUser.AsPropKeySet()
    );
}

/// 测试用序列算子：id 递增，set 取 max 并报告是否低于基线。
struct TestSequenceOperator(i64);

impl SequenceOperator for TestSequenceOperator {
    fn get_sequence_id(&self) -> i64 {
        self.0
    }

    fn get_sequence_next_val(&mut self) -> anyhow::Result<i64> {
        self.0 += 1;
        Ok(self.0)
    }

    fn set_sequence_val(&mut self, new_val: i64) -> anyhow::Result<(i64, bool)> {
        let already_under_base = new_val < self.0;
        self.0 = self.0.max(new_val);
        Ok((self.0, already_under_base))
    }
}

/// 序列 Reader 转发 db/name，并透传 Provider 返回的错误。
#[test]
fn sequence_reader_forwards_arguments_and_provider_errors() {
    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(SequenceOperatorProvider::new(|db, name| {
            assert_eq!((db, name), ("db1", "name1"));
            Ok(Box::new(TestSequenceOperator(41)) as Box<dyn SequenceOperator>)
        })));

    let mut operator = SequenceOperatorPropReader
        .get_sequence_operator(&context, "db1", "name1")
        .unwrap();
    assert_eq!(operator.get_sequence_id(), 41);
    assert_eq!(operator.get_sequence_next_val().unwrap(), 42);
    assert_eq!(operator.set_sequence_val(40).unwrap(), (42, true));

    let mut failing = TestEvalContext::default();
    failing
        .props
        .add(Box::new(SequenceOperatorProvider::new(|_, _| {
            Err(anyhow::anyhow!("mockErr2"))
        })));
    let error = SequenceOperatorPropReader
        .get_sequence_operator(&failing, "db1", "name1")
        .err()
        .expect("provider error must be propagated");
    assert_eq!(error.to_string(), "mockErr2");
}

/// 仅实现 MetaOnlyInfoSchema 的桩，用 version 区分会话/Domain 快照。
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
        panic!("unused by expropt")
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

/// InfoSchema 按 is_domain 选择快照；KV Store Reader 保持同一 Arc。
#[test]
fn infoschema_and_kv_readers_choose_and_preserve_provider_objects() {
    let session_schema = Arc::new(TestInfoSchema { version: 11 });
    let domain_schema = Arc::new(TestInfoSchema { version: 22 });
    let session_expected = Arc::clone(&session_schema);
    let domain_expected = Arc::clone(&domain_schema);
    let store = Arc::new(TestKVStore);
    let expected_store = Arc::clone(&store);

    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(InfoSchemaPropProvider::new(move |is_domain| {
            if is_domain {
                Arc::clone(&domain_schema)
            } else {
                Arc::clone(&session_schema)
            }
        })));
    context
        .props
        .add(Box::new(KVStorePropProvider::new(move || {
            Arc::clone(&store)
        })));

    let got_session = InfoSchemaPropReader
        .get_session_info_schema::<TestInfoSchema, _>(&context)
        .unwrap();
    let got_domain = InfoSchemaPropReader
        .get_latest_info_schema::<TestInfoSchema, _>(&context)
        .unwrap();
    let got_store = KVStorePropReader
        .get_kv_store::<TestKVStore, _>(&context)
        .unwrap();

    assert!(Arc::ptr_eq(&session_expected, &got_session));
    assert!(Arc::ptr_eq(&domain_expected, &got_domain));
    assert!(Arc::ptr_eq(&expected_store, &got_store));
    assert_eq!(got_session.version, 11);
    assert_eq!(got_domain.version, 22);
}

/// SQLExecutor 成功路径返回同一实例，失败路径透传 mock 错误。
#[test]
fn sql_executor_reader_preserves_success_and_provider_errors() {
    let executor = Arc::new(TestSQLExecutor);
    let expected = Arc::clone(&executor);
    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(SQLExecutorPropProvider::new(move || {
            Ok(Arc::clone(&executor))
        })));

    let got = SQLExecutorPropReader
        .get_sql_executor::<TestSQLExecutor, _>(&context)
        .unwrap();
    assert!(Arc::ptr_eq(&expected, &got));
    assert_eq!(
        got.exec_restricted_sql(&(), &[], "select 1", &[]).unwrap(),
        (Vec::<()>::new(), Vec::<()>::new())
    );

    let mut failing = TestEvalContext::default();
    failing
        .props
        .add(Box::new(SQLExecutorPropProvider::<TestSQLExecutor>::new(
            || Err(anyhow::anyhow!("mockErr2")),
        )));
    let error = SQLExecutorPropReader
        .get_sql_executor::<TestSQLExecutor, _>(&failing)
        .err()
        .unwrap();
    assert_eq!(error.to_string(), "mockErr2");
}

/// 内存中的顾问锁（Advisory Lock，会话级命名锁）实现。
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

/// 顾问锁 Reader 返回已注册 Provider，并转发加锁/查询/释放调用。
#[test]
fn advisory_lock_reader_returns_the_registered_provider_and_forwards_calls() {
    let locks = Arc::new(TestAdvisoryLocks::default());
    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(AdvisoryLockPropProvider::new(Arc::clone(&locks))));

    let reader = AdvisoryLockPropReader;
    let provider = reader.advisory_lock_ctx(&context).unwrap();
    provider.get_advisory_lock("lock-1", 10).unwrap();
    assert_eq!(provider.is_used_advisory_lock("lock-1"), 1);
    assert!(provider.release_advisory_lock("lock-1"));
    assert!(!provider.release_advisory_lock("lock-1"));
    provider.get_advisory_lock("lock-2", 10).unwrap();
    provider.get_advisory_lock("lock-3", 10).unwrap();
    assert_eq!(provider.release_all_advisory_locks(), 2);
}

/// 权限检查桩：仅对特定库表列与动态权限名返回 true。
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

/// 权限 Reader 返回同一 checker，并转发静态/动态两类校验。
#[test]
fn privilege_reader_returns_same_checker_and_forwards_both_checks() {
    let checker: Arc<dyn PrivilegeChecker> = Arc::new(TestPrivilegeChecker);
    let expected = Arc::clone(&checker);
    let mut context = TestEvalContext::default();
    context
        .props
        .add(Box::new(PrivilegeCheckerProvider::new(move || {
            Arc::clone(&checker)
        })));

    let got = PrivilegeCheckerPropReader
        .get_privilege_checker(&context)
        .unwrap();
    assert!(Arc::ptr_eq(&expected, &got));
    assert!(got.request_verification("db1", "table1", "column1", mysql::PrivilegeType(1)));
    assert!(got.request_dynamic_verification("BACKUP_ADMIN", true));
    assert!(!got.request_dynamic_verification("BACKUP_ADMIN", false));
}

/// 全局变量访问桩（SessionVars 构造可能需要）。
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

/// SessionVars Reader 保持指针同一性，并满足 location 契约。
#[test]
fn session_vars_reader_preserves_identity_and_context_location_contract() {
    let vars = Arc::new(variable::SessionVars::new());
    let expected = Arc::clone(&vars);
    let mut context = TestEvalContext {
        props: OptionalEvalPropProviders::default(),
        location: Some("+00:00".to_owned()),
    };
    context
        .props
        .add(Box::new(SessionVarsPropProvider::new(vars)));

    let got = SessionVarsPropReader.get_session_vars(&context).unwrap();
    assert!(std::ptr::eq(Arc::as_ptr(&expected), got));
    assert_eq!(got.location().to_string(), "+00:00");
}

/// 断言模式下必须同时校验语句上下文时区，不能把会话时区重复当作 StmtCtx 时区。
#[test]
fn session_vars_reader_rejects_statement_location_mismatch() {
    let mut vars = variable::SessionVars::new();
    vars.StmtCtx.SetTimeZone(chrono_tz::Asia::Shanghai);
    let vars = Arc::new(vars);
    let expected = Arc::clone(&vars);
    let mut context = TestEvalContext {
        props: OptionalEvalPropProviders::default(),
        location: Some("+00:00".to_owned()),
    };
    context
        .props
        .add(Box::new(SessionVarsPropProvider::new(vars)));

    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        crate::sessionvars::assert_session_vars_location_matches(
            context.location.as_deref().unwrap(),
            expected.as_ref(),
        );
    }));

    assert!(
        result.is_err(),
        "statement location mismatch must trip the Go-compatible assertion"
    );
}

/// 声明 CurrentUser 键但非正确具体类型的 Provider，用于类型错配测试。
struct WrongCurrentUserProvider;

impl exprctx::OptionalEvalPropProvider for WrongCurrentUserProvider {
    fn Desc(&self) -> &'static exprctx::OptionalEvalPropDesc {
        exprctx::OptPropCurrentUser.Desc()
    }

    fn as_any(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
}

/// 键存在但具体类型不匹配时，应报错而非不安全强转。
#[test]
fn provider_type_mismatch_is_an_error_instead_of_an_unsafe_cast() {
    let mut context = TestEvalContext::default();
    context.props.add(Box::new(WrongCurrentUserProvider));
    let error = CurrentUserPropReader
        .current_user(&context)
        .err()
        .expect("wrong concrete provider type must fail");
    assert!(
        error
            .to_string()
            .contains("cannot cast OptionalEvalPropProvider")
    );
}
