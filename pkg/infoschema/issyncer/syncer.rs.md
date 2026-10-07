# `pkg/infoschema/issyncer/syncer.rs`

## 文件定位

本文件实现 `astersql-infoschema-issyncer` crate 对外的 InfoSchema 同步编排器 `Syncer`。crate 根 `pkg/infoschema/issyncer/lib.rs` 以 `mod syncer` 装配本模块，并用 `pub use syncer::*` 导出其构造函数、trait 和方法；`Cargo.toml` 的 `[lib] path = "lib.rs"` 以及 `[package.metadata.porting] go-package = "pkg/infoschema/issyncer"` 明确了 crate 边界和 Go 移植来源。

它位于“存储中的 schema 元数据”与“会话可见的最新 InfoSchema”之间：`Loader` 负责读取、增量/全量构建及缓存，本文件负责何时加载、如何续租 `SchemaValidator`、怎样发布本节点 schema 版本，以及怎样在 MDL（Metadata Lock）条件满足后按 DDL job 发布版本。生产接线可见 `pkg/session/runtime/session_factory.rs::prepare_normal_schema_runtime`，后台线程入口可见 `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaRuntime::start`。

## 核心职责

1. 通过 `New`、`NewCrossKSSyncer` 和内部 `newSyncer` 组装普通或跨 keyspace 的 `Loader`、validator、系统会话池及同步状态。
2. 通过 `ReloadWithContext` 串行加载最新 schema，处理 flashback 期间的可用时间戳回退，向版本协议发布新版本，更新或重置 validator，并执行 reload 后动作。
3. 通过 `SyncLoop` 响应全局 schema 版本通知和半租约周期定时器；版本同步器失效时先停止 validator，再重启协议、反复 reload，最后恢复 validator。
4. 通过 `RefreshMDLFromSQL`、`CheckMDL` 和 `MDLCheckLoop` 将 `mysql.tidb_mdl_info` 的待处理 job 转换为版本发布；跨 KS 模式只保留涉及系统保留表 ID 的 job。
5. 暴露 `LoadWithTS`、`InfoSchema`、`GetSchemaValidator`、`ChangeSchemaCacheSize` 等薄委托，维持 Loader 与上层 Domain/DDL 的边界。

本文件不实现元数据读取和 schema diff 应用；这些逻辑在同 crate 的 `loader.rs`。它也不实现 etcd/内存版本协议，而是依赖 `astersql_ddl_schemaver::Syncer` trait。

## 主要符号

- `MDLSessionPool::ReadMDLRows(min_job, version)`：系统 SQL 边界。实现方负责借用会话、执行必要的 rollback、读取 MDL 行并正确归还或销毁资源；本文件只消费按 job ID 索引的 `JobMDL`。
- `InfoSchemaCoordinator`：向会话协调层提供两个回调。`CheckOldRunningTxn` 会从候选 job 中剔除仍被旧事务阻挡者；`KillNonFlashbackClusterConn` 用于 flashback cluster 动作后的连接清理。
- `SchemaValidator`：抽象 validator 的 `Update`、`Reset`、`Stop`、`Restart` 生命周期。文件末尾为 `astersql_infoschema_isvalidator::Validator` 提供适配实现，并把 `RelatedSchemaChange` 转换成下游类型。
- `MDLProgress`：受互斥锁保护的循环状态。`last_version` 避免无变化且无待重试 job 时重复工作，`pending` 表示仍需再次检查，`published` 缓存每个 job 已成功发布的最高版本。
- `Syncer`：主状态对象。关键字段包括 `loader`、串行 reload 的 `reload_lock`、`schemaLease`、可选的系统会话池/协调器/版本同步器/min-job 刷新器、MDL 快照与唤醒条件变量、延迟回调队列、validator 及 `crossKS` 模式位。
- `New` / `NewCrossKSSyncer` / `newSyncer`：普通模式把 `DeferFn` 和可选 `Filter` 交给 `newLoader`；跨 KS 模式优先调用 `NewLoaderForCrossKS`，不传 Filter，并设置 `crossKS = true`。`_targetKS` 当前只为签名对齐而未用于 Rust 内部日志上下文。
- `InitRequiredFields`：后台循环启动前注入 coordinator getter 和版本同步器，并同步全局 MDL/NextGen 协议开关。`SetMinJobIDRefresher` 再注入 SQL 查询下界来源。
- `RefreshMDLFromSQL`：要求已经加载 schema 且三个运行期依赖均已设置，读取 `[current_min_job_id, current_schema_version]` 范围内的 MDL job，然后刷新快照。
- `CheckMDL` / `check_mdl_with_context`：执行单次、可确定性测试的 MDL 检查；对可推进 job 调用 `VersionSyncer::UpdateSelfVersion(context, job_id, version)`。
- `MDLCheckLoop`：最多等待 50ms 或显式唤醒；仅在 MDL 开启时检查，单次失败打印错误并继续循环，直到 `Context` 取消。
- `SyncLoop`：InfoSchema 主同步循环。监听 `GlobalVersionCh`、同步器 `Done` 和半租约定时点，并在每次 reload/recovery 路径后刷新 MDL SQL 快照、唤醒 MDL 检查线程。
- `ReloadWithContext`：可取消的核心 reload；`Reload` 只是以后台 context 调用它。
- `postReload`：schema 版本确有变化且存在 change 时，动作码 28 删除 Loader 的表缓存，动作码 62 请求协调器杀死非 flashback 连接。
- `getFlashbackStartTSFromErrorMsg`：只接受恰好一个固定 marker 且后缀能直接解析为 `u64` 的消息，否则返回 0。

## 执行流程

生产初始化流程如下：

1. `prepare_normal_schema_runtime` 创建真实 `KvSchemaStore`、共享 `InfoCache`、系统会话池和 validator，调用 `New`。
2. 同一入口调用 `InitRequiredFields`、`SetMinJobIDRefresher`，先初始化 schema-version 协议，再调用 `ReloadWithContext` 完成首次加载；因此后台循环正常运行所需字段在启动前已具备。
3. `NormalSchemaRuntime::start` 分别启动 `SyncLoop`、`MDLCheckLoop`、job schema-version 同步循环和 min-job-id 刷新循环。
4. `SyncLoop` 收到全局版本通知或到达 `schemaLease / 2` 时调用 `ReloadWithContext`。若通知通道断开，它重新注册 watch；若版本同步器的 `Done` 已关闭，则先 `SchemaValidator::Stop`，每秒重试 `Restart`，每 200ms 重试 reload，成功后以当前版本 `Restart` validator 并取得新通知通道。
5. 每轮同步后，循环检查到期的 `DeferFn`（仅到半租约时点）、从 SQL 刷新 MDL job，并通过 `Condvar` 唤醒 `MDLCheckLoop`。

一次 reload 的顺序是：先检查 context；配置协议开关；持有 `reload_lock`；取得 Loader 当前存储版本；调用 `LoadWithTS`。若错误包含合法 flashback start TS，则以 `start_ts - 1` 再加载一次。非缓存命中且版本前进时尽力发布 job 0 的节点版本，发布失败只记录而不使 reload 失败；非缓存命中的全量加载会 `Reset` validator。若加载耗时超过半租约，则读取更新的存储时间戳，并仅在该时间戳对应的 schema 版本仍等于刚加载版本时用它续租。最后无论是否命中缓存都调用 validator `Update`，再执行 `postReload`。

一次 MDL 检查先复制 `(newest_version, jobs)` 快照，再在 `mdl_progress` 锁内判断是否需要工作。协调器可能从副本中移除被旧事务阻塞的 job；剩余 job 才能发布。成功发布会写入 `published` 去重，任一失败会设置 `pending` 并在处理完其他 job 后返回最后一个发布错误，因此单个 job 失败不会阻止其他 job 尝试发布。缓存超过 1000 项时整体清空，后续可安全地重新发布。

## 数据与状态

- `loader` 是 schema 数据的权威操作入口；`InfoSchema()` 返回它的最新缓存，`LoadWithTS` 和缓存容量修改均直接委托给它。
- `reload_lock: Mutex<()>` 保证同一 `Syncer` 同时只有一个 reload；锁覆盖版本读取、加载、慢加载补偿、validator 更新及 `postReload`，从而维持一次状态切换的顺序。
- `mdlCheckTableInfo` 自身提供 `replace/snapshot/contains`；本文件不在持有其内部锁时调用协调器或网络协议，而是用快照工作。
- `mdl_progress` 把“最近观察版本、是否需重试、已发布 job 版本”作为一个原子临界区维护。当前实现会在持有该 mutex 时调用 coordinator 和 `UpdateSelfVersion`，因此这些回调/网络操作会串行化所有 `CheckMDL` 调用。
- `mdl_wake` 是 `(Mutex<bool>, Condvar)`：`SyncLoop` 写 true 并 `notify_one`，检查循环取出后立即清 false；布尔位避免通知在检查线程尚未等待时完全丢失。
- `schemaLease` 以构造参数毫秒数保存。构造允许 0 以支持局部测试，但 `SyncLoop` 明确拒绝零租约；直接 `Reload` 仍可在零租约下工作。
- `crossKS` 只影响 Loader 构造和 MDL job 过滤。跨 KS 下，空表集合或全部为用户表 ID 都会跳过；只要包含任一 `metadef::IsReservedID`，该 job 就保留。

## 依赖与调用关系

上游直接证据：

- `pkg/session/runtime/session_factory.rs::prepare_normal_schema_runtime` 调用 `schema::New`，注入 coordinator、protocol、min-job refresher，执行协议 `Init` 和首次 `ReloadWithContext`。
- `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaRuntime::start` 在线程 `normal-schema-sync` 与 `normal-mdl-check` 中分别调用 `SyncLoop`、`MDLCheckLoop`；其 `SchemaLoader::reload` 实现把 DDL 请求转发给 `ReloadWithContext`。
- `pkg/session/runtime/session_factory.rs` 的目标 keyspace 路径调用 `NewCrossKSSyncer`，证明跨 KS 构造器不是仅测试 API。

主要下游依赖：

- crate 内 `Loader` / `InfoCache` / `MDLCheckTableInfo` / `DeferFn` 提供加载、缓存、快照和延迟释放。
- `astersql-ddl-schemaver` 提供可取消 `Context` 与 `VersionSyncer` 协议，包括全局 watch、节点/作业版本发布、Done、Restart。
- `astersql-ddl-systable::MinJobIdRefresher` 提供 MDL SQL 扫描的最小 job ID。
- `astersql-sessionctx-vardef` 和 `astersql-config-kerneltype` 决定 MDL 与 NextGen 协议开关。
- `astersql-infoschema-isvalidator` 是 `SchemaValidator` 的生产实现，`metadef` 判定系统保留表 ID。

RustCodeGraph 的文件节点报告本文件被 12 个文件引用，并能给出上述生产文件；但当前索引未把 Rust `impl Syncer` 方法作为可由 `query --kind method` 精确定位的独立节点，因此方法级 callers/callees 由索引文件源码与 `rg` 的精确调用点共同核验，而非宣称存在完整静态调用图。

## 错误处理与边界

- 未注入版本同步器、未加载 schema、未注入会话池或 min-job refresher 时，相关入口返回描述性的 `SyncError`；构造成功不等于后台运行依赖已完整。
- `SyncLoop` 要求正租约，否则立即返回错误，防止半租约间隔为零造成忙循环。
- context 在 `ReloadWithContext` 入口即检查；版本协议恢复和 reload 重试也通过 `Context::Wait` 响应取消。单次正常 reload 中，Loader 已开始后没有再次抢占式取消，context 主要约束版本发布和循环生命期。
- flashback 回退只在错误文本精确匹配固定 marker 且 TS 可解析时触发；TS 为 0 视为不匹配。代码随后计算 `flashback - 1`，由于 0 已被排除，不发生无符号下溢。
- job 0 的普通 schema 版本发布是 best effort，失败打印后仍更新 validator；MDL job 发布错误则由 `CheckMDL` 返回，并保留 `pending` 以便循环重试。
- `Mutex::lock`、`Condvar::wait_timeout` 使用 `unwrap`，锁中发生 panic 后的 poisoning 会使后续调用 panic；这是当前实现边界，而非可恢复 `SyncError`。
- `postReload` 通过并行迭代 `PhyTblIDS` 与 `ActionTypes`，长度不一致时只处理较短一侧；其动作码 28/62 是与 Go `ActionUnlockTable`/`ActionFlashbackCluster` 的数值契约。
- Go 版本记录 SQL 获取失败并继续；Rust `RefreshMDLFromSQL` 将错误返回给调用方，而 `SyncLoop` 记录后继续。直接调用者需要自行决定是否重试。

## 并发与资源生命周期

`Syncer` 通过 `Arc` 由 `NormalSchemaRuntime` 共享给两个 OS 线程。`SyncLoop` 是 schema reload 与 MDL 快照刷新生产者，`MDLCheckLoop` 是 MDL 快照消费者；二者通过线程安全字段和条件变量协调。`Context` 是统一关闭信号：runtime `close` 先取消 context 和 min-job refresher，再 join 全部线程，随后关闭版本协议、停止 validator、关闭会话池（见 `NormalSchemaRuntime::close`），避免后台线程继续使用已释放资源。

reload 由 `reload_lock` 串行化；MDL 快照由 `MDLCheckTableInfo` 自身锁保护；检查进度由另一把 mutex 保护。代码没有同时嵌套持有 reload 锁和 MDL progress 锁，但 `check_mdl_with_context` 会持有 progress 锁跨越外部 coordinator 与版本发布调用，扩展时不能在这些回调中反向等待同一 Syncer 的 MDL 检查，否则可能造成阻塞。

`MDLSessionPool` 的资源借还规则属于 trait 实现方；Go 对照明确要求不用的 session 必须 Destroy/Close，Rust 注释也要求实现执行 rollback 后正确返回或销毁资源。`DeferFn` 仅由普通构造路径与 Loader 共享，跨 KS Loader 不使用该 filter/defer 接线；它由 `SyncLoop` 在半租约时点调用 `check`。

## 与 Go 版本的对应关系

`pkg/infoschema/issyncer/syncer.go` 是直接语义基准。Rust 保留了普通/跨 KS 两类构造、MDL 表过滤、全局版本 watch、同步器断线恢复、半租约 reload、慢加载续租补偿、flashback TS 回退、validator 生命周期、unlock-table 缓存删除和 flashback-cluster 连接清理。

需要注意的实现差异：

- Go `Syncer` 直接持有 store/logger/channel；Rust 把存储访问集中到 `Loader`，用 `Condvar + bool` 代替 `mdlCheckCh`，当前只用 `eprintln!` 记录循环错误。
- Go `refreshMDLCheckTableInfo` 在文件内显式借用系统 session、rollback、执行 SQL；Rust 把这些细节抽象为 `MDLSessionPool::ReadMDLRows`，便于由生产会话池实现并在单元测试替换。
- Go `InitRequiredFields` 还给 Loader 注入 autoid client 和系统执行器工厂；当前 Rust 签名只注入 coordinator 与版本同步器，Loader 的生产依赖在构造时由 `SchemaStore`/`InfoCache` 提供。
- Go ticker 与 channel 直接 select；Rust 每次最多等待 50ms 轮询 context/watch，并以 `Instant` 管理半租约期限。
- Go 的 restart/reload 辅助函数独立存在；Rust 将相同重试策略内联进 `SyncLoop`，重试间隔仍分别为 1 秒和 200ms。
- Rust 增加 `ReloadWithContext`、`CheckMDL` 等可取消或单步入口，供服务生命周期和确定性测试使用；Go `Reload`/循环主要使用 background context。
- Go 还有 `FetchAllSchemasWithTables`，当前 Rust `Syncer` 未暴露同名方法；不能从本文件推断该 API 已对齐。

测试对照方面，Go `syncer_test.go::TestSyncerSkipMDLCheck` 只覆盖跨 KS 过滤。Rust 独立测试 `syncer_test.rs` 保留该分支并额外覆盖 flashback 文本解析、真实 KV 元数据 reload、版本发布、validator 租约更新与同步器恢复；`pkg/session/tests/system_session.rs` 再以真实系统 SQL 路径验证旧事务阻挡、部分发布失败及循环重试。

## 扩展指南

- 新增 reload 后动作应接入 `postReload`，同时核对 `RelatedSchemaChange` 两个数组的配对契约、Go action 数值及 Loader/连接管理副作用；不要把动作逻辑塞进 Loader。
- 新增 schema 加载或租约规则应修改 `ReloadWithContext`，保持“串行加载—可选版本发布—validator 更新—post action”的顺序，并为缓存命中、全量/增量、慢加载、flashback 与取消分别补测试。
- 调整 MDL 协议应优先修改 `RefreshMDLFromSQL`、`check_mdl_with_context` 或 `skipMDLCheck`。网络调用不能导致已成功 job 因另一 job 失败而丢失；必须保留 `pending` 重试和按 `(job_id, version)` 去重语义。
- 添加运行期必需依赖时，应在 `InitRequiredFields` 或明确的 setter 接入，并在 `version_syncer` 类似的访问点返回可诊断错误；同时更新 `prepare_normal_schema_runtime`，确保线程启动前完成注入。
- 并发修改需审视 `reload_lock`、`mdl_progress` 和 `MDLCheckTableInfo` 的锁顺序，避免在持锁期间加入可能回调 Syncer 的操作；若缩短锁范围，要保证并发 `CheckMDL` 不会重复破坏发布进度。
- Rust 单元测试继续放在独立 `pkg/infoschema/issyncer/syncer_test.rs`，不要嵌入生产文件。跨模块生产路径可扩展 `pkg/session/tests/system_session.rs` 或同目录 runtime 测试；Go 语义变化时同步检查 `syncer.go` 与 `syncer_test.go`。
- 若增加 crate 依赖或改变装配，更新 `pkg/infoschema/issyncer/Cargo.toml` 和相应 Bazel 元数据；本文件现有直接依赖可由 manifest 的常规 `[dependencies]` 复核，`cfg(any())` 区仅保留未启用的迁移声明，不能作为当前可执行依赖证据。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/infoschema/issyncer` 确认目标、Go 对照和独立测试均已索引。
- 目标源码：`pkg/infoschema/issyncer/syncer.rs` 全部 547 行；主要证据为 `Syncer`、`newSyncer`、`InitRequiredFields`、`RefreshMDLFromSQL`、`check_mdl_with_context`、`MDLCheckLoop`、`SyncLoop`、`ReloadWithContext`、`postReload`、`getFlashbackStartTSFromErrorMsg`。
- crate 与装配：`pkg/infoschema/issyncer/Cargo.toml`、`pkg/infoschema/issyncer/lib.rs`。该目录不存在 `doc.go`，故无额外 package contract 文件可读。
- 生产上游：`pkg/session/runtime/session_factory.rs::prepare_normal_schema_runtime`、`pkg/session/runtime/normal_ddl_service.rs::NormalSchemaRuntime::{start,close}` 与其 `SchemaLoader::reload` 实现。
- Go 对照：`pkg/infoschema/issyncer/syncer.go` 全部 548 行；测试 `pkg/infoschema/issyncer/syncer_test.go::TestSyncerSkipMDLCheck`。
- Rust 测试：`pkg/infoschema/issyncer/syncer_test.rs` 全部 265 行；跨 crate 集成证据 `pkg/session/tests/system_session.rs` 的 MDL SQL/发布测试段。
- RustCodeGraph `explore` 给出了 `configure_protocol`、`version_syncer`、`ReloadWithContext`、`SyncLoop`、`MDLCheckLoop` 等文件内调用关系及文件引用集合；精确 `query --kind method` 未返回 Rust impl 方法节点，故用索引源码节点和精确调用点搜索补证，并在文档中明确此限制。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前以任务指定命令验证本文恰有 11 个固定二级章节，并人工检查没有把测试文件内嵌进生产源码的建议。
