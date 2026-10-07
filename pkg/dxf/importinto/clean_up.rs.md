# `pkg/dxf/importinto/clean_up.rs`

## 文件定位

本文件属于 `astersql-dxf-importinto` crate（边界见 `pkg/dxf/importinto/Cargo.toml`），由 `lib.rs` 的 `clean_up` 模块公开导出。它位于 IMPORT INTO 分布式任务的收尾阶段：框架调度器从任务表取得终态任务后，按任务类型查找 `Cleaner`，优先通过 `BatchCleaner` 成批清理，成功的任务才会被迁入历史表（`pkg/dxf/framework/scheduler/scheduler_manager.rs::clean_task_batch`）。本模块同时实现普通/批量任务清理以及 owner 侧过期冲突文件清理能力。

Rust 侧通过 `RegisterImportCleanerFactory` 提供 `ImportInto` 类型的清理器注册入口；当前仓库搜索只发现 `clean_up_test.rs` 直接调用它，未发现生产 Rust 入口调用，因此不能据此断言生产进程已自动完成注册。Go 对照版则在 `clean_up.go::init` 中自动注册。

## 核心职责

- `ImportCleaner::BatchCleanWithContext` 统一清理顺序：解析任务元数据并立即脱敏、classic 部署恢复表模式、按原始云存储 URI 分组删除全局排序文件、最后为 next-gen 的成功任务发送计量数据。
- 同 URI 的多个任务合并为一次对象存储扫描；目录名使用任务 ID，删除逻辑由 `astersql_ingestor_globalsort::CleanUpFilesInDirectories` 完成，并保留其他任务目录和 `conflicted-rows` 等邻接对象。
- `CleanupStoreGuard` 保证成功打开的对象存储在正常返回、错误返回或栈展开时都调用 `close`。
- `sendMeterOnCleanInParallel` 以最多 4 个作用域线程并发计量，首个错误或父上下文取消会停止领取后续任务，并等待所有已启动 worker 退出。
- `ExpiredFileCleaner::clean_expired_files` 把框架任务信息接口适配为 `conflictrows::TaskInfoGetter`，复用 `CleanConflictRowFiles` 清理过期冲突行文件。

## 主要符号

- `RestoreTableModeError::{TableNotFound, Other}`：把可忽略的“表已删除/截断”与必须传播的恢复失败分开。
- `CleanupMeteringContext`：包装框架计量 `Context` 和批内 `AtomicBool`；`is_cancelled` 同时观察父取消与同批 worker 失败，`check` 区分 `context canceled` 和 deadline exceeded。
- `ImportCleanUpRuntime`：DDL、任务元数据、对象存储构造和计量发送的依赖注入边界。带 context 的默认方法先检查取消，再调用无 context 方法；阻塞 I/O 实现应覆写它们以持续观察取消。
- `ImportCleanUpStorage` 与 `CleanupStoreGuard`：在全局排序 `Storage` 上增加 `close` 生命周期契约，并用 `Drop` 自动关闭。
- `ImportCleaner::{new, Clean, BatchClean, BatchCleanWithContext}`：分别负责构造、单任务转批处理、后台上下文批处理、可取消批处理。
- `sendMeterOnClean`：读取唯一的 post-process 元数据，汇总 checksum 后调用运行时发送计量；没有元数据时安静跳过。
- `sendMeterOnCleanInParallel`：包内可见的有界并行调度器，捕获 worker panic 并转换成共享错误。
- `meterDataFromPostProcess`：返回 `(row_count, data_kv_size, index_kv_size)`；数据组取 `DataKVGroupID`，其他组的 `Size` 以 `wrapping_add` 汇总为索引体积。
- 三个框架 trait 实现：`Cleaner` 暴露批量和过期文件能力，`BatchCleaner` 转换框架任务并回写脱敏后的 meta，`ExpiredFileCleaner` 连接冲突文件回收。
- `RegisterImportCleanerFactory`：向全局 registry 注册 `ImportInto -> ImportCleaner` 工厂。

## 执行流程

1. 框架的 `Manager::clean_task_batch` 按 `task_type` 分组；若清理器暴露 `batch_cleaner`，整组调用 `batch_clean`。目标适配层先用 `scheduler::frameworkTaskToImportTask` 将框架任务快照转换为 IMPORT INTO 协议任务。
2. `BatchCleanWithContext` 顺序遍历任务。每项先 `TaskMeta::Unmarshal`；保存未脱敏的 `CloudStorageURI` 和 `IsGlobalSort` 结果，再调用 `redactSensitiveInfo` 清空 SQL、脱敏路径和 URI，并立即写回任务 meta。
3. classic 部署调用 `restore_table_mode(DBID, TableInfo.ID)`。缺少 `TableInfo` 时 Rust 当前使用 `0`；`TableNotFound` 被视为已达到正常模式，其他错误立刻终止。next-gen 不执行此步骤。
4. 对 global-sort 任务按原始 URI 聚合任务 ID 目录。只有 next-gen 且状态为 `TaskStateSucceed` 的任务进入计量列表；失败任务仍参与排序文件清理。
5. 每个 URI 打开一次 store，把该组所有任务目录交给 `CleanUpFilesInDirectories`；扫描/删除错误立即返回。guard 随作用域结束关闭 store。全部文件组完成后才开始计量。
6. 计量阶段最多启动 `min(4, task_count)` 个作用域线程。worker 在互斥迭代器中领取任务前检查取消，执行 `sendMeterOnClean`；首个错误被保存并设置批内取消标志，未领取任务不再启动，作用域结束保证所有 worker 已 join。
7. 框架 `BatchCleaner` 无论后续副作用成功与否，都会把已转换任务的 `Meta` 回写原框架任务，因此已完成的脱敏不会因打开 store、删除或计量失败而丢失；然后把错误转换成 `SchedulerError`。框架只把成功清理的组迁入历史表，副作用与迁移并非事务。
8. 独立的过期文件流程由 `Manager::run_expired_file_clean` 调用注册清理器的 `expired_file_cleaner`，构造带取消标志的对象存储 context 后委托 `CleanConflictRowFiles`。

## 数据与状态

- 输入任务的核心状态是 `Task.ID`、`Task.State` 和序列化 `Task.Meta`。`TaskMeta.Plan` 提供 `DBID`、可选 `TableInfo.ID`、云存储 URI 与 global-sort 判定。
- `groups: HashMap<String, Vec<String>>` 以包含实时凭据的原始 URI 为键，以十进制任务 ID 为待删除目录。该原始 URI 只保存在局部变量；持久化回任务的 URI 已脱敏。
- `meter_tasks` 保存原切片索引而不是提前借用任务；遍历完成后再转成 `&Task`，避免与可变遍历冲突，并确保计量看到脱敏后的 meta。
- checksum 中 `DataKVGroupID` 决定行数和数据 KV 体积；若出现多个数据组，后遍历项覆盖前项。其他所有组都算作索引 KV，按 Go `uint64` 语义模 $2^{64}$ 回绕。
- 清理没有事务性回滚：表模式、各 URI 文件删除、计量、任务迁历史可能分阶段成功。框架契约和实现注释都要求重试安全；扩展副作用必须保持幂等。

## 依赖与调用关系

上游主链是 `pkg/dxf/framework/scheduler/scheduler_manager.rs::clean_finished_tasks/process_clean_task_batch -> clean_task_batch -> BatchCleaner::batch_clean`。过期文件链是 `Manager::run_expired_file_clean -> ExpiredFileCleaner::clean_expired_files`。注册边界是 `RegisterImportCleanerFactory -> astersql_dxf_framework_scheduler::RegisterCleanerFactory`；如上所述，生产 Rust 调用点当前未验证到。

下游依赖包括：`proto::{TaskMeta, PostProcessStepMeta}` 负责元数据；`scheduler::{frameworkTaskToImportTask, redactSensitiveInfo}` 负责框架转换与敏感信息处理；`astersql_ingestor_globalsort::CleanUpFilesInDirectories` 负责单次扫描并删除目标目录文件；`astersql_lightning_verification::DataKVGroupID` 区分 checksum 类型；`conflictrows::CleanConflictRowFiles` 负责过期冲突行对象；framework 的 proto、scheduler、storage、metering crates 提供任务模型、能力接口、任务清理信息与取消上下文。`Cargo.toml` 明确声明了这些同工作区依赖，并用 `nextgen` feature 转发 `astersql-config-kerneltype/nextgen`，但本文件的部署判断通过运行时接口而非条件编译完成。

## 错误处理与边界

- 任一 meta 反序列化失败会停止后续任务；此前任务已完成的脱敏仍保留，当前及后续任务未必已处理。
- classic 的表不存在错误被忽略；其他表模式错误立即返回，尚未遍历的任务不会脱敏，也不会执行文件或计量阶段。
- store 打开失败时没有可关闭资源；打开成功后，无论扫描或删除失败，`CleanupStoreGuard::drop` 都会关闭。文件清理失败会阻止所有计量。
- post-process 元数据不存在时跳过该任务计量；读取或发送错误进入并行首错路径。并发竞争下“首错”是最先取得互斥锁的错误，不保证按任务顺序。
- worker panic 被 `catch_unwind` 转成错误；仅识别 `String`/`&str` panic payload，其他 payload 使用 `metering panic`。互斥锁中毒处使用 `unwrap`，属于再次 panic 的边界。
- 空任务切片直接成功；非 global-sort 任务不会打开 store或计量。`BatchCleaner` 转换任何任务失败时不会开始实际清理，也不会回写部分转换结果。
- `redactSensitiveInfo` 内部忽略序列化回写错误；这是相邻函数当前契约，本文件不会单独获知脱敏写回失败，扩展时不可假定该操作可失败传播。

## 并发与资源生命周期

计量并发上限 `CLEAN_METERING_CONCURRENCY` 固定为 4。任务迭代器和首错槽分别由 `Mutex` 保护；批内取消用 `AtomicBool` 的 Release 写/Acquire 读传播。父 `Context` 不会因某个 worker 失败而被取消，失败仅设置子批的原子标志。`std::thread::scope` 使线程可以安全借用任务和回调，并在函数离开作用域前全部 join；因此返回错误时不会遗留后台 worker。

对象存储是逐 URI 串行打开和清理的，同组只扫描一次，不同 URI 当前不并行。每个成功打开的 store 由 guard 精确关闭一次。过期冲突文件清理显式创建可观察框架取消标志的 storage context；`CleanConflictRowFiles` 自身也在执行后关闭其 store。表模式恢复和文件清理均在计量 worker 启动前完成。

## 与 Go 版本的对应关系

Rust 主顺序与 `clean_up.go::BatchClean` 一致：逐任务解析和脱敏、classic 表模式恢复、按 URI 批量清理、next-gen 成功任务计量。两边都把并发数设为 4、只将成功的 global-sort 任务计量、忽略表不存在、在计量前完成全部文件清理，并按数据 checksum 与其他 checksum 分别计算体积。

主要适配差异如下：Go 从全局 `kerneltype`、task manager、domain、importer 和 handle 直接取得副作用，Rust 将它们收敛到 `ImportCleanUpRuntime`，便于主机接线和独立测试；Go `init` 自动注册，Rust使用显式 `RegisterImportCleanerFactory` 且生产调用点未验证；Go error group/channel 实现并发，Rust用作用域线程、共享迭代器和原子取消；Go 的 `uint64` 溢出自然回绕，Rust显式使用 `wrapping_add`；Go 期望 `TableInfo` 存在并直接取 ID，Rust缺失时传 `0`。Go 还保留日志、failpoint `mockCleanupError` 和按任务日志字段，Rust本文件没有对应日志或 failpoint。Rust新增了框架任务模型转换与 meta 回写适配，以及对 panic payload 的错误化处理。

## 扩展指南

- 新增清理阶段时应接入 `BatchCleanWithContext` 的明确阶段边界，并维持“持久化脱敏先于可能失败的外部副作用”“全部文件清理先于计量”的不变量；若副作用可能在重试中重复，必须设计为幂等。
- 新增运行时 I/O 应优先扩展 `ImportCleanUpRuntime`，并为阻塞实现覆写 context-aware 方法；不要让 worker 只在调用前检查一次取消。
- 调整文件分组或目录规则时同时检查 `CleanUpFilesInDirectories` 的单扫描语义和 `conflicted-rows` 保留规则，避免扩大删除范围。
- 调整计量字段时同步修改 `meterDataFromPostProcess`、`sendMeterOnClean` 和独立测试 `pkg/dxf/importinto/clean_up_test.rs`；保持 Go `uint64` 回绕及整数转换语义，评估大于 `i64::MAX` 时的计量兼容性。
- 改动框架接线时同步核对 `Cleaner`/`BatchCleaner`/`ExpiredFileCleaner` 三个能力和生产启动路径；在声称功能可用前，为 `RegisterImportCleanerFactory` 增加并验证真实生产调用者。
- 测试仍应放在独立的 `clean_up_test.rs`，覆盖部分失败后的 meta 回写、store 关闭、每 URI 单扫描、失败任务不计量、首错取消并 join、父取消/deadline、panic 恢复和过期文件适配；不要把测试内嵌回生产文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 文件、307,296 节点；`files --filter pkg/dxf/importinto` 定位目标、Go 对照与独立测试；`node --file pkg/dxf/importinto/clean_up.rs --offset 1 --limit 400` 及 `--offset 390 --limit 80` 核对完整 419 行源码。对 `RegisterImportCleanerFactory`、`sendMeterOnCleanInParallel`、`meterDataFromPostProcess` 执行 callers/callees 未返回额外边，随后用仓库精确搜索补证。
- 生产源码：`pkg/dxf/importinto/clean_up.rs`；模块入口 `pkg/dxf/importinto/lib.rs`；crate/feature/依赖边界 `pkg/dxf/importinto/Cargo.toml`；转换和脱敏 `pkg/dxf/importinto/scheduler.rs::{frameworkTaskToImportTask, redactSensitiveInfo}`；框架能力及主链 `pkg/dxf/framework/scheduler/{interface.rs,scheduler_manager.rs}`；文件删除 `pkg/ingestor/globalsort/util.rs::CleanUpFilesInDirectories`；冲突文件删除 `pkg/dxf/importinto/conflictrows.rs::CleanConflictRowFiles`。
- Go 对照：`pkg/dxf/importinto/clean_up.go`；Go 测试 `pkg/dxf/importinto/clean_up_test.go` 验证原始凭据用于存储、按 bucket 单次扫描、邻接对象保留、计量成功/取消/panic。
- Rust 独立测试：`pkg/dxf/importinto/clean_up_test.rs` 验证 checksum 回绕、脱敏时序、URI 分组、store 关闭、阶段错误短路、表不存在、空/畸形输入、计量阶段顺序、注册能力、4 worker 并发、首错取消与 join、父取消/deadline 和 panic。
- 仓库搜索 `rg -n "RegisterImportCleanerFactory|..."` 只定位到定义和测试调用，因此生产注册状态按“未验证接线”记录，不以设计预期替代当前证据。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务给定命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核唯一输出、源码路径和无整段源码复制。
