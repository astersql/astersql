# `pkg/dxf/importinto/conflictrows.rs`

## 文件定位

该文件属于 `astersql-dxf-importinto` crate（见 `pkg/dxf/importinto/Cargo.toml`），由 `pkg/dxf/importinto/lib.rs` 以 `pub mod conflictrows` 装配并整体再导出。它处在 IMPORT INTO 冲突行文件的“命名 + 到期清理”边界：写入侧通过 `NewFileNamePrefix` / `NewFileNamePrefixWithUUID` 生成对象名前缀，调度器的过期文件清理侧通过 `CleanConflictRowFiles` 扫描并删除不应继续保留的对象。

对象固定放在 `conflicted-rows/<task-id>/<subtask-id>-<uuid>...` 命名空间，而不是一般的 `<task-id>/` 临时目录。直接调用证据是 `pkg/dxf/importinto/collect_conflicts.rs::getConflictRowFilenamePrefix`（生成前缀）和 `pkg/dxf/importinto/clean_up.rs::ImportCleaner::clean_expired_files`（触发清理）。

## 核心职责

1. 用 `NewFileNamePrefix` 生成带随机 UUID 的冲突行对象前缀，并用 `NewFileNamePrefixWithUUID` 提供可注入 UUID 的确定性路径构造。
2. 用 `parse_task_id` 从 `conflicted-rows/` 下的对象名提取正整数任务 ID；格式错误、零、负数、带符号、溢出或没有非空后代路径的名称均视为不可解析。
3. 用 `should_delete` 实现保留策略：IMPORT INTO 的失败/回滚任务立即删除；成功任务仅在结束时间加七天 `RETENTION` 后（含到期瞬间）删除；运行中等其他状态保留。
4. 用 `CleanFiles` 分批扫描对象存储、批量查询任务元数据、执行批量删除并累计有界诊断统计。
5. 用 `CleanConflictRowFiles` 打开云存储、调用清理核心并确保关闭存储句柄；空 URI 是无操作成功。

## 主要符号

- `STORAGE_DIR` / `STORAGE_PREFIX`：分别为 `conflicted-rows` 与 `conflicted-rows/`，统一写入和扫描命名空间。
- `RETENTION`：成功任务冲突行的七天保留期；为 `pub(crate)`，供独立测试验证时间边界。
- `MAX_TASK_IDS_PER_FLUSH`、`MAX_OBJECTS_PER_FLUSH`：单批元数据查询和对象累积的软上限，分别为 128 和 1000。代码在纳入当前对象后用 `>` 判断，因此一批最多可比阈值多一个任务 ID 或对象。
- `MAX_LOGGED_SAMPLES`：每类诊断最多保留 16 个样本；计数仍记录全部命中量。
- `TaskInfoGetter`：清理核心所需的元数据最小接口，`GetTaskCleanupInfoByIDs` 按任务 ID 批量返回 `TaskCleanupInfo`。`clean_up.rs::SchedulerTaskInfoGetter` 将调度器 `TaskManager` 适配为此接口。
- `CountWithSamples`：某类异常的总数和有限样本，序列化字段为 `count`、`samples`，空值省略。
- `CleanupStats`：记录删除文件数、缺失任务及其文件、任务类型冲突文件、无法解析任务 ID 的文件和失败数；空字段经 Serde 省略。
- `CleanupFailure`：同时携带失败前已提交的 `CleanupStats` 和原始 `anyhow::Error`；`Display` 与 `source()` 均委托给原始错误。
- `NewFileNamePrefix(task_id, subtask_id)`：生成 UUID 后委托给确定性版本。
- `NewFileNamePrefixWithUUID(task_id, subtask_id, uuid)`：返回 `conflicted-rows/{task_id}/{subtask_id}-{uuid}`。
- `CleanFiles(ctx, store, info_getter, now)`：可测试的核心清理函数；显式注入存储、元数据源和时钟。
- `CleanConflictRowFiles(ctx, info_getter, cloud_storage_uri)`：生产入口；建立存储、使用当前系统时间清理并关闭存储。

## 执行流程

`CleanFiles` 的主流程如下：

1. 创建累计统计、按任务 ID 分组的 `task_files`、不可解析文件列表和当前批对象计数。
2. 调用 `Storage::WalkDir`，将 `WalkOption.sub_dir` 设为 `conflicted-rows/`，因此不会处理命名空间之外的对象。
3. 每个对象由 `parse_task_id` 分类：成功则加入相应任务的文件列表，失败则加入 `unparsed_files`；随后递增对象数。
4. 当对象数大于 1000 或不同任务 ID 数大于 128 时执行局部 `flush`。因为检查发生在插入之后，阈值是软上限；`conflictrows_test.rs::task_batch_soft_limit_keeps_the_current_walk_callback` 验证 129 个任务仍在同一批查询。
5. `flush` 对任务 ID 排序，使元数据查询和诊断样本顺序稳定；没有可解析任务 ID 时跳过元数据查询。
6. 不可解析对象直接列入删除集合。逐任务判断时：元数据缺失、任务类型不是 `proto::ImportInto`、或 `should_delete` 返回真都会删除；其他任务保留。
7. 删除集合非空时一次调用 `Storage::DeleteFiles`。只有删除成功后，才把本批 `DeletedFiles` 和诊断合并进总统计并清空批状态。
8. 遍历结束后再刷新尾批，记录一次结构化清理日志并返回统计。遍历或刷新失败时，失败数加一、记录截至失败点已成功合并的统计，然后返回 `CleanupFailure`。

`CleanConflictRowFiles` 在 URI 非空时通过 `astersql_objstore::storage::NewFromURL` 建立存储，调用 `CleanFiles(..., SystemTime::now())`，随后无条件调用 `Close`，最终仅向上返回原始 `Source`，不暴露内部统计类型给调度接口。

## 数据与状态

文件没有全局可变状态。一次清理调用的临时状态全部位于 `CleanFiles` 栈上：`HashMap<i64, Vec<String>>` 按任务聚合路径，`Vec<String>` 收集不可解析路径，`CleanupStats` 累计已完成批次的结果。

删除策略的重要不变量是：非 IMPORT INTO 元数据不会交给 `should_delete` 决定，而是作为任务 ID 冲突的异常对象记录样本并删除；缺失元数据同样删除。成功状态只有在 `EndTime` 存在、`EndTime + RETENTION` 不溢出且 `now >= expires_at` 时才删除。时间加法溢出会保守地保留对象。

统计采用“完整计数、有限样本”：`record_count_with_samples` 总是按输入数量增加 `Count`，但 `Samples` 最多扩充到 16 条；跨批合并也保持该上限。只有成功完成的批次才通过 `merge_completed_flush` 进入累计统计，因此一次失败不会把尚未成功删除的一批误报为已删除。

## 依赖与调用关系

上游关系：

- `pkg/dxf/importinto/collect_conflicts.rs::getConflictRowFilenamePrefix` 调用 `NewFileNamePrefixWithUUID`，把冲突收集结果放入本模块管理的命名空间。
- `pkg/dxf/importinto/clean_up.rs::ImportCleaner::clean_expired_files` 是调度框架的 `ExpiredFileCleaner` 实现；它把调度器取消标志转换成对象存储 `Context`，用 `SchedulerTaskInfoGetter` 桥接元数据查询，再调用 `CleanConflictRowFiles`。
- `pkg/dxf/importinto/lib.rs` 公开模块并再导出符号；`conflictrows_test.rs` 作为独立测试文件通过 `#[path = "conflictrows_test.rs"]` 接入。

下游关系：

- `astersql_dxf_framework_proto` 提供 `ImportInto` 任务类型和任务状态常量。
- `astersql_dxf_framework_storage::TaskCleanupInfo` 提供任务类型、状态和可选结束时间。
- `astersql_objstore::storage::{Storage, Context, WalkOption, NewFromURL}` 提供扫描、批量删除、取消传播、存储创建和关闭。
- `serde` / `serde_json` 将清理统计编码为日志字段；`astersql_util_logutil::BgLogger` 输出一次 Info 级完成/失败摘要。
- `uuid` 只用于生产前缀的随机后缀；`anyhow` 用于存储和元数据错误传播。

`Cargo.toml` 明确声明上述框架、对象存储、日志、Serde、JSON 和 UUID 依赖；本文件不受 crate 的 `nextgen` feature 条件编译控制，也没有自身的条件编译项。

## 错误处理与边界

- 空云存储 URI 立即返回成功，且不会尝试创建存储。
- `NewFromURL` 失败直接返回；由于尚未进入 `CleanFiles`，不会输出包含 URI 的清理统计日志，避免凭证 URI 被本模块日志带出。
- `WalkDir`、任务元数据查询或 `DeleteFiles` 的任一错误都会终止本轮。`CleanFiles` 用 `CleanupFailure` 保存错误和此前已完成批次的统计；外层入口向调度器返回原始错误。
- 遍历失败（包括取消）发生在刷新前时，不会进行元数据查询；独立测试 `cancelled_walk_returns_failure_stats_without_metadata_lookup` 验证此行为。
- 删除不是跨批事务：早先成功删除的批次不会因后续错误回滚。重试会重新扫描剩余对象，因而依赖删除与扫描结果实现可恢复，而不是依赖内存状态续跑。
- `parse_task_id` 仅接受 ASCII 十进制数字并要求值大于零；前导零允许，路径中任务 ID 后必须仍有非空后代。无法解析的对象位于扫描前缀内时会被删除并计入诊断。
- 对活动任务、成功但未到期、成功但无结束时间，以及时间加法溢出的任务均采取保守保留策略。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或通道。`Storage` trait 本身要求可跨线程共享，但 `CleanFiles` 在一次调用内串行遍历、查询和删除；`TaskInfoGetter` 也按批同步调用。

取消由调用方构造的 `Context` 传给 `WalkDir`、元数据查询接口和 `DeleteFiles`。调度适配器 `clean_up.rs::ImportCleaner::clean_expired_files` 从调度上下文共享取消标志，因此清理能够随调度任务取消。

存储生命周期由 `CleanConflictRowFiles` 管理：创建成功后，先保存 `CleanFiles` 结果，再调用 `store.Close()`，最后返回结果。因此清理成功和失败路径都会关闭句柄；当前 `Close` 无返回值，关闭失败不存在可传播通道。批内集合在成功刷新后清空以限制常驻内存，但由于阈值在加入当前对象后检查，允许单对象/单任务的一个单位超限。

## 与 Go 版本的对应关系

直接 Go 对照分为 `pkg/dxf/importinto/conflictrows/path.go` 和 `pkg/dxf/importinto/conflictrows/cleanup.go`。Rust 的路径常量、七天保留期、128/1000/16 三个上限、任务元数据接口、路径解析、删除判定、批次刷新、单次汇总日志和生产入口均保持相同策略。

类型映射上，Go 的 `map[int64][]string` / `[]string` 对应 Rust 的 `HashMap<i64, Vec<String>>` / `Vec<String>`；`*time.Time` 对应 `Option<SystemTime>`；Go 返回 `(cleanupStats, error)`，Rust 用 `Result<CleanupStats, CleanupFailure>` 同时保留部分统计和源错误。Go 通过 `defer sortStore.Close()` 管理资源，Rust 显式在 `CleanFiles` 返回后调用 `Close()`，覆盖相同的成功/失败范围。

存在一处接线层面的实现差异：Go 入口用 `importer.GetSortStore`，Rust 入口直接用 `astersql_objstore::storage::NewFromURL`。就本文件可验证的契约而言，两者都把 URI 解析成对象存储并执行同一清理策略；若未来两种构造器加入不同包装、凭证处理或后端选择，必须专门复核此差异，不能仅以函数名相近认定等价。

Go 测试 `conflictrows/cleanup_test.go` 覆盖更广的故障重试、日志样本上限和对象批次边界；Rust 独立测试覆盖核心移植语义，但尚未逐项复刻所有 Go 故障注入场景。本文据此只声明已有 Rust 测试实际覆盖的行为，不把 Go-only 用例当作 Rust 已验证结果。

## 扩展指南

- 修改命名格式时，应同时调整 `STORAGE_DIR` / `STORAGE_PREFIX`、`NewFileNamePrefixWithUUID`、`parse_task_id`，并核对 `collect_conflicts.rs::getConflictRowFilenamePrefix` 和 Go 的 `conflictrows/path.go`。必须在独立的 `conflictrows_test.rs` 增加兼容用例；不能把测试嵌回生产文件。
- 修改保留策略时，首要入口是 `should_delete` 和 `RETENTION`。应覆盖成功到期前一瞬、精确到期、无结束时间、失败、回滚、活动状态和非 IMPORT INTO 类型，并同步核对 Go `cleanup.go::shouldDelete`。
- 新增诊断分类时，应扩展 `CleanupStats`、相应记录/合并函数、Serde 省略规则和 `log_cleanup_stats` 的可观察结果；样本必须继续受 `MAX_LOGGED_SAMPLES` 限制，避免对象路径导致日志无界增长。
- 修改批处理策略时，要保持“成功删除后才合并统计”这一不变量，并评估对象数量、不同任务数量、元数据查询规模及删除请求规模。若把软上限改为硬上限，需要同步 Rust/Go 测试和行为，避免同一对象在批次边界被遗漏。
- 替换存储构造器时，应验证 URI 解析、后端包装、取消传播、关闭顺序和敏感 URI 不进入日志；`CleanFiles` 的依赖注入边界应保留，以便独立测试存储错误。
- 新增功能或修复应优先扩展同目录独立测试 `pkg/dxf/importinto/conflictrows_test.rs`；如果要求与 Go 完全对齐，还应对照 `pkg/dxf/importinto/conflictrows/cleanup_test.go` 的故障与日志用例补齐，而不是简化 Go 语义。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 `pkg/dxf/importinto/conflictrows.rs`；`files --filter pkg/dxf/importinto` 确认模块和独立测试；`node --file ...conflictrows.rs` 读取 299 行完整生产源码；`query` 精确定位 `NewFileNamePrefix`、`CleanFiles`、`CleanConflictRowFiles` 及其 Go 对照。图的精确 callers/callees 查询未在命令时限内返回，因此调用点使用下列源码搜索补证，没有据此推断未验证的边。
- 生产源码：`pkg/dxf/importinto/conflictrows.rs`；直接写入调用者 `pkg/dxf/importinto/collect_conflicts.rs::getConflictRowFilenamePrefix`；直接清理调用者 `pkg/dxf/importinto/clean_up.rs::ImportCleaner::clean_expired_files`；模块入口 `pkg/dxf/importinto/lib.rs`。
- Crate 边界：`pkg/dxf/importinto/Cargo.toml` 的 package 名、`lib.rs` 路径、`nextgen` feature 以及 proto/storage/objstore/logutil/serde/serde_json/uuid/anyhow 依赖。
- Rust 测试：`pkg/dxf/importinto/conflictrows_test.rs` 验证正任务 ID 解析、混合删除策略、成功保留期精确边界、命名格式、空统计序列化、任务批次软上限和取消错误统计。
- Go 对照：`pkg/dxf/importinto/conflictrows/path.go`、`pkg/dxf/importinto/conflictrows/cleanup.go`、`pkg/dxf/importinto/conflictrows/cleanup_test.go`，以及 Go 调用点 `pkg/dxf/importinto/collect_conflicts.go`、`pkg/dxf/importinto/clean_up.go`。
- 本任务仅生成文档，按计划不运行 Cargo；最终使用任务给定的命令验证文档存在且恰有 11 个固定二级章节，并人工复核本文件的定位、主流程、安全扩展点均有上述源码或测试依据。
