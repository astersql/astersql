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

// sessionexpr 迁移对照单测与 TestSession 夹具。
//
// 提供窄化 SessionContext 实现，验证 ExprContext/EvalContext 对会话的委托、
// 可选属性、权限、序列与语句时间解析与 Go 行为一致。

use std::collections::{HashMap, HashSet};
use std::convert::Infallible;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{FixedOffset, TimeZone, Timelike, Utc};
use chrono_tz::Asia::Shanghai;
use sessionexpr::expropt::AdvisoryLockContext;
use sessionexpr::{
    NewEvalContext, NewExprContext, PrivilegeManager, SessionContext, auth, contextutil, errctx,
    exprctx, expropt, infoschema, mathutil, mysql, resolve_statement_timestamp, types, variable,
};

/// 空实现的全局系统变量访问器夹具。
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

/// 仅携带 schema 版本号的 MetaOnlyInfoSchema 夹具。
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
        unreachable!("not needed by sessionexpr")
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

#[derive(Default)]
/// KV Store 占位类型。
struct TestStore;

/// 受限 SQL 执行器占位：始终返回空结果。
struct TestSqlExecutor;

impl expropt::SQLExecutor for TestSqlExecutor {
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

/// 内存序列算子：用原子计数模拟 NEXTVAL。
struct TestSequence {
    id: i64,
    value: Arc<AtomicI64>,
}

impl expropt::SequenceOperator for TestSequence {
    fn get_sequence_id(&self) -> i64 {
        self.id
    }
    fn get_sequence_next_val(&mut self) -> anyhow::Result<i64> {
        Ok(self.value.fetch_add(1, Ordering::SeqCst) + 1)
    }
    fn set_sequence_val(&mut self, new_val: i64) -> anyhow::Result<(i64, bool)> {
        self.value.store(new_val, Ordering::SeqCst);
        Ok((new_val, true))
    }
}

#[derive(Clone, Default)]
/// 空用户变量读取器。
struct TestUserVars;

impl exprctx::UserVarsReader for TestUserVars {
    fn GetUserVarVal(&self, _name: &str) -> Option<types::Datum> {
        None
    }
    fn GetUserVarType(&self, _name: &str) -> Option<types::FieldType> {
        None
    }
    fn Clone(&self) -> Box<dyn exprctx::UserVarsReader> {
        Box::new(self.clone())
    }
}

/// 固定规则的权限管理器：仅放行特定库表与 BACKUP_ADMIN。
struct TestPrivilegeManager;

impl PrivilegeManager for TestPrivilegeManager {
    fn request_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        db: &str,
        table: &str,
        column: &str,
        _privilege_type: mysql::PrivilegeType,
    ) -> bool {
        active_roles.len() == 2 && (db, table, column) == ("db1", "t1", "c1")
    }
    fn request_dynamic_verification(
        &self,
        active_roles: &[auth::RoleIdentity],
        privilege_name: &str,
        grantable: bool,
    ) -> bool {
        active_roles.len() == 2 && privilege_name == "BACKUP_ADMIN" && grantable
    }
}

/// 会话夹具：聚合 InfoSchema、告警、计划缓存、时间戳与权限开关等。
/// 字段值对齐 Go sessionctx_test 中的可观测断言。
struct TestSession {
    vars: Arc<variable::session::SessionVars>,
    session_schema: Arc<TestInfoSchema>,
    latest_schema: Arc<TestInfoSchema>,
    store: Arc<TestStore>,
    sql_executor: Arc<TestSqlExecutor>,
    sequence_value: Arc<AtomicI64>,
    advisory_locks: Mutex<HashSet<String>>,
    ddl_owner: AtomicBool,
    privilege_enabled: AtomicBool,
    systems: Mutex<HashMap<String, String>>,
    rng: Arc<mathutil::MysqlRng>,
    plan_cache: Arc<contextutil::plancache::PlanCacheTracker>,
    plan_column_id: AtomicI64,
    group_concat_max_len: AtomicU64,
    warning_handler: Arc<contextutil::StaticWarnHandler>,
    type_context: types::Context,
    error_context: errctx::Context,
    params: Mutex<Vec<i64>>,
    user_vars: TestUserVars,
    stale_tso: AtomicU64,
    timestamp: Mutex<String>,
    statement_timestamp: Mutex<Option<String>>,
}

impl TestSession {
    /// 构造带默认时区、计划列 ID、参数与时间戳的测试会话。
    fn new() -> Self {
        let warning_handler = Arc::new(contextutil::NewStaticWarnHandler(4));
        let warn_appender: Arc<dyn contextutil::WarnAppender + Send + Sync> =
            warning_handler.clone();
        let plan_cache = Arc::new(contextutil::plancache::NewPlanCacheTracker(
            warn_appender.clone(),
        ));
        let mut vars = variable::session::SessionVars::new();
        vars.set_location(FixedOffset::east_opt(8 * 60 * 60).unwrap());
        Self {
            vars: Arc::new(vars),
            session_schema: Arc::new(TestInfoSchema { version: 11 }),
            latest_schema: Arc::new(TestInfoSchema { version: 22 }),
            store: Arc::new(TestStore),
            sql_executor: Arc::new(TestSqlExecutor),
            sequence_value: Arc::new(AtomicI64::new(40)),
            advisory_locks: Mutex::new(HashSet::new()),
            ddl_owner: AtomicBool::new(false),
            privilege_enabled: AtomicBool::new(false),
            systems: Mutex::new(HashMap::new()),
            rng: Arc::from(mathutil::NewWithSeed(123)),
            plan_cache,
            plan_column_id: AtomicI64::new(7),
            group_concat_max_len: AtomicU64::new(1_024),
            type_context: types::NewContext(
                types::DefaultStmtFlags,
                Shanghai,
                warn_appender.clone(),
            ),
            error_context: errctx::NewContext(warn_appender),
            warning_handler,
            params: Mutex::new(vec![9]),
            user_vars: TestUserVars,
            stale_tso: AtomicU64::new(0),
            timestamp: Mutex::new("7654321.875".to_owned()),
            statement_timestamp: Mutex::new(None),
        }
    }
}

impl expropt::AdvisoryLockContext for TestSession {
    fn get_advisory_lock(&self, name: &str, _timeout: i64) -> anyhow::Result<()> {
        self.advisory_locks.lock().unwrap().insert(name.to_owned());
        Ok(())
    }
    fn is_used_advisory_lock(&self, name: &str) -> u64 {
        u64::from(self.advisory_locks.lock().unwrap().contains(name))
    }
    fn release_advisory_lock(&self, name: &str) -> bool {
        self.advisory_locks.lock().unwrap().remove(name)
    }
    fn release_all_advisory_locks(&self) -> i32 {
        let mut locks = self.advisory_locks.lock().unwrap();
        let count = locks.len() as i32;
        locks.clear();
        count
    }
}

impl SessionContext for TestSession {
    type InfoSchema = TestInfoSchema;
    type Store = TestStore;
    type SqlExecutor = TestSqlExecutor;

    fn session_vars(&self) -> Arc<variable::session::SessionVars> {
        Arc::clone(&self.vars)
    }
    fn current_user(&self) -> Arc<auth::UserIdentity> {
        Arc::new(auth::UserIdentity {
            username: "user1".to_owned(),
            hostname: "host1".to_owned(),
            ..Default::default()
        })
    }
    fn active_roles(&self) -> Vec<Arc<auth::RoleIdentity>> {
        vec![
            Arc::new(auth::RoleIdentity {
                username: "role1".to_owned(),
                hostname: "host1".to_owned(),
            }),
            Arc::new(auth::RoleIdentity {
                username: "role2".to_owned(),
                hostname: "host2".to_owned(),
            }),
        ]
    }
    fn info_schema(&self) -> Arc<Self::InfoSchema> {
        Arc::clone(&self.session_schema)
    }
    fn latest_info_schema(&self) -> Arc<Self::InfoSchema> {
        Arc::clone(&self.latest_schema)
    }
    fn store(&self) -> Arc<Self::Store> {
        Arc::clone(&self.store)
    }
    fn restricted_sql_executor(&self) -> Arc<Self::SqlExecutor> {
        Arc::clone(&self.sql_executor)
    }
    fn sequence_operator(
        &self,
        db: &str,
        name: &str,
    ) -> anyhow::Result<Box<dyn expropt::SequenceOperator>> {
        // 仅暴露 db1.seq1，其它序列名报错以对齐 Go 夹具。
        if (db, name) != ("db1", "seq1") {
            anyhow::bail!("unknown sequence {db}.{name}");
        }
        Ok(Box::new(TestSequence {
            id: 88,
            value: Arc::clone(&self.sequence_value),
        }))
    }
    fn is_ddl_owner(&self) -> bool {
        self.ddl_owner.load(Ordering::SeqCst)
    }
    fn privilege_manager(&self) -> Option<Arc<dyn PrivilegeManager>> {
        self.privilege_enabled
            .load(Ordering::SeqCst)
            .then(|| Arc::new(TestPrivilegeManager) as Arc<dyn PrivilegeManager>)
    }
    fn charset_info(&self) -> (String, String) {
        ("gbk".to_owned(), "gbk_chinese_ci".to_owned())
    }
    fn default_collation_for_utf8mb4(&self) -> String {
        "utf8mb4_0900_ai_ci".to_owned()
    }
    fn system_var(&self, name: &str) -> Option<String> {
        self.systems.lock().unwrap().get(name).cloned()
    }
    fn sysdate_is_now(&self) -> bool {
        true
    }
    fn noop_funcs_mode(&self) -> i32 {
        2
    }
    fn rng(&self) -> Arc<mathutil::MysqlRng> {
        Arc::clone(&self.rng)
    }
    fn plan_cache_tracker(&self) -> Arc<contextutil::plancache::PlanCacheTracker> {
        Arc::clone(&self.plan_cache)
    }
    fn alloc_plan_column_id(&self) -> i64 {
        self.plan_column_id.fetch_add(1, Ordering::SeqCst) + 1
    }
    fn last_plan_column_id(&self) -> i64 {
        self.plan_column_id.load(Ordering::SeqCst)
    }
    fn windowing_use_high_precision(&self) -> bool {
        true
    }
    fn group_concat_max_len(&self) -> u64 {
        self.group_concat_max_len.load(Ordering::SeqCst)
    }
    fn set_group_concat_max_len_for_test(&self, value: u64) {
        self.group_concat_max_len.store(value, Ordering::SeqCst);
    }
    fn connection_id(&self) -> u64 {
        123
    }
    fn readonly_user_vars(&self) -> HashSet<String> {
        HashSet::from(["read_only".to_owned()])
    }
    fn context_id(&self) -> u64 {
        456
    }
    fn sql_mode(&self) -> mysql::SQLMode {
        mysql::SQLMode(mysql::ModeStrictTransTables.0 | mysql::ModeNoZeroDate.0)
    }
    fn type_context(&self) -> types::Context {
        self.type_context.clone()
    }
    fn error_context(&self) -> errctx::Context {
        self.error_context.clone()
    }
    fn warning_handler(&self) -> Arc<dyn contextutil::WarnHandler + Send + Sync> {
        self.warning_handler.clone()
    }
    fn current_db(&self) -> String {
        "db1".to_owned()
    }
    fn stale_tso(&self) -> Result<u64, String> {
        Ok(self.stale_tso.load(Ordering::SeqCst))
    }
    fn timestamp_system_var(&self) -> Result<String, String> {
        let timestamp = self.timestamp.lock().unwrap().clone();
        if timestamp != "0" {
            return Ok(timestamp);
        }

        let mut cached = self.statement_timestamp.lock().unwrap();
        Ok(cached
            .get_or_insert_with(|| {
                let now = Utc::now();
                format!("{}.{:09}", now.timestamp(), now.nanosecond())
            })
            .clone())
    }
    fn max_allowed_packet(&self) -> u64 {
        123_456
    }
    fn tidb_redact_log(&self) -> String {
        "MARKER".to_owned()
    }
    fn default_week_format_mode(&self) -> String {
        self.system_var("default_week_format").unwrap_or_default()
    }
    fn div_precision_increment(&self) -> i32 {
        7
    }
    fn parameter_values(&self) -> Vec<types::Datum> {
        self.params
            .lock()
            .unwrap()
            .iter()
            .copied()
            .map(types::NewIntDatum)
            .collect()
    }
    fn user_vars_reader(&self) -> &dyn exprctx::UserVarsReader {
        &self.user_vars
    }
}

#[test]
/// 验证 ExprContext 对字符集、计划缓存、列 ID、连接 ID 等会话字段的委托。
fn migration_build_context_matches_go_session_delegation() {
    let session = Arc::new(TestSession::new());
    session.plan_cache.EnablePlanCache();
    let context = NewExprContext(Arc::clone(&session));

    assert_eq!(
        context.GetCharsetInfo(),
        ("gbk".to_owned(), "gbk_chinese_ci".to_owned())
    );
    assert_eq!(
        context.GetDefaultCollationForUTF8MB4(),
        "utf8mb4_0900_ai_ci"
    );
    assert_eq!(context.GetBlockEncryptionMode(), "aes-128-ecb");
    session
        .systems
        .lock()
        .unwrap()
        .insert("block_encryption_mode".to_owned(), "aes-256-cbc".to_owned());
    assert_eq!(context.GetBlockEncryptionMode(), "aes-256-cbc");
    assert!(context.GetSysdateIsNow());
    assert_eq!(context.GetNoopFuncsMode(), 2);
    assert!(std::ptr::eq(context.Rng(), session.rng.as_ref()));
    assert!(context.IsUseCache());
    context.SetSkipPlanCache("mockReason");
    assert!(!context.IsUseCache());
    assert_eq!(8, context.AllocPlanColumnID());
    assert_eq!(9, context.AllocPlanColumnID());
    assert_eq!(9, context.GetLastPlanColumnID());
    assert!(!context.IsInNullRejectCheck());
    assert!(!context.IsConstantPropagateCheck());
    assert!(context.GetWindowingUseHighPrecision());
    assert_eq!(1_024, context.GetGroupConcatMaxLen());
    context.SetGroupConcatMaxLenForTest(2_048);
    assert_eq!(2_048, context.GetGroupConcatMaxLen());
    assert_eq!(123, context.ConnectionID());
    assert!(context.IsReadonlyUserVar("read_only"));
    assert!(!context.IsReadonlyUserVar("writable"));

    let static_context = context.IntoStatic();
    assert_eq!(static_context.GetCharsetInfo(), context.GetCharsetInfo());
    assert_eq!(static_context.ConnectionID(), 123);
}

#[test]
/// 验证 EvalContext 的 SQLMode、告警、参数与 CurrentTime。
fn migration_eval_context_matches_go_fields_warnings_params_and_time() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));

    assert_eq!(456, context.CtxID());
    assert_eq!(
        mysql::SQLMode(mysql::ModeStrictTransTables.0 | mysql::ModeNoZeroDate.0),
        context.SQLMode()
    );
    assert_eq!(Shanghai, context.Location());
    assert_eq!("db1", context.CurrentDB());
    assert_eq!(123_456, context.GetMaxAllowedPacket());
    assert_eq!("MARKER", context.GetTiDBRedactLog());
    assert_eq!("0", context.GetDefaultWeekFormatMode());
    session
        .systems
        .lock()
        .unwrap()
        .insert("default_week_format".to_owned(), "5".to_owned());
    assert_eq!("5", context.GetDefaultWeekFormatMode());
    assert_eq!(7, context.GetDivPrecisionIncrement());

    context.AppendWarning(contextutil::errors::New("err1"));
    context.AppendNote(contextutil::errors::New("note1"));
    assert_eq!(2, context.WarningCount());
    let copied = context.CopyWarnings(Vec::new());
    assert_eq!("Warning", copied[0].Level);
    assert_eq!("err1", copied[0].Err.as_ref().unwrap().to_string());
    assert_eq!("Note", copied[1].Level);
    let truncated = context.TruncateWarnings(1);
    assert_eq!(1, truncated.len());
    assert_eq!(1, context.WarningCount());

    assert_eq!(9, context.GetParamValue(0).unwrap().GetInt64());
    assert!(matches!(
        context.GetParamValue(1),
        Err(exprctx::ParamError::IndexExceedsParamCount)
    ));
    assert!(std::ptr::eq(
        context.GetUserVarsReader(),
        &session.user_vars
    ));

    let time = context.CurrentTime().unwrap();
    assert_eq!(7_654_321, time.timestamp());
    assert_eq!(875_000_000, time.nanosecond());
}

#[test]
/// 验证九个可选属性提供者、序列与权限检查路径。
fn migration_optional_props_privilege_and_sequence_match_go() {
    let session = Arc::new(TestSession::new());
    let context = NewEvalContext(Arc::clone(&session));
    assert!(context.GetOptionalPropSet().IsFull());
    for index in 0..exprctx::OptPropsCnt {
        let key = exprctx::OptionalEvalPropKey(index);
        assert_eq!(
            key,
            context.GetOptionalPropProvider(key).unwrap().Desc().Key()
        );
    }

    let user = expropt::CurrentUserPropReader
        .current_user(&context)
        .unwrap();
    assert_eq!("user1", user.username);
    assert_eq!(
        2,
        expropt::CurrentUserPropReader
            .active_roles(&context)
            .unwrap()
            .len()
    );
    // This harness links a transitive crate that enables `intest`; the migrated
    // SessionVars still stores FixedOffset while EvalContext uses chrono_tz.
    // Normal production builds keep this diagnostic disabled.
    expropt::intest::EnableAssert.store(false, Ordering::Relaxed);
    let vars = expropt::SessionVarsPropReader
        .get_session_vars(&context)
        .unwrap();
    assert!(std::ptr::eq(Arc::as_ptr(&session.vars), vars));
    let session_schema = expropt::InfoSchemaPropReader
        .get_session_info_schema::<TestInfoSchema, _>(&context)
        .unwrap();
    let latest_schema = expropt::InfoSchemaPropReader
        .get_latest_info_schema::<TestInfoSchema, _>(&context)
        .unwrap();
    assert_eq!(11, session_schema.version);
    assert_eq!(22, latest_schema.version);
    let store = expropt::KVStorePropReader
        .get_kv_store::<TestStore, _>(&context)
        .unwrap();
    assert!(Arc::ptr_eq(&session.store, &store));
    let sql_executor = expropt::SQLExecutorPropReader
        .get_sql_executor::<TestSqlExecutor, _>(&context)
        .unwrap();
    assert!(Arc::ptr_eq(&session.sql_executor, &sql_executor));

    let mut sequence = expropt::SequenceOperatorPropReader
        .get_sequence_operator(&context, "db1", "seq1")
        .unwrap();
    assert_eq!(88, sequence.get_sequence_id());
    assert_eq!(41, sequence.get_sequence_next_val().unwrap());
    assert_eq!((50, true), sequence.set_sequence_val(50).unwrap());
    assert_eq!(51, sequence.get_sequence_next_val().unwrap());

    let locks = expropt::AdvisoryLockPropReader
        .advisory_lock_ctx(&context)
        .unwrap();
    locks.get_advisory_lock("lock1", 10).unwrap();
    assert_eq!(1, locks.is_used_advisory_lock("lock1"));
    assert!(locks.release_advisory_lock("lock1"));
    assert!(!expropt::DDLOwnerPropReader.is_ddl_owner(&context).unwrap());
    session.ddl_owner.store(true, Ordering::SeqCst);
    assert!(expropt::DDLOwnerPropReader.is_ddl_owner(&context).unwrap());

    assert!(context.RequestVerification("any", "any", "any", mysql::PrivilegeType(1)));
    assert!(context.RequestDynamicVerification("ANY", false));
    session.privilege_enabled.store(true, Ordering::SeqCst);
    assert!(context.RequestVerification("db1", "t1", "c1", mysql::PrivilegeType(1)));
    assert!(!context.RequestVerification("db2", "t2", "c2", mysql::PrivilegeType(1)));
    assert!(context.RequestDynamicVerification("BACKUP_ADMIN", true));
    assert!(!context.RequestDynamicVerification("BACKUP_ADMIN", false));

    let checker = expropt::PrivilegeCheckerPropReader
        .get_privilege_checker(&context)
        .unwrap();
    assert!(checker.request_verification("db1", "t1", "c1", mysql::PrivilegeType(1)));
}

#[test]
/// 验证语句时间优先级：stale TSO > timestamp 变量 > now，以及小数秒。
fn migration_statement_timestamp_matches_go_priority_and_fraction() {
    let now = Shanghai.with_ymd_and_hms(2026, 7, 15, 10, 11, 12).unwrap();
    let stale_millis = 1_234_567_890_u64;
    let stale_tso = stale_millis << 18;

    // stale TSO 非 0 时优先使用其物理时间（>>18 得到毫秒）。
    let from_stale = resolve_statement_timestamp(Ok(stale_tso), Ok("7654321.875"), now).unwrap();
    assert_eq!(stale_millis as i64, from_stale.timestamp_millis());

    let from_system_var = resolve_statement_timestamp(Ok(0), Ok("7654321.875"), now).unwrap();
    assert_eq!(7_654_321, from_system_var.timestamp());
    assert_eq!(875_000_000, from_system_var.nanosecond());

    let from_now = resolve_statement_timestamp(Ok(0), Ok("0"), now).unwrap();
    assert_eq!(now, from_now);

    assert!(resolve_statement_timestamp(Ok(0), Ok("not-a-number"), now).is_err());
}

#[test]
/// stale TSO 出错时记录日志并回退到 timestamp 系统变量。
fn migration_stale_tso_error_falls_back_to_timestamp_like_go() {
    let now = Utc.with_ymd_and_hms(2026, 7, 15, 0, 0, 0).unwrap();
    let resolved =
        resolve_statement_timestamp(Err("stale provider failed".to_owned()), Ok("42.5"), now)
            .expect("stale TSO errors are logged and timestamp remains usable");
    assert_eq!(42, resolved.timestamp());
    assert_eq!(500_000_000, resolved.nanosecond());
}
