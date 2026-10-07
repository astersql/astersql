# `br/pkg/stream/crr/internal/checkpoint/calculator.rs`

## 文件定位

本文件是 CRR（持续恢复/复制恢复）下游安全检查点计算器的公共契约与主循环实现。crate 由同目录 [`Cargo.toml`](Cargo.toml) 定义为 `astersql-br-pkg-stream-crr-internal-checkpoint`，入口 [`lib.rs`](lib.rs) 以 `pub mod calculator` 挂载本文件并扁平再导出其公共符号。它不是独立进程入口：生产侧 [`../../service/service.rs`](../../service/service.rs) 的 `Service::New` 构造 `Calculator`，`Service::run_once` 持锁调用 `ComputeNextCheckpoint`，并在首次运行前通过 `RestorePersistentState` 恢复进度、在成功推进后通过 `StateSnapshot` 保存进度。

文件自身定义配置、依赖 trait、事件/统计模型、轻量上下文、持久化状态与 `ComputeNextCheckpoint` 的阶段编排；扫描 meta、等待对象同步和推进各 store 水位的具体算法位于 [`progress.rs`](progress.rs)，存储 URI 与 meta 解析规则位于 [`storage.rs`](storage.rs)。包级安全不变量由 [`doc.go`](doc.go) 说明：只有本轮发现的必要对象均可在下游安全消费，才可返回新的上游检查点。

## 核心职责

1. `NewCalculator` 校验上游存储能力和任务名，并把非正的 `PollInterval`、`MetaReadConcurrency` 归一到 `2s`、`16`。
2. `ComputeNextCheckpoint` 串联“探测上游检查点 → 获取存活 store → 规划本轮 → 等待下游对象 → 推进内存状态 → 发事件”六个阶段；任一步骤出错都返回 `Error` 并尽力发送 `EventCalculationFailed`。
3. `Calculator` 跨轮保存 `last_checkpoint`、`synced_ts` 和 `synced_by_store`，通过快照 API 支持服务重启续跑。
4. `PDMetaReader`、`UpstreamStorageReader`、`ObjectSyncChecker` 把 PD、源对象存储与下游同步判定隔离成可替换依赖；`ExistenceSyncChecker` 提供“存在即已同步”的最小适配器。
5. `CheckpointEvent`、`FileStatistic` 与 `Observer` 提供稳定的进度观测契约。`EventType::as_str` 的六个字符串是日志/状态消费方可见值，不应随意改名。

该文件只决定何时调用各阶段以及何时提交 `last_checkpoint`，不会读取下游对象内容，也不会自行持久化状态；这两点分别由 `ObjectSyncChecker` 和 service 层的 `ResumeStateStore` 承担。

## 主要符号

- 常量：`DefaultPollInterval = 2s`、`DefaultMetaReadConcurrency = 16`，分别限制下游轮询节奏和 meta 读取默认并发度。
- 观测模型：`EventType` 包含 waiting/upstream advanced/round planned/waiting downstream/checkpoint advanced/calculation failed 六相；`CheckpointEvent` 携带任务、轮次、水位、store 映射、文件统计和可选错误；`FileStatistic::snapshot` 深拷贝两个后缀计数表，`record_downstream_check` 累加检查次数与路径后缀分布。
- 外部边界：`PDMetaReader::{GetGlobalCheckpointForTask, Stores}`；`UpstreamStorageReader::{WalkDir, ReadFile, URI}`；`ObjectSyncChecker::FileSynced`；`FileExistenceChecker::FileExists`；`Observer::OnCheckpointEvent`。这些 trait 均要求 `Send + Sync`。
- 适配器：`NewExistenceSyncChecker` 返回 `ExistenceSyncChecker<C>`；其 `FileSynced` 原样委托 `C::FileExists`，不缓存、不重试，也不改变错误。
- 配置与状态：`CheckpointCalculatorConfig`、`PersistentState`、内部 `calculatorState`。`PersistentState` 使用公开的 Go 风格字段名，内部状态使用 Rust snake_case；三项水位字段一一对应。
- 取消模型：`Context::{Background, WithCancel, WithTimeout, Done, Err}`。子上下文记录父级取消标志链，子取消不反向传播；超时选择父截止时间和请求截止时间中的较早者。
- 构造与主 API：`NewCalculator`、`Calculator::{ComputeNextCheckpoint, SyncedTS, LastCheckpoint, StateSnapshot, RestorePersistentState}`。
- 内部辅助：`Calculator::observe` 同步调用观察者并在时间为空时填 `SystemTime::now()`；`CalculatorDeps::validate` 调用 `validate_incremental_meta_scan_storage(Upstream.URI())`；`Error` 是仅含消息文本的包内错误类型。

本文件没有条件编译项或内嵌测试。测试由 [`lib.rs`](lib.rs) 在 `cfg(test)` 下从独立的 `*_test.rs` 文件挂载，符合源码与测试分离约束。

## 执行流程

`Service::run_once` 是生产入口：先初始化恢复状态，读取旧 `LastCheckpoint`，再调用 `ComputeNextCheckpoint`，推进后保存 `StateSnapshot`；若返回值未变化，则由 service 的 watcher 阻塞等待上游变化。

`ComputeNextCheckpoint` 的单轮流程如下：

1. 调用 `poll_upstream_checkpoint`。只有 PD 返回值严格大于 `state.last_checkpoint` 才继续；否则发 `EventWaitingUpstream` 并返回旧检查点，状态不变。
2. 调用 `load_alive_stores` 获取非零 store ID 集合。存活集合只用于阻塞缺少进度的 store 和成功轮次后的离线 store 裁剪。
3. 调用 `plan_round`。该函数从 `synced_ts` 之后扫描 meta，跳过不超过相应 store 已同步水位的 meta，并以 `MetaReadConcurrency` 为上限并发读取内容，形成去重的 `pending_paths`、每个 store 的最大 flush TS 和统计。
4. `observe_round_planned` 发送不可变统计快照。随后 `wait_object_sync` 按 `PollInterval` 反复调用 `ObjectSyncChecker::FileSynced`；只有所有 pending 路径移除后才返回成功。等待失败前会再次快照统计。
5. `advance_synced_state` 先合并每个 store 的最大 flush TS，再根据存活 store 判定是否允许提升全局 `synced_ts`，并裁剪离线 store。这里的 store 级安全推进发生在 `progress.rs`。
6. 仅在上述步骤全成功后，把 `state.last_checkpoint` 设为本轮上游检查点，发送 `EventCheckpointAdvanced`，返回该检查点。
7. 主路径用闭包聚合 `Result`，闭包结束后统一处理失败：若结果为 `Err`，调用 `observe_calculation_failed`，携带当时能够获得的统计快照。已发送的早期事件不会回滚。

提交顺序是安全边界：`wait_object_sync` 成功在前，store 进度更新和 `last_checkpoint` 提交在后。若把 `last_checkpoint` 提前写入，重试会误认为上游没有推进，从而跳过未完成的下游同步检查。

## 数据与状态

`last_checkpoint` 表示最近一次成功返回给调用方的上游全局检查点；`synced_ts` 表示对象复制已完成的全局扫描水位；`synced_by_store` 保存每个仍相关 store 的最大已确认 flush TS。三者含义不同：上游 checkpoint 已推进不代表所有对象已同步，且全局 `synced_ts` 必须受 store 级最小进度和“存活但尚无进度的 store”约束。

`StateSnapshot` 克隆 `synced_by_store`，因此调用方修改快照不会污染计算器。`RestorePersistentState` 直接接管传入 map，并把空 map 归一化为可写的空 `HashMap`；它只以当前 `last_checkpoint != 0` 判断计算是否已开始。因此初始或恢复后的 `LastCheckpoint == 0` 仍可再次恢复，这是与代码条件一致的精确边界，而不是更强的“一生只能调用一次”。

`CheckpointEvent::SyncedByStoreSet` 是 Rust 为表达 Go 的 nil map/显式空 map 差异而增加的标记；成功推进事件在 `progress.rs` 中将其设为 `true`。`Time` 用 `Option<SystemTime>` 表示 Go 的零时间。`Statistic` 与 `Err` 也是可选值：例如轮次规划前失败时没有统计，等待阶段失败时则携带已累计数据。

`Context` 的取消标志以 `Arc<AtomicBool>` 共享，使用 `SeqCst` 读写；祖先标志保存在 vector 中。deadline 是 `Instant`，只在调用 `Err`/`Done` 时比较，不创建计时线程。`Error` 只保留字符串，不保存错误源链或类型码。

## 依赖与调用关系

crate 的直接 Cargo 依赖是 `astersql-br-pkg-stream-backupmetas`、`astersql-br-pkg-streamhelper`、`serde` 和 `serde_json`；本文件直接使用其中 `streamhelper::Store`，而 backupmeta 解析和 serde 主要由相邻模块消费。crate metadata 明确 `go-package = "br/pkg/stream/crr/internal/checkpoint"`、`kind = "library"`、`lane = 2`。

上游调用边由 RustCodeGraph 与精确引用搜索共同确认：

- [`../../service/service.rs`](../../service/service.rs) `Service::New → NewCalculator`；
- `Service::run_once → Calculator::ComputeNextCheckpoint`；
- `Service::initialize_resume_state → Calculator::RestorePersistentState`；
- `Service::queue_resume_state_save → Calculator::StateSnapshot`。

主要下游边是：

- `NewCalculator → CalculatorDeps::validate → UpstreamStorageReader::URI → storage::validate_incremental_meta_scan_storage`；
- `ComputeNextCheckpoint → progress::{poll_upstream_checkpoint, load_alive_stores, plan_round, wait_object_sync, advance_synced_state}`；
- `ComputeNextCheckpoint → progress::{observe_round_planned, observe_checkpoint_advanced, observe_calculation_failed}`；
- `Calculator::observe → Observer::OnCheckpointEvent`；
- `ExistenceSyncChecker::FileSynced → FileExistenceChecker::FileExists`；
- `FileStatistic::record_downstream_check → progress::path_suffix`。

`Calculator` 不在内部加锁；生产 service 用 `Mutex<Calculator>` 串行化计算、恢复与快照。trait 的 `Send + Sync` 约束允许依赖跨线程借用，`plan_round` 才能在 `progress.rs` 的 scoped threads 中并发读 meta。

## 错误处理与边界

构造阶段会拒绝空任务名和不支持增量 meta 扫描的上游 URI。Rust 的 `CalculatorDeps` 使用非可选 `Box<dyn ...>`，安全 Rust 无法构造 nil trait object，因此 Go `validate` 中三条 nil 依赖错误在 Rust 类型层被消除；`test_checkpoint_calculator_requires_object_sync_checker` 实际通过构造一个会返回错误的 checker 验证运行时传播，并不是传入 nil。

主循环对错误不做重试，只补充阶段上下文并上抛：PD checkpoint、store 列表、meta 扫描/读取、同步检查和 context 取消均可使本轮失败；service 层决定重试。失败不会更新 `last_checkpoint`，但早先发出的事件和 `progress.rs` 在成功等待后才进行的状态变更遵循各自发生顺序。`Observer::OnCheckpointEvent` 没有 `Result` 返回值，无法向主循环报告观察失败；它由 `observe` 在调用线程同步执行，所以“不得阻塞或回写 Calculator”是调用方必须遵守的契约，而非运行时隔离。

`Context::Err` 优先报告 `context canceled`，其次才是 `context deadline exceeded`。它是对本包所需 Go context 语义的轻量建模，不提供 value、select channel 或主动唤醒；等待代码必须显式轮询/使用可取消休眠。`WithTimeout` 使用 `Instant::now() + timeout`，极端超大 duration 的溢出行为没有在本文件单独处理。

路径后缀统计使用 `i32` 计数，超大轮次理论上存在整数溢出风险；当前代码没有饱和或 checked add。`HashMap` 遍历顺序不稳定，任何新增测试都不应依赖事件中 map 或 pending 文件的顺序。

## 并发与资源生命周期

`Calculator` 是有状态对象，预期跨轮复用，但 `ComputeNextCheckpoint` 需要 `&mut self`，单对象不能在安全 Rust 中无同步并发调用。service 将它放在 `Mutex` 中，并在一次可能持续等待下游的计算期间持锁；状态查询走独立 `StatusStore`，避免直接抢计算器锁。

本文件拥有的共享资源只有 `Context` 的原子取消标志。`WithCancel`/`WithTimeout` 返回取消闭包；闭包持有当前子上下文的 flag，调用后所有 clone 都能看到取消。祖先取消向下传播，子取消不向上传播。deadline 不启动后台任务，因此没有线程回收问题。

meta 读取并发由 `progress.rs` 的 `thread::scope`、`ConcurrencyLimiter` 和 RAII guard 管理；scope 返回前线程必然结束，许可在 guard drop 时归还。`CalculatorDeps` 中的 trait 要求 `Send + Sync` 是这条并发路径的必要条件。下游 `FileSynced` 检查则是单线程循环，避免把同步检查实现的并发假设放大。

observer 由 `Box<dyn Observer>` 独占在 `Calculator` 内，但调用是同步借用；事件载荷克隆 map/统计以避免观察者持有内部可变状态。持久化资源不归 Calculator 管理：service 获取 `StateSnapshot` 后交给 `ResumeStateStore`，保存成功或失败不会改变本文件的资源所有权。

## 与 Go 版本的对应关系

直接对照文件是 [`calculator.go`](calculator.go)，公共常量、事件字符串、配置字段、依赖接口、状态字段、构造默认值、计算阶段顺序、快照/恢复规则与错误消息均保持对应。独立 Go 测试位于 [`checkpoint_calculator_test.go`](checkpoint_calculator_test.go)、[`integration_test.go`](integration_test.go) 和 [`randomized_integration_test.go`](randomized_integration_test.go)；Rust 对应测试分别位于同名 `*_test.rs` 文件，并由 `lib.rs` 的 `cfg(test)` 模块挂载。

已确认的语言/移植差异如下：

- Go `EventType` 是字符串别名；Rust 是 enum，并通过 `as_str` 显式稳定映射。
- Go 用 nil interface 检查三项依赖；Rust 用非可选 boxed trait 在类型层保证存在，只保留 URI 能力校验。
- Go 使用标准 `context.Context`；Rust 的 `Context` 仅实现取消链与 deadline。
- Go `time.Time`/nil map/nil error 由零值表示；Rust 分别用 `Option<SystemTime>`、`SyncedByStoreSet` 和 `Option<Error>` 表达。
- Go `PersistentState` 带 JSON tag；Rust 该结构在本文件没有 `Serialize`/`Deserialize` derive，序列化由 service/相邻层的显式逻辑承担。
- Go `ComputeNextCheckpoint` 含 `failpoint.InjectCall("begin-calculate-checkpoint")`；Rust 本文件没有对应 failpoint。
- Go 恢复状态时写结构化日志；Rust `RestorePersistentState` 不记录日志。
- Go 与 Rust 都同步调用 observer；Rust 的 `Send + Sync` 约束更明确，但不会自动把回调调度到后台。

这些差异不应被误写为 Rust 已完整提供 Go 标准库/故障注入能力。行为一致性的现有证据来自 parity、单元、集成和随机化测试，而不是仅凭字段同名。

## 扩展指南

- 新增计算阶段时，从 `ComputeNextCheckpoint` 接线，并先确定其相对 `wait_object_sync`、`advance_synced_state`、`last_checkpoint` 提交的位置；任何安全性检查都必须发生在提交检查点之前。同步更新 Go `calculator.go` 或明确记录有意差异。
- 新增事件类型时，同时修改 `EventType`、`as_str`、事件生产点、service 的 `StatusObserver`/指标消费逻辑，以及 Rust/Go 的 observer 生命周期测试；字符串值视为兼容契约。
- 扩展依赖能力时优先给现有 trait 添加最小方法或引入独立 trait，并检查 `plan_round` 的并发调用是否仍满足 `Send + Sync`。不要让 calculator 读取下游对象内容，这会突破 `ObjectSyncChecker` 边界。
- 修改持久化字段时，同步 `calculatorState`、`PersistentState`、`StateSnapshot`、`RestorePersistentState`、service 的编码/解码与 Go JSON 契约，并覆盖旧状态缺字段、空 map 和重启首轮行为。
- 修改取消/超时语义时，重点验证父取消向下传播、子取消不反传、子 timeout 不延长父 deadline，以及下游等待能及时退出。
- 修改统计时使用 `snapshot` 隔离已发送事件，避免后续等待继续累计时改变观察者已收到的数据；计数类型或路径分类变化可能影响监控兼容性和性能。
- 测试必须放在独立文件。核心构造、错误和 store 边界应扩展 [`checkpoint_calculator_test.rs`](checkpoint_calculator_test.rs)；跨轮、并发、恢复和事件时序应扩展 [`integration_test.rs`](integration_test.rs)；Go/Rust 公共契约差异应扩展 [`parity_test.rs`](parity_test.rs)；长序列状态机应扩展 [`randomized_integration_test.rs`](randomized_integration_test.rs)。

主要风险是错误推进造成恢复读取未同步对象（正确性）、事件/状态字段变化破坏持久化与监控消费者（兼容性），以及轮询间隔、observer 阻塞、meta 并发度或 map 深拷贝引起的延迟与内存开销（性能）。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件、11,467 个总文件；目标目录的 `calculator.rs` 被识别为 51 个符号。`explore` 确认 `NewExistenceSyncChecker` 被 Rust 单元/集成测试调用，目标默认值和主 API 被多组 checkpoint 测试覆盖。
- RustCodeGraph `node --file` 完整读取了 [`calculator.rs`](calculator.rs) 1–503 行，并读取了 [`doc.go`](doc.go)、[`lib.rs`](lib.rs)、[`progress.rs`](progress.rs)、[`calculator.go`](calculator.go) 和生产调用者 [`../../service/service.rs`](../../service/service.rs) 的相关实现。
- RustCodeGraph 对精确 Rust symbol id 执行了 `callers/callees`；当前 CLI 对同名 Go/Rust 符号产生歧义并混入整文件符号。为避免据此臆测，另用 `rg` 精确核对 `NewCalculator`、`ComputeNextCheckpoint`、`RestorePersistentState`、`StateSnapshot` 和 `NewExistenceSyncChecker` 的 Rust 引用，得到“service 构造/运行/恢复/保存”生产链及独立测试调用面。
- Cargo 边界来自 [`Cargo.toml`](Cargo.toml)；Go 语义来自 [`calculator.go`](calculator.go) 与包契约 [`doc.go`](doc.go)。测试清单核对了 Rust 的 [`checkpoint_calculator_test.rs`](checkpoint_calculator_test.rs)、[`integration_test.rs`](integration_test.rs)、[`parity_test.rs`](parity_test.rs)、[`randomized_integration_test.rs`](randomized_integration_test.rs)，以及对应 Go 测试文件。
- 关键边界由测试名称与实现直接佐证：无同步 checker/同步错误、缺失新存活 store、离线 store 裁剪、已同步 meta 跳过、empty meta、恢复状态、上游未变化、并发读取上限、未来 flush meta、成功/失败事件生命周期和随机化时序。
- 本任务只新增文档，按计划不运行 Cargo。交付验证为固定 11 章节结构检查、路径/链接存在性检查、仅目标文档的 diff 自审和 `git diff --cached --check`。仓库说明引用的 `.agents/skills/tidb-verify-profile` 在当前检出中不存在，因此无法加载其额外 Ready 命令；任务文件明确给出的结构验证仍可完整执行。
