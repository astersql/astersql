# [`pkg/dxf/importinto/scheduler.rs`](scheduler.rs)

## 文件定位

本文件是 `astersql-dxf-importinto` crate 的分布式 IMPORT INTO 调度层。crate 由同目录 `Cargo.toml` 定义、由 `lib.rs` 的 `pub mod scheduler; pub use scheduler::*;` 暴露。当前仓库可检索到的工厂装配点位于 `pkg/session/runtime_test/ddl.rs`：它用 `ImportSchedulerServices::FromEncodeRuntime` 构造服务并调用 `RegisterImportSchedulerFactoryWithServices`；尚未从直接 Rust 引用中验证另一处生产启动装配。注册后，DXF 通用 `BaseScheduler` 通过 `framework_scheduler::Extension` 动态分派到 `ImportSchedulerExtension`，本文件再把通用框架任务转换成 IMPORT INTO 协议任务，并协调 `planner.rs`、`proto.rs`、importer、任务存储、对象存储、统计以及运行时。

它不是子任务的数据处理器：编码排序、写入、冲突收集/解决和后处理由物理计划生成的子任务执行器完成。本文件负责的是任务级状态机、作业表状态、子任务批次生成所需的上下文和外围资源生命周期。`Cargo.toml` 的 `nextgen` feature 传递给 `astersql-config-kerneltype/nextgen`；同一份代码通过运行时的 Classic/NextGen 判断处理不同 keyspace 和准入规则。

## 核心职责

1. **框架桥接与注册**：`frameworkTaskToImportTask` 校验任务类型并复制状态、步骤、资源限制、keyspace、meta、错误和允许的修改项；`NewImportSchedulerWithServices` 创建 `BaseScheduler`；`RegisterImportSchedulerFactoryWithServices` 为 `ImportInto` 注册工厂，坏 meta 则返回在 `init` 阶段稳定报错的 `FailedImportScheduler`。
2. **导入准备**：`prepareImportTask` 在取消检查后把 job 推进到 `preparing`，构造表和 `LoadDataController`，发现输入文件、检查容量、计算资源、切分 chunk，把 `PreparedMeta` 写入对象存储，再把更新后的计划、外部路径、槽位数和最大节点数写回任务。
3. **步骤规划**：`nextImportSubtasksBatch` 按 local/global sort 状态机读取必要的前序 subtask meta，推进 job step，调用 `LogicalPlan::ToPhysicalPlan` 与 `ToSubtaskMetas`，并持久化摘要。
4. **完成收口**：`doneImportTask` 尽力恢复 Classic 表模式；取消、失败和成功分别调用 `CancelJob`、`FailJob`、`FinishJob`。成功分支把统计增量与 job 完成放在同一事务中，统计失败按 Go 语义忽略。
5. **运行时维护**：`importScheduler::OnTickWithContext` 仅在 running 状态刷新 PD 注册租约，并在真正写 TiKV 的步骤节流切换 import mode；任务终止时撤销注册并恢复 normal mode。
6. **协议适配**：`ImportJobSqlSession`/`ImportJobStorageSession` 把两类 SQL 执行器适配成 importer 的 job 接口，`ImportJobJsonCodec` 保持 Go 命名 JSON 字段，`GetImportJobTaskManager` 处理 NextGen 用户 keyspace 的系统会话池边界。

## 主要符号

- 常量 `warningIndexCount`、`registerTaskTTL`、`refreshTaskTTLInterval`、`registerTimeout`、`defaultSwitchTiKVModeInterval` 分别定义多索引告警阈值 32、10 分钟租约、3 分钟刷新、5 秒单次注册操作超时和 5 分钟 TiKV 模式切换节流。
- `frameworkTaskToImportTask(&framework_scheduler::Task) -> Result<framework_proto::Task, _>` 只接受 `ImportInto`，修改类型仅允许 required slots、max node count、batch size 和 max write speed；框架未保存的 scheduler/time 字段用空值或 Unix epoch 补齐。
- `ImportJobSqlSession`、`ImportJobStorageSession` 与各自 row wrapper 实现 importer 的执行/行读取接口；查询结果列数不符立即报错。`withImportJobSession` 保证借还 DXF session，`withImportJobSessionRetry`/`retryImportSQL` 增加可取消退避。
- `ImportJobJsonCodec` 对 `ImportParameters` 和 `Summary` 编解码，字段名与 Go JSON 保持一致；缺省字段回落为空串、空 map、零或 false。
- `ImportSchedulerServices` 汇集 controller、resource calculator、table importer、KV codec、可选表/对象存储、空表检查、规划上下文和可选统计刷新；`FromEncodeRuntime` 复用节点编码侧 runtime，并用宿主适配器包裹存储、大小估算、资源采样和 importer。
- `PreparedController` 与 `OwnedSortStore` 是 RAII 守卫，`Drop` 分别调用 `LoadDataController::Close` 和对象存储 `Close`；外部注入的 `SortStore` 不由本文件关闭。
- `prepareImportTask`、`nextImportSubtasksBatch`、`doneImportTask` 是准备、规划、结束三个核心过程函数；`updateTaskSummary`、`updateMeta`、`getStepOfEncode` 是它们的数据更新辅助。
- `ImportSchedulerExtension` 实现 `framework_scheduler::Extension`，把 `on_tick`、`on_prepare`、`on_next_subtasks_batch`、`on_done`、`eligible_instances`、`next_step` 和 retryability 接到上述过程函数。
- `TaskRegistration` 与 `ImportSchedulerRuntime` 隔离 PD 租约、keyspace session pool 和 TiKV 模式切换；`taskInfo` 保存单任务注册句柄与刷新时间。
- `importScheduler` 保存 runtime、metrics、排序模式、模式切换时间、任务注册 map、当前任务 ID、禁用模式位和任务 keyspace。共享可变状态分别由 `Mutex` 或原子变量保护。
- `redactSensitiveInfo` 清空原始语句，并对源路径和云存储 URI 脱敏后写回 meta；它是公开辅助函数，但当前文件内没有调用点。

## 执行流程

1. 宿主必须先用 `RegisterImportSchedulerFactoryWithServices` 注册 `ImportInto` factory；当前可确认的演示/验证装配在 `pkg/session/runtime_test/ddl.rs`。收到任务后，factory 调用 `NewImportSchedulerWithServices`：先经 `frameworkTaskToImportTask` 校验/转换，再通过 `GetImportJobTaskManager` 选择默认或系统 keyspace manager，解析 `TaskMeta` 得出 `GlobalSort`，最后把 `ImportSchedulerExtension` 交给 `BaseScheduler`。
2. 若框架启用了 prepare mode，`on_prepare_with_context` 调用 `prepareImportTask`。NextGen 先查询 job，已取消或不存在时在任何文件/规划副作用之前终止；随后以 SQL 重试启动 job，构造 controller，初始化文件、做容量检查、计算线程/节点参数、更新 prepared 信息和格式约束、生成 chunk map并写 `PreparedMetaPath(task.ID)`，最后更新 task meta、required slots 和 max node count。
3. `next_step` 使用 `GetNextStep`：local sort 为 `Init/Prepared -> Import -> PostProcess -> Done`；global sort 为 `Init/Prepared -> EncodeAndSort -> MergeSort -> WriteAndIngest -> CollectConflicts -> ConflictResolution -> PostProcess -> Done`。
4. 每次 `on_next_subtasks_batch_with_context` 先把框架 task 转成协议 task，再调用 `nextImportSubtasksBatch`。该函数先做取消门禁；Classic 初始步骤还在事务中复查空表。它按目标步骤读取恰当的前序 meta，更新导入 job step，构造 `PlanCtx`（NextGen 用任务最大节点数，Classic 用当前 executor 数），将逻辑计划转物理计划和 subtask metas，再将逻辑摘要写回 task meta。
5. 进入 post-process 前先恢复 TiKV normal mode，把 job 转到 validating，并从编码步骤 summaries 与冲突 metas 计算最终行数。global sort 会扣除可精确计数的冲突行；若索引冲突过多，只设置 `TooManyConflicts`，不做不可靠的扣减。
6. running 期间 `on_tick_with_context` 调用 `switchTiKVMode` 和 `registerTaskWithContext`。仅 `Import`/`WriteAndIngest` 会尝试 import mode；注册句柄懒创建并每 3 分钟最多刷新一次。
7. `on_done_with_context` 调用 `doneImportTask`。Classic 表模式恢复是 best effort。reverting 任务按标准取消错误区分 cancel/fail；成功任务在事务内 best-effort 更新 stats 后 FinishJob。所有终止路径移除任务注册；取消/失败额外切回 normal mode，正常路径在进入 post-process 时已切回。

## 数据与状态

- 核心持久状态是 `framework_proto::Task.Meta` 内的 `TaskMeta`：包含 `JobID`、`Plan`、`Stmt`、`EligibleInstances`、`Summary`、chunk 信息/外部 prepared 路径。修改后必须经 `TaskMeta::Marshal` 和 `updateMeta` 写回，否则后续调度回调看不到变化。
- `prepareImportTask` 把 controller 最终 `Plan` 覆盖回 meta，并把 `ThreadCnt` 映射为 `RequiredSlots`、`MaxNodeCnt` 映射为 `MaxNodeCount`。prepared chunk map 存于对象存储而非无限膨胀任务行。
- `updateTaskSummary` 按即将执行的 step 选择 Encode/Merge/Ingest/Collect/Resolve 摘要。post-process 对已有 `ImportedRows` 做 wrapping 累加，再在 global sort 下做 wrapping 扣减，这与 Go 的整数算术/累积语义一致，而不是重置后重算。
- `taskInfoMap: Mutex<HashMap<i64, taskInfo>>` 保存本 owner 的注册句柄；`modeSwitch: Mutex<Option<Instant>>` 是整个 scheduler 的最近模式切换时间，不按任务划分；`currTaskID: AtomicI64` 避免同任务反复反序列化；`disableTiKVImportMode: AtomicBool` 来自计划的显式禁用或 `IsRaftKV2`。
- `Close` 只清空本地句柄并注销 metrics，不显式调用 `TaskRegistration::close`，以免新 owner 接管相同租约时被旧 owner 撤销；终态路径的 `unregisterTaskWithContext` 才负责关闭注册。
- `ImportJobJsonCodec` 省略 summary 中的零值字段，但解码缺失字段为零；parameters 中必写 `file-location`/`format`，其余非空才写。该格式是与 Go 系统表内容兼容的持久协议。

## 依赖与调用关系

- 上游：`pkg/session/runtime_test/ddl.rs` 是当前直接检索到的服务创建与工厂注册点；是否另有生产启动装配未验证。`astersql-dxf-framework-scheduler::BaseScheduler` 通过 `Extension` trait 间接调用 `ImportSchedulerExtension`。RustCodeGraph 对 trait 回调未给出静态 caller，但源码中的 factory 注册和 `Extension` 实现构成动态调用证据。
- 调度下游：`prepareImportTask` 调用取消检查、job SQL、table factory、`LoadDataController`、对象存储和 `updateMeta`；RustCodeGraph 直接记录其到 `checkImportJobNotCancelled`、`withImportJobSessionRetry`、`PreparedController`、`OwnedSortStore`、`PreparedMeta`、`updateMeta` 的边。
- 规划下游：`nextImportSubtasksBatch` 调用 `TaskHandle` 读取前序数据，依赖 `crate::planner::{PlanCtx, LogicalPlan}` 生成物理任务；图谱直接记录其到 `ProductionCheckImportTableEmpty`、`switchTiKV2NormalMode`、`getStepOfEncode`、`updateTaskSummary` 等边。
- 完成下游：`doneImportTask` 依赖 `resetClassicTableMode`、`switchTiKV2NormalMode`、`unregisterTaskWithContext`、`withImportJobSessionRetry`/`retryImportSQL` 和 importer 的 Cancel/Fail/Finish job 操作。
- 存储/协议：`framework_storage::TaskManager` 提供 session/transaction，`astersql_executor_importer` 提供 job SQL、controller、计划与摘要结构，`proto.rs` 提供 IMPORT INTO task meta，`astersql_objstore`/`astersql_ingestor_globalsort` 保存 prepared meta，`astersql_statistics_handle_storage` 写统计增量。
- 横向使用：仓库文本引用表明 `pkg/executor/import_into_storage.rs`、对应 executor 测试及 real-TiKV recorded harness 使用该模块；主要生产 factory 注册点目前在 Rust session runtime 装配中。

## 错误处理与边界

- 任务类型错误、未知 modification、meta/JSON 反序列化错误、缺少 table info、表工厂未安装、对象存储/规划/SQL 错误均向框架传播为 `SharedError`/`SchedulerError`；`FailedImportScheduler` 保证 factory 不能返回 `Result` 时仍在 `init` 暴露构造错误并清理 metrics。
- SQL 重试最多 `RETRY_SQL_TIMES` 次，间隔按 3、6、12、24、30 秒封顶；`Context::wait` 使取消能中断等待。`checkImportJobNotCancelled` 对短暂读取错误重试，但 “job not found” 和 “cancelled by user” 是非重试终态。Classic 因 job/task 同事务提交而直接跳过此额外门禁。
- `IsImportSchedulerRetryableError` 特判跨 keyspace session pool 缺失，并从 `[class:code]` 前缀恢复 normalized error code，其他错误委托 Lightning common retry 分类。
- PD 注册、注册关闭、TiKV 模式切换、Classic 表模式恢复和统计刷新是刻意的 best effort；失败不会覆盖核心任务终态。注意 `taskInfo::registerWithContext` 即使刷新失败也更新时间，从而在 10 分钟 TTL 内保留约两次后续重试机会。
- `ProductionCheckImportTableEmpty` 对数据库/表名中的反引号做双写转义并在新事务内 `select 1 ... limit 1`；此检查只针对 Classic 初始准入。空表检查函数可由服务注入以便宿主实现或测试替换。
- post-process 必须提供 `PostProcessSummaryInput`，否则明确报错；非法步骤报 `unknown step`；`StepDone` 返回空 batch。`ModifyMeta` 当前是兼容占位，原样返回旧 meta，不应理解为已支持在线参数修改。
- 多个锁使用 `expect`，mutex poisoned 会 panic；row getter 假设类型由 SQL schema 保证，但显式检查列数。`redactSensitiveInfo` 忽略最终 marshal 失败，与 Go 的仅记录/不阻断安全清理路径接近，但调用者不能把它当作可确认成功的 API。

## 并发与资源生命周期

- `importScheduler` 可跨线程共享：runtime/metrics 使用 `Arc`，模式时间与注册表由 `Mutex` 串行化，当前任务和禁用位用 Acquire/Release/AcqRel 原子序。模式切换在持锁期间调用 runtime，保证同一 scheduler 不会并发切换；锁粒度也意味着慢 runtime 调用会阻塞同类操作。
- `taskInfoMap` 的锁覆盖句柄创建、注册或关闭调用，避免同一任务重复创建/关闭。注册接口接收 scheduler context，在已取消时不进入外部调用。终态 `unregisterTaskWithContext` 从 map 中移除后关闭；普通 scheduler `Close` 仅 drop 本地句柄，保留远端租约供 owner 接管。
- `PreparedController` 和本地创建的 `OwnedSortStore` 依靠 Drop 在所有成功/错误返回路径释放资源。服务注入的 `SortStore` 被视作共享借用，不建立 owned guard，因此其生命周期由注入方管理。
- `TaskManager::WithNewSession`/`WithNewTxn` 管理 session 归还与事务边界；成功完成时 stats 和 FinishJob 位于同一 transaction callback。`ImportJobSqlSession` 仅借用 executor，不能逃逸其生命周期。
- controller 和对象存储 context 都从 `framework_scheduler::Context::cancellation_flag()` 构造，取消会传到文件发现、prepared meta 写入与资源采样。测试 `scheduler_cancellation_reaches_both_import_object_store_contexts` 专门验证这条传播。

## 与 Go 版本的对应关系

Rust 主对照文件是同目录 `scheduler.go`，独立测试为 `scheduler_test.go`、`scheduler_testkit_test.go`；Rust 对应测试放在 `scheduler_test.rs` 与 `scheduler_testkit_test.rs`，符合源文件与测试分离要求。

- Go `NewImportScheduler`/`Init`/`OnTick`/`OnPrepare`/`OnNextSubtasksBatch`/`OnDone`/`GetNextStep` 对应 Rust factory + `ImportSchedulerExtension`、`prepareImportTask`、`nextImportSubtasksBatch`、`doneImportTask` 和 `importScheduler` 方法。Rust 将 Go 直接持有的 BaseScheduler/TaskRuntime 拆成通用框架 trait 与注入服务。
- local/global 两条步骤链、前序 meta 收集、prepare-mode 的 StartJob/Job2Step 差异、多索引告警、post-process 行数与冲突数算法均保持 Go 逻辑。Rust 测试 `scheduler_local_sort_runs_go_import_validate_done_and_revert_phases` 与 `scheduler_global_sort_runs_go_seven_stage_subtask_matrix` 对应验证两条完整矩阵。
- Go 的 etcd client + `TaskRegister` 被 Rust `ImportSchedulerRuntime::new_task_registration` 抽象；TTL、刷新节奏、失败仍节流、终态撤销和 owner Close 不撤销租约的语义保留。Rust 用 `Mutex<HashMap>` 替代 Go `sync.Map`。
- Go 通过 store 判断用户 keyspace并缓存 task manager；Rust `GetImportJobTaskManager` 在构造 scheduler 时选择系统 session pool，缺失时使用相同错误文本并判为可重试。
- Go 直接使用 session、DDL、PD client 与 TLS；Rust 把这些宿主能力放在 `framework_storage`、`ImportSchedulerServices` 和 `ImportSchedulerRuntime` 后，便于生产绑定和独立测试。`Cargo.toml` 的 `nextgen` feature 只控制内核配置依赖，主要分支仍由 `IsClassic`/`IsNextGen` 判断。
- 已知边界应按当前事实描述：Rust `ModifyMeta` 与 Go 一样原样返回；Rust 的 `redactSensitiveInfo` 忽略写回错误；部分 Go failpoint/日志细节未逐项复刻，但核心状态、错误分类、事务与资源行为由对应 Rust 测试覆盖。

## 扩展指南

- 新增或调整任务步骤时，首先同步 `importScheduler::GetNextStep`、`nextImportSubtasksBatch` 的前序 meta/JobStep 分支、`updateTaskSummary` 和 `planner.rs` 的物理计划生成；同时扩展 `scheduler_test.rs` 的步骤矩阵以及 `scheduler_testkit_test.rs` 的 local/global 端到端阶段测试，并核对 Go `scheduler.go` 的同一增量。
- 改动 prepare 流程时应保持顺序不变量：取消门禁先于副作用，StartJob 先于文件发现，资源参数/格式校验先于 chunk 持久化，外部 prepared meta 成功后才写回 task meta 和槽位。测试应放在独立 `scheduler_test.rs` 或 `scheduler_testkit_test.rs`，不要嵌入生产文件。
- 新增 job 字段必须同步 `ImportJobJsonCodec` 编解码、importer model/SQL 和 Go JSON 字段名；考虑旧数据缺字段的默认值以及零值是否省略。新增 framework task 字段则同步 `frameworkTaskToImportTask`，避免动态回调丢状态。
- 接入新的宿主能力优先扩展 `ImportSchedulerServices` 或 `ImportSchedulerRuntime`，明确其 ownership、取消传播和线程安全；不要在核心状态机内硬编码测试后端。若引入 owned 外部资源，仿照 `PreparedController`/`OwnedSortStore` 用 Drop 覆盖错误路径。
- 调整重试或 best-effort 策略前必须区分核心一致性写入与外围恢复动作；尤其不要让统计、模式恢复或租约错误遮蔽原始任务结果。跨 keyspace 改动需复核 `job_doc.go` 描述的 job/task 分事务窗口和取消竞态。
- 性能风险主要来自锁内网络调用、重复读取大量前序 metas、prepared meta/JSON 大小和节点数计算。兼容风险集中在持久 JSON、步骤编号/顺序、错误文本及 normalized code、Go 的 wrapping 整数行为和 Classic/NextGen 分支。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；`files --filter pkg/dxf/importinto` 确认目标源、Go 对照和独立测试均已索引。
- 查询：`explore "pkg/dxf/importinto/scheduler.rs scheduler ImportScheduler register_task unregister_task submit_task schedule_task"`；`query` 分别唯一定位 `prepareImportTask`、`nextImportSubtasksBatch`、`doneImportTask`、`NewImportSchedulerWithServices`、`RegisterImportSchedulerFactoryWithServices`。`callees` 证实准备路径到取消/重试/RAII/updateMeta，规划路径到空表检查、模式恢复、步骤与摘要，完成路径到恢复/注销/重试；trait 动态 caller 由 factory 与 `Extension` 源码补证。
- 已读生产/边界文件：`pkg/dxf/importinto/scheduler.rs`（1710 行，RustCodeGraph 分段读取）、`pkg/dxf/importinto/Cargo.toml`、`pkg/dxf/importinto/lib.rs`、`pkg/dxf/importinto/job_doc.go`、`pkg/dxf/importinto/scheduler.go`、`pkg/session/runtime_test/ddl.rs` 的注册引用。
- 已读测试：`pkg/dxf/importinto/scheduler_test.rs`、`pkg/dxf/importinto/scheduler_testkit_test.rs` 及 Go 对照 `scheduler_test.go`、`scheduler_testkit_test.go`。关键覆盖包括任务桥接、SQL 绑定、退避/取消、Classic 空表事务、stats 事务、用户 keyspace manager、JSON round-trip、模式节流、租约 Close、eligible/step matrix、prepare 文件发现、取消无副作用及 local/global 完整阶段。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前仅执行任务规定的 11 章节结构命令，并人工复核仅新增本文档、未修改 Rust/Go/Cargo/`plan.md`。
