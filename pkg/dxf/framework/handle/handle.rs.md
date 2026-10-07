# `pkg/dxf/framework/handle/handle.rs`

## 文件定位

本文件是 `astersql-dxf-framework-handle` crate 的任务控制实现；`pkg/dxf/framework/handle/lib.rs` 将它声明为私有 `handle` 模块，再通过 `pub use handle::*` 暴露其 API。所在的 DXF（Distributed eXecution Framework）负责统一调度、分布式任务执行和资源管理；框架在所有 TiDB 节点运行，由 owner 负责调度，其余节点执行任务（`pkg/dxf/framework/doc.go`）。

crate 的直接必需依赖只有 `proto`、`schstatus`、`storage` 三个 DXF crate；`Cargo.toml` 中其余 Go 对应组件多为 optional 依赖。与 Go 版直接取得全局任务管理器、配置和对象存储组件不同，Rust 文件通过进程级 `Runtime` trait 将这些集成点集中注入。因此它既是对外的便捷 API 层，也是 Rust 移植中隔离底层实现差异的适配边界，并非任务调度器或任务存储本身。

## 核心职责

1. 提供任务生命周期入口：`SubmitTask*`、`WaitTask*`、`CancelTask`、`PauseTask`、`ResumeTask`。
2. 用 `Runtime` 抽象任务存储、节点/owner 信息、调度配置、对象存储和计量写入，并通过 `InstallRuntime`/`ClearRuntime` 管理进程级实现。
3. 提供可取消的轮询与重试设施：`Context`、`wait_task_with_lookup`、`Backoffer`、`RunWithRetry`。
4. 计算部署相关配置：`GetDefaultRegionSplitConfig`、`GetTargetScope`、`GetCloudStorageURI`、`GetScheduleTuneFactors`。
5. 提供容量为 1 的任务变更唤醒信号、对象存储访问计数、采样日志配置和任务计量记录组装。

文件不负责状态机推进、任务执行、SQL 持久化或真正的对象存储 I/O；这些行为分别由调度器、执行器和 `Runtime` 实现承担。RustCodeGraph 将本文件标记为被 31 个文件使用，但精确 `callers` 查询无法可靠消歧同名 Go/Rust 符号，因此生产接线还以限定名文本检索核验；没有发现生产调用的位置在下文明确标为“当前仅见测试覆盖”。

## 主要符号

- 错误与结果：`Error = storage::Error`、`Result<T>` 使 handle API 与存储层使用同一错误表示。
- 时间与容量常量：`CHECK_TASK_FINISH_INTERVAL` 为 300 ms；`TASK_CHANGED_CH_CAPACITY` 为 1；`SAMPLE_LOG_TICK` 为 60 s；`SAMPLE_LOG_FIRST` 为 10。
- 部署常量：`NEXT_GEN_TARGET_SCOPE = "dxf_service"`；Classic Region 默认阈值为 96 MiB、960,000 keys。
- `Context`：持有 `Arc<(Mutex<bool>, Condvar)>` 和共享 `AtomicBool`；`cancel` 同时设置原子标志并唤醒等待者，`wait` 提供可取消超时等待。
- 日志模型：`LogField`、`SampleLogger` 和 `NewSampleErrVerboseLogger` 只组装配置；本文件没有真正输出日志。
- 对象存储模型：`AccessStats` 以 `Mutex<u64>` 计数；`ObjectStorage` 目前只要求 `uri()`。
- 计量模型：`MeterValue::{Integer, Text}` 与有序的 `MeterItem = BTreeMap<...>`。
- `Runtime`：文件的核心集成 trait，覆盖任务 CRUD/状态控制、节点与 owner 查询、调度 TTL 配置、部署模式、对象存储创建和计量写入。它还包含供同 crate `status.rs` 使用的状态查询方法，因此范围大于本文件直接调用集合。
- 全局运行时：私有 `runtime_cell`、公开 `InstallRuntime`/`ClearRuntime`、crate 内 `runtime`。未安装时返回 `DXF handle runtime is not installed`。
- 任务通知：私有 `task_changed_channel`，公开 `NotifyTaskChange` 与 `TryRecvTaskChange`。
- 任务控制：`GetCPUCountOfNode`、`SubmitTask`、`SubmitTaskWithExtraParams`、`WaitTaskDoneOrPaused*`、`WaitTaskDoneByKey*`、`WaitTask`、`CancelTask`、`PauseTask`、`ResumeTask`。
- 辅助能力：`Backoffer`、`RunWithRetry`、`GetDefaultRegionSplitConfig`、`GetTargetScope`、`GetCloudStorageURI`、公开纯函数 `resolve_cloud_storage_uri`、`UpdatePauseScaleInFlag`、`GetScheduleTuneFactors`、`NewObjStore*`、`SendRowAndSizeMeterData`。

文件没有条件编译项；测试模块的 `#[cfg(test)]` 声明位于 `lib.rs`，测试逻辑独立放在 `handle_test.rs`。

## 执行流程

**提交任务。** `SubmitTask` 补上默认 `ExtraParams` 后委托 `SubmitTaskWithExtraParams`。后者先取得同一份 `Runtime`，用 `get_task_by_key_with_history` 检查活跃表和历史表：已有记录时返回 `ErrTaskAlreadyExists`；`ErrTaskNotFound` 被视为键可用；其他错误直接传播。随后依次 `create_task`、按新 ID `get_task_by_id`，成功后才调用 `NotifyTaskChange` 并返回任务。这样通知只是创建后的调度加速信号，不是持久化成功的依据。

**等待任务。** `WaitTask` 固定使用同一 `Runtime`，把查询闭包交给 `wait_task_with_lookup`。循环先通过 `Context::wait(300 ms)` 等待，再查询历史任务基座；查询错误被当作瞬时错误忽略，只有匹配谓词成立或上下文取消才退出。`WaitTaskDoneOrPausedWithResult` 先等到 done/paused，再读取完整历史任务：`Succeed`、`Paused` 返回任务，`Reverted` 优先返回任务内错误文本，`Failed` 组合状态和错误，其他状态保留兼容性地返回任务。`WaitTaskDoneByKeyWithManager` 特意使用调用方传入的 `storage::TaskManager` 完成探测和等待，避免用户 keyspace 的取消流程误切换到另一份全局 task store；生产调用见 `pkg/executor/import_into_storage.rs`。

**取消、暂停、恢复。** `CancelTask` 先按 key 查活跃任务，不存在（`None` 或 `ErrTaskNotFound`）都幂等成功，存在时按 ID 取消。`PauseTask`/`ResumeTask` 调用 Runtime 后忽略表示“是否找到/可操作”的布尔值，只传播底层错误，与 Go 版“不可暂停/恢复时记录信息并成功”保持可观察结果一致。

**重试。** `RunWithRetry` 最多执行 `max_retry` 次。成功或不可重试错误立即返回；可重试错误触发 `on_retry`、保存最后错误，并用 `Backoffer::backoff(retry)` 可取消地等待。次数耗尽返回最后错误；`max_retry == 0` 不调用操作并返回成功，这是源码注释明确保留的 Go `nil lastErr` 语义。

**部署与存储配置。** `GetDefaultRegionSplitConfig` 和 `GetTargetScope` 根据 `Runtime::is_next_gen` 在 Next-gen 固定值与 Classic 配置之间选择。`GetCloudStorageURI` 延迟读取 cluster ID，并由 `resolve_cloud_storage_uri` 在 URI path 尾部加入 `/dxf/`；仅非 SEM、原 URI 已有显式 path 且能取得 cluster ID 时再加入 `/<cluster_id>/`，query/fragment 原样保留。

**计量。** `SendRowAndSizeMeterData` 将 `StateUpdateTime` 向下对齐到分钟作为写入时间戳，构造任务 ID、keyspace、类型、行数、KV 字节、资源参数和执行时长；`data_kv_bytes` 只在正数时加入。写入 key 为 `<task.Type>_<task.ID>`，同一任务重试会覆盖同一计量对象，最后返回实际写入的 `MeterItem`。

## 数据与状态

- `Runtime` 是进程级可替换状态，存放在 `OnceLock<RwLock<Option<Arc<dyn Runtime>>>>`。`OnceLock` 固定容器地址，`RwLock` 允许读取和替换其中的 trait object；每次 `runtime()` 会 clone `Arc`，调用期间不持有全局锁。
- 任务变更 channel 同样由 `OnceLock` 惰性初始化。容量 1 表明它只表达“可能发生过变化”，不记录变化次数；两个连续通知可能合并为一个信号。
- `Context` 的 `AtomicBool` 是对调度任务或对象存储共享的取消事实，`Condvar` 是本地唤醒优化。`wait` 每次最多以 10 ms 小片等待，因此即使外部只设置共享原子标志、没有调用 `Context::cancel`，也会在有限时间内发现取消。
- `AccessStats` 的计数在进程内有效，溢出策略沿用 Rust debug/release 整数加法行为；当前没有清零接口。
- `MeterItem` 使用 `BTreeMap`，字段迭代顺序稳定。任务时间基于 `SystemTime`：更新时间早于 Unix epoch 时返回错误；创建时间晚于更新时间时，`duration_seconds` 保留负值以对齐 Go `time.Sub`。
- `GetScheduleTuneFactors` 把缺失值或 `ExpireTime < SystemTime::now()` 视为无有效配置并返回默认值；恰好等于当前采样时刻时不满足“小于”分支。

## 依赖与调用关系

向下依赖均通过 `Runtime` 或三个必需 crate：任务和状态类型来自 `proto`，调度 TTL/调优类型来自 `schstatus`，错误、任务管理器和历史页来自 `storage`。RustCodeGraph 对 `SubmitTaskWithExtraParams` 的有效 callee 边包括 `runtime`、`get_task_by_key_with_history`、`create_task`、`get_task_by_id`、`NotifyTaskChange`；对 `WaitTaskDoneByKey` 的边包括历史 key 查询和 `WaitTask`。

当前可核验的生产上游包括：

- `pkg/executor/import_into_storage.rs` 调用 `WaitTaskDoneByKeyWithManager`，完成 Import Into 清理路径中的任务等待。
- `pkg/dxf/framework/planner/planner.rs`、`pkg/dxf/framework/scheduler/autoscaler.rs` 和 `pkg/dxf/importinto/job.rs` 调用 `GetTargetScope`；`job.rs` 还调用 `NotifyTaskChange`。
- `pkg/executor/importer/production_resource.rs` 调用 `GetScheduleTuneFactors`。
- `pkg/ddl/index.rs` 再导出 `resolve_cloud_storage_uri`，`pkg/session/runtime/system_session.rs` 通过该 DDL 接口复用相同 URI 规则。
- `status.rs` 共享同一个 `Runtime`，使用其中的节点、owner、任务摘要等方法，这解释了 trait 中并非由 `handle.rs` 直接调用的成员。

在生产 Rust 源码文本检索中未找到本文件 `SendRowAndSizeMeterData`、`NewObjStoreWithRecording` 或 `RunWithRetry` 的调用；它们当前由 `handle_test.rs` 直接覆盖。不能由 Go 版已接线推断 Rust 版也已接线。`SubmitTask` 的直接 Rust 使用目前主要出现在 DXF 集成测试，Import Into 生产提交另有 `pkg/dxf/importinto/job.rs::SubmitTask` 实现。

## 错误处理与边界

- 所有 Runtime 委托错误通常原样传播；特殊兼容分支只有提交/取消时的 `ErrTaskNotFound`、等待轮询时吞掉查询错误，以及暂停/恢复忽略 `found` 布尔值。
- Runtime 未安装是所有依赖它的 API 的前置错误；`resolve_cloud_storage_uri` 是少数不依赖全局 Runtime 的纯函数。
- `Mutex`/`RwLock`/`Condvar` 中毒均通过 `expect` panic，而不是转换为 `storage::Error`。因此 Runtime 安装、通知接收、访问计数和 Context 内部锁都假设持锁代码不 panic。
- `NotifyTaskChange` 对 channel 满和断开静默成功，调用方不能把通知成功当作调度器已观察到变化；调度器仍需依靠持久化状态和周期检查保证正确性。
- 等待逻辑会永久重试所有查询错误；若底层持续故障且 Context 永不取消，调用不会自行失败或超时。调用方必须提供可取消 Context。
- URI 处理器只按 `://`、首个 path、`?`/`#` 做字符串拆分，不承担完整 URI 合法性校验；其前提与 Go 注释一致：配置写入时已校验。
- `SendRowAndSizeMeterData` 对 Unix epoch 前的 `StateUpdateTime` 返回错误；秒数和时间戳转换为 `i64` 时没有额外范围检查。

## 并发与资源生命周期

`Runtime` 需要 `Send + Sync`，通过 `Arc` 在调用者间共享。安装与清理由 `RwLock` 串行化，但没有所有权 guard 或栈式恢复机制；测试因此用 `RuntimeGuard::drop` 清理，并用文件系统独占锁串行化所有会替换全局 Runtime 的用例。生产代码若动态替换 Runtime，也必须保证没有并发测试式竞态或语义混用。

任务通知的发送端是无锁等待的 `try_send`，接收端因标准库 `Receiver` 非 `Sync` 而包在 `Mutex` 中；同一时刻只有一个接收者可尝试消费。容量 1 和丢弃重复信号可避免生产路径阻塞。

`Context::cancel` 以 Release 写入、`is_cancelled` 以 Acquire 读取；`Condvar::notify_all` 唤醒同一 Context clone 的全部等待者。`wait_task_with_lookup` 和 `RunWithRetry` 不创建线程、定时器或后台任务，等待资源随同步调用返回而释放。与 Go 的 `time.Ticker`/`time.After` 相比，Rust 用 `Condvar` 和短周期检查，不产生需显式 stop 的计时器对象。

对象存储以 `Arc<dyn ObjectStorage>` 返回，访问统计也以 `Arc<AccessStats>` 共享；生命周期由最后一个 `Arc` 持有者结束，本文件没有 close/flush 协议。计量写入是同步 Runtime 调用，没有本地队列。

## 与 Go 版本的对应关系

Rust 文件以 `pkg/dxf/framework/handle/handle.go` 为直接对照，任务提交、等待状态判定、取消幂等、暂停/恢复结果、重试上限、Region 阈值、target scope、URI 前缀、TTL 调优回退、对象存储和计量字段均保持 Go 的核心语义。`handle_test.rs` 还对照 `handle_test.go` 覆盖任务键唯一性、历史任务去重、重试、Next-gen 配置和 URI 矩阵。

主要实现差异如下：

- Go 直接使用 `storage.Get*TaskMgr`、全局配置、SEM、objstore、metering 和 logger；Rust 将这些能力收敛到 `Runtime`，所以部分 Cargo 依赖保持 optional，实际集成由 Runtime 实现决定。
- Go `context.Context` 同时承载取消、deadline 和值；Rust `Context` 只承载取消，不提供 deadline/value。超时需要调用方在共享原子标志上实现。
- Go 等待查询失败时写采样日志；Rust 保留继续轮询语义，但没有在 `wait_task_with_lookup` 中记录错误。`NewSampleErrVerboseLogger` 也仅返回配置模型。
- Go 提交和计量路径含 failpoint，Rust 对应函数没有这些注入点；Rust 独立测试通过可替换 Runtime 直接观察副作用。
- Go `GetCloudStorageURI` 使用解析后的 URL/prefix 类型；Rust 是保留 query/fragment 的字符串算法。现有 Rust 测试覆盖根路径和单层 path，但 Go 测试还覆盖多层 `path/sub`；扩展 URI 算法时应同步补齐。
- Go `SendRowAndSizeMeterData` 返回 `error` 并写一元素数组；Rust 返回写入的 `MeterItem` 以便测试和调用者观察。二者都以分钟时间戳和 `<type>_<id>` key 实现重试覆盖。
- Go 的日志和 retry metric 副作用没有进入 Rust `RunWithRetry`；Rust 以 `on_retry` 回调开放同一接入点，调用者必须显式提供。

## 扩展指南

- 新增任务存储操作时，先在 `Runtime` 增加最窄方法，再在 handle API 中编排；同步更新 Runtime 的所有实现，尤其是 `handle_test.rs::MockRuntime`，不要在 handle 层复制存储状态机。
- 修改提交规则时重点维护“活跃与历史 key 均唯一”“只有持久化和回读成功后通知”两个不变量，并扩展独立测试 `pkg/dxf/framework/handle/handle_test.rs`；同时核对 Go 的 `handle.go`/`handle_test.go`。
- 修改终态处理时必须基于 `pkg/dxf/framework/doc.go` 的任务状态机，分别覆盖 `Succeed`、`Paused`、`Reverted`（有/无 Error）、`Failed` 和取消中的 Context；不要把测试写回生产 `handle.rs`。
- 修改轮询或重试时保留可取消性，明确持续查询错误是否仍应无限重试，并验证 `max_retry == 0`、不可重试立即返回、耗尽返回最后错误。引入真实 sleep 的测试应使用短/零 backoff，避免不稳定。
- 修改全局 Runtime 生命周期时，应优先设计作用域 guard 或显式实例传递，评估已有 `status.rs` 共享者；任何并发测试仍需串行保护，避免一例清掉另一例的 Runtime。
- 修改 URI 规则时优先扩展纯函数 `resolve_cloud_storage_uri`，补齐多层 path、query、fragment、SEM 开关、无 path、无 cluster ID 的矩阵，并检查 `pkg/ddl/index.rs` 再导出消费者。
- 修改计量字段时同步 Go 常量/SDK约定，保留分钟对齐与稳定 key 的幂等语义；评估字段缺失与零值兼容性。对象存储和计量 API 当前缺少生产 Rust 调用者，接线前还应确认 Runtime 的真实实现位置。
- 性能敏感点主要是 300 ms 轮询频率、Context 最多 10 ms 的取消检查、全局锁和同步计量写入；变更前应先测量，而不是只为了减少代码而偏离 Go 行为。

## 验证依据

- RustCodeGraph：`status` 显示索引覆盖 11,467 个文件；`files --filter pkg/dxf/framework/handle` 覆盖 `handle.rs`、Go 对照及两套测试；`node --file ...handle.rs` 完整读取 688 行并显示被 31 个文件使用；`query` 精确定位 Rust/Go 的 `SubmitTaskWithExtraParams`、`WaitTask`、`RunWithRetry`、`GetCloudStorageURI`、`SendRowAndSizeMeterData`；callee 结果确认提交、等待等内部边。`callers` 对同名跨语言符号未能正确消歧，故没有将其异常结果作为上游结论。
- 已读生产与边界文件：`pkg/dxf/framework/handle/handle.rs`、`pkg/dxf/framework/handle/lib.rs`、`pkg/dxf/framework/handle/Cargo.toml`、`pkg/dxf/framework/doc.go`。
- 已读 Go 对照与测试：`pkg/dxf/framework/handle/handle.go`、`pkg/dxf/framework/handle/handle_test.go`、`pkg/dxf/framework/handle/handle_test.rs`。
- 上游文本证据：`pkg/executor/import_into_storage.rs`、`pkg/dxf/importinto/job.rs`、`pkg/dxf/framework/planner/planner.rs`、`pkg/dxf/framework/scheduler/autoscaler.rs`、`pkg/executor/importer/production_resource.rs`、`pkg/ddl/index.rs`、`pkg/session/runtime/system_session.rs` 中的限定名调用或再导出。
- 测试证明的边界：真实 SQL task manager 的提交/失败/历史去重，`ErrTaskNotFound` 的提交与取消兼容，三类重试结果和取消，Classic/Next-gen 配置，SEM/路径 URI 矩阵，对象存储记录，分钟计量与负时长，以及外部原子取消标志及时中断等待。
- 本任务按计划只做文档分析，不运行 Cargo。交付结构检查要求目标文件存在且恰有十一个固定二级标题；其实际命令和退出码在任务交付时记录。
