# `pkg/util/dbutil/index.rs`

## 文件定位

本文件属于 `astersql-util-dbutil` crate 的索引元数据辅助模块，由 [`pkg/util/dbutil/lib.rs`](lib.rs) 以 `pub mod index` 暴露。crate 在 [`pkg/util/dbutil/Cargo.toml`](Cargo.toml) 中声明，并通过 `astersql-infoschema` 取得 `ColumnInfo`；查询抽象、返回值和错误类型来自同 crate 的 [`interface.rs`](interface.rs)。

它是 Go 文件 [`pkg/util/dbutil/index.go`](index.go) 的 Rust 对照实现，负责把 `SHOW INDEX` 结果转换为轻量索引信息，并基于表的列/索引元数据选择适合扫描、排序或遍历的列。仓库搜索未发现这些公开函数在非测试 Rust 代码中的调用点；当前能够确认的直接入口是独立测试 [`index_test.rs`](index_test.rs)，所以它目前是“已实现、已测试、尚未接入 Rust 生产主链”的公共模块，而不是 SQL 执行主链本身。

## 核心职责

1. `ShowIndex` 通过 `QueryExecutor` 执行 `SHOW INDEX FROM <schema>.<table>`，按列名解析每一行，并生成 `IndexInfo`。
2. `FindSuitableColumnWithIndex` 按“主键首列 → 唯一索引首列 → `SHOW INDEX` 中基数最大的普通索引首列”的优先级选择一个列对象。
3. `FindAllIndex` 稳定地把索引排列为主键、唯一索引、普通索引；`FindAllColumnWithIndex` 再按该顺序收集索引列并按列原名去重。
4. `SelectUniqueOrderKey` 选择能构成唯一排序键的列：主键优先；无主键时使用声明顺序中最后一个唯一索引；两者都没有时退回全部列。

这些职责分别可由 `ShowIndex`、`FindSuitableColumnWithIndex`、`FindAllIndex`、`FindAllColumnWithIndex` 和 `SelectUniqueOrderKey` 的实现，以及 [`index_test.rs`](index_test.rs) 中的四个 Rust 测试复核。

## 主要符号

- `IndexInfo`：一行 `SHOW INDEX` 的投影，保存 `Table`、`KeyName`、`ColumnName`、`SeqInIndex`、`Cardinality` 和 `NoneUnique`。字段名保留 Go 风格；`NoneUnique` 对应结果列 `Non_unique`，值为 `"1"` 时为 `true`。
- `TableIndexInfo`：表内单个索引的轻量描述。`columns` 是指向 `IndexedTable.columns` 的位置列表，另有 `primary`、`unique` 标记；`name` 在排序和选择算法中不参与决策。
- `IndexedTable`：供本模块算法消费的表视图，包含表名、`ColumnInfo` 列数组和 `TableIndexInfo` 索引数组。它并非完整的 TiDB `TableInfo`。
- `text(Option<&Value>) -> String`：内部宽松转换器。字符串原样返回，字节按 UTF-8 有损转换，数值和布尔值转成文本，`NULL`、缺值及不支持的情况转为空串。
- `required_text(&QueryResult, &[Value], &str) -> Result<String, DbError>`：内部严格字段读取器。结果列名按 ASCII 大小写不敏感匹配；列缺失、行过短或值为 `NULL` 时返回 `DbError`。
- `ShowIndex(&dyn QueryExecutor, &str, &str) -> Result<Vec<IndexInfo>, DbError>`：数据库查询和结果映射入口。
- `FindSuitableColumnWithIndex(&dyn QueryExecutor, &str, &IndexedTable) -> Result<Option<&ColumnInfo>, DbError>`：选择一个适合索引扫描的列；返回引用的生命周期绑定到输入表。
- `FindAllIndex(&IndexedTable) -> Vec<&TableIndexInfo>`：返回排序后的索引引用，不复制或修改表内索引。
- `FindAllColumnWithIndex(&IndexedTable) -> Vec<&ColumnInfo>`：返回按索引优先级排列且按 `column.name.original` 去重的列引用。
- `SelectUniqueOrderKey(&IndexedTable) -> (Vec<String>, Vec<&ColumnInfo>)`：同时返回排序键列名副本和对应列引用。

## 执行流程

`ShowIndex` 首先用 `TableName(schema, table)` 引用 schema 与表名，拼成不带绑定参数的 `SHOW INDEX` 语句，然后调用 `QueryExecutor::QueryContext`。它按返回行顺序遍历：通过 `required_text` 取得六个必需字段，将 `Seq_in_index` 和 `Cardinality` 解析为 `i64`，将 `Non_unique == "1"` 转成布尔值，最后按原行顺序追加到结果向量。任何查询、字段读取或整数解析错误都会立即终止，已解析的部分不会返回。

`FindSuitableColumnWithIndex` 的短路顺序如下：

1. 顺序查找第一个 `primary` 索引，取其第一个位置并在 `table.columns` 中查列。
2. 若无主键，顺序查找第一个 `unique` 索引，并执行相同的位置查找。
3. 仅当前两类索引均不存在时才调用 `ShowIndex`。
4. 只考虑 `SeqInIndex == 1` 的行；仅当 `Cardinality` 严格大于当前最大值时更新候选，因此相同基数保留 `SHOW INDEX` 中先出现的索引。
5. 普通索引路径按 `ColumnInfo.name.lower` 与 `ColumnName.to_lowercase()` 匹配；最优候选列不存在时返回错误；没有正基数候选时返回 `Ok(None)`。

需要注意，主键或唯一索引存在但 `columns` 为空、或者首位置越界时，函数直接返回 `Ok(None)`，不会继续尝试较低优先级索引。这是当前源码的真实短路行为。

`FindAllIndex` 先收集索引引用，再以类别键 `0/1/2` 稳定排序，所以同类别保持原始声明顺序。`FindAllColumnWithIndex` 依次展开排序后的每个索引，只接纳有效位置，并以列的原始名称作为 `HashSet` 键去重。`SelectUniqueOrderKey` 先找第一个主键；没有主键时反向寻找最后一个唯一索引；没有候选时生成覆盖全部列的位置序列，之后过滤越界位置并从实际列生成名字与引用。

## 数据与状态

本文件没有全局可变状态。所有算法状态都是调用栈内局部值：`ShowIndex` 的结果向量、最大基数与当前候选，`FindAllColumnWithIndex` 的已访问列名集合，以及 `SelectUniqueOrderKey` 的位置和列向量。

`IndexedTable` 维护两组必须由构造方保持一致的数据：`TableIndexInfo.columns` 中的每个 `usize` 应指向 `IndexedTable.columns`。当前实现没有构造期校验；不同入口对无效位置采取不同策略：索引列收集和唯一排序键选择会静默过滤，主键/唯一键的适合列选择会得到 `None`。列名同时包含 `original` 与 `lower` 形式：普通索引匹配使用小写形式，去重和输出使用原始形式。

## 依赖与调用关系

- crate 边界：[`Cargo.toml`](Cargo.toml) 指定库入口为 `lib.rs`，本文件唯一直接的外部 crate 依赖是 `astersql-infoschema::infoschema::ColumnInfo`。
- 同 crate 下游：`ShowIndex` 调用 [`common.rs`](common.rs) 的 `TableName` 进行标识符引用，并调用 [`interface.rs`](interface.rs) 的 `QueryExecutor::QueryContext`；解析使用 `QueryResult`、`Value` 和 `DbError`。
- 文件内调用边：`FindSuitableColumnWithIndex -> ShowIndex -> required_text -> text`；`FindAllColumnWithIndex -> FindAllIndex`。`SelectUniqueOrderKey` 不调用其他本文件函数。
- 已验证的 Rust 上游：[`index_test.rs`](index_test.rs) 直接调用 `ShowIndex`、`FindSuitableColumnWithIndex`、`FindAllIndex` 和 `FindAllColumnWithIndex`。仓库级 Rust 搜索未发现测试外调用；`SelectUniqueOrderKey` 当前也没有直接 Rust 调用证据。
- Go 上游不能当作 Rust 接线证据。RustCodeGraph 显示 Go `index.go` 被 `pkg/util/mviewutil/util.go`、`pkg/util/regionsplit/split_handle.go` 和 `pkg/util/rowDecoder/decoder_test.go` 使用；这些只说明 Go 包在整体应用中的既有位置，不证明 Rust API 已接入对应链路。

## 错误处理与边界

数据库执行错误由 `ShowIndex` 原样通过 `?` 传播。模块自行构造的 `DbError` 均使用 `code = 0`、`sql_state = None`，消息区分结果缺列、结果值为 `NULL`、整数格式无效以及索引指向未知列。

`required_text` 对列名采用 ASCII 大小写不敏感匹配，但普通索引的列名关联通过 Unicode `to_lowercase` 后与 `CiString.lower` 比较。`Value::Bytes` 使用 `from_utf8_lossy`，非法 UTF-8 会被替换而不会报错。`Non_unique` 只有文本恰为 `"1"` 才为真；其他文本都被当作假。`Seq_in_index` 与 `Cardinality` 接受完整 `i64` 域，包括负数；测试明确覆盖了负值解析。

空索引列表下，`FindSuitableColumnWithIndex` 仍会查询 `SHOW INDEX`；若没有可选的正基数首列则返回 `None`。空列列表下，`SelectUniqueOrderKey` 返回两个空向量。越界索引位置会被若干入口过滤或转成 `None`，不会 panic；但这也可能掩盖元数据不一致，扩展时应决定是否需要统一为显式错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。`QueryExecutor: Send + Sync` 允许调用方跨线程共享执行器，但 `ShowIndex` 自身是同步调用，并假定执行器一次性返回拥有所有行的 `QueryResult`；与 Go 版本的 `sql.Rows` 不同，这里没有游标 `Close`、逐行 `Next` 或结束后的行错误检查生命周期。

返回的 `&ColumnInfo` 和 `&TableIndexInfo` 均借用 `IndexedTable`，因此 Rust 类型系统保证这些引用不超过表对象的生命周期。`ShowIndex` 返回拥有数据的 `Vec<IndexInfo>`，与执行器返回的结果无借用关系。独立测试中的 `IndexFixture` 用 `Mutex<QueryResult>` 模拟 `Send + Sync` 执行器；这只是测试夹具的同步机制，不是生产实现要求调用方加锁的证据。

## 与 Go 版本的对应关系

Rust 的五个公开函数与 [`index.go`](index.go) 中同名函数逐项对应，选择优先级、稳定排序、最高基数只取复合索引首列、同基数保留先出现项，以及“无主键时最后一个唯一索引覆盖之前候选”的语义均被保留。`FindAllColumnWithIndex` 也遵循 Go 的意图，按列名而非索引位置去重。

主要表示差异如下：

- Go 直接消费 `model.TableInfo`、`model.IndexInfo` 和 `model.ColumnInfo`；Rust 引入 `IndexedTable`、`TableIndexInfo` 轻量适配层，仅复用 infoschema 的 `ColumnInfo`。
- Go `ShowIndex` 接收 `context.Context`，流式扫描 `sql.Rows` 并负责 `Close`；Rust `QueryExecutor` 接口没有 context 参数，直接返回内存中的 `QueryResult`。
- Go 的字段扫描来自 `ScanRow`；Rust 按结果列名定位，并显式拒绝缺列与 `NULL`。Rust 字节转文本为有损 UTF-8，未必与 Go 的原始 `[]byte -> string` 在非法字节上完全等价。
- Go 在 `FindAllColumnWithIndex` 中假定索引列总能按名找到，随后直接解引用；Rust 用位置索引并过滤无效位置，异常元数据下更宽松。
- Go 测试 [`index_test.go`](index_test.go) 只覆盖索引/列顺序；Rust 测试在保留该意图之外，还覆盖查询映射、基数选择、缺列错误、最佳索引列不存在、负整数解析和按列名去重。

## 扩展指南

若要把本模块接入生产 Rust 路径，优先在调用边界把完整表元数据可靠地转换为 `IndexedTable`，并验证所有 `TableIndexInfo.columns` 位置；不要在本文件中复制另一套完整 `TableInfo`。新增执行器能力时应保持 `QueryExecutor` 抽象，并明确是否需要引入取消/context、流式行和资源关闭语义。

修改 `SHOW INDEX` 解析时，应集中调整 `required_text`/`text` 或 `ShowIndex`，同步在 [`index_test.rs`](index_test.rs) 增加缺列、`NULL`、类型与数值格式用例。修改选择优先级时，应同时覆盖：多个主键/唯一标记的声明顺序、复合索引非首列、基数相等、基数为零/负数、列名大小写，以及无效列位置。修改唯一排序键逻辑时，应为 `SelectUniqueOrderKey` 新增独立测试；当前测试没有直接覆盖该函数。

兼容风险主要来自改变 Go 对齐的稳定顺序或最后唯一索引语义；正确性风险来自静默过滤无效位置、列名大小写/重复名，以及把非 `"1"` 的 `Non_unique` 值解释为唯一；性能风险主要是 `ShowIndex` 全量物化结果和每行线性查找列名，不过通常索引行数很小。Rust 单元测试必须继续放在独立的 [`index_test.rs`](index_test.rs)，不要内嵌进生产源文件。

## 验证依据

- RustCodeGraph `status`：索引可用，共 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 15 个符号。
- RustCodeGraph `node --file pkg/util/dbutil/index.rs`：读取了 226 行完整实现，核对所有结构、辅助函数、公开函数及文件内调用关系。
- RustCodeGraph 文件级读取：[`lib.rs`](lib.rs)、[`interface.rs`](interface.rs)、[`common.rs`](common.rs)、[`index.go`](index.go)、[`index_test.rs`](index_test.rs) 和 [`index_test.go`](index_test.go)。
- Cargo 证据：[`Cargo.toml`](Cargo.toml) 中的 package 名、库入口、Go 包迁移元数据和 `astersql-infoschema` 依赖。
- 调用点检查：RustCodeGraph 的文件使用关系与仓库 `rg` 精确搜索共同确认，公开函数当前无测试外 Rust 调用；Go 文件的使用者仅作为 Go 侧架构背景记录。
- 测试证据：`indices_and_columns_follow_primary_unique_normal_order` 验证类别排序和列顺序；`show_index_and_cardinality_selection_preserve_go_order_and_errors` 验证查询映射、最高基数和缺列错误；`cardinality_selection_errors_when_the_best_index_column_is_missing` 验证未知列错误；`show_index_accepts_go_int_domain_and_columns_deduplicate_by_name` 验证负整数与按名称去重。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文恰有 11 个固定二级章节，并人工复核源码链接、当前接线状态和未覆盖边界均有明确依据。
