# `pkg/infoschema/metrics_schema.rs`

## 文件定位

本文件属于 `astersql-infoschema` crate（`pkg/infoschema/Cargo.toml` 的 `[lib]` 指向 `lib.rs`），由 `pkg/infoschema/lib.rs` 以 `pub mod metrics_schema` 纳入模块树，并在 crate 根重导出 `GetMetricTableDef`、`IsMetricTable` 和 `MetricTableDef`。它是 `METRICS_SCHEMA` 的静态元数据与 PromQL 模板工具层：定义每张指标表的描述结构，将 `MetricTableMap` 投影为 InfoSchema 所需的 `DBInfo`/`TableInfo`，并提供表名查询、列生成和 PromQL 占位符展开。

它不负责向 Prometheus 发起网络请求。Rust 执行侧的拉取、超时、权限和结果转行在 `pkg/executor/metrics_reader.rs` 的 `MetricsReaderBackend`/`MetricRetriever` 边界内；当前该执行文件使用自己的运行时 `MetricTableDef`，不是本文件的静态定义类型。

## 核心职责

1. 用 `MetricTableDef` 表达 PromQL 模板、可过滤 label、默认 quantile 和注释，并以 `EMPTY` 保留 Go 结构体零值语义。
2. 用 `genColumnInfos` 按固定顺序生成 `time -> Labels -> quantile(可选) -> value` 列描述，用 `GenPromQL`/`genLabelCondition`/`GenLabelConditionValues` 稳定展开查询模板。
3. 用 `IsMetricTable` 和 `GetMetricTableDef` 封装对 `MetricTableMap` 的判定与查询，要求调用方传入已转小写的表名。
4. 用 `metric_schema_db` 生成名为 `METRICS_SCHEMA` 的内存数据库描述，对表名排序后连续分配稳定负 ID。
5. 用 `metricSchemaTable`、`tableFromMetaForMetricsTable` 和 `metricTables` 提供虚拟表包装/导出辅助。当前 `tableFromMetaForMetricsTable` 把空行集交给 `infoschemaTable`，因而本层只携带元数据，不物化 Prometheus 数据。

## 主要符号

- `promQLQuantileKey`、`promQLLabelConditionKey`、`promQRangeDurationKey`：三个公开的模板 token，分别对应 `$QUANTILE`、`$LABEL_CONDITIONS`、`$RANGE_DURATION`。
- `MetricSchemaDBID: i64 = -2000`：Rust 版的 metrics schema 固定 ID，用负值避免与用户对象的正 ID 空间冲突。
- `MetricTableDef { PromQL, Labels, Quantile, Comment }`：所有字段都是公开静态数据；类型是 `Clone + Copy`，定义来自 `pkg/infoschema/metric_table_def.rs` 的静态 `MetricTableMap`。
- `MetricTableDef::genColumnInfos(&self) -> Vec<columnInfo>`：将定义转换为 `pkg/infoschema/tables.rs` 中的列描述。方法为 crate 内可见，不是 crate 对外 API。
- `MetricTableDef::GenPromQL(...) -> String`：公开模板展开方法；它只替换字符串，不校验 PromQL 语法、负数时间窗口、quantile 范围或 label 值的正则转义。
- `IsMetricTable`/`GetMetricTableDef`：按 key 直接访问 `MetricTableMap`；后者在缺失时返回 `Err(String)`，错误文案为 `can not find metric table: <name>`。
- `GenLabelConditionValues`：对 `HashSet<String>` 中的值做不稳定排序（输出顺序仍确定）后以 `|` 连接，为多值 `=~` 条件生成 alternation 文本。
- `metric_schema_db() -> DBInfo`：当前 Rust 主链中有直接调用；`pkg/session/runtime/system_query.rs::build_virtual_system_catalog` 用它将 metrics 表名、ID 和列名放入会话级虚拟系统 catalog。
- `metricSchemaTable`：内含 `infoschemaTable` 的私有包装，只向外转发 `Meta` 和 `IterRecords`。
- `tableFromMetaForMetricsTable(meta) -> metricSchemaTable`：以空 `Vec` 作为行集构造包装。`metricTables() -> Vec<Table>` 则把 `metric_schema_db().tables` 中的 `Arc<TableInfo>` 直接包装为通用 `Table`。

## 执行流程

**Schema 构建链：**

1. `metric_schema_db` 遍历 `MetricTableMap`，收集 `(&str, &MetricTableDef)`。
2. 按表名升序排序，消除 map 遍历顺序对 ID 的影响。
3. 对每项调用 `genColumnInfos`，再把每个 `columnInfo` 投影为简化 `ColumnInfo { id, name, auto_increment: false }`。表 ID 从 `MetricSchemaDBID + 1` 连续递增，列 ID 从 1 连续递增。
4. 返回 `DBInfo { id: -2000, name: "METRICS_SCHEMA", tables, table_name_2_id: empty }`。
5. `build_virtual_system_catalog` 遍历返回的表，用小写 schema/table key 和列名注册会话元数据。

**PromQL 生成链：**

1. `GenPromQL` 先用 `quantile.to_string()` 替换全部 `$QUANTILE`。
2. `genLabelCondition` 只按 `self.Labels` 的定义顺序查找输入 map；未在定义内的 key、缺失 key 和空值集均被忽略。
3. 单值生成 `label="value"`，多值生成 `label=~"a|b"`；多个 label 用逗号连接，值经 `GenLabelConditionValues` 排序以保证结果可重复。
4. 最后用 `<range_duration>s` 替换 `$RANGE_DURATION`并返回完整字符串。

## 数据与状态

- 本文件没有可变全局状态。定义源 `MetricTableMap` 是静态 catalog，`MetricTableDef` 的文本与 label 都是 `'static` 引用。
- `genColumnInfos` 为 quantile 列默认值调用 `Box::leak`，将每次生成的字符串提升为 `&'static str`。这与 catalog 的静态生命期假设相容，但重复调用会永久泄漏少量内存，扩展或高频重建时必须考虑这一边界。
- `metric_schema_db` 每次都新建 `Vec` 并生成新的 `Arc<TableInfo>`；稳定性由表名排序和连续 ID 规则保证，不依赖渐变计数器。
- `metricSchemaTable` 与底层 `infoschemaTable` 通过 `Arc` 共享表元数据和行集。由于构造时行集为空，`IterRecords` 当前不会产生指标样本。
- 当前 `TableInfo` 是简化元数据结构。`metric_schema_db` 虽调用 `genColumnInfos`，但投影时只保留列 ID/名称/自增标志，没有把 `ColumnType`、size、default 或 `MetricTableDef::Comment` 写入返回的 `TableInfo`。因此这些细节只能从 `genColumnInfos` 的中间结果观察，不应宣称已完整注册到 Rust catalog。

## 依赖与调用关系

- 上游：`pkg/session/runtime/system_query.rs::build_virtual_system_catalog` 直接调用 `metric_schema_db`，把表/列投影进 session 系统 catalog。`pkg/infoschema/lib.rs` 对外重导出三个核心查询符号。RustCodeGraph 的文件反向关系另标识 `pkg/infoschema/go_merge_45_test.rs` 与上述 session 文件使用本文件；前者的 metrics 测试直接检查 `MetricTableMap`。
- 下游：定义数据来自 `pkg/infoschema/metric_table_def.rs::MetricTableMap`；元数据类型来自 `pkg/infoschema/infoschema.rs::{CiString, ColumnInfo, DBInfo, Table, TableInfo}`；列描述和虚拟表容器来自 `pkg/infoschema/tables.rs::{ColumnType, columnInfo, infoschemaTable}`。
- Cargo 边界：这些类型都是同 crate 模块，本文件没有直接使用 `pkg/infoschema/Cargo.toml` 中声明的第三方包；它仅使用 `std::{collections, sync::Arc}` 和 crate 内 API。
- 执行分层：`pkg/executor/metrics_reader.rs` 的 backend trait 提供 `metric_table_def`、`label_condition_values` 和 `generate_promql` 注入点，说明 Prometheus I/O 发生在执行器而不是本元数据文件。当前两侧类型分离，修改模板契约时需同时检查执行器适配。

## 错误处理与边界

- 显式可恢复错误只有 `GetMetricTableDef` 的未知表名 `Err(String)`；`IsMetricTable` 用 `false` 表示缺失。两者均不负责大小写规范化，测试明确要求大写 `RUST_PROCESS_THREADS` 查询失败。
- `GenPromQL` 是机械字符串替换：它不校验模板是否遗留 token，不验证 PromQL 可解析性，也不对 label 名/值做引号或正则转义。输入必须由上层保证安全且符合 Prometheus 语法。
- label 多值用 `|` 直接拼接，如果值自身含正则元字符，会被 PromQL regex 按语法解释；本层不转义。
- `metric_schema_db` 的 ID 计算是 `MetricSchemaDBID + index + 1`，在当前 catalog 规模下无溢出检查；表名必须唯一由 map 结构自然保证。
- `tableFromMetaForMetricsTable` 没有错误返回通道，且只构造空行集；它不等价于 Go 版注册 driver 后的完整虚拟表读取链。

## 并发与资源生命周期

所有公开计算都是同步、无 async、无锁、无任务、无通道且无网络 I/O。`MetricTableDef` 只借用静态数据，查询函数返回 `&'static MetricTableDef`；`metric_schema_db` 通过 `Arc<TableInfo>` 移交共享所有权，`metricSchemaTable` 又借助底层 `infoschemaTable` 的 `Arc` 使 clone 共享元数据/行数据。

并发读取不会修改本地状态，但 `Box::leak` 是显式的进程级生命期选择：quantile 默认值字符串不会被回收。如果未来把 catalog 从启动/少量构建改为动态高频重建，应先把 `columnInfo.default_value` 改为可持有的字符串或缓存静态格式化结果。

## 与 Go 版本的对应关系

`pkg/infoschema/metrics_schema.go` 是最近的语义对照：

- 常量、`MetricTableDef`、`IsMetricTable`、`GetMetricTableDef`、`genColumnInfos`、`GenPromQL`、`genLabelCondition`、`GenLabelConditionValues`、`metricSchemaTable` 和 `tableFromMetaForMetricsTable` 都有同名或直接对应实现。Rust 保留了列顺序、单值 `=`/多值 `=~`、label 定义顺序、值排序、token 替换顺序和缺失表错误文案。
- Go `init` 调用 `buildTableMeta`，写入 `Comment`、`DBID`、`MaxColumnID`/`MaxIndexID`，配置 charset/collation，然后通过 `RegisterVirtualTable` 注册 driver。Rust 把建库逻辑改为显式 `metric_schema_db` 调用，当前仅构造简化 `DBInfo`/`TableInfo`，没有全局 `init` 注册和等价的 charset/collation/comment/完整列类型元数据。
- Go `tableFromMetaForMetricsTable` 把每个 model column 转为 `table.Column`，并标记 `table.VirtualTable`；Rust 当前只用空行集包装简化 meta，是较窄的元数据门面。
- Go 用 `strconv.FormatFloat(..., 'f', -1, 64)`，Rust 用 `f64::to_string()`。对现有常规 quantile 测试值（如 `0.99`、`0.95`）输出一致；若新增极端浮点值，应额外验证两语言的格式差异。
- `pkg/infoschema/metrics_schema_test.go::TestMetricSchemaDef` 使用 Prometheus `parser.ParseExpr` 真实解析展开后的 PromQL。Rust 的 `pkg/infoschema/metrics_schema_test.rs::test_metric_schema_def` 保留 quantile/label/小写/instance 首位等结构断言，但只以括号平衡近似代替 parser，因此 Rust 单测不能证明完整 PromQL 语法有效。

## 扩展指南

- 新增/修改指标表应优先改 `pkg/infoschema/metric_table_def.rs::MetricTableMap`，确保 key 小写、`instance` 若存在则排在 labels 首位、模板的 `$QUANTILE`/`$LABEL_CONDITIONS` 与定义字段一致。同步扩展独立测试 `pkg/infoschema/metrics_schema_test.rs`，必要时也更新 Go 对照测试。
- 改变列布局时修改 `MetricTableDef::genColumnInfos`，并保留稳定顺序；扩展 `metric_table_helpers_match_go_contract` 对列名、类型、长度和默认值的断言。同时检查 `metric_schema_db` 的简化投影是否也需升级，否则新属性不会进入 session catalog。
- 改变 PromQL 生成时同步检查 `GenPromQL`、`genLabelCondition`、`GenLabelConditionValues`、Go 同名函数，以及 `pkg/executor/metrics_reader.rs::MetricsReaderBackend::{generate_promql,label_condition_values}` 的生产适配。关键风险是 regex 转义、浮点文本兼容、label 顺序和 PromQL parser 兼容。
- 改变 ID 规则时修改 `metric_schema_db`，必须保留跨实例稳定性，并扩展 `metric_schema_metadata_has_stable_sorted_ids`；表名排序是当前稳定 ID 的核心不变量。
- 若要完整对齐 Go 虚拟表 driver，接入点是 `metric_schema_db`、`tableFromMetaForMetricsTable` 和 session/executor 之间的注册桥接。不应让本文件直接负责 Prometheus I/O；超时、权限和异步资源生命周期仍应保留在 executor backend 边界。
- Rust 测试必须继续放在独立 `pkg/infoschema/metrics_schema_test.rs`，不把测试内嵌进生产文件。如果修复缺陷，应先用该独立测试复现，再修生产逻辑。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/infoschema/metrics_schema.rs` 确认目标文件被索引且有 26 个符号。`node --file ... --offset 1 --limit 500` 读取全文，并报告文件使用方 `pkg/infoschema/go_merge_45_test.rs` 和 `pkg/session/runtime/system_query.rs`。
- RustCodeGraph 符号查询：对 `MetricTableDef`、`GenPromQL`、`IsMetricTable`、`GetMetricTableDef`、`GenLabelConditionValues`、`metric_schema_db`、`tableFromMetaForMetricsTable`、`metricTables` 执行 `query`，确认 Rust 定义与 Go 同名对照。`callers`/`callees` 精确查询在本地数据库上多次 30 秒内未返回，因此本文档没有据此臆测函数级边，而是用索引的文件反向关系和直接调用点补证。
- 已读源码/边界：`pkg/infoschema/metrics_schema.rs`、`pkg/infoschema/lib.rs`、`pkg/infoschema/Cargo.toml`、`pkg/infoschema/metric_table_def.rs`（由符号与定义 map 引用核对）、`pkg/infoschema/tables.rs`、`pkg/infoschema/infoschema.rs`、`pkg/session/runtime/system_query.rs`、`pkg/executor/metrics_reader.rs`。目标 package 下无 `doc.go`，故以 `lib.rs` 的 crate 文档为最近模块契约。
- 已读语义对照/测试：`pkg/infoschema/metrics_schema.go`、`pkg/infoschema/metrics_schema_test.go`、`pkg/infoschema/metrics_schema_test.rs`、`pkg/infoschema/go_merge_45_test.rs::go_merge_45_metrics_and_storage_class_metadata`。Rust 测试覆盖定义不变量、Rust 进程指标替代 Go-only 指标、列/PromQL helper、缺失表错误和稳定 ID；Go 测试额外用真实 PromQL parser 验证语法。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前仅运行任务指定的十一章结构检查，并人工复核文档中的当前实现限制没有被表述为“已完整支持”。
