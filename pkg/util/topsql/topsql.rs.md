# `pkg/util/topsql/topsql.rs` 逻辑说明

源文件：[`topsql.rs`](topsql.rs)

## 文件定位

本文件是 `astersql-util-topsql` crate 的包级业务入口。`lib.rs` 将 `collector`、`stmtstats`、`topsql_state` 重新导出，并将本文件的公开项整体 re-export；因此调用方通过 `astersql_util_topsql::*` 使用这里的初始化、元数据注册和 profiling 上下文挂接 API，而不直接依赖私有 `topsql` 模块。

它位于 SQL 执行路径与 TopSQL/TopRU 子系统之间：上游提供 SQL/Plan digest、规范化文本、连接与语句标识；本文件把数据写入 `collector::ProfileContext` 或转交 reporter，并负责把 reporter、单目标 data sink、语句统计聚合器和 RU 版本提供者按固定顺序装配。`pkg/session/runtime/scan_adapter_runtime.rs` 的 `TopSQLStart` 是当前可见的 Rust 运行时调用点之一：先检查 `TopProfilingReporterAvailable`，再调用 `RegisterSQL` 和 `RegisterPlan`。

`pkg/util/topsql/Cargo.toml` 声明该 crate 直接依赖 collector、parser、plancodec、stmtstats、state 和 reporter 子 crate；可选 feature `failpoints` 仅控制高 CPU 模拟钩子的真实启用。该 crate 还被 session、server、DDL、executor 和 planner 等 Cargo 包依赖，但依赖声明本身不等同于这些包必然调用本文件的每个入口。

## 核心职责

1. 用 `PIPELINE: LazyLock<Mutex<PipelineState>>` 保存进程级 reporter、single-target sink 及已经注册的 statement/RU collector；`default_pipeline` 代替 Go 包的 `init()` 创建真实默认管线。
2. `SetupTopProfiling` 按 Go 顺序绑定 keyspace 与 CPU updater、启动 reporter/sink、注册 statement/RU collector、绑定 RU 版本提供者并启动聚合器；`Close` 按匹配的收尾顺序释放资源。
3. `RegisterSQL`、`RegisterPlan` 以及内部 `link*WithDigest` 建立 digest 到规范化 SQL/Plan 的元数据关联，并执行 4 KiB SQL 截断和 2 KiB 大计划标记。
4. `AttachAndRegisterSQLInfo`、`AttachSQLAndPlanInfo`、`AttachAndRegisterProcessInfo` 将 digest 或进程标识放入 `ProfileContext`，在 TopSQL 开启时把标签镜像到当前线程 TLS。
5. 通过 `TopSQLReporter`、`TopSQLDataSink`、`PubSubServer` 和三个适配器隔离具体 reporter/collector 类型，使生产实现和测试 mock 共用同一包级生命周期入口。
6. `MockHighCPULoad` 与两个 failpoint helper 为采样测试制造可观测 CPU 时间，不参与正常业务计算。

## 主要符号

- `MaxSQLTextSize = 4 * 1024`：SQL 元数据最大 UTF-8 字节数；限制按字节而非字符应用。
- `MaxBinaryPlanSize = 2 * 1024`：计划超过该字节长度时传给 reporter 的 `is_large` 为 `true`，本文件并不自行丢弃计划正文。
- `PubSubServer`：接收 reporter 创建的 `TopSqlPubSubService` 的服务端边界。
- `TopSQLReporter`：覆盖绑定、启动/关闭、SQL/Plan 注册和 statement stats 收集；RU 与 pub/sub 能力是带默认空实现的可选能力，由 `SupportsRUCollector` 显式声明 RU 支持。
- `TopSQLDataSink`：只定义 `Start`/`Close` 生命周期。
- `ProcessCPUTimeUpdaterAdapter`、`StatementCollectorAdapter`、`RUCollectorAdapter`：分别把动态 updater/reporter 对象适配到 collector 或 stmtstats 所要求的 trait。
- `PipelineState`：持有 `reporter`、`single_target_data_sink`、`statement_collector`、`ru_collector` 四个强引用，确保已注册适配器在管线期间存活。
- `PIPELINE` 与 `pipeline()`：全局惰性初始化互斥状态；互斥锁中毒时通过 `into_inner` 继续取得状态。
- `CURRENT_THREAD_PROFILE_LABELS` 与 `current_thread_profile_labels`：线程局部标签镜像及其只读快照接口，模拟 Go goroutine pprof labels 的可观测结果。
- 生命周期入口：`InitializeTopProfiling`、`SetupTopProfiling`、`SetupTopProfilingForTest`、`RegisterPubSubServer`、`Close`。
- 数据入口：`RegisterSQL`、`RegisterPlan`、三个 `Attach*` 函数以及测试辅助 `MockHighCPULoad`。
- 条件编译：两版 `failpoint_enabled` 由 `feature = "failpoints"` 选择；关闭 feature 时所有 failpoint 查询恒为 `false`。

## 执行流程

默认构造流程如下：首次访问 `PIPELINE` 时，`default_pipeline` 用 `plancodec::DecodeNormalizedPlan` 和 `plancodec::Compress` 构造 `RemoteTopSQLReporter`，再用该 reporter 构造 `SingleTargetDataSink`。解码结果必须是 UTF-8；解码或 UTF-8 转换错误被转成字符串交给 reporter，而不是在本文件 panic。

启动流程 `SetupTopProfiling` 先在短持锁区间克隆 reporter 与 sink，随后释放全局锁，再依次执行：

1. `BindKeyspaceName`；
2. `BindProcessCPUTimeUpdater`；
3. reporter `Start`；
4. sink `Start`；
5. 构造并注册 `StatementCollectorAdapter`；
6. 若 `SupportsRUCollector` 为真，构造并注册 `RUCollectorAdapter`；
7. `BindRUVersionProvider(Some(...))`；
8. `SetupAggregator`；
9. 再次持锁，将 collector 强引用写回 `PipelineState`。

关闭流程 `Close` 在锁内克隆 reporter/sink 并 `take` 掉 RU collector，然后在锁外依次注销 RU collector、关闭 sink、关闭 reporter、关闭 aggregator、解绑 RU provider。`statement_collector` 没有在本文件中显式注销；它仍保存在管线状态中，实际 collector 生命周期由 stmtstats 的注册/聚合器语义约束。

SQL 注册流程先拒绝 `None` digest，再在 `linkSQLTextWithDigest` 中按 UTF-8 原始字节截到最多 4096 字节，最后调用 reporter。注意 Rust 字符串切成字节不会 panic，但截断可能落在多字节字符中；生产 reporter 的 Rust 适配实现使用 `String::from_utf8_lossy`，因此边界处可能替换为 U+FFFD，这与 Go 字符串允许任意字节的表示能力不同。

Plan 注册流程拒绝 `None` digest；`linkPlanTextWithDigest` 依据 `String::len()` 的 UTF-8 字节数计算 `is_large`，再把完整字符串和标志交给 reporter。是否保存或压缩大计划由 reporter 决定。

挂接流程中，SQL digest 缺失或其 `String()` 为空时，`AttachAndRegisterSQLInfo` 和 `AttachSQLAndPlanInfo` 原样返回上下文。有效 SQL digest 会先写入 `ProfileContext`；线程标签只在 `TopSQLEnabled()` 时更新，但 SQL 元数据注册不受该开关阻止，因而仅启用 TopRU 时仍可注册 SQL/Plan。进程信息始终写入上下文，TLS 镜像仍受 TopSQL 开关控制。

## 数据与状态

全局 `PipelineState` 是进程级共享状态，由 `Mutex` 串行保护。大部分调用只在取得或替换 `Arc` 时持锁，启动、关闭、注册 pub/sub 及 reporter 回调均在锁外执行，避免在外部实现中长期占用全局锁。`InitializeTopProfiling` 会替换 reporter/sink、清空已保存的 collector，并只清除调用线程的 TLS 标签；它主要是依赖注入/测试边界，并不自动关闭被替换对象。

`CURRENT_THREAD_PROFILE_LABELS` 为每线程独立的 `HashMap<String, String>`。`set_goroutine_labels` 用 `ProfileContext::labels()` 的完整克隆覆盖旧值，不做增量合并；其他线程看不到该线程的标签。`InitializeTopProfiling` 同样不能清除其他线程的 TLS 副本。

digest 在 API 边界以 `parser::Digest` 引用传入，注册前复制成 `Vec<u8>`；SQL 文本最终以字节向量进入抽象 reporter，计划则保留为 `String`。`isInternal` 原样传递给 SQL 注册，`is_large` 由本文件计算。

## 依赖与调用关系

上游关系的直接证据包括：

- `pkg/session/runtime/scan_adapter_runtime.rs::TopSQLStart` 在 statement stats 开始后调用 `TopProfilingReporterAvailable`，并将 statement context 中的规范化 SQL/Plan 交给 `RegisterSQL`/`RegisterPlan`。
- `pkg/util/topsql/topsql_test.rs::mock_execute_sql` 依次调用 `AttachAndRegisterSQLInfo`、`AttachSQLAndPlanInfo`、`RegisterPlan`，模拟执行路径并形成 CPU 记录。
- `pkg/util/topsql/migration_aster_unit_test.rs` 直接覆盖 Initialize、Setup、PubSub、Close 及三个 attach 入口，证明这些公开 API 的迁移契约。

下游调用为：collector 提供 `ProfileContext`、digest/process label 构造及 CPU updater；stmtstats 提供 collector 注册、RU collector、版本提供者和 aggregator 生命周期；topsqlstate 决定是否把标签安装到线程；reporter crate 提供真实 remote reporter、pub/sub service 与 single-target sink；plancodec 为默认 reporter 解压规范化计划并压缩计划。

RustCodeGraph 将本文件标记为被 16 个文件使用，并能准确定位本文件 73 个符号；但本次环境中对带同名 Go/Rust 符号的精确 `callers`/`callees` 命令多次在 30 秒内无输出。调用关系因此只陈述 RustCodeGraph 文件级证据以及定向源码搜索实际命中的边，不根据 Cargo 依赖推断未观察到的调用。

## 错误处理与边界

- `reporter()`、`SetupTopProfiling` 和 `Close` 在缺少必需 reporter/sink 时 `expect` panic；调用者必须保证管线已由默认构造或 `InitializeTopProfiling` 建立。
- `pipeline()` 不传播 poisoned mutex 错误，而是保留并继续使用内层状态；这提高可恢复性，但不能保证导致中毒的操作已经完整提交。
- `RegisterSQL`/`RegisterPlan` 对 `None` digest 静默跳过；两个 digest attach API 对空 SQL digest 也直接返回。Plan digest 可以缺失，此时上下文仍挂接 SQL digest，并使用空 plan label。
- SQL 截断是纯字节边界，存在多字节 UTF-8 被截断后由真实 reporter 有损转换的兼容风险；若需要保持有效 UTF-8，必须先明确是否允许偏离 Go 的字节截断语义。
- `MockHighCPULoad` 会小写化 SQL，过滤包含 `mysql` 但不含 `global_variables` 的语句，再按前缀匹配。`load <= 0` 时目标时长为零，但循环条件是 `<=`，因此仍至少可能执行一次内部 busy loop；它只能用于测试/failpoint。
- `SetupTopProfilingForTest` 仅替换 reporter，不重建 sink，也不清理旧 collector；需要完整隔离时应使用 `InitializeTopProfiling` 或测试中的默认管线重置辅助。
- `RegisterPubSubServer` 的默认 trait 实现为空；只有具体 reporter 覆盖该能力时才产生注册效果。真实 `RemoteTopSQLReporter` 实现会创建并交付 `TopSqlPubSubService`。

## 并发与资源生命周期

所有进程级指针均以 `Arc<dyn ... + Send + Sync>` 共享；`PubSubServer` 只要求 `Send`，因为注册以可变借用同步发生。全局 `Mutex` 只保护管线结构，不替 reporter、sink 或 stmtstats 内部状态提供并发保证，这些实现必须自行同步。

正常生命周期是不重复的 `SetupTopProfiling` → 运行期注册/采集 → `Close`。代码没有显式的“已启动/已关闭”状态，也不拒绝重复 setup/close；是否幂等取决于下游实现。因此新增调用点不应自行重复启动或在仍有采集线程时关闭。

启动和关闭的耗时操作在全局锁外执行。`Close` 先从状态中取走 RU collector，阻止本文件再次注销同一对象；随后 sink 先于 reporter 关闭，最后停止 aggregator 并清除 RU provider，与 Go 顺序一致。测试 `setup_close_and_pubsub_preserve_go_pipeline_order` 验证 reporter/sink 的可观察顺序；`default_pipeline_restores_go_init_wiring` 验证默认管线无需显式 Initialize 即可注册真实 pub/sub 服务。

线程标签不跨线程传播；调用者必须在执行实际被采样工作的线程上执行 attach。Rust 测试说明 pprof-rs 不携带 Go goroutine labels，因此 CPU profile 测试在 collector 边界人工喂入记录；这证明元数据/生命周期接线，但不是 Go goroutine 标签机制的逐字等价实现。

## 与 Go 版本的对应关系

Rust 文件逐项对应同目录 `topsql.go`：两个大小常量、包级 reporter/sink、Setup/Close、SQL/Plan 注册、三个 attach 函数、high-CPU failpoint 和两个内部 link helper 均保留。Go `init()` 被 Rust 的 `LazyLock(default_pipeline)` 代替；Go 的接口类型断言被 `SupportsRUCollector` 和带默认实现的 trait 方法表达。

生命周期顺序保持一致：Setup 中先绑定并启动 reporter/sink，再注册 statement/RU collector、绑定版本提供者和启动 aggregator；Close 中先注销 RU、再关 sink/reporter/aggregator 并解绑版本提供者。`migration_aster_unit_test.rs` 将该顺序作为迁移基线验证。

存在以下实现层差异：

- Go 使用 `context.Context` 和 `pprof.SetGoroutineLabels`；Rust 使用值语义 `ProfileContext` 并将标签镜像到线程局部 `HashMap`。
- Go 通过接口断言发现 RU/pub-sub 可选能力；Rust 的 RU 能力需返回 `SupportsRUCollector = true`，pub/sub 由虚方法覆盖。
- Go 字符串是任意字节序列；Rust 输入是有效 UTF-8 `str/String`，但 SQL 仍按 Go 语义截字节，真实 reporter 再有损恢复字符串。
- Go failpoint 值直接断言为 bool；Rust `failpoint_enabled` 尝试解析布尔文本，无法解析时为 `false`，未启用 feature 时完全关闭。
- Rust 额外公开 `InitializeTopProfiling`、`TopProfilingReporterAvailable`、`current_thread_profile_labels`，用于显式接线、调用保护和测试可观测性；Go 原文件没有这些同名 API。

独立测试的对应关系明确标注在 `topsql_test.rs`：`test_top_sql_cpu_profile`、`test_top_sql_reporter`、`test_max_sql_and_plan_test`、`test_top_sql_pub_sub`、`test_pub_sub_when_reporter_is_stopped`、`test_top_ru_only_registers_sql_and_plan` 分别对照相应 Go 测试意图。Rust CPU 测试因运行时 profiling 能力差异采用 collector 边界注入，不应把它描述成完整复刻 Go 的 goroutine 采样链。

## 扩展指南

- 新增 reporter 能力时，先判断它是所有实现必需还是类似 RU/pub-sub 的可选能力；更新 `TopSQLReporter` 后必须同步真实 `RemoteTopSQLReporter` 适配、`TestCollector`/`MockReporter` 以及独立测试，避免默认空实现悄悄吞掉必需行为。
- 修改启动或关闭顺序时，应同时核对 Go `topsql.go`、stmtstats 生命周期契约和 `setup_close_and_pubsub_preserve_go_pipeline_order`；资源顺序变化可能造成最后一批统计丢失或后台任务访问已关闭 reporter。
- 修改 SQL/Plan 上限或编码策略时，应扩展 `sql_plan_registration_matches_go_byte_limits_and_nil_rules` 与 `test_max_sql_and_plan_test`，至少覆盖 None/空 digest、阈值等于/大于一字节、多字节 UTF-8 边界和 reporter 对 `is_large` 的处理。
- 新增 attach 标签时，应在 collector 的 `CtxWith*` 入口集中定义标签，更新迁移基线对 `ProfileContext` 与 TLS 快照的断言；测试逻辑继续放在 `topsql_test.rs` 或 `migration_aster_unit_test.rs`，不要内嵌到生产文件。
- 改动全局管线时要避免在持有 `PIPELINE` 锁时调用外部 trait 方法，并明确重复 Setup/Close、替换现有 reporter、跨线程 TLS 清理的行为。
- 接入新的运行时路径时，参考 `pkg/session/runtime/scan_adapter_runtime.rs::TopSQLStart`：先确认 reporter 可用和 digest 非空，再注册文本；不要把 Cargo 依赖存在误认为初始化已完成。
- failpoint/忙等逻辑只应用于测试验证。生产功能不得依赖 `MockHighCPULoad` 的耗时或循环次数；调整它时同步 Go 前缀和 mysql 系统表过滤规则。

## 验证依据

本说明读取并核对了以下直接材料：

- 生产源：`pkg/util/topsql/topsql.rs`（559 行，RustCodeGraph `node --file` 分段读取）。
- crate/模块边界：`pkg/util/topsql/Cargo.toml`、`pkg/util/topsql/lib.rs`，以及各消费 crate 的 Cargo 依赖定向搜索。
- Go 对照：`pkg/util/topsql/topsql.go`；测试对照：`pkg/util/topsql/topsql_test.go`。
- Rust 独立测试：`pkg/util/topsql/topsql_test.rs`、`pkg/util/topsql/migration_aster_unit_test.rs`。
- 真实调用点：`pkg/session/runtime/scan_adapter_runtime.rs::TopSQLStart`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/topsql` 确认目录对象；目标文件节点报告 73 个符号并列出 16 个使用文件。`query AttachAndRegisterSQLInfo --kind function --json` 区分了 Go 与 Rust 同名函数。精确 `callers`/`callees` 查询在本环境多次 30 秒超时且无输出，故未用其推导细粒度调用边，改用上述已读源码和定向 `rg` 交叉验证。

行为事实由测试意图支撑：迁移基线覆盖 Setup/Close/PubSub 顺序、默认管线、None digest、字节上限、attach 开关和 CPU 负载过滤；独立 `topsql_test.rs` 覆盖 CPU collector 边界、remote reporter、SQL/Plan 上限、pub/sub 停止以及仅启用 TopRU 时仍注册元数据。根据任务范围未运行 Cargo；最终仅执行文档的 11 章节结构校验和文档交付检查。
