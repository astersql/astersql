# `br/pkg/registry/registration.rs`

## 文件定位

本文件是 Cargo 包 `astersql-br-pkg-registry` 中的恢复任务注册表实现，源码由 `br/pkg/registry/lib.rs` 以 `pub mod registration` 挂载，并经 `pub use registration::*` 作为 crate 的公开接口导出。`br/pkg/registry/Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/registry` 的 library 移植，但当前依赖表为空：TiDB session、domain、filter、错误类型等依赖面由同 crate 的 `stubs.rs` 提供，心跳线程由 `heartbeat.rs` 提供。因此它既不是单纯的声明门面，也还不是直接连接完整 TiDB Rust 基础设施的最终形态。

文件管理 SQL 表 `mysql.tidb_restore_registry` 中恢复任务的创建、恢复、暂停、注销、过期判断、冲突检查和全局配置操作协调。当前仓库中可见的生产 Rust 上游是 `br/pkg/task/stream.rs::restoreStreamWithTiKVConfigControl`，它调用 `OperationAfterWaitIDs` 和 `GlobalOperationAfterSetResettingStatus` 来串行化 TiKV 全局配置的修改与恢复；其余公开接口主要由本 crate 的对齐测试调用。Go 同名文件的调用面更广，不能据此推断所有 Go 上游都已接入 Rust 实现。

## 核心职责

1. 用 `RegistrationInfo` 描述一项恢复任务的过滤规则、时间范围、上游集群、系统表开关与命令类别，并以 Unit Separator（`FilterSeparator`）稳定地序列化过滤规则。
2. 通过 `ResumeOrCreateRegistration` 在悲观事务内锁定同参数记录：恢复 `paused` 任务、拒绝同参数的 `running/resetting` 任务，或插入新任务并读取自增 ID。
3. 通过 `resolveRestoreTS` 处理未显式指定 `restored_ts` 时的复用规则，并用心跳观察与条件更新识别、暂停失联任务，避免无条件抢占活跃任务。
4. 通过 `GetRegistrationsByMaxID`、`CheckTablesWithRegisteredTasks` 和 `checkForTableConflicts` 检查当前恢复对象是否与更早注册的任务重叠。
5. 通过 `OperationAfterWaitIDs` 与 `GlobalOperationAfterSetResettingStatus` 协调所有恢复任务共享的 TiKV 全局配置：新任务先等历史 `resetting` 任务结束，收尾任务仅在没有其他未完成任务时执行全局恢复操作。
6. 管理独立 SQL session、心跳 session 与 `HeartbeatManager` 的生命周期，并在暂停、注销或关闭时停止心跳。

## 主要符号

- `RestoreRegistryDBName` / `RestoreRegistryTableName`：固定注册表位置 `mysql.tidb_restore_registry`；SQL 模板与 `heartbeat.rs` 都引用这两个常量。
- `FilterSeparator`：值为 `\x1F`，用于 `Vec<String>` 与单列 `filter_strings` 之间的无歧义连接/拆分；过滤字符串和 SQL 标识通常不会包含该控制字符。
- `StaleTaskThresholdMinutes`：过期判定轮数为 5。实际 tick 时长来自 `stubs::stale_ticker_duration`，便于测试缩短等待。
- `RegistrationInfo`：公开输入值，包含 `FilterStrings`、`StartTS`、`RestoredTS`、`UpstreamClusterID`、`WithSysTable`、`Cmd`。
- `RegistrationInfoWithID`：为已持久化记录补充私有 `restoreID`；只由本模块冲突检查消费。
- `Registry`：核心状态对象。`se` 执行注册表事务和查询，`heartbeat_session` 专供心跳更新，`heartbeat_manager` 保存后台管理器，`wait_ids` 保存注册时观察到的 `resetting` 任务，`table_exists` 记录构造时表是否存在。
- `NewRestoreRegistry`：用 `Glue::CreateSession` 建立两个 session，并通过 `Domain::InfoSchema().TableByName` 区分“表不存在”与其他错误。
- `ResumeOrCreateRegistration`：注册状态机入口，返回 `(task_id, resolved_restore_ts)`。
- `PauseTask` / `Unregister`：先停止心跳，再分别做条件状态更新或删除记录。
- `StartHeartbeatManager` / `StopHeartbeatManager` / `UpdateHeartbeat`：心跳生命周期与单次更新时间接口。
- `OperationAfterWaitIDs` / `GlobalOperationAfterSetResettingStatus`：跨任务的全局操作协调接口。
- `FindAndDeleteMatchingTask`：abort 路径；只删除匹配的暂停任务或经持续心跳观察确认过期的运行/重置任务。
- `execute_in_transaction`：统一执行 `BEGIN PESSIMISTIC`、闭包、`ROLLBACK/COMMIT` 的局部事务框架。
- `is_task_stale_with` / `transition_stale_task_to_paused`：前者持续观察心跳，后者以“状态仍为 running/resetting 且心跳时间未变”为条件原子暂停。

## 执行流程

注册流程从 `ResumeOrCreateRegistration` 开始。它先调用 `resolveRestoreTS`，按过滤串、`start_ts`、上游集群、系统表开关和命令查找最近任务。用户显式指定的 `RestoredTS` 若与已有任务不同则报参数错误；未显式指定时，已有暂停任务或成功转为暂停的过期任务可贡献其 `RestoredTS`，否则保留调用方值。

随后函数把过滤数组以 `FilterSeparator` 连接，在 `execute_in_transaction` 的悲观事务中执行带 `FOR UPDATE` 的精确查询。命中 `running/resetting` 记录即拒绝重复运行；命中 `paused` 记录则更新为 `running` 并刷新心跳；未命中则插入新记录，再用同一 session 的 `SELECT LAST_INSERT_ID()` 取得 ID。事务闭包成功后提交，失败则尝试回滚。最后 `collectResettingStatusTasks` 快照当前所有 `resetting` ID，供后续全局操作等待。

冲突检查由 `CheckTablesWithRegisteredTasks` 读取 `id < 当前 restore_id` 的历史记录，解析并包装为大小写不敏感过滤器。若提供的 `PiTRIdTracker` 有数据，则优先逐 schema/table 检查并在该分支结束；否则，“Point Restore”记录先与快照恢复的数据库列表做 schema 冲突检查，所有记录再与 table 列表做表冲突检查。无法解析的历史过滤器会被跳过，而不是中止全部检查。

共享配置流程中，`OperationAfterWaitIDs` 按 10 个 ID 分块轮询注册时捕获的 `resetting` 任务；查询为空后继续下一块，累计重试超过 75 次则仍执行回调。`GlobalOperationAfterSetResettingStatus` 先把当前任务从 `running` 条件更新为 `resetting`，再查询是否存在任意非 `resetting` 任务；只有查询为空才执行全局收尾回调。若构造时注册表不存在，这两个接口都直接执行回调。

abort 流程 `FindAndDeleteMatchingTask` 先复用 `resolveRestoreTS`，然后在悲观事务中锁定完全匹配的记录。无记录返回 0，多条记录报不变量错误；`paused` 可直接删除，`running/resetting` 只有在心跳查询成功且五轮均未变化时才删除，其他状态或无法可靠判断时保守返回 0。

## 数据与状态

任务状态由 `TaskStatusRunning`、`TaskStatusPaused`、`TaskStatusResetting` 三个值驱动。允许的主要转换是：新建到 `running`；`paused` 恢复到 `running`；`running/resetting` 经 `PauseTask` 或过期条件更新到 `paused`；`running` 经全局收尾准备到 `resetting`；注销/abort 删除记录。代码对查询得到的未知状态不做隐式转换：注册时报 `ErrInvalidArgument`，abort 时跳过。

`filter_hash = MD5(filter_strings)` 只是 SQL 侧候选定位的一部分，完整相等条件仍包括原始参数相关字段。恢复任务查找以 ID 倒序并加锁；冲突扫描则只读取小于当前 ID 的记录并按 ID 升序返回，使后注册任务对先注册任务负责避让。

`wait_ids` 是注册完成时的快照，不会自动跟随数据库变化；轮询时仅关注这些 ID 是否仍处于 `resetting`。`table_exists` 同样是构造时快照。`unix_now` 在系统时间早于 Unix epoch 时回退为 0，这会进入数据库时间字段而不会单独报错。

## 依赖与调用关系

上游关系：`br/pkg/registry/lib.rs` 导出全部符号；`br/pkg/task/stream.rs::restoreStreamWithTiKVConfigControl` 在修改 TiKV 全局 GC ratio 与后台任务数之前调用 `OperationAfterWaitIDs`，在恢复结束时调用 `GlobalOperationAfterSetResettingStatus`。RustCodeGraph 将本文件直接关联到 `br/pkg/registry/parity_test.rs` 和 `br/pkg/task/stream_test.rs`；全仓搜索还显示独立的 `tests/realtikvtest/brietest` harness 有同名 API，但它实现的是测试 harness 镜像，并非直接调用本 crate 源码。

下游关系：注册表 SQL 通过 `Session` / `RestrictedSQLExecutor` 执行；上下文统一用 `WithInternalSourceType(..., InternalTxnBR)` 标记内部 BR 事务；过滤冲突依赖 `ParseFilter`、`CaseInsensitive`、`MatchSchema`、`MatchTable`；PiTR 冲突依赖 `PiTRIdTracker`；错误分类依赖 `berrors`；心跳后台任务依赖 `heartbeat.rs::{NewHeartbeatManager, update_heartbeat}`。

crate 边界值得特别注意：`br/pkg/registry/Cargo.toml` 没有外部依赖，以上类型均来自本地 `stubs.rs`。因此扩展代码时不能假定这里已获得真实 TiDB session、failpoint 或 table-filter crate 的全部行为；应同时核对 stub 契约和 Go 实现。

## 错误处理与边界

- session 创建、InfoSchema 查询、事务开始、主要 SQL 查询/更新均向上传播，并用 `Error::Trace`、`Annotate` 或 `Annotatef` 增加操作语境。
- 事务闭包失败时会尝试 `ROLLBACK`，但回滚错误被忽略，原始闭包错误优先返回；`COMMIT` 失败直接返回。
- `Mutex::lock()` 多数路径使用 `unwrap()`，锁中毒会 panic，而不是转换为 `Result`；`Close` 对锁中毒则跳过对应 session 的 `Close`。
- 已关闭的 `se` 或 `heartbeat_session` 返回明确的 `registry session closed` / `heartbeat session closed`。
- `updateTaskStatusFromMultiple` 拒绝空状态集合，防止生成无效 `IN ()`；当前状态列表来自代码内枚举，不接受外部 SQL 文本。
- `resolveRestoreTS` 对过期检查或条件暂停失败采取保守策略：保留当前 `RestoredTS`，不抢占旧任务。`is_task_stale_with` 遇到查询错误或记录消失也判为非过期。
- `OperationAfterWaitIDs` 达到等待上限后仍调用回调，这是与 Go 一致的“超时后继续”语义，不是超时报错。
- `GlobalOperationAfterSetResettingStatus` 没有验证条件更新实际影响行数；其后是否执行回调只由全表未完成任务查询决定。
- `render_sql` 只替换存在的 `{}`；参数不足会静默留下后续模板片段，参数过多会被忽略。当前所有调用都使用固定内部模板和固定参数数量。

## 并发与资源生命周期

`Registry` 为注册 SQL 与心跳分别持有 `Arc<Mutex<Box<dyn Session>>>`，避免后台心跳与前台事务共用同一 session。每次 SQL 操作在持锁期间执行；`resolveRestoreTS` 在进入长时间心跳观察前显式释放第一次查询的 guard，随后 `is_task_stale` 重新加锁并在整个观察循环中持有注册 session。abort 的过期观察发生在 `execute_in_transaction` 闭包内，因而事务及 session 锁会跨多个 tick 持续存在，这是安全扩展时必须评估的阻塞风险。

`StartHeartbeatManager` 总是先停止旧 manager，再克隆心跳 session 并启动新 manager；`PauseTask`、`Unregister` 与 `Close` 也先或最终停止 manager，防止已暂停/删除任务继续刷新心跳。`Close` 用 `Option::take` 保证两个 session 最多关闭一次，再停止 manager；之后任何依赖对应 session 的 API 都返回关闭错误。

过期任务转换使用 SQL 条件同时比较 ID、允许状态和旧心跳时间，避免观察结束与更新之间的竞态覆盖新心跳；更新后仍在同一悲观事务中回读状态确认。全局配置协调则是数据库状态协议，不依赖进程内锁，因此多个 `Registry` 实例可通过注册表互相观察。

## 与 Go 版本的对应关系

Rust 的常量、SQL 模板、结构体字段、公开方法及主要状态机对应 `br/pkg/registry/registration.go`。关键一致点包括：两个独立 session；悲观事务和当前 session 选项；暂停任务恢复；重复运行任务拒绝；五轮心跳过期检测；带旧心跳条件的原子暂停；10 个 ID 分块等待；75 次累计重试上限；仅在不存在非 `resetting` 任务时执行全局操作；abort 对不确定状态采取跳过策略。

可观察差异主要是迁移基础设施而非预期产品语义：Rust 通过本地 trait/stub 表达 Go 的 `domain`、`glue.Session`、`sqlexec`、filter、failpoint/ticker；Go 记录大量日志，而 Rust 省略日志；Go 的 ticker 在函数退出时显式停止，Rust 的 `maybe_sleep` 是同步循环且没有独立 ticker 资源。Rust `Registry` 使用 `Arc<Mutex<_>>` 表达跨线程 session 共享，Go 版本依赖接口对象和 heartbeat goroutine。后续替换 stub 时应维持上述返回值、错误分类、SQL 条件与保守失败策略，而不是只保持方法名称。

直接 Rust 单元对齐位于 `br/pkg/registry/parity_test.rs::go_rust_public_contract_matches`，覆盖创建、心跳、暂停/恢复、活动任务冲突、表/schema 冲突、显式 `RestoredTS` 不匹配、自动复用、全局操作、缺表快速路径、abort 与资源关闭。`tests/realtikvtest/brietest/registry_test.rs` 与 Go RealTiKV 场景同名，但导入 `astersql_tests_realtikvtest_brietest::harness::registry`；它验证独立 harness 行为，不能作为本 crate 已连接真实 TiKV 的证据。

## 扩展指南

新增注册参数时，应同时修改 `RegistrationInfo`、查找/冲突/插入 SQL 的字段与参数顺序、`GetRegistrationsByMaxID` 的列解码、完全匹配逻辑、Go 同名结构与 SQL，并扩展 `parity_test.rs` 的新建、恢复、冲突和 abort 场景。特别要保证新参数参与“同一任务”的身份判定，否则可能错误复用或误删记录。

新增状态时，应明确它能否恢复、暂停、删除、算作“未完成”，以及是否需要被 `selectAnyUnfinishedTaskSQLTemplate`、`updateTaskStatusFromMultiple`、过期转换和 abort 接受。未知状态目前刻意保守处理，不应为了方便直接归并到现有状态。

修改心跳/过期策略时，应优先保持“观察期间有任一次心跳变化即活跃”和“更新时再次比较旧心跳”的双重保护，并在独立测试文件 `br/pkg/registry/parity_test.rs` 增加活跃、消失、查询失败、取消和竞争更新用例。不要把 Rust 测试嵌回生产文件。

修改全局操作协调时，应同步检查 `br/pkg/task/stream.rs::restoreStreamWithTiKVConfigControl` 的调用顺序和错误替换语义，评估累计重试计数跨 chunk 的行为、超时仍执行回调的兼容性，以及长时间持锁/事务对性能的影响。若要接入真实 TiDB Rust 依赖，应在 crate 层替换 stub 契约并增加真正调用该 crate 的集成测试，不能把现有 RealTiKV harness 镜像当作完成证据。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/registry` 确认本目录的 Rust/Go 实现与独立测试文件。
- RustCodeGraph `node --file br/pkg/registry/registration.rs`：逐段核对 1,238 行源码，确认常量、SQL、`RegistrationInfo`、`Registry`、全部公开方法、事务、冲突与过期流程；图报告本文件被 `br/pkg/registry/parity_test.rs` 和 `br/pkg/task/stream_test.rs` 使用。
- RustCodeGraph `node --file br/pkg/registry/registration.go`：核对 Go 同名实现的 session 生命周期、状态转换、五分钟心跳观察、全局操作和 abort 语义。
- RustCodeGraph `node --file br/pkg/task/stream.rs`：确认 `restoreStreamWithTiKVConfigControl` 对两个全局协调接口的生产调用。
- `br/pkg/registry/Cargo.toml`、`br/pkg/registry/lib.rs`、`br/pkg/registry/BUILD.bazel`：核对 crate 名称、Go 包映射、模块导出、空 Rust 依赖表和 Go 依赖边界。
- `br/pkg/registry/parity_test.rs`：核对直接 Rust 单元对齐场景；`tests/realtikvtest/brietest/registry_test.rs` 与 `harness.rs`：确认 RealTiKV 风格测试使用独立 harness 镜像。
- 全仓 `rg`：确认当前生产 Rust 调用点位于 `br/pkg/task/stream.rs`，并区分 parity test、stream test 与 realtikv harness 的同名引用。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前用任务指定命令确认文档存在且恰好包含 11 个固定二级章节。
