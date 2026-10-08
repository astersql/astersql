# `pkg/util/stmtsummary/v2/logger.rs`

## 文件定位

[`logger.rs`](logger.rs) 属于 `astersql-util-stmtsummary-v2` crate。crate 根 [`lib.rs`](lib.rs) 以私有 `mod logger` 装载它，再用 `pub use logger::*` 导出其公开项；[`Cargo.toml`](Cargo.toml) 表明本 crate 是 `pkg/util/stmtsummary/v2` 的 Rust 移植单元，并直接依赖 `serde`、`serde_json`。本文件负责语句摘要记录的 JSON 行编码，以及一个面向任意 `std::io::Write` 的通用存储适配器。

当前接线需要分开理解：`marshalStmtRecord` 与 `marshalEvictedStmtRecord` 已由 [`stmtsummary.rs`](stmtsummary.rs) 的 `fileStmtStorage::writeRecord` 生产路径调用；而本文件的 `StmtLogStorage<W>`、`StmtWindowForLog` 和 `newStmtLogStorage` 在仓库内没有生产调用者或 trait 实现，直接使用证据仅见独立测试 [`column_1_aster_unit_test.rs`](column_1_aster_unit_test.rs) 的 `logger_json_keeps_flat_record_and_markers`。因此它们是可复用且已测试的 writer 抽象，但不能描述成当前 `StmtSummary` 的实际存储后端。

## 核心职责

1. `marshalStmtRecord`、`marshalEvictedStmtRecord` 和 `marshalStmtRecordWithEvicted` 将 [`record.rs`](record.rs) 定义的 `StmtRecord` 编码为 JSON，同时保持记录字段扁平，并按需增加顶层 `evicted: true` 或嵌套的 `additional_fields`。
2. `setStmtLogAdditionalFields` 管理进程内全局附加字段；`persistedEvictedCount` 暴露成功写入的逐条淘汰记录累计数，供测试或观测使用。
3. `encodeStmtLogEntry` 实现“消息原文加一个换行”的轻量编码，对应 Go `stmtLogEncoder.EncodeEntry` 的输出契约。
4. `StmtLogStorage<W>` 提供窗口批量持久化、单条普通记录写入、逐条淘汰记录批量写入、刷新和取回 writer 的能力。

它不负责创建/滚动真实日志文件、启动后台线程、决定何时淘汰或轮转窗口，也不维护 LRU；这些职责当前位于 [`stmtsummary.rs`](stmtsummary.rs) 的 `fileStmtStorage`、`onEvict`、`evictedLogLoop`、`rotateLoop` 和 `rotateWindow`。

## 主要符号

- `STMT_LOG_ADDITIONAL_FIELDS: LazyLock<RwLock<HashMap<String, String>>>`：惰性初始化的全局附加字段快照。写入者整体替换 map，序列化者持读锁完成一次编码。
- `PERSISTED_EVICTED_COUNT: AtomicU64`：仅在 `StmtLogStorage::logEvicted` 完成整批 `write_all` 后递增，使用 `Relaxed` 顺序，因为它只表达统计值，不承担跨线程同步协议。
- `setStmtLogAdditionalFields(fields)`：取得写锁并替换全局字段；锁中毒时以 `expect` panic。
- `persistedEvictedCount()`：读取累计成功数。
- `evictedStmtRecord`、`stmtRecordWithAdditionalFields`、`evictedStmtRecordWithAdditionalFields`：三个私有借用包装类型，均通过 `#[serde(flatten)]` 展开 `StmtRecord`；后两个把附加字段放在 `additional_fields` 对象中。
- `marshalStmtRecord(record)` / `marshalEvictedStmtRecord(record)`：分别以 `evicted=false/true` 委托给四分支编码函数。
- `marshalStmtRecordWithEvicted(record, evicted)`：按照“附加字段是否为空 × 是否淘汰”选择原始记录或三个包装类型之一，并调用 `serde_json::to_vec`。
- `encodeStmtLogEntry(message)`：预分配 `message.len() + 1` 字节，复制 UTF-8 内容并追加 `\n`。
- `StmtWindowForLog`：窗口适配 trait，要求提供 Unix 起始时间、可变遍历记录和可选的聚合淘汰记录。仓库搜索未发现实现。
- `StmtLogStorage<W: Write>` / `newStmtLogStorage(writer)`：拥有 writer 的通用存储器及其构造函数。
- `StmtLogStorage::persist(window, end)`：写入窗口记录，并在聚合淘汰记录 `ExecCount > 0` 时追加该记录。
- `sync()`、`log()`、`logEvicted()`、`intoInner()`：分别刷新、写单条普通 JSON 行、批量写淘汰 JSON 行、交还 writer。

## 执行流程

普通生产编码从 [`stmtsummary.rs`](stmtsummary.rs) 的 `fileStmtStorage::writeRecord` 开始：根据 `evicted` 参数调用 `marshalStmtRecord` 或 `marshalEvictedStmtRecord`，把 `serde_json::Error` 映射为 `io::Error`，再向文件写 JSON 字节和换行。窗口轮转由 `rotateWindow` 生成快照并调用 `stmtStorage::persist`；逐条淘汰记录则由 `onEvict` 非阻塞入队、`evictedLogLoop` 聚批后调用 `stmtStorage::logEvicted`。这些生产路径复用了本文件的 JSON 编码函数，但没有构造 `StmtLogStorage<W>`。

若调用通用 `StmtLogStorage::persist`，流程是：先缓存 `window.beginUnix()`；然后通过 `forEachRecordMut` 逐条把 `Begin`、`End` 改为本次窗口边界并调用 `log`；首个错误被保存在 `result` 中，闭包后续调用会立即返回，遍历结束后用 `result?` 终止。普通记录全部成功后，才检查 `evictedForPersistMut()`；仅当存在且 `ExecCount > 0` 时设置同样的时间边界并写入。这里的聚合淘汰记录通过 `log` 写出，不带 `evicted: true`，与 Go `stmtLogStorage.persist` 一致；逐条淘汰事件才由 `logEvicted` 加标记。

`logEvicted` 先在内存中构造整个批次：每条记录调用 `marshalEvictedStmtRecord`，序列化失败则跳过；成功记录之间加一个换行，批次尾再加一个换行。空批次不触碰 writer。非空批次只调用一次 `write_all`，成功后才更新原子计数并返回成功条数。

## 数据与状态

编码不会复制 `StmtRecord` 的业务字段，而是通过引用包装与 `serde(flatten)` 直接展开它。四种可见形态是：普通记录；普通记录加 `evicted`；普通记录加 `additional_fields`；普通记录同时加两者。`additional_fields` 始终是嵌套对象，不会与 `StmtRecord` 的顶层键混合；独立测试验证了 `keyspace_name` 的嵌套位置以及记录的 `begin`、`digest` 等字段仍保持顶层。

`StmtLogStorage` 独占其 writer，因此同一实例的写操作需要 `&mut self`。`persist` 会原地改写传入窗口内记录的 `Begin`/`End`；调用者必须接受这一状态变化。`intoInner` 消耗存储器并返回 writer，主要用于内存 writer 的结果断言。全局附加字段和累计数跨所有 `StmtLogStorage` 实例共享，也会跨测试共享；测试在设置附加字段后显式恢复空 map，并用“调用前计数 + 本次成功数”断言，避免假设初始计数为零。

## 依赖与调用关系

上游模块边界由 [`lib.rs`](lib.rs) 建立，所有公开函数和类型经 crate 根再导出。生产调用边为 `rotateWindow` / `evictedLogLoop` → [`stmtsummary.rs`](stmtsummary.rs) 的 `stmtStorage` 实现 → `fileStmtStorage::writeRecord` → `marshalStmtRecord` / `marshalEvictedStmtRecord` → `serde_json::to_vec`。测试调用边包括 [`record_test.rs`](record_test.rs) 的 `TestStmtRecord` 与 `ia_json_keys_match_persisted_log_contract`，以及 [`column_1_aster_unit_test.rs`](column_1_aster_unit_test.rs) 的 `logger_json_keeps_flat_record_and_markers`。

下游依赖很小：`crate::StmtRecord` 提供被编码数据；`serde` 的 `Serialize`/`flatten` 定义 JSON 形态；`serde_json` 执行编码；标准库 `Write` 提供输出边界；`RwLock` 与 `AtomicU64` 提供全局并发状态。`Cargo.toml` 没有为 logger 声明 feature gate，本文件也没有条件编译项。

RustCodeGraph 已索引该文件并识别 21 个符号；精确 `callers/callees` 命令未返回边且超时，因此调用边又以仓库精确符号搜索和上述源码位置核验。搜索结果同时确认：排除定义文件后，`StmtLogStorage`/`newStmtLogStorage` 只在 `column_1_aster_unit_test.rs` 被直接使用，`StmtWindowForLog` 没有实现。

## 错误处理与边界

`marshal*` 将 `serde_json::Error` 原样返回；`log` 与 `persist` 把它转换为 `io::Error::other`，并传播序列化、写入或刷新错误。`persist` 在第一条失败后不再写后续普通记录，也不会写聚合淘汰记录。记录可能已经被设置新的 `Begin`/`End`，因此失败不具备内存状态回滚或文件事务性；先前成功写入的行也不会撤销。

`logEvicted` 对序列化错误采取 Go 对照中的“跳过坏记录、继续批次”策略，但 Rust 版本当前不记录 warning；writer 写失败则整次返回错误，且不增加成功计数。一次 `write_all` 仍不等于磁盘持久化，调用者需要显式 `sync`/底层 writer 的持久化语义。`ExecCount == 0` 的聚合淘汰占位不会由 `persist` 写出。空淘汰批次返回 `Ok(0)`。

全局 `RwLock` 中毒会在 setter 或编码时 panic，而不是返回可恢复错误。包装结构使用 `flatten`；如果未来 `StmtRecord` 新增名为 `evicted` 或 `additional_fields` 的序列化字段，会形成 JSON 键冲突风险，扩展时必须先验证输出契约。

## 并发与资源生命周期

`STMT_LOG_ADDITIONAL_FIELDS` 允许多读单写；一次序列化在持有读锁期间完成，因此不会看到被部分替换的 map，但较大的记录编码会延长 setter 的等待时间。`PERSISTED_EVICTED_COUNT` 可并发累加，`Relaxed` 足以提供原子计数但不承诺与日志内容的内存可见性顺序。

`StmtLogStorage<W>` 本身没有内部锁，方法要求可变借用；是否能跨线程取决于 `W` 的 trait 能力以及外部同步。本文件不创建线程或通道。真实 `StmtSummary` 的并发生命周期在 [`stmtsummary.rs`](stmtsummary.rs)：`onEvict` 向有界通道非阻塞发送快照，`evictedLogLoop` 按容量/定时器批量刷写，`rotateLoop` 负责窗口轮转，关闭时停止并 join worker，再持久化剩余窗口并 sync。这里不应把 `StmtLogStorage` 的无锁模型等同于生产 `fileStmtStorage` 的 `Mutex<File>` 模型。

`StmtLogStorage` 拥有 writer，从构造持续到被 drop 或 `intoInner` 消耗；`sync` 仅调用 `Write::flush`，具体是否执行 fsync 由 writer 实现决定。

## 与 Go 版本的对应关系

直接对照文件是 [`logger.go`](logger.go)。两端都把 `StmtRecord` 扁平编码；无附加字段时保持原形，有附加字段时增加 `additional_fields`；逐条淘汰事件增加 `evicted: true`；窗口持久化会先填写 `Begin`/`End`，并跳过 `ExecCount == 0` 的聚合淘汰占位；日志条目最终以换行结尾。

差异同样重要：Go `newStmtLogStorage` 接收日志配置，调用 PingCAP logger 创建带滚动配置的 `zap.Logger`，并以专用 encoder 替换统一日志格式；Rust `newStmtLogStorage` 只包装调用者提供的 `Write`，不创建文件或实现滚动。Go `persist` 锁住窗口中的每条 `lockedStmtRecord` 及淘汰聚合后写入；Rust trait 要求调用者提供可变记录访问，本文件不加记录锁。Go `log`/`persist` 对 marshal 失败记录 warning 后继续其 void 接口，Rust `log`/`persist` 返回 `io::Result` 并在普通窗口首错停止。Go `logEvicted` 对坏记录告警后跳过、通过 zap 写一个多行 message，并更新 Prometheus 指标；Rust 同样跳过坏记录，但直接写字节，使用本地 `AtomicU64`，且将 writer 错误返回调用者。

附加字段来源也不同：Go 每次从全局配置 `GetKeyspaceObservabilityStmtLogFields()` 读取；Rust 由 `setStmtLogAdditionalFields` 显式更新进程全局 map。`encodeStmtLogEntry` 仅复现 Go `stmtLogEncoder.EncodeEntry` 的消息加换行结果，未移植 zap encoder 的整套空操作字段方法。上述差异说明当前 Rust 文件是语义移植和可测试 I/O 抽象，不是 Go 日志后端的一比一类型替换。

## 扩展指南

新增 JSON 元数据时，优先修改 `marshalStmtRecordWithEvicted` 选择的包装结构，不要改变 `StmtRecord` 扁平字段或把 `additional_fields` 展开到顶层；同步扩展 [`record_test.rs`](record_test.rs) 和独立测试 [`column_1_aster_unit_test.rs`](column_1_aster_unit_test.rs)，覆盖普通/淘汰 × 有/无附加字段四种组合。新增键前检查与 `StmtRecord` 序列化字段冲突，并与 [`logger.go`](logger.go) 及日志消费者兼容。

若要把 `StmtLogStorage<W>` 接入生产路径，必须先为实际窗口实现 `StmtWindowForLog`，明确记录锁顺序、淘汰聚合是否应带 `evicted` 标记，并协调或替换 [`stmtsummary.rs`](stmtsummary.rs) 已有的 `fileStmtStorage`，避免两套落盘路径重复写入。还应增加独立 logger 测试文件，而不是把 Rust 单元测试嵌入 `logger.rs`；至少覆盖首个普通记录写失败后的短路、空/非空聚合淘汰记录、部分状态已修改但 I/O 失败、`sync` 传播和并发附加字段更新。

性能相关改动应保留批量淘汰只做一次 `write_all` 的特性，并评估全局读锁覆盖完整 JSON 编码的竞争。若要求真正落盘，不能只依赖当前 `sync` 的 `Write::flush`，需要在具体文件后端定义 `sync_all` 语义；若要求与 Go 滚动日志完全等价，还需在本文件之外提供日志初始化、轮转和指标接线。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标目录 32 个文件；`files --filter pkg/util/stmtsummary/v2` 确认源、Go 对照及测试集合；`node --file pkg/util/stmtsummary/v2/logger.rs --offset 1 --limit 260` 返回完整 209 行和 21 个符号。精确 `callers/callees` 查询超时且无输出，未据此虚构调用边。
- 源码与模块：[`logger.rs`](logger.rs)、[`lib.rs`](lib.rs)、[`stmtsummary.rs`](stmtsummary.rs)、[`record.rs`](record.rs)。其中精确符号搜索验证生产编码调用和 `StmtLogStorage` 未接入生产路径的现状。
- crate 配置：[`Cargo.toml`](Cargo.toml)，核对 crate 名、`lib.rs` 入口、`serde`/`serde_json` 依赖及无 logger feature gate。
- Go 对照：[`logger.go`](logger.go) 与 [`stmtsummary.go`](stmtsummary.go)，核对初始化、窗口写入、逐条淘汰批处理、锁和指标语义。
- Rust 测试：[`record_test.rs`](record_test.rs) 的 `ia_json_keys_match_persisted_log_contract`、`TestStmtRecord`；[`column_1_aster_unit_test.rs`](column_1_aster_unit_test.rs) 的 `logger_json_keeps_flat_record_and_markers`。Go 相关测试入口包括 [`record_test.go`](record_test.go) 对普通/淘汰 marshal 的断言，以及 [`stmtsummary_test.go`](stmtsummary_test.go) 的 logger 初始化错误场景。
- 本任务是纯文档分析，按计划不运行 Cargo。交付结构以任务规定的 11 个固定二级标题命令验证，并人工复核“为何存在、如何运行、如何安全扩展”三项问题均有源码证据支撑。
