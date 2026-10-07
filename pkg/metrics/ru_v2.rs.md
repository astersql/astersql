# `pkg/metrics/ru_v2.rs`

## 文件定位

本文件属于 `astersql-metrics` crate；crate 入口 `pkg/metrics/lib.rs` 以公开模块 `pub mod ru_v2` 暴露它，`pkg/metrics/Cargo.toml` 则声明该 crate 的库入口是 `lib.rs`，并直接依赖 `astersql-metrics-common` 与 `prometheus = "0.14"`。它是 RU v2（Request Unit 第二版）监控指标的定义与写入边界：负责构造 Prometheus counter、保存可供其他 crate 使用的句柄，并把已经算出的 RU 结果归入总量、SQL 类型和执行引擎维度；它不负责采集语句执行证据、计算 RU、注册 collector，也不访问 TiKV。

正常启动链为 `pkg/metrics/metrics.rs::InitMetrics` → `ru_v2::InitRUV2Metrics`，之后 `metrics.rs::RegisterMetrics` 注册 `RUV2Total`、`RUV2TTLTotal`、`RUV2BySQLType`、`RUV2ByEngine`、`RUV2Unit` 和 `RUV2Statements`。运行期主要写入者是 `pkg/executor/statement_ru_result.rs::StatementRUContextSink`；DDL 完成路径还会从 `pkg/ddl/job_worker.rs::transit_persisted_job_step` 写入 DDL RU。

## 核心职责

1. 用固定的 `tidb` namespace 和 `ruv2` subsystem 创建六个指标族：RU 总量、TTL RU、按 SQL 类型的 RU、按执行引擎的 RU、原始 statement unit，以及完整报告结果状态（`InitRUV2Metrics`）。
2. 预绑定高频标签组合，避免语句完成热路径反复执行标签查找：SQL 类型缓存覆盖 `select`、`insert`、`replace`、`update`、`delete`、`commit`、`analyze` 和兜底 `other`；引擎缓存覆盖 `tidb`、`tikv`、`tiflash`。
3. 把一条语句已计算好的 RU 同时累加到 `RUV2Total`、对应 SQL 类型 counter 和三个引擎 counter（`AddRUV2Results`）。
4. 在 DDL job 已持久化为 `Synced` 后，将正 RU 同时累加到总量、SQL 类型 `ddl` 和引擎 `tikv`（`AddDDLJobRU`）。
5. 提供 12 个 raw-unit 标签常量，使上游报告逻辑对 CPU、扫描字节、网络字节、写入键/字节等单位使用稳定字符串契约。

## 主要符号

- `LblRUV2Unit` 是 `RUV2Unit` 第三个 label 的名称 `unit`；其余 `LblRUV2Unit*` 常量是 unit label 的合法业务值，包括 `cpu_work`、`scan_bytes`、`net_bytes`、`cross_az_net_bytes`、`frontend_compile_bytes`、`hash_state_rows`、`join_output_rows`、`write_statement`、`operator_num`、`write_keys` 和 `write_bytes`。
- `RUV2Total` / `RUV2TTLTotal` 是无 label counter，分别表示全部 RU v2 消耗和其中来自 TTL 用户表扫描、删除及提交的子集；TTL 指标的 help 明确说明其值也包含在总量内。
- `RUV2BySQLType` 是 label 为 `LblSQLType`（实际为 `sql_type`）的 counter vector；`RUV2BySQLTypeDDL` 是其中 `ddl` 标签的预绑定句柄。私有的 `ruv2Select` 等变量缓存其他语句类型句柄。
- `RUV2ByEngine` 是 label 为 `LblEngine`（实际为 `engine`）的 counter vector；`RUV2ByEngineTiKV` 是公开的 `tikv` 句柄，私有的 `ruv2TiDB`、`ruv2TiFlash` 缓存另两个引擎句柄。
- `RUV2Unit` 是 labels `[engine, opclass, unit]` 的 counter vector；它只在本文件中创建，实际逐项写入发生在 `StatementRUContextSink::unit`。
- `RUV2Statements` 是 labels `[status, reason]` 的 counter vector；实际成功/跳过/失败结果由执行器报告路径写入。
- `counter(name, help)` 是无 label counter 工厂。`counter_vec(name, help)` 构造只带 `LblType` 的 vector，但当前文件和仓库引用搜索均未发现调用者，属于当前未使用的私有辅助函数，不能视为运行链的一部分。
- `InitRUV2Metrics()` 初始化所有公开 collector 和私有热点句柄。
- `AddRUV2Results(tikv_ru, tidb_ru, tiflash_ru, total_ru, sql_type)` 记录普通语句的聚合结果。
- `AddDDLJobRU(ru)` 记录已经同步完成的 DDL job RU；非正值直接忽略。

## 执行流程

初始化时，`InitRUV2Metrics` 先创建 `ttl_ru_total` 与 `ru_total`；再创建 `ru_by_sql_type_total{sql_type=...}`，从同一个 vector 取得 `ddl` 和八个热点 SQL 类型句柄；随后创建 `ru_by_engine_total{engine=...}` 并取得 `tidb`、`tiflash`、`tikv` 句柄；最后创建 `unit_total{engine,opclass,unit}` 和 `statements_total{status,reason}`。`pkg/metrics/metrics.rs::InitMetrics` 在 `INIT_METRICS_ONCE` 中调用它，因此生产初始化只执行一次。

语句完成时，`pkg/executor/statement_ru_result.rs::StatementRUContextSink::results` 先持有 `PACKAGE_INIT_LOCK`；若是 TTL job，先将 `snapshot.result.total_ru` 加到 `RUV2TTLTotal`，再调用 `AddRUV2Results`。后者按精确、区分大小写的字符串选择 SQL counter，未知值归入 `other`，然后依次把 `total_ru` 加到总量和 SQL 类型，把 `tikv_ru`、`tidb_ru`、`tiflash_ru` 加到各自引擎 counter。`StatementRUContextSink::unit` 与 `statement` 分别使用本文件构造的两个 vector 写入原始单位和终态分类。

DDL 路径中，`pkg/ddl/job_worker.rs::transit_persisted_job_step` 只有在数据库事务已经 `commit`、job 状态为 `Synced` 且 `job.ru > 0.0` 后才调用 `AddDDLJobRU`；该函数再次防御 `ru <= 0.0`，正值则同时写入总量、`ddl` SQL 类型和 `tikv` 引擎。这说明 DDL RU 不经过 `AddRUV2Results`，且当前统一归属 TiKV。

## 数据与状态

所有 collector 都存放在进程级 `static mut Option<...>` 中，初值为 `None`。`InitRUV2Metrics` 将完整的新句柄集合写入这些槽位；公开 vector 与预绑定 counter 指向同一指标族，因此从任一预绑定句柄增加数值，随后从 vector 以相同 label 读取会看到同一累计值。Prometheus counter 只累计，文件中没有 reset、删除 label 或清理资源的逻辑。

正常初始化和注册由 `metrics.rs` 管理：`INIT_METRICS_ONCE` 防止生产路径重复替换 collector，`PACKAGE_INIT_LOCK` 用于串行化包级可变静态量的赋值/克隆访问。测试会在隔离进程内直接调用 `InitRUV2Metrics`，以免全局 collector 值污染并行用例（见 `pkg/metrics/metrics_internal_test.rs`）。

`AddRUV2Results` 不验证 `total_ru` 是否等于三个引擎值之和，也不修改输入；守恒与结果冻结属于执行器 RU 汇总层的职责。它也不跳过零值。`AddDDLJobRU` 则明确跳过零值和负值。

## 依赖与调用关系

- 上游初始化：`pkg/metrics/metrics.rs::InitMetrics` 调用 `InitRUV2Metrics`；同文件 `RegisterMetrics` 注册本模块六个公开 collector。`pkg/metrics/lib.rs` 暴露模块。
- 上游语句结果：RustCodeGraph 将 `AddRUV2Results` 的生产调用者定位到 `pkg/executor/statement_ru_result.rs::StatementRUContextSink::results`。该 sink 同时读取 `RUV2TTLTotal`、`RUV2Unit` 和 `RUV2Statements`。
- 上游 DDL：RustCodeGraph 将 `AddDDLJobRU` 的生产调用者定位到 `pkg/ddl/job_worker.rs::transit_persisted_job_step`；调用位于成功提交且 job 已 `Synced` 之后。
- 测试调用者：`pkg/metrics/metrics_internal_test.rs` 直接覆盖初始化、普通结果、DDL 结果、指标名和 label 维度；`pkg/executor/statement_ru_plan_walk_test.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs`、`pkg/session/tests/paging_rpc.rs` 等跨模块测试显式初始化这些全局指标后验证真实报告链。
- 下游构造：`counter`、`InitRUV2Metrics` 调用 `astersql-metrics-common` 的 `NewCounter` / `NewCounterVec` 兼容 API，并通过 `bindinfo::compat_prometheus` trait 取得 Go 风格的 `Add`、`WithLabelValues` 方法；底层 collector 类型来自 `prometheus` crate。
- 常量依赖：`LblSQLType`、`LblSQLTypeDDL`、`LblEngine`、`LblEngineTiKV`、`LblEngineTiFlash` 由 crate 根再导出的 session 指标常量提供，确保跨指标模块 label 契约一致。

## 错误处理与边界

本文件没有返回 `Result`。指标构造经兼容层完成；运行期写入以 `Option::expect` 检查初始化前置条件，因此在 `InitRUV2Metrics` 之前调用 `AddRUV2Results` 或正值的 `AddDDLJobRU` 会 panic，错误文本分别指出 RU v2、SQL 类型或引擎指标未初始化。调用者若直接读取 `RUV2TTLTotal`、`RUV2Unit`、`RUV2Statements` 也使用相同的显式前置条件。

SQL 类型匹配只有七个已列出的值走专用 counter，任何其他字符串（包括新的类型、空字符串或大小写不同的字符串）都归入 `other`，不会报错或动态采用原字符串。引擎值不是由本函数动态选择，而是四个数值参数与固定句柄的位置绑定。

`AddDDLJobRU` 对 `ru <= 0.0` 直接返回，因此该分支甚至不要求指标已经初始化；对 `NaN`，比较结果为 false，函数会继续向 counter 传值，最终行为取决于 Prometheus counter 的输入约束，本文件没有单独验证。普通结果函数同样没有本地的有限性、非负性或守恒校验，调用方必须保证输入符合 counter 与计费语义。

## 并发与资源生命周期

collector 是进程生命周期对象，不持有数据库连接、事务、任务或通道；本文件也不启动线程。Prometheus `Counter`/`CounterVec` 句柄用于并发累加，但包级槽位本身是 `static mut`，因此初始化和替换需要外部同步。生产路径由 `metrics.rs::INIT_METRICS_ONCE` 保证只初始化一次；语句结果 sink 在克隆或写入这些槽位时持有 `PACKAGE_INIT_LOCK`。直接公开静态量和公开 `InitRUV2Metrics` 意味着其他调用者若绕开这套协议重复初始化或无同步访问，会承担数据竞争/替换句柄及读到 `None` 的风险。

预绑定 counter 与 vector clone 都是轻量句柄关系，不建立独立采集器；collector 的注册生命周期由 `RegisterMetrics` 和 Prometheus registry 管理。测试用 `Registry::new()` 注册 clone 来隔离验证，且通过子进程隔离涉及全局默认 registry/可变静态量的用例。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/metrics/ru_v2.go`。Rust 保留了 Go 的全部 12 个 unit 常量、六个指标族、namespace/subsystem、metric name、help 文本、label 次序、SQL 类型集合、未知 SQL 类型归入 `other` 的规则，以及 `AddRUV2Results` 的五路累加顺序。`pkg/metrics/metrics_internal_test.go::TestRUV2MetricDefinitions` 与 Rust 的 `pkg/metrics/metrics_internal_test.rs::{go_merge_6_ru_results_use_go_label_and_engine_contract, ru_metric_definitions_preserve_labels_values_and_registration, go_merge_6_ru_metric_definitions}` 共同提供名称、标签和值的对照证据。

表示层差异是 Go 全局变量直接持有 collector，Rust 以 `Option` 表达初始化前状态并通过 `expect` 暴露误用；Go 由 package `init` 生命周期保障，Rust 的总入口以 `Once`、`AtomicBool`、错误槽和互斥锁补足并发初始化边界。Rust 文件还提供 `AddDDLJobRU`，而所读的同路径 Go 文件没有同名辅助函数；Rust 调用点显示它将已同步 DDL job 的 RU 计入总量、`ddl` 与 `tikv`，相关 Rust 独立测试验证三者同步增加。该差异应视为 Rust 当前已接线行为，而不是从 Go 同路径文件推导出的语义。

## 扩展指南

- 新增 raw unit 时，应在本文件增加 `LblRUV2Unit*` 常量，并同步更新执行器将单位投影到 `RUV2Unit` 的枚举/匹配逻辑；至少扩展 `pkg/metrics/metrics_internal_test.rs` 的 label 断言，并更新对应 Go 常量与 Go 测试，避免 dashboard label 漂移。
- 新增 SQL 类型专用归因时，应在 `InitRUV2Metrics` 预绑定 counter，并在 `AddRUV2Results` 的 match 中显式选择；否则新字符串会静默落入 `other`。同步修改 `pkg/metrics/ru_v2.go` 和两侧 metrics internal tests，检查高基数风险，不要把任意 SQL 文本作为 label。
- 新增引擎时，需要同时扩展函数参数/结果结构、初始化缓存、执行器 `StatementRUEngineResult` 汇总以及 Go 对照；只给 `RUV2ByEngine` 增加动态 label 不会让 `AddRUV2Results` 自动写入它。
- 修改 metric name、help 或 label 名称/顺序属于监控兼容性变更，会影响 PromQL、Grafana 与告警；应同步检查 `pkg/metrics/grafana/`、`pkg/metrics/nextgengrafana/` 和 `pkg/metrics/alertmanager/` 中的引用，并保留现有名称的迁移策略。
- 若调整初始化模型，优先消除或封装 `static mut`，但必须同时审查 `metrics.rs::{InitMetrics,RegisterMetrics,PACKAGE_INIT_LOCK}` 与所有跨 crate 直接静态访问。Rust 单元测试应继续放在独立的 `*_test.rs` 文件，不应内嵌到 `ru_v2.rs`。
- `counter_vec` 当前未使用；删除或启用前先确认它的 `LblType` 单标签形状确实符合新指标，不能误用于现有 `sql_type`、`engine` 或多维指标。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；`node --file pkg/metrics/ru_v2.rs` 完整读取 213 行源码。
- RustCodeGraph 调用证据：`InitRUV2Metrics` 的 Rust 使用点包括 `metrics.rs::InitMetrics` 及执行器/会话测试；`AddRUV2Results` 的调用边指向 `statement_ru_result.rs::StatementRUContextSink::results`；`AddDDLJobRU` 的调用边指向 `job_worker.rs::transit_persisted_job_step`。由于常见符号的宽泛 `explore` 结果噪声较高，最终调用集合又以限定 `*.rs` 的精确引用搜索复核。
- 已读生产文件：`pkg/metrics/ru_v2.rs`、`pkg/metrics/lib.rs`、`pkg/metrics/metrics.rs`、`pkg/executor/statement_ru_result.rs`、`pkg/ddl/job_worker.rs`。
- 已读边界与 Go 对照：`pkg/metrics/Cargo.toml`、`pkg/metrics/ru_v2.go`、`pkg/metrics/metrics.go`、`pkg/executor/statement_ru_result.go`。
- 已读独立测试：`pkg/metrics/metrics_internal_test.rs`；并通过引用搜索定位 `pkg/executor/statement_ru_plan_walk_test.rs`、`pkg/session/runtime/scan_adapter_runtime_test.rs`、`pkg/session/tests/paging_rpc.rs` 等调用链测试，以及 Go 的 `pkg/metrics/metrics_internal_test.go`、`pkg/executor/statement_ru_result_test.go`、`pkg/executor/statement_ru_reporting_test.go`。
- 人工复核结论：本文件存在的原因是集中维护 RU v2 的 Prometheus schema 与低开销写入句柄；实际 RU 计算在执行器/DDL 上游完成；安全扩展必须同步 label 契约、预绑定分派、Go 对照和独立测试。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构验证命令及结果记录在最终报告中。
