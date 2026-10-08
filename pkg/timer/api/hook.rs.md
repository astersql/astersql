# `pkg/timer/api/hook.rs`

## 文件定位

`hook.rs` 属于 `astersql-timer-api` crate，是定时器框架与业务调度逻辑之间的公共契约层。crate 根模块 `pkg/timer/api/lib.rs` 以 `pub mod hook` 声明本模块，并通过 `pub use hook::*` 再导出这里的 API；`pkg/timer/api/Cargo.toml` 则确认该 crate 对应 Go 包 `pkg/timer/api`。本文件只定义事件视图、前置回调结果、生命周期 trait 和工厂类型，不负责选择到期定时器、更新存储或执行具体业务。

运行时的直接承接者位于 `pkg/timer/runtime/runtime.rs` 与 `pkg/timer/runtime/worker.rs`：前者按 `HookClass` 注册和查找 `HookFactory`，后者把实际定时器记录包装成 `TimerEvent`，再通过动态分派调用 `Hook`。一个真实业务实现是 `pkg/session/runtime/ttl_timer.rs` 的 `SqlTtlTimerHook`。

## 核心职责

- `TimerShedEvent` 为 hook 暴露本次候选事件的 ID 和定时器快照，隔离 worker 的内部请求结构（`pkg/timer/api/hook.rs:28-33`）。名称中的 `Shed` 与 Go 原接口保持一致，并非另一个“schedule event”类型。
- `PreSchedEventResult` 让前置回调选择立即触发或延迟重试，并把预计算的二进制数据交给即将写入存储的事件（`hook.rs:35-42`；消费位置 `pkg/timer/runtime/worker.rs:412-436,501-525`）。
- `Hook` 规定实例的启停和调度前后两个回调，是框架控制生命周期、业务控制触发条件及后续动作的边界（`hook.rs:44-61`）。
- `HookFactory` 把 hook class 和具备存储访问能力的 `TimerClient` 交给业务方，用于构造独立 hook 实例（`hook.rs:63-65`）。

本文件没有默认实现、全局注册表或内部可变状态。其行为语义来自 trait 契约和 runtime 对返回值的处理，不能脱离 `worker.rs` 单独理解。

## 主要符号

### `pub trait TimerShedEvent: Send + Sync`

- `fn EventID(&self) -> String` 返回本次调度尝试的事件 ID。调用者取得拥有所有权的字符串，因而实现通常会克隆内部值；runtime 的 `TimerEvent::EventID` 正是如此（`pkg/timer/runtime/worker.rs:155-168`）。
- `fn Timer(&self) -> Option<TimerRecord>` 返回定时器记录快照。`Option` 允许实现表达“记录尚不可用”，业务实现必须处理 `None`；TTL hook 将它转为 `TimerError("TTL timer missing")`（`pkg/session/runtime/ttl_timer.rs:108-118,137-151`）。当前 runtime 的 `TimerEvent` 总是返回 `Some(self.record.clone())`。
- `Send + Sync` 允许事件视图跨线程传递或共享。回调参数只是借用 `&dyn TimerShedEvent`，hook 不能在不额外复制数据的情况下把该引用保存到回调之外。

### `pub struct PreSchedEventResult`

该值派生 `Clone`、`Debug`、`Default`、`Eq`、`PartialEq`。`Default` 等价于零延迟和空载荷：

- `Delay: Duration` 大于零时，本次触发被推迟，worker 直接返回带该时长的重试响应，不写入 Trigger 状态，也不调用 `OnSchedEvent`（`worker.rs:421-432`）。零值进入正常触发路径。
- `EventData: Vec<u8>` 在零延迟且存储更新成功时写入 `TimerUpdate.EventData`，随后可从触发态的 `TimerRecord` 读取（`worker.rs:436,501-512`）。该字段不解释编码格式，格式由具体 hook 与其消费者共同约定。

### `pub trait Hook: Send`

- `Start(&mut self)` 在一个 worker 会话开始处理请求前调用。
- `Stop(&mut self)` 在该会话退出时调用，用于终止后台工作并释放资源。
- `OnPreSchedEvent(&mut self, &Context, &dyn TimerShedEvent) -> TimerResult<PreSchedEventResult>` 仅在请求快照仍为 `SchedEventIdle` 时调用，可延迟、附加数据或返回错误。
- `OnSchedEvent(&mut self, &Context, &dyn TimerShedEvent) -> TimerResult<()>` 在存储中的记录已重新读取、且 `EventID` 仍匹配后调用；对于已处于 Trigger 状态的请求，会跳过前置回调而直接走这里。

trait 要求 `Send`，但不要求 `Sync`。所有方法都接收 `&mut self`，表达同一实例的回调需要独占访问；当前 `HookWorker` 在线程内串行持有和调用实例（`worker.rs:296-337`）。

### `pub type HookFactory`

完整类型为 `Arc<dyn Fn(String, Box<dyn TimerClient>) -> Box<dyn Hook> + Send + Sync + 'static>`。`Arc` 使注册表和运行时可以廉价克隆工厂；`Fn` 允许捕获并复用环境；`Send + Sync + 'static` 允许其存入共享 runtime 并由后台线程调用。参数分别是注册时的 hook class 和绑定当前 timer store 的客户端，结果的所有权交给 worker。

## 执行流程

1. 调用方通过 `TimerRuntimeBuilder::RegisterHookFactory` 将 `HookFactory` 按 hook class 放入 runtime 的 `HashMap`（`pkg/timer/runtime/runtime.rs:117-125,140-149`）。
2. runtime 发现某类定时器需要 worker 时，`ensureWorker` 查找并克隆工厂，构造 `NewDefaultTimerClient(store.clone())`，再闭包化成无参 `HookFactoryFn`；每个 hook class 的 worker 被缓存复用（`runtime.rs:414-459`）。
3. `runWorkerSession` 调工厂取得 `Box<dyn Hook>`，先执行 `Start`，再从有界通道逐个取请求；会话结束时执行 `Stop`（`pkg/timer/runtime/worker.rs:296-337`）。
4. 对 Idle 记录，`triggerEventWithCounters` 构造包含候选 `eventID` 和记录快照的 `TimerEvent`，调用 `OnPreSchedEvent`（`worker.rs:412-433`）。
5. 若 `Delay > 0`，worker 按指定时长重试；若回调报错，按默认间隔重试。两种情况都不推进事件状态。若成功且无延迟，`EventData` 随 `EventID`、`EventStart`、watermark 等一起更新到存储（`worker.rs:421-451,501-525`）。
6. worker 重新读取记录以处理并发更新，确认 `EventID` 仍是本次候选值后，构造新的事件视图调用 `OnSchedEvent`。成功返回完成响应；失败保留最新记录并请求默认重试（`worker.rs:454-476`）。
7. 上下文取消、通道断开或会话异常会结束当前会话；外层 worker 循环可在 panic 后重建 hook。`Stop` 仍在会话清理路径执行，随后才决定是否恢复传播 panic（`worker.rs:251-280,296-337`）。

## 数据与状态

本文件自己的类型均不持有全局状态：`PreSchedEventResult` 是一次调用的拥有型返回值，`HookFactory` 是共享的不可变函数对象，两个 trait 只描述行为。

事件数据有两个容易混淆的层次：`TimerShedEvent::EventID()` 是 runtime 为本次尝试生成的候选 ID；在 `OnPreSchedEvent` 阶段，事件视图中的 `TimerRecord` 仍可能是 Idle 快照，其 `EventID` 尚未写入。业务代码应使用 `event.EventID()` 识别本次事件。只有前置回调成功且无延迟后，worker 才将候选 ID 与 `PreSchedEventResult.EventData` 原子地组织进一次 `TimerUpdate`（Go 契约见 `pkg/timer/api/hook.go`，Rust落库见 `worker.rs:412-451,501-525`）。

当前 runtime 的 `TimerEvent` 对 `EventID` 和 `TimerRecord` 都采用克隆返回，hook 修改所得值不会反向修改 worker 快照。真正的状态推进通过 `TimerStore::Update` 完成，并带 `CheckVersion` 做并发版本检查；本文件不暴露直接修改记录的入口。

## 依赖与调用关系

本文件的直接 Rust 依赖很小：

- `crate::client::TimerClient`：工厂收到的客户端抽象。
- `crate::error::TimerResult`：两个业务回调统一使用的错误通道。
- `crate::store::Context`：向回调传递取消/执行上下文。
- `crate::timer::TimerRecord`：事件视图返回的记录快照。
- `std::sync::Arc` 与 `std::time::Duration`：分别承载共享工厂和延迟值。

主要上游是 `pkg/timer/runtime/runtime.rs` 的 `RegisterHookFactory`/`ensureWorker` 和 `pkg/timer/runtime/worker.rs` 的 `TimerEvent`、`runWorkerSession`、`triggerEventWithCounters`。主要实现者包括 `pkg/session/runtime/ttl_timer.rs::SqlTtlTimerHook`；该实现利用前置回调检查 TTL 开关、表存在性和调度窗口，并在后置回调中启动/跟踪 TTL 作业。测试实现位于 `pkg/timer/api/hook_test.rs`、`pkg/timer/runtime/main_test.rs` 和 `pkg/timer/runtime/worker_test.rs`。

RustCodeGraph 对 trait 动态调用的精确 `callers` 查询未产生边，但符号查询定位到了上述实现与 worker；源码中的 `hook.OnPreSchedEvent`、`hook.OnSchedEvent`、`hook.Start`、`hook.Stop` 提供了直接动态分派证据。

## 错误处理与边界

- `Start` 和 `Stop` 没有返回值；初始化/清理失败只能由实现内部处理或 panic。runtime 使用 `catch_unwind` 隔离 panic，并在未取消时重启会话（`worker.rs:251-280,305-336`）。
- `OnPreSchedEvent` 返回 `Err` 时，worker 不落库、不调用后置回调，改用默认重试响应；非零 `Delay` 不是错误，而是使用指定延迟的正常控制流（`worker.rs:421-432`）。
- `OnSchedEvent` 返回 `Err` 时，事件已经可能处于 Trigger 状态；worker 携带重新读取的记录重试，不回滚前置落库（`worker.rs:454-473`）。实现必须让后置处理可重入，不能假设只调用一次。
- `Timer()` 的 `None` 是接口允许的边界，即使当前 runtime 实现恒为 `Some`。新实现者不得无条件解包，除非调用环境明确保证该不变量。
- 回调执行期间若记录发生版本冲突、被删除或换成其他 `EventID`，处理逻辑由 worker 在调用边界外完成；hook 不应把事件视图当作长期有效的存储事实。
- `EventData` 是无模式字节数组，本层不验证大小、编码或兼容性。新增格式应由业务层版本化并覆盖旧数据读取。

## 并发与资源生命周期

`HookFactory` 可跨线程共享，而具体 `Hook` 只要求可移动到 worker 线程。当前每个 hook class 懒创建一个 `HookWorker`，同一 worker 会话只持有一个 hook，并从有界 channel 串行处理请求，所以 `&mut self` 回调不会在同一实例上并发执行（`pkg/timer/runtime/runtime.rs:414-459`；`pkg/timer/runtime/worker.rs:228-280,296-337`）。若 hook 自己派生线程或共享内部对象，则其实现仍须自行同步。

生命周期边界是工厂创建、`Start`、零到多次回调、`Stop`、实例释放。上下文取消会让接收循环退出；runtime 的 `stopAndJoin` 等待 worker 线程结束（`worker.rs:339-349`）。TTL 实现展示了资源型 hook 的做法：保存作业 `JoinHandle`，在 `Stop` 设置原子停止标志、唤醒并 join 所有任务（`pkg/session/runtime/ttl_timer.rs:24-43,97-106`）。

`TimerShedEvent` 的 `Send + Sync` 不表示传入引用具有 `'static` 生命周期。异步任务需要先复制 `EventID` 和 `TimerRecord` 中所需字段；TTL hook 正是在创建线程前取得拥有型值（`ttl_timer.rs:147-169`）。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/timer/api/hook.go`，四个公共概念一一对应：`TimerShedEvent`、`PreSchedEventResult`、`Hook`、`HookFactory`。Rust runtime 的前置延迟、`EventData` 落库、后置回调和按 hook class 复用 worker，也与 `pkg/timer/runtime/worker.go`、`runtime.go` 的流程一致。

需要注意的语言映射差异：

- Go 的 `Timer() *TimerRecord` 用 `nil` 表达缺失；Rust 显式使用 `Option<TimerRecord>`，同时返回拥有型克隆而非指针。
- Go 的 `context.Context` 是接口值；Rust 传 `&Context`。两者都由 runtime 提供，hook 不拥有其生命周期。
- Go `HookFactory` 是普通函数类型；Rust 用 `Arc<dyn Fn(...) + Send + Sync + 'static>` 表示可捕获、可共享的闭包。`pkg/timer/api/hook_test.rs::hook_factory_accepts_capturing_closures_like_go` 专门验证捕获闭包可用。
- Go hook 接口没有显式并发标记；Rust 对事件要求 `Send + Sync`、对 hook 要求 `Send`、对工厂要求 `Send + Sync`，把当前线程模型编码进类型系统。
- Go 的 `TimerClient` 作为接口值传入；Rust 工厂接收 `Box<dyn TimerClient>`，明确转移客户端对象所有权。

当前 Rust API 和核心 worker 行为已有对应实现，并非占位门面；不过具体业务 hook 的语义仍应逐个与其 Go 实现核对，不能仅凭本接口宣称所有业务迁移完成。

## 扩展指南

新增 hook 实现时，应在独立生产文件中实现 `Hook`，通过 `TimerRuntimeBuilder::RegisterHookFactory` 注册，测试也放在独立 `*_test.rs` 文件中，不要嵌入本源文件。最小安全检查包括：

1. `Start`/`Stop` 成对且可应对部分初始化；`Stop` 必须终止并回收实现创建的后台任务。
2. `OnPreSchedEvent` 将“暂不可触发”表达为正 `Delay`，将真正失败表达为 `TimerResult::Err`；不要在返回延迟时假设 `EventData` 会被保存。
3. `OnSchedEvent` 按至少一次调用设计，依据 `event.EventID()` 去重；触发态已落库，失败重试不能依赖回滚。
4. 对 `Timer()` 的 `None`、陈旧记录、上下文取消以及客户端/store 错误给出明确策略。
5. 若扩展 `PreSchedEventResult` 或改变 `HookFactory` 签名，需要同步 `pkg/timer/runtime/worker.rs`、`runtime.rs`、Go 对照接口及所有实现者；这是公共 API 兼容性变更。
6. `EventData` 新格式需定义版本/兼容策略，并在 worker 成功、延迟、前置错误、后置错误及重复调用路径补测试。最接近的回归位置是 `pkg/timer/runtime/worker_test.rs`；工厂类型行为在 `pkg/timer/api/hook_test.rs`；具体 TTL 行为应在 session/TTL 对应的独立测试中覆盖。

性能上，`EventID()` 与 `Timer()` 每次都会克隆，尤其 `TimerRecord` 可能包含二进制数据；高频实现应避免重复调用。若要改成借用返回，必须评估 trait object 生命周期和所有调用者，不能只改本文件。

## 验证依据

- 目标定义：`pkg/timer/api/hook.rs:20-65`。
- crate 边界与再导出：`pkg/timer/api/Cargo.toml`、`pkg/timer/api/lib.rs`。
- Go 语义来源：`pkg/timer/api/hook.go`；运行时对照为 `pkg/timer/runtime/runtime.go::ensureWorker`、`pkg/timer/runtime/worker.go::triggerEvent`。
- Rust 主调用链：`pkg/timer/runtime/runtime.rs:117-125,414-459`；`pkg/timer/runtime/worker.rs:155-168,251-280,296-337,368-477,501-525`。
- 真实实现：`pkg/session/runtime/ttl_timer.rs:24-43,97-250`。
- 独立测试：`pkg/timer/api/hook_test.rs::hook_factory_accepts_capturing_closures_like_go`；`pkg/timer/runtime/worker_test.rs::TestWorkerStartStop`、`TestWorkerProcessIdleTimerSuccess`、`TestWorkerProcessTriggeredTimerSuccess`、`TestWorkerProcessDelayOrErr`、`TestHookWorkerLoopPanicRecover`。这些测试分别覆盖工厂捕获、生命周期、前置结果落库、跳过前置回调、延迟/错误重试及 panic 恢复。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、目标文件 17 个符号；`query TimerShedEvent/PreSchedEventResult/Hook/HookFactory/OnPreSchedEvent/OnSchedEvent` 定位公共定义、TTL 实现、runtime 与测试；`callees` 确认 `runWorkerSession` 调用 `Start`/`Stop`，并定位各业务实现的下游调用。动态 trait 的 `callers` 输出为空，因此调用结论进一步以 runtime 精确源码位置核验。
- 本任务是纯文档分析，未运行 Cargo；交付前另以任务指定命令校验目标文件存在且固定二级章节恰为 11 个。
