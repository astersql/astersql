# `pkg/dxf/importinto/conflict_resolution.rs`

## 文件定位

本文件实现 `IMPORT INTO` 分布式任务在 `ImportStepConflictResolution` 阶段的 Rust 执行逻辑。它位于“收集冲突 KV”之后：输入是子任务元数据中按 KV group（数据组 `data` 或索引组）记录的冲突 SST 文件，输出效果是通过集群事务删除与这些冲突对应的表行及索引项。模块由 `pkg/dxf/importinto/lib.rs` 的 `pub mod conflict_resolution` 导出，所属 crate 是 `astersql-dxf-importinto`；`pkg/dxf/importinto/Cargo.toml` 声明其直接使用的 framework execute/proto、conflictedkv、importer、globalsort、objstore、tablecodec 等内部 crate，并只提供与本文件无直接条件分支的 `nextgen` feature。

框架侧有两条可见接线。`pkg/dxf/importinto/task_executor.rs::GetConflictResolutionStepExecutor` 校验步骤并构造本文件的 `ConflictResolutionStepExecutor`；同文件的 `ConflictNodeStepExecutor` 与 `ImportConflictExtension::GetStepExecutor` 又把它适配到节点执行器的命令通道和生命周期。目标包没有 `pkg/dxf/importinto/doc.go`，因此包级定位以 `lib.rs`、Cargo manifest 和上述真实接线为准。

## 核心职责

1. `ReadConflictResolutionMeta` 兼容读取 Go wire shape：元数据可以直接包含 `infos.conflict-infos`，也可以只给出 `ExternalPath`，再从同一对象存储加载完整 JSON。
2. `ConflictResolutionStepExecutor` 实现 DXF `execute::StepExecutor`：初始化 importer、打开带访问统计的对象存储、逐组解决冲突、汇总进度/计量并清理 importer。
3. `ResolveConflictGroupFromMeta` 为每个删除 worker 独立构造 importer encoder/codec，待所有 worker 初始化完成后才读取并分发冲突 KV，避免共享 AST/session 初始化状态引起数据竞争。
4. `ResolveConflictGroup` 提供已经准备好 codec 时的较低层处理入口；两条路径最终都使用 `conflictedkv::NewDeleter` 执行真实的快照读取、行/索引定位和事务删除。
5. `ImporterConflictCodec` 在 importer 的编码模型与 conflictedkv 的 `ConflictRowCodec` 协议之间转换，保留整型、公共和分区 handle，并正确处理隐藏函数索引列。
6. `ConflictObjectStorage` 把生产对象存储 API 适配成 globalsort reader 所需的 `Storage`，同时记录 GET/PUT 请求数和流量。

## 主要符号

- `ReadConflictResolutionMeta(bytes, object_store) -> Result<ConflictResolutionStepMeta, SharedError>`：解析内联 JSON；若 `ExternalPath` 非空则读取并解析外部 JSON；逐组校验 `Count` 为无符号整数、`Files` 为字符串数组。缺失字段按空值处理。
- `conflictResolutionStepExecutor`：较低层状态辅助，直接持有具体 `TableImporter` 和 `SubtaskSummary`。`RunGroups` 串行遍历 group、调用 `createEncoders` 后交给回调；`Cleanup`、`RealtimeSummary`、`ResetSummary` 和 `Collector::Processed` 管理资源与计数。它不同于后面的框架生产执行器。
- `ResolveConflictGroup`：接收调用方已创建的 `Vec<Box<dyn ConflictRowCodec + Send>>`，为每个 codec 建立有界同步通道和 scoped worker，读取冲突文件并调用 `conflictWorkerForPair`/`sendConflictPair` 分发。
- `ResolveConflictGroupFromMeta`：生产路径使用的组级入口。每个 scoped worker 内调用 `NewTableDefinitionFromMeta`、`NewTableKVEncoderFromMeta`、`NewImporterConflictCodecWithOptions`，通过 ready channel 完成初始化屏障，然后才开始读 SST。
- `ConflictObjectStorage`：实现 globalsort `Storage` 的 `read`、`write`、`delete_files`、`list_prefix`；读写成功后更新可选 `recording::AccessStats`。
- `ImporterConflictCodec`：保存 keyspace、`TableKVEncoder`、`TableKVDecoder` 和可见列物理 offset；实现 keyspace 去前缀、行/索引 handle 解码、行解码/重编码、行键生成和关闭 encoder。
- `importerHandle`：递归转换 tablecodec handle。分区 handle 保留 `PartitionID`，整型 handle 转成 `IntHandle`，公共 handle 使用原始编码构造，避免依赖可能歧义的字符串表示。
- `ConflictResolutionImporter`：把生产执行器所需的 importer 能力缩成 `Plan`、`TableInfo`、`DatumConverter`、`Close` 四项；`TableImporter` 提供实际实现。
- `ConflictResolutionRuntime` 与 `ConfiguredConflictResolutionRuntime`：注入 importer 构建、对象存储打开和集群 `ConflictStore`。配置实现从持久化 `TaskMeta` 的表信息与 SQL 重建 controller/importer。
- `ConflictResolutionStepExecutor` / `NewConflictResolutionStepExecutor`：框架面对的生产执行器及构造器，保存 task ID/meta、runtime、可选 importer、summary 和 framework info。
- `ResolutionProgress`、`ResolutionMeterTraffic`：分别把 deleter 的 processed 回调写入原子计数、把集群读写字节写入 framework meter。
- `createEncoders`：在 worker 启动前顺序创建指定数量的查重 encoder；中途失败会关闭此前已创建的 encoder。

文件没有自身 feature gate；唯一条件编译项是 `#[cfg(test)] importerHandleForTest`，仅向同 crate 的独立测试暴露私有 handle 转换逻辑。

## 执行流程

框架主路径如下：

1. `task_executor.rs` 解码 `TaskMeta`，确认步骤是 `ImportStepConflictResolution`，调用 `NewConflictResolutionStepExecutor`；节点模式通过 `ConflictNodeStepExecutor` 的专用线程把 `Init`、`RunSubtask`、summary/reset 和 cleanup 命令转给同一个执行器。
2. `Init` 先检查取消状态，再让 runtime 的 `BuildImporter(task_id, task_meta)` 构造并保存 importer。`ConfiguredConflictResolutionRuntime::BuildImporter` 要求 `Plan.TableInfo` 存在，依次构表、解析 `Stmt`、创建并初始化 load-data controller，最后创建 `TableImporter`。
3. `RunSubtask` 从 importer 取得计划、表元数据和 datum converter，用 `Plan.SQLMode`、`ImportantSysVars` 构造 `SessionOptions`；并从 framework `StepResource.CPU.Capacity()` 取得 worker 数。
4. runtime 按 `Plan.CloudStorageURI` 打开底层对象存储，随后用 `ConflictObjectStorage` 包装访问统计。执行器创建独立 `ConflictContext` 和对象存储 context，并启动 watcher：每 10ms 检查 framework context，取消时同时取消冲突处理和对象存储操作。
5. `ReadConflictResolutionMeta` 读取内联或外部元数据。执行器逐个遍历 `meta.Infos.ConflictInfos`；各组之间当前串行，容器的迭代顺序不应被视为业务顺序保证。
6. 每组调用 `ResolveConflictGroupFromMeta`。该函数先识别 group 对应的目标索引，再创建 `concurrency` 个同步通道和 scoped worker。每个 worker 在自己的线程内创建 table definition、encoder 和 codec，配置集群 keyspace，发送 ready 信号，然后运行 `NewDeleter(...).Run(...)`。
7. 主线程收齐所有 ready 结果。只有无初始化错误时才用 `ReadKVFilesAsync` 顺序消费 `ConflictInfo.Files`；每个 pair 经 `conflictWorkerForPair` 选择稳定 worker，再由 `sendConflictPair` 写入有界通道。完成、取消或出错后丢弃所有 sender，使 worker 收到流结束，并 join 全部 worker。
8. 组级函数按“初始化错误、读取/分发错误、worker 错误”的优先顺序返回首个记录的错误。每组结束后，`RunSubtask` 无论成功与否都把该组已经处理的数量合并到 summary；遇错立即停止后续组。
9. `RunSubtask` 标记 watcher 结束并 join，关闭原始对象存储，合并对象存储请求和 meter 流量，最后返回主体结果。框架稍后调用 `Cleanup`，从 `Option` 中取出并关闭 importer；重复 cleanup 不会再次关闭。

## 数据与状态

- 持久化输入由 `TaskMeta` 与 `ConflictResolutionStepMeta` 分担。前者提供 SQL、表定义、SQL mode、系统变量和对象存储 URI；后者的 `KVGroupConflictInfos.ConflictInfos` 把 group 名映射到 `ConflictInfo { Count, Files }`。当前处理实际依赖 `Files`；`Count` 用于元数据/日志语义，不控制循环次数。
- `ConflictResolutionStepExecutor.importer: Option<Box<dyn ConflictResolutionImporter>>` 表达 `Init`/`Cleanup` 生命周期。`RunSubtask` 在未初始化时明确报错；cleanup 使用 `take()` 保证所有权只被消费一次。
- `SubtaskSummary.Processed` 是原子计数。生产路径为每组建立局部 `AtomicI64`，deleter 经 `ResolutionProgress` 增量，组结束后再合入总 summary；`RealtimeSummary` 调用 `Update()` 生成框架可读进度。
- `ImporterConflictCodec.keyspace` 在 worker 启动后、处理 KV 前由 `ConfigureKeyspace` 设置。`StripKeyspacePrefix` 委托 `decodeConflictKey`，确保 tablecodec 解码面对的是逻辑 TiDB key。
- `visible_column_offsets` 在 codec 构造时从 `TableInfo.Columns` 过滤隐藏列得到。`DecodeRow` 仍先按物理布局解码完整 row，再投影可见列给 importer encoder，避免隐藏函数索引列导致后续可见列错位或 NULL 丢失。
- `ConflictObjectStorage.access` 可选；生产 `RunSubtask` 总会传入新的统计对象，低层测试可以不记录。GET/PUT 在成功后累计请求和字节；删除/列表没有在本适配器中增加对应计数。

## 依赖与调用关系

上游调用链的直接证据是：

- `pkg/dxf/importinto/lib.rs` 声明并再导出 `conflict_resolution`。
- `pkg/dxf/importinto/task_executor.rs::GetConflictResolutionStepExecutor` 从持久化 task 分发到 `NewConflictResolutionStepExecutor`；`GetImportStepExecutor` 把 conflict-resolution 与其余 IMPORT INTO 阶段一起纳入 step dispatch。
- `pkg/dxf/importinto/task_executor.rs::ConflictNodeStepExecutor::new` 构造本执行器，并用框架资源的 required slots 设置 CPU；`ImportConflictExtension::GetStepExecutor` 只拦截 conflict-resolution，其余步骤继续交给 `OtherSteps`。

主要下游依赖是：

- `crate::collect_conflicts::{getKVGroupIndexInfo, conflictWorkerForPair, sendConflictPair, decodeConflictKey}`：识别 data/index group、选择 worker、可取消发送及 keyspace 解码。
- `astersql_dxf_importinto_conflictedkv::{NewDeleter, ConflictStore, ConflictRowCodec}`：负责快照/事务层的实际冲突行删除；本文件负责供给 codec、通道、store 和进度/流量回调。
- `astersql_ingestor_globalsort::reader::ReadKVFilesAsync`：从 `ConflictInfo.Files` 读取排序 KV；`CancellationToken` 在取消或读取/分发错误时终止读取。
- `astersql_executor_importer` 与 `astersql_lightning_backend_kv`：从表元数据创建 encoder/decoder并在 canonical datum 与 backend datum 之间转换。
- `astersql_objstore`、`astersql_objstore_recording`：打开、读写、关闭对象存储并收集请求/流量。
- `astersql_tablecodec`、`astersql_kv`、`astersql_meta_model`：解码/编码 table key、保留各种 handle 身份以及读取表结构。

RustCodeGraph `status` 显示索引覆盖本文件，`files --filter pkg/dxf/importinto` 能找到源、Go 对照和独立测试；文件节点报告目标被 `task_executor.rs`、`collect_conflicts.rs`、独立测试等使用。对重名 Go 风格方法直接运行 `callers/callees` 未产生可用输出，因此具体调用边以上述文件限定源码节点为准，没有把模糊图结果当成事实。

## 错误处理与边界

- JSON 语法错误、外部文件读取错误、非法 `Count`、非数组 `Files` 或非字符串文件名都会转换为 `SharedError`；缺失 `infos/conflict-infos`、`Count` 或 `Files` 则分别形成空组集合、0 或空列表。
- `Init` 对已取消 context、缺少表信息、无法构表/解析 SQL/初始化 data store/创建 importer 等错误直接返回；`RunSubtask` 在 importer 缺失时返回 `conflict resolution is not initialized`。
- codec 构造中若 table definition 或 decoder 创建失败，会主动关闭传入 encoder 后返回错误；`createEncoders` 同样在部分成功后关闭已有 encoder。
- 行 key 解码错误、过短索引 key、索引值不含 handle、公共 handle datum 类型不支持等都会返回字符串错误。`DecodeIndexHandle` 明确要求 key 长度至少为 `prefixLen + idLen`。
- 读取循环会检测 `ConflictContext` 取消；reader、worker 选择或通道发送失败时取消 reader、关闭所有 sender 并等待 worker。scoped worker panic 被转换为 `conflict deleter worker panicked`。
- worker 初始化失败时不会启动 SST reader，但仍关闭 sender 并 join 所有 worker；最终优先返回初始化错误。
- 当前生产 `RunSubtask` 把负 CPU capacity 截为 0，缺失资源也得到 0；`ResolveConflictGroupFromMeta` 在并发度为 0 时不会创建 deleter，读取到的 pair 也因 sender 为空而被跳过并可能返回成功。这是调用方必须保证 CPU capacity 至少为 1 的重要前置条件/扩展风险，源码没有在该入口内兜底。较低层 `RunGroups` 会把并发度提升到至少 1，但生产路径没有经过它。
- `TaskMetaModified` 和 `ResourceModified` 当前明确返回 `not implemented`，不能宣称支持运行中热更新任务元数据或资源。

## 并发与资源生命周期

组之间串行，组内并行。`ResolveConflictGroupFromMeta` 使用 `std::thread::scope`，因此 worker 不会逃出函数生命周期；每个 worker 独占 codec/encoder 和 receiver，公共表元数据、cluster store、datum converter、collector/traffic recorder通过 `Arc` 共享。ready channel 是初始化屏障：所有 encoder 构造完毕后才开始读取，这与 Go 为规避 generated-column AST 重写数据竞争而预初始化 encoder 的意图一致，但 Rust 生产路径选择“各 worker 内构造 + 屏障”，避免移动非 `Send` 的共享解析/session 状态。

每个 worker 的输入使用容量为 `BufferedHandleLimit.max(1)` 的 `sync_channel`，对 reader 施加背压。对普通 KV 可按轮转索引分配；涉及多值索引时，`conflictWorkerForPair` 可依据 handle 选择 worker，以让相关冲突进入一致处理分区。退出时先 drop sender，再 join worker，保证 receiver 能正常结束。读取错误优先于 worker 错误返回，但所有 worker 都会被回收。

框架取消由单独 watcher 线程桥接到 `ConflictContext` 与对象存储 context；主体完成后通过 `AtomicBool` 通知 watcher，随后 join。`RunSubtask` 无论主体结果如何都会关闭 `raw_store` 并合并统计。codec 的 encoder 由 deleter/handler 生命周期最终调用 `Close`；框架 importer 则由 `Cleanup` 关闭。节点适配器还在显式 cleanup 或 `Drop` 时发送停止命令并 join 专用执行线程。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/importinto/conflict_resolution.go`，独立 Go 回归为 `pkg/dxf/importinto/conflict_resolution_test.go`。

- 两版都有 step executor 的 `Init`、`RunSubtask`、`Cleanup`、summary/reset 和 collector processed 计数；都从 cloud storage 读冲突文件，按 KV group 串行、组内并行调用 `conflictedkv.NewDeleter`。
- Go 的 `RunSubtask` 用 `json.Unmarshal` 后在 `ExternalPath` 非空时调用 `ReadJSONFromExternalStorage`；Rust 把兼容解析集中到 `ReadConflictResolutionMeta`，显式重建当前需要的 `Count`/`Files` 映射。
- Go 在主 goroutine 先用 `createEncoders` 创建全部 encoder，然后启动 error-group worker。Rust保留 `createEncoders` 和低层 `ResolveConflictGroup` 以表达/测试该模式，但框架生产路径使用 `ResolveConflictGroupFromMeta`，在各 scoped worker 内创建 encoder，并用 ready 屏障保证“读取前全部初始化”的同一不变量。
- Go 通过 context/error group 传播取消与 panic recovery；Rust使用 `CancellationToken`、`ConflictContext`、scoped thread join 和 watcher 完成等价的读取/worker收尾，但错误是字符串化后包装为 `SharedError`/`anyhow::Error`，错误类型身份不保留。
- Go 直接依赖 TiDB session/store/logger 构造 importer；Rust通过 `ConflictResolutionRuntime`/`ConflictResolutionImporter` 隔离宿主集成，并由 `ConfiguredConflictResolutionRuntime` 提供生产桥接，便于独立测试真实执行生命周期。
- Go 测试制造 3 行 data/index 冲突并验证只剩 4、5 两行。Rust 独立测试不仅保留该意图，还覆盖内联/外部 meta、reader 错误与取消、真实 data/unique-index 删除、完整 framework/node 生命周期、分区与公共 handle 保真，以及隐藏函数索引列之后的可见列重编码。
- Go 包含 `forceHandleConflictsBySingleThread` 与 `afterResolveOneKVGroup` failpoint；本 Rust 文件没有对应 failpoint。新增依赖这些注入点的调试/重试行为时不能假定已经移植。

## 扩展指南

- 新增/变更子任务 wire 字段：先同步 `pkg/dxf/importinto/proto.rs`，再扩展 `ReadConflictResolutionMeta` 的内联与外部两条路径，并在 `conflict_resolution_test.rs::resolution_meta_loads_inline_and_external_conflict_infos` 和 wire-format 测试中覆盖缺失、非法类型和兼容旧字段。Go 格式变化还应同步核对 `proto.go` 与 `conflict_resolution.go`。
- 改变 group 调度或并发度：主要入口是 `ConflictResolutionStepExecutor::RunSubtask` 和 `ResolveConflictGroupFromMeta`。必须保留 encoder 初始化屏障、同一冲突 handle 的 worker 归属、通道关闭、全部 join、取消传播和已处理计数；应增加并发为 0/1/多 worker、初始化部分失败和 worker panic 的独立测试。
- 扩展 key/row 类型：修改 `ImporterConflictCodec` 和 `importerHandle`，同步验证 int/common/partition handle、index value 无 handle、keyspace prefix、NULL、隐藏生成列和 datum 类型转换。最接近的测试是 `importer_codec_reencodes_a_real_table_row`、`importer_preserves_partition_and_common_handle_identity`、`data_handler_reencodes_visible_columns_after_functional_index`。
- 更换对象存储或计量：修改 `ConflictObjectStorage` 与 `RunSubtask` 的 close/merge 顺序，确保失败路径仍关闭存储且只合并一次；扩展 `framework_conflict_resolution_lifecycle_deletes_rows_and_closes_resources` 检查资源与统计。
- 增强 runtime 热更新：`TaskMetaModified`/`ResourceModified` 目前不实现；若支持，需定义 importer、正在运行 worker、下一 group 和 framework resource 之间的一致性边界，不能只把错误改成成功。
- 修改删除语义应优先进入 `pkg/dxf/importinto/conflictedkv`，本文件只负责输入、codec 和调度。测试仍应放在独立 `conflict_resolution_test.rs`（或 conflictedkv 对应独立测试）中，不能内嵌回生产文件。
- 性能风险集中在 worker 数、每 worker 一个 encoder、同步通道容量、对象存储读取方式和 group 串行；正确性风险集中在 handle/keyspace、函数索引隐藏列投影、错误优先级以及资源取消/关闭顺序。

## 验证依据

- RustCodeGraph：`status`（索引 11,467 个文件，目标在索引内）、`files --filter pkg/dxf/importinto`、目标文件 `node --file ... --offset/--limit` 全量 1–1035 行；另读取 `lib.rs`、`task_executor.rs` 的分发/节点适配段和 `conflict_resolution_test.rs` 的相关源码节点。对精确符号执行的 `callers/callees`/限定 `explore` 未返回可用细边，故未据此扩张结论。
- 生产源码：`pkg/dxf/importinto/conflict_resolution.rs`；模块入口：`pkg/dxf/importinto/lib.rs`；直接框架接线：`pkg/dxf/importinto/task_executor.rs`。
- crate 边界：`pkg/dxf/importinto/Cargo.toml`，确认包名、lib 入口、`nextgen` feature 和本文件所用内部依赖。
- Go 对照：`pkg/dxf/importinto/conflict_resolution.go`；Go 回归：`pkg/dxf/importinto/conflict_resolution_test.go::TestConflictResolutionStepExecutor`。
- Rust 独立测试：`pkg/dxf/importinto/conflict_resolution_test.rs`，重点包括 `conflict_resolution_reads_files_and_deletes_conflicted_rows`、`conflict_resolution_propagates_reader_error_and_cancellation`、`worker_local_importer_codecs_delete_three_real_conflicted_rows`、`unique_index_conflict_group_deletes_three_real_rows`、`resolution_meta_loads_inline_and_external_conflict_infos`、`framework_conflict_resolution_lifecycle_deletes_rows_and_closes_resources`、`importer_preserves_partition_and_common_handle_identity`、`data_handler_reencodes_visible_columns_after_functional_index`。
- 本任务是纯文档分析，按计划不运行 Cargo；验收依赖固定 11 章节的结构检查和人工事实复核。
