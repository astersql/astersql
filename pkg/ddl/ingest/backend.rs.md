# `pkg/ddl/ingest/backend.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate；crate 入口在 [`lib.rs`](lib.rs) 中以 `pub mod backend` 暴露它，包边界由 [`Cargo.toml`](Cargo.toml) 定义。它位于 DDL add-index/reorg 的本地 ingest 路径中：上层按一个 DDL job 构造 `BackendContext`，为该 job 的每个索引维护一个本地 `EngineInfo`，并在写入过程中根据磁盘压力或周期执行 flush、推进 checkpoint，最后关闭和注销引擎。

从 DDL 全局流程看，它不是 SQL 入口、DDL job 状态机或 schema-version 推进器，而是 reorg/backfill 阶段的局部资源与进度上下文。直接装配入口是 [`BackendContextBuilder::build`](backend_mgr.rs)，便捷注册/注销入口是 [`register_engines` 与 `finish_and_unregister_engines`](engine_mgr.rs)，流水线侧通过 [`IndexWriter::ingest_if_quota_exceeded`](../backfilling_operators.rs) 表达相同的配额检查契约。

当前 Rust 实现是 Go `pkg/ddl/ingest` 的局部移植，不应被理解为完整的 Lightning/TiKV ingest 后端：本文件的 `ingest` 只 flush 本地抽象、累计次数并推进 checkpoint，没有直接执行 Go 版本的 TiKV import/reset 流程。

## 核心职责

- 以 `BackendContext` 聚合单个 `job_id` 下的 `EngineInfo`、`MemRoot`、`DiskRoot` 与可选 `CheckpointManager`。
- 通过 `register` 保证“一组索引只注册一次”：整组重复注册返回原 `Arc<EngineInfo>`；部分重叠注册报错；关闭后的上下文拒绝注册。
- 通过 `check_flush` 把运行状态归约为 `FlushDecision::{None, Flush, Import}`。决策只考虑 `force_sync`、`DiskRoot::should_import()` 和十分钟 checkpoint 更新周期，不因内存占用本身触发 flush。
- 通过 `ingest_if_quota_exceeded`、`ingest` 和 `advance_watermark` 连接引擎 flush 与 checkpoint 的本地/全局水位线推进。
- 在结束阶段通过 `finish_and_unregister` 可选检查唯一索引重复值、关闭引擎并清空注册表；通过 `close` 阻止后续注册。
- 将 checkpoint 的恢复起点、键计数、chunk 进度和 import timestamp 以窄接口转发给 `CheckpointManager`；未配置 checkpoint 时采用空键、零值或 no-op。

## 主要符号

- `CHECKPOINT_UPDATE_INTERVAL: Duration`：固定为 `10 * 60` 秒，与 Go `checkpointUpdateInterval` 的十分钟保持一致。
- `FlushDecision`：三态决策枚举。`None` 继续写入；`Flush` 只 flush 并以 `imported = false` 推进本地水位线；`Import` 执行 `ingest` 并以 `imported = true` 推进全局水位线。
- `BackendContext`：核心状态对象。
  - `job_id` 标识 DDL job。
  - `engines: BTreeMap<i64, Arc<EngineInfo>>` 按 index ID 确定性排序并共享引擎。
  - `mem_root` 交给新引擎及 writer 做内存配额登记；本文件自身不读取内存压力来决定 flush。
  - `disk_root` 提供 import 决策。
  - `checkpoint` 可选地保存 reorg 进度。
  - `import_count` 是当前 Rust 局部实现的导入调用计数。
  - `closed`、`force_sync`、`last_flush`、`update_interval` 控制生命周期和决策。
- `BackendContext::new`：建立空引擎表、零 import 次数、开放状态和当前 `last_flush`。
- `register`：验证生命周期及参数长度，处理整组幂等/部分重叠，再逐 index ID 构造 `EngineInfo`。
- `collect_remote_duplicate_rows`：名称沿用 Go API，但 Rust 当前只扫描指定本地 `EngineInfo::rows()` 的 value，统计出现次数大于 1 的值；非唯一索引返回空列表。
- `flush_engines`：依次调用所有 `Engine::flush`，首个错误立即返回。
- `check_flush` / `ingest_if_quota_exceeded` / `ingest`：分别负责纯决策、按决策执行、无条件执行一次 Rust 局部“导入”。
- `finish_and_unregister` / `close`：前者完成 flush、可选查重、关闭并清空引擎；后者关闭但不清理引擎数据、设置 `closed`，且保留 map 中的 `Arc`。
- `next_start_key`、`total_key_count`、`add_chunk`、`update_chunk`、`finish_chunk`、`import_ts`、`advance_watermark`：可选 checkpoint 的转发层。

## 执行流程

1. [`BackendContextBuilder::build`](backend_mgr.rs) 可选创建 `CheckpointManager`，然后调用 `BackendContext::new(job_id, mem_root, disk_root, checkpoint)`。此时没有引擎，`last_flush` 从构造时刻计时。
2. 上层通过 [`register_engines`](engine_mgr.rs) 调用 `register`。若目标 index ID 全部已存在，返回已有引擎；只命中一部分时返回包含 job ID 和数量的错误；全未命中时逐个创建以 `job-{job_id}-index-{index_id}` 标记的 `EngineInfo`。
3. writer/reader 处理 chunk 时，上层可调用 `add_chunk`、`update_chunk`、`finish_chunk`。这些方法只有在 `checkpoint` 为 `Some` 时才改变状态。
4. 写入循环周期性调用 `ingest_if_quota_exceeded`：
   - `None`：返回 `Ok(false)`；
   - `Flush`：flush 全部引擎，重置 `last_flush`，调用 `advance_watermark(false)`，返回 `Ok(false)`；
   - `Import`：调用 `ingest`，成功后返回 `Ok(true)`。
5. `ingest` 先 flush 全部引擎，再增加 `import_count`，最后调用 `advance_watermark(true)`。因此 checkpoint 失败时计数已经增加，不会回滚。
6. 正常结束时 `finish_and_unregister(cleanup, check_duplicates)` 先 flush；若要求查重，则逐 index ID 检查，任一重复立即报错；全部通过后关闭所有引擎并清空 map。
7. 放弃或关闭上下文时 `close` 以 `cleanup = false` 关闭当前引擎并设置 `closed = true`。它不清空 `engines`，也不调用 `CheckpointManager::close`。

## 数据与状态

`BackendContext` 自身需要 `&mut self` 的操作（注册、配额执行、结束、chunk 更新）由调用方串行化；共享的引擎对象用 `Arc` 分发。`EngineInfo` 内部使用 `Mutex<EngineState>` 保护有序 `rows`、writer 数、closed 与 flushed 标志；`Engine::close` 会释放本引擎在 `MemRoot` 中的 tag，`cleanup = true` 时还清空 rows。

`engines` 使用 `BTreeMap`，因此 flush、查重和关闭的遍历顺序按 index ID 稳定。重复注册的幂等性只按 index ID 集合判断：重复调用时新的 `unique` 标记和 `writer_memory` 不会覆盖原引擎。调用者必须保证同一组 ID 的语义参数一致。

checkpoint 包含两级进度：flush 后可推进 local watermark，成功 import 后才把 global watermark 对齐到 local watermark。`CheckpointManager::advance_watermark` 只连续越过读写均完成的 task ID，防止跨过未完成区间。`next_start_key` 在本地数据仍可信时使用 local key，否则退回 global key。

`force_sync` 是实例字段；为 true 时 `check_flush` 直接选择 `Import`。`last_flush` 仅在周期 `Flush` 分支更新；直接 `ingest`/`Import` 分支不会更新它。`import_count` 只是内存内统计，不是持久化进度依据。

## 依赖与调用关系

上游与装配关系：

- [`backend_mgr.rs::BackendContextBuilder::build`](backend_mgr.rs) 创建可选 checkpoint 并调用 `BackendContext::new`。
- [`engine_mgr.rs::register_engines`](engine_mgr.rs) 直接委托 `BackendContext::register`；同文件的 `finish_and_unregister_engines` 使用 `engines`、`Engine::close` 与 `collect_remote_duplicate_rows` 实现选项式注销。注意它不是本文件 `finish_and_unregister` 的简单调用者，两条 Rust API 当前并存且执行顺序不同。
- [`backfilling_operators.rs`](../backfilling_operators.rs) 的 `IndexWriter` trait 定义写入流水线需要的 `flush` 和 `ingest_if_quota_exceeded(task_id, row_count)` 契约；具体适配器负责把流水线事件连接到后端/checkpoint。

下游依赖：

- [`engine.rs`](engine.rs)：`EngineInfo::new`、`Engine::flush/close`、`EngineInfo::rows/unique`。这是实际本地行缓存、锁和内存 tag 生命周期所在。
- [`disk_root.rs`](disk_root.rs)：`DiskRoot::should_import`；当已统计的 backend 使用量超过 quota，或磁盘使用率达到 90% 时请求 import。调用方需要另行刷新 `DiskRoot` 的 usage，`check_flush` 自身不做探测。
- [`checkpoint.rs`](checkpoint.rs)：所有 chunk、watermark、恢复起点和 import TS 的真实状态机。
- 标准库 `BTreeMap`、`Arc`、`Duration`、`Instant`：分别提供确定性映射、共享所有权和单调时间间隔。

RustCodeGraph 将目标文件标为被 15 个文件使用，并识别到 `backend_mgr.rs`、`backfilling_operators.rs`、`index.rs`、`backend_test.rs` 等直接关联；但对 impl 内函数 ID 执行精确 `callers/callees` 未产生静态边，因此上面的具体连接由这些已索引入口源码交叉核验，不能据此声称图覆盖了动态 trait 调用。

## 错误处理与边界

- 所有可失败操作统一返回 `Result<_, String>`，缺少结构化错误类型和 source chain。
- `register` 在上下文关闭、`index_ids`/`unique` 长度不等、或注册组部分重叠时失败。它没有显式拒绝同一输入切片内重复的 index ID；这种输入会覆盖 map 条目但仍在返回向量中产生多个 `Arc`，调用者应保证 ID 唯一。
- 新建引擎阶段当前不会调用 Go 版本的 backend open，也不会预检 `MemRoot` 总量；真正的 writer 内存不足在 `EngineInfo::create_writer` 返回 `"memory used up"`。
- `flush_engines` 在首个失败处停止，后续引擎不会 flush；已 flush 的引擎不回滚。
- `collect_remote_duplicate_rows` 对未知 index ID 返回 `"engine not found"`。它按 value 重复判断，而 `EngineInfo` 的 rows 是按 key 存储且相同 key 会覆盖，因此它既不是远端 TiKV 扫描，也不能完整复现 Go 唯一键冲突语义。
- `finish_and_unregister` 若查重或 flush 失败，不关闭、不清空引擎；若某个 `close` 内部发生 mutex poison，底层 `unwrap` 会 panic 而非返回错误。
- `ingest` 在 flush 成功后先增加 `import_count` 再推进 checkpoint；checkpoint 失败会向上传播，但计数保留。
- 未启用 checkpoint 时，所有更新为 no-op，`next_start_key` 为空、计数和 TS 为 0；这不是错误。
- `advance_watermark(true)` 可能因 global watermark 大于 local watermark，或 checkpoint storage 保存失败而返回错误。

## 并发与资源生命周期

`BackendContext` 没有实现内部 mutex，也没有声明 `ingest` 可并发调用；需要可变访问的方法通过 Rust 借用规则在单一所有者内串行，但若调用方再包一层锁，共享调度策略仍由调用方负责。与之相对，每个 `EngineInfo` 内部状态由 `Mutex` 保护，`Arc<EngineInfo>` 可安全共享给多个 writer；writer drop 时释放自己的 `MemRoot` tag，engine close 时释放 engine tag。

典型生命周期是 builder 构造 → register engines → 创建/使用 writers 与更新 chunks → 周期 flush/import → finish-and-unregister 或 close。安全扩展必须保持以下顺序不变量：数据先 flush，才可推进 local watermark；只有远端 import 确认成功，才可用 `imported = true` 推进 global watermark；重复键检查所需数据未检查完之前不能 cleanup。

当前 Rust 与 Go 并发保证存在显著差异：Go `Ingest` 声明并发安全，使用 `atomic.Bool` 抑制并行 flush、每引擎 `flushLock`、etcd 分布式 import 锁，以及 `unregisterMu` 串行注销；本文件都没有对应机制。`close` 也只关闭引擎并置位，未从 `DiskRoot` 移除 tracker，tracker 的注册/移除应由拥有者配对处理。

## 与 Go 版本的对应关系

主要对应如下：Rust `BackendContext` 对应 Go `litBackendCtx`，`FlushDecision` 展开了 Go `checkFlush() (shouldFlush, shouldImport)` 的两个布尔值，checkpoint 转发方法对应 Go `CheckpointOperator`，十分钟间隔与 Go `checkpointUpdateInterval` 一致。Rust `register` 保留 Go 的整组幂等与部分重叠报错，相关 Rust 回归测试明确锁定该行为。

仍未等价的关键点：

- Go `Register` 先刷新/检查内存、通过 Lightning engine manager 打开真实 engine，并在部分创建失败时清理；Rust 只构造内存 `EngineInfo`，writer 创建时才检查内存。
- Go `IngestIfQuotaExceeded` 先 `FinishChunk(taskID, count)`，用 atomic 保证单次 flush，并在 import 前取得分布式锁；Rust 方法没有 task 参数、原子 flush gate 或分布式锁。
- Go `Ingest` 调用 `SetTSBeforeImportEngine`、closed-engine `Import`、`ResetEngineSkipAllocTS` 并转换 duplicate-key 错误；Rust `ingest` 不访问 TiKV，仅 flush、计数和推进 checkpoint。因此文档中的“Import”是当前 Rust 决策名称，不代表已完成 Go 的远端导入行为。
- Go `CollectRemoteDuplicateRows` 通过 Lightning dupe controller 扫描远端 TiKV，并把错误转换为用户可见的 key-exists；Rust 只统计本地 values，API 名称比当前能力更宽。
- Go `checkFlush` 主动 `LitDiskRoot.UpdateUsage()`，支持 failpoint 调整；Rust 只读取已缓存的 `DiskRoot` 状态，并暴露普通 `force_sync` 字段。
- Go `FinishAndUnregisterEngines` 用 mutex 保证可重复并发调用，先 close 后远端查重；Rust 本文件的方法先 flush/查重后 close，且不并发安全。Rust [`engine_mgr.rs`](engine_mgr.rs) 的另一路实现更接近 Go 的 close-then-check 顺序，但仍没有 mutex。
- Go 无 checkpoint 时 `GetImportTS` 返回独立的 `importTS`；Rust 无 checkpoint 时返回 0。
- Go `Close` 负责从全局 disk root 移除 job 并更新测试计数，不直接关闭 `engines`；Rust `close` 关闭 engine 并禁止新注册，但不移除 `DiskRoot` tracker。

因此移植状态应描述为“保留核心形状、注册策略、flush 决策与 checkpoint 接口的可测试局部实现”，不能标记为完整 Go 行为对齐。

## 扩展指南

- 若接入真实存储导入，应优先扩展 `ingest`，并把“flush → 获得跨实例互斥 → 设置 import TS → import → reset → `advance_watermark(true)`”作为不可拆乱的成功序列；失败时不得提前推进 global watermark。同步扩展独立的 `backend_test.rs`，并为存储适配器另建测试文件，不把测试嵌入生产源码。
- 若补并发 ingest/注销，需为 flush/import 和 unregister 分别定义锁粒度，避免在持有 `EngineInfo` mutex 时执行长时间远端 I/O；对齐 Go 的 `flushing`、`flushLock`、distributed lock 与 `unregisterMu` 意图。
- 若修正重复键能力，应明确检查对象是编码 key、row value 还是远端索引记录，并返回可映射到 `ErrKeyExists` 的结构化信息；应覆盖唯一/非唯一索引、相同 key 覆盖、不同 key 相同 value 和远端冲突。
- 若改变注册语义，保持整组幂等、部分重叠拒绝，并增加“输入 index ID 自身重复”与重复调用参数不一致的测试。真实 engine 创建必须具备失败清理，避免半注册。
- 若改变 flush 策略，应同步检查 `DiskRoot::update_usage/should_import`、checkpoint 周期和 `memory_pressure_alone_does_not_trigger_go_flush_policy`；不要擅自把内存压力加入 Go 未定义的决策路径。
- 若调整 checkpoint 接口，必须保持 task ID 连续推进、本地数据有效性判定、local/global 水位线顺序以及无 checkpoint 的零值契约，并同步 [`checkpoint_test.rs`](checkpoint_test.rs) 与 [`backend_test.rs`](backend_test.rs)。
- 兼容性风险集中在恢复起点与 import TS；正确性风险集中在提前推进 watermark、查重遗漏和部分注册；性能风险集中在逐次克隆全部 rows、串行 flush 全部引擎以及过粗锁导致的 ingest 停顿。

## 验证依据

- RustCodeGraph 索引状态：项目共 11,467 个文件、目标 `pkg/ddl/ingest/backend.rs` 已索引，目标文件报告 30 个符号、15 个引用文件；完整读取目标 1–285 行，并查询 `BackendContext`、`ingest_if_quota_exceeded`、`finish_and_unregister`、`collect_remote_duplicate_rows`、`advance_watermark`。精确 `callers/callees` 无返回，已在“依赖与调用关系”中限定证据边界。
- 已读 Rust 直接证据：[`backend.rs`](backend.rs)、[`backend_mgr.rs`](backend_mgr.rs)、[`engine_mgr.rs`](engine_mgr.rs)、[`engine.rs`](engine.rs)、[`checkpoint.rs`](checkpoint.rs)、[`disk_root.rs`](disk_root.rs)、[`lib.rs`](lib.rs)、[`backfilling_operators.rs`](../backfilling_operators.rs)。
- crate 证据：[`Cargo.toml`](Cargo.toml) 声明包名 `astersql-ddl-ingest`、库入口 `lib.rs` 和 Go 包映射 `pkg/ddl/ingest`；目标文件直接使用的是本 crate 模块与标准库，未受 Cargo 中 Windows 条件依赖改变语义。
- Go 对照证据：[`backend.go`](backend.go) 的 `BackendCtx`/`CheckpointOperator`/`litBackendCtx`、`IngestIfQuotaExceeded`、`Ingest`、`checkFlush`、checkpoint 转发与 `Close`；[`backend_mgr.go`](backend_mgr.go) 的构造与十分钟间隔；[`engine_mgr.go`](engine_mgr.go) 的整组注册和注销逻辑。
- 测试证据：[`backend_test.rs`](backend_test.rs) 覆盖整组重复注册复用、部分重叠拒绝，以及“仅内存压力不触发 Go flush 策略”；Go [`checkpoint_test.go`](checkpoint_test.go) 覆盖 local/global watermark 推进和恢复，[`integration_test.go`](integration_test.go) 覆盖 ingest duplicate message、job conflict 与 partition checkpoint 等完整 Go 路径。后两者证明 Go 语义背景，不证明 Rust 已实现完整集成能力。
- 本任务是纯文档分析，按计划不运行 Cargo。结构验收使用任务指定命令，要求目标文件存在且恰有 11 个固定二级标题；另人工复核本文明确回答文件存在原因、运行流程、安全扩展点与未移植边界。
