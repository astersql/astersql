# `pkg/lightning/mydump/common.rs`

## 文件定位

`common.rs` 是 `astersql-lightning-mydump` crate 的公共数据契约文件。crate 根模块在 [`lib.rs`](lib.rs) 中以 `mod common; pub use common::*;` 将本文件的类型重新导出，因此 loader、reader、parser、router、region、schema/view import，以及 crate 外的 Lightning importer 都可以从 crate 根使用这些类型。crate 边界和 Go 包映射由 [`Cargo.toml`](Cargo.toml) 的 `name = "astersql-lightning-mydump"`、`path = "lib.rs"` 和 `go-package = "pkg/lightning/mydump"` 明确给出。

本文件不扫描目录、不读取数据，也不解析 SQL/CSV；它只定义统一错误 `MydumpError` 和描述输入文件的三个值类型 `ExtendColumnData`、`FileMeta`、`FileInfo`。实际构造与消费发生在相邻模块，例如 [`loader.rs`](loader.rs) 的 `constructFileInfo`、[`reader.rs`](reader.rs) 的文件打开/读取流程和 [`region.rs`](region.rs) 的分片计算。

## 核心职责

1. `MydumpError` 为 mydump Rust 子 crate 提供一个可比较、可克隆且可展示的错误边界，使配置、编码、语法、I/O、路由、schema 和输入耗尽能被调用者按类别处理。
2. `ExtendColumnData` 保存文件路由带入的扩展列名和常量值；二者按相同下标组成列值对，但本类型本身不校验长度相等。
3. `FileMeta` 保存一个输入对象的路径、物理大小、估算导入大小、来源类型、整体压缩格式和排序键。
4. `FileInfo` 将 `FileMeta` 与扩展列组合为 loader/parser/region 间传递的文件描述对象。

这些类型是“数据载体而非行为对象”：除标准派生实现和 `std::io::Error -> MydumpError` 转换外没有方法，也不自行维护跨字段不变量。

## 主要符号

- `pub enum MydumpError`（`common.rs:16`）共有七个变体：
  - `Eof`：Rust 解析接口用于表示输入正常耗尽的哨兵；上层可精确匹配，而不是解析错误字符串。
  - `Configuration(String)`：无效配置，例如不支持的字符集或路由配置。
  - `Encoding(String)`：字符集编解码失败。
  - `Syntax(String)`：SQL/CSV 词法或语法失败。
  - `Io(String)`：底层读写或存储访问失败。
  - `Routing(String)`：文件路径不能按路由规则解释。
  - `Schema(String)`：schema 文件缺失、解析或执行相关失败。
  `thiserror::Error` 为每个变体生成稳定的显示前缀；`Clone + PartialEq + Eq` 允许上层复制或精确判断错误类别和值。
- `impl From<std::io::Error> for MydumpError`（`common.rs:40`）把标准 I/O 错误降格为 `Io(error.to_string())`。转换保留可读消息，但不保留原始 `io::ErrorKind`、错误链或可供 `downcast` 的源对象。
- `pub struct ExtendColumnData`（`common.rs:48`）包含 `columns: Vec<String>` 与 `values: Vec<String>`；`Default` 产生两个空向量。
- `pub struct FileMeta`（`common.rs:57`）包含：
  - `path: String`：存储对象路径；
  - `file_size: i64`：对象物理字节数；
  - `real_size: i64`：解压或格式估算后的导入体量；
  - `source_type: crate::SourceType`：由 [`router.rs`](router.rs) 定义的 `Ignore/SchemaSchema/TableSchema/Sql/Csv/Parquet/ViewSchema`；
  - `compression: crate::Compression`：文件整体压缩格式，不是 Parquet 内部 codec；
  - `sort_key: String`：同表多文件的稳定排序键。
- `pub struct FileInfo`（`common.rs:74`）包含 `file_meta: FileMeta` 与 `extend_data: ExtendColumnData`。

三个结构体都派生 `Clone + Debug + Default + PartialEq + Eq`。默认值会产生空字符串、零大小、空扩展列，以及 `SourceType::Ignore`、`Compression::None`；默认对象只是便于逐字段构造，不代表一个已经验证、可导入的文件。

## 执行流程

典型数据流如下：

1. [`router.rs`](router.rs) 的 `FileRouter` 从路径提取 schema、table、文件类型、压缩格式与分片 key，形成 `RouteResult`；非法规则或类型进入 `MydumpError::Routing`。
2. [`loader.rs`](loader.rs) 的 `constructFileInfo(path, size, route)` 创建 `FileInfo`，把 `path`、`file_size`、初始 `real_size = file_size`、`source_type`、`compression` 和 `sort_key` 写入 `FileMeta`。
3. loader 按库表聚合文件，并按 `sort_key` 排序；`loader_test.rs::TestLoader` 和 `TestFileRouting` 分别验证了 `0001/0002` 顺序及自定义 key `99` 的传递。
4. 后续 reader/parser 根据 `path` 与 `compression` 打开输入，根据 `source_type` 选择 SQL、CSV 或 Parquet 路径；region 逻辑使用 `file_size`、`real_size` 和类型计算分块与引擎分配。
5. 路由规则产生扩展列时，调用方填充 `extend_data`，导入链将这些常量附加到每行。该文件只携带数据，不负责列值对齐或写入行。
6. 下游操作失败时返回相应 `MydumpError`；标准读取错误可以经 `From<std::io::Error>` 自动转换为 `Io`。解析循环则把 `Eof` 当作终止条件，其他变体继续向上传播。

## 数据与状态

所有状态都由调用者拥有，结构体内部没有缓存或隐藏状态。

- `FileMeta.path` 是后续 Storage 查找键；本文件不规范化路径，也不判断相对/绝对路径。
- `file_size` 与 `real_size` 都采用有符号 `i64`，与 Go 的 `int64` 对齐。构造时常令二者相等；压缩文件或 Parquet 采样后，`real_size` 可被替换为估算值。`region_test.rs::TestParquetFileRegionUsesRealSizeForEngineAllocation` 验证 Parquet 引擎分配读取 `real_size`，而非只看物理大小。
- `sort_key` 是字符串而非数值，排序语义取决于上游生成格式；默认 mydumper 的零填充键可按字典序稳定排列。
- `ExtendColumnData.columns[i]` 与 `values[i]` 被设计为一对，但类型允许长度不一致、重复列或空列名。校验责任在填充或消费端。
- `Default` 不执行合法性检查：空路径、负大小、`SourceType::Ignore` 都可以存在于内存中。因此不能把“可构造”解释为“可导入”。

## 依赖与调用关系

直接依赖很小：标准库的 `std::io::Error` 和 Cargo 依赖 `thiserror = "2"`。`FileMeta` 还引用 crate 根重新导出的 `SourceType` 与 `Compression`，其真实定义在 [`router.rs`](router.rs)。

RustCodeGraph 对 `common.rs` 的文件节点报告 32 个使用文件，直接相关区域包括 `pkg/lightning/mydump` 的 loader、reader、parser、router、region、schema/view import，以及 `lightning/pkg/importer` 的 chunk process、pre-info、table import 和 duplicate detection。主要可复核边包括：

- `loader.rs::constructFileInfo -> FileInfo/FileMeta`：从路由结果建立文件描述；
- `loader.rs::calculateFileBytes -> FileInfo.file_meta.file_size`：汇总物理文件大小；
- `region.rs` 的分片函数读取 `FileInfo.file_meta`：按类型、压缩和大小选择分片策略；
- `test_support.rs::file -> FileInfo/FileMeta`：独立测试辅助构造最小文件描述；
- `schema_import.rs`、`charset_convertor.rs`、`router.rs` 等返回或构造 `MydumpError`；
- `lightning/pkg/importer/get_pre_info.rs` 和 `chunk_process.rs` 精确匹配解析实现的 `MydumpError::Eof`，说明 EOF 类别会跨 mydump/importer 边界影响循环终止。

本文件没有调用业务函数；RustCodeGraph 对 `common.rs::from` 未发现下游调用边，符合它只读取参数并构造 `Io(String)` 的实现。

## 错误处理与边界

- `MydumpError::Eof` 是控制流哨兵，不应包装成 `Syntax` 或 `Io`，否则读取循环无法区分正常结束与失败。
- `From<std::io::Error>` 统一简化了 `?` 传播，但会丢失 `ErrorKind`。如果新增逻辑需要区分 `NotFound`、`UnexpectedEof`、`Interrupted` 等，不能只依赖现有 `Io(String)`；应先评估是否扩展枚举或在转换前分支处理。
- 字符串载荷没有结构化字段。调用者应匹配枚举变体，不应依赖完整显示文本；只有需要诊断时才展示消息。
- `FileMeta` 不验证大小非负、路径非空、类型与扩展名一致，也不验证压缩格式是否受 Storage 支持。上述检查由 router、loader、reader 等真实操作点完成。
- `ExtendColumnData` 不保证 `columns.len() == values.len()`。新增生产者必须成对生成，新增消费者必须决定长度不匹配时是拒绝还是安全截断，不能无条件按一个向量的长度索引另一个。
- `Clone` 会深拷贝路径、排序键和两个扩展列向量；大量或高频复制可能增加内存与分配开销。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、文件句柄或事务。四个公开类型都只包含拥有所有权的 `String`/`Vec`、整数和可复制枚举，因此值离开构造作用域后不借用外部缓冲区，释放由 Rust 所有权和 `Drop` 自动完成。

文件本身没有显式 `Send`/`Sync` 实现；这些 auto trait 由字段决定，当前字段均支持在线程间移动/共享。真正的并发策略位于调用者，例如 loader 的并行扫描，以及测试辅助 [`test_support.rs`](test_support.rs) 中带 `Mutex` 的 `MemoryStorage`。修改本文件字段时应重新检查 auto trait、克隆成本和并行调用方兼容性。

`MydumpError` 保存的是拥有所有权的字符串，所以错误可以跨线程和作用域传递；代价是从 `std::io::Error` 转换后原错误对象立即不再保留。

## 与 Go 版本的对应关系

Go 对照主要分散在 [`loader.go`](loader.go) 与 [`router.go`](router.go)：

- Go `ExtendColumnData { Columns, Values []string }` 与 Rust `ExtendColumnData { columns, values }` 字段语义一致。
- Go `FileInfo` 同时包含 `TableName filter.Table` 与 `FileMeta SourceFileMeta`；Rust `common.rs::FileInfo` 只包含文件元数据和扩展列。Rust 的库表归属由 `MDDatabaseMeta`/`MDTableMeta` 层级和路由结果承载，因此不能假定 Rust `FileInfo` 自带表名。
- Go `SourceFileMeta` 含 `Path/Type/Compression/SortKey/FileSize/ExtendData/RealSize/Rows`。Rust 在当前移植中将常用子集拆为 `common.rs::FileMeta` 加 `FileInfo.extend_data`，而 [`loader.rs`](loader.rs) 又定义了一个带 `rows` 的 `SourceFileMeta`。这两个 Rust 类型并存，扩展字段时必须确认目标调用链，不能只改其中一个。
- `SourceType` 和 `Compression` 的数值顺序及含义由 Rust/Go 两份 [`router.rs`](router.rs)、[`router.go`](router.go) 对照；`Default` 对应 Go 零值，即 Ignore/None。
- Go 使用 `error`、`io.EOF` 和 PingCAP 结构化错误；Rust 用本地 `MydumpError` 分类，其中 `Eof` 对应 Go 侧正常 EOF 语义，但不是 `std::io::ErrorKind::UnexpectedEof` 的自动等价物。
- Go 的错误链可用 `errors.Cause`/包装保留更多来源信息；现有 Rust `From<std::io::Error>` 只保留字符串，这是已实现行为差异。

## 扩展指南

- 新增错误类别时，在 `MydumpError` 增加语义明确的变体，并同步所有需要精确分支的 parser/importer 调用点与独立 `*_test.rs`；不要把新的控制流状态塞入通用字符串。
- 修改 `FileMeta` 字段时，至少同步检查 `loader.rs::constructFileInfo`、`test_support.rs::file`、reader/parser 打开路径、region 分片逻辑，以及所有结构体字面量。由于类型派生 `Eq`，新增字段也会改变相等性语义。
- 若字段对应 Go `SourceFileMeta`，同时审计 Rust `loader.rs::SourceFileMeta`，明确字段应属于公共 `FileMeta`、外层 `FileInfo`，还是 loader 专用对象，避免两套表示继续漂移。
- 扩展列功能应在生成端保证列和值一一对应，并在 importer 侧增加异常长度、空名称、重复名称测试；不要在本文件里嵌入测试，遵守同目录独立 `*_test.rs` 约定。
- 改动排序或大小语义时，同步更新 `loader_test.rs` 的排序/估算用例和 `region_test.rs` 的分片/引擎分配用例；压缩与读取行为则应同步 `reader_test.rs`。
- 若需要保留 I/O 错误种类或 source chain，应设计结构化载荷，并评估 `Clone + Eq` 是否仍然必要；直接保存 `std::io::Error` 会破坏当前派生能力和 API 契约。
- 性能风险主要来自深克隆 `String`/`Vec` 及在大文件清单中复制 `FileInfo`。引入共享所有权或借用会影响生命周期与线程边界，应先检查 loader/importer 的并行使用方式。

## 验证依据

- 源文件：[`common.rs`](common.rs)，核对 79 行完整实现、四个公开符号、七个错误变体、标准 I/O 转换及所有派生实现。
- crate 装配与依赖：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)，确认 crate 名、Go 包映射、`thiserror` 依赖和 `pub use common::*` 公共出口。
- RustCodeGraph：执行 `status`，索引包含 11,467 个文件且覆盖 `pkg/lightning/mydump/common.rs`；执行 `files --filter pkg/lightning/mydump`、目标文件 `node`、`MydumpError`/`ExtendColumnData`/`FileMeta`/`FileInfo` 查询及 callers/callees 检查。文件节点报告 32 个使用文件；`from` 无业务 callee。
- Rust 直接实现：[`router.rs`](router.rs)、[`loader.rs`](loader.rs)、[`reader.rs`](reader.rs)、[`region.rs`](region.rs)、[`schema_import.rs`](schema_import.rs)、[`test_support.rs`](test_support.rs)。这些文件分别验证枚举定义、构造与排序、打开输入、大小消费、schema 错误和测试构造方式。
- 独立 Rust 测试：[`loader_test.rs`](loader_test.rs) 覆盖 `sort_key`、路由与 `real_size`；[`region_test.rs`](region_test.rs) 覆盖 `FileInfo/FileMeta` 分片和 Parquet `real_size`；[`reader_test.rs`](reader_test.rs) 覆盖压缩元数据；[`schema_import_test.rs`](schema_import_test.rs) 覆盖 `MydumpError::Schema` 的传播/处理。
- Go 对照与测试：[`loader.go`](loader.go)、[`router.go`](router.go)、[`loader_test.go`](loader_test.go)、[`region_test.go`](region_test.go)、[`parser_test.go`](parser_test.go) 与 [`csv_parser_test.go`](csv_parser_test.go)，核对数据结构、枚举零值、排序/大小语义和 `io.EOF` 行为。
- 未运行 Cargo 或代码测试：本任务只新增说明文档，按计划以源码/调用图事实复核和文档结构校验代替运行时验证。
