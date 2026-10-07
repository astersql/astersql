# `dumpling/export/metadata.rs`

## 文件定位

`metadata.rs` 属于 `astersql-dumpling-export` library crate。crate 根 `dumpling/export/lib.rs` 先引入公共依赖和 `stubs.rs`，再以 `include!("metadata.rs")` 将本文件拼入与 Go `dumpling/export` 包相近的单包命名空间；因此文件本身没有 `use` 或 `mod` 声明，`SystemTime`、`UNIX_EPOCH`、`tcontext`、`Conn`、`ServerInfo`、`Storage` 等名字来自 crate 根和此前包含的实现。

它位于逻辑导出主链的收尾侧：`Dumper::Dump`（`dumpling/export/dump.rs`）创建 `globalMetadata`，在导出前记录开始时间和数据库复制位点，在 writer 消费完任务后记录结束时间并把固定名 `metadata` 写入导出存储。它不生成库表 DDL 或表数据，而是为恢复、审计和故障定位保留一次导出的时间边界、主库 binlog/GTID 信息及可见的上游复制位点。

`dumpling/export/Cargo.toml` 将该目录声明为 `astersql-dumpling-export`，`[lib] path = "lib.rs"`，porting 元数据指向 Go 包 `dumpling/export`。本文件直接使用的数据库、存储、版本和日志抽象目前由同 crate 的共享命名空间（尤其 `stubs.rs`、`conn.rs`、`sql.rs`）提供，而不是由 `metadata.rs` 自己声明额外依赖或 feature。

## 核心职责

1. `globalMetadata` 累积一次导出的文本内容，并分开保存初始位点与“连接池建立后”的补充位点。
2. `recordStartTime`、`recordFinishTime` 以 `YYYY-MM-DD HH:MM:SS` 写入导出时间边界；结束时先合并 `after_conn_buffer`，保证补充位点位于完成时间之前。
3. `recordGlobalMetaData` 按 `ServerInfo.ServerType` 查询 MySQL、MariaDB 或 TiDB 的主状态，规范化为兼容的 `SHOW MASTER STATUS:` 文本；必要时继续记录 follower/replica 状态。
4. `writeGlobalMetaData` 将完整缓冲区写到 `metadataFileName == "metadata"`；没有 storage 时允许只在内存中构建。
5. `getValidStr` 对版本间字段数不同的主状态结果做越界保护，使缺少 GTID 列的旧 MySQL 返回空串而非 panic。

该文件只负责组装和落盘 metadata。`ShowMasterStatus` 如何根据 MySQL 版本选择 `SHOW MASTER STATUS` 或 `SHOW BINARY LOG STATUS`、`Conn` 如何执行/模拟查询、`Storage` 如何持久化，分别是相邻 SQL/连接/存储抽象的职责。

## 主要符号

- `pub struct globalMetadata`：导出级可变累积器。`tctx` 用于 MariaDB GTID 查询失败时告警；`buffer` 保存最终正文；`after_conn_buffer` 暂存第二阶段位点；`storage: Option<Arc<dyn Storage>>` 是可共享输出端；`snapshot` 是 TiDB 显式快照位置。
- `pub const metadataFileName: &str = "metadata"`：Rust 写入和测试读取共同使用的固定文件名。
- `pub fn newGlobalMetadata(...) -> globalMetadata`：只初始化字段，不查询数据库、不创建文件。`snapshot: impl Into<String>` 允许字符串切片和已有字符串作为输入。
- `impl Display for globalMetadata`：将 `buffer` 作为 UTF-8 展示；若内部字节不是合法 UTF-8则展示空串。正常生产写入均来自 Rust 字符串，数据库原始字节也先经有损 UTF-8 转换。
- `globalMetadata::recordStartTime(SystemTime)`：追加 `Started dump at: ...`。
- `globalMetadata::recordFinishTime(SystemTime)`：把 `after_conn_buffer` 追加到主缓冲区，再追加 `Finished dump at: ...`。它不会清空补充缓冲区，重复调用会重复合并。
- `globalMetadata::recordGlobalMetaData(&Conn, &ServerInfo, bool) -> Result<()>`：选择目标缓冲区；`after_conn=true` 时先清空旧补充内容，再委托同名自由函数。
- `globalMetadata::writeGlobalMetaData() -> Result<()>`：存在 storage 时调用 `Storage::WriteFile(metadataFileName, &buffer)` 并传播错误；storage 为 `None` 时成功空操作。
- `format_time` / `civil_from_days`：内部纯函数，把相对 Unix epoch 的整秒换算为公历 UTC 样式文本；支持 epoch 之前的负天数，不保留亚秒。
- `pub fn recordGlobalMetaData(...) -> Result<()>`：核心数据库分支和文本编码函数。虽为 `pub`，当前生产代码经结构体方法间接调用；独立测试也直接或间接覆盖它。
- `pub fn getValidStr(&[String], usize) -> String`：安全克隆指定字段，缺失时返回空字符串。

文件没有 trait、enum、宏或条件编译项。公开面保留 Go 风格命名，crate 根通过 lint allow 接受这些非 Rust 惯用名称。

## 执行流程

生产主链可由 `Dumper::Dump`（`dumpling/export/dump.rs:128-213`）还原：

1. consistency controller 完成 `Setup` 后取得一致性连接。
2. `newGlobalMetadata(tctx, ext_storage, conf.Snapshot)` 创建累积器，`recordStartTime(SystemTime::now())` 写开始时间。
3. 以 `after_conn=false` 记录初始全局位点。该调用失败时，`Dumper::Dump` 只记录 `get global metadata failed` 警告并继续导出；因此初始 metadata 查询不是导出任务的硬失败点。
4. 主流程准备表清单、生成任务、用单个 writer 顺序消费 channel 中的任务并关闭相关连接。
5. `recordFinishTime(SystemTime::now())` 合并可能存在的第二阶段位点并写完成时间；`writeGlobalMetaData()` 的存储错误作为 `Dump` 的最终错误返回。

核心自由函数 `recordGlobalMetaData` 的分支如下：

1. MySQL/TiDB：调用 `ShowMasterStatus`，取索引 0 的日志文件、索引 1 的位置、索引 4 的 GTID。仅 TiDB 且 `snapshot` 非空时以配置 snapshot 替代查询位置。
2. MariaDB：同样取得日志文件和位置；另执行 `SELECT @@global.gtid_binlog_pos`。该附加查询失败只告警并保留空 GTID，成功则取首行首列。
3. 未知 server type：立即返回 `unsupported serverType ...`，不向缓冲区写内容。
4. 日志文件非空时写 master status；`after_conn=true` 会在标题追加 `/* AFTER CONNECTION POOL ESTABLISHED */`。无日志文件时跳过该段，但随后仍追加一个换行。
5. TiDB 或 `after_conn=true` 到此返回，不读取 follower 状态。
6. 其他初始采样先用 `SELECT @@default_master_connection` 判断 MariaDB multi-source：查询成功且存在一行时选 `SHOW ALL SLAVES STATUS`；否则 MySQL 8.4+ 选 `SHOW REPLICA STATUS`，其余选 `SHOW SLAVE STATUS`。
7. 逐行读取动态列集合，列名以 ASCII 小写匹配新旧术语：位置接受 `exec_master_log_pos`/`exec_source_log_pos`，日志接受 `relay_master_log_file`/`relay_source_log_file`，host 接受 `master_host`/`source_host`，GTID 接受 `executed_gtid_set`/`gtid_io_pos`。只有 host 非空才输出该 follower；multi-source 还输出 connection name。

RustCodeGraph 的调用结果表明，`after_conn=true` 当前由 `metadata_test.rs` 验证，但 `Dumper::Dump` 生产主链只调用一次 `after_conn=false`。因此双缓冲设计和输出格式已实现，生产中的第二次采样尚未在本文件的直接上游接线。

## 数据与状态

`globalMetadata` 的核心不变量是：`buffer` 是最终写入内容，`after_conn_buffer` 只有在 `recordFinishTime` 时进入最终内容。初始位点直接追加到 `buffer`；每次 after-connection 采样先清空补充缓冲区，故只保留最近一次成功/部分成功采样结果。`recordFinishTime` 本身不标记对象已完成，调用方必须保证只调用一次并在最后写文件。

输出是面向人的稳定纯文本，而非结构化序列化。master/follower 段中的空字段仍以空值行保留；主状态日志文件为空时不输出整段；follower host 为空时忽略该行。数据库字节经 `String::from_utf8_lossy` 转换，非法 UTF-8 会以替换字符进入 metadata，而不会导致查询流程失败。

时间换算只使用 `SystemTime` 到 Unix epoch 的整秒数，`format_time` 不携带时区或偏移信息，并按 UTC epoch 算术得到日期。`civil_from_days` 使用 400 年公历周期换算，不访问系统时区和时钟状态。与 Go `time.Time.Format(time.DateTime)` 相比，Rust 不保留传入时间的 location 语义；当前生产输入是 `SystemTime::now()`，测试则用 epoch 固定值锁定格式。

`storage` 以 `Arc<dyn Storage>` 共享，但 metadata 对象自身没有内部锁；所有缓冲区修改都要求 `&mut self`。`snapshot` 在构造后只读，`ServerInfo` 和连接由调用方借用。

## 依赖与调用关系

上游关系：

- `dumpling/export/lib.rs` 在运行期基础设施层包含本文件，并在 `#[cfg(test)]` 下把 `metadata_test.rs` 注册为独立测试模块。
- `Dumper::Dump` 是 RustCodeGraph 找到的生产上游：创建对象、记录开始/初始状态，导出结束后记录完成并写文件。
- `dumpling/export/metadata_test.rs` 是主要测试调用方，覆盖构造器、两个时间方法、结构体方法、存储写入和数据库类型分支。

下游关系：

- `ShowMasterStatus(&Conn, &ServerInfo)` 提供版本相关主状态查询；MySQL 8.4 的实际查询名变化由该下游处理，metadata 的输出标题仍保持 `SHOW MASTER STATUS:` 兼容文本。
- `Conn::QueryContext`、rows 的 `Next`/`Scan`/`Columns`/`Close` 支撑 MariaDB GTID 和 follower 查询。
- `ServerInfo.ServerType`、`ServerInfo.ServerVersion`、`parse_semver("8.4.0")` 决定数据库类型与 MySQL 术语分支。
- `tcontext::Context::L().Warn` 只用于 MariaDB GTID 的可降级错误；`errors_errorf` 构造未知 server type 错误。
- `Storage::WriteFile` 是 Rust 当前唯一落盘动作。

Cargo 边界方面，本文件处于 library crate，不是命令入口。它使用的 dumpling context/log 和 objstore store API 均由 `dumpling/export/Cargo.toml` 的本地 path 依赖供给；没有 metadata 专属 feature 或平台条件。

## 错误处理与边界

- `ShowMasterStatus` 失败会原样向上传播，并且发生在任何 master 文本写入之前。`test_no_privilege` 证明权限错误返回 `Err` 且缓冲区保持空。
- 未知 server type 是硬错误；`test_unsupported_server_type` 锁定错误中包含 `unsupported serverType Unknown` 且无输出。
- MariaDB `@@global.gtid_binlog_pos` 查询、无行或扫描失败均被降级为警告/空 GTID；rows close 的错误在此分支被忽略。这与主状态和 follower 查询的硬错误策略不同。
- multi-source 探测失败被解释为“不是 multi-source”，随后回退到 MySQL/通用 follower 查询；这会隐藏探测错误，但保持 Go 路径的兼容降级。
- follower 主查询、列读取、行扫描和最终 close 的错误都会通过 `?` 返回。函数按流式方式直接写目标缓冲区，因此后段出错时可能留下已经写入的 master 或部分 follower 文本，不具备事务性回滚。
- `getValidStr` 使旧 MySQL 缺少 GTID 列时得到空字符串。动态 follower 行中的 SQL NULL 经 `unwrap_or(b"")` 处理为空字段。
- `Display` 遇到非法 UTF-8 返回空展示，但 `writeGlobalMetaData` 直接写原始 `buffer`；当前本文件生成路径保证字符串编码合法，此差异仍是手工修改公开 buffer 时的边界。
- storage 为 `None` 时写入是成功空操作。storage 存在时 `WriteFile` 错误直接传播；该路径由 `Dumper::Dump` 作为导出末尾硬错误处理。
- `SystemTime` 早于 epoch 时可格式化，但换算只取整秒，负的亚秒时间会丢失亚秒精度；本文件没有年份范围校验。

## 并发与资源生命周期

本文件不创建线程、异步任务、channel、锁或事务。`globalMetadata` 由 `Dumper::Dump` 栈上独占持有，缓冲区修改依靠 Rust 的 `&mut self` 保证同一时刻只有一个写者；`Arc<dyn Storage>` 只是与 Dumper/writer 共享存储句柄，不表示 metadata 自身可并发写。

数据库 rows 的生命周期是同步且局部的。MariaDB GTID 查询无论是否取得值都会尝试 `Close`，但忽略 close 错误；multi-source 探测在读取首行后以 `?` 传播 close 错误；follower rows 在完整迭代后关闭并传播 close 错误。连接本身由上游创建和关闭，本文件只借用，不拥有连接生命周期，也不开始事务。

输出文件直到所有 writer 任务处理完成后才由 `writeGlobalMetaData` 一次写出。当前 Storage API 调用的原子性、覆盖方式和远端资源回收由具体 `Storage` 实现决定，本文件没有临时文件、rename、压缩或显式 teardown。若导出在末尾写入前退出，内存中的 metadata 不会由本对象自动持久化。

## 与 Go 版本的对应关系

Rust 的 `globalMetadata`、`newGlobalMetadata`、`recordStartTime`、`recordFinishTime`、两层 `recordGlobalMetaData`、`writeGlobalMetaData` 和 `getValidStr` 与 `dumpling/export/metadata.go` 基本一一对应。数据库分支、字段索引、TiDB snapshot 覆盖、MariaDB GTID 降级、multi-source 探测、MySQL 8.4 replica 术语兼容、动态 follower 列匹配和文本格式均保留了 Go 测试意图。

已确认的当前差异包括：

- Go 使用 `bytes.Buffer` 和 `time.Time.Format(time.DateTime)`；Rust 使用 `Vec<u8>` 和自行实现的 epoch/公历换算，因而没有 Go `time.Location` 语义。
- Go `writeGlobalMetaData` 经 `buildFileWriter(..., NoCompression)`、`write`、`tearDown` 写入，并规定写错误优先于 teardown 错误；Rust 直接调用 `Storage::WriteFile`，没有该 writer/teardown 生命周期。两者都保持 metadata 不压缩的外部目标，但底层过程并非等价实现。
- Go storage 是必填接口；Rust 用 `Option<Arc<dyn Storage>>`，允许无存储的内存模式。
- Go 的 MariaDB 单值读取使用 `QueryRowContext(context.Background())`；Rust 使用 `QueryContext`、首行扫描和显式 close，并将扫描失败也降级为空 GTID。
- Rust 的 `Display` 对非法 UTF-8返回空字符串；Go `bytes.Buffer.String()` 保留任意字节到 string 的直接视图。
- Go `Dump` 也创建并最终写 metadata；Rust 当前主链同样执行初始采样，但 RustCodeGraph 未显示生产路径调用 `after_conn=true`。不能仅凭双缓冲实现断言连接池建立后的采样已经在 Rust 生产流程启用。

测试对应关系由 `dumpling/export/metadata_test.go` 与独立的 `metadata_test.rs` 提供。Rust 测试覆盖 MySQL 8.0/8.4、旧 MySQL 无 GTID、TiDB snapshot、MariaDB 及 multi-source follower、after-connection 合并、时间/存储、权限错误和未知类型；Go 用例还用 sqlmock 更完整地断言多 follower 行和查询期望。扩展时应同时核对两套测试，不能把 Rust 当前较轻的 fixture 当作删减 Go 行为的依据。

## 扩展指南

- 新增数据库类型或改变 master status 规则：修改自由函数 `recordGlobalMetaData` 的 server type 分支，并同步 `metadata_test.rs` 与 `metadata_test.go` 的格式和错误用例；保持未知类型在写入前失败。
- 支持新的 MySQL/MariaDB 复制术语：优先扩展 follower 列名匹配和查询选择，确保旧、新列名可共存；至少添加空列、NULL、多行和大小写混合测试。
- 真正启用连接池建立后采样：应在拥有连接池生命周期的 `Dumper::Dump` 或对应编排点调用结构体方法的 `after_conn=true` 路径，而不是在本文件猜测时机；验证它只覆盖 `after_conn_buffer`，并在 `recordFinishTime` 前合并一次。
- 改变输出格式时须考虑恢复工具/人工脚本对固定标题、缩进、空行和 `metadata` 文件名的兼容依赖。建议扩展现有完整字符串断言，避免只用 `contains` 掩盖格式回归。
- 增强写入可靠性时，可评估对齐 Go 的 file writer、无压缩和 teardown 错误优先级；这会跨越本文件与 Storage/writer 抽象，不能仅替换一次调用后宣称语义完全对齐。
- 调整时间语义时，应先明确需要 UTC、系统本地时区还是显式 offset，再修改 `format_time`/`civil_from_days`；加入 epoch 前、闰日、世纪边界和非 UTC 需求的独立测试。
- 若要并发采样或后台刷新，不应直接共享可变 `globalMetadata`；需要在上游明确同步、取消和连接/rows 所有权，并证明最终写入相对 writer 完成的顺序。
- Rust 单元测试必须继续放在独立的 `dumpling/export/metadata_test.rs`，由 `lib.rs` 的 `#[cfg(test)]` 模块挂载，不要内嵌回生产文件。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、7,032 个 Rust 文件；目标文件已索引。
- RustCodeGraph `files --filter dumpling/export`：确认 `metadata.rs`、`metadata_test.rs`、Go 对照文件和 crate 入口均在索引中。
- RustCodeGraph `node --file dumpling/export/metadata.rs`：核对 247 行实现的全部结构、分支、资源关闭与文本写入。
- RustCodeGraph `explore 'dumpling/export/dump.rs newGlobalMetadata recordStartTime recordGlobalMetaData recordFinishTime writeGlobalMetaData'`：确认结构体方法到自由函数的调用边、`Dumper::Dump` 生产调用者，以及测试调用者集合；结果显示生产主链没有 `after_conn=true` 的第二次调用。
- RustCodeGraph `node --file dumpling/export/dump.rs --offset 128 --limit 100`：核对 metadata 相对 consistency setup、任务生成/消费、连接关闭和最终落盘的顺序。
- RustCodeGraph `node --file dumpling/export/lib.rs`：核对 `include!("metadata.rs")`、crate 共享导入和独立 `metadata_test.rs` 挂载。
- `dumpling/export/Cargo.toml`：核对 crate 名称、library 边界、Go package 映射及 context/log/store API 等依赖来源。
- RustCodeGraph `node` 阅读 `dumpling/export/metadata.go` 与 `metadata_test.go`：核对 Go 的查询选择、输出格式、writer/teardown 写入路径及 MySQL/MariaDB/TiDB 测试意图。
- RustCodeGraph `node --file dumpling/export/metadata_test.rs`：核对 Rust 的 MySQL 8.0/8.4、after-connection、follower、MariaDB、TiDB snapshot、时间/存储和错误路径覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文恰有 11 个固定二级标题，并人工复核唯一生产物、源码链接、已验证事实与明确的未接线边界。
