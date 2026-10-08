# `pkg/util/stmtsummary/v2/reader.rs`

## 文件定位

[`reader.rs`](reader.rs) 是 `astersql-util-stmtsummary-v2` crate 的语句摘要读取层。crate 根在 [`lib.rs`](lib.rs) 中以私有 `mod reader` 装配并用 `pub use reader::*` 重新导出本文件的公开 API；[`Cargo.toml`](Cargo.toml) 声明该 crate 是 `pkg/util/stmtsummary/v2` 的 Rust 移植单元。本文件位于摘要记录与展示列之间：上游从内存中的 `StmtSummary` 窗口或 `tidb-statements*.log` 历史文件取得 `StmtRecord`，下游通过 [`column.rs`](column.rs) 的 `ColumnFactory` 把记录投影为查询所需的 `types::Datum`。

RustCodeGraph 对当前索引未给出 `NewMemReader`、`NewHistoryReader` 的非测试生产调用者；可以确认的使用点包括 [`stmtsummary.rs`](stmtsummary.rs) 对 `MemorySummarySource` 的实现、[`reader_test.rs`](reader_test.rs) 的独立单元测试，以及 [`tests/harness.rs`](tests/harness.rs) 和 [`tests/table_test.rs`](tests/table_test.rs) 的表级测试路径。因此，本文件提供了可供应用层接线的公开读取 API，但其生产调用链在当前 Rust 仓库中未验证，不能据此声称已经接入完整的 `INFORMATION_SCHEMA` 查询链。

## 核心职责

本文件同时实现两条读取路径，并让二者共享权限、digest、时间范围和列投影语义。

- `MemReader::Rows` 读取 `MemorySummarySource::currentWindowSnapshot` 返回的当前窗口快照，填入统一的窗口 `Begin`/当前 `End`，逐条应用 `StmtChecker`，并在没有 digest 白名单时追加非空的淘汰聚合记录。
- `HistoryReader` 枚举活动日志与轮转日志，先按文件时间边界裁剪，再通过 scan/parse worker 流水线批量读取 JSON 行、过滤记录并异步返回投影行。
- `StmtChecker` 集中实现用户可见性、digest 白名单、时间区间重叠和有序日志提前停止条件，避免内存与历史路径各自解释查询条件。
- `openStmtFile`、`StmtFiles`、`readLine`/`readLines` 处理日志发现、轮转竞态、时间元数据和输入上限；`HistoryReader::Close`、`Drop` 与 `sendCancelable` 负责取消及线程回收。

## 主要符号

- 常量 `logFileTimeFormat`、`maxLineSize`、`batchScanSize` 分别规定轮转文件时间戳格式、单行 1 GiB 上限和 64 行扫描批次。`STMT_SUMMARY_FILENAME` 是由 `RwLock<PathBuf>` 保护的全局日志路径，公开设置入口为 `setStmtSummaryFilename`。
- `ReaderError(String)` 统一包装 I/O、JSON 和线程 panic 的文本错误，可跨通道发送；它实现 `Display`、`Error` 以及从 `io::Error`、`serde_json::Error` 的转换。
- `StmtTimeRange { Begin, End }` 与 `StmtChecker` 表达查询过滤条件。`hasPrivilege` 在无 PROCESS 权限且指定用户时要求 `AuthUsers` 非空并包含该用户；`isDigestValid` 将 `None` 解释为不过滤；`isTimeValid` 接受与任一区间重叠的记录；`needStop` 仅在所有区间都有闭合右端且都早于当前 begin 时停止。
- `MemWindowSnapshot` 是脱离内部锁的当前窗口值快照；`MemorySummarySource` 是 `Send + Sync` 数据源接口；`NewMemReader` 构造 `MemReader`，`MemReader::Rows` 同步返回全部当前行。
- `StmtFile` 保存打开的 `File`、首条合法记录的 begin、由文件名推导的 end 与路径。`StmtFileCandidate` 允许活动文件携带已经钉住的句柄，轮转候选则延迟打开；`StmtFiles` 保存候选集合和活动 inode 元数据。
- `NewHistoryReader` 启动调度线程并返回 `HistoryReader`；`HistoryReader::Rows` 每次取得一个非空批次，通道正常关闭时返回 `Ok(None)`；`HistoryReader::Close` 发出取消并 join 调度线程，`Drop` 保证遗漏显式关闭时仍尝试回收。
- 内部函数 `scheduleTasks`、`scanWorker`、`readBatch`、`parseWorker` 组成历史流水线；`buildRow`/`buildValues` 执行列工厂投影；`sendCancelable` 为有界通道提供可取消发送；`timeRangeOverlap` 进行包含端点的重叠判断。

## 执行流程

内存路径从 `NewMemReader` 开始：构造函数用请求列创建 `column_factories`，用实例地址和时区创建 `ColumnContext`，并冻结查询过滤条件。`MemReader::Rows` 请求窗口快照；数据源为空、摘要已关闭或窗口不与查询时间相交时直接返回空列表。随后它以调用时的 `unixNow()` 作为窗口 end，依次检查每条记录的 digest 和用户权限，覆盖记录的 `Begin`/`End` 后调用 `buildRow`。最后，仅当没有指定 digest 集合时，才对 `ExecCount > 0` 且有权限的 `evicted` 记录做相同投影。

历史路径从 `NewHistoryReader` 开始：`StmtFiles::new` 先打开活动文件以钉住 inode，再枚举同目录中扩展名相同且 stem 等于配置 stem 或以 `stem-` 开头的文件，去重相同 inode 并按路径排序。构造器把并发度提升到至少 2，创建容量为并发度的 rows/error 通道，并生成共享 `StmtChecker`、列工厂、取消标志和调度线程。

`scheduleTasks` 将 `concurrent / 2` 个线程先作为 scan worker，扫描结束后再转为 parse worker；其余线程从一开始只解析。管理循环延迟打开轮转候选，重新检查 inode 去重和文件级时间交集，然后经零容量 `files` 通道交给 scanner。scanner 用 `readBatch` 跳过损坏的批首 JSON，遇到 `needStop` 或 EOF 结束文件，并以 64 行为上限发送原始字节批次。所有 scanner 报告完成后，调度器关闭 lines 发送端；parser 排空剩余批次，反序列化 `stmtPersistedRecord`，跳过损坏行和 `evicted=true` 行，应用停止/时间/digest/权限条件，并经 `buildValues` 产生行。全部 worker join 后发送端析构，前台 `HistoryReader::Rows` 最终观察到通道关闭并返回 `None`。

## 数据与状态

过滤状态在读取器构造后保持不变。`StmtChecker` 的 `digests: Option<HashSet<String>>` 区分“未提供过滤器”和“提供空集合”：前者接受所有 digest 并允许内存淘汰行，后者不匹配任何普通记录且不会附加淘汰行。`time_ranges` 为空表示全时间；`timeRangeOverlap` 把 `End == 0` 或 `End < Begin` 解释为开放右端，并按 `a_begin <= b_end && a_end >= b_begin` 接受端点相交。

内存读取不持有 `StmtSummary` 内部锁遍历记录。实际实现 [`stmtsummary.rs`](stmtsummary.rs) 的 `currentWindowSnapshot` 在检查 `closed` 后锁住窗口，克隆 LRU 中每个受锁保护的 `StmtRecord` 和可选淘汰聚合，再把值快照交给读取器；`MemReader` 修改 `Begin`/`End` 只影响克隆值。

历史文件状态分为路径候选和打开句柄。活动文件在目录枚举前打开，其 metadata 用于识别轮转期间出现的同一 inode；非活动文件只在即将派发时打开，使打开描述符数量受 scan worker 和零容量 handoff 限制。文件 `begin` 来自首条合法 JSON 的 `begin`；活动文件或不匹配轮转命名的文件 `end=0`，匹配 `{configured_stem}-{timestamp}` 的文件以本地时区解析 end。

## 依赖与调用关系

RustCodeGraph 核对出的核心边为：`NewHistoryReader → scheduleTasks`，`scheduleTasks → openStmtFile/sameFile/StmtChecker::isTimeValid/scanWorker/parseWorker/sendCancelable`，`scanWorker → readBatch/sendCancelable`，`parseWorker → StmtChecker::{needStop,isTimeValid,isDigestValid,hasPrivilege}/buildValues/sendCancelable`，以及 `MemReader::Rows → MemorySummarySource::currentWindowSnapshot/unixNow/StmtChecker::{isTimeValid,isDigestValid,hasPrivilege}/buildRow`。

crate 内部依赖中，`StmtRecord` 来自 [`record.rs`](record.rs)，`ColumnContext`、`ColumnFactory`、`ColumnValue` 和 `makeColumnFactories` 来自 [`column.rs`](column.rs)，列元信息和 Datum 兼容类型由 [`lib.rs`](lib.rs) 转出。外部依赖与 [`Cargo.toml`](Cargo.toml) 一致：`chrono`/`chrono-tz` 解析轮转时间并保存展示时区，`crossbeam-channel` 提供有界通道、超时发送和 `select!`，`serde`/`serde_json` 反序列化轻量与完整记录；标准库提供文件、锁、原子量和线程生命周期。

上游实证包括 [`stmtsummary.rs`](stmtsummary.rs) 的 `impl MemorySummarySource for StmtSummary`，以及 [`tests/harness.rs`](tests/harness.rs) 中按当前用户身份和 PROCESS 权限构造 `NewMemReader` 的表级模拟查询。由于图索引未确认非测试生产调用者，新增应用接线前应继续从实际系统表执行入口验证，而不是只依赖 re-export。

## 错误处理与边界

构造历史读取器时，目录枚举失败、活动文件 metadata 失败等会立即返回 `ReaderError`。单个候选文件在调度阶段无法打开或无法解析文件名时间戳时会被跳过；候选 metadata 读取失败、扫描 I/O/超长行和 worker panic 会尝试写 error 通道并设置取消标志。`HistoryReader::Rows` 优先监听 error 通道，在 rows 通道关闭后还会再 `try_recv` 一次错误，以减少尾部错误被正常结束覆盖的机会。

输入容错有意分层：`parseBeginTsAndReseek`、`readBatch` 和 `parseWorker` 都跳过非法 JSON；空文件在 `openStmtFile` 中得到 `begin=0`；历史 `evicted` 行明确不参与查询。`readLine` 在无字节时返回 `UnexpectedEof`，超过 `maxLineSize` 返回 `InvalidData`，并依次剥离 `\n`、`\r`。时间戳解析要求本地时间唯一；DST 模糊时间返回 `ReaderError("ambiguous statement log timestamp")`。

需要注意两类静默边界：全局日志路径锁中毒会由 `expect` panic；`Drop` 忽略 `Close` 返回的调度线程 panic 错误。另一个兼容约束是文件级裁剪依赖日志按 begin 递增，`needStop` 一旦成立会放弃该文件剩余记录；改变日志排序必须同步改变提前停止策略。

## 并发与资源生命周期

`HistoryReader` 的取消令牌是共享 `Arc<AtomicBool>`：前台 `Close` 以 `Release` 写入，调度器和 worker 以 `Acquire` 轮询。rows、errors、lines 均为有界通道；files 通道容量为 0，保证管理器不能预先积累大量已打开文件。`sendCancelable` 每 10 ms 重试一次 `send_timeout`，因此下游停止消费后取消仍能打断背压，而不会永久阻塞在发送操作。

调度线程拥有所有 worker 的 join handle。它先等待 scan worker 完成，再关闭 lines 的原始发送端，随后 join 全部 worker；最后其持有的 rows/error sender 随线程退出而释放。`Close` 可重复调用：第一次取走并 join `scheduler`，后续调用只保持取消状态。`Drop` 调用 `Close`，让未显式关闭的读取器通常也能释放线程和文件；`StmtFile` 的 `File` 则由 Rust RAII 在候选被丢弃、scan 完成或调度退出时关闭。

内存路径本身不创建线程，也不跨调用保留窗口锁；并发一致性边界由 `MemorySummarySource` 负责。当前 `StmtSummary` 实现在一个窗口锁作用域中复制窗口 begin、LRU 记录和淘汰聚合，因此一次 `Rows` 使用自洽的值快照，但它的 end 是快照之后读取的当前 Unix 秒。

## 与 Go 版本的对应关系

同目录 [`reader.go`](reader.go) 是直接语义对照。Rust 保留了 Go 的两个 reader、`stmtChecker` 四个判断、活动 inode 钉住和延迟打开、半数 worker 先 scan 后 parse、64 行批次、损坏 JSON 跳过、历史 evicted 跳过、列工厂投影和包含端点的区间重叠公式。[`reader_test.rs`](reader_test.rs) 也对应 Go 的时间区间、文件解析、文件枚举、checker、内存读取、历史过滤和非法行用例，并额外覆盖开放区间、轮转竞态、文件描述符上界与 `ia_exec_count` 保存。

Rust 的资源表达与 Go 不同：Go 使用 `context.CancelFunc`、`WaitGroup`、显式关闭文件和 monitor select；Rust 用原子取消、有界 crossbeam 通道、线程 join 与 `File` 的 RAII 达到相同流水线生命周期。Go 的 `MemReader` 在读窗口后逐个锁定记录，Rust 通过 `MemorySummarySource` 先克隆完整快照，隔离了读取器与摘要内部锁结构。Go 从全局 config 读取文件名，Rust 当前由 `STMT_SUMMARY_FILENAME`/`setStmtSummaryFilename` 管理；生产配置如何写入该全局路径在本文件和已确认调用边中未验证。

Go 注释把 `StmtTimeRange` 描述为 `[Begin, End)`，但 Go/Rust 实现及两边测试都使用包含端点的重叠公式；本文以实际行为“闭区间端点相交，0/逆序 end 表示开放右端”为准。Rust 还将历史 `Rows` 的结束状态显式建模为 `Result<Option<...>, ReaderError>`，对应 Go 的 `(nil, nil)`。

## 扩展指南

- 新增过滤维度时，应在 `StmtChecker` 增加单一判断，并同时接入 `MemReader::Rows` 与 `parseWorker`；若可用于文件级裁剪，还要审视 `scheduleTasks`/`readBatch` 的单调性前提。同步扩展独立的 [`reader_test.rs`](reader_test.rs)，不要把测试嵌入生产文件。
- 新增或改变输出列时，优先修改 [`column.rs`](column.rs) 的列工厂，并确认 `buildRow` 与 `buildValues` 仍保持请求列顺序；使用内存和历史各一个测试证明两条路径一致。
- 改变持久化 JSON 时，同步核对 `stmtTinyRecord`、`stmtPersistedRecord`、[`record.rs`](record.rs) 的 serde 字段以及 Go `stmtTinyRecord`/`stmtPersistedRecord`。必须保留旧日志兼容性，并增加损坏行、缺省字段、evicted 与新增字段的回归用例。
- 改变日志命名或配置来源时，修改 `parseEndTs` 与 `StmtFiles::newWithReadDir`，并覆盖绝对路径、扩展名、DST/非法时间戳、轮转前后相同 inode 及活动文件缺失。配置接线还需从真实生产入口补证。
- 调整并发和通道容量时，保持“scan 完成后才关闭 lines”“所有 parser 退出后 rows sender 才释放”“取消可突破背压”三个不变量；重点回归 `go_merge_37_history_reader_bounds_open_files`、轮转钉住测试以及提前 `Close`。
- 性能风险主要是 1 GiB 单行上限带来的内存峰值、快照克隆全部 `StmtRecord`、列工厂逐字段构造，以及过高并发造成的批次/线程占用；兼容风险主要是区间端点、用户名匹配、全局路径和历史 JSON 语义。不要为了简化实现而偏离 Go 的过滤与生命周期逻辑。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标目录的 `reader.rs`、`reader_test.rs`、`reader.go` 均已索引；`node --file pkg/util/stmtsummary/v2/reader.rs` 阅读了 1–812 行；对 `NewMemReader`、`NewHistoryReader`、`scheduleTasks`、`scanWorker`、`parseWorker`、`Rows`、`Close` 执行了 query/callers/callees，其中关键调用边记录于“依赖与调用关系”，公开构造器未返回非测试生产 callers。
- 源码与 crate 边界：完整核对 [`reader.rs`](reader.rs)、[`Cargo.toml`](Cargo.toml) 和 [`lib.rs`](lib.rs)；读取 [`stmtsummary.rs`](stmtsummary.rs) 的 `MemorySummarySource` 实现及 [`tests/harness.rs`](tests/harness.rs) 的 reader 使用点。
- Go 对照：完整核对 [`reader.go`](reader.go) 的两个 reader、checker、文件发现、scan/parse worker、取消和辅助读取函数。
- 测试证据：完整核对 [`reader_test.rs`](reader_test.rs) 的 10 个测试，涵盖开放/闭合时间范围、首行损坏、活动 inode 轮转、打开文件数、IA 执行计数、文件裁剪、权限/digest、内存淘汰行、历史 evicted 跳过及损坏行跳过；测试由 [`lib.rs`](lib.rs) 的独立 `#[path = "reader_test.rs"] mod reader_test` 装配。
- 本任务仅新增说明文档，按计划不运行 Cargo；交付检查使用任务指定的固定 11 章节结构命令，并人工复核所有链接、符号名、未验证声明和变更范围。
