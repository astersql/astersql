# `pkg/lightning/mydump/reader.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-lightning-mydump`（见 `pkg/lightning/mydump/Cargo.toml`），由 `pkg/lightning/mydump/lib.rs` 以 `mod reader; pub use reader::*;` 纳入并公开。它位于 Lightning/mydumper 导入链的输入边界，承担两类职责：读取并规范化 schema SQL 文件，以及为 SQL/CSV 分块解析器提供可 seek、可限流的读取器。它不是 SQL 语法解析器；行/Token 解析继续由 `parser.rs`、`csv_parser.rs` 等模块完成。

直接上游分为两条链：`loader.rs::MDDatabaseMeta::GetSchema` 和 `MDTableMeta::GetSchema` 调用 `ExportStatement` 取得建库、建表或建视图 SQL；`parser.rs::makeBlockParser` 调用 `MakePooledReader`，把数据文件流装入 `BlockParser`。`lib.rs` 将本文件 API 再导出给 `lightning/pkg/importer`、`pkg/executor/importer` 及测试使用。

## 核心职责

1. `ExportStatement` 通过本地 `Storage` 抽象打开 `FileInfo` 指向的对象，逐行拼接以分号结尾的 schema 语句，剥离文件头 UTF-8 BOM，并忽略形如 `/* ... */;` 的整段块注释。
2. `decodeCharacterSet` 将读取结果统一成 UTF-8 字节：`binary` 原样通过，`utf8mb4` 严格校验，`auto` 先接受合法 UTF-8、否则尝试 GB18030，`latin1` 按 MySQL 兼容习惯使用 Windows-1252。
3. `StringReader` 为内存字节提供 `Read + Seek`，主要用于构造解析器和测试。
4. `PooledReader` 在底层 `ReadSeekCloser` 外按单次 `read`、非查询型 `seek` 或 `ReadFull` 操作获取 `WorkerPool` 令牌，限制共享池上的并发 I/O。

这里的 schema 处理是轻量边界整理，不是通用 SQL lexer：语句完成条件仅为累计字节的最后一个字节是 `;`，块注释过滤也只识别整条累计内容以 `/*` 开始并以 `*/;` 结束的情形（`ExportStatement`、`trim_ascii`）。

## 主要符号

- `ErrInsertStatementNotFound: &str`：与 Go 错误文本对齐的公开常量；本文件内不消费它，供 crate 的解析逻辑或外部调用者复用。
- `decodeCharacterSet(Vec<u8>, &str) -> Result<Vec<u8>, MydumpError>`：字符集转换主实现。`decode_character_set` 是等价的 snake_case 转发入口。
- `Storage: Send + Sync`：最小存储接口。`open(path, compression)` 返回 `Box<dyn Read + Send>`，`list` 默认返回空列表。压缩解释由具体存储实现负责。
- `ExportStatement(&dyn Storage, &FileInfo, &str) -> Result<Vec<u8>, MydumpError>`：schema 读取主入口；`export_statement` 是 snake_case 别名。
- `trim_ascii(&[u8]) -> &[u8]`：只裁剪 ASCII 空白，返回原切片的子视图，不分配。
- `ReadSeekCloser: Read + Seek + Send`：Rust 侧的组合 trait；通过 blanket impl 自动覆盖所有满足约束的类型。名称保留 Go 兼容语义，但 trait 本身没有 `close` 方法。
- `StringReader(Cursor<Vec<u8>>)`、`NewStringReader`、`from_bytes`：内存读取器。`Close` 是空操作。
- `WorkerPool::new`、`WorkerPool::acquire`、`WorkerGuard`：基于 `Mutex<usize> + Condvar` 的计数信号量。构造时把零容量提升为 1；守卫析构时归还令牌并唤醒一个等待者。
- `PooledReader`、`MakePooledReader`：保存可选底层 reader 和可选共享工作池；实现标准 `Read`、`Seek`，并提供 Go 风格的 `Read`、`Seek`、`ReadFull`、`Close` 方法。
- `reader_closed`：在 `Close` 后访问底层 reader 时构造 `ErrorKind::BrokenPipe`。

## 执行流程

schema 路径从 `loader.rs` 的元数据对象进入。`ExportStatement` 将 `file.file_meta.path` 和 `compression` 传给 `Storage::open`，再用 `BufReader` 逐次 `read_until(b'\n')`。第一次读到的数据若以 `EF BB BF` 开头就移除 BOM；每行经 `trim_ascii` 后，空行跳过，非空行追加到 `stmt`。若累计缓冲以分号结束，则纯块注释被丢弃，其他内容追加到最终 `data`；否则追加换行继续累计。EOF 后，非空且不是完整块注释的残留会触发“缺少尾分号”语法错误。最后 `decodeCharacterSet` 解码并返回字节。

`auto` 解码先用 `std::str::from_utf8` 检查；成功即不复制地返回原 `Vec`，失败则走 `encoding_rs::GB18030.decode`。显式 `utf8mb4` 遇到非法 UTF-8 直接失败。GB18030/Windows-1252 解码若报告错误或结果含替换字符 `U+FFFD`，都映射为 `MydumpError::Encoding`。

数据解析路径由 `parser.rs::makeBlockParser` 接收 `Box<dyn ReadSeekCloser>` 与可选 `Arc<WorkerPool>`，调用 `MakePooledReader` 后存入 `BlockParser.reader`。普通 `read` 先取得令牌，再直接调用底层流；`seek(Current(0))` 仅查询当前位置，不获取令牌，其他 seek 会限流；`ReadFull` 在一次令牌持有期间直接对底层流执行 `read_exact`，避免每个内部读取动作重复回收令牌。`Close` 对 `Option` 执行 `take`，立即析构底层对象；重复关闭成功，但后续 I/O 返回 `BrokenPipe`。

## 数据与状态

`ExportStatement` 的可变状态只有 `data`（已完成语句）、`stmt`（当前累计语句）、`line`（单行复用缓冲）和 `first`（BOM 只处理一次）。它一次性把输出保存在内存中，内存上界与解压后的 schema 内容量相关；Rust 版本没有根据 `file_size` 预分配。

`StringReader` 的状态由 `Cursor<Vec<u8>>` 持有，包含拥有所有权的字节与当前位置。`PooledReader.reader` 使用 `Option` 表达打开/关闭状态，`workers` 使用 `Option<Arc<WorkerPool>>` 表达是否启用跨 reader 的共享限流。`WorkerPool.used` 是当前已占用令牌数，不记录任务身份；不变量是正常运行时 `0 <= used <= limit`，且 `limit >= 1`。

文件没有全局可变状态。公开 API 多数保留 PascalCase 以便与 Go 移植代码对应，同时提供部分 snake_case 别名。

## 依赖与调用关系

- crate 内依赖：`Compression`、`FileInfo` 和 `MydumpError` 从 crate 根再导入；其数据定义在同 crate 的其他模块中。
- 外部依赖：只有字符集转换直接使用 `encoding_rs::{GB18030, WINDOWS_1252}`；该依赖在 `pkg/lightning/mydump/Cargo.toml` 声明。其余读取、同步与游标类型来自标准库。
- schema 上游：`loader.rs::MDDatabaseMeta::GetSchema -> ExportStatement`；读取失败或空 schema 时数据库元数据会回退生成 `CREATE DATABASE IF NOT EXISTS`。`loader.rs::MDTableMeta::GetSchema -> ExportStatement`；表/视图路径会传播错误，并把结果转为字符串。
- parser 上游：`parser.rs::NewChunkParser -> makeBlockParser -> MakePooledReader`。随后 `BlockParser::read_block` 使用 `Read`，`SetPos`/`ScannedPos` 使用 `Seek`，`Close` 释放底层 reader。
- 内存 reader 上游：`parser_test.rs`、`parser_generated_test.rs`、`csv_parser_test.rs` 以及 executor/importer 的测试和运行接线用 `NewStringReader` 构造输入。

RustCodeGraph 的文件节点显示 `reader.rs` 被 23 个文件引用；精确符号搜索与源码核对确认本文件自身没有反向依赖 loader/parser，因此边界保持为底层输入适配层。

## 错误处理与边界

`Storage::open` 和 `BufRead::read_until` 的错误通过 `?` 转为/传播为 `MydumpError`；`reader_test.rs::TestExportStatementHandleNonEOFError` 验证非 EOF 读取错误不会被当作文件结束。残留非注释内容缺尾分号时返回 `MydumpError::Syntax`，消息包含源路径。未知字符集、非法 UTF-8、GB18030 解码失败或替换字符返回 `MydumpError::Encoding`。

边界规则由 `reader_test.rs::TestExportStatementMissingTrailingSemicolon` 固化：纯尾部块注释可以没有分号，行注释不能获得同样豁免，BOM 仅允许在文件开头。空行被忽略，语句之间不额外插入分隔符。当前算法不理解引号内分号、行尾注释或嵌套注释，扩展时不能假定它已实现完整 SQL 分词。

`WorkerPool` 对 poisoned mutex 使用 `unwrap`，因此持锁线程 panic 后，后续 acquire/drop 可能继续 panic；这不是可恢复的 `io::Error`。`PooledReader::Close` 本身永远返回 `Ok(())`，因为 Rust 组合 trait 不含显式 close；底层释放错误无法从这里表达。关闭后 `Read`、`Seek`、`ReadFull` 返回 `BrokenPipe`，重复关闭是幂等的。

## 并发与资源生命周期

`Storage` 要求 `Send + Sync`，其打开的流要求 `Read + Send`；可 seek 的解析流还需满足 `ReadSeekCloser`。`PooledReader` 的 I/O 方法需要 `&mut self`，所以同一个实例不会通过安全 Rust 被多个线程同时操作；多个实例可以持有同一 `Arc<WorkerPool>`，从而共享并发上限。

`WorkerPool::acquire` 在 `used >= limit` 时用条件变量等待，处理虚假唤醒的方式是循环重检。成功后增加计数；局部 `WorkerGuard` 即使在底层 I/O 返回错误时也会因 RAII 析构而减计数并 `notify_one`，避免令牌泄漏。令牌只覆盖单次操作，不覆盖 reader 全生命周期。`SeekFrom::Current(0)` 被视为无磁盘 I/O 的位置查询，特意绕过限流。

`ExportStatement` 的 `BufReader` 和底层 `Box<dyn Read>` 在函数返回或报错时依靠 Drop 释放。`PooledReader::Close` 通过清空 `Option` 提前触发 Drop；`reader_test.rs::TestPooledReaderCloseReleasesUnderlyingReader` 用析构标志验证了立即释放，并验证关闭后读取失败。

## 与 Go 版本的对应关系

对应实现为 `pkg/lightning/mydump/reader.go`，核心分支基本逐项对齐：字符集名称区分大小写；`auto` 从 UTF-8 回退 GB18030；`latin1` 使用 Windows-1252；schema 按行裁剪、按尾分号提交并过滤纯块注释；`Seek(0, Current)` 不占 worker；`ReadFull` 只申请一次 worker。

关键实现差异如下：

- Go `ExportStatement` 接受 `context.Context` 与 `storeapi.Storage`，对压缩文件先包装存储，并在解码失败时记录带路径的日志；Rust `Storage::open` 直接接收 `Compression`，没有 context/logger，返回的编码错误也没有附加文件路径。
- Go 按 `FileSize` 为两个缓冲预分配容量；Rust 使用空 `Vec` 动态增长。
- Go reader 是 `io.ReadSeekCloser`，`Close` 调用底层显式关闭并可传播关闭错误；Rust `ReadSeekCloser` 仅组合 `Read + Seek + Send`，`Close` 通过 Drop 释放且总是成功。这个差异对需要“显式关闭可失败”的新存储后端尤其重要。
- Go `PooledReader` 方法使用值接收者并持有 `worker.Pool`；Rust 使用 `&mut self`、`Arc<WorkerPool>` 和 RAII 守卫。Rust 还明确定义关闭后的 `BrokenPipe` 行为，相关 Rust 测试比同路径 Go 测试多覆盖这一点。
- `reader_test.rs` 基本复刻 `reader_test.go` 的 schema 用例，并新增字符集大小写拒绝和底层析构验证；Go 测试使用真实临时文件/对象存储 mock，Rust 测试使用 `test_support::MemoryStorage`。

## 扩展指南

- 新增字符集时修改 `decodeCharacterSet` 的匹配分支，并在独立的 `reader_test.rs` 增加有效、非法字节、替换字符和字符集名称大小写用例；同时核对 `reader.go`，避免 Rust/Go 接受集合漂移。
- 修改 schema 切分或注释处理时优先扩展 `ExportStatement`，但若需求涉及引号内分号、SQL 注释词法或 delimiter 指令，应评估复用真正的 lexer，而不是继续叠加字节前后缀判断。同步覆盖无尾换行、BOM、空行、纯块注释、行注释、缺分号、压缩和非 EOF 错误。
- 新增存储后端需实现 `Storage::open`，并确保传入的 `Compression` 被正确处理；若资源关闭可能失败，当前 trait/`PooledReader::Close` 无法表达该错误，改变契约会影响 `parser.rs::BlockParser` 和所有实现者。
- 调整限流策略时修改 `WorkerPool::acquire` 或 `PooledReader` 的三条 I/O 路径，并补充独立并发测试，验证上限、等待唤醒、错误时令牌归还、`Current(0)` 绕过和零容量归一化。不要把测试嵌入 `reader.rs`。
- 兼容性风险主要是 schema 文本输出变化和错误类型/消息变化；性能风险主要是 schema 全量驻留、逐行拼接重分配，以及 `ReadFull` 长时间持有令牌造成的公平性影响。

## 验证依据

- Rust 主实现：`pkg/lightning/mydump/reader.rs`，已核对全部 274 行及 `decodeCharacterSet`、`Storage`、`ExportStatement`、`StringReader`、`WorkerPool`、`PooledReader`、`reader_closed`。
- crate 边界：`pkg/lightning/mydump/Cargo.toml` 与 `pkg/lightning/mydump/lib.rs`；确认包名、`encoding_rs` 依赖、公开再导出和独立测试挂载。
- 直接调用证据：`pkg/lightning/mydump/loader.rs` 的 `MDDatabaseMeta::GetSchema`、`MDTableMeta::GetSchema`；`pkg/lightning/mydump/parser.rs` 的 `makeBlockParser`、`BlockParser::{read_block,SetPos,ScannedPos,Close}` 和 `NewChunkParser`。
- Rust 独立测试：`pkg/lightning/mydump/reader_test.rs`，覆盖无尾换行、块注释、GB18030、乱码、字符集名称大小写、非 EOF I/O 错误、gzip、尾分号/BOM和关闭后行为。
- Go 对照：`pkg/lightning/mydump/reader.go` 与 `pkg/lightning/mydump/reader_test.go`，用于核对移植分支、worker 使用、压缩/上下文/日志及显式关闭差异。
- RustCodeGraph：执行了 `status`、`files --filter pkg/lightning/mydump`、针对 `reader.rs`/`MakePooledReader` 的 `explore`、`query`、文件节点读取；索引包含 11,467 个文件，目标文件有 36 个符号，调用流确认 `parser.rs::makeBlockParser -> reader.rs::MakePooledReader`。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令和人工事实复核验收。
