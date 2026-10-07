# `pkg/lightning/mydump/test_support.rs`

## 文件定位

该文件属于 `astersql-lightning-mydump` crate；crate 根由 `pkg/lightning/mydump/Cargo.toml` 的 `[lib] path = "lib.rs"` 指向 `pkg/lightning/mydump/lib.rs`，后者以公开模块 `pub mod test_support` 导出这里的辅助设施。它不是导入主链中的生产存储实现，而是实现同一 `Storage` 契约的内存适配器，供 mydump 单元测试、session 导入测试和 RealTiKV 导入场景注入可控文件内容。

文件当前公开两个入口：`MemoryStorage` 和 `file`。RustCodeGraph 将该文件识别为 7 个符号，并显示 `MemoryStorage` 经导入边被 `loader_test.rs`、`reader_test.rs`、`region_test.rs`、`schema_import_test.rs` 使用；仓库搜索还确认了 `pkg/session/runtime/import_compression_test.rs`、`tests/realtikvtest/importintotest3/compression_harness.rs` 和 `tests/realtikvtest/importintotest4/split_file_test.rs` 的 crate 外调用。

## 核心职责

- `MemoryStorage` 用进程内的路径到字节数组映射模拟 `Storage`，避免测试依赖真实文件系统或对象存储。
- `MemoryStorage::with` 一次性建立测试夹具；`MemoryStorage::insert` 支持测试运行中新增或覆盖对象，例如模拟 GCS 对象创建和多文件导入。
- `Storage::open` 返回拥有数据所有权的 `Cursor<Vec<u8>>`，使下游 `ExportStatement`、loader、region 切分和 session IMPORT 逻辑能够按普通 `Read + Send` 流消费内容。
- `Storage::list` 提供稳定的路径、长度清单，支持 loader 扫描和通配符导入。
- `file` 以最少字段构造 `FileInfo`，统一 reader/schema_import 测试中“路径 + 来源类型”的夹具写法。

该文件只模拟存储边界，不负责路由、解压、字符集转换、CSV/SQL 解析、schema 执行或 TiKV 写入；这些行为仍由调用方的生产代码完成。

## 主要符号

- `pub struct MemoryStorage { files: Mutex<BTreeMap<String, Vec<u8>>> }`：唯一状态是互斥保护的有序映射。字段私有，调用者只能通过构造、插入以及 `Storage` trait 访问。
- `impl Default for MemoryStorage`：由派生实现生成空映射，适合逐步用 `insert` 填充。
- `pub fn MemoryStorage::with(files: &[(&str, &[u8])]) -> Self`：复制每个路径和内容，重复路径按 `BTreeMap::collect` 的结果由后项覆盖前项。
- `pub fn MemoryStorage::insert(&self, path: &str, data: impl Into<Vec<u8>>)`：在共享引用上加锁后插入；同路径会替换旧内容。
- `impl Storage for MemoryStorage`：实现 `open` 和 `list`，因此可作为 `&dyn Storage` 或 `Arc<dyn Storage>` 传入生产逻辑。
- `fn open(&self, path: &str, _compression: Compression) -> Result<Box<dyn Read + Send>, MydumpError>`：查找并克隆完整字节数组，缺失路径返回 `MydumpError::Io("missing test file …")`；参数名前的下划线明确表示适配器本身不解释压缩格式。
- `fn list(&self) -> Result<Vec<(String, i64)>, MydumpError>`：克隆每个路径，并把 `Vec<u8>::len()` 转换为 `i64`；顺序继承 `BTreeMap` 的键排序。
- `pub fn file(path: &str, source_type: SourceType) -> FileInfo`：只设置 `FileMeta.path` 与 `FileMeta.source_type`，`file_size`、`real_size`、`compression`、`sort_key` 和 `extend_data` 均沿用默认值。

## 执行流程

典型流程如下：

1. 测试以 `MemoryStorage::with` 预置文件，或用 `MemoryStorage::default` 创建空存储后调用 `insert`。输入切片会复制进 `Vec<u8>`，不借用调用方缓冲区。
2. loader 或 session 通过 `Storage::list` 获得按路径排序的对象及其当前字节长度；例如 loader 测试据此发现 schema 与数据文件，通配符 IMPORT 据此选择多个对象。
3. 下游通过 `Storage::open` 请求某一路径。实现短暂持有锁、克隆内容，然后用 `Cursor` 包装克隆值；读游标后续不再占用共享锁，也不会观察到该路径的后续覆盖。
4. 生产层根据 `FileInfo` 或路径识别压缩与来源类型，再完成解压、解析、region 规划或 schema 导入。`pkg/session/runtime/import_compression_test.rs` 证明压缩缓冲区仍由 session 导入链处理，而非由 `MemoryStorage::open` 处理。
5. 需要元数据时，测试调用 `file(path, source_type)` 后按场景补充字段；例如 `reader_test.rs::TestExportStatementCompressed` 再把 `compression` 改为 `Compression::Gz`。

## 数据与状态

`files` 的键是测试传入的完整字符串路径，包括本地式相对路径和 `gs://…?endpoint=…` 形式的 URI；该适配器不规范化路径、不拆分查询参数，也不创建目录层级。值是文件原始字节，因此可包含文本、压缩帧或故意损坏的数据。

使用 `BTreeMap` 有两个可见效果：`list` 的结果按路径字典序稳定排列；同一路径只能保留一个当前值。`with`、`insert` 和 `open` 都复制数据，换取简单、隔离的所有权语义。`list` 报告的是存储中字节数组长度，即压缩对象的压缩后大小，而不是解压后的逻辑大小。

`file` 生成的 `FileInfo` 不是从 `MemoryStorage` 自动派生：它不会查询实际长度，也不会推断压缩后缀或 sort key。调用者若依赖这些字段，必须显式补齐，或通过 loader 的生产发现流程生成元数据。

## 依赖与调用关系

直接标准库依赖为 `BTreeMap`、`Cursor`、`Read` 和 `Mutex`，没有新增第三方依赖；`pkg/lightning/mydump/Cargo.toml` 的依赖列表也没有专为本文件设置 feature。crate 内类型来自公开重导出的 `Compression`、`FileInfo`、`FileMeta`、`MydumpError`、`SourceType` 和 `Storage`。

下游关键边为：

- `Storage::open` → `pkg/lightning/mydump/reader.rs::ExportStatement`，后者按 `FileInfo.file_meta.path/compression` 打开并读取 schema。
- `Storage::list/open` → `pkg/lightning/mydump/loader_test.rs` 所覆盖的 `NewLoaderWithStore`、压缩比估算和目录扫描流程。
- `Storage::open` → `pkg/lightning/mydump/region_test.rs` 所覆盖的 `SplitLargeCSV` / `MakeTableRegions`。
- `file` → `reader_test.rs` 和 `schema_import_test.rs`，分别构造 schema 读取及表/视图导入元数据。
- `MemoryStorage` → `pkg/session/runtime/import_compression_test.rs` 与两个 `tests/realtikvtest` 场景，经 `SetImportFileStorage` 注入 session IMPORT 主链。

该模块由 `lib.rs` 无条件公开，故虽然主要用于测试，它也可被其他 crate 的测试目标引用；实际业务代码不应把它误当作持久化对象存储。

## 错误处理与边界

`open` 唯一显式业务错误是路径不存在，映射为 `MydumpError::Io`，消息包含原路径。它不会区分权限、网络、校验和或压缩错误，也不会模拟部分读取失败；需要这些边界时，`reader_test.rs` 使用独立的 `ErrorStorage`，而不是扩张 `MemoryStorage`。

`with`、`insert`、`open` 和 `list` 均对 `Mutex::lock()` 直接 `unwrap()`：若持锁线程 panic 导致 mutex 中毒，后续调用也会 panic，而不会返回 `MydumpError`。`data.len() as i64` 没有溢出检查；在现实可分配内存范围内通常安全，但它并非面向超大对象的精确存储模拟。

`open` 忽略 `Compression` 是刻意的测试边界：它返回存储的原始字节。某些 reader 单测直接存入未压缩文本却把元数据标记为 `Gz`，只验证该层的元数据传递；session 压缩测试则写入真实压缩帧，由上层解压组件验证。新增断言时必须区分这两类语义，不能据此声称 `MemoryStorage` 自己支持解压。

## 并发与资源生命周期

`Storage: Send + Sync` 要求实现可跨线程共享；`Mutex<BTreeMap<…>>` 使 `MemoryStorage` 满足这一契约，并允许放入 `Arc<MemoryStorage>`。所有映射访问串行化，但临界区很短：`open` 在锁内克隆内容，`list` 在锁内克隆整个索引，返回后立即释放锁；游标读取期间不持锁。

因此，已打开游标获得的是打开时快照。另一个线程随后用 `insert` 覆盖同一路径，不会改变现有游标，只影响之后的 `open/list`。单次 `list` 在锁内生成完整快照，不会观察到半次插入；但 `list` 后再 `open` 之间没有事务保证，期间路径可能被覆盖。没有后台任务、通道、显式关闭或外部资源；最后一个所有者释放后，映射和所有字节由 Rust 自动回收，`Cursor<Vec<u8>>` 随 reader 被 drop。

## 与 Go 版本的对应关系

同路径不存在 `test_support.go`，所以这里不是某个 Go 文件的一对一翻译。Go 测试把相同职责分散在三类设施中：`loader_test.go` 和 `csv_parser_test.go` 使用 `objstore.NewMemStorage` 及 `WriteFile`；reader 测试使用本地临时文件和 `mockobjstore.NewMockStorage`；部分 region/schema 测试直接写临时目录。

语义对应点是“向生产读取/枚举接口注入可控对象集合”，而不是 API 名称完全相同。Rust 版本以同步 `Storage::open/list`、`Read + Send` 和 `Arc` 表达该边界；Go 版本通常带 `context.Context`、可关闭/可 seek 的对象 reader，并能由 gomock 精确配置失败。Rust 的 `file` 只是测试便利构造器，对应 Go 测试中反复出现的 `FileInfo{FileMeta: SourceFileMeta{…}}` 字面量，而不是 Go 生产 API。

## 扩展指南

若生产 `Storage` trait 新增必需方法，应先在本文件补齐最小且确定性的内存语义，再同步所有直接测试使用者。若只需模拟打开失败、短读、seek 失败等单测特例，优先像 `reader_test.rs::ErrorStorage` 那样在独立测试文件定义专用实现，避免让通用夹具携带易误用的故障状态。

增加删除、重命名或条件失败等可变操作时，应明确其与 `list/open` 的原子性，并保持锁临界区内不调用外部代码。若对象可能很大，应评估 `open` 的全量克隆和 `list` 的全量克隆成本；可考虑共享不可变字节，但必须保持已打开 reader 的快照语义。

扩展 `file` 时要谨慎：默认字段目前让调用点清楚表达自己依赖的元数据。不要根据文件名暗中推断压缩或来源类型，否则会改变现有负面测试的构造方式。测试应继续放在同目录独立文件中：通用存储契约可扩展 `loader_test.rs` 或 `reader_test.rs`，schema 元数据场景扩展 `schema_import_test.rs`，跨 crate IMPORT 行为扩展 `pkg/session/runtime/import_compression_test.rs` 或对应 RealTiKV 测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/lightning/mydump` 确认目标及相邻 Rust/Go 测试；`node --file pkg/lightning/mydump/test_support.rs` 读取完整 81 行并报告 17 个文件的使用关系；精确 `node pkg/lightning/mydump/test_support.rs::MemoryStorage` 确认结构定义及 4 个同 crate 测试导入边。
- 源码与 crate 边界：`pkg/lightning/mydump/test_support.rs`、`pkg/lightning/mydump/lib.rs`、`pkg/lightning/mydump/Cargo.toml`、`pkg/lightning/mydump/common.rs`、`pkg/lightning/mydump/reader.rs`、`pkg/lightning/mydump/router.rs`。
- 独立 Rust 测试：`pkg/lightning/mydump/loader_test.rs`、`reader_test.rs`、`region_test.rs`、`schema_import_test.rs`、`pkg/session/runtime/import_compression_test.rs`、`tests/realtikvtest/importintotest3/compression_harness.rs`、`tests/realtikvtest/importintotest4/split_file_test.rs`。
- Go 对照：`pkg/lightning/mydump/loader_test.go`、`reader_test.go`、`region_test.go`、`schema_import_test.go`；仓库搜索确认无同路径 `test_support.go`。
- 人工复核重点：公开边界、快照与锁生命周期、缺失路径错误、压缩参数被忽略、`file` 默认字段以及 Go 测试设施差异，均由上述符号与调用点直接支持；未把内存夹具描述成持久化存储或解压实现。
