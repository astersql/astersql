# `pkg/ddl/backfilling_import_cloud.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（见 `pkg/ddl/Cargo.toml` 的 `[package]` 与 `[lib] path = "lib.rs"`），由 `pkg/ddl/lib.rs` 以 `pub mod backfilling_import_cloud` 暴露。它描述分布式 DDL 全局排序路径中的“云端文件写入并摄取”阶段：上游读取/排序阶段把索引 KV 与统计信息写入对象存储，当前模块把这些文件组织成外部引擎配置并交给导入后端摄取。

需要区分 Rust 当前实现与 Go 生产主链：Go 的 `pkg/ddl/backfilling_dist_executor.go:newBackfillStepExecutor` 在 `BackfillStepWriteAndIngest` 分支直接构造 `newCloudImportExecutor`；Rust 的 `pkg/ddl/backfilling_dist_executor.rs:BackfillDistExecutor::get_step_executor` 目前只校验并返回 `BackfillStep::WriteAndIngest`，全仓库 Rust 引用中没有构造 `CloudImportExecutor` 的生产接线。因此本文件已有可测试的阶段逻辑和公共数据类型，但尚不能据此认定 Rust 完整应用主链已经调用该执行器。

## 核心职责

- `CloudImportExecutor<B>` 管理单个云端导入执行环境的初始化、子任务导入、动态限速/资源调整、统计重置和清理。
- `CloudImportBackend` 把真实 Lightning/ingest 后端能力抽象为同步接口，使本文件不依赖具体后端类型，并允许 `pkg/ddl/backfilling_import_cloud_test.rs:BackendStub` 验证控制流。
- `run_subtask` 从 `BackfillSubTaskMeta` 选择索引、合并 `SortedKvMeta`、构造 `ExternalEngineConfig`、关闭外部引擎后检查其存在性，再执行摄取并归一化错误。
- `get_index_info_and_id` 保留旧版本子任务元数据缺少元素 ID 时的 Go 兼容行为；`has_unique_index` 提供索引集合属性判断；`IngestCollector` 累加写入集群的字节数。

本模块不负责扫描表、生成/排序 KV、序列化或从对象存储解码子任务元数据，也不实现真实网络、SST 或 Region 操作；这些信息由 `BackfillSubTaskMeta` 和后端抽象输入。

## 主要符号

- `IndexInfo { id, name, unique }`：本文件所需的最小索引视图。`id` 参与引擎标识，`name` 用于重复键错误上下文，`unique` 供 `has_unique_index` 判断。
- `ImportError`：执行器的领域错误，包括未初始化、后端/外部引擎缺失、元素 ID 数量异常、引擎未启动、重复键及后端文本错误。`LocalBackendNotFound`、`IndexNotFound` 在当前文件没有构造点，属于已声明但尚未接线的错误变体。
- `BackendImportError::{DuplicateKey, Other}`：后端导入错误的最小分类；`run_subtask` 用它决定是否补充索引名。
- `CloudImportBackend`：后端契约，包含外部引擎关闭/查找/导入、资源更新、工作并发读写、限速更新和关闭。
- `ExternalEngineConfig`：传给后端的不可变导入快照，携带数据/统计文件、全局键区间、作业与 Region 切分键、文件大小、时间戳、内存和冲突策略。
- `CloudImportExecutor<B>`：公开字段保存作业、表、索引、云存储 URI、后端、摘要、限速及运行标志；私有 `initialized` 强制执行初始化协议。
- `CloudImportExecutor::{new, init, run_subtask, cleanup, reset_summary, task_meta_modified, resource_modified}`：生命周期与回调入口。
- `has_unique_index`、`get_index_info_and_id`：公开辅助函数；后者也被 `pkg/ddl/backfilling_merge_sort.rs:run_subtask` 复用。
- `IngestCollector::processed`：按 Go `uint64(bytes)` 语义累计字节，使用 wrapping 加法。

## 执行流程

1. `CloudImportExecutor::new` 保存作业和后端依赖，摘要/限速清零，`engine_running` 与 `initialized` 均为 `false`。
2. 框架应调用 `init(concurrency)`；该方法原样把并发度传给后端并将执行器标记为已初始化，包括 `0`，不引入 Rust 特有的下限。
3. `run_subtask` 首先拒绝未初始化实例，然后调用 `get_index_info_and_id(meta.element_ids, indexes)`。引擎 ID 是 `"{table_name}-{index_id}"`。
4. 它依次调用 `SortedKvMeta::merge` 合并 `meta.meta_groups`，得到整体起止键和 `total_kv_size`。`range_job_keys` 为空时使用 `range_split_keys` 兼容旧元数据，否则保留显式作业切分键。
5. 它以子任务文件、键范围、切分键、`ts` 和调用方传入的 `memory_capacity` 构造 `ExternalEngineConfig`；当前实现固定 `check_hotspot = true`、`duplicate_key_is_error = true`。
6. 后端先执行 `close_external_engine`，随后 `has_external_engine` 必须为真；否则返回 `ExternalEngineNotFound`，不进入导入。
7. 导入前置 `engine_running = true`，同步调用 `import_engine` 后立即复位为 `false`。成功直接返回；重复键映射为带可选索引名的 `ImportError::DuplicateKey`；其他错误映射为 `ImportError::Backend`。
8. 运行期间框架可调用 `resource_modified` 调整并发和内存；任务元数据变化可通过 `task_meta_modified` 更新写速限制。子任务间可 `reset_summary`，最终以 `cleanup` 关闭后端并回到未初始化状态。

## 数据与状态

`CloudImportExecutor` 有两个关键布尔状态。`initialized` 是生命周期门闩：只有 `init` 之后 `run_subtask` 才可运行，`cleanup` 会清除它。`engine_running` 表示同步 `import_engine` 调用正在进行，仅这段窗口允许实际资源更新；当前类型没有状态枚举，也不会阻止连续多次执行子任务。

`summary: SubtaskSummary` 由 `reset_summary` 清零，但当前文件的 `run_subtask` 和 `IngestCollector` 没有把导入字节合并到该字段；`cloud_storage_uri`、`job_id` 也只被保存，当前方法内未读取。这些字段对应 Go 执行器更完整的对象存储、计量与指标生命周期，不应把 Rust 字段存在解读为这些能力已接通。

`ExternalEngineConfig` 克隆文件列表与切分键，取得合并结果中的键区间所有权，因此后端收到的是该次调用的独立配置。`get_index_info_and_id` 有三种兼容分支：一个 ID 时匹配成功返回索引和 ID，匹配失败刻意返回 `(None, 0)`；没有 ID 时直接读取 `indexes[0]`，只有恰好一个索引才返回索引引用，但始终返回首个索引 ID；多个 ID 返回 `UnexpectedElementIds`。因此“无元素 ID 且索引列表为空”会 panic，这是对 Go 无条件访问首元素不变量的保留，而不是可恢复错误。

## 依赖与调用关系

直接 Rust 依赖是 `crate::backfilling::Key`、`crate::backfilling_dist_executor::BackfillSubTaskMeta`、`crate::backfilling_read_index::{SortedKvMeta, SubtaskSummary}`。`pkg/ddl/Cargo.toml` 将本文件编入 `astersql-ddl`；本文件自身只使用 crate 内类型和标准库，不直接引用 Cargo 中的 Lightning/ingestor crate，真实导入依赖被 `CloudImportBackend` 隔离。

RustCodeGraph 对 `run_subtask` 给出的关键下游边为 `get_index_info_and_id`、`SortedKvMeta`/`ExternalEngineConfig` 构造及 `CloudImportBackend::{close_external_engine, has_external_engine, import_engine}`；对 `resource_modified` 给出的边为 `worker_concurrency`、`update_engine_resource`、`set_worker_concurrency`。图中 `get_index_info_and_id` 还被 `pkg/ddl/backfilling_merge_sort.rs:run_subtask` 调用。源码搜索确认 `IndexInfo` 也被 `backfilling_merge_temp.rs` 使用。

上游方面，`pkg/ddl/lib.rs` 公开模块并在 `cfg(test)` 下挂载独立测试；当前 Rust 生产文件没有 `CloudImportExecutor::new` 调用者。完整意图可由 Go 的 `newBackfillStepExecutor -> newCloudImportExecutor -> Init/RunSubtask` 链核对，但这条 Go 链不能冒充 Rust 调用边。

## 错误处理与边界

- 初始化前调用 `run_subtask` 返回 `NotInitialized`；后端关闭外部引擎失败和普通导入失败保留文本并包装为 `Backend`。
- 外部引擎关闭成功后仍显式检查是否可取到引擎，失败返回带引擎 ID 的 `ExternalEngineNotFound`。
- 重复键只在 `BackendImportError::DuplicateKey` 分支特殊处理；能定位索引时带 `Some(index.name)`，旧元数据或未知单 ID 可为 `None`。
- `resource_modified` 先比较并发度；相同即成功，即使当前无运行引擎。并发变化而 `engine_running == false` 时返回 `EngineNotStarted`，意图是让上层重试。后端资源更新失败时不会改写 worker concurrency。
- `task_meta_modified` 仅在值变化时调用后端，且先更新本地 `max_write_speed`；后端接口不返回错误，所以本模块无法报告限速更新失败。
- `run_subtask` 使用同步调用前后手工切换 `engine_running`。若后端实现 panic，该标志不会经由 RAII 守卫复位；trait 也未表达取消、超时或异步错误。
- `get_index_info_and_id` 的未知单 ID 返回零值而非 `IndexNotFound`，空 ID/空索引会 panic；两者均是兼容约束，扩展时不能未经 Go 对照就“修正”。

## 并发与资源生命周期

本文件没有线程、任务、锁、通道或原子量；所有可变操作通过 `&mut self` 串行发生，线程安全及并发调度责任属于调用方。`engine_running` 只是普通布尔值，不支持另一个线程无同步地观察或修改。与之相比，Go 实现用 `atomic.Pointer[globalsort.Engine]` 暴露正在运行的引擎，以便资源回调并发加载；Rust 当前同步模型是简化的可测试实现，不能推断为等价的跨线程回调机制。

资源顺序为 `new -> init -> run_subtask* -> cleanup`。`cleanup` 先清运行标志，再调用 `backend.close()`，最后清初始化标志；它不返回关闭错误。每次导入的外部引擎配置由后端接管处理，本文件没有显式删除外部文件或关闭单独引擎的清理步骤。动态资源更新只在导入窗口执行，并在后端更新成功后才提交新的 worker concurrency。

## 与 Go 版本的对应关系

Rust 的 `CloudImportExecutor`、`CloudImportBackend` 和 `ExternalEngineConfig` 对应 Go `pkg/ddl/backfilling_import_cloud.go:cloudImportExecutor` 对真实 ingest/Lightning 对象的使用；`run_subtask` 对应 `RunSubtask` 的核心索引选择、元信息合并、旧版 `RangeJobKeys` 回退、外部引擎关闭/查找/导入及重复键转换流程。

已明确保留的语义包括：未知单元素 ID 返回命名返回值零值；空元素 ID 使用首个索引且仅单索引时携带索引信息；多个 ID 报错；资源并发相同可直接成功、引擎未开始则要求重试；`Processed` 把有符号字节数转换为 `u64`，负数按补码产生大值；并发度 `0` 原样传递。

Rust 尚未复刻 Go 的完整外围能力：创建本地 backend/backend context、对象存储打开与访问计量、子任务字节解码、Lightning 指标注册、真实 UUID 生成、分区表元数据、split-region 参数、failpoint、`RealtimeSummary`、错误链中的 `terror`/`kv.ErrKeyExists` 转换，以及分布式框架的真实执行器构造。Rust 还把 Go 的原子引擎指针简化为布尔运行标志，并用字符串作为引擎 ID。`LocalBackendNotFound` 与 `IndexNotFound` 的 Rust 枚举存在但当前路径未使用。相关差异是迁移状态，不应通过文档宣称已经完整等价。

Go 同名专用测试文件不存在；`pkg/ddl/backfilling_dist_scheduler_test.go` 只覆盖包含 `BackfillStepWriteAndIngest` 的调度阶段。当前文件的局部契约主要由独立 Rust 测试 `pkg/ddl/backfilling_import_cloud_test.rs` 覆盖。

## 扩展指南

- 接通 Rust 生产主链时，应在 `pkg/ddl/backfilling_dist_executor.rs` 的 `WriteAndIngest` 阶段构造真实适配器，而不是把 Lightning/对象存储细节塞回通用算法；同时补独立测试验证工厂接线、初始化失败清理、解码和摘要/计量传播。
- 新增后端能力优先扩展 `CloudImportBackend`，并同步更新 `BackendStub`。需特别评估真实实现是否允许 `&mut self` 同步调用，以及资源回调是否需要锁、原子或异步接口。
- 修改 `run_subtask` 时同步覆盖：`meta_groups` 合并、旧版 `range_job_keys` 回退、外部引擎不存在、普通后端错误、重复键带/不带索引名，以及导入结束后状态复位。若引入可 panic/取消路径，宜用作用域守卫确保 `engine_running` 复位。
- 修改索引解析必须同步 `get_index_info_and_id` 和 `pkg/ddl/backfilling_merge_sort.rs` 的使用者，并以 Go 的 `getIndexInfoAndID` 为兼容基准；不可擅自把未知 ID 或空索引改成新错误语义。
- 修改资源或限速逻辑时同步 `resource_modified`、`task_meta_modified` 和独立测试，确认零并发、相同并发、未启动引擎、后端更新失败及内存值透传。
- 修改计量时注意 Go 注释指出 Region job 重试可能使累计值大于总 KV 大小；不要把计数强制截断到 `total_file_size`。测试仍应位于独立的 `pkg/ddl/backfilling_import_cloud_test.rs`，不要内嵌到生产文件。
- 兼容与性能风险集中在旧版元数据解释、唯一索引冲突呈现、切分键选择、资源更新时机、对象存储访问量和大文件导入内存；任何接线都应保留这些边界并增加端到端阶段验证。

## 验证依据

- Rust 源码：`pkg/ddl/backfilling_import_cloud.rs`（RustCodeGraph `node --file ... --offset 1 --limit 500` 覆盖全 319 行）。
- RustCodeGraph：`explore "pkg/ddl/backfilling_import_cloud.rs symbols callers callees cloud import backfill"`；`callees CloudImportExecutor::run_subtask` 确认 Rust `run_subtask` 到后端 trait、配置和索引解析的边；`callees resource_modified` 确认三个资源方法调用；`callees get_index_info_and_id` 确认异常分支。限定方法名查询会同时返回 Go/Rust 同名候选，因此调用链结论均按文件路径消歧。
- crate/模块：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；Cargo 未为本文件设置单独 feature，测试模块由 `cfg(test)` 独立挂载。
- 直接 Rust 证据：`pkg/ddl/backfilling_dist_executor.rs`、`pkg/ddl/backfilling_merge_sort.rs`、`pkg/ddl/backfilling_merge_temp.rs`、`pkg/ddl/backfilling_read_index.rs`。
- Go 对照：`pkg/ddl/backfilling_import_cloud.go`、`pkg/ddl/backfilling_dist_executor.go:newBackfillStepExecutor`；Go 专用 `backfilling_import_cloud*_test.go` 不存在，阶段调度参考 `pkg/ddl/backfilling_dist_scheduler_test.go`。
- 独立 Rust 测试：`pkg/ddl/backfilling_import_cloud_test.rs` 验证未知单 ID 的零值结果、零并发透传、有符号到无符号计量转换、以及引擎未运行时资源更新要求重试。
- 仓库 DDL 契约：`pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md` 用于定位 DDL/reorg 背景；具体结论均由上述源码与测试复核。
- 本任务仅新增说明文档，未运行 Cargo。交付结构检查要求目标文件存在且恰有十一个规定的二级标题；路径和引用通过 `rg`/`test -f` 核验。
