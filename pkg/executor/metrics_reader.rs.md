# `pkg/executor/metrics_reader.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指定，模块由 [`lib.rs`](lib.rs) 中的 `pub mod metrics_reader;` 公开。它承载 `METRICS_SCHEMA` 普通指标表、`metrics_summary` 和 `metrics_summary_by_label` 三类虚拟表读取逻辑：前者把 Prometheus range query 的矩阵结果转换为 SQL 行，后两者通过受限 SQL 再聚合指标表。

当前 Rust 仓库事实需要与 Go 生产链区分：RustCodeGraph 能定位本文件的类型和测试实例化边，但仓库搜索未发现测试之外的 Rust 调用者或 `MetricsReaderBackend` 生产实现。因此这里是已实现、可独立验证但尚未接入 Rust 执行器构造主链的模块。Go 生产入口仍位于 [`builder.go`](builder.go) 的 `executorBuilder.buildMemTable`，它分别构造 Go 的三个 retriever。

## 核心职责

- `MetricRetriever` 负责一次性读取单张指标表：解析表定义和时间范围，按请求或默认分位数生成 PromQL，有限重试查询 Prometheus，再按表定义的列顺序生成 `Datum` 行。
- `MetricsSummaryRetriever` 要求 `PROCESS` 权限，枚举并过滤指标表，为每张表生成 `sum/avg/min/max`（以及可选 `quantile`）聚合 SQL，返回每表/分位数一行摘要。
- `MetricsSummaryByLabelRetriever` 同样要求 `PROCESS` 权限，但按表 labels 和可选 `quantile` 分组；它把首个 `instance` label 单列输出，其余 labels 拼为字符串。
- `MetricsReaderBackend` 把 InfoSync、Prometheus 客户端、会话变量、权限、告警、failpoint/mock 和受限 SQL 隔离在边界之后。该 trait 没有默认成功实现，调用方必须显式提供所有外部能力。

## 主要符号

- `promReadTimeout: Duration`：固定为 10 秒，交给 `MetricsReaderBackend::query_context` 构造 Prometheus 查询上下文。
- `Datum`、`RestrictedRow`：本模块的轻量 SQL 值与受限 SQL 行模型。`RestrictedRow::string`/`float64` 对列类型和下标采用强约束，违约会 panic。
- `SamplePair`、`SampleStream`、`PrometheusValue`：表示 Prometheus 采样点、带标签序列和查询结果；当前只展开 `PrometheusValue::Matrix`。
- `MetricTableDef`：记录输出 labels、默认 `quantile` 和注释。`MetricTableExtractor` 保存普通表的 skip、分位数、时间窗和 label 条件；`MetricSummaryTableExtractor` 保存摘要表的 skip、表名过滤集合和分位数。
- `PromQLQueryRange`、`QueryTimeRange`：前者是 Prometheus 查询的起止时间与秒级步长，后者保存拼入受限 SQL 的时间条件文本。
- `PrometheusAddress<E>`：区分取得地址、明确未配置和可重试瞬时错误；`PrometheusQueryError<E>` 区分 Prometheus API 业务错误和其它后端错误。
- `MetricRetriever::{retrieve, queryMetric, getQueryRange, genRows, genRecord}`：单表读取的入口及四个内部阶段。
- `MetricsSummaryRetriever::retrieve`、`MetricsSummaryByLabelRetriever::retrieve`：两个摘要读取入口。
- `metricEnabled`：空集合代表全部启用，否则按精确表名过滤。
- `MockMetricsPromDataKey`：对应 Go 测试上下文键的占位类型；Rust 实际 mock 注入由 backend 方法承担。

## 执行流程

`MetricRetriever::retrieve` 首先在 `retrieved` 已置位或 `extractor.skip_request` 为真时返回空结果；skip 分支不会改变 `retrieved`。真正执行时先置位，随后允许 `mock_table_data` 直接返回。否则加载 `MetricTableDef`，由 `getQueryRange` 组合 extractor 的起止时间和 backend 的 schema 步长，并选择 extractor 分位数；未指定时使用表定义默认值。每个分位数调用 `queryMetric`，成功结果经 `genRows` 展开后追加到总行集。

`queryMetric` 先检查 `mock_prometheus_data`。真实路径最多尝试五次读取 Prometheus 地址：`Address` 成功，`NotSet` 立即结束重试并报错，`Error` 每次等待 100 ms 后重试。获得地址后创建客户端，以 `promReadTimeout` 创建查询上下文，使用表定义、range duration、label 条件和分位数生成 PromQL；`query_range` 也最多尝试五次，每次失败等待 100 ms，五次均失败时返回最后一个错误。

`genRows` 只接受 Matrix；每条 `SampleStream` 的每个 `SamplePair` 生成一行。`genRecord` 的列序固定为时间、表定义中的 labels、可选 quantile、value。序列缺少或给出空 label 时，使用小写 label 对应的谓词条件合成回填值；采样值为 NaN 时输出 `Datum::Null`。

两个摘要入口都先检查 `PROCESS` 权限，再处理 `retrieved/skip_request`，随后排序表名以保证稳定顺序并用 `metricEnabled` 过滤。表定义加载失败不会终止整体查询，而是追加 warning 并跳过该表；受限 SQL 失败则立即返回包含完整 SQL 的上下文错误。普通摘要按 quantile 决定 SQL 列和分组；按 label 摘要再把 labels 加入 select/group/order，并将 `store`/`store_id` 值规范化为 `store_id:<值>`。

## 数据与状态

三个 retriever 都持有可变布尔值 `retrieved`，它是单实例“一次拉取”的状态门闩，而不是缓存：首次真实执行前即置为 `true`，后续调用返回空行。`MetricRetriever.tblDef` 初始为 `None`，在生成 PromQL 或行之前由 `retrieve` 填充；私有方法用 `expect("table definition is loaded")` 维护这一调用顺序不变量。

普通指标数据是按值拥有的 `Vec<Vec<Datum>>`。Matrix 展开会按输入 stream 和 sample 的原顺序输出；多个 quantile 则按 extractor 给定顺序依次追加。摘要查询先对表名排序，保证跨 `HashMap`/backend 枚举顺序的稳定性。`MetricSummaryTableExtractor.metrics_names` 为空表示不筛选，非空时只接受精确命中的名称。

SQL 行形状是硬契约：普通摘要期望前四列为 sum、avg、min、max，直方图时末列为 quantile；按 label 摘要期望前四列仍为聚合值，之后依次是 labels，quantile 若存在则位于末列。backend 返回的列类型或数量不符会在 `RestrictedRow` 访问时 panic。

## 依赖与调用关系

上游方面，`lib.rs` 公开模块，并在 `#[cfg(test)]` 下通过 `#[path = "metrics_reader_test.rs"]` 挂载独立测试。RustCodeGraph 的 `MetricRetriever` 节点显示调用/构造边仅来自 `metrics_reader_test.rs` 的导入、`retriever` 辅助函数和 `metric_retriever_retries_and_matches_go_record_shape`；仓库级搜索也只发现测试侧 Rust 引用。因此不能把 Go 的生产接线描述成 Rust 已接线。

下游方面，本文件只直接使用标准库的 `HashMap`、`HashSet`、`Display`、`Duration` 和 `SystemTime`。真实系统依赖全部经 `MetricsReaderBackend` 反转注入：表定义与 schema 参数、label 条件和 PromQL 生成、Prometheus 地址/客户端/超时查询、权限、warning、受限 SQL 和测试 mock。虽然 `pkg/executor/Cargo.toml` 声明了 infoschema、domain/infosync、kv、planner 等 executor 依赖，本文件本身没有直接 import 它们，当前也没有具体 backend 将它们接入。

Go 主链的直接证据是 `builder.go:2660-2770`：`buildMemTable` 对 `METRICS_SCHEMA` 构造 `MetricRetriever`，对 information_schema 的 `TableMetricSummary` 和 `TableMetricSummaryByLabel` 构造两个摘要 retriever。相应 Go 方法在 [`metrics_reader.go`](metrics_reader.go) 中被 `MemTableReaderExec` 使用。

## 错误处理与边界

- `MetricRetriever::retrieve` 保留 Prometheus API 错误的 `message/detail`，其它错误统一包装为 `query metric error: ...`；创建客户端、取地址和查询失败都通过 backend 错误类型返回。
- 明确未配置 Prometheus 地址不重试；瞬时地址错误最多五次。查询错误不分类，统一最多五次后返回最后一次错误。
- 非 Matrix 的 Prometheus 值不是错误，而是空结果；NaN 不是错误，而是 SQL `NULL`。
- 摘要权限检查先于 skip/retrieved 检查，因此即使请求会被跳过，无 `PROCESS` 权限仍返回拒绝错误；独立 Rust 测试明确固定了这个顺序。
- 摘要中单个表定义缺失只产生 warning，允许其它表继续；受限 SQL 执行失败则终止并带上 SQL 文本。
- SQL 字符串中的表名、条件和列名由 backend/规划器边界提供，本模块不做额外转义或语法校验；扩展时必须保持这些来源可信且与 Go 行为一致。
- `tblDef.expect`、`RestrictedRow` 的下标/类型访问和“五次失败必有最后错误”的 `expect` 都是内部不变量检查，不是可恢复输入错误。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道或共享全局可变状态。retriever 通过 `&mut self` 串行更新 `retrieved`/`tblDef`，并不声明为可并发复用；并发安全责任留给拥有它的执行器。

Prometheus 客户端与查询上下文是 `queryMetric` 的局部值：每个 quantile 都重新取地址、建客户端和上下文。超时值为 10 秒，但取消/释放语义由 backend 的 `QueryContext` 类型负责；trait 没有显式 close/cancel 方法。两类重试均在当前线程同步调用 `backend.sleep(100 ms)`，最坏会增加阻塞延迟。受限 SQL 行和 Matrix 数据按拥有权传入并在函数返回后释放，没有跨调用缓存。

## 与 Go 版本的对应关系

[`metrics_reader.go`](metrics_reader.go) 是逐函数对照来源。Rust 的三个 retriever、`promReadTimeout`、`queryMetric/getQueryRange/genRows/genRecord`、默认 quantile、五次地址/查询重试、100 ms 等待、NaN 转 NULL、摘要权限与 SQL 形状均保持 Go 意图。Rust 用 `MetricsReaderBackend` 代替 Go 对 InfoSync、sessionctx、Prometheus API、failpoint 和 restricted SQL 的直接调用，并用自有 `Datum`/Prometheus 值模型代替 Go 类型。

仍有重要集成差异：Go 类型嵌入 `dummyCloser`、持有 `TableInfo`，并由 `builder.go` 接入 `MemTableReaderExec`；Rust 类型只保存 `table_name` 等最小状态，当前没有生产 backend 或 builder 调用。Go 在摘要查询前通过 `kv.WithInternalSourceType(..., InternalTxnOthers)` 标记内部事务来源；Rust trait/API 中没有等价显式动作，若未来接线需由 backend 或调用方证明该语义被保留。Go 的 Prometheus context 使用 `defer cancel()`，Rust 只把 context 构造委托给 backend。

[`metrics_reader_test.go`](metrics_reader_test.go) 当前只包含与语句标签相关的 `TestStmtLabel`，没有直接覆盖本文件对应 Go retriever；本任务的具体对齐证据主要来自 Go 源文件和 Rust 独立测试，而不是该 Go 测试文件。

## 扩展指南

- 接入 Rust 生产链时，应在 executor builder/memtable reader 层构造三个 retriever，并实现生产 `MetricsReaderBackend`；同时补齐 Go 已有的内部事务来源标记、查询 context 取消/释放和权限错误语义，不能只用测试 backend 代替。
- 新增普通指标列或改变行形状时，修改 `MetricTableDef`、`genRecord` 及其列序，并在 [`metrics_reader_test.rs`](metrics_reader_test.rs) 增加独立测试；不要把测试嵌入生产源文件。
- 改动重试、超时或错误分类时，集中修改 `queryMetric` 与 `promReadTimeout`，并覆盖地址 `NotSet`、瞬时错误耗尽、API `message/detail` 和查询失败耗尽。需关注同步 sleep 带来的延迟与每 quantile 重建客户端的性能成本。
- 扩展摘要维度或聚合列时，同步修改两类摘要的 SQL 生成和 `RestrictedRow` 解码下标；特别保持 `instance` 首列约定、`store/store_id` 前缀、quantile 末列约定及确定性排序。
- 修改权限/skip 行为时保持 `PROCESS` 检查顺序，除非 Go 版本也改变且有兼容性依据。新增表过滤规则应从 `metricEnabled` 和 `MetricSummaryTableExtractor.metrics_names` 入手。
- 所有 Rust 行为调整都应同步独立测试 `metrics_reader_test.rs`，并逐项与 `metrics_reader.go` 核对，避免把尚未接线的能力误报为生产可用。

## 验证依据

- 源与模块：[`metrics_reader.rs`](metrics_reader.rs)；[`lib.rs`](lib.rs) 的 `pub mod metrics_reader` 及独立 `metrics_reader_test` 声明；[`Cargo.toml`](Cargo.toml) 的 crate 名、lib 路径、feature 与依赖边界。`pkg/executor` 下未发现 `doc.go`，因此无可读取的包级 Go 契约文件。
- Go 对照与入口：[`metrics_reader.go`](metrics_reader.go) 的三个 retriever 及其辅助方法；[`builder.go`](builder.go) 的 `buildMemTable` 构造分支；[`metrics_reader_test.go`](metrics_reader_test.go) 经核对不含对应 retriever 测试。
- Rust 测试：[`metrics_reader_test.rs`](metrics_reader_test.rs) 覆盖 mock 一次性读取/skip、地址与 query 各两次失败后成功、100 ms sleep 次数、10 秒 timeout、label 条件回填、NaN、摘要权限先行、缺表 warning、quantile SQL、按 label 分组和 `store_id:` 规范化。
- RustCodeGraph：`status` 报告索引包含 7,032 个 Rust 文件；`query MetricRetriever`、`query MetricsSummaryRetriever`、`query MetricsSummaryByLabelRetriever`、`query metricEnabled` 均定位到本文件。`node MetricRetriever` 给出的 Rust trail 只有 `metrics_reader_test.rs` 的导入/实例化；限定符 `callers/callees` 查询未返回额外生产调用边。由于 `files --filter pkg/executor/metrics_reader` 未命中，源码细节按技能规则改由直接读取和 `rg` 复核。
- 结构校验使用任务指定命令，要求目标文件存在且恰有十一个固定二级标题；本任务是纯文档分析，未运行 Cargo。
