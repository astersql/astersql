# `pkg/dxf/importinto/subtask_executor.rs`

## 文件定位

本说明对应源码 [`subtask_executor.rs`](./subtask_executor.rs)。该文件属于 `astersql-dxf-importinto` crate（边界见 `pkg/dxf/importinto/Cargo.toml`），承接 IMPORT INTO 的两段执行逻辑：一是把单个 `importStepMinimalTask` 的输入 chunk 送入真实 importer 编码管线；二是执行最终 post-process，包括 allocator rebase、本地 checksum 合并与远端 checksum 校验。模块由 `pkg/dxf/importinto/lib.rs` 公开导出。

在最小任务主链上，直接上游是 `encode_and_sort_operator.rs` 的 `chunkWorker::HandleTask`，它准备 global-sort writer 后调用 `runImportMinimalTask`。在 post-process 主链上，`planner.rs` 的 `PostProcessSpec::ToSubtaskMeta` 聚合此前步骤的 checksum、删除行 checksum 和最大 allocator ID，生成本文件消费的 JSON；`task_executor.rs::GetImportStepExecutor` 当前把 `ImportStepPostProcess` 委托给 `ImportStepHost::NewStepExecutor`，而本文件的 `GetPostProcessStepExecutor`/`NewPostProcessStepExecutor` 提供可由该宿主边界采用的具体实现。也就是说，Rust 代码已经具备后处理执行逻辑，但从顶层任务执行器到该具体构造器的生产接线由宿主实现决定，不能仅凭本文件断言所有运行环境都直接调用它。

## 核心职责

1. `runImportMinimalTask` 根据 `Plan::IsLocalSort` 在 local sort 与 global sort 两条写入路径间分派，执行一个 chunk 的解析、编码及 data/index KV 写入，并把分组 checksum 汇入 subtask 共享状态。
2. `LocatedImportChunk` 在不改变持久化 `importer::Chunk` wire 格式的情况下，把当前 `TableImporter.LoadDataController.ParquetLocation()` 暴露给 importer 的 `ImportChunk` 接口。
3. `decodePostProcessMeta` 解析 Go 兼容的 post-process JSON，并把字符串 allocator 键映射为 `AllocatorType`。
4. `PostProcessStepExecutor` 按“先 rebase、再构造最终 checksum、必要时校验”的顺序执行后处理，并依据 Classic/NextGen 内核选择远端 checksum 实现。
5. `buildFinalChecksum` 和 `runPostProcessWith` 把确定性的 checksum 算法及步骤顺序从外部存储/SQL/TiKV I/O 中分离，便于独立验证。

## 主要符号

- `PostProcessChecksumManager`：NextGen checksum manager 的最小接口。`Checksum` 返回远端三元组；`Close` 由执行器保证调用。
- `PostProcessHost`：宿主注入的运行时边界，提供 `RebaseAllocatorBases`、Classic SQL checksum，以及 NextGen manager 构造。文件本身不持有具体 storage/session 实现。
- `PostProcessWire`：仅用于 serde 解码的私有 wire 类型；字段名保持 Go JSON 兼容，冲突过多标记同时接受 `too-many-conflicts-from-index` 和 `TooManyConflictsFromIndex`。
- `decodePostProcessMeta(bytes)`：把 JSON 转为 `PostProcessStepMeta`；allocator 键支持数字 `0..3` 与 `_tidb_rowid`、`auto_increment`、`auto_random`、`sequence`。
- `PostProcessStepExecutor`：保存 `task_id`、`importer::Plan`、宿主和可选 `FrameworkInfo`，实现 `StepExecFrameworkInfo` 与 `StepExecutor`。
- `GetPostProcessStepExecutor(task, host)`：解析 `TaskMeta`、验证 task step 必须为 `ImportStepPostProcess`，再返回 trait object。
- `NewPostProcessStepExecutor(task_id, plan, host)`：供测试或宿主直接构造具体执行器。
- `LocatedImportChunk`：借用原始 chunk 和 location；除 `ParquetLocation` 外，`ImportChunk` 方法均转发原始字段。
- `runImportMinimalTask`：本文件的 chunk 编码入口。
- `buildFinalChecksum`：合并所有 KV group 后减去冲突解决阶段删除行的 checksum。
- `runPostProcessWith`：强制 rebase 在 checksum 构造/校验之前；仅在 rebase 成功后，才允许因索引冲突过多跳过远端校验。

## 执行流程

最小任务流程如下：

1. `chunkWorker::HandleTask` 从 worker 持有的 writer 构造代理并调用 `runImportMinimalTask`。
2. 函数构造 `LocatedImportChunk`，把 plan 选定的 parquet location 附着到本次解析视图。
3. 使用 table importer 的 keyspace 创建 `KVGroupChecksum`，并包在 `Arc<Mutex<_>>` 中供 importer 管线更新。
4. local sort 分支要求 `SharedVars.DataEngine` 和 `IndexEngine` 均已初始化，然后调用 `importer::ProcessChunkAndLogger`；global sort 分支要求调用方同时传入 data/index writer，然后调用 `ProcessChunkWithWriterAndLogger`。
5. importer 成功后取得 checksum 锁；即使 mutex 曾 poison，也通过 `into_inner` 取回数据；随后调用 `SharedVars.Checksum.Add` 汇总本 chunk 结果。

后处理流程如下：

1. `StepExecutor::RunSubtask` 把 `subtask.Meta` 交给 `RunMetaWithContext`；后者调用 `decodePostProcessMeta`。
2. `runPostProcessWith` 首先调用宿主的 `RebaseAllocatorBases`。失败立即返回，不执行 checksum 校验。
3. `buildFinalChecksum` 用 `AddRawGroup` 恢复各 group 的 `Size/KVs/Sum`，合并后减去 `DeletedRowsChecksum`。
4. 若 `TooManyConflictsFromIndex` 为真，因删除行 checksum 可能不准确而跳过远端校验，但 allocator rebase 已经完成。
5. Classic 内核通过 `RemoteChecksumClassic` 获取远端值；NextGen 内核先创建 manager，再由 `importer::VerifyChecksum` 调用其 `Checksum`。局部 `CloseGuard` 的 `Drop` 在成功或错误返回时都会执行 `Close`。

## 数据与状态

- `importStepMinimalTask` 拥有 `Plan`、`Chunk`、`SharedVars` 与 logger。`runImportMinimalTask` 修改的是 `SharedVars.Checksum` 的聚合结果，并通过 importer 间接写入 engine/writer。
- local-sort engine 存在于 `SharedVars.DataEngine/IndexEngine`；global-sort writer 属于 `chunkWorker`，以 `Box<dyn EngineWriter>` 代理传入。函数不负责最终关闭 writer，生命周期由 worker 的 `Close` 管理。
- chunk checksum 以 keyspace 初始化，保持同一导入任务的 key 编码域；每个 chunk 先在独立的 mutex 中累加，再合并进共享 checksum。
- `PostProcessStepMeta.Checksum` 是 `group_id -> Checksum`；`DeletedRowsChecksum` 表示冲突处理删除的数据；`MaxIDs` 保存各 allocator 的最大基址；这些数据由 `planner.rs::PostProcessSpec::ToSubtaskMeta` 从前序 subtask meta 聚合。
- `PostProcessStepExecutor.framework` 初始为 `None`。设置后，resource、meter recorder 和 checkpoint 回调访问均委托给 `FrameworkInfo`；未设置时 step 为 `0`、其余可选值为 `None`，`SetResource` 无操作。

## 依赖与调用关系

直接上游与接线证据：

- `encode_and_sort_operator.rs::chunkWorker::HandleTask -> runImportMinimalTask` 是目标文件外已确认的直接 Rust 调用边。
- `planner.rs::PostProcessSpec::ToSubtaskMeta -> PostProcessStepMeta JSON -> PostProcessStepExecutor::RunSubtask` 构成后处理数据链。
- `task_executor.rs::GetImportStepExecutor` 对 import/collect/conflict/post-process 分支调用 `ImportStepHost::NewStepExecutor`；宿主若采用本文件实现，可调用 `GetPostProcessStepExecutor`。仓库内未发现 `GetPostProcessStepExecutor` 的直接生产调用，因此该接线状态应视为显式宿主扩展点，而非已验证的固定调用边。

主要下游依赖：

- `astersql-executor-importer`：`ImportChunk`、`ProcessChunkAndLogger`、`ProcessChunkWithWriterAndLogger`、`VerifyChecksum`、`Plan` 与远端 checksum 类型。
- `astersql-lightning-backend` / `backend-encode`：engine writer 与执行 context。
- `astersql-lightning-verification`：分组 checksum 创建、合并和减法。
- `astersql-dxf-framework-taskexecutor-execute` / `framework-proto`：collector、step executor 生命周期、framework info、task/subtask/resource。
- `astersql-meta-autoid`：wire allocator 类型到运行时 enum 的映射。
- `astersql-config-kerneltype`：运行时选择 Classic 或 NextGen checksum 路径；Cargo feature `nextgen` 同时把 feature 传给该依赖。

## 错误处理与边界

- post-process JSON 非法、allocator 类型未知、task meta 解析失败或 step 不匹配都会立即返回错误；未知 allocator 不被静默忽略。
- local sort 缺少任一 engine、global sort 缺少任一 writer 都返回带模式信息的明确错误，且不会继续调用 importer。
- importer 的字符串错误统一经 `errors::New` 转换；只有 importer 成功后才把 chunk checksum 加入共享汇总，避免失败 chunk 被计入。
- `runPostProcessWith` 保留 rebase 与远端 checksum 错误原文；rebase 失败时不调用 verify。
- `TooManyConflictsFromIndex` 只跳过远端 checksum 校验，不跳过 allocator rebase；这是与 Go 顺序一致的重要不变量。
- checksum 策略 `Required/Optional/Off` 的最终差异由 `importer::VerifyChecksum` 决定，本文件不重复实现。测试证明 Required mismatch 报错，而 Optional/Off 可成功。
- `StepExecutor::Init`、`Cleanup`、`TaskMetaModified` 和 `ResourceModified` 当前为空操作；`RealtimeSummary` 始终为 `None`。扩展资源或动态 meta 行为时不能假设这些 hook 已实现。

## 并发与资源生命周期

- `runImportMinimalTask` 接收的 checksum 用 `Arc<Mutex<_>>` 与 importer 共享；读取结果时恢复 poisoned mutex 的 inner value，避免因先前 panic 永久丢失已积累 checksum。
- `SharedVars` 是 minimal task 共享状态；本函数最终调用 `Checksum.Add`，而 `proto.rs` 中 `SharedVars` 还包含用于 Go 对齐的 `mu`。当前调用点没有显式取得该 `mu`，因此并发安全依赖 `KVGroupChecksum::Add`/上层 worker 调度的实际保证；本文不把未见的串行性当作已验证事实。
- global-sort writer 是 worker 所有资源的代理，本函数消费代理但不关闭底层 writer；跨 chunk 复用及 flush/close 由 `encode_and_sort_operator.rs::chunkWorker` 负责。
- NextGen manager 创建成功后立即包装为 `CloseGuard`，Rust RAII 保证正常返回、checksum 失败或策略短路时离开作用域均调用 `Close`。若 manager 构造本身失败则不存在可关闭资源。
- `PostProcessStepExecutor` 持有 `Arc<dyn PostProcessHost>`，允许框架与测试共享宿主；trait 要求 `Send + Sync`。框架 resource 同样以 `Arc` 传递。

## 与 Go 版本的对应关系

- Go `importMinimalTaskExecutor.Run` 与 Rust `runImportMinimalTask` 都复制/包装 chunk、设置 parquet location、按 local/global sort 选择 `ProcessChunk` 或 writer 版本、累积 checksum。Rust 用 `LocatedImportChunk` 视图避免修改持久化 chunk wire；Go 直接复制 chunk 并设置 `ParquetMeta.Loc`。
- Go local sort 直接把 engine 传给 `ProcessChunk`；Rust 在进入管线前显式检查 `Option`，缺失时返回可诊断错误。global sort 同理检查 writer。
- Go 文件含 `beforeSortChunk`、`errorWhenSortChunk`、`syncBeforeSortChunk` failpoint；目标 Rust 函数中没有对应 failpoint，现有 Rust 重试测试通过 importer/engine host 注入错误覆盖失败清理，而非复刻这些注入点。
- Go `postProcess` 的顺序是 rebase、合并 checksum、减去删除 checksum、冲突过多则跳过验证，再选择 NextGen manager 或 Classic SQL。Rust 的 `runPostProcessWith` 与 `RunMetaWithContext` 保持这一顺序。
- Go 具体持有 store、task table、keyspace 和 logger，并直接创建 TiKV checksum manager或新 session；Rust 把这些 I/O 收敛到 `PostProcessHost`。因此 Rust 是可执行的算法与框架适配层，但具体存储/session 行为取决于宿主实现。
- Go NextGen 使用 `defer mgr.Close()`；Rust 使用 `Drop` guard，错误路径同样关闭。Go 还附加 internal source type 与 logger；这些上下文装饰不在本文件中，若宿主需要完全等价，应在 host 实现中提供。
- Go 的日志与排序前/后处理 failpoint 更完整；Rust 本文件只把 logger 传入 importer，不复刻 post-process task log。它们是可观测性差异，不能描述为已对齐。

## 扩展指南

- 新增 chunk 输入元数据时，优先扩展 `importer::ImportChunk` 及 `LocatedImportChunk` 的转发方法，不要为临时运行时字段破坏已有 `Chunk` JSON 格式；同步更新 `subtask_executor_test.rs` 的 parser-boundary 测试。
- 新增 sort 模式或 writer 类型时，在 `runImportMinimalTask` 分派，并保持“先验证所需资源、成功后才汇总 checksum”的不变量；同时检查 `encode_and_sort_operator.rs::chunkWorker` 的创建与关闭逻辑。
- 改动 post-process wire 时，同步修改 `PostProcessWire`、`planner.rs::post_process_value` 和 `proto.rs::PostProcessStepMeta`，并覆盖 Go 字段名、旧别名和未知值行为。
- 新增 allocator 类型必须同时更新 `decodePostProcessMeta` 的数字/名称映射、planner 序列化和独立测试；未知类型应继续显式报错，以免错误 rebase。
- 改动 checksum 顺序时，必须保持 rebase 错误短路、冲突过多仅跳过 verify、删除 checksum 在 verify 前扣除，以及 NextGen manager 必关闭。对应测试放在独立的 `subtask_executor_test.rs` 或较高层 `task_executor_testkit_test.rs`，不要内嵌到生产文件。
- 若要把具体后处理接入所有 Rust 生产运行时，应实现/核对 `ImportStepHost::NewStepExecutor` 对 `ImportStepPostProcess` 的分支并调用 `GetPostProcessStepExecutor`；不要绕过 `TaskMeta::Unmarshal` 和 step 校验。
- 性能风险主要在每 chunk checksum 锁、writer 生命周期及远端 checksum I/O；兼容风险集中在 Go JSON 字段、allocator enum 和 checksum 策略语义。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/dxf/importinto` 确认目标及相邻模块；`node --file pkg/dxf/importinto/subtask_executor.rs --offset 1 --limit 500` 返回目标文件全部 396 行及被 7 个文件使用的信息。对主要符号执行了 `callers/callees`，命令未返回可用边，因此调用关系另以直接引用核验，未把缺失图边当作不存在调用。
- 目标源码：`pkg/dxf/importinto/subtask_executor.rs`，核对全部 trait、struct、函数、impl、错误分支和资源 guard；文件无条件编译项，只有运行时 `IsNextGen()` 分支。
- crate/模块：`pkg/dxf/importinto/Cargo.toml`、`pkg/dxf/importinto/lib.rs`，核对 crate 名、`nextgen` feature、直接依赖和测试模块装配。
- 直接 Rust 调用与元数据来源：`pkg/dxf/importinto/encode_and_sort_operator.rs`、`pkg/dxf/importinto/task_executor.rs`、`pkg/dxf/importinto/planner.rs`、`pkg/dxf/importinto/proto.rs`。
- Go 对照：`pkg/dxf/importinto/subtask_executor.go` 与 `pkg/dxf/importinto/task_executor.go`，核对 minimal-task 分派、post-process 顺序、具体存储/session I/O、manager 关闭及顶层 step 分派。
- 独立测试：`pkg/dxf/importinto/subtask_executor_test.rs` 覆盖 wire 解码、host 调用顺序、parquet location、跳过校验、错误传播；`pkg/dxf/importinto/task_executor_testkit_test.rs` 覆盖 Required/Optional/Off 策略；Go 的 `pkg/dxf/importinto/task_executor_testkit_test.go` 覆盖真实表 checksum 成功、mismatch 与策略行为。
- 本任务为纯文档分析，按计划未运行 Cargo；交付结构检查要求文档存在且恰有十一个固定二级标题。
