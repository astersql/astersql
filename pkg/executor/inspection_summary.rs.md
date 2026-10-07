# `pkg/executor/inspection_summary.rs`

## 文件定位

[`inspection_summary.rs`](inspection_summary.rs) 属于 `astersql-executor` crate；crate 根在 [`lib.rs`](lib.rs) 中以 `pub mod inspection_summary` 公开该模块，包边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确认。它把 `metrics_schema` 指标表按巡检规则聚合为巡检摘要行，是 Go 文件 [`inspection_summary.go`](inspection_summary.go) 的 Rust 语义移植。

当前接线状态需要特别区分：Rust 源码已经公开规则、抽象运行时和检索算法，但全仓 `.rs` 引用搜索只找到独立测试 [`inspection_summary_test.rs`](inspection_summary_test.rs)，没有生产代码实现 `InspectionSummaryRuntime` 或实例化 `InspectionSummaryRetriever`。因此本文件当前是可测试的执行核心，尚不能据此认定 Rust SQL 主链已经把它接入 `INFORMATION_SCHEMA.INSPECTION_SUMMARY`。Go 版本则通过 executor 的 memtable 路径实际运行。

## 核心职责

- `inspectionSummaryRules` 维护 10 类摘要规则到指标表名的静态目录：`query-summary`、`wait-events`、`read-link`、`write-link`、`ddl`、`stats`、`gc`、`rocksdb`、`pd`、`raftstore`（源码第 79—479 行）。同一指标可以属于不同规则，但每条规则内部不应重复。
- `InspectionSummaryRetriever::retrieve` 根据规则名、指标名和分位数过滤条件，逐个查找指标定义，生成受限 SQL，对 `value` 计算 `avg/min/max`，再把查询结果转换成固定九列摘要行（第 483—596 行）。
- `InspectionSummaryRuntime` 隔离指标元数据、受限 SQL 执行、warning 写入和行取值，使算法不依赖具体 session/row 类型（第 51—66 行）。这是当前 Rust 实现连接真实 executor 上下文时必须实现的边界。

## 主要符号

- `InspectionSummaryExtractor`：调用方提供的过滤快照。`skip_inspection` 控制整次跳过；空 `rules` 或 `metric_names` 表示不过滤；`quantiles` 仅用于带分位数的指标（第 27—32 行）。字段均为 `pub`，结构本身不做合法性校验。
- `MetricDefinition`：单个指标表的 `labels`、展示 `comment` 和 `quantile` 标记（第 36—40 行）。`quantile > 0.0` 表示该表需要把 `quantile` 当作额外标签列。
- `InspectionSummaryValue::{String, Float, Null}`：运行时无关的输出/测试值模型（第 44—48 行）。摘要中的分位数对非分位指标使用 `Null`。
- `InspectionSummaryRuntime`：含 `Context`、`MetricRow`、`Error` 三个关联类型，以及 `metric_definition`、`append_warning`、`execute_restricted_sql`、`row_len`、`row_string`、`row_float` 六个适配方法（第 51—66 行）。错误类型必须同时实现 `Display` 与 `From<String>`。
- `InspectionSummaryRetriever<R>`：持有运行时、一次性状态 `retrieved`、过滤器和已格式化的 `time_range_condition`（第 69—74 行）。
- `inspectionSummaryRules() -> HashMap<&'static str, Vec<&'static str>>`：每次调用新建并返回规则目录（第 79—479 行）；名称沿用 Go 风格，文件级 `#![allow(non_snake_case)]` 为其放宽命名检查。
- `InspectionSummaryRetriever::retrieve(&mut self, &mut R::Context)`：唯一执行入口，返回 `Result<Vec<Vec<InspectionSummaryValue>>, R::Error>`（第 483—596 行）。

## 执行流程

1. 若 `retrieved` 已为真或 `extractor.skip_inspection` 为真，立即返回空数组；否则先把 `retrieved` 置真（`retrieve` 第 487—492 行）。这意味着后续 SQL 失败后也不会在同一个 retriever 上自动重试。
2. 遍历 `inspectionSummaryRules()`。非空 `rules` 只保留命中的规则，非空 `metric_names` 只保留命中的指标（第 494—503 行）。规则目录来自 `HashMap`，规则间遍历次序没有稳定保证。
3. 通过 `runtime.metric_definition(name)` 获取标签、注释和分位数属性。定义缺失时追加 `metrics table: {name} not found` warning 并继续其他指标，不把它提升为致命错误（第 505—510 行）。
4. 从 `time_range_condition` 克隆条件。若 `definition.quantile > 0.0`，把 `quantile` 追加到分组列；未指定分位数时补 `and quantile=0.99`，否则按六位小数拼成 `and quantile in (...)`（第 511—529 行）。
5. 无分组列时生成只含 `avg(value),min(value),max(value)` 的 SQL；有标签时追加反引号包裹的标签列，并使用相同列串 `group by`、`order by`（第 531—541 行）。表路径固定为 ``metrics_schema`.`{name}``。
6. 调用 `execute_restricted_sql`。错误被改写为包含完整 SQL 的 `execute '{sql}' failed: {error}` 后立即返回，后续指标不再处理（第 542—545 行）。
7. 对每个返回行，约定前三列依次是 avg、min、max。只有定义的第一个标签恰为 `instance` 时才单独抽取实例列；剩余标签值按 `, ` 连接，`store`/`store_id` 标签值增加 `store_id:` 前缀（第 546—573 行）。
8. 分位指标从行的最后一列读取 quantile，否则输出 `Null`。最终九列顺序为：规则、实例、指标名、标签串、分位数、平均值、最小值、最大值、注释（第 574—591 行）。

## 数据与状态

`InspectionSummaryRetriever` 是有状态的一次性检索器。`retrieved` 在任何元数据查找或 SQL 执行之前被设置，保证同一对象最多发起一轮扫描；测试 `inspection_summary_retrieve_matches_go_row_and_sql_semantics` 验证第二次调用返回空且 SQL 总数仍为 1。`runtime` 由 retriever 独占持有，`Context` 则在调用时以可变借用传入。

过滤集合使用 `HashSet<String>`，查找是集合成员判断；规则目录使用新建的 `HashMap`。结果按“规则遍历、规则内指标列表顺序、SQL 行顺序”追加，但第一层 `HashMap` 顺序不稳定，所以调用方若要求全局稳定展示顺序，应在更外层排序或改变目录表示，不能依赖当前返回顺序。

行协议没有运行时模式检查：`row_string`/`row_float` 的含义由运行时实现保证；分位行还要求 `row_len() >= 1`。独立测试的 `MockRuntime` 对错误类型直接 panic，这是测试适配器行为，不是 trait 强制的生产错误策略。

## 依赖与调用关系

直接语言依赖只有标准库 `HashMap`、`HashSet`。本文件不直接引用 `astersql-executor` 的其他 crate 依赖；真实系统依赖被压缩进 `InspectionSummaryRuntime`。`Cargo.toml` 表明它随 `astersql-executor` 库编译，没有专属 feature，`nextgen` feature 与本文件无条件编译无关。

RustCodeGraph 将 `retrieve` 的有效本地调用边识别为：调用 `inspectionSummaryRules`，并通过泛型运行时调用 `metric_definition`、`append_warning`、`execute_restricted_sql`、`row_len`、`row_string`、`row_float`；还构造 `InspectionSummaryValue::Float`。图查询对常见名 `retrieve`/`join` 存在跨文件同名噪声，因此全仓精确符号搜索用于复核实际接线，结果只有 [`inspection_summary_test.rs`](inspection_summary_test.rs) 实例化 retriever。RustCodeGraph 的文件关系还报告 [`analyze.rs`](analyze.rs) 使用该文件，但精确 Rust 符号搜索未发现上述公开符号引用，不能将该弱文件级关系当作生产调用证据。

Go 对应调用链是 `information_schema.inspection_summary` 的 memtable retriever → `inspectionSummaryRetriever.retrieve` → `infoschema.MetricTableMap` → session 的 `RestrictedSQLExecutor.ExecRestrictedSQL`。Rust 尚缺少把这些具体类型适配到 `InspectionSummaryRuntime` 并从 memtable 构建路径实例化 retriever的生产接线。

## 错误处理与边界

- 跳过与重复调用都成功返回空结果，不产生 warning（第 487—490 行）。
- 指标定义缺失是可恢复条件：记录 warning 后继续（第 505—510 行）；Rust 测试验证缺失 `tidb_qps` 得到空结果和精确 warning 文本。
- SQL 执行失败是致命条件：错误文本包含生成 SQL 和底层错误，测试 `inspection_summary_wraps_execution_error_with_sql_like_go` 锁定该格式。由于 `retrieved` 已置真，调用者不能在原对象上重试。
- `time_range_condition`、指标名和分位数字面量直接参与 SQL 拼接。本文件假设这些值来自受控的 planner/指标目录；如果未来允许不可信输入，必须在进入本层前验证，或改用安全的标识符/参数构造接口。
- 分位数用 `{quantile:.6}`，与 Go `%f` 的六位小数保持一致；本层不限制 NaN、无穷、范围外值或空集合之外的语义。
- 标签协议只识别首标签 `instance`；处于其他位置的 `instance` 会进入普通标签串。`store`/`store_id` 只改变展示值，不改变 SQL 分组。
- 空标签且为分位指标时，`quantile` 会成为唯一分组列，因此进入“有标签”SQL 分支；非分位空标签才使用无 `GROUP BY` 分支。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、锁、事务或长期缓存。`retrieve` 使用 `&mut self` 和 `&mut Context`，同一个 retriever 在安全 Rust 中不能被并发调用；运行时及上下文的线程安全要求不由该 trait 声明。

资源生命周期由 `execute_restricted_sql` 的实现负责。本层只接收已经物化的 `Vec<MetricRow>`，逐行读取后释放，不持有游标。与 Go 版本相比，Rust 文件没有显式 `context.Context` 取消语义，也没有设置 `kv.InternalTxnMeta`；若生产接线需要等价的取消、内部 SQL 标记或事务归属，应由 `Context` 和运行时适配器明确承载并补测试。

## 与 Go 版本的对应关系

[`inspection_summary.go`](inspection_summary.go) 的 `inspectionSummaryRules` 与 `inspectionSummaryRetriever.retrieve` 是直接对照。Rust 保留了规则/指标过滤、缺失定义 warning、分位数默认 0.99、六位小数列表、标签分组排序、`instance` 抽取、store 标签格式化、九列顺序和带 SQL 的错误包装。Rust 独立测试明确验证了这些核心语义。

两者仍有重要接线差异：Go 结构直接持有 `TableInfo`、planner extractor 和 `QueryTimeRange`，从 session 获取指标定义与 restricted SQL executor，并给 context 标记 `InternalTxnMeta`；Rust 使用自定义 extractor、字符串时间条件和泛型 trait，尚无生产实现。Go 测试 `TestInspectionSummary` 通过真实 SQL 路径、mock metrics failpoint 验证聚合和展示，`TestValidInspectionSummaryRules` 还逐项确认指标存在于 `infoschema.MetricTableMap`；Rust 规则测试只验证规则非空、规则内无重复以及关键指标存在，没有逐项连接真实 Rust 指标目录。

Go 的 nil 分位数对应 Rust `InspectionSummaryValue::Null`。Go 的 `types.Datum` 能表达更广泛 SQL 值，Rust 枚举当前只覆盖字符串、浮点和空值，足够服务本文件现有九列协议，但不是通用 Datum 替代物。

## 扩展指南

- 新增或移除摘要规则/指标时修改 `inspectionSummaryRules`，同步扩展 [`inspection_summary_test.rs`](inspection_summary_test.rs) 的目录完整性断言，并与 Go [`inspection_summary.go`](inspection_summary.go) 及 [`inspection_summary_test.go`](inspection_summary_test.go) 对照。应特别检查规则内重复、指标定义存在性和跨版本名称漂移。
- 改变 SQL、分位数、标签或输出列时，主要修改 `InspectionSummaryRetriever::retrieve`；同步独立 Rust 测试中的 SQL 精确值、九列结果、warning 和错误文本。不要把测试嵌回生产源文件。
- 接入 Rust 生产链时，实现 `InspectionSummaryRuntime` 并在 memtable/executor builder 的相应分支构造 retriever；同时补端到端 SQL 测试，验证权限/内部事务标记、取消传播、时间范围、warning 和结果排序。不能以当前 mock trait 测试代替生产接线证据。
- 若需稳定的规则间顺序，优先采用有序静态目录或在输出层排序，并评估每次调用重建大 `HashMap<Vec<_>>` 的成本。若改为全局惰性缓存，还要明确初始化和并发访问语义。
- 若增强健壮性，可在运行时边界验证行宽、标签列类型和分位数范围，并用可返回错误的读取接口替代隐式索引/类型假设；这会改变 trait 和错误契约，需要同步所有实现与测试。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件完整索引为 597 行。`node inspection_summary.rs::retrieve` 确认入口源码及其本地调用，`query InspectionSummaryRetriever`、`query inspectionSummaryRules` 确认 Rust/Go 对照符号，`callers`/`callees` 查询暴露同名噪声后以精确仓库搜索复核。
- 已读生产与装配文件：[`inspection_summary.rs`](inspection_summary.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、Go 对照 [`inspection_summary.go`](inspection_summary.go)。目标包没有 `doc.go`。
- 已读独立测试：Rust [`inspection_summary_test.rs`](inspection_summary_test.rs)；Go [`inspection_summary_test.go`](inspection_summary_test.go)。Rust 测试覆盖目录基本不变量、SQL/九列转换、一次性读取、默认 0.99、缺失定义 warning 和 SQL 错误包装；Go 测试补充真实 SQL 聚合与指标目录存在性证据。
- 实际 Rust 接线复核：全仓 `*.rs` 对 `InspectionSummaryRetriever|InspectionSummaryExtractor|InspectionSummaryRuntime|InspectionSummaryValue|MetricDefinition|inspectionSummaryRules` 的精确搜索仅命中本文件与其独立测试，所以本文将生产集成标为“尚未验证/尚未接线”，没有用 Go 现状替代 Rust 事实。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构检查要求本文恰好包含上述 11 个固定二级标题。
