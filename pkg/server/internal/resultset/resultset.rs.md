# `pkg/server/internal/resultset/resultset.rs`

## 文件定位

本文说明的真实源文件是 [`resultset.rs`](./resultset.rs)。它属于 Cargo crate `astersql-server-internal-resultset`（`pkg/server/internal/resultset/Cargo.toml`，库入口为同目录 `lib.rs`）。`lib.rs` 将本文件的结果集 API 与 `cursor.rs` 的逐行游标 API 一并重新导出，供服务端协议层把执行器的 `astersql_util_sqlexec::RecordSet` 适配成可按 chunk 拉取、可关闭、可分离的服务端结果集。

当前 Rust 服务端中的直接消费位置包括 `pkg/server/protocol_result.rs`：`WorkerResult` 保存 `Box<dyn ResultSet>`，普通结果通过 `NewChunk`/`Next` 取批，游标结果通过 `cursor.rs::WrapWithLazyCursor` 逐行读取，协议生命周期操作再转发到 `Finish`、`OnFetchReturned` 和 `Close`。因此本文件位于“session 已产生记录集”与“MySQL 协议编码/游标发送”之间，而不负责 SQL 规划、执行或网络编码。

## 核心职责

- `ResultSet` 定义服务端需要的统一能力：列元数据、chunk 分配与拉取、字段类型、关闭状态、执行结束、可分离结果集、fetch 通知和游标 RU v2 增量同步。
- `TidbResultSet` 包装一个 `sqlexec::RecordSet`，将列描述转换为协议层 `column::Info`，并在实例与可选的 `PlanCacheStmt` 中缓存转换结果。
- `finish_lock` 串行化 `NewChunk`、`Next`、`Finish`、`Close` 对底层记录集生命周期的访问；其中 `Finish` 采用非阻塞尝试，避免外部终止 SQL 的路径等待正在进行的拉取。
- `TryDetach` 把底层可分离记录集重新包装为独立的 `ResultSet`，供游标等需要脱离原 session 上下文的场景使用。
- `CursorRUV2Tracker` 将 `RUDetails` 中尚未同步的 TiKV coprocessor response bytes 转入 `RUV2Metrics`，并用互斥锁串行化多次同步。

## 主要符号

- `PreparedStmtRef = Arc<PlanCacheStmt<Arc<Info>>>`：可共享的预编译语句引用；其 `CachedColumnInfos`/`CacheColumnInfos` 承载列信息缓存。
- `trait ResultSet`：公开对象安全接口。`Next` 和 `Finish` 返回 `sqlexec::GoError`；`Close` 不向调用者返回错误。Rust 接口还直接包含 Go 版本通过可选接口表达的 fetch/RU tracker 钩子。
- `CursorRUV2Tracker { metrics, ru_details, state }`：同时持有两个 `Arc`，`state: Mutex<()>` 只保护同步操作本身。
- `NewCursorRUV2Tracker(metrics, ru_details)`：任一输入缺失或 `metrics.Bypass()` 为真时返回 `None`；成功时先同步已有增量，再返回共享 tracker。
- `AttachCursorRUV2Tracker`、`ReportCursorRUV2Delta`：面向 `dyn ResultSet` 的薄入口，分别安装 tracker 和触发同步。
- `New(record_set, prepared_stmt)`：返回装箱的 `TidbResultSet` trait object。
- `TidbResultSet`：拥有 `record_set: Option<Box<dyn RecordSet>>`、预编译引用、可选 RU tracker、可选列缓存、共享生命周期锁以及原子关闭标志。
- `recordSet`/`recordSetMut`：取得底层记录集的内部辅助函数；在 `Close` 取走底层对象后继续调用会 panic。
- `FinishLockForTest`：仅在 `cfg(test)` 下暴露锁，用于独立测试构造锁竞争。

## 执行流程

1. 上游用 `New`/`TidbResultSet::new` 注入已创建的 `sqlexec::RecordSet`，实例初始未关闭、无本地列缓存、无 RU tracker。
2. 首次 `Columns` 先查实例缓存；未命中时查 `prepared_stmt.CachedColumnInfos()`；仍未命中才读取 `RecordSet::Fields`，逐项调用 `ConvertColumnInfo`，随后同时写入预编译缓存（若存在）和实例缓存。后续调用克隆 `Vec` 与其中的 `Arc`，不重复转换列对象。
3. 协议层调用 `NewChunk`，本文件在持有 `finish_lock` 时把分配器转交给底层 `RecordSet::NewChunk`；`Next` 在同一把锁下把 context 和目标 chunk 转交给底层 `RecordSet::Next`。空 chunk 由底层约定表示 EOF。
4. 正常 chunk 路径可见于 `pkg/server/protocol_result.rs::WorkerResults::operate` 的 `Operation::Chunk` 分支；lazy cursor 则由 `pkg/server/internal/resultset/cursor.rs::LazyRowIterator::Next` 在当前 chunk 耗尽后再次调用本接口的 `Next`。
5. 外部结束执行时调用 `Finish`。锁空闲则转发到底层 `RecordSet::Finish`；锁正由 `Next` 等持有时直接返回 `Ok(())`，不会排队等待或补做一次 Finish。
6. `TryDetach` 先调用底层 `RecordSet::TryDetach`。返回错误时原样传播；`detached == false` 时规范化为 `(None, false)`；成功时要求记录集存在，并用相同预编译引用和当前列缓存构造新 `TidbResultSet`。
7. `Close` 取得生命周期锁后以 CAS 将 `closed` 从 false 改为 true；仅首个成功者取走 `record_set` 并通过 `astersql_parser_terror::Call` 调用底层 `Close`。重复关闭直接返回。
8. 游标 RU 路径先用 `NewCursorRUV2Tracker` 同步游标建立前已有的 response bytes，再用 `AttachCursorRUV2Tracker` 挂到结果集；每次客户端 fetch 边界可调用 `ReportCursorRUV2Delta`，最终进入 tracker 的 `reportDelta` 串行排空新增值。

## 数据与状态

`record_set` 使用 `Option` 表达所有权是否仍在本实例中：构造时为 `Some`，首次 `Close` 时 `take()` 后永久为 `None`。`closed: AtomicBool` 是公开关闭状态的来源，CAS 使用 `AcqRel`，读取使用 `Acquire`；它保证 Close 幂等，但并不让整个 `TidbResultSet` 自动成为可并发无条件调用的共享对象。

`columns` 是实例级缓存，`prepared_stmt` 提供跨结果集可复用的缓存。两者存放 `Arc<Info>`，所以返回 `Vec` 的克隆只增加共享引用，不复制列信息内容。`SetPreparedStmt` 只替换预编译引用，不清空已存在的 `columns`；调用者若在列缓存形成后改绑语句，必须自行保证 schema 相容。

分离出的结果集继承 `prepared_stmt` 和 `columns`，但拥有新的 `finish_lock`、新的 `closed` 状态；当前实现不会复制 `cursor_ruv2`。这意味着 RU tracker 若仍需用于分离结果，必须在分离后的对象上重新挂载。

`CursorRUV2Tracker` 对 metrics 与 RU details 采用共享所有权，并以自身 mutex 保证同一 tracker 的同步操作串行执行。实际“只转移新增量”的账本语义由下游 `SyncRUV2MetricsFromRUDetails`（`pkg/util/execdetails/internal/util/lib.rs`）实现，本文件不自行计算差值。

## 依赖与调用关系

直接依赖如下：

- `astersql-util-sqlexec`：提供被包装的 `RecordSet`、`RecordChunk`、执行 context 与 `GoError`，是本文件的核心下游边界。
- `astersql-server-internal-column`：`Info` 是对协议层暴露的列描述，`ConvertColumnInfo` 将 `ResultField` 转为该类型。
- `astersql-planner-core`：`PlanCacheStmt` 保存预编译 point-get 等路径可复用的列缓存。
- `astersql-util-chunk`：提供 allocator 与 `FieldType`；`FieldTypes` 从每个 `ResultField.column.FieldType` 克隆出独立 boxed 值。
- `astersql-util-execdetails`：提供 `RUV2Metrics`、TiKV `RUDetails` 和同步函数。
- `astersql-parser-terror`：`Close` 通过 `terror::Call` 执行并吞掉底层关闭错误，与 Go 的关闭辅助函数语义对齐。

上游/旁路证据包括 `pkg/server/protocol_result.rs::WorkerResults::operate`（chunk、Finish、fetch 通知、Close）、`pkg/server/internal/resultset/cursor.rs::WrapWithLazyCursor` 与 `LazyRowIterator`（逐行游标），以及 `cursor.rs` 的 `impl_result_set_forwarder!`（两种游标包装器完整转发本 trait）。Cargo 清单还声明 `astersql-resourcegroup`、`astersql-types` 等 crate 级依赖；它们不在本文件中直接引用，不应据此推断本文件承担资源组或类型系统逻辑。

RustCodeGraph 对精确符号确认的下游边包括：`NewCursorRUV2Tracker -> CursorRUV2Tracker`（构造）与 `SyncRUV2MetricsFromRUDetails`（调用），`Columns -> ConvertColumnInfo`/`recordSet`，`TryDetach -> RecordSet::TryDetach`/`recordSetMut`。通用方法名的上游边在索引中不完整，因此上游位置由同索引文件读取和 `rg` 引用搜索补足。

## 错误处理与边界

- `Next`、`Finish`、`TryDetach` 的底层错误保持为 `sqlexec::GoError` 向上传播；`TryDetach` 不吞掉 detach 错误。
- `Close` 的底层错误被 `terror::Call` 消化，调用方只能通过 `IsClosed` 看到本地已关闭状态，无法获知清理失败；即使底层关闭报错，CAS 和 `record_set.take()` 仍保证不会重试。
- `Finish` 在锁忙时把“未执行 Finish”也表示为 `Ok(())`。扩展调用方不能把成功返回等同于底层一定执行过 Finish。
- `NewCursorRUV2Tracker` 对缺失输入和 bypass 都安静返回 `None`；已创建 tracker 后临时切换 bypass 的具体同步效果来自下游 metrics 实现，`pkg/server/conn_test.rs` 验证 bypass 期间不增加、恢复后再同步累计值。
- `recordSet`、`recordSetMut`、`NewChunk`、`Next` 在 Close 后使用会以 `result set used after Close` panic；`FieldTypes` 还要求每个 `ResultField.column` 必须存在，否则以明确消息 panic。
- 成功 detach 声明 `detached == true` 却不给出记录集时会 panic，因为这是底层 trait 契约违约而非可恢复业务错误。
- `CursorRUV2Tracker::reportDelta` 对 poisoned mutex 使用 `expect`，会 panic；`NewChunk`、`Next`、`Close`、`Finish` 对 poisoned `finish_lock` 则恢复 guard 后继续。

## 并发与资源生命周期

生命周期顺序是：构造并拥有底层记录集 → 可选缓存列/挂载 tracker → 多次分配与拉取 → 可选 Finish/Detach/fetch 通知 → Close 取走并释放底层对象。`Drop` 没有在本文件中实现；安全释放依赖协议层/游标层明确调用 `Close`（例如 `LazyRowIterator::Close` 和 `protocol_result.rs` 的关闭操作）。

`finish_lock` 的设计目标是协调常规拉取线程与外部终止 SQL 的额外执行路径。`NewChunk`、`Next` 和 `Close` 阻塞取得锁；`Finish` 只尝试取得锁。这保证同一实例上这些底层操作不会在锁保护区内并发，但也刻意允许繁忙时遗漏 Finish。`Arc<Mutex<()>>` 主要支持测试持锁和共享锁句柄；公开 API 普遍接收 `&mut self`，游标包装又使用单线程 `Rc<RefCell<_>>`，因此本文件没有承诺结果集可跨线程共享调用。

关闭幂等由锁和原子 CAS 双重约束：拿到锁后只有第一次 CAS 成功的调用者能取走记录集。分离结果拥有独立锁与关闭标志，原对象和分离对象之后必须分别关闭；独立测试验证两者各关闭一次。

RU tracker 可以被 `Arc` 共享，内部 mutex 避免多个 fetch 边界同时同步相同 RU details。创建时的首次同步很重要：它建立当前累计值，后续调用才按下游状态转移新增 response bytes。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/server/internal/resultset/resultset.go`。Rust 的 `TidbResultSet`、`ResultSet`、`CursorRUV2Tracker`、构造/挂载/报告函数以及 `Columns`、`NewChunk`、`Next`、`Finish`、`Close`、`IsClosed`、`FieldTypes`、`SetPreparedStmt`、`TryDetach` 均对应 Go 同名或同职责符号，核心顺序与锁策略保持一致。

主要表示差异如下：

- Go 用 nil 指针/interface 表示缺失，Rust 用 `Option`；Go `TryDetach` 先类型断言 `DetachableRecordSet`，Rust 的 `sqlexec::RecordSet` trait 本身定义 `TryDetach`，再用返回的 bool 表示能力/结果。
- Go `ResultSet` 接口不含 `OnFetchReturned` 和 RU tracker 方法，而是通过 `FetchNotifier`、私有 `cursorRUV2Trackable` 做可选类型断言；Rust 将三项钩子直接并入 `ResultSet`，所以自定义实现必须提供方法，即使只是 no-op（见 `pkg/server/protocol_result.rs::CanonicalResultSet`）。
- Go 的 `Finish` 还对底层记录集做可选接口断言；Rust 直接调用 `RecordSet::Finish`。二者在锁忙时都返回成功且不等待。
- Go 的列缓存存放在 `PlanCacheStmt.PointGet.ColumnInfos` 的动态值中；Rust 通过强类型的 `CachedColumnInfos`/`CacheColumnInfos` API 存取 `Arc<Info>`。
- Go 用 `int32` 原子值表示关闭状态，Rust 用 `AtomicBool`；二者都只允许一次底层 Close，并在之后移除记录集引用。

Go 回归证据 `pkg/server/conn_stmt_test.go::TestCursorWithParams` 验证 RU response bytes 的初始同步、增量同步、bypass 以及首次 Next 出错时不提前访问列。Rust 的 `pkg/server/conn_test.rs::go_merge_139_cursor_consumer_only_synchronizes_response_bytes` 对应验证缺失输入、重复报告、bypass 暂停/恢复；同目录 `resultset_aster_unit_test.rs` 则覆盖适配器内部生命周期。

## 扩展指南

- 新增结果集生命周期能力时，先修改 `ResultSet`，然后同步 `TidbResultSet`、`cursor.rs::impl_result_set_forwarder!`、`protocol_result.rs::CanonicalResultSet`、`pkg/server/tests/commontest/cursor_test.rs::BatchResultSet` 等全部实现；Rust trait 无默认实现的方法会让遗漏在编译期暴露。
- 修改列缓存时应集中在 `Columns`、`SetPreparedStmt` 和 `TryDetach` 检查：明确换绑 prepared statement 是否要失效本地缓存、分离结果是否共享同一批 `Arc<Info>`，并保持 Go `PointGet.ColumnInfos` 的可观察语义。
- 修改 Finish/Close 同步策略时应保留“外部 Finish 不阻塞正在拉取”的意图，并为锁竞争、poison、重复 Close、Close 错误补充独立测试；不要把测试逻辑内嵌回生产文件。
- 修改 detach 时必须同时定义 tracker、列缓存、prepared statement、关闭所有权如何迁移。当前 tracker 不继承是明确事实；若改变，需验证不会重复计量 RU。
- 修改 RU 同步时应联动 `pkg/util/execdetails/internal/util/lib.rs::SyncRUV2MetricsFromRUDetails`，并覆盖创建前累计、连续增量、零增量、bypass 切换和并发报告；同时对照 Go `resultset.go` 与 `conn_stmt_test.go`。
- 修改 chunk/字段路径时应覆盖空结果、首个 Next 失败、缺失 column、allocator 分配以及 lazy cursor 跨 chunk 行序；性能上避免在热路径重复转换列信息或深拷贝整批 metadata。
- 本仓库约定测试与生产 Rust 源文件分离；本模块现有独立测试文件是 `pkg/server/internal/resultset/resultset_aster_unit_test.rs`，协议级补充测试位于 `pkg/server/conn_test.rs` 和 `pkg/server/tests/commontest/cursor_test.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；`files --filter pkg/server/internal/resultset` 确认目标 Rust/Go 文件、`cursor.rs`、`lib.rs` 和独立测试均在图中。
- RustCodeGraph 源码读取：`resultset.rs` 全部 310 行、`resultset_aster_unit_test.rs` 全部 316 行、`lib.rs`、`cursor.rs`、`pkg/server/protocol_result.rs` 和 `pkg/server/conn_test.rs` 的相关片段。
- RustCodeGraph 符号/调用查询：`query TidbResultSet`、`query NewCursorRUV2Tracker`、`query AttachCursorRUV2Tracker`、`query ReportCursorRUV2Delta`；文件限定的 `callers`/`callees` 查询确认上述下游边，并显示部分通用上游方法边缺失。
- Cargo 边界：`pkg/server/internal/resultset/Cargo.toml` 的 package、lib path、porting metadata 和 dependencies。
- Go 对照：`pkg/server/internal/resultset/resultset.go` 全部 231 行；Go 测试 `pkg/server/conn_stmt_test.go::TestCursorWithParams` 相关子测试。
- Rust 测试：`pkg/server/internal/resultset/resultset_aster_unit_test.rs` 的七项测试，以及 `pkg/server/conn_test.rs::go_merge_139_cursor_consumer_only_synchronizes_response_bytes`；另读取 `pkg/util/execdetails/internal/util/lib.rs::SyncRUV2MetricsFromRUDetails` 核对 pending 值排空语义。本任务按计划只做文档分析，未运行 Cargo 或代码测试。
- 补充引用搜索：使用 `rg` 查找 Rust/Go 中 tracker、detach、finish、fetch 通知和 crate 引用，以补足代码图对通用名称 caller 边的覆盖限制。
- 结构检查按任务指定命令执行，要求文件存在且固定二级标题恰好 11 个；交付前另人工核对本说明只陈述可由以上符号、文件和测试复核的当前行为。
