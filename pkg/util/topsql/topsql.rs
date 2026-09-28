// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// TopSQL / TopRU 包级入口：初始化上报管线、注册 SQL/Plan、挂接 profiling 标签。
//
// TopSQL 用于采集高 CPU 语句的规范化 SQL、执行计划与资源用量，并上报给 reporter；
// 本文件桥接 collector、stmtstats 与 reporter，对齐 Go 包的生命周期顺序。

#![allow(non_snake_case, non_upper_case_globals)]

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::{collector, parser, stmtstats, topsqlstate};
use topsql_reporter as reporter_integration;

/// Maximum SQL text size registered with the TopSQL reporter. Go strings are
/// byte strings, so the limit is applied to UTF-8 bytes rather than characters.
/// TopSQL 上报注册的最大 SQL 文本字节长度（按 UTF-8 字节截断）。
pub const MaxSQLTextSize: usize = 4 * 1024;

/// Plans above this byte length are handed to the reporter as large plans.
/// 超过该字节长度的计划按「大计划」交给 reporter。
pub const MaxBinaryPlanSize: usize = 2 * 1024;

/// The server boundary used by reporter implementations that support TopSQL
/// pub/sub registration. The package integration supplies the concrete server.
/// 支持 TopSQL pub/sub 注册的服务端边界；具体实现由包集成注入。
pub trait PubSubServer: Send {
    /// 注册 reporter crate 提供的真实 TopSQL PubSub 服务。
    fn RegisterTopSQLPubSubService(
        &mut self,
        service: reporter_integration::pubsub::TopSqlPubSubService,
    );
}

/// Rust counterpart of reporter.TopSQLReporter. Keeping this boundary as a
/// trait preserves the Go interface assertions and permits focused tests to use
/// the same package-level entry points without starting remote workers.
/// reporter.TopSQLReporter 的 Rust 对应 trait；测试可用 mock 走同一入口。
pub trait TopSQLReporter: Send + Sync {
    /// 绑定 keyspace 名称（多租户/键空间标识）。
    fn BindKeyspaceName(&self, keyspace: Vec<u8>);
    /// 绑定进程 CPU 时间更新器。
    fn BindProcessCPUTimeUpdater(&self, updater: Arc<dyn collector::ProcessCPUTimeUpdater>);
    /// 启动 reporter。
    fn Start(self: Arc<Self>);
    /// 关闭 reporter。
    fn Close(&self);

    /// 注册规范化 SQL 文本与 digest。
    fn RegisterSQL(&self, digest: Vec<u8>, normalized_sql: Vec<u8>, is_internal: bool);
    /// 注册规范化执行计划与 digest。
    fn RegisterPlan(&self, digest: Vec<u8>, normalized_plan: String, is_large: bool);

    /// 收集一批语句统计映射。
    fn CollectStmtStatsMap(&self, stats: stmtstats::StatementStatsMap);

    /// Mirrors Go's optional `stmtstats.RUCollector` interface assertion.
    /// 是否额外实现 RU 收集（对齐 Go 可选接口断言）。
    fn SupportsRUCollector(&self) -> bool {
        false
    }

    /// 收集 RU 增量（默认空实现）。
    fn CollectRUIncrements(
        &self,
        _increments: stmtstats::RUIncrementMap,
        _version: stmtstats::RUVersion,
    ) {
    }

    /// RU 协议版本变更回调（默认空实现）。
    fn OnRUVersionChange(&self, _version: stmtstats::RUVersion) {}

    /// Mirrors Go's optional `reporter.DataSinkRegisterer` assertion.
    /// 注册 pub/sub 服务端（默认空实现）。
    fn RegisterPubSubServer(self: Arc<Self>, _server: &mut dyn PubSubServer) {}
}

/// Lifecycle boundary for reporter.SingleTargetDataSink.
/// 单目标 DataSink 的生命周期边界。
pub trait TopSQLDataSink: Send + Sync {
    /// 启动 sink。
    fn Start(self: Arc<Self>);
    /// 关闭 sink。
    fn Close(self: Arc<Self>);
}

/// 将动态 CPU updater 包装为 reporter 所需的具体类型。
struct ProcessCPUTimeUpdaterAdapter(Arc<dyn collector::ProcessCPUTimeUpdater>);

impl collector::ProcessCPUTimeUpdater for ProcessCPUTimeUpdaterAdapter {
    fn UpdateProcessCPUTime(&self, connID: u64, sqlID: u64, cpuTime: Duration) {
        self.0.UpdateProcessCPUTime(connID, sqlID, cpuTime);
    }
}

impl TopSQLReporter for reporter_integration::reporter::RemoteTopSQLReporter {
    fn BindKeyspaceName(&self, keyspace: Vec<u8>) {
        reporter_integration::reporter::RemoteTopSQLReporter::BindKeyspaceName(self, keyspace);
    }

    fn BindProcessCPUTimeUpdater(&self, updater: Arc<dyn collector::ProcessCPUTimeUpdater>) {
        reporter_integration::reporter::RemoteTopSQLReporter::BindProcessCPUTimeUpdater(
            self,
            Arc::new(ProcessCPUTimeUpdaterAdapter(updater)),
        );
    }

    fn Start(self: Arc<Self>) {
        reporter_integration::reporter::RemoteTopSQLReporter::Start(&self);
    }

    fn Close(&self) {
        reporter_integration::reporter::RemoteTopSQLReporter::Close(self);
    }

    fn RegisterSQL(&self, digest: Vec<u8>, normalized_sql: Vec<u8>, is_internal: bool) {
        reporter_integration::reporter::RemoteTopSQLReporter::RegisterSQL(
            self,
            digest,
            String::from_utf8_lossy(&normalized_sql).into_owned(),
            is_internal,
        );
    }

    fn RegisterPlan(&self, digest: Vec<u8>, normalized_plan: String, is_large: bool) {
        reporter_integration::reporter::RemoteTopSQLReporter::RegisterPlan(
            self,
            digest,
            normalized_plan,
            is_large,
        );
    }

    fn CollectStmtStatsMap(&self, stats: stmtstats::StatementStatsMap) {
        reporter_integration::reporter::RemoteTopSQLReporter::CollectStmtStatsMap(self, stats);
    }

    fn SupportsRUCollector(&self) -> bool {
        true
    }

    fn CollectRUIncrements(
        &self,
        increments: stmtstats::RUIncrementMap,
        version: stmtstats::RUVersion,
    ) {
        reporter_integration::reporter::RemoteTopSQLReporter::CollectRUIncrements(
            self, increments, version,
        );
    }

    fn OnRUVersionChange(&self, version: stmtstats::RUVersion) {
        reporter_integration::reporter::RemoteTopSQLReporter::OnRUVersionChange(self, version);
    }

    fn RegisterPubSubServer(self: Arc<Self>, server: &mut dyn PubSubServer) {
        server.RegisterTopSQLPubSubService(reporter_integration::pubsub::NewTopSQLPubSubService(
            self,
        ));
    }
}

impl TopSQLDataSink for reporter_integration::single_target::SingleTargetDataSink {
    fn Start(self: Arc<Self>) {
        reporter_integration::single_target::SingleTargetDataSink::Start(&self);
    }

    fn Close(self: Arc<Self>) {
        reporter_integration::single_target::SingleTargetDataSink::Close(&self);
    }
}

/// 将 TopSQLReporter 适配为 stmtstats::Collector。
struct StatementCollectorAdapter(Arc<dyn TopSQLReporter>);

impl stmtstats::Collector for StatementCollectorAdapter {
    fn CollectStmtStatsMap(&self, stats: stmtstats::StatementStatsMap) {
        self.0.CollectStmtStatsMap(stats);
    }
}

/// 将 TopSQLReporter 适配为 stmtstats::RUCollector。
struct RUCollectorAdapter(Arc<dyn TopSQLReporter>);

impl stmtstats::RUCollector for RUCollectorAdapter {
    fn CollectRUIncrements(
        &self,
        increments: stmtstats::RUIncrementMap,
        version: stmtstats::RUVersion,
    ) {
        self.0.CollectRUIncrements(increments, version);
    }

    fn OnRUVersionChange(&self, version: stmtstats::RUVersion) {
        self.0.OnRUVersionChange(version);
    }
}

/// 全局管线状态：reporter、sink 与已注册的收集器。
struct PipelineState {
    reporter: Option<Arc<dyn TopSQLReporter>>,
    single_target_data_sink: Option<Arc<dyn TopSQLDataSink>>,
    statement_collector: Option<Arc<dyn stmtstats::Collector>>,
    ru_collector: Option<Arc<dyn stmtstats::RUCollector>>,
}

/// 构造 Go `init()` 对应的真实 reporter + single-target 默认管线。
fn default_pipeline() -> PipelineState {
    let reporter = reporter_integration::reporter::NewRemoteTopSQLReporter(
        |plan| {
            // The reporter accepts UTF-8 normalized input, so rendered fields stay UTF-8.
            let bytes = plancodec::DecodeNormalizedPlan(plan).map_err(|error| error.to_string())?;
            String::from_utf8(bytes).map_err(|error| error.to_string())
        },
        |plan| Ok(plancodec::Compress(plan.as_bytes())),
    );
    let single_target_data_sink =
        reporter_integration::single_target::NewSingleTargetDataSink(reporter.clone());
    PipelineState {
        reporter: Some(reporter),
        single_target_data_sink: Some(single_target_data_sink),
        statement_collector: None,
        ru_collector: None,
    }
}

/// 测试专用：恢复与进程首次访问时完全相同的真实默认管线。
#[cfg(test)]
pub(crate) fn reset_default_pipeline_for_test() {
    *pipeline() = default_pipeline();
}

/// 进程级 TopSQL 管线单例。
static PIPELINE: LazyLock<Mutex<PipelineState>> = LazyLock::new(|| Mutex::new(default_pipeline()));

thread_local! {
    /// 当前线程最近一次挂接的 pprof 风格标签（对齐 Go goroutine labels）。
    static CURRENT_THREAD_PROFILE_LABELS: RefCell<HashMap<String, String>> =
        RefCell::new(HashMap::new());
}

/// 获取管线互斥锁；中毒时仍取出内层状态。
fn pipeline() -> std::sync::MutexGuard<'static, PipelineState> {
    PIPELINE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// 返回已初始化的 reporter，否则 panic。
fn reporter() -> Arc<dyn TopSQLReporter> {
    pipeline()
        .reporter
        .clone()
        .expect("TopSQL pipeline must be initialized before use")
}

/// Whether bootstrapping installed a reporter for the process pipeline.
pub fn TopProfilingReporterAvailable() -> bool {
    pipeline().reporter.is_some()
}

/// 将 ProfileContext 中的标签写入本线程 TLS。
fn set_goroutine_labels(ctx: &collector::ProfileContext) {
    CURRENT_THREAD_PROFILE_LABELS.with(|labels| {
        *labels.borrow_mut() = ctx.labels().clone();
    });
}

/// Returns the labels installed by the latest enabled attach operation on this
/// thread. It is the Rust-native equivalent of Go's goroutine pprof labels.
/// 返回本线程最近一次启用挂接写入的 profiling 标签。
pub fn current_thread_profile_labels() -> HashMap<String, String> {
    CURRENT_THREAD_PROFILE_LABELS.with(|labels| labels.borrow().clone())
}

/// Rust has no package `init` hook. The module integration calls this once with
/// the concrete remote reporter and single-target sink created from the
/// completed reporter package. Tests use the same boundary with mocks.
/// 初始化管线：注入 reporter 与单目标 sink，并清空收集器与线程标签。
pub fn InitializeTopProfiling(
    reporter: Arc<dyn TopSQLReporter>,
    single_target_data_sink: Arc<dyn TopSQLDataSink>,
) {
    let mut state = pipeline();
    state.reporter = Some(reporter);
    state.single_target_data_sink = Some(single_target_data_sink);
    state.statement_collector = None;
    state.ru_collector = None;
    CURRENT_THREAD_PROFILE_LABELS.with(|labels| labels.borrow_mut().clear());
}

/// Sets up the shared TopSQL and TopRU pipeline in the same order as Go.
/// 按与 Go 相同的顺序装配并启动 TopSQL / TopRU 共享管线。
pub fn SetupTopProfiling(
    keyspaceName: Vec<u8>,
    updater: Arc<dyn collector::ProcessCPUTimeUpdater>,
    ruVersionProvider: Arc<dyn stmtstats::RUVersionProvider>,
) {
    let (reporter, data_sink) = {
        let state = pipeline();
        (
            state
                .reporter
                .clone()
                .expect("TopSQL pipeline must be initialized before setup"),
            state
                .single_target_data_sink
                .clone()
                .expect("TopSQL data sink must be initialized before setup"),
        )
    };

    reporter.BindKeyspaceName(keyspaceName);
    reporter.BindProcessCPUTimeUpdater(updater);
    reporter.clone().Start();
    data_sink.clone().Start();

    let statement_collector: Arc<dyn stmtstats::Collector> =
        Arc::new(StatementCollectorAdapter(reporter.clone()));
    stmtstats::RegisterCollector(statement_collector.clone());

    // 仅当 reporter 声明支持 RU 时注册 RU 收集器。
    let ru_collector = reporter.SupportsRUCollector().then(|| {
        let collector: Arc<dyn stmtstats::RUCollector> =
            Arc::new(RUCollectorAdapter(reporter.clone()));
        stmtstats::RegisterRUCollector(collector.clone());
        collector
    });

    stmtstats::BindRUVersionProvider(Some(ruVersionProvider));
    stmtstats::SetupAggregator();

    let mut state = pipeline();
    state.statement_collector = Some(statement_collector);
    state.ru_collector = ru_collector;
}

/// Replaces only the reporter, matching Go's test helper. The data sink is not
/// rebuilt because callers that need it install the complete pipeline first.
/// 仅替换 reporter（测试辅助）；不重建 data sink。
pub fn SetupTopProfilingForTest(reporter: Arc<dyn TopSQLReporter>) {
    pipeline().reporter = Some(reporter);
}

/// 将 pub/sub 服务端注册到当前 reporter。
pub fn RegisterPubSubServer(server: &mut dyn PubSubServer) {
    reporter().RegisterPubSubServer(server);
}

/// Closes resources in Go order: optional RU collector, data sink, reporter,
/// aggregator, then RU version provider.
/// 按 Go 顺序关闭：可选 RU 收集器 → sink → reporter → 聚合器 → RU 版本提供者。
pub fn Close() {
    let (reporter, data_sink, ru_collector) = {
        let mut state = pipeline();
        (
            state
                .reporter
                .clone()
                .expect("TopSQL pipeline must be initialized before close"),
            state
                .single_target_data_sink
                .clone()
                .expect("TopSQL data sink must be initialized before close"),
            state.ru_collector.take(),
        )
    };

    if let Some(ru_collector) = ru_collector {
        stmtstats::UnregisterRUCollector(&ru_collector);
    }
    data_sink.Close();
    reporter.Close();
    stmtstats::CloseAggregator();
    stmtstats::BindRUVersionProvider(None);
}

/// 在存在 SQL digest 时把规范化 SQL 文本关联注册到 reporter。
pub fn RegisterSQL(
    normalizedSQL: impl AsRef<str>,
    sqlDigest: Option<&parser::Digest>,
    isInternal: bool,
) {
    if let Some(sqlDigest) = sqlDigest {
        linkSQLTextWithDigest(
            sqlDigest.Bytes().to_vec(),
            normalizedSQL.as_ref(),
            isInternal,
        );
    }
}

/// 在存在 Plan digest 时把规范化计划关联注册到 reporter。
pub fn RegisterPlan(normalizedPlan: impl Into<String>, planDigest: Option<&parser::Digest>) {
    if let Some(planDigest) = planDigest {
        linkPlanTextWithDigest(planDigest.Bytes().to_vec(), normalizedPlan.into());
    }
}

/// 向 profiling 上下文挂接 SQL digest，并在 TopSQL 开启时注册文本与标签。
pub fn AttachAndRegisterSQLInfo(
    mut ctx: collector::ProfileContext,
    normalizedSQL: impl AsRef<str>,
    sqlDigest: Option<&parser::Digest>,
    isInternal: bool,
) -> collector::ProfileContext {
    let Some(sqlDigest) = sqlDigest.filter(|digest| !digest.String().is_empty()) else {
        return ctx;
    };

    ctx = collector::CtxWithSQLDigest(ctx, sqlDigest.String().to_owned());
    if topsqlstate::TopSQLEnabled() {
        set_goroutine_labels(&ctx);
    }

    let normalized_sql = normalizedSQL.as_ref();
    linkSQLTextWithDigest(sqlDigest.Bytes().to_vec(), normalized_sql, isInternal);
    mock_high_load_for_sql_failpoint(normalized_sql);
    ctx
}

/// 向 profiling 上下文同时挂接 SQL 与 Plan digest，并按需写入线程标签。
pub fn AttachSQLAndPlanInfo(
    mut ctx: collector::ProfileContext,
    sqlDigest: Option<&parser::Digest>,
    planDigest: Option<&parser::Digest>,
) -> collector::ProfileContext {
    let Some(sqlDigest) = sqlDigest.filter(|digest| !digest.String().is_empty()) else {
        return ctx;
    };

    let plan_digest = planDigest.map_or_else(String::new, |digest| digest.String().to_owned());
    ctx = collector::CtxWithSQLAndPlanDigest(ctx, sqlDigest.String().to_owned(), plan_digest);
    if topsqlstate::TopSQLEnabled() {
        set_goroutine_labels(&ctx);
    }

    mock_high_load_for_plan_failpoint();
    ctx
}

/// 向 profiling 上下文挂接连接/语句进程信息，并按需写入线程标签。
pub fn AttachAndRegisterProcessInfo(
    mut ctx: collector::ProfileContext,
    connID: u64,
    sqlID: u64,
) -> collector::ProfileContext {
    // CtxWithProcessInfo installs its labels unconditionally, as the Go helper
    // itself calls pprof.SetGoroutineLabels before this package-level guard.
    ctx = collector::CtxWithProcessInfo(ctx, connID, sqlID);
    if topsqlstate::TopSQLEnabled() {
        set_goroutine_labels(&ctx);
    }
    ctx
}

/// Busy-spins only for matching SQL prefixes. The filtering and duration are
/// deliberately byte-for-byte equivalents of the Go failpoint helper.
/// 仅对匹配前缀的 SQL 忙等制造 CPU 负载；过滤与时长对齐 Go failpoint 辅助函数。
pub fn MockHighCPULoad(sql: &str, sqlPrefixs: &[&str], load: i64) -> bool {
    let lowerSQL = sql.to_lowercase();
    if lowerSQL.contains("mysql") && !lowerSQL.contains("global_variables") {
        return false;
    }
    if !sqlPrefixs.iter().any(|prefix| lowerSQL.starts_with(prefix)) {
        return false;
    }

    let start = Instant::now();
    let target = if load <= 0 {
        Duration::ZERO
    } else {
        Duration::from_millis(12_u64.saturating_mul(load as u64))
    };
    // 忙等到目标时长，用于单测中模拟高 CPU。
    while start.elapsed() <= target {
        for value in 0..1_000_000_u32 {
            std::hint::black_box(value);
        }
    }
    true
}

/// 截断 SQL 文本后注册到 reporter。
fn linkSQLTextWithDigest(sqlDigest: Vec<u8>, normalizedSQL: &str, isInternal: bool) {
    let bytes = normalizedSQL.as_bytes();
    let normalized_sql = bytes[..bytes.len().min(MaxSQLTextSize)].to_vec();
    reporter().RegisterSQL(sqlDigest, normalized_sql, isInternal);
}

/// 按是否超长标记大计划后注册到 reporter。
fn linkPlanTextWithDigest(planDigest: Vec<u8>, normalizedBinaryPlan: String) {
    let is_large = normalizedBinaryPlan.len() > MaxBinaryPlanSize;
    reporter().RegisterPlan(planDigest, normalizedBinaryPlan, is_large);
}

#[cfg(feature = "failpoints")]
fn failpoint_enabled(name: &str) -> bool {
    fail::eval(name, |value| {
        value
            .as_deref()
            .map_or(true, |value| value.parse::<bool>().unwrap_or(false))
    })
    .unwrap_or(false)
}

#[cfg(not(feature = "failpoints"))]
fn failpoint_enabled(_name: &str) -> bool {
    false
}

/// SQL 挂接路径上的高负载 failpoint 钩子。
fn mock_high_load_for_sql_failpoint(normalized_sql: &str) {
    if !failpoint_enabled("mockHighLoadForEachSQL") {
        return;
    }
    let prefixes = [
        "insert",
        "update",
        "delete",
        "load",
        "replace",
        "select",
        "begin",
        "commit",
        "analyze",
        "explain",
        "trace",
        "create",
        "set global",
    ];
    if MockHighCPULoad(normalized_sql, &prefixes, 1) {
        log::info!("attach SQL info; sql={normalized_sql}");
    }
}

/// Plan 挂接路径上的高负载 failpoint 钩子。
fn mock_high_load_for_plan_failpoint() {
    if failpoint_enabled("mockHighLoadForEachPlan") && MockHighCPULoad("", &[""], 1) {
        log::info!("attach SQL info");
    }
}
