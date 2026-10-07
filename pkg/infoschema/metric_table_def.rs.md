# `pkg/infoschema/metric_table_def.rs`

## 文件定位

[`metric_table_def.rs`](metric_table_def.rs) 属于 `astersql-infoschema` crate；crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod metric_table_def` 注册该模块，并重新导出 `MetricTableMap`。[`Cargo.toml`](Cargo.toml) 将该 crate 的库入口指定为 `lib.rs`，没有为本文件设置条件 feature。

这个文件是 METRICS_SCHEMA 的静态指标目录，不是指标采集器：它只声明“SQL 指标表名 → PromQL 模板及元数据”的映射，不连接 Prometheus、不展开模板，也不读取物理表。定义的解释、虚拟表元数据生成和模板展开位于 [`metrics_schema.rs`](metrics_schema.rs)。

## 核心职责

- 用公开静态量 `MetricTableMap` 集中保存 638 张指标表的定义。键是小写 SQL 表名，值是 `MetricTableDef`。
- 为每张表提供 `PromQL`，并按需提供可过滤的 `Labels`、默认 `Quantile` 和 `Comment`；未显式填写的字段通过 `MetricTableDef::EMPTY` 取得 Go 零值语义。
- 以注释将目录粗分为 TiDB、TiFlash、PD、PD 内嵌 etcd、资源管控和 TiKV 六个区域。这些分组只帮助维护，不形成运行时层级。
- 通过 `$QUANTILE`、`$LABEL_CONDITIONS`、`$RANGE_DURATION` 三类占位符把静态定义与运行时查询参数连接起来；替换动作由 `MetricTableDef::GenPromQL` 完成。

## 主要符号

`pub static MetricTableMap: LazyLock<HashMap<&'static str, MetricTableDef>>` 是本文件唯一的生产符号和公开 API。它的键和值都只借用静态字符串，因此映射构造完成后不需要复制或管理字符串所有权。

初始化闭包调用 `HashMap::from` 接收一个由 638 个 `(表名, MetricTableDef)` 元组组成的数组。每个值可设置：

- `PromQL`：实际查询模板，是每项必须有意义的字段；
- `Labels`：允许生成标签条件及指标表列的标签顺序；
- `Quantile`：大于零时会生成 `quantile` 列，也作为未显式指定查询分位数时的默认值；
- `Comment`：表说明；
- `..MetricTableDef::EMPTY`：把未指定字段补为 `""`、`&[]` 或 `0.0`。

文件级 `#![allow(dead_code, non_snake_case, non_upper_case_globals)]` 保留了 Go 移植命名，并允许这个大型目录在接线尚未完全覆盖时编译。

## 执行流程

1. `MetricTableMap` 第一次被解引用或调用 `iter`、`get`、`contains_key` 时，`LazyLock` 执行闭包并分配、填充 `HashMap`；后续访问复用同一映射。
2. `metrics_schema::metric_schema_db` 遍历映射，把每项转换成 `TableInfo`。它先按表名排序，再从 `MetricSchemaDBID + 1` 连续分配稳定表 ID，并通过 `MetricTableDef::genColumnInfos` 生成 `time`、label、可选 `quantile`、`value` 列。
3. `pkg/session/runtime/system_query.rs` 的 `build_virtual_system_catalog` 调用 `metric_schema_db`，把所得表登记到会话的 `metrics_schema` 虚拟系统目录。这是当前能从仓库代码验证的应用接入链。
4. 查表侧，`metrics_schema::IsMetricTable` 使用 `contains_key` 判断小写表名，`GetMetricTableDef` 使用 `get` 返回静态定义；`GenPromQL` 才会把本文件中的占位符替换为分位数、标签条件和秒级范围。
5. 执行器的 [`pkg/executor/metrics_reader.rs`](../executor/metrics_reader.rs) 通过 `MetricsReaderBackend::metric_table_def` 抽象获取等价定义并生成 Prometheus 查询；仓库中没有发现把该 trait 方法直接接到 `GetMetricTableDef` 的生产实现，因此不能把这条抽象边写成已经完成的直接接线。

## 数据与状态

目录是进程级、只初始化一次的只读状态。`HashMap` 自身迭代顺序不稳定，所以需要稳定顺序的消费者必须排序；`metric_schema_db` 已明确这样做。键查找区分大小写，公开帮助函数也要求调用者传入已转小写的表名。

当前 Rust 目录包含 638 个键，其中 91 项显式设置 `Quantile`、628 项显式设置 `Labels`、493 项显式设置 `Comment`。这些计数描述当前快照，不是 API 常量；增加或删除定义时会变化。

关键不变量由 [`metrics_schema_test.rs`](metrics_schema_test.rs) 固化：表名必须小写；含 `$QUANTILE` 或 `histogram_quantile` 的模板必须有正分位数，其他模板的分位数必须为零；需要动态标签条件的模板必须声明 labels；出现 `instance` 时它必须是第一个 label；`by (` 分组中的 labels 必须能在模板中找到。

## 依赖与调用关系

本文件的直接代码依赖只有标准库 `HashMap`、`LazyLock`，以及同 crate 的 `metrics_schema::MetricTableDef`。`Cargo.toml` 的大量包级依赖属于整个 infoschema crate，不能据此推断本文件直接调用了外部 crate。

RustCodeGraph 对 `MetricTableMap` 的节点记录显示直接使用文件为 `metrics_schema.rs`、`lib.rs` 和测试 `go_merge_45_test.rs`。仓库文本搜索还验证了如下关系：

- `lib.rs` 公开重导出映射；
- `metrics_schema.rs` 的 `IsMetricTable`、`GetMetricTableDef`、`metric_schema_db` 读取映射；
- `pkg/session/runtime/system_query.rs::build_virtual_system_catalog` 通过 `metric_schema_db` 间接消费目录；
- `metrics_schema_test.rs` 遍历整个目录并检查结构不变量；
- `go_merge_45_test.rs::go_merge_45_metrics_and_storage_class_metadata` 检查特定 Go 合并项仍存在。

## 错误处理与边界

本文件没有 `Result`、显式错误分支、网络 I/O 或运行时输入。未知表名的错误由 `GetMetricTableDef` 在相邻模块生成，文案为 `can not find metric table: <name>`。

这里不会验证 PromQL 的完整语法，也不会转义运行时 label 值。Rust 测试只用括号、花括号和方括号平衡检查近似 Go 的 PromQL 解析，因此“结构测试通过”不等于 Prometheus 一定接受表达式；Go 测试使用 `promql/parser.ParseExpr`，是更强的语法证据。

维护时还要注意 `HashMap::from` 对重复键不会报告业务错误，后出现的值会覆盖前值。新增定义必须用目录级检查或测试排除重复键，并遵守 labels、分位数和占位符之间的不变量。

## 并发与资源生命周期

`LazyLock` 保证并发首次访问时初始化闭包只执行一次；初始化完成后所有线程共享同一 `HashMap`。公开接口只提供共享读取，没有可变引用、锁持有区间、后台任务、通道、事务或显式清理动作。

主要资源成本发生在首次访问：一次哈希表分配和 638 项插入。`MetricTableDef` 及其字符串/切片都是静态借用，映射生命周期等同于进程；后续查找是普通哈希查找。若初始化闭包发生 panic，`LazyLock` 的失败行为由标准库控制，本文件没有恢复策略；正常路径中闭包只有静态数据建表。

## 与 Go 版本的对应关系

直接对照文件是 [`metric_table_def.go`](metric_table_def.go)。Go 用包级 `map[string]MetricTableDef`，Rust 用 `LazyLock<HashMap<&'static str, MetricTableDef>>` 延迟完成等价初始化；Go 结构体字面量省略字段时自然得到零值，Rust 用 `..MetricTableDef::EMPTY` 显式复现。

当前 Go 目录有 642 个键，Rust 有 638 个键。键集合差异已通过排序比较确认：Rust 移除了 `go_gc_count`、`go_gc_cpu_usage`、`go_gc_duration`、`go_heap_mem_usage`、`go_threads`、`goroutines_count` 六个 Go runtime 专用表，增加 `rust_process_mem_usage` 和 `rust_process_threads` 两个 Rust 进程表；`runtime_metric_catalog_uses_rust_process_metrics` 对这项有意差异提供回归覆盖。

相邻 [`metrics_schema.go`](metrics_schema.go) 在包 `init` 中立即注册虚拟库，并把 `Comment` 写入表元数据；当前 Rust `metric_schema_db` 是显式调用，并且构造 `TableInfo` 时没有使用定义中的 `Comment`。因此文档不能声称 Rust 已逐字段完整复刻 Go 注册行为。Go 的 [`metrics_schema_test.go`](metrics_schema_test.go) 使用正式 PromQL parser，Rust 对应测试只做分隔符平衡，这是另一个已知验证强度差异。

## 扩展指南

新增或修改指标表时，优先在 `MetricTableMap` 对应子系统分组中改一项，并同步检查：

1. 键保持小写且唯一；若 `instance` 出现在 `Labels` 中，必须排第一。
2. PromQL 使用动态 labels 时包含 `$LABEL_CONDITIONS`；使用范围向量时正确放置 `$RANGE_DURATION`；使用分位数时同时设置正 `Quantile`。
3. `Labels` 的顺序既决定 SQL 列顺序，也决定生成 label 条件的顺序；带 `by (` 聚合时确保每个 label 与模板分组一致。
4. 与 Go 共同支持的指标同步更新 `metric_table_def.go`；运行时特有指标必须记录有意差异，不能为了机械等量而恢复 Go runtime 表。
5. 扩展独立测试文件 [`metrics_schema_test.rs`](metrics_schema_test.rs)，不要把测试写进本生产文件。涉及特定上游合并语义时也应更新 [`go_merge_45_test.rs`](go_merge_45_test.rs)。

兼容风险主要是重命名/删除键会使 SQL 表消失，调整 labels 会改变表列和过滤语义，修改默认分位数会改变列默认值及默认查询结果，错误 PromQL 会把失败推迟到查询期。性能上，增加条目会线性增加首次目录构造和虚拟元数据生成成本，但常规单表查找仍为哈希查找。

## 验证依据

- RustCodeGraph：`status` 确认索引含目标文件；`files --filter pkg/infoschema/metric_table_def.rs` 确认该文件被索引；`query MetricTableMap` 与 `node MetricTableMap` 确认唯一 Rust 定义及 `lib.rs`、`metrics_schema.rs`、`go_merge_45_test.rs` 使用边；按文件 `node` 读取了定义开头与结尾。`callers MetricTableMap` 在 30 秒内未返回，因此调用结论还用仓库定点搜索复核。
- 生产源码：`pkg/infoschema/metric_table_def.rs`、`pkg/infoschema/metrics_schema.rs`、`pkg/infoschema/lib.rs`、`pkg/session/runtime/system_query.rs`、`pkg/executor/metrics_reader.rs`。
- crate 与 Go 对照：`pkg/infoschema/Cargo.toml`、`pkg/infoschema/metric_table_def.go`、`pkg/infoschema/metrics_schema.go`。
- 独立测试：`pkg/infoschema/metrics_schema_test.rs`、`pkg/infoschema/go_merge_45_test.rs`、`pkg/infoschema/metrics_schema_test.go`；另读 `pkg/executor/metrics_reader_test.rs` 以确认执行器通过 backend 抽象测试，而非直接读取本映射。
- 静态核验：精确统计 Rust 638 个键，并对 Go/Rust 键集合排序比较得到六个 Go-only 与两个 Rust-only 条目；搜索确认仓库中没有 `MetricsReaderBackend::metric_table_def` 到 `GetMetricTableDef` 的生产接线实现。
- 本任务只新增说明文档，按计划不运行 Cargo；最终结构验证要求文档存在且恰有十一个固定二级标题。
