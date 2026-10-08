# `pkg/server/internal/column/convert.rs`

## 文件定位

本文件属于 Cargo crate `astersql-server-internal-column`，crate 根由 [`lib.rs`](lib.rs) 以私有模块 `convert` 装配，再通过 `pub use convert::*` 对外导出。它位于规划器结果字段与 MySQL 服务端协议元数据之间：输入是 `planner-resolve` 提供的 `resolve::ResultField`，输出是同 crate 的 `Info`（对应 MySQL `ColumnDefinition41` 所需字段）。crate 边界和依赖见 [`Cargo.toml`](Cargo.toml)，其中 `planner-resolve`、`parser-charset`、`parser-mysql`、`meta-model` 与 `types-metadata` 是本转换直接或经 `lib.rs` 再导出使用的来源。

当前 Rust 应用中至少有两条直接接线：[`pkg/server/internal/resultset/resultset.rs`](../resultset/resultset.rs) 的 `TidbResultSet::Columns` 把查询 `RecordSet.Fields()` 逐列转换并缓存；[`pkg/server/runtime.rs`](../../runtime.rs) 的 `protocol_column` 转换后再组装运行时 `ColumnInfo`。因此该文件只负责“推导协议列元数据”，不负责编码协议包、执行查询或维护结果集缓存。

## 核心职责

- `ConvertColumnInfo` 将规划/解析阶段的列身份信息（库名、表别名、列别名、原始名称）复制到协议侧 `Info`。
- 将内部字段类型、标志、字符集和默认值映射为协议字段；其中字符集名称通过 `mysql::CharsetNameToID` 转为数值 ID。
- 按 MySQL/TiDB 兼容规则计算 `ColumnLength`：DECIMAL 计入符号和小数点，字符串/ENUM/SET 按字符集最大字节数放大，未指定长度则使用类型默认值。
- 规范化未指定的小数精度，并将 `VARCHAR` 对外报告为旧客户端兼容的 `VAR_STRING`。

它不修改传入的 `ResultField` 或 `ColumnInfo`，也不验证 SQL 语义；返回值是新建的、拥有自身字符串和默认值数据的 `Info`。

## 主要符号

`pub fn ConvertColumnInfo(field: &resolve::ResultField) -> Info` 是文件唯一的生产符号，也是公开 API。参数借用 `ResultField`，返回拥有所有字段的 `Info`。函数内部首先要求 `field.column: Option<Rc<model::ColumnInfo>>` 为 `Some`；随后读取 `ColumnInfo` 的 `GetFlag`、`GetCharset`、`GetType`、`GetDefaultValue`、`GetFlen` 和 `GetDecimal`。

输出类型 `Info` 定义在 [`column.rs`](column.rs)；本函数填充 `Name`、`OrgName`、`Table`、`OrgTable`、`Schema`、`Flag`、`Charset`、`Type`、`DefaultValue`、`ColumnLength` 和 `Decimal`。`Info::Dump`/`DumpWithDefault` 才负责后续 `ColumnDefinition41` 字节编码，本文件不会直接写网络缓冲区。

文件没有模块级常量、类型、trait、`impl`、异步函数或条件编译项。命名保留 Go 风格大驼峰，以便与同路径 Go API 对齐；crate 根允许 `non_snake_case`。

## 执行流程

1. 从 `field.column` 取得底层列；缺失时以固定消息 panic，因为协议元数据转换要求规划结果已经带有列描述。
2. 用结果字段别名和列属性初始化 `Info`：`Name` 取 `column_as_name`，`OrgName` 取原始列名，`Table` 取 `table_as_name`，`Schema` 取 `db_name`；标志、字符集 ID、类型和默认值来自列本身。
3. 若 `empty_org_name` 为真，清空 `OrgName`；若 `field.table` 存在，则将其真实表名写入 `OrgTable`，否则保留默认空串。
4. 计算显示长度。若 `GetFlen()` 已指定，先转为 `u32`：
   - `TypeNewDecimal` 总是为负号加一；当 `GetDecimal() > DefaultFsp` 时再为小数点加一。
   - 字符串类型以及 `TypeEnum`、`TypeSet` 按 `charset::GetCharsetInfo(...).Maxlen` 放大；未知字符集使用倍数 4。
   - 其他类型保持原始 `flen`。
5. 若 `flen == UnspecifiedLength`，直接采用 `mysql::GetDefaultFieldLengthAndDecimal(type).0`；此分支不再做字符集乘法或 DECIMAL 加宽。
6. 计算 `Decimal`。明确值直接转为 `u8`；未指定时，`TypeDuration` 使用 `DefaultFsp`，其他类型使用协议哨兵 `NotFixedDec`。
7. 若类型仍是 `TypeVarchar`，改写为 `TypeVarString`，最后返回 `Info`。

## 数据与状态

函数是无持久状态的纯转换器。它只读取 `ResultField` 及其 `Rc<ColumnInfo>` / 可选 `Rc<TableInfo>`，并通过克隆名称、默认值等字段构造独立的 `Info`；没有全局变量、缓存或可变共享状态。

关键数据不变量如下：

- `field.column` 必须存在；这是调用方提供完整规划结果的前置条件。
- `Name`/`Table` 表示结果集可见别名，`OrgName`/`OrgTable` 表示原始对象身份；`empty_org_name` 显式覆盖原始列名。
- `ColumnLength` 表示协议显示宽度而非 Rust 内存大小。字符串宽度按最大编码字节数扩展，以避免旧客户端按过小宽度截断显示。
- 长度加法和乘法使用 `wrapping_add` / `wrapping_mul`，显式保持 Go `uint32` 的环绕溢出语义。输入 `isize` 先转 `u32`，调用方应保证除 `UnspecifiedLength` 外的 `flen` 合法。
- `Decimal` 和类型最终压缩为协议字节；这里沿用上游元数据约束，不进行范围错误检查。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 与源码共同确认：

- `TidbResultSet::Columns`（[`resultset.rs`](../resultset/resultset.rs)）在本地缓存和预编译语句缓存均未命中时，对 `recordSet().Fields()` 调用 `ConvertColumnInfo`，将结果包装为 `Arc<Info>`，再同时写入语句缓存和结果集缓存。
- `protocol_column`（[`pkg/server/runtime.rs`](../../runtime.rs)）调用转换函数后，把 `Info` 拆为运行时协议 `ColumnInfo`，并继续应用 `dumpType`、`DumpFlag` 与默认值渲染。
- `canonical_convert_column_info_preserves_mysql_display_width_rules`（[`pkg/server/driver_tidb_test.rs`](../../driver_tidb_test.rs)）是直接调用该函数的跨 crate Rust 回归测试。

下游依赖通过 crate 根再导出：`resolve::ResultField` 来自 `planner-resolve`；`Info` 来自本 crate `column.rs`；`charset::GetCharsetInfo` 来自 `parser-charset`；MySQL 类型、字符集 ID、默认长度与 `NotFixedDec` 来自 `parser-mysql`；`types::UnspecifiedLength`、`DefaultFsp` 及 `IsString` 来自类型相关 crates。函数没有 I/O 调用，其结果之后才由 `Info::Dump` 或运行时协议层编码。

## 错误处理与边界

该 API 不返回 `Result`。唯一显式失败是 `field.column == None` 时的 `expect` panic：`ResultField.column must be present when converting protocol metadata`。这表示缺列不是可恢复的客户端输入错误，而是上游接线违反内部契约。

字符集查询失败不会传播错误；未知字符集使用最大字节倍数 4，与 Go 回退路径一致。未指定 `flen` 使用类型默认长度，保证协议不会输出无用的未指定值。未指定 decimal 对 DURATION 与其他类型分别回退到 `DefaultFsp` 和 `NotFixedDec`。`field.table == None` 是正常边界，只会让 `OrgTable` 为空；`empty_org_name` 也是正常的表达式列/隐藏原名路径。

整数转换不做饱和或报错：长度运算刻意环绕；`decimal as u8` 与 `flen as u32` 依赖上游字段元数据的合法范围。扩展代码不能把这些转换误写成 debug 模式可 panic 的普通 `+` / `*`，否则会偏离 Go 行为。

## 并发与资源生命周期

本函数不创建线程、异步任务、锁、通道、事务或外部资源。输入只读借用在函数返回时结束；输出拥有克隆后的字符串和默认值，不借用输入，因此可由调用者缓存。

并发与缓存生命周期属于调用方：`TidbResultSet::Columns` 将每个结果包装为 `Arc<Info>` 并缓存，而本函数本身不共享 `Rc`、`Arc` 或内部引用。它读取的 `ResultField` 使用 `Rc` 表示上游单线程共享元数据，但转换过程不会改变引用计数指向的内容（除临时借用外）。因此新增逻辑应保持无副作用，避免把会话状态、锁或懒加载缓存引入这一低层转换边界。

## 与 Go 版本的对应关系

同路径 [`convert.go`](convert.go) 是直接语义基准。Rust 实现逐分支复刻 Go `ConvertColumnInfo`：字段来源一致；`EmptyOrgName` 和可选表处理一致；DECIMAL 加宽、字符串/ENUM/SET 字符集乘数、未知字符集乘 4、未指定长度默认值、DURATION 默认精度和 `VARCHAR -> VAR_STRING` 均一致。

语言层差异主要有三点：

- Go 输入是指针且直接解引用 `fld.Column`；Rust 用 `Option<Rc<_>>` 表达可缺失，并通过 `expect` 明确同等的内部失败契约。
- Go 返回 `*Info`；Rust 返回拥有值 `Info`，由调用方按需包装为 `Arc`。
- Go 的 `uint32` 运算自然环绕；Rust 明确使用 `wrapping_add` / `wrapping_mul`，避免 debug 构建溢出 panic。

Go [`pkg/server/driver_tidb_test.go`](../../driver_tidb_test.go) 的 `TestConvertColumnInfo` 验证 BIT/TINY/YEAR 等基础长度，以及一组 MySQL 类型在未指定 `flen` 时采用非零默认长度。Rust [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步逐项验证别名与原名、真实表名、默认值、utf8mb4 放大、DECIMAL 加宽、未知字符集回退、DURATION 精度及溢出环绕；[`driver_tidb_test.rs`](../../driver_tidb_test.rs) 另有公开 crate 边界的 VARCHAR 兼容回归。

## 扩展指南

新增或调整协议元数据规则时，最可能修改的入口只有 `ConvertColumnInfo`，但应先判断规则属于“元数据推导”还是“协议编码”：前者放在本文件，后者应放在 `column.rs` 的 `Info::Dump`、`dumpCharset`、`dumpLength`、`dumpType` 或 `DumpFlag`。结果集缓存策略则属于 `internal/resultset`，不应混入本函数。

安全扩展需保持以下同步项：

- 先与 `convert.go` 的对应分支核对，保留类型判断顺序、默认值和整数语义；若有意产生 Rust/Go 差异，必须明确记录兼容原因。
- 在独立测试文件 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 增加边界用例；不得把测试内嵌到 `convert.rs`。若涉及公开服务端接线，再同步 [`driver_tidb_test.rs`](../../driver_tidb_test.rs)；同时检查 Go `TestConvertColumnInfo` 的同类用例。
- 新类型若是字符串族，要决定是否参加字符集 `Maxlen` 放大以及是否需要类型兼容映射；新 decimal/时间类型要明确未指定精度策略。
- 修改长度算法要评估老客户端截断兼容、默认长度过大导致的溢出、未知字符集回退和 `u32` 环绕性能/正确性风险。
- 若改变 `Info` 字段含义，还需审查 `column.rs` 的编码以及 `runtime.rs::protocol_column` 的二次映射，避免转换值被再次改写。

## 验证依据

事实核验于 2026-10-08 完成，未运行 Cargo（本任务为纯文档分析）。依据包括：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录的 `convert.rs`、`column.rs`、`lib.rs` 及测试均已索引。
- RustCodeGraph `query/node`：精确定位 `pkg/server/internal/column/convert.rs::ConvertColumnInfo`，确认函数完整源码；调用 trail 指向 `TidbResultSet::Columns` 和 `canonical_convert_column_info_preserves_mysql_display_width_rules`。对 `resultset.rs`、`driver_tidb_test.rs`、`runtime.rs` 与 `column.rs` 的按文件节点查询补充了调用及输出消费证据。
- 读取的生产与装配文件：[`convert.rs`](convert.rs)、[`column.rs`](column.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`resultset.rs`](../resultset/resultset.rs)、[`pkg/server/runtime.rs`](../../runtime.rs) 及对应 resultset Cargo/模块入口。
- Go 对照：[`convert.go`](convert.go)、[`pkg/server/internal/resultset/resultset.go`](../resultset/resultset.go)、[`pkg/server/driver_tidb.go`](../../driver_tidb.go)。
- 测试证据：[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`pkg/server/driver_tidb_test.rs`](../../driver_tidb_test.rs) 与 [`pkg/server/driver_tidb_test.go`](../../driver_tidb_test.go)。
- 人工复核结论：文档区分了转换、编码和缓存职责；每项长度/精度/别名规则均可回溯到上述源码或测试；没有把未运行测试描述为本次已通过。
