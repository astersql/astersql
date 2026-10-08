# `pkg/server/internal/column/column.rs`

## 文件定位

[`column.rs`](./column.rs) 属于 `astersql-server-internal-column` crate，是 SQL 执行结果进入 MySQL 客户端线协议前的列元数据与行数据编码层。crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，模块由 [`lib.rs`](./lib.rs) 私有加载后整体再导出；因此外部 Rust 调用者通过 crate 根使用 `Info`、`DumpFlag`、`dumpType`、`DumpTextRow` 和 `DumpBinaryRow`。

它位于结果集抽象和网络包写出之间：上游提供 `resolve::ResultField` 转换得到的 `Info`、`chunk::Row` 与可选 `textrow::ResultEncoder`；本文件生成 ColumnDefinition41 字段体或一行的文本/二进制协议载荷。包头、分包、socket 写入和 flush 不在这里处理。Go 生产链可在 [`pkg/server/conn.go`](../../conn.go) 的 `handleFieldList`、`writeColumnInfo`、`writeChunks`、`writeChunksWithFetchSize` 中看到；Rust 当前接线状态见“依赖与调用关系”和“与 Go 版本的对应关系”。

## 核心职责

1. `Info::Dump` / `Info::DumpWithDefault` 通过私有 `Info::dump` 写出 ColumnDefinition41：固定 catalog `def`、五个可编码名称字段、字符集、显示长度、协议类型、标志、小数位及可选默认值。
2. `dumpType`、`DumpFlag`、`Info::dumpCharset`、`Info::dumpLength` 将内部 SET、ENUM、BLOB、TiDB Vector 类型伪装成客户端能理解的 MySQL 协议形态。Vector 对外表现为非 binary 的 long blob，并使用默认校对集和最大 long-blob 宽度。
3. `DumpTextRow` 把一行的每个非 NULL 单元格交给共享 `textrow::FormatValueText`，再套长度编码；NULL 直接写 `0xfb`。
4. `DumpBinaryRow` 写 OK 头、从第 2 bit 开始的 NULL 位图，并按 MySQL 类型选择定长数值、二进制日期时间或长度编码字符串载荷。
5. `render_default_value` / `format_go_float` 保持 Go `fmt.Sprintf("%v", value)` 的默认值输出约定，包括原始字符串字节、`NaN`、`+Inf`、`-Inf` 以及两位指数格式。

本文件不负责从 planner 字段构造 `Info`；该职责在相邻 [`convert.rs`](./convert.rs) 的 `ConvertColumnInfo`。它也不负责执行查询、遍历结果集生命周期或写网络包。

## 主要符号

- `const maxColumnNameSize: usize = 256`：`Name` 与 `OrgName` 的协议字节上限；截断单位是字节。
- `pub struct Info`：ColumnDefinition41 所需的值对象。`Schema`、`Table`、`OrgTable`、`Name`、`OrgName` 是名称元数据；`ColumnLength`、`Charset`、`Flag`、`Decimal`、`Type` 是类型元数据；`DefaultValue: Option<DefaultValue>` 只在 `DumpWithDefault` 路径追加。
- `Info::Dump(buffer, encoder)`：公开的无默认值列定义编码入口。
- `Info::DumpWithDefault(buffer, encoder)`：公开的 COM_FIELD_LIST 列定义编码入口。
- `Info::dump(..., with_default)`：两种列定义编码的唯一共享实现。
- `Info::dumpCharset()` / `Info::dumpLength()`：Vector 专用兼容映射，否则返回结构体原值。
- `Info::toTextRow()`：只投影文本格式化需要的 `Type`、`Charset`、`Flag`、`Decimal`、`Table`。
- `render_default_value()` / `format_go_float()`：私有的默认值兼容格式化器。
- `pub fn DumpFlag(tp, flag)`：SET/ENUM 补协议标志，Vector 清除 `BinaryFlag`。
- `pub fn dumpType(tp)`：SET/ENUM→`TypeString`，Vector→`TypeLongBlob`，三种非 tiny 的 blob 变体连同 tiny blob→`TypeBlob`，其他类型原样返回。
- `pub fn DumpTextRow(...) -> Result<Vec<u8>, err::Error>`：文本结果行编码入口。
- `pub fn DumpBinaryRow(...) -> Result<Vec<u8>, err::Error>`：二进制结果行编码入口。

文件没有 trait、枚举、条件编译项或全局可变状态。公开 API 保留了 Go 风格命名，crate 根通过 lint allow 支持这一迁移约定。

## 执行流程

列定义流程如下：

1. `Dump` 或 `DumpWithDefault` 选择 `with_default`，进入 `Info::dump`。
2. 调用者未提供编码器时，函数在栈上创建 utf8mb4 `ResultEncoder`；否则复用调用者的可变编码器。
3. `Name`、`OrgName` 先按最多 256 字节取切片。随后依次写 catalog `def` 及 schema/table/original-table/name/original-name；除 catalog 外均经 `EncodeMeta` 转码，再经 `dump::LengthEncodedString` 加长度框。
4. 写入定长字段长度 `0x0c`，再写字符集 ID、列宽、映射后的类型和标志、小数位与两个保留零字节。
5. 仅 `DumpWithDefault` 追加默认值：`None`、字节值恰为 `CURRENT_TIMESTAMP` 或 `CURRENT_DATE` 时写 `0xfb`；其他值先按 Go 表示法渲染，再写长度编码字节串。

文本行流程由 `DumpTextRow` 对 `columns` 顺序枚举：`row.IsNull(index)` 为真时追加 `0xfb`；否则把 `Info::toTextRow()` 的投影和该单元格交给 `FormatValueText`，成功后追加长度编码结果。该函数不自行实现整数、时间或精度规则，共享格式化器才是这些规则的真实实现。

二进制行流程由 `DumpBinaryRow` 执行：先追加 `mysql::OKHeader`，预留 `(columns.len() + 9) / 8` 字节位图；NULL 列设置 `(index + 2)` 对应的位且不写载荷；非 NULL 列按 `Type` 分派。整数和浮点以小端数值/位型写入，decimal、字符串、blob、enum、set、JSON、Vector 以长度编码字节写入，日期时间走 `BinaryDateTime`，duration 走 `BinaryTime`。JSON 和 Vector 固定以默认校对集编码；普通字符、blob、enum、set 在写入前切换到列字符集。

## 数据与状态

`Info` 是可克隆、可比较且有默认值的纯数据结构，不持有连接或结果集。编码函数接收并返回 `Vec<u8>`，会保留传入缓冲区已有前缀并在其后追加数据；Go 调用端常用四字节包头占位并在每个包前复用缓冲区。函数不记录“已经编码到第几行”等跨调用状态。

`ResultEncoder` 是唯一可变协作状态。元数据路径调用 `EncodeMeta` 和 `ColumnCharsetID`；行路径会通过 `UpdateDataEncoding` 切换数据编码。若传入 `None`，每次调用创建 utf8mb4 fallback，生命周期仅限本次调用；若传入 `Some(&mut encoder)`，字符集状态由调用者在串行调用间复用。

NULL 有两种上下文但相同标记值：文本行中的 NULL 单元格直接是 `0xfb`；COM_FIELD_LIST 的无默认值和两个动态时间默认值也用 `0xfb`。二进制行的 NULL 不写值标记，而由带两位保留偏移的位图表达。

## 依赖与调用关系

直接下游依赖可由源码和 [`Cargo.toml`](./Cargo.toml) 复核：

- `charset`、`mysql` 提供默认字符集、类型码、标志和宽度常量；实际 crate 分别是 `astersql-parser-charset`、`astersql-parser-mysql`。
- `chunk` 提供 `Row` 及所有类型 getter；`textrow` 提供元数据/数据转码和文本单元格格式化。
- `dump` 提供长度编码整数/字符串、小端整数、二进制日期时间与 duration 编码。
- `err::ErrInvalidType` 是公开行编码错误的构造入口；`time::Duration` 用于把内部 duration 纳秒值交给 `BinaryTime`。
- `DefaultValue` 从 `meta-model` 再导出，布尔、整数、无符号整数、浮点和原始字节串是当前渲染分支。

RustCodeGraph 对精确符号确认的内部边包括 `Dump → Info::dump`、`DumpWithDefault → Info::dump`、`DumpTextRow → Info::toTextRow`；源码还明确给出 `Info::dump → dumpCharset/dumpLength/dumpType/DumpFlag/render_default_value` 与 `render_default_value → format_go_float`。RustCodeGraph 的 callers 查询在本地超时，故上游接线使用仓库文本搜索和已索引文件源码补证。

当前 Rust 生产代码中，[`pkg/server/runtime.rs`](../../runtime.rs) 的 `protocol_column` 调用相邻 `ConvertColumnInfo`，随后调用本文件的 `dumpType` 和 `DumpFlag` 生成协议层 `ColumnInfo`；[`pkg/server/protocol_result.rs`](../../protocol_result.rs) 与 [`pkg/server/internal/resultset/resultset.rs`](../resultset/resultset.rs) 使用 `Info` 保存/缓存列元数据。仓库搜索未发现生产 Rust 直接调用 `Info::Dump`、`DumpTextRow` 或 `DumpBinaryRow`；这些入口目前直接出现在 [`column_test.rs`](./column_test.rs) 与 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)。因此不能仅凭 Go 的完整接线声称 Rust 网络主链已经通过这三个函数写包。

## 错误处理与边界

- `Dump`、`DumpWithDefault`、`DumpFlag`、`dumpType` 是无错误返回 API；它们依赖传入 `Info` 已具备一致的协议字段。
- `DumpTextRow` 把 `FormatValueText` 的任意失败统一映射为 `ErrInvalidType`，消息带列类型号，但不保留下游错误细节。
- `DumpBinaryRow` 对未列举的类型返回同一 `ErrInvalidType`；已列举分支直接调用与类型匹配的 `Row` getter，因此调用者必须保证 `columns` 数量、顺序、类型与 `row` 一致。
- 两个行函数都先写入部分缓冲区再可能报错；发生错误时返回 `Err` 而不返回部分 `Vec<u8>`。调用者不得把原缓冲区视为事务式回滚的输出。
- 名称限制按 UTF-8/原始字符串的字节切片处理，而非字符数；这与 Go 的 `[]byte` 截断一致。元数据编码器必须能够处理截断后的字节序列。
- `format_go_float` 显式处理 NaN 和正负无穷；有限值根据科学计数指数在 `[-4, 6)` 内选择 Rust 最短普通表示，否则输出带符号、至少两位的指数。它依赖 Rust `e` 格式始终含可解析指数，内部 `expect` 表达这一不变量。
- `CURRENT_TIMESTAMP` / `CURRENT_DATE` 只有在 `DefaultValue::String` 字节完全相等时才按 NULL 默认值处理，大小写或附加精度形式不会命中。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或 I/O 资源；所有输出都在调用线程内同步完成。`Info` 和输入 `Row` 只借用读取，缓冲区所有权按值传入并按值返回。

`Option<&mut ResultEncoder>` 的独占借用阻止同一编码器在单次调用期间并发使用。调用者若跨连接共享编码器，必须自行同步；现有 Go 主链是每连接持有并串行初始化/清理 `rsEncoder`。fallback 编码器在函数返回时释放。对高频路径，传入并复用编码器和已有容量的 `Vec<u8>` 可减少分配；[`column_test.rs`](./column_test.rs) 的 `benchmark_dump_column` 保留了这一热路径形状，但它只是普通 Rust 辅助函数，不是原生 benchmark harness。

## 与 Go 版本的对应关系

直接对照文件是 [`column.go`](./column.go)。Rust 保留同名数据字段和主要入口，ColumnDefinition41 字段顺序、`0x0c` 定长头、NULL 位图偏移、类型 getter 分派、字符集切换以及 SET/ENUM/Vector/BLOB 映射均逐分支对应 Go。

已验证的表示差异包括：Go `Info.DefaultValue` 是 `any`，Rust 收窄为 `Option<DefaultValue>`；Rust 通过 `render_default_value` 模拟 Go `%v`，并专门保持任意字符串字节和 Go 风格浮点指数。Go `[]*Info` 在 Rust 中是 `&[Info]`，因此 Rust 行编码不表达空指针列。Go 返回 `([]byte, error)`，Rust 返回 `Result<Vec<u8>, err::Error>`。

测试对应关系：[`column_test.go`](./column_test.go) 的列定义字节、SET/ENUM 类型标志、256 字节名称限制及文本值场景移植到独立 [`column_test.rs`](./column_test.rs)；Rust 补充的 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 进一步覆盖默认值原始字节/浮点格式、Vector 元数据、二进制行 NULL 位图和未知类型错误。Rust 测试不与生产源文件混放，符合仓库约束。

生产接线尚不完全同构：Go [`conn.go`](../../conn.go) 直接用 `DumpWithDefault` 响应 COM_FIELD_LIST，用 `Dump` 写列定义，并在普通/游标结果路径调用文本或二进制行编码；当前 Rust 网络运行时只确认直接使用 `Info`、`dumpType` 和 `DumpFlag`，行编码函数仍主要由测试覆盖。后续迁移不得把 Go 的网络写包接线当成 Rust 当前事实。

## 扩展指南

- 新增 MySQL 类型时，先判断客户端暴露类型是否等于内部类型；必要时同步修改 `dumpType`、`DumpFlag`、`dumpCharset`、`dumpLength`，再为 `DumpBinaryRow` 增加与 `chunk::Row` 表示一致的载荷分支。文本表现应优先扩展共享 `textrow::FormatValueText`，不要在这里复制格式化规则。
- 修改列定义布局、默认值或名称限制时，应同步核对 [`column.go`](./column.go) 和 COM_FIELD_LIST/ColumnDefinition41 兼容性，并扩展 [`column_test.rs`](./column_test.rs)；Rust 特有的默认值、Vector、二进制协议边界放在 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，不要把测试嵌入 `column.rs`。
- 扩展 `DefaultValue` 枚举时必须同步 `render_default_value`，并明确 Go `%v` 的精确字节形式。特别检查非 UTF-8 字节、浮点阈值、NaN/Inf 和动态时间默认值。
- 若把 `DumpTextRow` / `DumpBinaryRow` 接入 Rust 生产网络主链，应从结果集列缓存取得同序 `Info`，复用每连接编码器，保持已有包头前缀，并补覆盖文本查询、prepared statement、cursor fetch 和编码失败传播的独立集成测试。
- 性能风险集中在每单元格字符串化、转码和长度编码追加；兼容风险集中在客户端可见类型/flag/charset/length 与 NULL 位图。不要以“客户端能显示”为唯一验收，应断言精确协议字节。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/server/internal/column` 确认目标 Rust/Go/测试文件均已索引。
- RustCodeGraph 源码与符号检查：`node --file pkg/server/internal/column/column.rs --offset 1 --limit 500` 读取完整 342 行和 14 个符号；`query DumpBinaryRow`、`query DumpTextRow`、`query dumpType`、`query DumpFlag` 均定位到 Rust 与 Go 对照定义；`callees` 确认 `Dump`/`DumpWithDefault` 到 `dump`、`DumpTextRow` 到 `toTextRow` 的边。精确 `callers` 查询超时，未把空输出作为无调用证据。
- 已读取的直接证据：[`column.rs`](./column.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`column.go`](./column.go)、[`column_test.rs`](./column_test.rs)、[`column_test.go`](./column_test.go)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`pkg/server/conn.go`](../../conn.go)、[`pkg/server/runtime.rs`](../../runtime.rs)、[`pkg/server/protocol_result.rs`](../../protocol_result.rs)、[`pkg/server/internal/resultset/resultset.rs`](../resultset/resultset.rs)。目标目录没有 `doc.go`。
- 关键上游边：Go `handleFieldList → Info.DumpWithDefault`、`writeColumnInfo → Info.Dump`、`writeChunks → DumpBinaryRow/DumpTextRow`、`writeChunksWithFetchSize → DumpBinaryRow`；Rust `protocol_column → ConvertColumnInfo → dumpType/DumpFlag`，以及结果集/协议适配层对 `Info` 的持有。
- 独立测试证据：`column_test.rs` 覆盖精确列定义字节、默认值、名称限制和多种文本类型/gbk；`migration_aster_unit_test.rs` 覆盖默认值字节与 Go 浮点格式、Vector 兼容映射、文本 NULL/数值/ENUM/JSON、二进制 NULL 位图和未知类型错误。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前另运行任务指定的 11 章节结构检查，并人工复核本文只陈述上述源码、调用边、Cargo 和测试能够支持的事实。
