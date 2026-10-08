# `pkg/session/runtime/modify_column_cloud_planner.rs` 逻辑说明

## 文件定位

`modify_column_cloud_planner.rs` 属于 `astersql-session` crate（`pkg/session/Cargo.toml`），但不是 `pkg/session/runtime.rs` 的直接子模块：`pkg/session/runtime/modify_column_dist_backfill.rs` 通过 `#[path = "modify_column_cloud_planner.rs"] mod cloud_planner;` 将它纳入分布式 MODIFY COLUMN 回填调度器。文件只扩展父模块的私有 `Planner`，没有对 crate 外公开的 API，也没有条件编译项。

它负责云存储/global-sort 回填的后两个计划阶段：根据 `ReadIndex` 输出生成 `MergeSort` 子任务，再根据 `MergeSort` 或被跳过时的 `ReadIndex` 输出生成 `WriteAndIngest` 子任务。普通读索引范围和临时索引合并计划仍由父文件的 `Planner::ranges` 处理。

## 核心职责

- `groups` 从 DXF `TaskHandle` 读取指定前置步骤的持久化子任务 Meta，通过 `wire::read` 恢复外置字段，按索引分组合并 `SortedMeta`，并保留第一个子任务的 `ele_ids`。
- `Planner::cloud_plans` 在 `BackfillStepMergeSort` 中评估文件重叠度；全部分组均低于调整后阈值时返回空计划，否则按节点数和 runtime slots 分配 merge-sort 数据文件。
- 在 `BackfillStepWriteAndIngest` 中优先消费 MergeSort Meta；若合并阶段没有子任务，则回退到 ReadIndex Meta。它根据活跃执行实例、DXF 资源和 Region 分裂配置切分 key range，为每段生成导入 Meta。
- 所有计划的大字段都由 `wire::write` 写入 `CloudStore`，调度器返回的是持久指针 JSON。WriteAndIngest 成功计划后还会把合并后的 KV 总字节数写入任务 `summary.index_kv_size`。

## 主要符号

- `fn groups(handle, store, task, steps) -> Result<(Vec<wire::SortedMeta>, Vec<i64>), String>` 是文件内私有聚合器。它按 `steps` 顺序查找第一个有 Meta 的步骤，一旦找到就不再读后续候选。第一个 Meta 决定分组数和 `ele_ids`，后续 Meta 按位置合并。旧版 Meta 的 `meta_groups` 为空时，它将扁平 `legacy` 字段推入唯一分组。
- `pub(super) fn Planner::cloud_plans(&self, handle, task, node_count, next_step) -> Result<Vec<Vec<u8>>, String>` 是父模块可见的唯一入口。它只接受 `BackfillStepMergeSort` 和 `BackfillStepWriteAndIngest`，其他值返回 `unknown cloud plan step` 错误。
- `publish` 是 `cloud_plans` 内部闭包：为每个计划元素建立只含 `ts` 的行内 JSON，将 `ExternalFields` 写入 `<task-id>/<step>/<ordinal>` 样式的 `sort::util::PlanMetaPath`，再把指针字节加入返回值。
- 本文件不定义新 struct/enum/trait/常量；`Planner` 、`TaskMeta`、`TaskSummary`、`scheduler`、`storage`、`Arc` 和 `AtomicBool` 都通过父模块的 `use super::*` 进入作用域。

## 执行流程

1. `Planner::on_next_subtasks_batch` 根据 NextGen 模式选择 `task.base.max_node_count.max(1)` 或当前节点数；当 `next_step` 是 MergeSort/WriteAndIngest 时调用 `cloud_plans`。
2. `cloud_plans` 先以 `cloud_storage_uri` 和独立的未取消标记打开 `CloudStore`，再建立封装 `wire::write` 的 `publish` 闭包。
3. MergeSort 分支仅读 ReadIndex 的完成 Meta。每个 `SortedMeta.files` 被转换为 `simple::MultipleFilesStat`，其 key 边界先经 base64 解码。若 failpoint `github.com/pingcap/tidb/pkg/ddl/forceMergeSort` 未启用，且每组最大重叠总数均不超过 runtime slots 对应阈值，立即返回空列表表示跳过持久化合并子任务。
4. 不可跳过时，每个索引分组抽取 file pair 的第一个路径（data file），交给 `DivideMergeSortDataFiles(files, node_count, max(slots, 1))`。每个分片发布一份 Meta，并在存在对应 ID 时写入单元素 `ele_ids`。
5. WriteAndIngest 分支依次尝试 MergeSort、ReadIndex Meta，对合并后各组 `size` 使用 `wrapping_add` 计算总量。真实运行时通过 `GetAllServerInfo` 获取活跃实例；测试 mock 路径使用 `node_manager.get_nodes()`。零实例、缺失节点资源或非正 CPU 均直接失败。
6. 默认 Region 分裂配置在 NextGen 为 1 GiB/102,400,000 keys，其他模式为 96 MiB/960,000 keys；若存储返回更大值则取两者最大值，查询失败只输出警告并继续使用已有默认值。`CalRangeSize`以每 CPU 内存计算 range 阈值。
7. 每个非空 key 范围分配一个非零导入 TS，构造 `NewRangeSplitter`。循环调用 `SplitOneRangesGroup`，为 job keys 和 region split keys 同时补入当前起点/终点，发布数据文件、统计文件、单元素 Meta 分组及可选正 `ele_id`。终组使用原分组 end key，中间组使用 splitter 返回的上界；每次都强制 `start < upper`。
8. 分组完成后显式 `splitter.Close()`。全部计划成功后才解码 `task.meta`，替换 `summary` 并重新序列化；任一早期错误都不会修改任务 Meta。

## 数据与状态

输入数据分为三层：DXF 任务提供 `task.base.id`、runtime slots、max node count 和 JSON `task.meta`；前置子任务提供行内 JSON/外置指针；`CloudStore` 保存 `ExternalFields` 中的文件列表、范围 key、排序统计、Meta 分组和索引 ID。`groups` 不修改持久状态，只构造内存合并结果。

MergeSort 输出只含待合并的 `data_files` 与对应 `ele_ids`，`ts` 固定为 0。WriteAndIngest 输出含数据/统计文件、带首尾边界的 job/split keys、导入 TS，以及描述子范围的单个 `SortedMeta`。该 Meta 的 `size` 按 `group.size / live_count` 写入，它是 Go 对照中每执行实例的估算值，不是对切分后实际文件大小的重新统计。

`result` 只在局部内存中累积，但 `publish` 会在函数返回前逐项写对象存储；如果后续项失败，本文件没有删除先前已写的计划对象。相反，`task.meta` 只在 WriteAndIngest 整体成功后更新，因此失败尝试不会提前发布 `summary`。

## 依赖与调用关系

唯一直接上游是父文件 `modify_column_dist_backfill.rs` 中 `impl scheduler::Extension for Planner` 的 `on_next_subtasks_batch`；它把本文件的 `String` 错误转成 `SchedulerError`。同一实现的 `next_step` 确定云路径 `ReadIndex -> MergeSort -> WriteAndIngest -> Done`，所以 MergeSort 可返回空子任务列表，但步骤仍会转入 WriteAndIngest，由 `groups(..., &[MERGE, READ])` 完成回退。

直接下游是：`modify_column_cloud_meta.rs` 的 `read`/`write`/`decode`/`encode`、`ExternalFields` 和 `SortedMeta::merge/sort_files`；`modify_column_cloud_store.rs` 的 `CloudStore::open`；DXF scheduler 的前置 Meta 读取、Task/TaskBase 和 NodeManager；globalsort 的计划路径、合并文件分配、range splitter 及 range-size 计算；simplesst 的重叠统计；Domain 存储接口的 `DDLRegionSplitConfig` 和 `CurrentVersion`；以及 infosync 活跃服务器列表。

`pkg/session/Cargo.toml` 明确声明 `astersql-dxf-framework-proto`、`scheduler`、`storage`、`astersql-ingestor-globalsort`、`astersql-ingestor-simplesst`、`astersql-domain-infosync`、`astersql-config-kerneltype`、`astersql-testkit-testfailpoint`、`serde_json` 和对象存储相关 crates。crate 的 `nextgen` feature 传递给 config crates，本文件不使用 `#[cfg]`，而是运行时调用 `IsNextGen()` 切换策略。

## 错误处理与边界

多数下游错误统一转为 `String` 并通过 `?` 传播，包括前置 Meta 读取、JSON 解/编码、base64 key、排序元数据合并、文件分组、range splitter、TS 分配、对象存储写入和 task Meta 序列化。`groups` 显式拒绝后续子任务比第一个子任务拥有更多 Meta 分组；但后续分组更少时 `zip` 只合并已有部分，没有单独报错。第一个子任务决定的 `ele_ids` 也不与后续 Meta 做一致性校验。

可跳过的空情况有两类：所有候选步骤都没有子任务时 `groups` 返回两个空 vector；WriteAndIngest 遇到 start/end 均为空的分组时跳过该组。与之不同，没有活跃实例、无 DXF 节点资源、`TotalCPU <= 0`、TS 为 0、`start >= upper` 或未知步骤都是硬错误。

Region 分裂配置读取是特意的最佳努力路径：存储返回错误时仅 `eprintln!`，不中止计划。总 KV 字节数使用显式 wrapping 语义，与 `SortedMeta::merge` 和 Go `uint64` 溢出语义保持一致。`splitter.Close()` 只在正常离开循环时调用；`SplitOneRangesGroup` 或 `publish` 途中错误会提前返回，清理是扩展时需特别关注的边界。

## 并发与资源生命周期

本文件不启动线程、async 任务或 channel，`cloud_plans` 在调度器调用线程中同步完成。`node_count` 是上游根据当前实例或 NextGen max node count 拍摄的视图；WriteAndIngest 又单独获取 `live_count`用于目标分组大小，两者可随集群变化而不同。

`CloudStore::open` 得到的对象在整次计划调用内共享，传入的 `Arc<AtomicBool>` 始终为 false 且本文件不更新它，因此计划器没有将 DXF 取消状态连到对象存储 I/O。它依赖 Rust 所有权在函数退出时释放 store，但 range splitter 有显式 `Close` 契约。

`runtime_slots()` 同时影响 merge-sort 跳过阈值和文件分组并发度；文件分组时最小按 1 传入，重叠阈值则直接使用 `slots`。WriteAndIngest 要求 `TotalCPU > 0`，以 `TotalMem / TotalCPU` 得到每核内存；每个 range splitter 在该索引分组完成后关闭，其间生成的对象 Meta 作为持久资源留给后续 `CloudStep`。

## 与 Go 版本的对应关系

Go 直接对照是 `pkg/ddl/backfilling_dist_scheduler.go`：`LitBackfillScheduler.OnNextSubtasksBatch` 对应 Rust 父模块入口；`generateMergeSortPlan` 对应 `MERGE` 分支；`generateGlobalSortIngestPlan`、`splitSubtaskMetaForOneKVMetaGroup`、`getRangeSplitter` 和 `forEachBackfillSubtaskMeta` 合起来对应 `INGEST` 分支与 `groups`。`pkg/ddl/backfilling_dist_executor.go` 中 `BackfillTaskMeta`/`BackfillTaskSummary`/`BackfillSubTaskMeta` 是 JSON 契约来源。

两版保留的核心语义包括：按分组位置合并 Meta；merge 可按重叠阈值跳过且可由同名 failpoint 强制；只提取 file pair 的 data-file 成员；ingest 优先消费 merge 结果、否则回退 read-index；空表分组不生成导入子任务；range job/split keys 都含首尾边界；每个非空分组分配 TS；成功后才将 `IndexKVSize` 写回 task Meta。

可见差异需如实保留：Go 的对象存储和 range splitter 使用 `context.Context` 并通过 `defer` 关闭，Rust planner 当前使用恒 false 取消标记且只在正常循环后显式关闭 splitter。Go 获取活跃 exec ID 的错误直接传播，Rust 还显式拒绝零实例。Go 在 merge/ingest 路径中对空 group 有单独报错，Rust 用已初始化的默认 `SortedMeta` 表示每个槽位。Go 有计划日志和 `mockGlobalSortIngestPlanErr`/`mockWriteIngest` 测试 failpoint，本 Rust 文件没有这两个分支。

## 扩展指南

新增云计划步骤时，必须同时更新父模块的 `next_step`、`on_next_subtasks_batch` 和对应 `CloudStep` 工厂，再在 `cloud_plans` 增加明确分支；不应让新值落入未知步骤错误，或只改计划而没有执行器。如需改变 Meta 格式，应优先扩展 `modify_column_cloud_meta.rs` 的行内/外置兼容契约，并与 Go `BackfillSubTaskMeta` 的 JSON/external 标签同步。

修改分组规则时，要先明确“第一个 Meta 决定组数和 ID”、MergeSort 空计划回退到 ReadIndex、旧版扁平 Meta 兼容三个不变量。若改为严格检查每个子任务的组数/`ele_ids`，必须评估滚动升级中旧任务 Meta 的兼容风险。若修改 range size、node count 或 per-instance size，需同时评估子任务均衡、对象数、内存和 Region 分裂放大。

测试不应写入本生产文件。直接 planner 回归建议新增同目录独立 `modify_column_cloud_planner_test.rs` 并由父模块在 `#[cfg(test)]` 下引入；至少覆盖 merge 跳过/强制、多组合并、组数不匹配、MergeSort 结果优先级、空表、零节点/无资源/零 TS、非单调范围、task Meta 失败不变与成功 summary。端到端修改还要同步 `normal_ddl_masking_policy_test.rs` 的独立测试，并核对 Go `backfilling_dist_scheduler_test.go`。

## 验证依据

本说明直接核对了目标源 `pkg/session/runtime/modify_column_cloud_planner.rs`、父模块与调用入口 `pkg/session/runtime/modify_column_dist_backfill.rs`、crate 边界 `pkg/session/Cargo.toml`、元数据独立测试 `pkg/session/runtime/modify_column_cloud_meta_test.rs`、执行器独立测试 `pkg/session/runtime/modify_column_cloud_executor_test.rs`、云 MODIFY 端到端回归 `pkg/session/runtime/normal_ddl_masking_policy_test.rs`，以及 Go 对照 `pkg/ddl/backfilling_dist_scheduler.go`、`pkg/ddl/backfilling_dist_executor.go` 和 `pkg/ddl/backfilling_dist_scheduler_test.go`。`pkg/session` 下没有 `doc.go`；仓库也没有对应的独立 `modify_column_cloud_planner_test.rs`。

RustCodeGraph `status` 显示当前索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime` 确认目标文件已索引；`node --file pkg/session/runtime/modify_column_cloud_planner.rs` 读取了完整 271 行源码，并报告文件级使用关系。精确 `query/callers/callees` 未将私有 impl 方法 `cloud_plans` 建模为可查节点，因此唯一真实调用边由 `rg "cloud_plans\\("` 和父模块源码交叉确认，不将空图结果解释为“无调用者”。

本任务只新增说明文档，按计划不运行 Cargo。交付检查是任务指定的结构命令，并人工复核上述定位、执行分支、错误/资源边界、Go 差异和扩展测试建议均可由列出的符号与文件回溯。
