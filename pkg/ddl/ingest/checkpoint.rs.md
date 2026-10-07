# `pkg/ddl/ingest/checkpoint.rs`

## 文件定位

本文件属于 `astersql-ddl-ingest` crate，由 [`lib.rs`](lib.rs) 以 `pub mod checkpoint` 暴露。它位于 DDL add-index ingest/reorg 链路的进度管理层：扫描任务先把索引键写入本地排序引擎，随后再导入远端存储；本模块用“本地已刷入”和“全局已导入”两级水位线描述这两个阶段，并为中断恢复提供状态快照。

Rust 侧当前接线从 [`BackendContextBuilder::build`](backend_mgr.rs) 开始：仅当 builder 配置了 `checkpoint_storage` 时创建 `CheckpointManager`，再交给 [`BackendContext`](backend.rs) 持有。后端的 `add_chunk`、`update_chunk`、`finish_chunk`、`advance_watermark`、`next_start_key`、`total_key_count` 和 `import_ts` 都是对本文件同名能力的可选转发。它不是 DDL job 调度器，也不直接扫描表、写本地引擎或执行远端 ingest。

crate 边界见 [`Cargo.toml`](Cargo.toml)：crate 名为 `astersql-ddl-ingest`，Go 对照包标记为 `pkg/ddl/ingest`。本文件本身只使用标准库的 `BTreeMap`、`Arc` 和 `Mutex`；系统表、PD、分布式任务等 Go 侧依赖尚未进入这里。

## 核心职责

1. 用 `ReorgCheckpoint` 表示可持久化状态，包括格式版本、两级水位线及计数、实例地址、物理表 ID 和导入时间戳。
2. 通过 `CheckpointStorage` 隔离加载/保存方式，并提供测试及轻量接线使用的 `MemoryCheckpointStorage`。
3. 用 `TaskCheckpoint` 汇总每个分片的 reader/writer 进度，只有 reader 已报告末批、写入行数追平读取行数、完成批次数追平产生批次数时才判定分片完成。
4. 只沿从任务 0 开始的连续已完成前缀推进本地水位线，防止越过尚未完成的键区间。
5. 仅在调用 `advance_watermark(true)` 表示远端导入成功后，把全局水位线对齐本地水位线并保存；恢复时，本地数据不可信则回退到全局水位线。

## 主要符号

- `JOB_CHECKPOINT_VERSION_CURRENT: u64 = 1`：当前持久化格式版本。新建内存状态先使用版本 `0`，首次保存时由 `update_checkpoint` 写为当前版本。
- `Key = Vec<u8>`：编码键类型；所有水位线比较都是 Rust 字节切片/向量的字典序比较。
- `JobReorgMeta { checkpoint }`：与 Go `JobReorgMeta` 对应的包裹结构。本文件不负责序列化，供具体存储实现使用。
- `ReorgCheckpoint`：持久化快照。`local_sync_key/local_key_count` 表示本地已刷入进度；`global_sync_key/global_key_count` 表示已导入远端的进度；`instance_addr` 与 `physical_id` 用于判断恢复快照能否复用；`import_ts` 是当前导入使用的固定时间戳。
- `CheckpointStorage: Send + Sync`：同步存储接口；`load_checkpoint` 返回不存在或一份快照，`save_checkpoint` 保存完整快照，错误统一为 `String`。
- `MemoryCheckpointStorage`：`Arc<Mutex<Option<ReorgCheckpoint>>>` 包装的共享内存实现；加载和保存都克隆快照。
- `TaskCheckpoint`：内部任务状态，保存 `end_key`、读写键数、是否读到末批及读写两端的批次数。
- `CheckpointManager`：核心状态机。`tasks` 以 `BTreeMap<usize, _>` 保存任务；`min_task_id_finished` 标记连续前缀的下一任务 ID；`local_data_is_valid` 控制恢复时是否允许信任本地水位线；`dirty` 记录内存变化；`closed` 使成功关闭后的重复 `close` 幂等。
- `new` / `new_with_resume_options`：前者以 `physical_id = 0`、`local_data_available = true` 调用后者；后者执行完整恢复判断。
- `is_key_processed` / `next_start_key` / `total_key_count` / `import_ts`：只读查询接口。
- `add_chunk` / `update_chunk` / `finish_chunk`：分别注册任务、累计 reader 进度、累计 writer 进度。
- `advance_watermark`：推进入口；内部依次调用 `after_flush`，并在 `imported` 为真时调用 `after_import` 与 `update_checkpoint`。
- `close`：无论 `dirty` 是否为真都请求最终保存；只有保存成功后才设置 `closed`。

## 执行流程

初始化时，`new_with_resume_options` 先调用 `storage.load_checkpoint()`：

1. 若存在快照且 `physical_id` 相同，保留全局进度；只有 `local_data_available` 为真，并且快照实例地址等于当前地址或为空时，才保留本地进度。否则清空本地键和本地计数，从全局进度恢复。
2. 若没有快照，或快照属于其他物理表，则以 `start_key` 同时初始化本地、全局水位线，计数为 0，并使用调用方给出的实例、物理表和 `import_ts`。不匹配的旧快照不会被复用。
3. 任务表为空，`min_task_id_finished` 从 0 开始，初始不脏且未关闭。

正常处理时，调用方先用 `add_chunk(task_id, end_key)` 注册区间。reader 每产生一批调用 `update_chunk`，累计行数和批次数，并在末批令 `last_batch_read = true`；writer 每完成一批调用 `finish_chunk`，累计已写行数和完成批次数。`total_key_count` 返回已推进到本地水位线的计数，加上所有在途任务的 `written_keys`。

flush/import 后调用 `advance_watermark(imported)`：

1. `no_update` 为真时直接返回，不创建初始快照。
2. `after_flush` 从 `min_task_id_finished` 开始检查连续任务。只要某任务满足末批已读、`written_keys >= total_keys`、`chunks_finished >= chunks_total`，就移除该任务，把本地水位线设为其 `end_key`、累加 reader 统计的 `total_keys`，再检查下一个 ID；任何缺号或未完成任务都会停止推进。
3. `imported == false` 时到此结束，本地变化仅留在内存，等待后续 import 或 `close` 保存。
4. `imported == true` 时，`after_import` 先验证全局水位线没有超过本地水位线，再把全局键和计数对齐本地值；随后 `update_checkpoint` 写入版本 1 的完整快照。

恢复查询中，`is_key_processed` 始终信任非空的全局水位线；只有 `local_data_is_valid` 时才信任本地水位线。`next_start_key` 使用相同原则优先返回本地键，否则返回全局键。

## 数据与状态

关键不变量如下：

- 任务 ID 必须从 0 开始连续递增。代码不主动验证该契约，但 `after_flush` 只检查 `min_task_id_finished`，因此缺号会阻塞所有更大 ID 的水位线推进。
- 本地水位线只能覆盖连续完成的任务前缀，不能因高编号任务先完成而跳跃。[`checkpoint_advances_only_the_contiguous_completed_task_prefix`](checkpoint_test.rs) 覆盖乱序完成场景。
- 全局水位线代表已导入远端的数据，不能领先本地水位线；违反时 `after_import` 返回 `flushed key is less than imported key`。
- 分片完成同时依赖行数和批次数。即使行数相等，未读到末批或仍有未确认批次也不能推进；对应测试为 `checkpoint_waits_for_the_last_reader_batch_before_advancing` 和 `checkpoint_waits_for_every_finished_chunk_before_advancing`。
- 已确认计数使用 reader 的 `total_keys` 累加一次；writer 计数只用于在途展示与完成判定，避免重复计数。对应测试为 `checkpoint_counts_reader_rows_once_after_writer_completion`。
- 本地快照只在物理表匹配且本机数据仍可用时可信；全局快照不依赖本地目录。Rust 将目录/实例判断结果通过 `local_data_available` 和 `instance_addr` 参数注入。
- `dirty` 会在本地或全局推进时置真，保存成功后清零；当前 Rust 实现没有周期刷新循环，因此它只是状态标记，不会自行触发 I/O。

`add_chunk` 对相同 ID 会替换原状态；`update_chunk` 对未知 ID 静默不处理；`finish_chunk` 对未知 ID也静默返回。调用方应保证先注册且不重复注册正在执行的 ID。

## 依赖与调用关系

RustCodeGraph 对目标文件建立了 30 个符号，并报告该文件被 `backend_mgr.rs`、`checkpoint_test.rs`、`mock.rs`、`mock_test.rs` 等文件使用；精确方法调用边再由源码接线核对：

- 上游构造：[`BackendContextBuilder::build`](backend_mgr.rs) → `CheckpointManager::new` → `CheckpointManager::new_with_resume_options` → `CheckpointStorage::load_checkpoint`。
- 处理进度：[`BackendContext::add_chunk/update_chunk/finish_chunk`](backend.rs) → 本文件同名方法 → `TaskCheckpoint`。
- 刷新路径：`BackendContext::ingest_if_quota_exceeded` 的 `Flush` 分支 → `advance_watermark(false)` → `after_flush`。
- 导入路径：`BackendContext::ingest` → `advance_watermark(true)` → `after_flush` → `after_import` → `update_checkpoint` → `CheckpointStorage::save_checkpoint`。
- 恢复/展示：`BackendContext::next_start_key/total_key_count/import_ts` → 本文件查询方法。
- 测试替身：[`MockBackendContext`](mock.rs) 继续转发完整检查点契约，[`mock_backend_forwards_the_complete_checkpoint_contract`](mock_test.rs) 验证跨模块接线。

图索引对通用名称的首次 `explore` 命中了大量其他 checkpoint 符号，故调用关系使用精确文件符号查询并结合上述直接源码确认；没有把其他 BR/Lightning checkpoint 子系统当成本模块调用者。

## 错误处理与边界

- 构造阶段的加载错误、保存阶段的存储错误都以 `Result<_, String>` 原样向上传播；builder 的 `build` 也用 `?` 传播构造失败。
- 全局水位线领先本地水位线会阻止导入后保存，以免持久化一个违反阶段顺序的快照。
- `advance_watermark(false)` 不保存；如果进程在本地推进后退出，能否利用这段进度取决于之后是否调用 `close` 成功保存以及本地数据是否仍有效。
- `close` 成功后重复调用是无操作；若保存失败，`closed` 保持为假，调用方仍可重试。
- `MemoryCheckpointStorage` 对中毒的 `Mutex` 使用 `unwrap()`，因此持锁线程 panic 后后续访问也会 panic，而不是返回 `String` 错误。生产存储实现不应照搬这一测试实现的错误策略。
- 键比较是原始编码字节的字典序；调用方必须传入与扫描顺序一致的编码键。模块不验证 `end_key` 单调性，也不验证任务 ID 连续性。
- `JobReorgMeta`/`ReorgCheckpoint` 没有在本文件中实现序列化或版本迁移；存储实现必须负责稳定编码，并在引入新版本时定义兼容策略。

## 并发与资源生命周期

`CheckpointManager` 的修改方法都需要 `&mut self`，查询方法需要 `&self`；它本身没有内部互斥锁、后台任务、通道或定时器。共享和串行化责任属于持有者，Rust 借用规则可防止同一实例通过普通引用同时修改，但若未来放入外部锁或跨线程共享，仍需维持 reader/writer 上报与 flush/import 的顺序协议。

唯一的内部锁位于 `MemoryCheckpointStorage`，用于让多个 storage clone 共享同一份可选快照；锁只覆盖一次 clone 或替换，不跨越管理器状态变更。`CheckpointStorage: Send + Sync` 允许实现被跨线程持有，但接口调用是同步阻塞的。

资源生命周期由上游显式管理：构造时加载一次，import 成功时同步保存，`close` 时最终同步保存。`CheckpointManager` 没有 `Drop` 实现，`BackendContext::close` 当前也只关闭 engines 并标记 backend closed，没有调用 checkpoint 的 `close`；因此需要最终持久化的路径必须显式调用 `CheckpointManager::close` 或先执行成功的 imported watermark 推进，不能依赖析构。

## 与 Go 版本的对应关系

直接对照文件是 [`checkpoint.go`](checkpoint.go)，行为测试是 [`checkpoint_test.go`](checkpoint_test.go)。两版共同保留了两级水位线、连续任务 ID 前缀、reader/writer 双边完成条件、物理表与实例/本地数据恢复判断、未知 writer 任务不推进、最终保存等核心语义；Rust 的独立测试把这些规则拆成更聚焦的用例。

当前 Rust 实现是 Go 版本的同步、可注入存储子集，不能视为完整等价：

- Go 提供 `NormalCheckpointStorage`（`mysql.tidb_ddl_reorg.reorg_meta`）和 `DistTaskCheckpointStorage`；Rust 这里只定义 trait 和内存实现。
- Go 构造器从 PD 分配初始 TS，且每次 import 后获取更大的 TS；Rust 由调用方注入 `import_ts`，`after_import` 不更新它。
- Go 经理内部使用 `sync.Mutex`，并通过 goroutine、ticker 和 channel 周期保存脏状态；Rust 经理依靠 `&mut self` 串行修改，没有后台刷新。
- Go 从真实本地目录是否非空及 `InstanceAddr()` 判断本地数据有效性；Rust 由 `new_with_resume_options` 的 `local_data_available` 和实例字符串表达。当前 `BackendContextBuilder` 调用简化的 `new`，固定 `physical_id = 0`、`local_data_available = true`。
- Go `Close` 记录保存错误并停止后台循环，不向调用者返回错误；Rust `close` 返回 `Result`，成功后提供重复关闭幂等性。
- Go 的 `updateCheckpointImpl` 负责构造持久化快照并写系统存储；Rust 始终在内存中持有一份 `ReorgCheckpoint`，保存时克隆交给 storage。

因此，扩展 Rust 生产接线时应以 Go 的存储、授时和生命周期语义为目标补齐，而不能用当前内存实现代替持久化系统。

## 扩展指南

- 新增真实存储：实现 `CheckpointStorage`，在独立模块中处理事务、稳定序列化和上下文取消；不要把系统表或分布式任务细节塞进状态机。同步增加独立 Rust 测试，覆盖不存在、损坏、读写失败和物理表不匹配。
- 接入完整恢复上下文：优先让 `BackendContextBuilder` 调用 `new_with_resume_options` 或提供语义等价的构造参数，传入真实 `physical_id`、本地目录有效性和实例地址；同步测试 builder 到 backend 的接线。
- 补齐 TS 行为：若对齐 Go 的“下一次 ingest TS”，应在 import 成功边界接入授时抽象并验证单调性、授时失败不推进全局水位线；不要仅修改字段默认值。
- 引入周期保存：必须定义后台任务的启动、停止、取消、错误可见性和并发快照规则，并保证保存成功前不能清除 `dirty`。测试应放在独立 `*_test.rs` 文件，不嵌入生产源文件。
- 修改完成判定：同时审查 `TaskCheckpoint`、`after_flush`、`total_key_count` 及连续 ID 不变量，并同步 [`checkpoint_test.rs`](checkpoint_test.rs) 的乱序、末批、空末批和多批计数用例。
- 演进持久化格式：提升 `JOB_CHECKPOINT_VERSION_CURRENT` 前先定义旧版本读取、未知版本拒绝/降级策略，以及 Go/Rust JSON 或其他编码兼容性；`JobReorgMeta` 字段名也必须与现有持久化数据兼容。
- 变更关闭语义：同时检查 [`BackendContext::close`](backend.rs) 的资源链，避免只关闭 engines 而遗漏最终 checkpoint；持久化失败需要可观测且可重试。

主要风险是错误地信任已丢失的本地排序数据导致跳过回填、越过未完成任务导致数据缺失、过早推进全局水位线导致恢复错误，以及格式/TS 不兼容破坏 Go/Rust 交替运行。性能上应避免每个小批次同步持久化；当前设计仅在 import 或显式 close 保存。

## 验证依据

- 源码与模块：[`checkpoint.rs`](checkpoint.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。
- 直接 Rust 调用者：[`backend_mgr.rs`](backend_mgr.rs)、[`backend.rs`](backend.rs)、[`mock.rs`](mock.rs)。
- Rust 独立测试：[`checkpoint_test.rs`](checkpoint_test.rs) 覆盖持久化、local/global 分离、物理表/本地数据恢复、末批与批次数门槛、连续前缀、计数和未知任务；[`mock_test.rs`](mock_test.rs) 覆盖 backend/mock 转发契约。
- Go 对照：[`checkpoint.go`](checkpoint.go) 的 `CheckpointStorage`、`CheckpointManager`、`AdvanceWatermark`、`resumeOrInitCheckpoint`、`updateCheckpointLoop`；[`checkpoint_test.go`](checkpoint_test.go) 的 `TestCheckpointManager`、`TestCheckpointManagerUpdateReorg`、`TestCheckpointManagerResumeReorg`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ddl/ingest/checkpoint.rs` 确认目标文件含 30 个符号；`node --file ... --offset 1 --limit 500` 读取全部 354 行；对 `CheckpointManager`、`new_with_resume_options`、`advance_watermark`、`is_key_processed` 的查询用于消歧，直接调用边以调用者源码复核。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文档存在且恰有 11 个固定二级章节，并人工复核所有行为陈述均可回溯到上述源码或测试。
