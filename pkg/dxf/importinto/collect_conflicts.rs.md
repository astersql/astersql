# `pkg/dxf/importinto/collect_conflicts.rs`

## 文件定位

该文件属于 `astersql-dxf-importinto` crate；crate 根 `pkg/dxf/importinto/lib.rs` 以 `pub mod collect_conflicts` 声明并公开再导出本模块。它位于全局排序 IMPORT INTO 的冲突处理链中：规划器生成 `CollectConflictsStepMeta`，本文件负责把冲突 SST 中的 KV 交给 `conflictedkv::Collector`，汇总冲突行、checksum 和持久化文件信息；后续冲突删除还复用这里的索引识别、按行路由、键空间解码和有界通道发送逻辑。

当前 Rust 接线必须与 Go 版本区别看待：`collectConflictsStepExecutor` 提供子任务状态和按组编排方法，但未实现 Rust 的 `execute::StepExecutor`，仓库内也没有生产调用 `RunGroups` 或 `CollectConflictGroup`；`CollectConflictGroup` 的直接调用者目前是 `pkg/dxf/importinto/conflict_resolution_test.rs`。另一方面，`pkg/dxf/importinto/conflict_resolution.rs` 已在生产实现中直接复用 `getKVGroupIndexInfo`、`conflictWorkerForPair`、`sendConflictPair` 和 `decodeConflictKey`。因此本文件不是空桩，但 collect-conflicts 步骤尚不能仅凭此文件视为完成了 Go 版端到端框架接线。

## 核心职责

- `collectConflictsStepExecutor` 保存一个 collect 子任务跨 KV 组共享的结果、内存计数、冲突行文件大小、去重集合和进度摘要。跨组共享 `sharedRowKeySet` 是为了避免同一数据行同时命中多个唯一索引时重复计入 checksum。
- `RunGroups` 串行遍历 `meta.Infos.ConflictInfos`。串行是语义约束而非性能偶然：不同唯一索引组可能指向同一行，必须让共享去重状态覆盖整个子任务；组内则允许由回调按 `concurrency` 并行。
- `CollectConflictGroup` 实现一个真实 SST 组的读取、worker 初始化、分发、收集、关闭和结果合并边界。对象存储、集群存储和 codec 工厂由调用者注入，本函数拥有线程、通道和取消令牌的生命周期。
- MVI（多值索引）KV 必须按解码后的行 handle 做 IEEE CRC32 分片，使同一行派生的多个索引 KV 进入同一个 worker；普通索引、数据 KV 或单 worker 场景按输入序号轮转。
- `applyCollectResult` 把聚合结果转换为持久化的 `CollectConflictsStepMeta` 字段；`Collector::Processed` 则把已处理 KV 数累计到实时摘要。

## 主要符号

- `collectConflictsStepExecutor`：子任务级状态容器。`taskID` 和 `currSubtaskID` 标识输出命名范围；`sizeOfRowKeysFromIndex` 与 `sizeLimitOfRowKeysFromIndex` 控制索引行键缓存；`sizeOfConflictRowFiles` 统计输出；`result`、`sharedRowKeySet`、`summary` 分别保存聚合结果、跨组去重状态和进度。
- `collectConflictsStepExecutor::new`：用 importer 的真实 keyspace 创建空结果，所有计数归零；新实例的行键上限为零，实际子任务开始前必须调用 `resetForNewSubtask`。
- `resetForNewSubtask(subtask_id, memory_capacity)`：清零原子计数和聚合结果，以 `memory_capacity / 2` 作为索引行键去重上限，并重建有界集合。
- `RunGroups(meta, concurrency, collect)`：将并发度钳制为至少 1，先验证组名对应的表索引，再为每组创建 encoder，调用注入的收集闭包并合并结果。
- `onFinished` / `applyCollectResult`：写回 checksum、冲突行数、文件名、录制截断标志和索引冲突过多标志。`TotalFileSize` 不属于该步骤 meta 的持久化字段。
- `Cleanup`、`RealtimeSummary`、`ResetSummary` 与 `Collector` 实现：关闭 `TableImporter`，刷新/清空摘要，并以 Relaxed 原子加法记录处理量；`Accepted` 在 Go/Rust 中均为空实现。
- `getConflictRowFilenamePrefix`：委托 `conflictrows::NewFileNamePrefixWithUUID` 生成 `conflicted-rows/<task>/<subtask>-<uuid>`，刻意避开会被任务清理删除的 `<task-id>/` 目录。
- `getKVGroupIndexInfo`：数据组 `data` 返回 `None`；索引组先解析十进制索引 ID，再在 `TableInfo.Indices` 中查找并克隆 `IndexInfo`；非法组名或缺失索引返回字符串错误。
- `conflictWorkerForPair` / `mvIndexWorker`：校验 worker 数量；只有 `MVIndex && workers > 1` 才解码 handle 并做 CRC32，否则返回 `ordinal % workers`。
- `decodeConflictKey`：空 keyspace 原样返回键；非空时要求键以真实 keyspace 开头并移除该前缀，拒绝错 keyspace 或截断键。
- `dispatchMVIndexKVPairs`：从输入通道带超时接收，按 MVI handle 路由，再带取消检查写入有界输出通道；函数拥有 `outputs`，返回时所有 sender 被析构。
- `CollectConflictGroup`：组级主入口。它为每个 worker 创建 codec、`BoundedKeySet` 和 `conflictedkv::Collector`，读取 `ConflictInfo.Files`，分发 KV，等待 worker，合并 `CollectResult` 与本地行键集合。
- `sendConflictPair`：对满的同步通道以 1ms 间隔重试，并在每轮检查取消；接收端消失时返回 worker stopped 错误。

## 执行流程

1. 子任务状态通过 `resetForNewSubtask` 初始化：选择 importer keyspace，重置结果/文件计数，并把一半内存预算留给索引行键去重。
2. `RunGroups` 逐个读取 `CollectConflictsStepMeta.Infos.ConflictInfos`。每组先由 `getKVGroupIndexInfo` 验证 `data` 或索引 ID，再由 `createEncoders` 一次创建该组全部 encoder；初始化失败时不会启动该组读取。
3. 实际组收集可进入 `CollectConflictGroup`。函数首先拒绝零 worker，然后在 scoped threads 中创建每 worker 一个容量至少为 1、通常为 `BufferedHandleLimit` 的同步通道。
4. 每个 worker 先调用 codec 工厂并配置 `cluster.Keyspace()`，随后通过 ready 通道报告初始化结果。主线程收齐所有 ready 结果后才开始 `ReadKVFilesAsync`，保持 Go 版本“encoder/codec 初始化先于 SST 读取”的顺序。
5. 读取出的 KV 被转换为 `ConflictKVPair`。普通组按 `ordinal % workers` 分散；多 worker MVI 组先去除 keyspace、解码唯一索引 handle，再以 handle 编码值的 IEEE CRC32 选 worker，保证同一行的多个 MVI 项在同一 collector 批次中处理。
6. 每个 worker 的 `Collector::Run` 消费通道；无论运行结果如何都调用 `Collector::Close`。当前 Rust 使用 `result.and(closed)`：若 `Run` 已失败，优先保留运行错误；仅运行成功时返回关闭错误。
7. 主线程遇到取消、SST 读取错误、路由错误或发送错误时记录首个失败并取消 reader；随后丢弃全部 sender，使 worker 能结束，再逐个 join。worker panic 和 worker 返回错误也只填充尚未设置的首个失败。
8. 成功 worker 的结果和局部行键集合始终被合并；全部 join 后若存在失败则返回错误，否则返回 `(CollectResult, BoundedKeySet)`。外层 `RunGroups` 再把组结果合并进子任务结果。
9. 完成时 `applyCollectResult` 把最终聚合状态写入步骤 meta，供 scheduler/post-process 读取；其中 `TooManyConflictsFromIndex` 会影响后续 checksum 验证策略。

## 数据与状态

- `CollectConflictsStepMeta.Infos.ConflictInfos` 是按 KV 组索引的输入；每个 `ConflictInfo` 提供冲突计数和 SST 文件列表。Rust map 的具体迭代顺序不应被业务依赖，正确性由跨组共享去重集合保证。
- `CollectResult` 聚合 `Checksum`、`RowCount`、`Filenames`、`RowRecordingCapped` 和文件大小。`Merge` 用于 worker 到组、组到子任务两级归并。
- `sharedRowKeySet` 是整个子任务的全局去重集合；每个 worker 另建 local set，worker 完成后返回并合并。所有 local sets 共享 `Arc<AtomicI64>` 计量，从而共同遵守 `memory_capacity / 2` 上限。
- `sizeOfConflictRowFiles` 同样由所有 worker 共享，用于协同冲突行录制上限；具体截断决策由 `conflictedkv::Collector` 执行，本文件只负责共享计数和持久化 `RowRecordingCapped`。
- 输出前缀在每个 worker 下追加随机 UUID，因此并行 worker 和重复执行不会复用同一文件名。独立测试验证第二次收集生成的文件集合与第一次不相交。
- 原子内存计数重置使用 `Release`、collector 读取配置容量使用 `Acquire`；进度计数只需统计意义，使用 `Relaxed`。`CollectResult` 的合并发生在 join 后或 `&mut self` 串行上下文中，不靠原子同步。

## 依赖与调用关系

- crate 边界由 `pkg/dxf/importinto/Cargo.toml` 声明。本文件直接依赖 `astersql-dxf-framework-taskexecutor-execute`（进度接口）、`astersql-dxf-importinto-conflictedkv`（collector、上下文、结果和有界集合）、`astersql-executor-importer`（importer/encoder）、`astersql-ingestor-globalsort`（异步 SST reader）、`astersql-ingestor-engineapi`（`ConflictInfo`）、`astersql-objstore-storeapi`、`astersql-meta-model`、`astersql-tablecodec`、`astersql-kv`、`astersql-errors` 和 `uuid`。
- 上游规划和调度位于 `planner.rs`、`scheduler.rs` 与 `proto.rs`：它们生产、传递并汇总 `CollectConflictsStepMeta`，但当前 Rust 生产执行器没有调用本文件的 `RunGroups`/`CollectConflictGroup`。Go 的完整入口是 `collect_conflicts.go::RunSubtask`。
- 下游核心是 `conflictedkv::NewCollector`、`globalsort::reader::ReadKVFilesAsync`、`tablecodec::DecodeIndexHandle` 和 `conflict_resolution::createEncoders`。
- `conflict_resolution.rs` 是已确认的生产调用者：`ResolveConflictGroup` 及其它冲突删除路径使用 `getKVGroupIndexInfo`、`conflictWorkerForPair` 与 `sendConflictPair`，`ImporterConflictCodec` 使用 `decodeConflictKey`。这让收集和删除共享完全相同的 keyspace 与 MVI 路由规则。
- RustCodeGraph 将本文件识别为 51 个符号、被 9 个文件使用，但对关键符号的精确 `callers/callees` 查询返回空边；因此上述调用关系以源码调用点搜索复核，而非把缺失图边解释为“没有调用者”。

## 错误处理与边界

- `RunGroups` 把索引元数据错误包装为 `errors::SharedError`，并用 `?` 保持首个错误短路；已合并的先前组结果不会回滚。
- 并发度的边界分两层处理：`RunGroups` 将非正数钳制到 1；可独立调用的 `CollectConflictGroup` 和 `conflictWorkerForPair` 对零 worker 明确报错，避免取模或索引越界。
- 索引组必须是十进制 ID 且能在表元数据中找到。数据组是特殊字符串 `DataKVGroup`；任何其它不可解析或悬空 ID 都失败，不静默退化为数据组。
- 非空 keyspace 下，冲突键必须包含完整、匹配的前缀。handle 缺失、索引值解码失败或 worker 数量不能表示为有效路由都会阻止继续分发。
- 通道满不是错误，发送端会重试；取消和接收端断开才终止。输入断开是 `dispatchMVIndexKVPairs` 的正常完成条件，并通过 sender 析构关闭全部输出。
- `CollectConflictGroup` 保留最先观察到的失败，但仍 join 全部 worker 并收集其可用结果，以避免遗留线程。线程 panic 被转换为 `conflict collector panicked`，不会越过函数边界 unwind。
- codec 初始化用 ready barrier 聚合所有结果；即使某个 worker 初始化失败，主线程仍等待每个 worker 的初始化报告，随后关闭通道并 join，而不会先读 SST。
- `Cleanup` 无返回值并直接调用 `TableImporter::Close`，与 Go 版可返回 close error 的签名不同；该差异在为 Rust executor 增加框架接线时需要处理。

## 并发与资源生命周期

- KV 组在 `RunGroups` 层串行，组内 worker 并行。这个层次同时满足跨唯一索引去重和单组吞吐，不应简单改为跨组并行。
- `std::thread::scope` 保证所有 worker 在 `CollectConflictGroup` 返回前完成；每个 worker 独占 codec、collector、receiver 与 local key set，共享表、存储适配器、全局集合和原子计数。
- ready channel 是启动屏障：所有 worker 的 codec 都完成构造/配置后才允许读取对象存储，避免部分 worker 初始化失败时已经产生外部读取和部分输出。
- 同步通道提供背压，容量来自动态 `BufferedHandleLimit`。`sendConflictPair` 的短暂 sleep 避免忙等持续占满 CPU，同时保持取消可观测；这不是无界队列。
- reader cancellation token 与业务 `ConflictContext` 分离：上下文取消或任何读取/路由/发送失败会取消 SST reader；丢弃 senders 则通知 collector 输入结束。
- worker 总是尝试 `Close` collector，随后才导出结果与行键；输出 store、cluster store 等 `Arc` 克隆随 worker 结束释放。`collectConflictsStepExecutor::Cleanup` 另行负责 importer 生命周期。
- `dispatchMVIndexKVPairs` 拥有输出 sender 向量，因此成功、取消、解码错误或发送失败都会关闭输出；对应 Rust 测试明确验证了关闭行为。

## 与 Go 版本的对应关系

- Rust `collectConflictsStepExecutor` 对应 `collect_conflicts.go` 同名类型，字段覆盖 Go 的 importer、当前子任务、两个大小计数、结果、共享行键集合和摘要；Go 还内嵌框架基类并持有 task/store/taskMeta/logger，Rust 目前把实际 I/O 适配器移到函数参数。
- Go `RunSubtask` 负责对象存储录制、meta JSON/外置 meta 读取、资源并发度、failpoint、写回序列化和 metering；Rust 本文件没有等价的 `StepExecutor::RunSubtask`。Rust `RunGroups` 只覆盖按组串行、encoder 创建和结果合并，不能等同完整 Go 入口。
- Rust `CollectConflictGroup` 对应 Go `collectConflictsOfKVGroup` 的核心数据面，但采用显式 trait 注入和 scoped threads。Go 仅在多 worker MVI 时建立独立分发协程，普通组让多个 collector 直接消费共享输入；Rust 对所有场景建立每 worker 通道，再以 ordinal 轮转普通组，语义目标相同但调度实现不同。
- Go `dispatchMVIndexKVPairs` 使用存储 codec 解码 key、标准库 `crc32.ChecksumIEEE` 路由，并 defer 关闭通道；Rust `decodeConflictKey` 使用显式 keyspace，手写 IEEE CRC32，并依靠 sender 所有权关闭通道。`collect_conflicts_test.rs::mv_index_routes_encoded_handles_and_closes_channels` 用已知 CRC 值校验兼容性。
- Go `onFinished` 除赋值外还记录日志、序列化 meta 回 `subtask.Meta`；Rust `applyCollectResult` 只执行可测试的五字段转换，序列化和日志需由未来框架入口承担。
- Go `resetForNewSubtask` 从框架资源读取内存容量；Rust 把 `memory_capacity` 作为显式参数。两者都用一半容量，并按真实 keyspace 重建 checksum 和去重状态。
- Go 内部测试覆盖 API v1/v2 keyspace、int/common handle、解码失败与发送中取消；Rust 独立测试已覆盖 keyspace 前缀、MVI 同 handle 路由、输出关闭、元数据错误与取消，但 common handle 和完整 Go executor 生命周期仍主要由 Go 测试提供证据。

## 扩展指南

- 接入完整 Rust collect-conflicts 步骤时，应在独立执行器文件或本模块实现 `execute::StepExecutor`，补齐对象存储/外置 meta、metering、序列化回写和 importer 创建；不要把这些职责塞进 `CollectConflictGroup` 而破坏其可测试 I/O 边界。
- 修改 KV 组命名或索引查找时，从 `getKVGroupIndexInfo` 入手，并同步检查 `conflict_resolution.rs` 的复用路径以及 `collect_conflicts_test.rs`、Go `collect_conflicts_internal_test.go` 中的数据组、非法 ID、缺失 ID 用例。
- 修改 MVI 分片必须保持 Go 的“解码后 handle + IEEE CRC32”契约，并同时验证空/非空 keyspace、int/common handle、同一 handle 多 KV、单 worker、普通索引和零 worker。分片算法变化会影响批次内去重正确性，不能只做负载均衡测试。
- 修改通道或取消逻辑时，应保持：初始化屏障先于读取、满队列时仍可取消、任意返回路径关闭输出、发生错误后 join 全部 worker、collector 总是 close。新增回归测试应放在独立的 `pkg/dxf/importinto/collect_conflicts_test.rs`，不要内嵌进生产源文件。
- 修改 checksum/元数据时，从 `applyCollectResult` 和 `CollectResult::Merge` 的边界入手，保持五个持久化字段与 Go 一致；特别关注 `TooManyConflictsFromIndex` 对 post-process 校验的兼容影响。
- 修改冲突行路径时同步检查 `conflictrows.rs` 和清理逻辑；路径必须继续位于任务临时目录之外，并保证并发/重试生成唯一文件名。
- 性能风险集中在每对 KV 的 handle 解码与 CRC、1ms 重试、每 worker 有界缓存、行键集合内存上限和结果文件数量；优化前应保留正确性不变量，并增加基于真实非空 SST 的独立测试。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，`files --filter pkg/dxf/importinto/collect_conflicts.rs` 命中目标；`node --file ... --offset 1 --limit 1200` 读取了 481 行完整源码；`query` 定位 `CollectConflictGroup`、`conflictWorkerForPair`、`sendConflictPair`、`applyCollectResult` 和 `getConflictRowFilenamePrefix`。精确 callers/callees 返回空数组，调用边因此又用仓库搜索验证。
- 生产源码：`pkg/dxf/importinto/collect_conflicts.rs`、`lib.rs`、`conflict_resolution.rs`、`planner.rs`、`scheduler.rs`、`task_executor.rs`、`subtask_executor.rs`、`proto.rs`；其中 `conflict_resolution.rs` 提供直接生产调用证据，任务执行器搜索提供 collect 主入口尚未接线的证据。
- crate/架构：`pkg/dxf/importinto/Cargo.toml`、`docs/agents/dxf/README.md`、`docs/agents/import-into/README.md`。目标 Go package 没有 `doc.go`；读取了最近的 DXF/IMPORT INTO 导航说明作为包级背景。
- Go 对照：`pkg/dxf/importinto/collect_conflicts.go`、`collect_conflicts_internal_test.go`、`collect_conflicts_test.go`；关键对应点包括完整 `StepExecutor` 生命周期、跨组去重、MVI CRC32 路由、keyspace codec、取消、输出关闭和 meta 写回。
- Rust 独立测试：`pkg/dxf/importinto/collect_conflicts_test.rs` 验证五字段写回、冲突文件路径、MVI 路由与 keyspace、索引元数据错误、取消和通道关闭；`pkg/dxf/importinto/conflict_resolution_test.rs` 以真实非空 SST 调用 `CollectConflictGroup`，验证行数、local set、checksum、输出内容、进度和重复执行文件名唯一性。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求文档存在且恰好包含规定的 11 个二级标题。
