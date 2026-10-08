# `pkg/util/dbutil/query.rs`

## 文件定位

本文件是 `astersql-util-dbutil` crate 的结果集扫描辅助模块，由 `pkg/util/dbutil/lib.rs` 通过公开的 `query` 模块暴露。它位于 SQL 执行器产生的驱动无关结果与按行消费者之间：输入类型 `QueryResult` 和 `Value` 定义在 `pkg/util/dbutil/interface.rs`，本文件负责将已物化的行转成二维值数组，或将单行转成以列名索引的原始字节映射。

`pkg/util/dbutil/Cargo.toml` 声明该 crate 的 Go 对照包是 `pkg/util/dbutil`。本文件本身只使用 `std::collections::HashMap` 和 crate 内部的 `QueryResult`/`Value`，没有直接调用数据库驱动，也没有 feature 或条件编译分支。

## 核心职责

- `ScanRowsToInterfaces` 提供“全部行”视图：消费 `QueryResult` 并取出其 `rows`。
- `ScanRow` 提供“按列名访问单行”视图：检查列数后，将每个 `Value` 转换为 `ColumnData { Data, IsNull }`。
- `ColumnData` 把 SQL `NULL` 与非 NULL 空字节区分开；两者的 `Data` 都可以为空，必须查看 `IsNull` 才能区分。
- `format_scanned_float` 对非有限浮点数做数据库扫描风格的文本化，保证正无穷、负无穷和 NaN 分别得到 `+Inf`、`-Inf` 和 `NaN`。

它不负责执行 SQL、逐行拉取驱动结果、关闭 cursor，也不解析列的 SQL 类型；这些能力不在当前 `QueryResult` 抽象和本文件范围内。

## 主要符号

- `fn format_scanned_float(value: f64) -> String`：私有格式化函数。先按 `NaN`/正无穷/负无穷分支处理，其余值使用 `f64::to_string()`。
- `pub fn ScanRowsToInterfaces(rows: QueryResult) -> Vec<Vec<Value>>`：按值接收已物化结果，直接移出 `rows.rows`。它不克隆单元格，但同时丢弃 `QueryResult.columns`。
- `pub struct ColumnData`：公开数据载体，`Data: Vec<u8>` 保存字节，`IsNull: bool` 保存 SQL NULL 标志。派生 `Clone`/`Debug`/`Default`/`Eq`/`PartialEq`；默认值是空字节且 `IsNull == false`，不表示 NULL。
- `pub fn ScanRow(columns: &[String], row: &[Value]) -> Result<HashMap<String, ColumnData>, String>`：单行转换入口。输入是对列名和行值的借用，输出拥有列名、字节数据与 NULL 状态。

公开符号可通过 `astersql_util_dbutil::query::...` 访问；`pkg/util/dbutil/lib.rs` 未将它们再导出到 crate 根。

## 执行流程

`ScanRowsToInterfaces` 的流程只有一步：获取 `QueryResult` 所有权并返回其 `rows` 字段。因为输入已是完整内存结果，这里没有迭代、扫描失败或“下一行”状态。

`ScanRow` 的流程如下：

1. 比较 `columns.len()` 和 `row.len()`；不相等时立即返回包含两个计数的错误字符串。
2. 克隆列名，并与对应的 `Value` 引用按位置 `zip`。
3. 按变体转换：`Null` 变为空字节且 `IsNull = true`；`Bytes` 克隆原字节；`String` 复制 UTF-8 字节；`Bool`/`I64`/`U64` 使用文本形式；`F64` 通过 `format_scanned_float`。所有非 `Null` 变体均设为 `IsNull = false`。
4. 收集为 `HashMap<String, ColumnData>` 并返回。如果列名重复，`HashMap::collect` 保留最后一个同名列；`query_test.rs` 明确锁定了这一行为。

## 数据与状态

本文件没有全局变量、缓存或持久化状态。`ScanRowsToInterfaces` 转移 `QueryResult.rows` 的所有权；`ScanRow` 则从借用输入构造独立输出，会分配映射、克隆列名，并为字节、字符串或标量值分配/复制输出字节。

`ColumnData` 有一个重要不变量：只有 `IsNull` 表示 SQL NULL。`Value::Null` 产生“空 `Data` + `IsNull = true`”，而 `Value::Bytes(Vec::new())` 产生“空 `Data` + `IsNull = false`”；`pkg/util/dbutil/query_test.rs` 同时覆盖了这两种情况。调用者不应用 `Data.is_empty()` 推断 NULL。

## 依赖与调用关系

下游依赖很小：`ScanRowsToInterfaces` 仅访问 `QueryResult.rows`；`ScanRow` 依赖 `Value` 的全部七个当前变体和标准库 `HashMap`；`F64` 分支调用私有的 `format_scanned_float`。因为对 `Value` 进行穷尽匹配，在 `interface.rs` 新增变体时本文件会产生编译期提示，必须明确它的字节表示。

RustCodeGraph 对 `ScanRowsToInterfaces` 和 `ScanRow` 的 callers 查询均未找到生产 Rust 调用者。仓库文本检索也只找到 `pkg/util/dbutil/query_test.rs` 和 `pkg/util/dbutil/common_test.rs` 引用这些函数。因此当前 Rust 实现是可公开调用的移植边界，但没有证据表明它已接入应用生产主链。

Go 侧的直接上游包括 `pkg/util/dbutil/common.go` 中 `GetTidbLatestTSO` 的 `ScanRow(rows)`，以及 `pkg/util/dbutil/index.go` 中 `ShowIndex` 对每行的 `ScanRow(rows)`。这些调用说明该工具的预期位置是“查询完成后、字段解析前”，但不能作为 Rust 已接线的证据。

## 错误处理与边界

`ScanRowsToInterfaces` 是不可失败的所有权投影，因为 `QueryResult` 已经包含完整行。它不检查每行长度是否与 `QueryResult.columns` 匹配，也不保留列名。

`ScanRow` 唯一的显式错误是列数与值数不一致，返回类似 `column count 2 does not match value count 1` 的 `String`。它不使用 crate 的 `DbError`，因为错误是内存结构形状错误，而非 SQL 执行错误。转换过程没有 UTF-8 解码步骤：`Bytes` 保留任意字节，`String` 按自身 UTF-8 编码复制。

空列加空行是合法输入，得到空映射。重复列名不报错，后出现的值覆盖先出现的值，与 Go 版本向 map 逐项赋值的行为一致。非有限浮点的特殊拼写由 `query_test.rs` 验证；对其他浮点数，当前契约就是 Rust `f64::to_string()` 的输出，不应在没有对照证据时宣称与所有 Go SQL 驱动格式完全相同。

## 并发与资源生命周期

所有函数都是无共享可变状态的同步函数，不创建线程、异步任务、锁、通道或事务。`ScanRowsToInterfaces` 消费输入后由 Rust 所有权规则回收未返回的 `columns`；`ScanRow` 只在调用期间借用输入，返回值不引用它们。

与 Go 的 `*sql.Rows` 版本不同，这里没有 cursor 或连接资源，所以本文件不负责 `Close`，也没有“必须在 `Next` 后调用”的运行时状态。未来若改为流式查询接口，必须在新的驱动/迭代器边界单独定义关闭、错误传播与取消责任，不能从当前实现推导这些保证。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/util/dbutil/query.go`：

- Go `ScanRowsToInterfaces(rows *sql.Rows) ([][]any, error)` 从 `database/sql` cursor 读取列名，遍历 `rows.Next()`，并可传播 `Columns`/`Scan` 错误。Rust 版接收已物化的 `QueryResult`，因此只返回其行，没有对应的驱动 I/O 错误。
- Go `ScanRow(rows *sql.Rows)` 获取当前 cursor 的列名，直接扫描为 `[][]byte`，再用 `nil` 判断 NULL。Rust 版由调用者传入对齐的 `columns` 与 `row`，并根据 `Value` 变体显式构造字节和 NULL 标志。
- 两版的 `ColumnData` 都包含字节数据和 NULL 布尔值，同名列写入 map 时都是后值覆盖前值。Rust 输出直接存储 `ColumnData`，Go map 存储 `*ColumnData`；这不改变当前按列名读取的语义。
- Rust 额外显式检查列/值数量，并为 `Value` 标量变体定义文本转换。这是已物化抽象所需的局部接线，不等于完整复刻 `database/sql.Rows` 生命周期。

Rust 测试 `pkg/util/dbutil/query_test.rs` 验证特殊浮点格式、NULL/空字节区分和重复列名覆盖；`pkg/util/dbutil/common_test.rs` 中 `query_scanners_and_mysql_type_classification_match_go` 还验证全行取出、整数文本、NULL 和列数不匹配错误。仓库中没有独立的 `pkg/util/dbutil/query_test.go`；Go 侧的调用语义可由 `common.go` 和 `index.go` 的直接用法复核。

## 扩展指南

- 新增 `Value` 变体时，在 `ScanRow` 的穷尽 `match` 中定义稳定的字节表示，并在独立的 `pkg/util/dbutil/query_test.rs` 增加 NULL 语义、边界值与 Go 对照用例；不要把测试内嵌进生产文件。
- 若需保留重复列名，不能继续使用单值 `HashMap`；必须先明确新 API 的顺序/多值契约，同时考虑 Go 现有“后值覆盖”兼容性。
- 若需让 `ScanRowsToInterfaces` 验证行宽，其返回类型将需从不可失败的 `Vec` 变为 `Result`；这是对调用者可见的 API 变更，应与 `QueryResult` 的构造不变量一起设计。
- 若要接入真实流式 SQL 驱动，应在 `pkg/util/dbutil/interface.rs` 的查询抽象层处理 cursor 与 `DbError`，本文件保持为纯转换层；需要同步测试扫描错误、提前结束、关闭和资源释放。
- 优化性能时首先关注 `ScanRow` 的列名克隆、字节克隆和标量文本分配。借用型输出可减少复制，但会扩大生命周期约束，并可能破坏当前输出与输入独立的简单契约。

## 验证依据

- RustCodeGraph `status` 显示项目索引可用（包含 `pkg/util/dbutil/query.rs`）；`files --filter pkg/util/dbutil` 确认该文件有 7 个索引符号。
- RustCodeGraph `node --file pkg/util/dbutil/query.rs --offset 1 --limit 240` 读取了完整 87 行源码；`query` 查询确认 `ScanRowsToInterfaces`、`ScanRow`、`ColumnData` 和 `format_scanned_float` 的定义。
- RustCodeGraph 对 `ScanRowsToInterfaces`、`ScanRow` 的 `callers` 结果为空；由于其 `callees` 对标准库及字段访问覆盖不完整，下游关系以目标源码的穷尽 `match` 和直接调用为准。
- 已读取 `pkg/util/dbutil/Cargo.toml`、`lib.rs`、`interface.rs`、`query.go`、`query_test.rs`、`common_test.rs` 中相关用例，以及 Go 调用点 `common.go` 和 `index.go`。
- 仓库检索 `ScanRowsToInterfaces|ScanRow|ColumnData` 用于排除未被调用图捕获的 Rust 直接上游，并确认 Go 直接调用点。
- 本任务是纯文档分析，按计划不运行 Cargo；交付时使用任务指定的 11 章结构检查，并人工复核符号、调用关系、Go 差异和扩展风险。
