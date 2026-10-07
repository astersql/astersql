# `pkg/infoschema/catalog_columns_go45.rs`

## 文件定位

本文件是 `astersql-infoschema` crate 内部的静态目录定义文件，专门保存 `INFORMATION_SCHEMA.SLOW_QUERY`、`INFORMATION_SCHEMA.STATEMENTS_SUMMARY` 与 `INFORMATION_SCHEMA.STATEMENTS_SUMMARY_HISTORY` 的列元数据。它由父模块 [`tables.rs`](tables.rs) 通过 `#[path = "catalog_columns_go45.rs"] mod catalog_columns_go45;` 私有装配，不是 crate 的公共 API；两个构造函数也只开放到父模块（`pub(super)`）。crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定，父模块再由 [`lib.rs`](lib.rs) 的 `pub mod tables` 纳入 crate。

文件名中的 `go45` 与文件头注释共同表明，这批列定义来自 Go `pkg/infoschema/tables.go` 中 `slowQueryCols` 和 `tableStatementsSummaryCols` 在合并点 `ad193e964b` 的移植快照。它只描述目录结构，不读取慢日志、聚合语句、生成查询行或执行 SQL。

## 核心职责

核心职责有三项：

1. `slow_query_columns` 按协议顺序构造 97 个慢查询列，从 `Time` 到 `Query`，覆盖事务、解析/编译、Cop/TiKV、RocksDB、IA 远端读取、资源消耗、计划及会话连接属性等字段。
2. `statements_summary_columns` 按协议顺序构造 127 个语句摘要列，从 `SUMMARY_BEGIN_TIME` 到 `STORAGE_MPP`，覆盖执行次数、时延、键访问、事务阶段、资源消耗、计划缓存、绑定、RU、跨可用区流量及存储来源等聚合指标。
3. 私有辅助函数 `col` 把每个列声明统一转换为父模块的 `columnInfo`，集中设置名称、类型、显示长度、精度、无符号/非空标志、默认值和注释，并为慢日志的 `Time` 列补上主键与二进制标志。

列的顺序本身是接口的一部分：[`tables.rs`](tables.rs) 的 `buildTableMeta` 以迭代下标写入 `Offset`，调用方和回归测试会按固定下标访问列。因此不能把这里当作无序字段集合。

## 主要符号

- `fn col(name: &'static str, column_type: ColumnType, size: u32, decimal: Option<u8>, unsigned: bool, not_null: bool, comment: &'static str) -> columnInfo`：文件私有的列描述构造器。所有输入都是静态元数据；`default_value` 固定为 `None`，且仅当名称严格等于 `"Time"` 时同时设置 `primary_key` 和 `binary`。
- `pub(super) fn slow_query_columns() -> Vec<columnInfo>`：构造一份新的慢查询列向量。首列 `Time` 是 `Timestamp(26, 6)`、非空、主键、二进制；多项计数使用 `unsigned`；`Read_pool_task_details`、`Warnings`、`Plan`、`Binary_plan`、`Prev_stmt`、`Query` 等使用大对象类型；`Session_connect_attrs` 使用 JSON 类型。
- `pub(super) fn statements_summary_columns() -> Vec<columnInfo>`：构造一份新的语句摘要列向量。大量计数/时长列同时设置 `unsigned` 与 `not_null`，文本样本及计划使用 `Blob`，摘要、字符集、排序规则和资源组使用 `Varchar`，存储来源与布尔状态使用 `Tiny`。
- `ColumnType` 与 `columnInfo`：均定义在父模块 [`tables.rs`](tables.rs)。`ColumnType` 是 MySQL 字段类型子集，`columnInfo` 保存后续生成模型列所需的简化元数据。

本文件没有常量、结构体、枚举、trait、`impl` 或条件编译项，也没有对 crate 外公开的符号。

## 执行流程

1. 第一次调用 [`tables.rs`](tables.rs) 的 `table_registry()` 时，`OnceLock<HashMap<...>>` 遍历 `TABLE_NAMES`。
2. 对每个表名调用 `default_columns(name)`；匹配 `TableSlowQuery` 时调用 `slow_query_columns()`，匹配 `TableStatementsSummary | TableStatementsSummaryHistory` 时调用 `statements_summary_columns()`。历史表因此与当前摘要表共享同一套结构语义，但每次调用取得独立 `Vec`。
3. 两个构造函数按源码顺序反复调用 `col`。`col` 不校验参数，也不查询外部状态，只把参数和约定的默认字段装入 `columnInfo`。
4. `table_registry` 把结果连同稳定表 ID 缓存在 `VirtualTableMeta` 中。
5. `information_schema_db_with_storage_class` 遍历注册表，并由 `buildTableMeta` 将每个 `columnInfo` 转换为实际模型列：映射 MySQL 类型，选择字符集/排序规则，换算 Blob 长度，写入精度、标志、顺序、注释与默认值。
6. 后续查询看到的列名、类型、顺序、可空性和注释来自这些已注册的模型元数据；实际行内容由 infoschema/执行器的其他路径提供，不在本文件中生成。

## 数据与状态

两个公开到父模块的函数都返回拥有所有权的新 `Vec<columnInfo>`；元素只含 `Copy` 值、布尔值、整数、枚举和 `&'static str`，没有借用会话状态。文件本身没有全局可变状态。

关键不变量如下：

- 慢查询列数为 97，首尾分别是 `Time` 与 `Query`；`Time` 是本文件唯一由 `col` 自动标记为主键和二进制的列。
- 语句摘要列数为 127；`IA_EXEC_COUNT` 位于零基下标 45，并为 `Longlong`、无符号、非空。
- 声明顺序决定 `buildTableMeta` 写入的 `Offset`，不能在中间无意插入、删除或排序。
- `decimal: None` 在 `buildTableMeta` 中转成精度 0；慢查询 `Time` 显式使用 `Some(6)`。摘要时间戳沿用 Go 定义，未显式设置小数精度。
- `size == 0` 对 Blob 家族不是最终字段长度；`buildTableMeta` 分别把 `Blob`、`MediumBlob`、`LongBlob` 映射为 `1 << 16`、`1 << 24`、`1 << 32`。
- 所有列的 `default_value` 都为 `None`。列注释仅由 `statements_summary_columns` 的多数列提供；慢查询列注释均为空字符串。

## 依赖与调用关系

直接上游是 [`tables.rs`](tables.rs) 的 `default_columns`：

- `TableSlowQuery -> catalog_columns_go45::slow_query_columns()`；
- `TableStatementsSummary | TableStatementsSummaryHistory -> catalog_columns_go45::statements_summary_columns()`。

再上游是 `table_registry`，它把列向量装进 `VirtualTableMeta`；`information_schema_db_with_storage_class` 和 `buildTableMeta` 把注册信息变成 `DBInfo`/`TableInfo`/`model::ColumnInfo`，形成 INFORMATION_SCHEMA 元数据快照。集群表会复用相关目录语义，但集群表添加实例信息和读取远端数据的逻辑位于 `cluster.rs` 及测试辅助中，不属于本文件。

直接下游依赖只有父模块的 `ColumnType` 和 `columnInfo`。真正的类型及标志映射由 `buildTableMeta` 依赖 `astersql-parser-mysql`、`astersql-parser-charset` 和 `astersql-meta-model` 完成；本文件不直接依赖外部 crate。`Cargo.toml` 还表明该 crate 依赖 `astersql-meta-autoid` 等元数据组件，但这些是父层注册/建模路径的依赖，而不是本文件的直接调用。

RustCodeGraph 能识别本文件及三个函数，但未生成它们的 callers/callees 边；源码检索确认这是图未覆盖 `#[path]` 私有模块与 `match` 调用接线，而非未使用代码。

## 错误处理与边界

三个函数均不返回 `Result`、不抛出业务错误，也没有显式 `panic!`。它们接受的值全部写死在源码中，所以运行时不存在用户输入解析、I/O、分配上限检查或错误恢复分支；唯一可能的常规运行时失败是构造大型 `Vec` 时的进程级内存分配失败。

真正需要防范的是静态元数据错误：列名拼写或大小写错误、顺序漂移、类型/长度/精度不一致、`unsigned`/`not_null` 标志遗漏，以及把新字段错误放入当前表而未同步历史/集群表契约。这些错误通常不会在构造函数内暴露，而会表现为 INFORMATION_SCHEMA 查询兼容性变化或消费方按偏移取错值。

`col` 用 `name == "Time"` 隐式决定主键和二进制标志，这是一条大小写敏感的局部约定。若未来其他表也出现同名列并复用该辅助函数，就会自动获得这两个标志；扩展时必须确认这是预期行为。相反，摘要列没有名为 `Time` 的字段，因此不会误触发。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、文件句柄或网络资源。两个列函数是确定性的纯构造过程，可并发调用；每次调用得到互不共享的向量。

跨线程共享发生在父模块：`table_registry()` 通过 `std::sync::OnceLock` 只初始化一次 `HashMap`，之后返回 `'static` 只读引用。因而正常应用生命周期中，这两套列定义只在注册表首次访问时构造一次，随后随进程存在，不需要显式清理。`information_schema_db_with_storage_class` 还会把生成的表模型缓存在另一个 `OnceLock<DBInfo>` 中；克隆快照不会回写本文件的定义。

性能上，文件初始化成本与 224 个列声明线性相关（97 + 127），且只涉及小型元数据分配；稳定运行阶段没有逐行或逐查询执行成本。修改时应避免在这里引入运行时查询或锁，从而破坏当前的静态、一次初始化特性。

## 与 Go 版本的对应关系

Go 对照位于 [`tables.go`](tables.go)：`slowQueryCols` 定义慢查询列，`tableStatementsSummaryCols` 定义语句摘要列，`tableNameToColumns` 分别把它们绑定到 `TableSlowQuery`、`TableStatementsSummary` 和 `TableStatementsSummaryHistory`。Rust 的 `default_columns` 保留了同样的三条绑定关系。

主要表示差异是：Go 使用 `variable`、`execdetails`、`stmtsummary` 等包的字符串常量以及 `mysql.Type*`/位标志；Rust 快照把解析后的列名写成字符串字面量，把类型压缩成 `ColumnType`，把位标志拆成 `unsigned`、`not_null`、`primary_key`、`binary` 四个布尔字段。`types.UnspecifiedLength` 在 Rust 声明中表现为 `size: 0`，再由 `buildTableMeta` 按 Blob 类型恢复字段长度。

语义上需要保持的不是源码写法，而是列名、大小写、顺序、类型、显示长度、精度、可空性、无符号标志、主键/二进制标志和注释。当前 Rust 独立回归覆盖了关键数量、首尾、IA 字段位置与类型、`Read_pool_task_details` 的 LongBlob 长度及 `Time` 主键标志；它不是对全部 224 列逐字段自动比对，因此未来同步 Go 变更仍需人工或新增表驱动校验。

## 扩展指南

新增或调整慢查询字段时，修改 `slow_query_columns`；新增或调整当前/历史语句摘要字段时，修改 `statements_summary_columns`。遵循 Go 对照中的相对位置，不要只把新列追加到方便的位置。若新增类型超出 `ColumnType`，还必须同步 [`tables.rs`](tables.rs) 中 `ColumnType` 和 `buildTableMeta` 的类型、长度、字符集及标志映射。

安全修改清单：

1. 在 Go `tables.go` 的对应列数组确认名称来源、位置、类型、长度、精度、标志和注释。
2. 更新本文件对应函数，并检查 `col` 的隐式 `Time` 规则是否适用；不要通过更改通用默认值影响另一套列。
3. 同步独立测试 [`go_merge_45_test.rs`](go_merge_45_test.rs) 中 `go_merge_45_metrics_and_storage_class_metadata` 的列数、边界列、固定偏移及新字段类型/标志断言。Rust 测试必须继续放在独立文件，不能嵌入本源文件。
4. 对用户可见语义，参考 [`test/clustertablestest/tables_test.go`](test/clustertablestest/tables_test.go) 的 `TestStmtSummaryTable` 和 [`test/clustertablestest/cluster_tables_test.rs`](test/clustertablestest/cluster_tables_test.rs) 的慢查询/摘要行为测试，必要时在对应独立 Rust 测试文件增加回归。
5. 特别评估兼容风险：消费者可能依赖列序号；历史表必须与当前摘要表一致；集群表会在基础结构上形成远端查询契约。性能风险通常很低，但增加大量列会提高注册表和每行结果的元数据/传输开销。

不要在本文件实现慢日志解析或摘要聚合；这些属于数据生产路径。也不要为绕过类型缺口而把字段简化成通用字符串，因为 `buildTableMeta` 的模型类型会直接影响 SQL 类型检查和客户端元数据。

## 验证依据

- RustCodeGraph：`status` 显示仓库索引包含 7032 个 Rust 文件；`files --filter pkg/infoschema` 找到本文件并报告 5 个索引符号；`node --file pkg/infoschema/catalog_columns_go45.rs --offset ...` 完整读取 1868 行并确认 `col`、`slow_query_columns`、`statements_summary_columns` 的实现。对三者执行带 `--file` 的 `callers`/`callees` 未返回边，因此使用源码检索补证。
- Rust 生产源码：[`catalog_columns_go45.rs`](catalog_columns_go45.rs)；[`tables.rs`](tables.rs) 的模块装配、`ColumnType`、`columnInfo`、`default_columns`、`table_registry`、`buildTableMeta`、`information_schema_db_with_storage_class`；[`lib.rs`](lib.rs) 的 crate 模块入口。
- crate 声明：[`Cargo.toml`](Cargo.toml) 确认 crate 名为 `astersql-infoschema`、库入口为 `lib.rs`，并列出父层建模使用的本地依赖。
- Go 对照：[`tables.go`](tables.go) 的 `slowQueryCols`、`tableStatementsSummaryCols` 与 `tableNameToColumns`。
- 独立 Rust 回归：[`go_merge_45_test.rs`](go_merge_45_test.rs) 的 `go_merge_45_metrics_and_storage_class_metadata` 验证 97/127 列、关键偏移、类型、长度、标志和 IA 字段；该测试由 [`lib.rs`](lib.rs) 的 `#[cfg(test)] mod go_merge_45_test` 接入。
- 行为测试：[`test/clustertablestest/cluster_tables_test.rs`](test/clustertablestest/cluster_tables_test.rs) 验证慢日志字段、摘要开关、集群表识别和连接属性；Go [`test/clustertablestest/tables_test.go`](test/clustertablestest/tables_test.go) 的 `TestStmtSummaryTable` 验证列注释及 IA 字段在当前、历史和集群变体中的可见性。
- 本任务为纯文档分析，按计划不运行 Cargo。最终以任务指定命令校验目标文件存在且恰有 11 个固定二级标题，并人工复核未把数据读取/聚合逻辑归因给本文件。
