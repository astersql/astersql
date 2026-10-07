# `pkg/ddl/schema_version.rs`

## 文件定位

`pkg/ddl/schema_version.rs` 属于 `astersql-ddl` crate（由 `pkg/ddl/Cargo.toml` 定义），并由 `pkg/ddl/lib.rs` 以 `pub mod schema_version` 对外公开。它位于 DDL job 推进与集群 InfoSchema 可见性之间：一边提供简化的 schema 差分和版本锁模型，另一边实现普通 DDL owner 的 schema 发布/恢复屏障。

需要特别区分两条路径：`SchemaVersionManager` 被 `pkg/ddl/job_worker.rs` 中较轻量的 `JobWorker::transit_one_job_step` 使用；`NormalDdlSchemaBarrier` 则服务于持久化 job 路径，由 `pkg/session/runtime/normal_ddl_service.rs::schema_barrier` 构造，通过 `pkg/ddl/table_mode.rs::NormalDdlExecutor` 接入 `pkg/ddl/job_scheduler.rs` 的调度循环。该文件不负责解析 SQL、持久化 DDL job 本身，也不实现 InfoSchema 加载器。

## 核心职责

1. 用 `SchemaAction`、`AffectedOption` 和 `SchemaDiff` 表达一次 DDL 对 schema/table 的差分摘要，并由 `set_schema_diff` 从简化 `crate::ddl::Job` 构造差分。
2. 用 `SchemaVersionManager` 在内存中单调分配版本，以 job ID 防止不同 job 交叉更新。
3. 用 `should_check_assumed_server` 决定 schema 同步是否应覆盖 NextGen/cross-keyspace 场景的“假定服务端”；匹配条件是非 Classic 内核且 table ID 位于系统保留区间。
4. 用 `NormalDdlSchemaBarrier` 在 DDL owner 身份和调度器取消边界内，恢复前任 owner 未完成的同步，发布全局 schema 版本，等待跟随者，并清理 MDL 记录和按 job 保存的 etcd schema-version key。
5. `wait_version_synced` 是一个纯内存判定辅助函数：所有观测版本均不小于目标才成功。当前生产的集群等待使用 `astersql_ddl_schemaver::Syncer::WaitVersionSynced`，未搜索到该辅助函数的文件外调用。

## 主要符号

- `SchemaAction`：差分动作枚举，包含创建/删除/截断/重命名表、分区变更、恢复与集群闪回等类别；`Default` 为 `Other`。它是 Rust 简化模型，并非 Go `model.ActionType` 的完整枚举复制。
- `AffectedOption { schema_id, table_id, old_schema_id, old_table_id }`：表达主对象之外的受影响对象，尤其支持重命名、恢复、分区和 placement cache 更新所需的新旧 ID。
- `SchemaDiff { version, action, schema_id, table_id, affected, regenerate_schema_map }`：简化 schema 差分。`regenerate_schema_map` 仅在 `RecoverSchema` 或 `FlashbackCluster` 中自动设为 `true`。
- `set_schema_diff(action, version, job, affected) -> SchemaDiff`：从 job 拷贝主 schema/table ID，保留调用方给出的受影响列表，并设置是否重建 schema-map。RustCodeGraph 的直接边为 `SchemaVersionManager::update -> set_schema_diff -> SchemaDiff`。
- `SchemaVersionManager { current, locked_by }`：两个字段均私有。`lock` 允许无锁状态或同一 job 重入；`update` 先加锁、再将 `current` 加一、最后构造差分；`unlock` 只能由当前 job 解锁；`current` 只读返回已分配上界。
- `RESERVED_GLOBAL_ID_UPPER_BOUND` / `RESERVED_GLOBAL_ID_LOWER_BOUND`：与 Go `pkg/meta/metadef` 的保留 ID 上界及“上界减 1000”下界对齐。判定区间是 `(lower, upper]`。
- `should_check_assumed_server_for_kernel(job, is_classic)`：可测的核心谓词。Windows 的公开包装使用实际 `IsClassic()` 并以 `debug_assert_eq!` 校验常量；非 Windows 精简构建按 Classic 处理，因而总返回 `false`。
- `wait_version_synced(target, observed)`：对任意 `IntoIterator<Item = i64>` 执行全称判定；空集合也会成功，这是 `Iterator::all` 的真空语义。
- `NormalDdlSchemaBarrier`：持有 `Arc<dyn Syncer>`、可取消 `Context`、可选 etcd client、snapshot/session 工厂闭包、MDL 开关、owner ID 和 NextGen 标记。私有 `check` 做 owner/取消栅栏，`clean_mdl` 做容错清理；`DdlSchemaBarrier::recover` 和 `wait` 是它在持久化调度链上的两个入口。

## 执行流程

简化内存路径如下：

1. `JobWorker::transit_one_job_step` 在 job 处于 `Running` 时调用 `SchemaVersionManager::update`。
2. `update` 以 `job.id` 获取锁；若锁被另一 job 占用则不改变版本并返回错误。
3. 加锁成功后 `current += 1`，再通过 `set_schema_diff` 返回差分。
4. `transit_one_job_step` 紧接着用同一 job ID 解锁；成功时将 job 转为 `Done`。因此锁约束的是单个提交步，不是 job 的整个生命周期。

持久化的普通 DDL 路径如下：

1. `NormalDdlService::schema_barrier` 绑定 owner epoch 与 cancellation，并提供真实 schema syncer、KV snapshot 和 SQL session pool。
2. `jobScheduler::schedule_persisted` 调用 `JobWorker::transit_persisted_job_step`；后者在启动交易前调用 `executor.recover`，避免前一 owner 已改 metadata 但未完成 schema 同步时直接重跑 job。
3. `recover` 先检查 owner 和取消状态。未启动 job 直接返回；MDL 模式从 `mysql.tidb_mdl_info` 读取 job 版本，等待同步后再清理；非 MDL 模式仅在 `last_schema_version > 0` 时从 snapshot 读取最新非空差分版本，并转入 `wait`。
4. job 步骤在真实 SQL/KV 交易中执行并提交；调度器随后用返回的 schema version 调用 `executor.wait_synced` / `NormalDdlSchemaBarrier::wait`。
5. `wait` 对版本 0 只执行 MDL 清理；对非 `Running | Rollingback | Done | RollbackDone` 状态不发布。其余情况先执行 `OwnerUpdateGlobalVersion`，再执行 `WaitVersionSynced(job.id, version, check_assumed_server)`，重新检查 owner/取消状态，最后清理 MDL。
6. job 后续转为 `Synced` 并写入历史；`pkg/session/runtime/normal_ddl_test.rs::crossks_align_normal_ddl_public_schema_barrier_waits_for_follower` 验证在 follower 达到新版本前 job 停留于 `Done`。

## 数据与状态

- `SchemaVersionManager::current` 初始为 0，只在成功获锁的 `update` 中递增；它是进程内状态，没有在本类型中持久化。
- `locked_by` 是 `Option<i64>`：`None` 表示未锁定，同一 job ID 可重复 `lock`，不同 job ID 会得到错误，非持有者的 `unlock` 不会改变状态。
- `SchemaDiff` 是值对象；`affected` 的所有权从调用者移入。本文件不把该简化差分写入 KV；Go 完整路径的 `updateSchemaVersion` 会生成更丰富的 `model.SchemaDiff` 并调用 `metaMut.SetSchemaDiff`。
- `NormalDdlSchemaBarrier` 本身不保存可变版本计数；其状态来源是 syncer/etcd、KV snapshot、`mysql.tidb_mdl_info` 和 job 中的 `state`、`last_schema_version`。
- MDL 清理对普通 schema 附带转义后的 `owner_id` 谓词，以避免删除其他 owner 记录；`mysql`、`sys`、`workload_schema` 等系统相关 schema 不附带该谓词。job 已是 `Synced` 且存在 etcd client 时，还会按 `DDLAllSchemaVersionsByJob/<job.id>/` 前缀枚举并删除 key。
- assumed-server 判定只检查 `job.table_id`，这与 Go 注释中 NextGen 禁止系统表多 table-ID DDL 的前提一致。

## 依赖与调用关系

上游调用者：

- `pkg/ddl/job_worker.rs::JobWorker::transit_one_job_step` 持有 `SchemaVersionManager`，在简化 `Running` job 步中调用 `update` 和 `unlock`。
- `pkg/session/runtime/normal_ddl_service.rs::schema_barrier` 是 `NormalDdlSchemaBarrier` 的真实构造入口；RustCodeGraph 记录了它对该结构的 `instantiates` 边。
- `pkg/ddl/table_mode.rs::NormalDdlExecutor` 通过 `DdlSchemaBarrier` trait 把 `recover` 和 `wait` 转接给持久化 worker。
- `pkg/ddl/job_scheduler.rs::schedule_persisted` 在每个已提交 job 步骤后调用 `executor.wait_synced`，同步失败时保留冲突依赖以待重试。

下游依赖：

- `crate::ddl::Job` 为简化差分/版本管理提供 `id`、`schema_id` 和 `table_id`。
- `astersql-ddl-schemaver` 提供 `Syncer`、`Context`、`EtcdClient` 和按 job 记录的 etcd 前缀；`pkg/ddl/Cargo.toml` 将它声明为本地 path 依赖。
- `astersql-meta` 的 `SnapshotReader::get_schema_version_with_non_empty_diff` 为非 MDL 恢复路径提供最新有效差分版本；`astersql-kv::Snapshot` 由上游工厂创建。
- `crate::job_worker::{JobLease, DurableJobSession}` 提供 owner/取消栅栏和 SQL 查询；`crate::table_mode::{DdlSchemaBarrier, is_system_related_schema}` 提供接口与系统 schema 分类。
- Windows 包装额外依赖 `astersql-meta-metadef` 和 `astersql-config-kerneltype`；非 Windows 分支不调用 kerneltype。

`pkg/ddl/lib.rs` 只将 `schema_version_test.rs` 作为独立测试模块纳入，没有把测试内嵌到生产文件。

## 错误处理与边界

- 版本锁冲突返回 `"schema version is locked"`；冲突检查发生在递增前，因此失败不消耗版本。
- `wait_version_synced` 遇到任一观测版本落后时返回 `"schema version {target} is not synced"`；它不重试、不等待且不处理节点隔离。
- `NormalDdlSchemaBarrier::check` 先拒绝非 owner，再拒绝 lease 取消或 context done，错误分别为 `"not DDL owner"` 和 `"DDL scheduler cancelled"`。`recover`/`wait` 在外部等待前后重复检查，防止旧 owner 在失去任期后继续推进。
- MDL 恢复只使用查询结果第一行第一列；空列或非 `i64` 文本会返回错误。如果查询无行，则不等待也不清理，只做最终 lease 检查。
- `OwnerUpdateGlobalVersion` 失败时：MDL 模式立即返回错误；非 MDL 模式若仅是 deadline exceeded 且 lease 仍有效，按 Go 语义成功返回；其他非 MDL 发布错误被记录，仍继续等待。显式取消或 owner 丢失不能使用 deadline 快速路径。
- `WaitVersionSynced` 错误会转为 `String` 并传播，不在屏障内吞掉。
- `clean_mdl` 的 session 获取失败会传播；但 MDL `DELETE`、etcd 前缀枚举和单 key 删除失败只输出诊断信息，整体仍返回成功。这是有意的容错清理边界，不应在无兼容性评估时改成致命错误。

## 并发与资源生命周期

- `SchemaVersionManager` 本身没有内部 `Mutex`或原子类型；它依赖 `&mut self` 的 Rust 独占借用和上层 worker 所有权保证进程内串行修改。`locked_by` 表达 DDL 语义锁，不是线程同步原语。
- `NormalDdlSchemaBarrier` 用 `Arc` 共享 syncer/etcd client，并用 `Send + Sync` 工厂延迟获取 snapshot 和池化 SQL session；它自身不启动 owner、不创建后台任务。
- `Context` 的 done 检查由 `NormalDdlService::schema_barrier` 同时绑定 scheduler cancellation、`owner.IsOwner()` 和 owner epoch。退休调度器必须取消该 context，才能中断正在进行的 syncer 等待。
- snapshot 和 SQL session 按次创建，以局部变量持有，函数返回时释放；本文件不缓存 transaction 或 session。
- 同步发生在 metadata/job 步骤提交之后。因此等待失败不等于回滚已提交的 schema 差分；`jobScheduler` 保留 pending/conflict 状态，下一轮通过 `recover` 补齐同步。

## 与 Go 版本的对应关系

- Go 对照文件是 `pkg/ddl/schema_version.go`。其 `updateSchemaVersion` 根据完整 `model.ActionType` 分派到多个 `SetSchemaDiffFor*`，处理截断、视图替换、多表重命名、分区、物化视图、恢复等专用字段，最后持久化差分。Rust `set_schema_diff` 只实现其公共骨架与 schema-map 标志，不应被视为已完整覆盖 Go 的所有专用分支。
- Go `waitVersionSynced` 使用 schema-version syncer、job ID、假定服务端标记，并包含 metrics、日志和 failpoint。Rust 纯函数 `wait_version_synced` 仅比较版本；真正对应集群协议的是 `NormalDdlSchemaBarrier::wait`。
- Go `waitVersionSyncedWithoutMDL` 会在 worker 重启后从 snapshot 读取最新非空差分并重新同步；Rust `recover` 的非 MDL 分支保留了这一语义。
- Go `shouldCheckAssumedServer` 在 Classic 内核总是 false，其他内核使用 `metadef.IsReservedID(job.TableID)`。Rust 测试 `pkg/ddl/schema_version_test.rs::test_should_check_assumed_server_matches_go_kernel_and_id_boundaries` 比 Go `pkg/ddl/schema_version_test.go::TestShouldCheckAssumedServer` 更细地覆盖了下界、下界加一、上界和上界加一。
- Rust 持久化屏障还承载 Go 普通 owner/MDL 路径的局部移植：owner 丢失栅栏、MDL 版本恢复、非 MDL 发布错误分类以及容错清理。`pkg/session/runtime/normal_ddl_test.rs` 的 owner-loss 与 follower-wait 测试是这条 Rust 路径的直接回归证据。

## 扩展指南

- 新增差分动作时，先判断是否只扩展简化路径。如需与 Go 生产语义对齐，应对照 `updateSchemaVersion` 对应 `SetSchemaDiffFor*` 分支，核对新旧 ID、多对象列表、schema-map 重建和持久化格式，不要只在 `SchemaAction` 中加枚举值。
- 修改版本锁时必须保留三个不变量：冲突不消耗版本，同 job 可重入，非持有者不能解锁。应在独立 `pkg/ddl/schema_version_test.rs` 增加针对 `SchemaVersionManager` 的测试，不要把 `#[cfg(test)]` 测试写入本生产文件。
- 修改 assumed-server 区间时，需同步核对 `pkg/meta/metadef`、Windows 的 debug assertion、Rust 五个边界样例和 Go `TestShouldCheckAssumedServer`。特别注意当前区间是左开右闭。
- 修改 `NormalDdlSchemaBarrier` 时，应同时追踪 `NormalDdlService::schema_barrier`、`NormalDdlExecutor`、`jobScheduler::schedule_persisted` 和 `JobWorker::transit_persisted_job_step`。任何新的阻塞操作都必须在前后检查 owner/取消状态，并评估“metadata 已提交、schema 尚未同步”的重试语义。
- MDL SQL 变更要保留 owner 隔离和系统 schema 例外，并在 `pkg/session/runtime/normal_ddl_test.rs` 中扩展真实持久化路径测试；纯谓词或内存差分则放在 `pkg/ddl/schema_version_test.rs`。
- 性能风险主要在同步等待、snapshot/session 获取和 etcd 前缀枚举；兼容性风险主要在 Go/Rust 差分字段和 job state 过滤；正确性风险主要在 owner 任期切换与 MDL 记录清理。

## 验证依据

本文档的事实依据如下：

- 目标源码：`pkg/ddl/schema_version.rs`，已通过 RustCodeGraph `node --file` 读取全部 366 行，并核对了枚举、结构、常量、条件编译函数、屏障实现及错误分支。
- RustCodeGraph 精确查询：`query SchemaVersion`、`query schema_version`、`query NormalDdlSchemaBarrier`、`query wait_version_synced`、`query set_schema_diff`、`query should_check_assumed_server`；`node set_schema_diff` 确认 `update -> set_schema_diff -> SchemaDiff` 边，`node schema_barrier` 确认 `normal_ddl_service.rs::schema_barrier -> NormalDdlSchemaBarrier` 实例化边。`explore` 未返回候选，方法符号的 `callers/callees` 亦未产生可用输出，因此按技能规则用精确符号查询和文本搜索补齐了方法调用边。
- crate 与模块边界：`pkg/ddl/Cargo.toml` 和 `pkg/ddl/lib.rs`；前者确认 `astersql-ddl`、`astersql-ddl-schemaver`、`astersql-meta`、`astersql-kv`、`astersql-meta-model` 等依赖，后者确认公开模块与独立测试模块。
- 上下游 Rust 源码：`pkg/ddl/job_worker.rs`、`pkg/ddl/table_mode.rs`、`pkg/ddl/job_scheduler.rs`、`pkg/session/runtime/normal_ddl_service.rs`，用于核对内存 worker 和持久化 owner 两条路径。
- Go 对照：`pkg/ddl/schema_version.go`，已核对 `updateSchemaVersion`、`waitVersionSynced`、`shouldCheckAssumedServer` 和 `waitVersionSyncedWithoutMDL`。
- 测试：`pkg/ddl/schema_version_test.rs` 与 `pkg/ddl/schema_version_test.go` 验证 kernel/ID 边界；`pkg/session/runtime/normal_ddl_test.rs::crossks_align_normal_ddl_owner_loss_rolls_back_table_diff_job_and_history`、`crossks_align_normal_ddl_public_schema_barrier_waits_for_follower` 和 `crossks_align_normal_ddl_registers_mdl_in_metadata_commit` 提供 owner 丢失、follower 等待和 MDL 注册的直接证据。
- 包级契约：`pkg/ddl/doc.go` 声明集群在推进下一步前必须使所有 TiDB 同步到 N+1；`docs/agents/ddl/README.md` 只作导航，本文的行为结论均回到代码和测试核对。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前的结构检查要求本文件存在且恰好含有上述 11 个固定二级标题。
