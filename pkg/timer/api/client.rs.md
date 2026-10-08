# `pkg/timer/api/client.rs`

## 文件定位

本文件属于 `astersql-timer-api` crate（见 `pkg/timer/api/Cargo.toml`），位于定时器模型/存储抽象与上层运行时之间。`pkg/timer/api/lib.rs` 将 `client` 声明为公开模块并再导出其 API；本文件不保存定时器本身，而是把上层的创建、查询、更新、手动触发、关闭事件和删除请求转换为 `TimerStore` 操作。

生产侧有两条可见接入链：`pkg/timer/runtime/runtime.rs::ensureWorker` 为每个 Hook 构造 `DefaultTimerClient` 并以 `Box<dyn TimerClient>` 交给工厂；`pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command` 用它查询 TTL 定时器、提交手动触发并轮询请求处理结果。`pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook` 则持有 `Arc<dyn TimerClient>`，在后台任务中读取定时器并关闭 TTL 调度事件。

## 核心职责

- 用 `GetTimerOption` 和 `UpdateTimerOption` 把可组合的闭包选项折叠成 `TimerCond` 或 `TimerUpdate`，避免调用者直接拼装底层补丁。
- 以 `TimerClient: Send + Sync` 定义上层可依赖的 CRUD 和事件操作边界，并由 `DefaultTimerClient` 委托 `TimerStore` 实现。
- 在 `CreateTimer` 中为未指定命名空间的规格填入客户端默认命名空间，并在创建后回读完整记录。
- 在 `ManualTriggerEvent` 中校验事件/启用状态，以版本号执行乐观并发更新，并只对 `ErrVersionNotMatch` 做有限重试。
- 在 `CloseTimerEvent` 中限制可由调用者修改的字段，校验事件 ID，清理事件态，并按需推进水位线。

本文件不负责调度策略计算、记录持久化、Watch、事件实际执行或 Context 取消语义；这些分别位于 `timer.rs`、`store.rs`、具体 `TimerStoreCore` 和运行时模块。

## 主要符号

- `clientMaxRetry = 5`、`clientRetryBackoff = 1000`：手动触发遇到版本冲突时的最大尝试次数和默认毫秒退避。
- `GetTimerOption = Box<dyn Fn(&mut TimerCond) + Send + Sync>`：查询选项。`WithKey` 设置精确 Key 并关闭前缀模式；`WithKeyPrefix` 设置 Key 并开启前缀模式；`WithID` 设置 ID；`WithTag` 设置必须全部包含的标签。实际匹配语义由 `store.rs::TimerCond::Match` 决定。
- `UpdateTimerOption = Box<dyn Fn(&mut TimerUpdate) + Send + Sync>`：更新选项。`WithSetEnable`、`WithSetTimeZone`、`WithSetSchedExpr`、`WithSetWatermark`、`WithSetSummaryData`、`WithSetTags` 仅把对应字段标为 Present；验证和落库由存储层负责。
- `TimerClient`：要求实现者同时满足 `Send + Sync` 的对象安全 trait，公开 `GetDefaultNamespace`、`CreateTimer`、两种单条查询、条件查询、更新、手动触发、关闭事件和删除。
- `DefaultStoreNamespace = "default"`：默认客户端的命名空间。
- `DefaultTimerClient { namespace, store, retryBackoff }`：可克隆客户端。`store` 是内部持有 `Arc<dyn TimerStoreCore>` 的 `TimerStore`，`retryBackoff` 公开是为了测试或调用方调节冲突退避。
- `NewDefaultTimerClient(TimerStore) -> DefaultTimerClient`：以默认命名空间和 1000ms 退避构造具体实现；与 Go 版返回接口不同，Rust 版返回具体类型，仍可转成 `dyn TimerClient`。

## 执行流程

1. 普通 CRUD：`CreateTimer` 在 `TimerSpec.Namespace` 为空时填入 `self.namespace`，调用 `TimerStore::Create` 得到 ID，再用 `GetByID` 回读；`GetTimerByID`、`GetTimerByKey` 和 `DeleteTimer` 直接委托存储。`GetTimerByKey` 固定使用客户端命名空间。
2. 条件查询/更新：`GetTimers` 从空 `TimerCond` 开始按传入顺序执行所有 `GetTimerOption`，再调用 `List`；`UpdateTimer` 同理生成 `TimerUpdate` 并调用 `Update`。同一字段被多个 Option 设置时，后执行者覆盖前值。
3. 手动触发：`ManualTriggerEvent` 先生成一个 32 位十六进制 UUID 字符串，最多尝试五次。每次先回读最新记录；若 `EventID` 非空或定时器未启用则立即返回业务错误。否则写入含请求 ID、当前时间和 120 秒超时的 `ManualRequest`，并把读到的 `Version` 放入 `CheckVersion`。更新成功即返回原请求 ID；仅版本冲突会在尚有次数时睡眠后重读重试，其他错误立即传播；第五次仍冲突则返回 `ErrVersionNotMatch`。
4. 关闭事件：`CloseTimerEvent` 先应用调用方 Option，然后通过 `TimerUpdate::FieldsSet` 确认除 `Watermark`、`SummaryData` 外没有其他显式字段。它读取当前记录，设置 `CheckEventID`，把状态改为 `SchedEventIdle`，清空 ID/数据/开始时间和 `EventExtra`。调用方未指定水位线时，使用记录的 `EventStart`（包括其为 `None` 的情况），最后一次性提交存储更新。

## 数据与状态

Option 闭包捕获输入值，并在每次调用时克隆字符串、标签或字节数组；因此一个 Option 对象理论上可重复应用，但公开客户端方法接收 `Vec<Box<...>>` 并消费整组 Option。`OptionalVal` 的 Present 状态区分“未修改”与“显式写入空值”：例如空标签、空摘要、`Watermark = None` 都可作为真实更新。

客户端自身只有命名空间、共享存储句柄和退避参数，没有缓存或后台状态。`CreateTimer` 只在规格命名空间为空时应用默认值；`GetTimerByKey` 总在默认命名空间查询，但 `GetTimers` 不会自动注入命名空间条件，因此无 Option 的列表查询由底层存储决定范围，可能跨命名空间。

手动请求 ID 在整个重试循环外生成，所有冲突重试复用同一 ID。版本号来自每轮重新读取的记录，构成读—校验—条件更新的乐观并发协议。关闭事件使用 `CheckEventID` 防止旧工作者关闭已经切换到另一事件的记录；该路径没有版本重试。

## 依赖与调用关系

下游直接依赖如下：

- `error.rs::{TimerError, TimerResult, ErrVersionNotMatch}` 提供统一错误和冲突判别。
- `store.rs::{Context, TimerCond, TimerStore, TimerUpdate}` 提供请求上下文、查询/更新载体与持久化门面。`TimerStore::{Create,List,Update,Delete,GetByID,GetByKey}` 最终转发到线程安全的 `TimerStoreCore`。
- `timer.rs::{TimerSpec, TimerRecord, ManualRequest, EventExtra, Timestamp, SchedEventIdle, now_timestamp}` 提供领域数据和时间。
- 外部 `uuid` crate 的 v4 功能生成手动请求 ID；`std::thread::sleep` 和 `Duration` 实现同步退避与 120 秒请求超时。

上游生产调用的直接证据包括：`pkg/timer/runtime/runtime.rs::ensureWorker` 构造客户端供 Hook 使用；`pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command` 调用 `GetTimerByKey`、`ManualTriggerEvent`、`GetTimerByID`；`pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook::OnSchedEvent` 的后台线程调用 `GetTimerByID`、`CloseTimerEvent`，并组合 `WithSetWatermark`/`WithSetSummaryData`。`hook.rs::HookFactory` 的签名使用 `Box<dyn TimerClient>`，形成运行时与具体客户端的抽象边界。

## 错误处理与边界

除本文件显式构造的业务错误外，所有存储错误均用 `?` 或原样返回传播。`ManualTriggerEvent` 的特殊边界是：已有未关闭事件时报 `manual trigger is not allowed when event is not closed`；禁用时报 `manual trigger is not allowed when timer is disabled`；只重试精确等于 `ErrVersionNotMatch` 的错误，读取失败及其他更新失败均不重试。

`CloseTimerEvent` 只允许调用者提供 Watermark 和 SummaryData；违规字段名来自 `TimerUpdate::FieldsSet` 的固定顺序并进入错误文本。合法请求仍可能因定时器不存在、读取失败或 `CheckEventID` 不匹配而失败。字段限制发生在读存储之前，因此非法 Option 会优先返回参数错误。函数没有检查 EventID 是否为空，而是把一致性判定交给存储层。

空 Option 列表是合法输入：查询会提交空条件，更新会提交空补丁。是否允许创建重复 Key、时区/调度表达式是否合法、空补丁是否增加版本等均不是本文件的保证，应以具体 `TimerStoreCore` 和模型校验为准。

## 并发与资源生命周期

`TimerClient: Send + Sync`、Option 的 `Send + Sync` 约束以及 `TimerStore` 内部的 `Arc<dyn TimerStoreCore>` 允许客户端在线程间共享；`DefaultTimerClient` 的 `Clone` 复制字符串/退避值并克隆共享存储句柄，不复制底层存储。生产代码确实在 `SqlTtlTimerHook` 中把它转成 `Arc<dyn TimerClient>` 后移入多个后台任务。

手动触发的正确性依赖 `CheckVersion` 原子校验，而关闭事件依赖 `CheckEventID` 原子校验。重试使用阻塞式 `thread::sleep`：默认一次冲突会占用当前线程约一秒，最多在前四次失败后睡眠；`retryBackoff == 0` 时不睡眠。传入的 `Context` 会转交每次存储操作，但退避睡眠本身不观察取消信号。

本客户端没有 `Drop` 或 `Close`，也不拥有运行线程；存储关闭、Watch 通道和 Hook 后台任务的生命周期由上层负责。手动请求的 120 秒 `ManualTimeout` 是写入记录的业务期限，不会在客户端内启动计时器。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `pkg/timer/api/client.go`：Option 名称、`TimerClient` 方法集合、默认命名空间、CRUD 委托、手动触发前置条件和版本冲突重试、关闭事件的字段白名单及状态清理均保持一致。`pkg/timer/api/client_test.rs` 对照 `client_test.go` 验证主要语义，`client_1_aster_unit_test.rs` 另补充 AsterSQL 侧覆盖。

可见实现差异如下：Go `NewDefaultTimerClient` 返回接口且存储为指针，Rust 返回具体 `DefaultTimerClient` 并通过内部 `Arc` 共享；Go 的可变参数在 Rust 中表现为 `Vec<Box<dyn Fn...>>`。Go 的 `SchedPolicyType` 是专用类型，Rust Option 接受 `String`。Go 使用 `util.RunWithRetry` 并记录版本冲突警告，Rust 显式循环且不写日志。两边请求 ID 都是无分隔符 UUID 十六进制文本。Go 用零 `time.Time` 清空事件开始时间，Rust 的等价模型是 `Option<Timestamp>::None`。

Rust 版在 API 形态和可观测日志上并非逐字等价，但现有独立测试证明关键状态转换与错误结果对齐；文档不据此推断未测试的具体存储实现完全等价。

## 扩展指南

- 新增查询维度时，在 `TimerCond`/具体存储先定义匹配语义，再在本文件增加 `With...` 构建器，并同步 `client_test.rs::test_get_timer_option` 与 Go 的 `TestGetTimerOption`。若需要默认命名空间过滤，应明确选择修改 `GetTimers`，避免意外改变现有跨命名空间行为。
- 新增可更新字段时，在 `TimerUpdate` 和存储应用逻辑完成接线，再增加 `WithSet...`；同步 `test_update_timer_option`，并审查它是否应进入 `CloseTimerEvent` 的白名单。不要仅加 Option 而遗漏持久化层。
- 修改手动触发协议时，重点保持“每轮重读—同一请求 ID—版本条件更新”的不变量，并扩展 `client_test.rs::manual_trigger_retries_version_conflicts_like_go`、`test_default_client_manual_trigger_retry` 及 Go 的 `TestDefaultClientManualTriggerRetry`。调整退避时需评估阻塞线程和 Context 取消响应。
- 修改关闭事件时，应同步验证允许字段、默认 Watermark、`CheckEventID`、事件字段清理和手动请求保留行为；主要回归入口是 `client_test.rs::test_default_client`，TTL 使用场景还涉及 `pkg/session/runtime/ttl_timer.rs`。
- 测试逻辑继续放在独立的 `client_test.rs` 或 `client_1_aster_unit_test.rs`，不要内嵌到生产源文件；Go 对齐修改同时检查 `client.go`/`client_test.go` 的原始意图。

## 验证依据

- 源码与边界：`pkg/timer/api/client.rs`、`store.rs`、`error.rs`、`timer.rs`、`lib.rs`、`Cargo.toml`。
- Rust 独立测试：`pkg/timer/api/client_test.rs` 覆盖全部 Option、默认 CRUD、事件关闭、手动触发与版本冲突；`pkg/timer/api/client_1_aster_unit_test.rs` 提供补充的端到端及冲突测试。测试由 `lib.rs` 的 `#[cfg(test)] include!` 接入，未与生产文件混放。
- Go 对照：`pkg/timer/api/client.go`、`pkg/timer/api/client_test.go`。
- 生产调用证据：`pkg/timer/runtime/runtime.rs::ensureWorker`、`pkg/session/runtime/ttl_runtime.rs::trigger_ttl_command`、`pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook::OnSchedEvent`、`pkg/timer/api/hook.rs::HookFactory`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query TimerClient`、`query DefaultTimerClient`、`query ManualRequest` 定位了 Rust/Go 定义及 TTL/Hook 关联。对本文件执行 `explore` 以及带精确节点 ID 的 `callers/callees` 未返回调用边并超时，因此调用关系改由上述源码级 `rg` 搜索核验，不把缺失图边当成“无调用者”。
- 按任务约束未运行 Cargo；验证限于事实复核和 Markdown 结构检查。
