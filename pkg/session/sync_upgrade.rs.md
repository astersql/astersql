# `pkg/session/sync_upgrade.rs`

源文件：[`sync_upgrade.rs`](./sync_upgrade.rs)  
独立 Rust 测试：[`sync_upgrade_test.rs`](./sync_upgrade_test.rs)  
Go 对照：[`sync_upgrade.go`](./sync_upgrade.go)

## 文件定位

本文件属于 `astersql-session` crate。crate 根 [`lib.rs`](./lib.rs) 通过 `pub mod sync_upgrade` 公开该模块，并在 `cfg(test)` 下单独挂载 `sync_upgrade_test`；[`Cargo.toml`](./Cargo.toml) 指定 `lib.rs` 为库入口。文件没有条件编译项，也不直接使用 crate 的外部依赖，只依赖 `std::time::{Duration, Instant}`。

它移植 Go 会话层滚动升级状态编排：开始升级时发布 `Upgrading` 并等待 DDL owner 确认，结束升级时恢复暂停工作并发布 `NormalRunning`，同时提供全局升级状态查询及带重试的版本日志记录。不过当前仓库中的 Rust 生产代码没有 `SyncUpgradeRuntime` 实现，也没有调用四个编排函数的生产 Rust 调用者；实际副作用仅在独立测试的 `TestRuntime` 中模拟。因此当前事实是“编排逻辑和测试已存在、生产接线尚未建立”，不能把 trait 方法视为已经连接 etcd、DDL 或分布式任务。

Go 生产主链则已经接线：[`pkg/server/handler/upgrade_handler.go`](../server/handler/upgrade_handler.go) 的 `ClusterUpgradeHandler.StartUpgrade`/`FinishUpgrade` 分别调用 Go `SyncUpgradeState`/`SyncNormalRunning`；[`upgrade_run.go`](./upgrade_run.go) 的 `printClusterState` 在版本达到 `SupportUpgradeHTTPOpVer` 后调用 Go 的重试查询。

## 核心职责

- `SyncUpgradeState`：先写入 `ServerState::Upgrading`，再以 200 ms 间隔轮询 DDL owner，直到 owner 返回 `synced_upgrading_state = true` 或总截止时间到达。
- `SyncNormalRunning`：尽力恢复所有暂停 DDL job、调整分布式任务溢出并发度，最后以固定 3 秒超时写回 `ServerState::NormalRunning`。
- `IsUpgradingClusterState`：以固定 3 秒超时读取全局状态并转换成布尔值。
- `isUpgradingClusterStateWithRetry`：在调用方给定的总时间内重试状态读取；成功时记录旧版本、新版本与升级标志，耗尽时间时只记录警告，不向上返回错误。
- `SyncUpgradeRuntime`：将上述纯编排与全局状态存储、DDL owner、DDL job、分布式任务、日志和休眠等宿主副作用隔离。

## 主要符号

- `pub enum ServerState { Upgrading, NormalRunning }`：Rust 内部的两态模型，对应 Go `serverstate.StateUpgrading` 与 `serverstate.StateNormalRunning`。枚举不负责字符串或持久化编码，编码责任在运行时实现。
- `pub struct OwnerOp { pub synced_upgrading_state: bool }`：将 Go `owner.OpType.IsSyncedUpgradingState()` 的判定结果压缩成一个布尔字段。
- `pub fn isContextDone(deadline: Instant) -> bool`：用单调时钟比较当前时间和绝对截止点，替代 Go `context.Context.Done()` 的非阻塞检查。
- `pub trait SyncUpgradeRuntime`：公开关联错误类型 `Error`，并声明 10 个同步方法。`update_global_state`、`owner_operation`、`global_state` 和并发度调整返回可传播错误；恢复 job 同时返回逐 job 错误与整体错误；日志、休眠及超时错误构造也由宿主提供。
- `pub fn SyncUpgradeState<R: SyncUpgradeRuntime>(...) -> Result<(), R::Error>`：开始升级入口，保留 Go 风格名称。
- `pub fn SyncNormalRunning<R: SyncUpgradeRuntime>(...) -> Result<(), R::Error>`：结束升级入口。
- `pub fn IsUpgradingClusterState<R: SyncUpgradeRuntime>(...) -> Result<bool, R::Error>`：一次性状态查询入口。
- `pub fn isUpgradingClusterStateWithRetry<R: SyncUpgradeRuntime>(...)`：模块私有的重试查询辅助函数；无返回值，最终失败仅通过日志可见。

文件级 `#![allow(dead_code, non_snake_case)]` 允许尚未生产接线的符号和为对齐 Go 保留的导出函数名，不代表这些入口已被实际使用。

## 执行流程

`SyncUpgradeState` 的顺序与退出点如下：

1. 调用 `update_global_state(Upgrading, timeout)`；失败立即原样返回，且不会进入 owner 轮询。
2. 以当前 `Instant` 加总超时生成 `deadline`，轮询间隔固定为 200 ms。
3. 每轮先调用 `isContextDone`；到期则通过 `runtime.timeout_error(timeout)` 构造错误并返回。
4. 计算剩余时间，将单次 `owner_operation` 超时限制为“剩余时间与 3 秒的较小值”。
5. owner 已同步时成功返回；owner 尚未同步或查询失败时，每 10 次尝试记录一次警告，然后休眠 200 ms 继续。计数从 0 开始，因此第一次失败就会记录。

`SyncNormalRunning` 先调用 `resume_all_jobs`。整体错误和每个 job 错误都只记录，不中断；随后仅在 `task_manager_available()` 为真时调整任务溢出并发度，该错误同样只记录。最后调用 `update_global_state(NormalRunning, 3s)`，这是本函数唯一会传播的错误。由此保持“不因单个恢复动作失败而跳过最终状态恢复”的 Go 意图。

`IsUpgradingClusterState` 调用 `global_state(3s)`，读取成功后只在状态严格等于 `Upgrading` 时返回 `true`。`isUpgradingClusterStateWithRetry` 每轮调用它：成功即调用 `log_state` 并返回；失败时先检查总截止时间，到期则记录超时警告并返回，否则从第 0 次开始每 25 次记录一次读取失败警告，休眠 200 ms 后重试。

## 数据与状态

该模块自身不保存跨调用的共享状态。`deadline`、`attempt`、`remaining` 和 `child_timeout` 都是单次调用的栈上局部变量；集群状态、owner 状态、暂停 job、任务管理器和日志记录全部由 `SyncUpgradeRuntime` 的实现持有。

关键不变量是：开始流程必须先成功发布 `Upgrading` 才会等待 owner；owner 未确认前不会报告开始成功；结束流程总会在完成尽力恢复后尝试发布 `NormalRunning`；一次性查询使用固定 3 秒预算；两个轮询均使用 200 ms 间隔。`OwnerOp` 只表达“是否已同步”，不携带 Go `owner.OpType` 的其他值。

`Instant` 提供进程内单调时间语义，不能序列化或跨进程共享。状态持久化格式也不由 `ServerState` 定义，未来生产运行时必须显式映射到与 Go/etcd 兼容的表示。

## 依赖与调用关系

RustCodeGraph 显示：

- `SyncUpgradeState` 下调 `isContextDone` 以及 trait 的 `update_global_state`、`owner_operation`、`timeout_error`、`log_warning`、`sleep`。
- `SyncNormalRunning` 下调 `resume_all_jobs`、`task_manager_available`、`adjust_task_overflow_concurrency`、`update_global_state` 和 `log_warning`。
- `IsUpgradingClusterState` 仅下调 `global_state`；`isUpgradingClusterStateWithRetry` 再调用它及 `isContextDone`、`log_state`、`log_warning`、`sleep`。
- 图查询未找到上述 Rust 编排入口的生产调用者；仓库搜索只找到 [`sync_upgrade_test.rs`](./sync_upgrade_test.rs) 的 `TestRuntime` trait 实现与测试调用。

Go 对照实现的具体依赖包括 `domain.GetDomain(s).DDL().StateSyncer()`、`owner.GetOwnerOpValue`、`ddl.ResumeAllJobsBySystem`、`dist_store.GetTaskManager` 和后台日志器。Rust 文件把这些依赖折叠进 trait，因而 `Cargo.toml` 虽声明 DDL、domain、owner、分布式任务等 crate 依赖，本文件当前并未直接引用它们。

## 错误处理与边界

- `SyncUpgradeState` 会传播首次全局状态写入错误、总超时错误；owner 单次查询错误只告警并重试。发布 `Upgrading` 成功后若 owner 永远不确认，函数超时退出但不自动回滚为 `NormalRunning`。
- 单次 owner 超时不会超过剩余总预算或 3 秒；固定 200 ms 的 `sleep` 自身由运行时实现，函数不要求可取消，因此实际返回可能比截止点晚一次休眠和运行时调用开销。
- `SyncNormalRunning` 明确把恢复 job、单个 job 和任务并发调整错误降级为告警；只有最终写回正常状态失败会返回 `Err`。调用方不能从返回值获知先前的部分恢复失败，只能依赖日志。
- `IsUpgradingClusterState` 原样传播读取错误；除 `Upgrading` 外的枚举值当前只有 `NormalRunning`，返回 `false`。
- 重试查询最终失败不会向调用方传播，适用于“记录状态但不阻断升级流程”的用途，不适合需要强一致判定的入口。
- 与 Go 尚有可观察差异：Go owner 未同步和查询出错统一记录 `"get owner op failed"`，Rust 的错误分支记录 `"get owner operation failed"`；Go 重试耗尽仍记录 `"get global state failed"`，Rust 记录 `"get global state timed out"`。Go `SyncNormalRunning` 还包含 `mockResumeAllJobsFailed` failpoint 的测试捷径，Rust 没有对应分支。扩展或对齐时应先决定这些差异是否为有意改进。

## 并发与资源生命周期

所有入口都接收 `&mut R` 并同步执行：同一个运行时实例在一次调用期间具有独占可变借用，模块内部不创建线程、异步任务、锁、通道或事务。轮询通过 `runtime.sleep` 阻塞当前执行流；若宿主需要异步或可取消等待，现有同步 trait 不能直接表达，需调整接口而不能只替换实现。

该文件不拥有 etcd client、session、DDL owner 或 task manager，也不负责关闭它们；资源创建、复用、取消与释放均属于运行时实现。与 Go `context.WithTimeout` 不同，Rust 只传递 `Duration`，没有取消令牌或父上下文传播。`SyncUpgradeState` 的总截止点由本文件维护，而 trait 实现必须自行兑现每次传入的操作超时。

## 与 Go 版本的对应关系

Rust 四个函数逐一对应 [`sync_upgrade.go`](./sync_upgrade.go) 的同名函数，主要控制流、200 ms 轮询、owner 每 10 次告警、状态读取每 25 次告警、owner 单次 3 秒上限以及正常状态写回的 3 秒超时均得到保留。

主要适配是把 Go 的 `sessionctx.Context` 和 `domain` 全局查找改成泛型 `SyncUpgradeRuntime`；把字符串状态改成 `ServerState`；把 `owner.OpType` 改成 `OwnerOp`；把 Go context 截止/取消改成 `Instant` 截止检查。这样便于单元测试，但也使生产集成、状态编码、取消语义和日志后端成为尚待实现的边界。

Go 端到端测试 [`test/bootstraptest/bootstrap_upgrade_test.go`](./test/bootstraptest/bootstrap_upgrade_test.go) 验证 HTTP start/finish 前后的全局状态、无 HTTP 操作时状态保持正常，以及升级结束后暂停 DDL job 可恢复。Rust 独立测试目前只验证：未同步 owner 的首次告警和重试、恢复错误全部记录且仍写回正常状态、查询使用 3 秒超时、读取失败后重试成功并记录版本状态；它没有覆盖总超时、初始/最终状态写入失败、owner 查询错误分支、无 task manager 分支或真实 etcd/DDL 集成。

## 扩展指南

若要完成生产接线，最可能新增的是一个独立生产运行时类型及其 `SyncUpgradeRuntime` 实现，把每个 trait 方法映射到 Rust 的 server-state syncer、DDL owner、job 恢复、分布式任务管理器和日志设施；不要在本文件中加入会虚假成功的默认实现。接线后应从实际 HTTP 升级入口和 bootstrap 版本检查调用这些函数，并确认 crate 依赖与状态编码和 Go 兼容。

修改轮询或错误策略时，应同时更新 [`sync_upgrade_test.rs`](./sync_upgrade_test.rs)，并新增总超时、写入失败、owner 错误节流（0、10 次）、状态读取节流（0、25 次）及边界超时用例。测试逻辑继续放在独立测试文件，不内嵌到生产源文件。涉及真实集群行为时，还需补与 Go `bootstrap_upgrade_test.go` 同意图的集成覆盖。

兼容性风险集中在状态持久化映射、日志文本/频率、context 取消差异和 Go failpoint 语义；正确性风险是部分恢复失败被降级后仍切回正常状态，以及开始升级超时不回滚；性能风险主要来自同步 200 ms 轮询占用执行线程。新增枚举状态、异步接口或退避策略都属于协议变化，需同步审查所有运行时实现和调用者。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；目标源和测试均可按文件读取。
- RustCodeGraph `node --file pkg/session/sync_upgrade.rs`：核对 173 行生产源中的全部枚举、结构体、trait、函数和分支。
- RustCodeGraph `callers`/`callees`（限定 `--file pkg/session/sync_upgrade.rs`）：确认四个入口的下游调用边，且未返回生产 Rust 调用者。
- RustCodeGraph `node --file pkg/session/sync_upgrade_test.rs`：核对 `TestRuntime` 是仓库搜索到的唯一 trait 实现及四个现有单元测试的断言。
- [`Cargo.toml`](./Cargo.toml) 与 [`lib.rs`](./lib.rs)：确认 crate 名、库入口、公开模块和独立测试模块装配；本文件没有 feature gate。
- RustCodeGraph 读取 [`sync_upgrade.go`](./sync_upgrade.go)、[`upgrade_run.go`](./upgrade_run.go)、[`pkg/server/handler/upgrade_handler.go`](../server/handler/upgrade_handler.go) 和 [`test/bootstraptest/bootstrap_upgrade_test.go`](./test/bootstraptest/bootstrap_upgrade_test.go)：核对 Go 控制流、生产入口和端到端测试意图。
- 按任务约束未运行 Cargo；本次为纯文档分析，行为事实通过源码、调用图、Cargo/模块装配和现有测试源码交叉验证。
