# `pkg/session/runtime/crossks_owner.rs`

## 文件定位

本文件属于 `astersql-session` crate 的 `runtime` 模块：`pkg/session/lib.rs` 公开 `runtime`，`pkg/session/runtime.rs` 再公开 `crossks_owner`。它实现目标 keyspace（target keyspace）运行时中的一个专用 DDL owner，负责从目标 keyspace 的 `mysql.tidb_ddl_job` 持久队列消费 `ACTION_ALTER_TABLE_MODE` 作业，并把终态写入 `mysql.tidb_ddl_history`。

生产装配入口是 `pkg/session/runtime/crossks_runtime.rs::CrossKSProductionRuntimeFactory::create_runtime`：该函数创建 `CrossKSSchemaSyncer`、`CrossKSJobSubmitter` 与 `CrossKSDdlOwner`，调用 `install_schema_syncer` 和 `start`，随后把 owner 同目标 `Domain`、同步器和最小 job ID 刷新循环一起放入 `SessionManager` 的生命周期列表。因而本文件不是通用 DDL 引擎，也不是作业提交器，而是跨 keyspace 运行时中面向 `AlterTableMode` 的持久队列执行端。

## 核心职责

- `CrossKSDdlOwner` 通过 `astersql_owner::Manager` 参与 DDL owner 选举，只有持有 owner 身份时才处理队列。
- `run` 轮询并唤醒消费循环；若发现不是 `ACTION_ALTER_TABLE_MODE` 的 DDL 作业，立即退出 owner 循环并释放共享 DDL 选举，使完整 DDL owner 可以接管该队列。
- `process_one` 按 `job_id` 顺序寻找可处理作业，`process_job` 负责加锁认领、解码、执行表模式变更、等待 schema 版本传播、写历史并删除活动队列记录。
- `history_job` 将持久化的 `ModelJob.state` 映射成跨 keyspace API 使用的 `HistoryJobState`，供提交方等待结果。
- `close` 协调停止标记、条件变量、工作线程和选举 runtime，保证运行时销毁时释放 owner 身份。

职责边界很窄：文件只实现 `AlterTableMode`；作业构造和入队位于 `crossks_job_submit.rs`，目标运行时组装与上层 `DdlBackend` 位于 `crossks_runtime.rs`，schema 同步细节位于 `crossks_schema.rs`，SQL session 租约位于 `crossks_session_pool.rs`。

## 主要符号

- `DDL_OWNER_KEY: &str`：复用 `astersql_ddl_util::DDLOwnerKey`，保证 owner 路径与 Go DDL 协议一致。`new` 的 mock 选举还把 `EtcdClient` 指针拼入 key，使同一内存客户端上的 owner 竞争、不同客户端隔离。
- `sql_blob(&[u8]) -> String`：把编码后的 job 字节转成 SQL 十六进制字面量 `x'...'`，用于更新或插入 `job_meta`。
- `sql_name(&str) -> String`：生成单引号 SQL 字符串并把内部单引号翻倍；用于历史表中的库名和表名。
- `model_mode(i64) -> Result<TableMode, String>`：只接受 `0/1/2`，分别映射 Normal、Import、Restore；其他值返回显式错误。
- `CrossKSDdlOwner`：唯一公开类型。持有目标 `Domain`、`CrossKSSessionPool`、节点 ID、可选 schema syncer、选举管理器、专用 Tokio runtime、条件变量、原子停止位和工作线程句柄。
- `new`：使用 `NewMockManager` 构造内存/测试式选举；`new_with_election` 接收生产共享选举管理器。二者都汇入私有 `new_inner`。
- `start`：创建双 worker Tokio runtime，阻塞执行 `CampaignOwner`，保存 runtime 后创建命名 OS 线程运行 `run`。线程创建失败时会先释放选举。
- `acquire_ownership`：读取 `ElectionManager::IsOwner`；返回 `Result<bool, String>` 以统一调用接口，当前底层读取本身不产生错误。
- `run`、`foreign_jobs_present`、`notify`：构成消费调度。`notify` 是低延迟提示，100 ms 超时轮询仍保证遗漏通知后可继续推进。
- `install_schema_syncer`：在启动前安装目标 keyspace schema 同步器；生产装配确实按此顺序调用。
- `process_one`、`process_job`：核心队列消费与单作业状态机。
- `history_job`：查询并解释历史 job 终态。
- `close` 与 `impl Lifecycle`：幂等停止，并适配跨 keyspace `Lifecycle` trait。

## 执行流程

1. `CrossKSProductionRuntimeFactory::create_runtime` 为真实 etcd 构造 `NewOwnerManager`，测试注入路径则使用 `CrossKSDdlOwner::new`；随后安装 `CrossKSSchemaSyncer` 并调用 `start`。
2. `start` 先创建异步选举所需的 Tokio runtime，再调用 `CampaignOwner`。成功后启动 `crossks-ddl-owner-{id}` 线程；因此不会在线程已启动但选举 runtime 未保存的半初始化状态下返回成功。
3. `run` 在未停止时检查 owner 身份。成为 owner 后先用 `foreign_jobs_present` 查询活动队列；发现任意其他 DDL 类型便跳出最外层循环，最终 `release_ownership`，避免这个专用实现占住 Go 共用的 DDL owner 锁。
4. 若队列类型可处理，`process_one` 再次验证 owner 身份，按 `job_id` 升序读取所有 `ACTION_ALTER_TABLE_MODE` ID，并依次尝试 `process_job`。处理成功一个即返回 `true` 促使循环立即继续；无可认领作业返回 `false`；错误会让本轮退出到 100 ms 等待。
5. `process_job` 开启悲观事务并以 `FOR UPDATE` 重新读取 `job_meta`。记录不存在、类型改变或状态为 `Paused` 时提交事务但返回未处理；否则保留 `Cancelling`，或把状态改为 `Running`，设置 `processing = 1` 并写回编码后的 job 后提交。
6. 对 `Cancelling` 作业直接产生取消错误，不改变表。其他作业解析 `raw_args.table_mode`，通过 `model_mode` 校验，再调用 `Domain::ddl_set_table_mode_by_ids(schema_id, table_id, mode)`。
7. 变更成功且已安装同步器时，先 `publish_global`，再以 90 秒上限调用 `wait_all_versions_with_cancel`；停止原子量作为取消信号。只有执行及同步都成功时 job 进入 `Synced`，否则进入 `Cancelled`，记录错误并递增 `error_count`。
8. 最终状态编码后，在第二个悲观事务中插入 `tidb_ddl_history` 并删除 `tidb_ddl_job`，然后提交。持久化失败会尝试回滚；业务执行失败则在历史迁移成功后仍向调用循环返回带 job ID 的错误。
9. 上层 `CrossKSProductionDdlBackend::notify_owner` 先写 etcd 通知键，再调用本文件的 `notify`；`DdlClient` 通过 backend 的 `history_job` 轮询最终结果。

## 数据与状态

`CrossKSDdlOwner` 的长期状态均围绕单个目标 keyspace 运行时：

- `domain: Arc<Domain>` 是实际修改表元数据并查询 infoschema 的目标 Domain。
- `pool: Arc<CrossKSSessionPool>` 提供访问目标系统表的 `CrossKSSessionLease`；租约随局部变量释放回池。
- `schema: Mutex<Option<Arc<CrossKSSchemaSyncer>>>` 允许构造与安装分离。未安装时，成功作业不会执行全局 schema 发布/等待；生产装配始终安装，直接测试可选择不安装。
- `election` 与 `election_runtime` 共同维持 owner claim。runtime 存在表示选举协议仍可执行 `Close`；`release_ownership` 以 `take` 保证只关闭一次。
- `wake: (Mutex<bool>, Condvar)` 中布尔值仅表示待唤醒提示，不代表队列真值；队列真值始终来自 SQL 系统表。等待后将其清零。
- `stopped: AtomicBool` 使用 Acquire/AcqRel 顺序，既驱动线程退出，也传给 schema 等待作为取消条件。
- `thread` 保存唯一工作线程句柄，`close` 取出并 join。

持久作业状态分两阶段：第一事务将活动 job 标记 `Running`/`processing=1`；执行与 schema 同步在事务外完成；第二事务把终态 job 插入历史并从活动表删除。源码注释给出的恢复不变量是“设置同一 table mode 具有幂等性”，因此在第一事务提交后崩溃，由后继 owner 重放是安全的。

## 依赖与调用关系

上游关系：

- `crossks_runtime.rs::create_runtime` 创建、配置、启动并托管 `CrossKSDdlOwner`。
- `CrossKSProductionDdlBackend::notify_owner` 调用 `notify`；`CrossKSProductionDdlBackend::history_job` 调用同名查询方法。
- `Lifecycle::close` 由 `astersql_domain_crossks::SessionManager` 的生命周期清理链调用。
- `crossks_owner_test.rs` 直接调用构造、启动、owner 检查、历史查询和关闭方法，验证其完整行为。

下游关系：

- `astersql_owner::Manager::{CampaignOwner,IsOwner,Close}` 提供与共享 DDL owner 键兼容的选举协议。
- `CrossKSSessionPool::acquire` 与 `CrossKSSessionLease::query` 承担所有系统表事务和查询。
- `astersql_meta_model::group_3::Job::{decode,encode}` 读取和保存 Go 兼容的 DDL job 格式；`JobState` 表示运行、暂停、取消及同步终态。
- `Domain::ddl_set_table_mode_by_ids` 执行实际目标表模式变更。
- `CrossKSSchemaSyncer::{publish_global,wait_all_versions_with_cancel}` 传播并等待 schema 版本。

`pkg/session/Cargo.toml` 将该实现归入 `astersql-session`，直接声明了 `astersql-domain`、`astersql-domain-crossks`、`astersql-domain-serverinfo`、`astersql-ddl-util`、`astersql-meta-model`、`astersql-owner`、`serde_json` 和启用 `rt-multi-thread` 的 `tokio`；本文件没有条件编译项，只有测试模块在 `runtime.rs` 中以 `#[cfg(test)]` 接入。

## 错误处理与边界

- 所有外部失败被压成带上下文的 `String`；`Lifecycle` 边界再包装/适配为 `ManagerError`。选举 runtime、campaign 和线程创建分别给出不同错误前缀。
- SQL 辅助函数只在内部生成 job blob、库名和表名；数值 `job_id/schema_id/table_id` 来自解码后的整数或数据库解析。`sql_name` 处理单引号，`sql_blob` 不插入原始文本。
- `model_mode` 拒绝未知枚举值；缺少或非整数 `table_mode` 返回 `AlterTableMode job has no table_mode`。
- 无 owner 身份时 `process_one` 明确失败，不会静默消费。活动记录消失、暂停或类型变化则视为本次未处理。
- 第一事务中发生错误会尝试 `ROLLBACK`；但 `COMMIT` 自身失败由 `?` 直接返回，代码不会再显式回滚。第二事务在插入、删除或提交失败时尝试回滚。
- 业务失败不是“留在活动队列重试”：代码把 job 标为 `Cancelled`、写入历史并删除活动记录，随后向当前循环返回错误；`history_job` 将 `Cancelled`/`RollbackDone` 映射成 `Failed`。
- schema 发布或 90 秒等待失败同样使 job 终态为 `Cancelled`。关闭期间等待可由 `stopped` 取消。
- `run` 对 `foreign_jobs_present` 的错误按“未确认有外来 job”处理，随后 `process_one`；对 `process_one` 错误不终止线程，只回到短暂等待后重试。
- 互斥锁中毒使用 `expect`，属于进程内不变量破坏，会 panic 而非返回业务错误；工作线程 join 的 panic 结果在 `close` 中被忽略。

## 并发与资源生命周期

每个 owner 实例最多有一个工作线程和一个专用 Tokio runtime。`start` 没有显式防止重复调用，因此调用者必须遵守“一次启动”的生命周期约束；生产 `create_runtime` 只调用一次。选举状态由外部 manager 管理，消费前在 `run` 和 `process_one` 两处检查，从而缩小失去 owner 后继续处理的窗口，但代码没有在 `process_job` 每个阶段重新检查租约。

`notify` 在持锁时设置提示并广播；`run` 以 100 ms 超时等待，所以通知可能合并且无需逐条计数。SQL 的 `FOR UPDATE` 与悲观事务是多 owner/多尝试下认领同一 job 的主要串行化手段。表模式应用不持有 SQL 事务，依赖操作幂等及持久状态在崩溃后恢复。

`close` 先用 `AtomicBool::swap` 实现幂等；首次关闭唤醒线程、join 等待 `run` 完成，再调用 `release_ownership`。`run` 本身退出时也释放选举，而 `take` 避免双重关闭。生产初始化失败路径也显式关闭已创建 owner；正常路径由 `SessionManager` 的 `Lifecycle` 列表清理。`Arc` 保证工作线程和其他 backend 引用期间对象存活。

## 与 Go 版本的对应关系

Go 中不存在与 `pkg/session/runtime/crossks_owner.rs` 一一对应的同路径文件。当前 Go 路径 `pkg/domain/crossks/cross_ks.go::createSessionManager` 组装目标 store、session pool、schema/version syncer、系统表 manager 和 `ddlClient`，但不创建一个只消费 `AlterTableMode` 的本地 owner；`pkg/domain/crossks/ddl_submit.go::ddlClient.alterTableMode` 构建并提交 job、通过 etcd 通知 DDL owner，然后等待历史结果，真正消费仍由完整 Go DDL owner 完成。

Rust 文件保留的 Go 兼容契约包括：共享 `DDLOwnerKey`/生产选举协议、`mysql.tidb_ddl_job` 与 `mysql.tidb_ddl_history` 表、`meta/model.Job` 编码和 `JobState`、`ACTION_ALTER_TABLE_MODE` 类型、取消不应用表变更、成功后发布并等待 schema 版本，以及通过历史 job 向提交端返回终态。Rust 为尚未移植完整 DDL owner 的运行时增加了专用消费器；因此遇到其他 DDL 类型会主动让权，而不是尝试用简化逻辑执行它。

独立 Rust 测试名称中的 `go_merge_43` 记录了该移植批次意图：测试覆盖成功完成并移入历史、共享选举交接、mock 选举关闭释放、失败 job 记录、Cancelling job 不改变表，以及非 `AlterTableMode` 作业触发让权。这些测试比“实现应当如何”更直接地界定当前与 Go 语义对齐的范围。

## 扩展指南

- 若扩展新的 DDL 类型，不能只放宽 `foreign_jobs_present` 的过滤条件；必须同时扩展 job 参数解码、实际执行、状态迁移、幂等/崩溃恢复、schema 同步和历史结果语义，并在独立测试文件中覆盖。更安全的方向通常是接入完整 DDL owner，而不是继续扩大这个专用执行器。
- 修改认领协议时重点审查 `process_one` 和 `process_job` 的两个事务边界、`FOR UPDATE`、`processing` 字段及“应用成功但历史迁移前崩溃”的恢复行为。
- 修改 table mode 枚举时同步更新 `model_mode`、`astersql_meta_model::TableMode`/Go model 编码和 `crossks_owner_test.rs` 的非法值与每种模式用例，避免 Rust/Go 数值协议漂移。
- 修改 owner 通知或选举路径时同步检查 `crossks_runtime.rs::{create_runtime,CrossKSProductionDdlBackend::notify_owner}`、`astersql_owner` 和 `astersql_ddl_util::DDLOwnerKey`，并保持与 Go etcd 键兼容。
- 修改 schema 等待策略时同步检查 `crossks_schema.rs`；应覆盖发布失败、等待超时及关闭取消，评估 90 秒等待对关闭延迟和提交端可见延迟的影响。
- 新测试应继续放在独立的 `pkg/session/runtime/crossks_owner_test.rs`，不要内嵌进生产文件。至少保留成功、失败、暂停/取消、双 owner 竞争、失去 owner、外来 DDL、事务失败和关闭中的等待等边界。
- 性能风险主要在每轮扫描全部匹配 job ID、每个 job 多次 SQL 往返、100 ms 空闲轮询以及 schema 全节点等待；兼容风险主要在 job 编码、系统表事务语义和 owner key；正确性风险主要在租约丢失窗口与两阶段持久化恢复。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/session/runtime` 确认目标文件被索引且含 37 个符号。
- RustCodeGraph `node --file pkg/session/runtime/crossks_owner.rs --offset 1 --limit 420`：读取本文件全部 349 行，核对结构体、辅助函数、所有 impl 与状态机。
- RustCodeGraph `query CrossKSDdlOwner`、`query process_one --kind function --json`：分别定位结构体、`crossks_runtime.rs::new` 接线候选、测试使用者及核心方法；`callers/callees` 查询在本地索引上超时且未返回边，因此没有把缺失输出当成调用证据。
- RustCodeGraph `node --file pkg/session/runtime/crossks_runtime.rs`：核对 `create_runtime` 的创建/启动/清理，以及 backend 到 `notify`、`history_job` 的直接调用。
- RustCodeGraph `node --file pkg/session/runtime/crossks_owner_test.rs`：核对六项独立测试及其实际断言；本任务是纯文档分析，按计划不运行 Cargo。
- `pkg/session/Cargo.toml`、`pkg/session/lib.rs`、`pkg/session/runtime.rs`：核对 crate、依赖、公开模块和独立测试模块边界。
- `pkg/domain/crossks/cross_ks.go::createSessionManager` 与 `pkg/domain/crossks/ddl_submit.go::ddlClient.alterTableMode`：核对 Go 运行时只提交、通知并等待完整 DDL owner 的对应语义，以及不存在一一对应专用 owner 的架构差异。

人工复核结论：本文能从生产装配、选举、队列认领、实际执行、schema 传播、历史查询和关闭清理解释“文件为何存在、如何运行、如何安全扩展”；所有“已支持”陈述均落到上述源码符号或独立测试，没有把预期架构写成当前事实。
