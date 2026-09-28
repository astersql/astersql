// Copyright 2026 AsterSQL.

//! 扩展 Bootstrap 适配层的行为回归测试。
//!
//! 通过可编排的 SQL 执行器、结果集和会话池，对齐 Go 实现的关键语义：
//! Bootstrap SQL 使用内部请求来源、结果集始终关闭、错误保持既定优先级，
//! 且从系统会话池成功借出的资源最终都会归还。

use std::any::Any;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use astersql_extension as extension;
use astersql_kv as kv;
use astersql_util_sqlexec as sqlexec;

use super::{Bootstrap, BootstrapDomain, bootstrapContext};

/// 捕获一次内部 SQL 调用中适配层必须透传或补充的执行信息。
#[derive(Clone, Debug, Eq, PartialEq)]
struct ExecutorCall {
    sql: String,
    internal: bool,
    source: String,
    cancelled: bool,
    arg_count: usize,
}

/// 脚本化一次内部 SQL 调用的结果；队列顺序就是执行器的调用顺序。
enum ExecutePlan {
    None,
    Error(&'static str),
    Record(RecordPlan),
}

/// 描述结果集的分批数据，以及拉取和关闭阶段的故障注入点。
struct RecordPlan {
    batches: Vec<Vec<i64>>,
    fail_at: Option<usize>,
    close_error: bool,
    close_count: Arc<AtomicUsize>,
}

#[derive(Default)]
struct ExecutorState {
    calls: Vec<ExecutorCall>,
    plans: VecDeque<ExecutePlan>,
}

/// 记录调用并按先进先出顺序消费执行计划的 SQL 执行器。
struct MockExecutor {
    state: Arc<Mutex<ExecutorState>>,
}

impl sqlexec::SQLExecutor for MockExecutor {
    fn Execute(
        &mut self,
        _ctx: &sqlexec::context::Context,
        _sql: &str,
    ) -> Result<Vec<Box<dyn sqlexec::RecordSet>>, sqlexec::GoError> {
        Ok(Vec::new())
    }

    fn ExecuteInternal(
        &mut self,
        ctx: &sqlexec::context::Context,
        sql: &str,
        args: Vec<Box<dyn Any>>,
    ) -> Result<Option<Box<dyn sqlexec::RecordSet>>, sqlexec::GoError> {
        let mut state = self.state.lock().expect("executor state lock poisoned");
        let request_source = ctx.RequestSource().cloned().unwrap_or_default();
        state.calls.push(ExecutorCall {
            sql: sql.to_owned(),
            internal: request_source.RequestSourceInternal,
            source: request_source.RequestSourceType,
            cancelled: ctx.is_cancelled(),
            arg_count: args.len(),
        });
        match state.plans.pop_front().unwrap_or(ExecutePlan::None) {
            ExecutePlan::None => Ok(None),
            ExecutePlan::Error(message) => Err(std::io::Error::other(message).into()),
            ExecutePlan::Record(plan) => Ok(Some(Box::new(ScriptedRecordSet::new(plan)))),
        }
    }

    fn ExecuteStmt(
        &mut self,
        _ctx: &sqlexec::context::Context,
        _stmt_node: sqlexec::ast::NodeRef,
    ) -> Result<Option<Box<dyn sqlexec::RecordSet>>, sqlexec::GoError> {
        Ok(None)
    }
}

/// 按批次产出整数行，并可在指定的 `Next` 调用或关闭阶段报错。
struct ScriptedRecordSet {
    fields: Vec<sqlexec::resolve::ResultField>,
    batches: Vec<Vec<i64>>,
    next_call: usize,
    fail_at: Option<usize>,
    close_error: bool,
    close_count: Arc<AtomicUsize>,
}

impl ScriptedRecordSet {
    fn new(plan: RecordPlan) -> Self {
        Self {
            fields: Vec::new(),
            batches: plan.batches,
            next_call: 0,
            fail_at: plan.fail_at,
            close_error: plan.close_error,
            close_count: plan.close_count,
        }
    }
}

impl sqlexec::RecordSet for ScriptedRecordSet {
    fn Fields(&self) -> &[sqlexec::resolve::ResultField] {
        &self.fields
    }

    fn Next(
        &mut self,
        _ctx: &sqlexec::context::Context,
        request: &mut sqlexec::RecordChunk,
    ) -> Result<(), sqlexec::GoError> {
        if self.fail_at == Some(self.next_call) {
            self.next_call += 1;
            return Err(std::io::Error::other("next failed").into());
        }
        let batch = self
            .batches
            .get(self.next_call)
            .cloned()
            .unwrap_or_default();
        self.next_call += 1;
        request.with_chunk_mut(|chunk| {
            chunk.Reset();
            for value in batch {
                chunk.AppendInt64(0, value);
            }
        });
        Ok(())
    }

    fn NewChunk(
        &self,
        allocator: Option<&mut dyn sqlexec::chunk::Allocator>,
    ) -> sqlexec::RecordChunk {
        let fields = vec![sqlexec::chunk::types::NewFieldType(
            sqlexec::chunk::mysql::TypeLonglong,
        )];
        match allocator {
            Some(allocator) => sqlexec::RecordChunk::from_allocated(allocator.Alloc(&fields, 0, 8)),
            None => sqlexec::RecordChunk::from_boxed(sqlexec::chunk::New(fields, 8, 8)),
        }
    }

    fn Close(&mut self) -> Result<(), sqlexec::GoError> {
        self.close_count.fetch_add(1, Ordering::SeqCst);
        if self.close_error {
            Err(std::io::Error::other("close failed").into())
        } else {
            Ok(())
        }
    }
}

#[derive(Default)]
struct MockResource;

/// 统计资源借还次数，并支持在借用阶段注入错误的系统会话池。
struct MockPool {
    gets: AtomicUsize,
    puts: AtomicUsize,
    fail_get: AtomicBool,
}

impl MockPool {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            gets: AtomicUsize::new(0),
            puts: AtomicUsize::new(0),
            fail_get: AtomicBool::new(false),
        })
    }
}

impl extension::SessionPool for MockPool {
    fn Get(&self) -> Result<Box<dyn extension::SessionResource>, extension::ExtensionError> {
        self.gets.fetch_add(1, Ordering::SeqCst);
        if self.fail_get.load(Ordering::SeqCst) {
            Err(extension::ExtensionError::new("pool get failed"))
        } else {
            Ok(Box::new(MockResource))
        }
    }

    fn Put(&self, _resource: Box<dyn extension::SessionResource>) {
        self.puts.fetch_add(1, Ordering::SeqCst);
    }
}

/// 为 Bootstrap 提供可控的会话池、SQL 执行器和会话类型校验结果。
struct MockDomain {
    pool: Arc<MockPool>,
    executor: Arc<Mutex<ExecutorState>>,
    valid_session: AtomicBool,
}

impl MockDomain {
    fn new() -> Self {
        Self {
            pool: MockPool::new(),
            executor: Arc::new(Mutex::new(ExecutorState::default())),
            valid_session: AtomicBool::new(true),
        }
    }

    fn push_plan(&self, plan: ExecutePlan) {
        self.executor
            .lock()
            .expect("executor state lock poisoned")
            .plans
            .push_back(plan);
    }
}

impl BootstrapDomain for MockDomain {
    fn SysSessionPool(&self) -> Arc<dyn extension::SessionPool> {
        self.pool.clone()
    }

    fn GetSQLExecutor<'a>(
        &self,
        _resource: &'a mut dyn extension::SessionResource,
    ) -> Option<Box<dyn sqlexec::SQLExecutor + 'a>> {
        self.valid_session.load(Ordering::SeqCst).then(|| {
            Box::new(MockExecutor {
                state: Arc::clone(&self.executor),
            }) as Box<dyn sqlexec::SQLExecutor>
        })
    }

    fn SessionResourceTypeName(&self, _resource: &dyn extension::SessionResource) -> String {
        "mock.InvalidSession".to_owned()
    }

    fn GetEtcdClient(&self) -> Option<Arc<extension::etcd_client::Client>> {
        None
    }
}

fn record_plan(
    batches: Vec<Vec<i64>>,
    fail_at: Option<usize>,
    close_error: bool,
    close_count: &Arc<AtomicUsize>,
) -> ExecutePlan {
    ExecutePlan::Record(RecordPlan {
        batches,
        fail_at,
        close_error,
        close_count: Arc::clone(close_count),
    })
}

/// 组装只用于直接测试 `bootstrapContext::ExecuteSQL` 的最小上下文。
fn context_for<'a>(
    context: kv::Context,
    executor: Box<dyn sqlexec::SQLExecutor + 'a>,
    pool: Arc<MockPool>,
) -> bootstrapContext<'a> {
    bootstrapContext {
        context,
        sql_executor: executor,
        etcd_client: None,
        session_pool: pool,
    }
}

/// 编译期确认 KV 上下文可直接作为 SQL 执行上下文使用，无需另建适配类型。
#[test]
fn kv_context_is_the_sql_execution_context() {
    fn accepts_sql_context(_: &sqlexec::context::Context) {}

    let context = kv::Context::default();
    accepts_sql_context(&context);
}

/// 校验内部来源标记、原上下文取消状态、分批排空和结果集关闭行为。
#[test]
fn execute_sql_marks_internal_source_drains_rows_and_closes() {
    let state = Arc::new(Mutex::new(ExecutorState::default()));
    let close_count = Arc::new(AtomicUsize::new(0));
    state
        .lock()
        .expect("executor state lock poisoned")
        .plans
        .push_back(record_plan(
            vec![vec![1, 2], vec![3]],
            None,
            false,
            &close_count,
        ));
    let context = kv::Context::new();
    context.cancel();
    let pool = MockPool::new();
    let mut bootstrap_context = context_for(
        context,
        Box::new(MockExecutor {
            state: Arc::clone(&state),
        }),
        pool,
    );

    let rows = extension::BootstrapContext::ExecuteSQL(&mut bootstrap_context, "select 1")
        .expect("drain succeeds");

    assert_eq!(
        rows.iter().map(|row| row.GetInt64(0)).collect::<Vec<_>>(),
        [1, 2, 3]
    );
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
    assert_eq!(
        state.lock().expect("executor state lock poisoned").calls,
        [ExecutorCall {
            sql: "select 1".to_owned(),
            internal: true,
            source: kv::InternalTxnBootstrap.to_owned(),
            cancelled: true,
            arg_count: 0,
        }]
    );
}

/// 对齐 Go 的错误规则：排空错误优先于关闭错误，仅排空成功时返回关闭错误。
#[test]
fn execute_sql_preserves_go_error_and_close_precedence() {
    let state = Arc::new(Mutex::new(ExecutorState::default()));
    let close_count = Arc::new(AtomicUsize::new(0));
    {
        let mut state = state.lock().expect("executor state lock poisoned");
        // 四个计划依次覆盖执行失败、空结果集、排空与关闭同时失败、仅关闭失败。
        state.plans.push_back(ExecutePlan::Error("execute failed"));
        state.plans.push_back(ExecutePlan::None);
        state
            .plans
            .push_back(record_plan(vec![vec![1]], Some(1), true, &close_count));
        state
            .plans
            .push_back(record_plan(vec![vec![2]], None, true, &close_count));
    }
    let pool = MockPool::new();
    let mut bootstrap_context = context_for(
        kv::Context::new(),
        Box::new(MockExecutor {
            state: Arc::clone(&state),
        }),
        pool,
    );

    let error = extension::BootstrapContext::ExecuteSQL(&mut bootstrap_context, "execute error")
        .expect_err("execute error propagates");
    assert_eq!(error.to_string(), "execute failed");

    let rows = extension::BootstrapContext::ExecuteSQL(&mut bootstrap_context, "no result")
        .expect("nil record set maps to empty rows");
    assert!(rows.is_empty());

    let error = extension::BootstrapContext::ExecuteSQL(&mut bootstrap_context, "drain error")
        .expect_err("drain error wins over close error");
    assert_eq!(error.to_string(), "next failed");

    let error = extension::BootstrapContext::ExecuteSQL(&mut bootstrap_context, "close error")
        .expect_err("close error replaces successful drain");
    assert_eq!(error.to_string(), "close failed");
    assert_eq!(close_count.load(Ordering::SeqCst), 2);
}

/// 以作用域守卫复位全局扩展注册表，避免测试结束或断言失败后污染其他用例。
struct RegistryReset;

impl Drop for RegistryReset {
    fn drop(&mut self) {
        extension::Reset();
    }
}

/// 清空全局注册表并注册一个 Bootstrap 钩子，使每个场景互相隔离。
fn register_bootstrap(
    bootstrap: impl Fn(&mut dyn extension::BootstrapContext) -> Result<(), extension::ExtensionError>
    + Send
    + Sync
    + 'static,
) {
    extension::Reset();
    extension::Register(
        "bootstrap-test".to_owned(),
        vec![extension::WithBootstrap(bootstrap)],
    )
    .expect("register bootstrap extension");
}

/// 覆盖 Go Bootstrap 的短路、错误传播，以及成功借出资源后的统一归还路径。
#[test]
fn bootstrap_matches_go_short_circuit_pool_error_and_resource_cleanup_paths() {
    let _reset = RegistryReset;

    // 没有已注册扩展时应在访问系统会话池前直接返回。
    extension::Reset();
    let domain = MockDomain::new();
    Bootstrap(&kv::Context::new(), &domain).expect("no extensions short-circuit");
    assert_eq!(domain.pool.gets.load(Ordering::SeqCst), 0);

    // 注册表构造失败同样发生在借用会话之前。
    extension::Reset();
    extension::RegisterFactory(
        "broken".to_owned(),
        Arc::new(|| Err(extension::ExtensionError::new("registry failed"))),
    )
    .expect("register failing factory");
    let domain = MockDomain::new();
    let error = Bootstrap(&kv::Context::new(), &domain).expect_err("registry error propagates");
    assert_eq!(error.to_string(), "registry failed");
    assert_eq!(domain.pool.gets.load(Ordering::SeqCst), 0);

    // 会话池借用失败时不能执行扩展钩子，也没有资源需要归还。
    let hook_calls = Arc::new(AtomicUsize::new(0));
    let hook_calls_inner = Arc::clone(&hook_calls);
    register_bootstrap(move |_| {
        hook_calls_inner.fetch_add(1, Ordering::SeqCst);
        Ok(())
    });
    let domain = MockDomain::new();
    domain.pool.fail_get.store(true, Ordering::SeqCst);
    let error = Bootstrap(&kv::Context::new(), &domain).expect_err("pool error propagates");
    assert_eq!(error.to_string(), "pool get failed");
    assert_eq!(domain.pool.puts.load(Ordering::SeqCst), 0);
    assert_eq!(hook_calls.load(Ordering::SeqCst), 0);

    // 会话类型校验失败仍须归还已经借出的资源。
    register_bootstrap(|_| Ok(()));
    let domain = MockDomain::new();
    domain.valid_session.store(false, Ordering::SeqCst);
    let error =
        Bootstrap(&kv::Context::new(), &domain).expect_err("invalid session must be rejected");
    assert_eq!(
        error.to_string(),
        "type 'mock.InvalidSession' cannot be casted to 'sessionctx.Context'"
    );
    assert_eq!(domain.pool.gets.load(Ordering::SeqCst), 1);
    assert_eq!(domain.pool.puts.load(Ordering::SeqCst), 1);

    // 成功路径向钩子暴露同一会话池，并完整排空、关闭 SQL 结果集。
    let observed_rows = Arc::new(Mutex::new(Vec::new()));
    let observed_rows_inner = Arc::clone(&observed_rows);
    register_bootstrap(move |context| {
        assert!(context.EtcdClient().is_none());
        assert!(!context.is_cancelled());
        let resource = context.SessionPool().Get()?;
        context.SessionPool().Put(resource);
        let rows = context.ExecuteSQL("select bootstrap")?;
        *observed_rows_inner
            .lock()
            .expect("observed rows lock poisoned") =
            rows.iter().map(|row| row.GetInt64(0)).collect();
        Ok(())
    });
    let domain = MockDomain::new();
    let close_count = Arc::new(AtomicUsize::new(0));
    domain.push_plan(record_plan(vec![vec![10, 11]], None, false, &close_count));
    Bootstrap(&kv::Context::new(), &domain).expect("extension bootstrap succeeds");
    assert_eq!(
        *observed_rows.lock().expect("observed rows lock poisoned"),
        [10, 11]
    );
    assert_eq!(close_count.load(Ordering::SeqCst), 1);
    assert_eq!(domain.pool.gets.load(Ordering::SeqCst), 2);
    assert_eq!(domain.pool.puts.load(Ordering::SeqCst), 2);

    // 钩子返回错误时，外层 Bootstrap 借用的资源也必须归还。
    register_bootstrap(|_| Err(extension::ExtensionError::new("hook failed")));
    let domain = MockDomain::new();
    let error = Bootstrap(&kv::Context::new(), &domain).expect_err("hook error propagates");
    assert_eq!(error.to_string(), "hook failed");
    assert_eq!(domain.pool.gets.load(Ordering::SeqCst), 1);
    assert_eq!(domain.pool.puts.load(Ordering::SeqCst), 1);
}
