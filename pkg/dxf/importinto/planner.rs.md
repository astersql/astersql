# `pkg/dxf/importinto/planner.rs`

## 文件定位

本文件是 `astersql-dxf-importinto` crate 的 IMPORT INTO 分布式规划层。模块由 `pkg/dxf/importinto/lib.rs` 以 `pub mod planner` 暴露；上游在 `pkg/dxf/importinto/job.rs` 中构造 `LogicalPlan` 并以 `ToTaskMeta` 持久化任务元数据，在 `pkg/dxf/importinto/scheduler.rs:887-905` 中按下一任务步骤构造 `PlanCtx`、用 `FromTaskMeta` 恢复逻辑计划、调用 `ToPhysicalPlan`，再将对应 `PipelineSpec` 序列化成子任务 meta。

它不负责执行导入或调度状态机，而负责把作业级输入转换成当前步骤的处理器清单和线格式元数据。覆盖的步骤是 import/encode-and-sort、merge-sort、write-and-ingest、collect-conflicts、conflict-resolution 与 post-process（`LogicalPlan::ToPhysicalPlan`）。`pkg/dxf/importinto/Cargo.toml` 声明 crate 名为 `astersql-dxf-importinto`，`nextgen` feature 透传给 `astersql-config-kerneltype/nextgen`；本文件直接依赖 DXF proto/storage、importer、global sort/simple SST、KV、对象存储、table、auto-ID、MySQL 类型及 checksum 模块。

## 核心职责

1. 保存并往返序列化 IMPORT INTO 的作业级规划状态：`LogicalPlan::{ToTaskMeta, FromTaskMeta, GetTaskExtraParams}`。
2. 根据 `PlanCtx::NextTaskStep` 只生成当前下一步骤的物理计划，而不是一次生成整个 DAG（`LogicalPlan::ToPhysicalPlan`）。
3. 把各步骤的 typed meta 包装成 `PipelineSpec`，由 `PhysicalPlan::ToSubtaskMetas` 按步骤筛选并序列化。
4. 在全局排序模式下，把体积较大的字段写到对象存储，仅在子任务 envelope 中保留 `ExternalPath`（`write_external_plan_meta`、`previous_value`）。
5. 从 chunk map 或 prepare 产物生成 import 子任务，聚合 encode/merge/write 结果，决定 merge 是否可跳过，并按 KV range 拆分 ingest 子任务（`generateImportSpecs`、`generateMergeSortSpecs`、`generateWriteIngestSpecs`）。
6. 聚合数据/索引 KV 冲突与后处理 checksum、allocator 最大值（`collectConflictInfos`、`PostProcessSpec::ToSubtaskMeta`）。

## 主要符号

- `PlanCtx`：一次规划调用所需的运行时上下文。标量字段包含任务 ID、线程/节点数、下一步骤、region split 参数和每核内存；`PreviousSubtaskMetas` 保存真实 wire bytes，`PreviousImportMetas` 等 typed 列表是测试或适配期回退；`ObjectStore`/`KVStore`/`Table` 及 importer 服务允许调度层注入真实资源。源码注释明确它独立于尚未完成的 framework planner wire adapter。
- `PipelineSpec`：`Send + Sync` trait，要求 `ToSubtaskMeta`，并通过 `as_any`/`as_any_mut` 支持按具体规格 downcast。`ImportSpec`、`MergeSortSpec`、`WriteIngestSpec`、`CollectConflictsSpec`、`ConflictResolutionSpec` 和 `PostProcessSpec` 实现该 trait。
- `PhysicalPlan` / `ProcessorSpec`：有序处理器集合。`AddProcessor` 追加节点；`ToSubtaskMetas` 只序列化 `Step` 匹配的节点。非 post-process 处理器的输出链接指向 `processor_count` 这一汇聚占位；post-process 没有输出链接，并声明五个 `TypeLonglong` 加一个 `TypeJSON` 的输入列型。
- `LogicalPlan`：保存 `JobID`、importer `Plan`、原 SQL、可调度实例、chunk map、prepare 模式/外部路径、logger 和下一步骤摘要。`summary` 是内部状态，由生成函数更新，供 scheduler 更新任务摘要。
- `SortStoreLease`：对象存储引用的 RAII 包装。复用 `PlanCtx::ObjectStore` 时 `owned=false`，析构不关闭；按 CloudStorageURI 新建时 `owned=true`，`Drop` 调用 `Close`。
- `GlobalSortStoreAdapter`：把 objstore 的读、写、批量删除、按前缀遍历接口适配为 `globalsort::Storage`，供 range splitter 使用。
- `totalConflicts`：对所有 KV group 的 `ConflictInfo::Count` 做饱和求和，无法转成 `i64` 或总和溢出时钳制为 `i64::MAX`。

文件没有条件编译项。`impl_meta_spec!` 只消除四类“持有 typed meta 并 Marshal”的重复实现，不改变步骤语义。

## 执行流程

1. 提交阶段：`job.rs::doSubmitTask` 校验表信息并组装 `LogicalPlan`；`StorageTaskSubmissionService::submit_dxf_task` 调用 `ToTaskMeta` 和 `GetTaskExtraParams`，把任务交给 DXF 存储层。
2. 调度阶段：`scheduler.rs` 根据 `next_step` 拉取必要的前序步骤 meta，补齐 `PlanCtx` 的任务 ID、全局排序标志、线程/节点数及 controller/importer 服务，然后恢复 `LogicalPlan`。
3. 步骤分派：`LogicalPlan::ToPhysicalPlan` 按 `NextTaskStep` 调用对应 `generate*Specs`；未知或完成步骤生成空计划。除 post-process 外，全局排序模式随后调用 `write_external_plan_meta`。
4. Import/encode：`generateImportSpecs` 按优先级选择 chunk 来源：`PreparedChunkMapExternalPath`、内联 `ChunkMap`、最后用表元数据和 SQL 参数创建 `LoadDataController` 并 `PopulateChunks`。engine ID 排序后跳过 `IndexEngineID`，每个其余 engine 生成一个 `ImportSpec`，同时更新行数上界和总文件字节数。
5. Merge-sort：`encoded_kv_metas` 合并所有 import meta 的 data/index `SortedKVMeta`；空组跳过。除 `ForceMergeStep` 或 `ForceMergeGroup` 强制外，`skip_merge_sort` 用区间端点扫描计算最大重叠权重，并与并发度调整后的阈值比较。需要归并的文件再由 `DivideMergeSortDataFiles` 按节点数和线程数分组。
6. Write-and-ingest：`ingest_kv_metas` 合并 merge 结果，并补入判定为无需 merge 的 encode 结果；同一 group 同时出现于两侧会报错。`generateWriteIngestSpecs` 取得注入的 `CommitTS`，否则向 KV store 查询版本；随后 `split_for_one_subtask` 读取 stat 文件、计算 range/region 阈值并循环产生连续的 `WriteIngestSpec`。
7. 冲突步骤：`collectConflictInfos` 从 encode、merge、write 三类前序 meta 中只加载 `RecordedConflictKVCount > 0` 的记录，并按 data/index group 聚合。无冲突时两个生成函数都返回空列表；有冲突时各生成一个 spec，且摘要行数改为冲突 KV 总数。
8. Post-process：`PostProcessSpec::ToSubtaskMeta` 根据是否全局排序选择 encode 步骤，汇总 import checksum 和各 allocator 的正最大值，再叠加 collect-conflicts 的删除行 checksum 与“索引冲突过多”标志，输出与 Go 字段名兼容的 JSON。

## 数据与状态

- 持久状态边界是 `TaskMeta`：`ToTaskMeta` 写入 job、plan、SQL、实例、chunk map 和 prepare 外部路径；`FromTaskMeta` 恢复这些字段。`summary`、logger 和 `PrepareMode` 不由 `FromTaskMeta` 从 `TaskMeta` 恢复；prepare 模式通过 task extra params 传给框架。
- 前序结果有两种输入形态：生产接线优先读取 `PreviousSubtaskMetas` 的 wire bytes；对应步骤不存在时才使用 `Previous*Metas` typed 回退。全局排序 envelope 若带 `ExternalPath`，`previous_value` 会读取外部 JSON 并合并回 envelope 后再反序列化。
- 外置元数据按 spec 类型剥离不同大字段：import 的 chunks/sorted meta，merge 的文件、范围、统计与冲突信息，write 的 sorted meta/文件/range keys，冲突步骤的 infos。路径由 `PlanMetaPath(TaskID, Step2Str(...), index+1)` 生成。
- `LogicalPlan::summary` 的含义随步骤变化：import 记录总源文件字节和最大 `RowIDMax`；merge/ingest 累加 KV 字节，data group 还累加 KV 数；冲突步骤的 `RowCnt` 是冲突 KV 对数量。
- ingest range 保持 `[start, end)` 式的连续推进：每个子任务把当前 start 和本组 end 同时加入 job keys 与 region split keys，下一组从前一 end 开始；`start >= end` 被视为非法。

## 依赖与调用关系

上游主链为：

`job.rs::doSubmitTask` → `LogicalPlan::ToTaskMeta` → DXF task storage → `scheduler.rs` 恢复 `LogicalPlan` → `LogicalPlan::ToPhysicalPlan` → `PhysicalPlan::ToSubtaskMetas`。

主要下游关系为：

- `crate::proto::*` 提供各步骤 meta、`TaskMeta`、`SortedKVMeta`、冲突信息与 Marshal/Unmarshal。
- `astersql-executor-importer` 提供作业 `Plan`、chunk、controller、SQL 参数解析和表导入服务。
- `astersql-ingestor-globalsort` 提供计划外部路径、文件分组、range splitter、range 大小计算和 storage trait。
- `astersql-ingestor-simplesst` 提供 merge overlap 阈值与 stat 文件属性解码。
- `astersql-objstore` 同时承载 prepare meta、外置 plan meta 和全局排序文件；`PlanCtx::ObjectStore` 可避免重复打开/关闭共享存储。
- `astersql-kv` 提供未注入 commit TS 时的当前版本；`astersql-dxf-framework-storage` 提供节点资源回退；`astersql-config-kerneltype` 选择 classic/NextGen 默认 region 参数。
- `astersql-table` 在未注入 table 时从元数据构建表；controller 所需服务必须由 scheduler 在 chunk map 与外部 prepare 路径都为空时注入（`scheduler.rs:898-901`）。

RustCodeGraph 对目标文件报告其被包括 `pkg/dxf/importinto/job.rs`、`scheduler.rs`、`conflict_resolution.rs` 及测试在内的调用/使用面覆盖；精确生产入口又由上述源码接线核对。`pkg/dxf/importinto/lib.rs` 将 planner 的公开项再导出到 crate 根。

## 错误处理与边界

- 所有可失败规划 API 返回 `errors::SharedError`；serde、objstore、global sort、table/importer 错误大多转换为带原错误文本的 `errors::New`，KV `CurrentVersion` 错误直接传播。
- 动态生成 chunk 时缺少表信息、table factory、controller services 或 importer service 都立即失败；controller 无论初始化/填充分块成功与否都会在闭包后显式 `Close`。
- 读取无效 JSON、找不到外部对象或未知存储 URI会失败。冲突 envelope 的 `RecordedConflictKVCount == 0` 时不会读取其外部路径，避免无冲突记录触发无用 I/O。
- merge 输入为空不会生成任务；encode 与 merge 对同一 KV group 同时供 ingest 使用属于不变量破坏并报错。
- range split 缺少 stat 文件、节点资源不可用、CPU 非正数或 `start >= end` 均报错。`RegionSplitSizeKeys` 探测失败会回退到 `PlanCtx` 参数，再与内核默认值取较大值。
- post-process 对 collect meta 缺字段采用默认值，但非法 JSON、checksum/meta 反序列化失败仍向上传播；非正 allocator 最大值被忽略。
- 未识别的 `NextTaskStep` 当前返回空 `PhysicalPlan`，而不是错误；扩展步骤时必须显式补充分派，避免静默无子任务。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道；并发参数只用于规划粒度：`ThreadCnt` 影响 merge 跳过阈值及文件分组，`ExecuteNodesCnt` 影响 controller 分块和 merge 文件划分。涉及共享运行时对象的 trait object 均要求适当的 `Send + Sync`，`PipelineSpec` 本身也要求 `Send + Sync`。

对象存储遵循所有权区分：注入的共享 store 只 clone 引用且不关闭，自行按 URI 打开的 store 由 `SortStoreLease::drop` 关闭。range splitter 在成功创建后，无论循环结果成功或失败，都会执行一次 `Close`；当前代码忽略其关闭错误，与 Go 版本仅记录警告的非主路径语义相近。controller 在 `PopulateChunks` 之后显式关闭。`Arc` 持有 KV store、table 和 importer 服务，规划函数只借用或 clone 引用，不转移调度层的共享资源所有权。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/dxf/importinto/planner.go`，独立测试为 Rust 的 `planner_test.rs` 与 Go 的 `planner_test.go`。核心步骤、meta 类型、merge 跳过规则、range 拆分、冲突聚合、post-process checksum 和饱和冲突计数均沿用 Go 语义。

Rust 为适应当前迁移状态做了几项显式接线：

- Rust 在本文件定义本地 `PlanCtx`、`PhysicalPlan` 和 `PipelineSpec`，而 Go 使用 `pkg/dxf/framework/planner` 接口；文件头注释说明这是 framework wire adapter 未完成期间保留 typed meta 步骤边界的做法。实际 scheduler 已直接调用这些 Rust 类型。
- Go 每次通过 `importer.GetSortStore` 打开并 defer 关闭；Rust 支持从 `PlanCtx` 注入共享 `ObjectStore`，否则由 `SortStoreLease` 自行打开和关闭。
- Go 的 failpoint `forceMergeSort` 在 Rust 中由可注入的 `ForceMergeGroup` 表达；Go `mockWriteIngestSpecs` 测试 failpoint 没有出现在本文件的 Rust 生产逻辑中。
- Rust 可由 `CommitTS`、region 参数、每核内存及服务直接注入以形成确定性测试；没有注入时再走 KV、节点资源与 importer service 的生产路径。
- Go post-process 仅消费 wire meta；Rust 还保留 typed previous metas 回退。生产 scheduler 填充的是 `PreviousSubtaskMetas`，因此 wire 路径仍是主路径。
- Rust 把外部字段从 JSON envelope 中剥离后写对象存储，再设置 `BaseExternalMeta.ExternalPath`；读取端重新合并。结果保持 Go 的“轻 envelope + 外部大字段”协议意图，但实现机制是 Rust 侧显式 JSON 字段表。

## 扩展指南

- 新增任务步骤：在 `dxfproto`/`proto.rs` 定义步骤和 meta 后，实现独立 `PipelineSpec`，补充 `LogicalPlan::ToPhysicalPlan` 分派、scheduler 前序 meta 拉取、外置字段列表与独立的 `planner_test.rs` 测试；还需核对 Go `planner.go` 的对应增量。
- 修改 import 分块来源：优先改 `generateImportSpecs`，保持“外部 prepare 路径优先于内联 map，内联优先于 controller”不变量，并同步覆盖 controller 关闭、索引 engine 跳过和 summary 更新。
- 修改 merge 判定：集中在 `skip_merge_sort`、`encoded_kv_metas` 与 `ingest_kv_metas`，必须同时验证强制单 group/全部 group、低/高重叠以及 encode/merge 重复 group 错误。
- 修改 ingest 拆分：集中在 `split_for_one_subtask` 与 `GlobalSortStoreAdapter`。保持 range 单调、边界 keys 首尾完整、commit TS 透传、classic/NextGen 默认值和 splitter 关闭；性能风险主要来自 stat 文件逐个读取和子任务数量变化。
- 修改 wire 字段或外置策略：同步 `write_external_plan_meta`、`set_spec_external_path`、`previous_value`、`proto.rs` Marshal/Unmarshal 与 Go JSON 字段名；任何字段名漂移都会导致跨语言或跨步骤不兼容。
- 修改冲突/post-process 聚合：同步 `collectConflictInfos`、两个冲突 spec 生成器和 `PostProcessSpec::ToSubtaskMeta`，特别注意 `RecordedConflictKVCount` I/O 短路、data/index group 合并和 `i64::MAX` 饱和规则。
- 测试必须继续放在独立的 `pkg/dxf/importinto/planner_test.rs`，不要内嵌回生产文件。优先扩展现有真实 memstore、wire bytes、range split、controller bridge 与冲突聚合测试。

兼容性风险集中于 Go/Rust JSON 字段、步骤号和 external path 协议；正确性风险集中于漏消费前序 meta、range 重叠/缺口和错误跳过 merge；性能风险集中于对象存储 I/O、merge 分组与 range 子任务数量。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件、4,415 个 Go 文件；`files --filter pkg/dxf/importinto` 确认目标、Go 对照、独立测试与模块入口均被索引。
- RustCodeGraph 源码节点：完整读取 `pkg/dxf/importinto/planner.rs:1-1214`；查询确认 Rust/Go 两个 `LogicalPlan`、两个 `generateImportSpecs`、两个 `collectConflictInfos`，以及 Rust `split_for_one_subtask`。
- 调用证据：RustCodeGraph 初始 explore 报告 `Plan`/`LogicalPlan` 在 `job.rs`、`scheduler.rs`、`task_executor.rs` 和相关测试中的使用面；进一步核对 `job.rs:161-196, 391-437` 与 `scheduler.rs:860-939`，确认提交、恢复、逐步骤规划及 meta 生成主链。
- crate/模块证据：读取 `pkg/dxf/importinto/Cargo.toml` 和 `pkg/dxf/importinto/lib.rs`，确认 crate、feature、直接依赖、planner 再导出及独立测试模块声明。本目录不存在 `doc.go`；未把 `job_doc.go` 当作包级契约替代品。
- Go 对照：完整读取 `pkg/dxf/importinto/planner.go:1-892`，核对逻辑计划往返、各步骤生成、外置 meta、range split、冲突聚合及饱和计数。
- 测试证据：读取 `pkg/dxf/importinto/planner_test.rs`，实际 Rust 测试覆盖非法 JSON、merge overlap/强制 merge、存储打开错误、prepare 路径优先、外置 chunks、真实 stat 文件拆分与 commit TS、Go wire bytes、逻辑计划往返、post-process 列型、controller bridge、外部冲突信息、冲突聚合、合并/未合并 group 共存和饱和计数；`pkg/dxf/importinto/planner_test.go` 是 Go 对照测试面。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试。交付结构检查要求本文恰好包含上述十一个固定二级标题。
