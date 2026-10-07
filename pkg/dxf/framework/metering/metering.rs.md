# `pkg/dxf/framework/metering/metering.rs`

## 文件定位

本文件是 `astersql-dxf-framework-metering` crate 的 Meter 状态机实现。crate 入口 `pkg/dxf/framework/metering/lib.rs` 通过 `include!("metering.rs")` 将它置于 `metering` 模块，并在 crate 根重新导出公共 API；相邻的 `data.rs` 负责累计快照到 `MeterItem` 的增量计算，`recorder.rs` 负责单任务原子计数。

在 DXF 中，它位于任务执行与计量后端之间：任务侧按 `TaskBase.ID` 获取共享 `Recorder`，后台 Meter 定期抓取累计值、计算相对上次快照的增量并交给 `MeteringWriter`。仓库当前 Rust 生产调用证据只有 `pkg/dxf/framework/taskexecutor/execute/interface.rs` 的 `FrameworkInfo::new` 调用 `metering::RegisterRecorder`；未检索到 Rust 生产代码安装全局 Meter、启动循环、注销 recorder 或实现真实 SDK writer，因此完整后台上报链目前仍是边界抽象与测试覆盖状态，不能视为已完成接线。

`pkg/dxf/framework/metering/Cargo.toml` 声明 crate 默认无 feature，`nextgen` feature 映射到 `kerneltype/nextgen`。直接依赖包括 `proto`、`dxfmetric`、`kerneltype`、`recording`、`anyhow`、`log` 和 `uuid`；它没有依赖已发布的 Rust metering SDK。

## 核心职责

- 提供进程级全局 Meter 槽：`METERING_INSTANCE`、`metering_instance`、`SetMetering`。
- 按内核模式提供公共门面：`RegisterRecorder`、`UnregisterRecorder`、`WriteMeterData` 在 Classic 模式或未安装 Meter 时安全退化为空操作。
- 管理 recorder 生命周期：同一任务复用 recorder；注销仅打标；确认最后一批数据已经进入 flush 快照后才移除。
- 执行 `scrape_current_data -> calculate_data_items -> write_meter_data -> after_flush` 的增量上报流程。
- 将失败的精确载荷按原时间戳保存和重试，达到 `MAX_RETRY_COUNT` 后丢弃，避免与后续增量合并造成重复计量。
- 提供可取消的同步 `Context`，驱动 flush/retry 两个线程并给每次写入附加 `WRITE_TIMEOUT`。
- 通过 `MeteringWriter` 和 `WriterFactory` 隔离后端实现，并在写失败时增加 `EventMeterWriteFailed` 指标。

## 主要符号

- `WRITE_TIMEOUT = 10s`、`CATEGORY = "dxf"`、`MAX_RETRY_COUNT = 10`、`RETRY_INTERVAL = 5s`：写入和重试策略常量。
- `FlushIntervalMillis: AtomicU64`：默认 60 秒的 flush 周期，以毫秒原子值保存，主要便于测试调整；循环启动后只读取一次。
- `Context` / `ContextState`：共享 `AtomicBool + Mutex + Condvar` 的取消状态，子 context 可增加 `Instant` 截止时间。`wait` 返回“取消或截止是否胜出”。
- `MeteringData`：writer 接收的完整载荷，包含实例 `self_id`、秒级 `timestamp`、`category` 和任务增量 `items`。
- `MeteringWriter`：`Send + Sync + 'static` 的后端边界，定义 `write` 与 `close`。
- `MeteringConfig` / `WriterFactory`：构造后端所需的存储类型、bucket 和覆盖策略；`Meter::new` 只在两个必填字段均非空时调用工厂。
- `WrappedRecorder`：将共享 `Arc<Recorder>` 与延迟注销标志组合。
- `WriteFailData`：保存失败写入的原时间戳、重试次数和不可重新计算的精确 items。
- `MeterState`：一个互斥锁内聚合 `recorders`、`last_flushed_data`、`pending_retry_data` 三张表，使状态转换保持一致。
- `Meter`：持有状态、无短横线 UUID 与 writer。公开构造入口是 `new` 和 `with_writer`，运行入口是 `StartFlushLoop`，单次写入口是 `write_meter_data`，显式关闭入口是 `Close`。
- `RegisterRecorder` / `UnregisterRecorder` / `WriteMeterData` / `SetMetering`：进程级公共 API；名称保留 Go 风格并由 `lib.rs` 允许非 snake case。
- `contains_recorder`、`is_unregistered`、`pending_retry_len`、`last_flushed_data`：仅在 `cfg(test)` 下存在的状态观察辅助函数。

## 执行流程

1. 初始化时，调用方以 `Meter::new(config, factory)` 创建实例。若 `storage_type` 或 `bucket` 为空，返回 `Ok(None)`；否则复制配置、强制 `overwrite_existing = true`，创建 writer，再由 `with_writer` 生成实例 UUID。当前 Rust 生产树未找到执行这一步的接线。
2. `SetMetering(Some(meter))` 把实例写入进程级 `RwLock<Option<Arc<Meter>>>`。任务创建 `FrameworkInfo` 时调用 `RegisterRecorder`；NextGen 且 Meter 已安装时，以 task ID 注册或复用 recorder，否则返回一个不进入全局表的默认空 recorder。
3. `StartFlushLoop` 为 `retry_loop` 新建线程，当前线程执行 `flush_loop`，结束后 join 重试线程。flush 时间点按 Unix 毫秒对齐到下一个周期边界。
4. `flush` 先克隆 recorder 引用并在状态锁外调用 `curr_data`，然后由 `Data::cal_meter_data_item` 相对 `last_flushed_data` 计算正增量。无增量时不调用 writer，但仍推进快照和清理注销项。
5. 有增量时，`write_meter_data` 派生 10 秒截止时间，构造 `MeteringData` 并调用 writer。失败会增加计量失败指标并由 `add_failed_data` 按时间戳保存完整载荷；无论成功与否，常规快照都推进到本次 scrape 值，因此下一次常规 flush 只包含新增量。
6. `retry_loop` 每 5 秒调用 `retry_write`。它先克隆待处理载荷以避免持锁执行 I/O；成功项删除，失败项增加计数，达到 10 次即丢弃。取消期间出现错误则停止本轮遍历。
7. `UnregisterRecorder` 只设置 `unregistered`。`after_flush` 仅当该 recorder 的最新累计值等于刚记录的快照时移除 recorder 和快照，保证并发发生的新计数仍有一次后续 flush 机会；期间重新注册同一任务会清除注销标志。
8. context 取消后，`flush_loop` 仍按预定的下一时间戳做一次尽力最终 flush，然后关闭 writer。`StartFlushLoop` 等待 retry 线程退出；`Close` 则提供独立的显式关闭调用。

## 数据与状态

`MeterState.recorders` 以 task ID 唯一索引 recorder，因此同一任务跨 step 获取的是同一累计器。`last_flushed_data` 存储的是最近一次 scrape 快照，而不是“最近一次成功落盘”快照：这是失败载荷与后续增量不重复的核心不变量。写失败后，原批次只存在于 `pending_retry_data[timestamp]`；后续常规 flush 从已推进快照继续计算。

`pending_retry_data` 同样以时间戳为键；同一时间戳再次加入会覆盖旧值。对象名称由时间戳、category 和 UUID 等字段确定，故 `Meter::new` 强制允许覆盖同名对象。UUID 使用 `Uuid::new_v4()` 并将 `-` 替换为 `_`，与 Go/SKD 对对象名的约束保持一致。

计量计数本体位于 `Recorder` 的原子字段中，`Meter` 只保存快照和引用。`MeteringData.items` 使用 `data.rs` 的 `MeterItem = HashMap<String, MeterValue>`，而不是直接依赖 Go 的 `map[string]any` 或某个未发布的 Rust SDK 类型。

## 依赖与调用关系

上游 Rust 生产调用目前可确认：

- `pkg/dxf/framework/taskexecutor/execute/interface.rs::FrameworkInfo::new -> RegisterRecorder`，把 recorder 注入 step 执行框架。
- `pkg/dxf/framework/metering/lib.rs` 声明并重新导出本文件 API。

未找到 Rust 生产环境对 `Meter::new`、`SetMetering`、`StartFlushLoop`、`UnregisterRecorder`、全局 `WriteMeterData` 的调用，也未找到 `MeteringWriter`/`WriterFactory` 的非测试实现。这意味着 Rust 当前只有 recorder 获取入口接入任务框架；后端创建、循环生命周期和任务结束注销尚未形成与 Go 相同的完整主链。

本文件内部的关键下游关系是：`RegisterRecorder -> Recorder::new/get_or_register_recorder`；`flush -> scrape_current_data -> Recorder::curr_data`；`flush -> calculate_data_items -> Data::cal_meter_data_item`；`write_meter_data -> MeteringWriter::write`；写失败路径调用 `dxfmetric::InitDistTaskMetrics().ExecuteEventCounter...inc()`；`flush_loop` 和 `Close` 调用 `MeteringWriter::close`。

Go 生产主链提供对照证据：`pkg/domain/domain.go` 创建并安装 Meter、启动 `StartFlushLoop`；`pkg/dxf/framework/taskexecutor/task_executor.go` 注销 recorder；`pkg/dxf/framework/handle/handle.go` 直接写任务汇总计量。这些 Go 调用不能当作 Rust 已接线事实。

## 错误处理与边界

- `Meter::new` 将 factory 创建失败原样通过 `anyhow::Result` 返回；配置不完整不是错误，而是显式禁用计量。
- 公共全局 API 对 Classic 模式和空全局 Meter 采用 no-op 语义，避免计量能力影响主任务执行。
- 所有 `Mutex`/`RwLock` 获取都在 poison 后通过 `into_inner` 继续使用状态；这保持服务可继续运行，但意味着 panic 后状态一致性依赖临界区操作本身。
- writer 写错误向直接调用者返回；周期 `flush` 捕获并转入重试队列，`retry_write` 记录首个错误用于汇总日志。超过 10 次的载荷会被明确丢弃，属于允许的数据损失边界。
- `write_meter_data` 的超时只通过 `Context.deadline` 暴露给 writer；trait 实现必须主动检查 context，框架不会强制中断阻塞的 `write`。
- 取消后的最终 flush 复用已取消的父 context，因此 writer 若立即尊重取消，最终写入可能失败；代码只保证尽力尝试。
- `SystemTime` 早于 Unix epoch 时 `unix_millis` 退化为 0，毫秒数无法转换为 `u64` 时饱和为 `u64::MAX`。
- `flush` 不向调用方返回写错误；可靠性由失败队列、日志和失败指标提供。`Close` 则保留并返回 close 错误。

## 并发与资源生命周期

全局实例由 `OnceLock<RwLock<Option<Arc<Meter>>>>` 管理，读取时先克隆 `Arc` 再释放读锁，替换实例不会使正在使用的 Meter 失效。`MeterState` 使用单一 `Mutex` 保护三张相关表；抓取 recorder、writer I/O 和读取 recorder 原子计数均尽量在锁外完成，避免长时间阻塞注册/注销。

`Context` 的 clone 共享取消位和条件变量，但各 clone 的 deadline 是值拷贝；`cancel` 使用 Release 写并唤醒全部等待者，`is_cancelled` 使用 Acquire 读。`wait` 同时处理虚假唤醒、指定等待时长和 context deadline。

`StartFlushLoop` 总共使用调用线程和一个 retry 子线程。正常退出依赖共享 context 被取消；flush 线程随后执行最终 flush、关闭 writer，再等待 retry 线程 join。若 retry 线程 panic，只记录警告。直接调用 `Close` 与后台循环没有幂等协调，调用方必须避免与 `flush_loop` 重复关闭非幂等 writer。

注销是“两阶段”的：先标记，后在 flush 后比较快照与实时累计值；只有相等才释放 recorder。测试覆盖了 scrape 与写入之间新增数据、写入期间注销、注销后重新注册等竞态。

## 与 Go 版本的对应关系

主要行为直接对应 `pkg/dxf/framework/metering/metering.go`：相同的 10 秒写超时、`dxf` category、10 次重试上限、5 秒重试间隔和 1 分钟默认 flush；相同的 task ID 复用、延迟注销、失败仍推进快照、原时间戳重试、周期对齐、取消后最终 flush 与关闭行为。

Rust 为 Go 的 `context.Context` 提供同步 `Context`，为 `atomic.Pointer[Meter]` 提供 `OnceLock<RwLock<Option<Arc<Meter>>>>`，为 `map[string]any` 提供受限的 `MeterItem/MeterValue`。Go 的两个 goroutine 在 Rust 中实现为调用线程执行 flush、另建一个 retry 线程。Go 的 `retryData` 独立锁在 Rust 中合并进 `MeterState` 单锁。

存在明确差异与未迁移项：Go `NewMeter` 直接构造对象存储 provider 和 PingCAP metering SDK writer，Rust 仅声明 `WriterFactory`/`MeteringWriter`，没有生产适配器；Go 通过 `pkg/domain/domain.go` 完成安装和启动，Rust 没有对应生产调用；Go 的 failpoint `forceTSAtMinuteBoundary`、`meteringFinalFlush` 未出现在 Rust 实现中；Go 有 logger 字段与更丰富结构化日志，Rust 使用全局 `log` 宏；Rust `Close` 不需要处理空 writer，因为其类型不是 `Option`。

独立测试对应关系较完整：`metering_test.rs` 与 `metering_test.go` 都覆盖空/合法配置、Classic no-op、注册注销竞态、增量 flush、失败重试、循环关闭和本地读回；`migration_aster_unit_test.rs` 额外集中验证 Go/Rust 迁移语义与 context 可中断等待。

## 扩展指南

- 接入真实后端时，应在本 crate 或明确的适配 crate 实现 `MeteringWriter` 和 `WriterFactory`，不要把 SDK 细节塞进 `Meter` 状态机；同时补齐 Rust 进程初始化的 `Meter::new -> SetMetering -> StartFlushLoop` 和任务结束的 `UnregisterRecorder` 接线。
- 新增计量字段应优先修改 `data.rs::Data/DataValues::cal_meter_data_item` 和 `recorder.rs::Recorder`，保持累计快照、只发送增量、失败载荷不可与新增量合并这三个不变量，并同步独立的 `data_test.rs`、`recorder_test.rs` 和 `metering_test.rs`。
- 修改 flush/retry 策略时，重点检查时间戳唯一性、覆盖语义、最大重试次数和取消后的最终 flush；更改 `FlushIntervalMillis` 的读取时机还会影响运行中动态配置语义。
- 修改 recorder 注销逻辑时，必须保留“快照存在且等于当前值才删除”和“重新注册清除注销标志”，否则可能漏报并发产生的最后增量。
- writer 实现必须尊重 `Context::deadline/is_cancelled`，并明确 `close` 是否幂等、是否允许与写入并发；否则 10 秒超时与安全退出只停留在接口约定。
- 相关测试必须继续放在独立文件：核心状态机测试在 `pkg/dxf/framework/metering/metering_test.rs`，迁移对照在 `migration_aster_unit_test.rs`，不要把测试内嵌回生产文件。兼容风险主要是 Go 载荷字段/时间戳与 Classic no-op 语义；性能风险主要是全局状态锁竞争、周期内 recorder 数量以及重试积压。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出 `metering.rs`、`data.rs`、`recorder.rs`、Rust/Go 测试与 Go 对照文件。
- RustCodeGraph 源码节点：`pkg/dxf/framework/metering/metering.rs`（634 行、47 个符号）；核对了 `Context`、全局 API、`MeterState`、`Meter`、flush/retry/close 全流程。
- RustCodeGraph/文本调用证据：`pkg/dxf/framework/taskexecutor/execute/interface.rs::FrameworkInfo::new -> metering::RegisterRecorder`；目标 Rust 生产树未发现其他完整生命周期调用。
- crate 与模块证据：`pkg/dxf/framework/metering/Cargo.toml`、`pkg/dxf/framework/metering/lib.rs`；DXF 包定位参考 `pkg/dxf/framework/doc.go`。
- 相邻实现证据：`pkg/dxf/framework/metering/data.rs` 的增量与载荷模型，`pkg/dxf/framework/metering/recorder.rs` 的原子累计与快照。
- Go 对照与生产入口：`pkg/dxf/framework/metering/metering.go`、`pkg/domain/domain.go`、`pkg/dxf/framework/taskexecutor/execute/interface.go`、`pkg/dxf/framework/taskexecutor/task_executor.go`、`pkg/dxf/framework/handle/handle.go`。
- 测试证据：`pkg/dxf/framework/metering/metering_test.rs`、`migration_aster_unit_test.rs`，并以 `metering_test.go` 的九组测试核对测试意图。测试明确覆盖配置禁用/覆盖、Classic no-op、注销竞态、增量与空 flush、失败快照、部分重试成功与上限丢弃、取消关闭、本地文件读回。
- 本任务为纯文档分析，按计划不运行 Cargo；最终仅执行固定十一章节的结构检查并人工复核上述事实边界。
