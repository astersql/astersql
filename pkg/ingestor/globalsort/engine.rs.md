# `pkg/ingestor/globalsort/engine.rs`

## 文件定位

`load_range_batch_data` 完成单批读取、排序、重复键处理、内存在途计费与范围构造。

## 核心职责

- `NewExternalEngine` 验证构造参数，保存文件、键范围、资源限制、统计量与重复键策略，建立可共享的 `EngineResource`。
- `LoadIngestData`/`LoadIngestDataWith` 按工作并发度对 `job_keys` 分批，根据 stat 文件的偏移读取 data 文件，产出 `DataAndRanges`。前者收集所有批次以兼容旧调用者，后者逐批回调交付，是有界生产消费链路。
- `load_range_batch_data` 完成单批读取、排序、重复键处理、内存在途计费与范围构造。
- `EngineResource::UpdateResourceWith` 与 `handle_concurrency_change` 协作，在运行期更改批大小和内存上限，并保证重建缓冲前旧批次已释放。
- `MemoryIngestData`/`MemoryDataIter` 提供已排序 KV 的范围查询、零载荷共享迭代、显式引用计数、导入统计与一次性释放回调。

## 主要符号

- `writeStepMemShareCount` 与 `getEngineMemoryLimit(mem_capacity)`：按 Go 版 `writeStepMemShareCount = 6.5` 的份额算法，将总容量的 `3 / 6.5` 分配给引擎加载；非正容量映射为 0。
- `WorkerPoolTuner`：仅暴露 `Tune(usize)` 的可线程间共享 trait，由资源更新通知下游 worker pool。
- `DataAndRanges`：一批 `MemoryIngestData` 及与 job key 相邻对应的半开 `KeyRange` 列表。
- `LoadState`/`LoadGuard`：记录是否正在加载、是否完成、已应用并发度以及是否有资源更新等待确认。`LoadGuard::drop` 在所有返回路径清除 `loading`、设置 `finished` 并唤醒等待者。
- `EngineResource`：共享 worker pool、原子并发度/内存上限和 `Mutex<LoadState> + Condvar`。`ResourceHandle` 让框架在 loader 持有 `&mut Engine` 时仍能更新资源。
- `Engine`：外部引擎状态机。关键域分为文件/范围元数据，`loaded` 工作缓冲，在途批次计数与字节数，资源控制，计划/已导入统计，重复键临时状态和 `closed` 标志。
- `take_deduplicated_pairs`：转移而非克隆 KV payload；某 key 只要出现多次，其所有副本均不进入输出。`Record` 还会把所有被移除的对放入 `duplicate_pairs` 并累计冲突大小/数量。
- `MemoryIngestDataInner`/`MemoryIngestData`：共享 `Arc<Vec<KvPair>>`、时间戳、释放状态、逻辑引用数、导入计数器和 `FnOnce` 回调。外层可克隆，但克隆的是 `Arc`，不是 KV 数据。
- `MemoryDataIter`：持有同一 KV `Arc` 及 `[start, end)` 下标，提供 `First/Valid/Next/Key/Value/Close/Error/ReleaseBuf`。

## 执行流程

1. `NewExternalEngine` 先检查 data/stat 文件数相等、`worker_concurrency > 0` 且 `job_keys` 非降序，再以 `getEngineMemoryLimit` 初始化共享资源。与 Go 构造器相比，Rust 把非法构造参数提前变成 `Result` 错误。
2. `LoadIngestDataWith` 进入 `load_ingest_data_with(..., wait_for_release = true)`；兼容收集 API `LoadIngestData` 传 `false`。函数拒绝已关闭引擎，标记 load state，获取每个 job key 对应的文件读取偏移。
3. 主循环以当前并发度为批大小。每批前 `handle_concurrency_change` 检查新配置；变更时等待所有旧 `MemoryIngestData` 发布完成释放，清空加载缓冲，回写 `applied` 并向更新线程确认。
4. `load_range_batch_data` 调用 `reader::read_all_data` 读取 `[first_job_key, last_job_key)`。可用内存是 `memory_limit - in_flight_bytes`。有界消费路径遇 OOM 时通过 `wait_ingest_data_released` 等待下游释放后重试；收集 API 不等待自己保留的输出，直接返回 OOM，避免自锁。
5. 数据 `build` 后按 key 排序。`Ignore`/`Error` 发现首个相邻重复键即清空工作缓冲并返回 `DuplicateKey`；`Remove` 整组删除重复 key；`Record` 除删除外，将所有副本按流式写入 `<file_prefix>/dup`。写入器懒创建，主流程成功或失败都执行 `finish`，且优先保留先发生的加载/消费错误。
6. 去重后更新已加载数量，把保留 payload 的字节数和批次数加入在途计费，构造 `MemoryIngestData`。其释放回调先扣减在途字节，发布可合并的 Condvar 信号，再扣减在途批次数。
7. 由相邻 job keys 建立半开 `sorted_ranges`，调用 consumer。`ExternalEngineAdapter::LoadIngestData` 把批次适配到通用 engine API 后用 `SyncSender::try_send` 投递；取消或通道断开时显式 `release`。
8. 下游对一批数据 `IncRef`，按范围调用 `GetFirstAndLastKey`/`NewIter`，完成写入后用 `Finish` 累计成果并 `DecRef`。最后一个逻辑引用归零时执行一次性释放。

## 数据与状态

`job_keys` 是批次切分的核心不变量：必须有序，相邻两键定义一个半开范围。少于两键时加载成功但不产生批次。`split_keys` 只作为 Region 预分裂建议对外返回，不参与本文件的批内排序。

`loaded: MemKvsAndBuffers` 是 loader 独占的可复用工作区。产出批次时 `std::mem::take` 转移 KV，不在引擎预算外再克隆 payload。`in_flight_bytes` 计量已交给下游但未释放的保留 KV，`in_flight_data_count` 计批次，`active_ingest_data_flags` 则用于资源变更时判断是否可安全重置工作区。

`total_kv_size/total_kv_count` 是输入元数据，`total_loaded_kvs_count` 是实际去重后加载数，`imported_kv_size/imported_kv_count` 由下游 `Finish` 累加。`recorded_duplicate_count/size` 统计 `Record` 策略的所有被移除副本；`duplicate_pairs` 仅是当批写出前的短期缓冲，写出后立即清空。

`MemoryIngestData` 中的 Rust `Arc` 强引用数与 `reference_count` 用途不同：前者保证 Rust 对象安全共享，后者对齐 Go `IngestData` 的显式 `IncRef/DecRef` 协议。`released` 只在 KV 缓冲清空且 `on_release` 完成后以 Release 顺序发布。

## 依赖与调用关系

上游主链为 `GlobalSortWriteIngestBackend::CloseExternalEngine` → `NewExternalEngine` → `ExternalEngineAdapter::new`，见 `pkg/dxf/importinto/write_ingest_backend.rs`。通用导入框架调用适配器的 `Engine::LoadIngestData`，适配器再调本文件的 `LoadIngestDataWith`；其输出经 `DataAdapter` 对齐 `astersql-ingestor-engineapi::IngestData`。`write_ingest_backend.rs::ImportEngine` 根据 `sorted_ranges` 扫描 Region，生成 `RegionJob`，并将 `GetTS` 的时间戳传给 job。

主要下游依赖是 `reader::get_read_ranges_from_props` 和 `reader::read_all_data`（stat 偏移计算与 data 加载）、`merge::write_stream_pair`（按 `Storage::record_format` 流式写重复键），以及 `Storage`/`ObjectWriter`、`KvPair`、`KeyRange`、`OnDuplicateKey`、`Error` 等 crate 公共抽象。`Cargo.toml` 还声明 engine API、ingestor error definition、Lightning membuf、simplesst 和 workerpool 的路径依赖；需要注意，本文件自身以 crate 内抽象为主，具体对象存储与通用 engine trait 适配分别位于生产调用者和 `engine_api.rs`。

RustCodeGraph 将 `engine.rs` 标记为被 18 个文件使用，其中直接证据包括 `pkg/dxf/importinto/write_ingest_backend.rs`、`pkg/ingestor/globalsort/engine_api.rs`、`pkg/ingestor/globalsort/merge.rs` 及对应测试。本文档仅将上述已读取的直接生产路径作为主链结论，不把索引中的测试或同名符号推测为运行时调用。

## 错误处理与边界

- 构造边界：data/stat 数量不等、非正 worker 并发度、无序 job keys 均返回 `Error::InvalidArgument`。空或单个 job key 不是错误，只是无输出。
- 生命周期：`Close` 后再加载返回 `Error::Closed`；`Reset` 只清空工作缓冲，不关闭引擎。
- 取消：批循环、读取重试等待、重复键流写和资源确认等待均检查 `CancellationToken`，返回 `Error::Cancelled`。
- 内存：无在途批次且没有待消费释放信号时，`wait_ingest_data_released` 返回 `OutOfMemory`，因为等待不可能产生进展。
- 重复键：为保持旧 Go 兼容行为，`Ignore` 与 `Error` 一样报重复键，而非忽略冲突。`Record`/`Remove` 的语义是删除重复 key 的全部副本，不是保留第一个。
- 重复文件：仅 `Record` 且真有重复项时创建。`result.and(close)` 使主加载/消费错误不被同时发生的 writer finish 错误覆盖；主流程成功时才会传播 finish 错误。
- 锁：可恢复的 `Mutex`/`RwLock` poison 路径尽量转为 `Error::Poisoned`；`SetWorkerPool`、显式 release 和迭代器非法位置则使用 `unwrap`/`expect`/`assert` 表达编程协议违反。`DecRef` 无匹配 `IncRef` 或释放后再 `IncRef` 会 panic。
- 范围：`GetFirstAndLastKey` 和 `NewIter` 对 `[lower, upper)` 做二分定位；空 bound 表示无限制，空交集返回空键对/空迭代器。`Key`/`Value` 要求先通过 `Valid`。

## 并发与资源生命周期

`Engine` 的 loader 需要独占 `&mut self`，但 `EngineResource` 由 `Arc` 共享，允许框架在另一线程调用 `UpdateResourceWith`。更新时先发布新内存上限和并发度，loader 在批次边界观察到变化后等旧批次释放、`Reset`、写入 `applied`；更新线程等待该确认后才调 `WorkerPoolTuner::Tune`。若加载已结束，只更新共享参数，不再调工作池。

`release_signal` 用 `Mutex<bool> + Condvar` 模拟 Go 容量为 1 的 channel：多次释放可合并为一个 pending 信号，而未来的 waiter 仍能观察。回调的顺序是先扣字节、置 pending 并唤醒，再扣批次数；waiter 因此先检查 pending，再以 in-flight count 判断是否可能等到进展。

`MemoryIngestData::release` 用 `on_release: Mutex<Option<FnOnce>>` 串行化多个显式释放，先清空 KV，再调回，最后发布 `released=true`。`MemoryIngestDataInner::drop` 覆盖批次未经 `IncRef` 就被丢弃的路径，执行同样的清理与回调。这使资源变更只会在 payload 与回调真正完成后才视为批次已释放。

`MemoryDataIter` 的多个实例共享不可变 `Arc<Vec<KvPair>>`，各自只保存下标游标。关闭某个迭代器只会丢弃它自己的 `Arc`，不会使其他迭代器失效。原子计数器对生命周期协调使用 Acquire/Release 或 AcqRel，纯统计量使用 Relaxed。

## 与 Go 版本的对应关系

Go 对照文件为 `pkg/ingestor/globalsort/engine.go`，Rust 沿用了 `Engine`、`NewExternalEngine`、`LoadIngestData`、`loadRangeBatchData`、`handleConcurrencyChange`、`waitIngestDataReleased`、`MemoryIngestData` 及其范围/引用/统计 API 的语义结构。

Rust 的明确差异也应在扩展时保留意识：Go 构造器直接返回指针，Rust 构造器增加参数校验并返回 `Result<Engine>`；Go 使用 membuf pool/limiter，Rust 将已交付 payload 字节显式纳入 `in_flight_bytes`；Go 用 channel，Rust 用 Condvar 状态机表达资源确认和释放信号；Rust 的 `LoadIngestDataWith` 及适配器实现了原生有界通道的逐批交付，而 `LoadIngestData` 是为现有用例保留的收集式包装。Go 版还含 metrics、logging 和 failpoint，本 Rust 文件未实现这些可观测/故障注入细节，不应在文档中声称已对齐。

## 扩展指南

- 改变分批或读取计划时，以 `load_ingest_data_with` 和 `load_range_batch_data` 为接入点，保持 job key 相邻形成半开范围、stat 偏移首尾对应、取消可传播和收集 API 不因自持批次而永久等待。同步扩展 `pkg/ingestor/globalsort/engine_test.rs` 的多批次、预算与取消用例。
- 新增重复键策略时，同时检查 `OnDuplicateKey`、`load_range_batch_data`、`take_deduplicated_pairs`、`ConflictInfo` 和 `engine_api.rs`/上游模式映射。必须与 `engine.go` 语义核对，并在独立 `engine_test.rs` 覆盖无重复、部分重复、全重复、跨文件及写出失败，不得把测试内嵌到源文件。
- 改动动态资源管理时，需联合修改 `EngineResource::UpdateResourceWith`、`handle_concurrency_change`、`LoadState` 和释放顺序。关键不变量是：旧 payload 和回调完成前不重置，loader 确认新配置前不 Tune worker，取消必须能打断所有等待。
- 改动 `MemoryIngestData` 时，不要将 Rust `Arc` 强引用数误当作通用 engine API 的显式引用协议。要保持 `DecRef` 仅在从 1 到 0 时释放、回调最多一次、drop 能回收未被消费的批次，且迭代器共享原 payload。
- 新增公开 API 时，检查是否应同步 `engine_api.rs` 的 `ExternalEngineAdapter`/`DataAdapter` 以及 `astersql-ingestor-engineapi` trait，并在 `engine_api_test.rs` 增加适配边界测试。
- 性能风险集中在 payload 克隆、在途内存漏计、重复键临时缓冲、线性扫描和资源变更等待。正确性风险集中在半开边界、重复 key 整组删除、释放发布顺序和错误优先级。兼容性风险是更改 `Ignore` 的历史行为、`"external"` ID、dup 文件路径/编码或 Go 接口形状。
