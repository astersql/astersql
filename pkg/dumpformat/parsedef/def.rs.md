# `pkg/dumpformat/parsedef/def.rs`

## 文件定位

源文件 [`def.rs`](./def.rs) 是 `astersql-dumpformat-parsedef` crate 的数据定义实现，提供与 Go `pkg/dumpformat/parsedef` 对齐的行容器 `Row`、日志数组编码抽象 `ArrayEncoder`，并公开再导出 SQL 通用值类型 `Datum`。[`lib.rs`](./lib.rs) 以私有 `mod def` 装载本文件，再用 `pub use def::*` 将全部公共项暴露到 crate 根，因此外部应从 `astersql_dumpformat_parsedef::{Row, Datum, ArrayEncoder}` 使用这些类型，而不是依赖私有模块路径。

[`Cargo.toml`](./Cargo.toml) 声明本 crate 仅直接依赖 `astersql-types`，并用 `package.metadata.porting.go-package = "pkg/dumpformat/parsedef"` 记录 Go 对照包。根 `Cargo.toml` 又以 `facade_dumpformat_parsedef` 引入它，`pkg/lib.rs` 将其再导出为 `pkg::dumpformat::parsedef` 门面。

当前 Rust 接线需要谨慎区分“依赖已声明”和“类型已在热路径使用”：`pkg/dumpformat/parquetfile/Cargo.toml` 与 `pkg/executor/importer/Cargo.toml` 都声明了该 crate，但仓库内活跃 Rust 源码没有直接构造本文件的 `Row`。Parquet 的当前实现使用 `parser.rs::ParsedRow` 和 `file_parser.rs` 中的 `astersql_lightning_mydump::Row`；`parser.rs` 前半段保留的 `parsedef::Row` 代码是整段注释掉的迁移草稿，不是运行路径。本文件目前主要承担 Go 兼容的数据契约、公共门面和后续接线基础。

## 核心职责

- 定义一行数据的三个共享属性：逻辑行号 `RowID`、按列排列的 `Vec<Datum>` 和估算长度 `Length`。
- 通过 `pub use types_crate::datum::Datum` 固定行值所使用的 SQL 值容器，并让调用方无需额外导入 `astersql-types` 即可从本 crate 获取同一类型。
- 抽象 Go `zapcore.ArrayEncoder` 所需的最小能力 `AppendString`，使日志后端可以自行提供适配器。
- 实现 `Row::MarshalLogArray`，按列顺序把每个 `Datum::String()` 的结果交给编码器，复刻 Go 的日志数组语义。
- 提供 `Clone` 和 `Default`，支持行对象复制以及与 Go 结构体零值相同的初始状态。

本文件不读取文件、不解析 CSV/Parquet、不计算 `Length`、不递增 `RowID`，也不管理行缓冲池。注释中的“对象被复用时递增行号”是调用方约定；字段均为公开字段，本类型本身不会强制该不变量。

## 主要符号

- `pub use types_crate::datum::Datum`：公开再导出 `astersql-types` 的 Datum。`Row::Row` 和 `MarshalLogArray` 都使用这一确切类型；它不是本文件新定义的包装器。
- `pub trait ArrayEncoder`：日志数组编码器的最小接口。唯一方法为 `fn AppendString(&mut self, value: &str)`；方法名刻意保留 Go 风格，因此文件使用 `#![allow(non_snake_case)]`。
- `pub struct Row`：共享行容器，派生 `Clone` 与 `Default`，但未派生 `Debug`、相等比较或序列化 trait。
- `Row::RowID: i64`：调用方维护的行标识。默认值为 `0`；约定在读取下一行时递增，但类型允许任意设置或回退。
- `Row::Row: Vec<Datum>`：按列顺序保存的值数组。默认是空向量；克隆 `Row` 会克隆向量及其中各 Datum。
- `Row::Length: isize`：行内容的估算长度。使用平台指针宽度，对齐 64 位目标上 Go `int` 的范围；它不是由 `MarshalLogArray` 计算的精确序列化字节数。
- `Row::MarshalLogArray(&self, &mut dyn ArrayEncoder) -> Result<(), Infallible>`：唯一方法。它只读借用行、可变借用编码器，逐元素调用 `Datum::String()` 和 `AppendString`，最后返回 `Ok(())`。

文件没有模块级常量、枚举、条件编译项、异步函数或私有辅助函数。测试条件编译位于 `lib.rs`，而不是本生产文件。

## 执行流程

`MarshalLogArray` 的完整流程很短且确定：

1. 调用方准备一个 `Row` 和实现了 `ArrayEncoder` 的可变编码器。
2. 方法按 `Row.Row` 的向量顺序借用每个 Datum，不读取或改变 `RowID` 与 `Length`。
3. 对当前 Datum 调用 `Datum::String()` 得到其 Go 风格字符串表示。
4. 将该字符串以 `&str` 传给 `encoder.AppendString`；编码器决定是复制、格式化还是输出该值。
5. 所有列处理完后返回 `Ok(())`。空行不会调用编码器，也仍返回成功。

关键不变量是输出次数等于 `Row.len()`，输出顺序等于列顺序。该方法不加列名、不做 JSON/zap 外层数组装配、不转义字段，也不把 `RowID` 或 `Length` 写入日志；这些策略属于具体日志编码器。

行数据本身的预期生产流程可从 Go Parquet 调用方验证：`parser.go::ReadRow` 先递增 `lastRow.RowID`、清零 `Length`，读取并赋值 `Row`，再用 `estimateRowSize` 更新 `Length`；`LastRow` 返回当前行，`RecycleRow` 将其中的值向量归还缓冲池。当前 Rust Parquet 活跃路径实现了相似概念，但使用其他行类型，不能视为对本 `Row` 的直接调用。

## 数据与状态

`Row` 是普通的拥有型结构体。它拥有 `Vec<Datum>`，没有引用参数和显式生命周期；`Clone` 产生独立向量，`Default` 产生 `RowID = 0`、空 `Row`、`Length = 0`。独立测试 `row_zero_value_and_clone_match_go_struct_behavior` 明确验证这些状态及克隆后的值内容。

三个字段之间没有自动一致性检查：`Length` 可以与 `Row` 实际内容不符，`RowID` 也可以与读取位置不符。尤其 `Length` 只是上游估值，既不包含明确单位类型，也不由本文件重算；消费者若将其用于内存计量或进度控制，必须沿用生产者的计算约定。

`MarshalLogArray` 不缓存字符串。每次调用都会为每个 Datum 执行一次 `String()`；具体是否分配由 Datum 实现决定。传给 `AppendString` 的借用只保证在该次调用期间有效，因此编码器若要在返回后保存内容，必须像测试中的 `RecordingEncoder` 一样复制它。

`Result<(), Infallible>` 表示当前 trait 边界没有编码失败通道。返回类型保留了 Go `error`/数组 marshaler 风格的调用形状，但在安全 Rust 中无法构造 `Infallible` 错误。

## 依赖与调用关系

公共暴露链为：

`types_crate::datum::Datum` → `def.rs` 再导出及 `Row` 字段 → `parsedef/lib.rs` 再导出 → 根 `pkg/lib.rs::dumpformat::parsedef` 门面。

内部调用边为：

`Row::MarshalLogArray` → `Datum::String` → `ArrayEncoder::AppendString`。

RustCodeGraph 的精确文件节点确认 `def.rs` 有 5 个索引符号，并把 `AppendString` 标为 `MarshalLogArray` 的被调用项。由于仓库中存在多个 `Row`、`ArrayEncoder` 和 `MarshalLogArray` 同名符号，宽泛 `callers/callees` 查询混入了无关模块；调用关系因此用目标文件节点、唯一实现体和真实导入点交叉核验。

依赖/消费边界如下：

- 本 crate 唯一直接 Cargo 依赖是 `astersql-types`，用于 Datum。
- 根 workspace 与 `pkg/lib.rs` 提供公共门面再导出。
- Parquet 与 executor importer 的 Cargo manifest 声明依赖本 crate，但当前活跃 `.rs` 文件未直接引用其公共项。
- `pkg/dumpformat/parquetfile/parser.rs` 的注释迁移稿展示了预期的 `parsedef::Row` 接线；当前可执行实现则改用 `ParsedRow` 或 Lightning mydump 的 `Row`，所以不能把这段注释当作调用者。
- 当前直接行为验证者是 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，它实现 `RecordingEncoder` 并调用 `Row::MarshalLogArray`。

## 错误处理与边界

本文件没有可恢复错误分支：`ArrayEncoder::AppendString` 不返回 `Result`，`MarshalLogArray` 的错误类型是 `Infallible`。这意味着编码器实现不能通过该 trait 把 I/O、格式化或容量错误传播给调用方；需要可失败输出的后端必须在适配层预先缓冲、内部记录错误，或另行扩展接口。

主要边界包括：

- 空 `Row.Row` 合法，序列化时零次调用编码器并返回 `Ok(())`，由 `marshal_log_array_accepts_an_empty_row` 覆盖。
- Datum 的文本语义完全由 `Datum::String()` 决定。本文件不区分 NULL、数值、二进制、时间或字符串，也不追加类型标签；日志文本不应被当作可逆的 Datum 编码。
- `MarshalLogArray` 在编码过程中若具体 `AppendString` 实现 panic，panic 会直接传播；trait 没有捕获机制。
- 字段公开且没有构造器，调用方可以构造负 `RowID`、负 `Length` 或任意组合；本文件不验证这些状态。
- `Length: isize` 在不同目标架构宽度不同。迁移测试只验证 64 位目标可容纳大于 `i32::MAX` 的值；不能将原生内存布局作为跨平台或跨语言协议。
- `#![allow(non_snake_case)]` 允许公共 API 保留 Go 名称，但新增纯 Rust API 不应无条件继续扩大这一命名例外。

相关测试严格放在独立文件 `migration_aster_unit_test.rs`，没有嵌入生产源码，符合仓库的 Rust 测试组织约束。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。`Row` 拥有其值向量，离开作用域时按普通 Rust 所有权规则释放；没有自定义 `Drop`。`ArrayEncoder` 由调用方借入，`MarshalLogArray` 不取得其所有权，也不会关闭或刷新它。

方法使用 `&self`，所以序列化不会改变行；编码器使用排他的 `&mut dyn ArrayEncoder`，单次调用期间不能由安全 Rust 同时从其他路径可变访问同一实例。方法本身不保证跨线程共享：`ArrayEncoder` trait 没有 `Send`/`Sync` 上界，`Row` 能否跨线程取决于其字段（特别是 Datum）的自动 trait 实现，本文没有显式承诺。

若调用方复用 `Row`，正确生命周期是先消费或复制上一行需要保留的数据，再替换/回收 `Row` 向量并更新 `RowID`、`Length`。这一复用策略由上游 parser/pool 负责；`Clone` 可用于保留独立快照，但会复制整行值，可能增加分配和内存占用。

## 与 Go 版本的对应关系

直接对照文件是 [`def.go`](./def.go)。字段逐项对应：Go `RowID int64` ↔ Rust `i64`，Go `Row []types.Datum` ↔ Rust `Vec<Datum>`，Go `Length int` ↔ Rust `isize`。两边的日志方法都按列遍历，调用 Datum 的 `String`，再调用数组编码器的 `AppendString`，最后成功返回。

Rust 为保持对照特意保留 `RowID`、`Row`、`Length`、`MarshalLogArray` 与 `AppendString` 的 Go 式名称，并通过文件级 lint allowance 接受它们。语义差异主要是语言边界：

- Go `Row` 的零值天然可用；Rust 通过 `Default` 明确复现该状态。
- Go 结构体按值返回时 slice 头被复制、底层数组通常共享；Rust `Clone` 会克隆 `Vec<Datum>` 内容，独立测试只验证值相同，不主张共享缓冲。
- Go 的 `zapcore.ArrayEncoder.AppendString` 返回空值，Rust trait 同样不提供单元素失败；Go `MarshalLogArray` 返回 `error` 且固定为 `nil`，Rust对应为永不失败的 `Result<(), Infallible>`。
- Go `int` 与平台字宽相关，Rust 选择 `isize` 对齐这一属性；二者的内存布局仍没有 FFI 保证。
- Go 类型直接接入现有 Parquet parser 与多处 Go 消费链。Rust 类型目前已有公共导出和迁移测试，但活跃 Parquet 实现尚未直接使用它，因此迁移状态应描述为“定义完成、运行链路未统一”，而不是完全替代 Go 行类型。

## 扩展指南

- 增加或修改 `Row` 字段时，应先核对 `def.go` 的结构与所有实际消费者；如果是逐提交 Go 对齐任务，只移植对应增量和不可缺少的局部接线，不顺带重构 Parquet/mydump 的完整行体系。
- 调整日志语义时，修改点是 `MarshalLogArray` 与 `ArrayEncoder`。必须在独立测试文件中覆盖空行、多种 Datum、顺序和错误模型；不要把单元测试放回 `def.rs`。
- 若日志后端需要传播错误，不应把 `Infallible` 简单替换成某个具体错误而忽略公共 API 兼容。可考虑为 trait 增加关联错误类型或新建可失败适配接口，并同步根门面、调用方及 Go 对照语义。
- 若要把当前 Parquet/importer 活跃路径统一到本 `Row`，需要同时评估 Datum 类型差异、字段命名、`Length` 类型、行池所有权与 `RecycleRow` 行为；Cargo 依赖已经存在，但这不等于转换可零成本完成。
- 若改变 `Length` 的含义，应明确单位、是否包含容器开销以及溢出规则，并同步生产它的解析器、消费它的内存/进度逻辑和迁移测试。只改字段类型会产生兼容与跨平台风险。
- 性能敏感点是 `Clone` 的整行复制和 `MarshalLogArray` 对每列执行字符串转换。扩展时避免在循环中增加不必要的二次格式化或额外克隆，并用独立基准/测试证明需要的新行为。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter pkg/dumpformat/parsedef` 定位 `def.rs`、Go 对照、crate 入口与独立测试；`node --file pkg/dumpformat/parsedef/def.rs` 读取全部 62 行并确认 5 个索引符号；`query MarshalLogArray`、`query ArrayEncoder`、`query Row --kind struct` 用于发现并消歧同名符号；文件节点给出的直接内部边为 `MarshalLogArray` 调用 `AppendString`。
- 目标与 crate 边界：`pkg/dumpformat/parsedef/def.rs`、`lib.rs`、`Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`。
- Go 对照与真实 Go 消费：`pkg/dumpformat/parsedef/def.go`、`pkg/dumpformat/parquetfile/parser.go::{ReadRow, LastRow, RecycleRow, SetRowID}`、`parser_test.go` 的行号和值断言。
- Rust 接线现状：`pkg/dumpformat/parquetfile/Cargo.toml`、`pkg/executor/importer/Cargo.toml`、`pkg/dumpformat/parquetfile/parser.rs::{ParsedRow, Parser}`、`file_parser.rs::ImportParser`。源码搜索确认活跃 Rust 实现没有直接引用 `astersql_dumpformat_parsedef`；`parser.rs` 中的 `parsedef::Row` 仅存在于注释迁移稿。
- 独立 Rust 测试：`pkg/dumpformat/parsedef/migration_aster_unit_test.rs` 覆盖默认值与克隆、按序日志输出、空行和 64 位 `Length`。本任务是纯文档分析，按计划未运行 Cargo；这些测试被阅读作为既有行为证据，没有声称本轮重新执行通过。
- 人工复核重点：公共再导出、三字段状态、不可失败的日志调用链、当前 Rust 消费缺口、Go/Rust clone 与整数宽度差异、独立测试位置，以及本文件不拥有解析、并发或资源管理逻辑的边界。
