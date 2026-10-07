# `pkg/format/textrow/textrow.rs`

源文件：[`textrow.rs`](./textrow.rs)

## 文件定位

本文件属于 `astersql-format-textrow` crate，crate 根 `pkg/format/textrow/lib.rs` 通过 `mod textrow; pub use textrow::*;` 将这里的公开项导出。`pkg/format/textrow/Cargo.toml` 指明库入口为 `lib.rs`，并把 Go 包对应关系记录为 `pkg/format/textrow`。

它位于 MySQL 文本结果行的“单值序列化”层：`FormatValueText` 把 `chunk::Row` 中一个非 NULL 单元格变成尚未加协议外框的字节。直接生产入口 `pkg/server/internal/column/column.rs::DumpTextRow` 先处理 NULL，再调用本函数，最后用 `dump::LengthEncodedString` 添加 MySQL 文本协议长度编码。因此本文件不负责整行遍历、NULL 标记或长度前缀，也不负责二进制行协议。

## 核心职责

- `FormatValueText` 根据 `ColumnInfo::Type` 分派到整数、浮点、十进制定点、字节串、时态、Enum/Set、JSON 和向量的格式化路径。
- 字符串类值先用列的 collation 更新 `ResultEncoder`，再按会话结果字符集规则转换；JSON 和 `TypeTiDBVectorFloat32` 固定以 `mysql::DefaultCollationID` 更新数据编码，以保持其文本表示按 UTF-8 语义转换。
- `floatPrec` 决定 Float/Double 是否采用列声明的小数位数；只有表达式结果（`Table` 为空）、`Decimal > 0` 且不是 `mysql::NotFixedDec` 时才覆盖默认精度。
- `AppendFormatFloat` 实现 MySQL 文本输出所需的定点/科学计数法选择、非有限值归零、指数正号删除和尾随零裁剪。
- `format_with_scratch` 借用并回收 `ResultEncoder` 的数值格式化缓冲，避免每次都从零开始分配。

## 主要符号

- `InvalidTypeError`：无字段的公开错误类型，实现 `Display` 和 `std::error::Error`；文本为 `invalid column type for text serialization`。
- `ErrInvalidType: InvalidTypeError`：与 Go 包级哨兵错误同名的常量实例。未知或未支持的 MySQL 类型由 `FormatValueText` 返回它。
- `ColumnInfo`：公开、可克隆的逐列配置，只保留本格式化器需要的 `Table: String`、`Charset: u16`、`Flag: u16`、`Decimal: u8`、`Type: u8`。`Table` 控制浮点精度覆盖，`Charset` 控制字符串类编码，`Flag` 当前用于 `UnsignedFlag`，`Decimal` 用于浮点和 Duration，`Type` 驱动总分派。
- `FormatValueText(row, idx, col, enc) -> Result<Vec<u8>, InvalidTypeError>`：公开主入口，返回一个单值的无外框字节序列。
- `floatPrec(&ColumnInfo) -> i32`：私有精度规则，默认返回 `types::UnspecifiedLength as i32`。
- `AppendFormatFloat(Vec<u8>, f64, i32, i32) -> Vec<u8>`：公开浮点追加器，保留传入前缀并把格式化结果追加其后。
- `expFormatBig`、`expFormatSmall`、`defaultMySQLPrec`：分别为科学计数法上阈值 `1e15`、非零下阈值 `1e-15` 和 float32 科学计数法精度 `5`。

文件没有 trait、impl 块或条件编译项；`InvalidTypeError` 和 `ColumnInfo` 的 trait 实现均由派生或显式标准错误实现提供。

## 执行流程

1. 上游 `DumpTextRow` 遍历列；若 `row.IsNull(index)`，直接写 `0xfb`，不会进入 `FormatValueText`。
2. 对非 NULL 值，上游把 server 列元数据转换为本文件的 `ColumnInfo`，并传入可变 `ResultEncoder`。
3. `FormatValueText` 按 `col.Type` 分派：
   - Tiny/Short/Int24/Long 使用 `GetInt64` 和十进制 `AppendInt`；Year 的零值特判为四字符 `0000`。
   - Longlong 根据 `mysql::HasUnsignedFlag(col.Flag)` 选择 `GetUint64`/`AppendUint` 或 `GetInt64`/`AppendInt`。
   - Float/Double 经 `floatPrec` 取精度后调用 `AppendFormatFloat`，bit size 分别为 32/64。
   - NewDecimal、Date/Datetime/Timestamp、Duration 以及命名类型先调用相应 `chunk::Row` getter，再使用类型自身的 `String()`；Duration getter同时接收 `Decimal` 作为小数秒精度。
   - String/VarString/Varchar/Bit/各 Blob、Enum、Set 先以 `Charset` 更新 encoder，再调用 `EncodeData`。
   - JSON/向量先切换到默认 collation，再编码其字符串形式。
4. `AppendFormatFloat` 对 NaN 或绝对值超过 `f64::MAX` 的值输出 `0`；后者覆盖正负无穷。其余值按对应 bit size 判断绝对值是否位于 `[1e-15, 1e15)` 之外。阈值内用定点格式；阈值外用科学计数法，float32 强制精度 5，然后删除 `e+` 中的正号并裁掉尾随小数零及孤立小数点。
5. `DumpTextRow` 接收成功结果并补长度编码；若本函数返回 `ErrInvalidType`，上游转换为包含具体类型码的 server 错误。

## 数据与状态

本文件自身没有全局可变状态。三个浮点常量不可变；一次调用的输入状态由借用的 `chunk::Row`、列索引、`ColumnInfo` 和 `&mut ResultEncoder` 组成。

`ResultEncoder` 是跨列复用的有状态对象。字符串类分支通过 `UpdateDataEncoding` 改写“当前列编码”，数值分支通过 `take_scratch` 暂时取走 scratch。`format_with_scratch` 在渲染后调用 `recycle_scratch`：若 encoder 尚可复用，就复制本次输出作为下一次 scratch；返回的 `Vec<u8>` 仍由调用者独立拥有。若 encoder 已执行 `Clean`，`take_scratch` 会新建临时 `Vec` 且不再回收，调用仍不会因 scratch 缺失而 panic。

类型 getter 与 `ColumnInfo::Type` 必须一致，且 `idx` 必须指向有效的非 NULL 列；本文件不保存 Datum，也不校验行模式。字符集转换可能复用 `ResultEncoder` 的内部转换缓冲，因此调用者应在下一次相关编码前消费返回字节；当前 server 入口会立即把值复制进整行 buffer。

## 依赖与调用关系

上游直接调用边为 `pkg/server/internal/column/column.rs::DumpTextRow -> pkg/format/textrow/textrow.rs::FormatValueText`。`DumpTextRow` 还负责 `Info::toTextRow` 元数据投影、NULL 分支、length-encoded 外框和错误身份转换。crate 根公开再导出本文件，因此其他依赖该 crate 的代码也能调用两个公开函数；RustCodeGraph 的精确 callers 查询未建立额外调用边，不能据此断言不存在动态、再导出或未来调用者。

主入口的直接下游关系如下：

- `FormatValueText -> format_with_scratch -> ResultEncoder::{take_scratch,recycle_scratch}`：整数和浮点的缓冲复用。
- `FormatValueText -> AppendFormatFloat -> goish::strconv::AppendFloat`：浮点文本格式。
- `FormatValueText -> chunk::Row::{GetInt64,GetUint64,GetFloat32,GetFloat64,GetMyDecimal,GetBytes,GetTime,GetDuration,GetEnum,GetSet,GetJSON,GetVectorFloat32}`：按类型读取单元值。
- `FormatValueText -> ResultEncoder::{UpdateDataEncoding,EncodeData}`：字符串、Blob、Bit、Enum、Set、JSON 和向量的字符集转换。
- `floatPrec -> mysql::NotFixedDec / types::UnspecifiedLength`：复用 parser/type 层的精度约定。

`Cargo.toml` 的直接依赖与上述调用一致：`goish` 提供 Go 风格 strconv 追加函数，`parser-mysql` 提供类型码、flag 与 collation，`types-integration` 提供 Datum 相关表示，`chunk` 提供行视图；字符集的实际转换由相邻 `result_encoder.rs` 通过 `parser-charset` 完成。`pkg/server/internal/column/Cargo.toml` 以 `textrow-crate` 指向本 crate，证明 server 入口的 crate 边界接线。

## 错误处理与边界

- 唯一显式 `Result` 错误是类型分派的默认分支 `ErrInvalidType`；Rust 测试以 `mysql::TypeGeometry` 验证该分支。错误不携带类型码，上游 `DumpTextRow` 负责补充上下文。
- NULL 不在本函数的职责内；对 NULL 直接调用类型 getter 不符合调用契约。server 入口在调用前处理 NULL。
- 索引越界、Datum 与类型码不匹配等问题没有被转换成 `InvalidTypeError`；安全扩展时必须保持上游元数据与 row 布局一致。
- YEAR 0 必须输出 `0000`；Longlong 的无符号解释只由 `UnsignedFlag` 决定。
- 浮点科学计数法边界是绝对值 `>= 1e15` 或非零且 `< 1e-15`；零仍走定点路径。NaN、`+Inf`、`-Inf` 输出 `0`。科学计数法移除正指数的 `+`，但保留负号。
- 字符集 ID 未知时，具体策略位于 `ResultEncoder::UpdateDataEncoding`：记录 warning 后继续选择编码；转换失败时 `EncodeData` 记录 debug 并返回转换器产生的部分输出，本文件不额外包装为错误。
- 目前默认分支也意味着新增 MySQL 类型在接线前会明确失败，而不是静默产生错误文本。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源。一次调用是同步的，生命周期止于返回 `Vec<u8>`。

`FormatValueText` 要求独占可变借用 `&mut ResultEncoder`，同一个 encoder 不能在多个并发调用中无同步共享；这也保护了当前列编码和 scratch/buffer 的顺序状态。典型生命周期是语句内构造一个 encoder、逐列复用、语句结束调用相邻实现的 `ResultEncoder::Clean` 释放保留分配。数值输出的 scratch 回收属于性能优化，不改变文本语义；字符集分支会更新 encoder 的列侧编码，所以并发化逐列格式化时应为每个执行流提供独立 encoder，或在上层串行化访问。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/format/textrow/textrow.go`，公开结构、类型分派、精度规则和三个浮点常量逐项对应。`pkg/format/textrow/textrow_test.go` 与 `textrow_test.rs` 使用同组预期，覆盖 signed/unsigned、表达式浮点精度、非空 Table 保留完整精度、Blob/Varchar、UTF-8 到 GBK、Datetime/Duration/Decimal、YEAR、Enum/Set/JSON、Geometry 错误及浮点表。

需要注意的语言实现差异：

- Go 的 `ErrInvalidType` 是 `errors.New` 的错误值；Rust 用零大小 `InvalidTypeError` 加常量实例，便于 `Eq` 比较。
- Go `FormatValueText` 接收按值的 `ColumnInfo` 和 `chunk.Row`；Rust 分别借用 `&ColumnInfo`、`&chunk::Row`，并以 `usize` 表示索引。
- Go 的数值结果直接基于 `enc.scratch[:0]` 追加；Rust 暂取 `Vec`，返回拥有所有权的输出，并复制一份回 encoder 供下次复用。这避免返回值与 encoder scratch 共享 Rust 生命周期，但会产生一次回收复制。
- Go 对 Decimal/Time 等使用 `hack.Slice` 避免字符串到字节的显式复制；Rust 使用 `String().into_bytes()`，语义相同但分配特征不同。
- Go 默认错误返回 `nil, ErrInvalidType`；Rust 返回 `Err(ErrInvalidType)`。两侧均由外层协议代码决定最终错误身份与上下文。

当前 Rust 还覆盖 `TypeTiDBVectorFloat32`，与同路径 Go 实现一致。没有证据表明本文件是桩或未接线模块：server Rust 入口存在直接生产调用。

## 扩展指南

新增或修改支持类型时，最小接入点是 `FormatValueText` 的 `match col.Type`：先确认对应 `chunk::Row` getter、文本表示、NULL 的上游处理以及该类型应采用列 collation 还是默认 collation。若新类型属于字符串列，还需同步检查相邻 `result_encoder.rs::IsStringColumnType`，否则元数据字符集与数据编码可能不一致。

修改浮点规则应集中在 `floatPrec` 或 `AppendFormatFloat`，并保留三类不变量：32/64 位阈值按对应精度判断、传入 buffer 前缀不被破坏、科学计数法规范化不误删指数。性能修改还应检查数值 scratch 回收、字符串转换缓冲和 `String().into_bytes()` 的分配行为。

测试必须放在独立文件 `pkg/format/textrow/textrow_test.rs`，不要嵌入生产源文件；同步更新 Go 语义时还应对照 `pkg/format/textrow/textrow_test.go`。至少补充成功输出、错误类型、字符集或精度边界，并检查上游 `pkg/server/internal/column/column.rs::DumpTextRow` 的 length encoding/错误映射是否仍成立。兼容风险主要是客户端可见字节变化，性能风险主要是逐单元分配或编码器缓冲失去复用。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/format/textrow` 确认目标、crate 根、相邻 encoder 和独立测试均已索引。
- RustCodeGraph `node --file pkg/format/textrow/textrow.rs`：核对全部 233 行、8 个主要符号及完整类型分派。
- RustCodeGraph `query`：分别定位 Rust/Go 的 `FormatValueText`、`floatPrec`、`AppendFormatFloat`，以及私有 `format_with_scratch`；精确 `callers/callees` 未返回边，因此又以索引源码和仓库搜索核验直接接线。
- RustCodeGraph 索引源码：`pkg/format/textrow/lib.rs` 证明公开再导出和独立测试装配；`pkg/format/textrow/result_encoder.rs` 证明字符集、scratch、Clean 与转换失败策略；`pkg/server/internal/column/column.rs::DumpTextRow` 证明生产调用、NULL 处理、长度外框和错误映射。
- 配置：`pkg/format/textrow/Cargo.toml` 核对 crate 名、入口、依赖及 Go 包映射；`pkg/server/internal/column/Cargo.toml` 核对 server 到 textrow crate 的依赖。
- 对照与测试：`pkg/format/textrow/textrow.go`、`pkg/format/textrow/textrow_test.go`、`pkg/format/textrow/textrow_test.rs`。Rust 测试覆盖类型与字符集主路径、Geometry 错误和 32/64 位浮点边界；本任务按要求仅作静态文档分析，未运行 Cargo。
