# [`pkg/timer/api/store.rs`](store.rs)

## 文件定位

本文件是 `astersql-timer-api` crate 的存储契约层。`pkg/timer/api/lib.rs` 将 `store` 声明为公共模块并再导出其 API；`pkg/timer/api/Cargo.toml` 则表明该 crate 直接依赖 `crossbeam-channel`（Watch 通道）与 `chrono`（时区回退），并以 `pkg/timer/api` 为对应 Go 包。

它位于领域模型 `timer.rs` 与具体存储之间：上游 `DefaultTimerClient` 把客户端选项组装成 `TimerCond`/`TimerUpdate` 后调用 `TimerStore`，timer runtime 通过 `Watch` 接收增量事件；下游 `MemoryStoreCore` 和 `TableTimerStoreCore` 实现 `TimerStoreCore`，内存及 etcd 通知器实现 `TimerWatchEventNotifier`。因此本文件定义“存储能做什么”和通用的条件/补丁语义，但不负责真正的内存加锁、SQL 持久化或 etcd 通信。

## 核心职责

- 用 `Context`/`CancelContext` 提供可跨线程共享的最小取消信号，主要服务 Watch 生命周期。
- 用 `OptionalVal<T>` 区分“字段未出现”与“字段显式设置为值”；对 `OptionalVal<Option<Timestamp>>` 而言还可表达“显式清空时间”。
- 用 `TimerCond`、`Cond`、`Operator` 形成内存可求值且可组合的过滤条件。
- 用 `TimerUpdate` 表示部分更新，并在 `apply` 中先执行版本与事件 ID 的乐观并发检查，再克隆记录并应用补丁。
- 用 `TimerStoreCore` 统一 CRUD 与 Watch 后端，用 `TimerStore` 提供动态分派门面及 `GetByID`/`GetByKey` 便利方法。
- 用 Watch 事件常量、响应结构、接收通道别名和 `TimerWatchEventNotifier` 连接存储变更与 runtime 缓存刷新。

## 主要符号

- `Context { cancelled: Arc<AtomicBool> }`：可克隆的取消观察端。`background` 与 `todo` 都创建未取消上下文；`with_cancel` 同时返回持有 `Weak<AtomicBool>` 的 `CancelContext`；`is_cancelled` 使用 Acquire 读取。
- `CancelContext::cancel`：仅当对应 `Context` 仍存活时用 Release 写入取消标志；句柄本身不会延长上下文寿命。
- `OptionalVal<T>` 与 `NewOptionalVal`：内部以 `Option<T>` 保存存在性；`Present`、`Get`、`Set`、`Clear` 分别查询、借用、设置和移除值。
- `TimerCond`：公开字段为 `ID`、`Namespace`、`Key`、`KeyPrefix`、`Tags`。`Match` 对前三类值做精确/前缀判断，并要求记录包含查询中的全部标签；`FieldsSet` 按固定声明顺序返回已设置的可选字段；`Clear` 还会复位非可选字段 `KeyPrefix`。
- `TimerUpdate`：包含 13 个可写业务字段及 `CheckVersion`、`CheckEventID` 两个校验字段。`apply` 是 crate 内私有方法；`FieldsSet` 包括业务与校验字段；`Clear` 恢复空补丁。
- `Cond: Any + Send + Sync`：统一匹配接口；`as_any` 让 `Not` 能识别并复制现有 `Operator`。
- `OperatorTp`、`Operator`、`And`、`Or`、`Not`：构造条件树。`Operator::Match` 使用迭代器短路求值，再以 `matched != Not` 统一完成取反。空 AND 为真、空 OR 为假，这是 `all`/`any` 的自然结果。
- `WatchTimerEventType` 及 `WatchTimerEventCreate/Update/Delete`：`i8` 位标志，值分别为 1、2、4；当前 runtime 按单一值 `match`，未知值会忽略，并未把组合位掩码拆分处理。
- `WatchTimerEvent`、`WatchTimerResponse`、`WatchTimerChan`：分别携带事件类型和 timer ID、事件批次，以及 `crossbeam_channel::Receiver<WatchTimerResponse>`。
- `TimerStoreCore: Send + Sync`：后端契约，涵盖 `Create`、`List`、`Update`、`Delete`、`WatchSupported`、`Watch`、`Close`。
- `TimerStore { TimerStoreCore: Arc<dyn TimerStoreCore> }`：可克隆的类型擦除门面。`from_core` 包装具体后端，其同名方法直接委托；`GetByID`/`GetByKey` 构造条件并调用私有 `getOneRecord`。
- `TimerWatchEventNotifier: Send + Sync`：通知器契约，定义订阅、发布与关闭。

## 执行流程

1. 客户端侧，`DefaultTimerClient::{CreateTimer,GetTimerByID,GetTimerByKey,GetTimers,UpdateTimer}` 调用 `TimerStore`。查询选项写入 `TimerCond`，更新选项写入 `TimerUpdate`；手动触发还设置 `CheckVersion`，以版本冲突作为重试信号（`pkg/timer/api/client.rs`）。
2. `TimerStore` 将 CRUD 调用动态分派给 `Arc<dyn TimerStoreCore>`。`GetByID` 或 `GetByKey` 先构造精确条件，`getOneRecord` 调 `List`，返回第一条；列表为空时返回 `ErrTimerNotExist`，不会自行检查后端是否错误地返回多条。
3. 内存后端的 `List` 直接调用 `Cond::Match`；表后端将受支持条件转换成 SQL。组合条件由 `Operator::Match` 递归求值，`And` 要求全真，`Or` 要求至少一真，最后按 `Not` 翻转。
4. 内存后端更新时调用 `TimerUpdate::apply`：先比对 `CheckVersion`，再比对 `CheckEventID`；均通过后克隆原记录，只覆盖 Present 的字段。设置 `TimeZone` 时同步重算 `Location`；完成后由后端校验记录、增加版本并写回。
5. CRUD 成功后，具体后端经 `TimerWatchEventNotifier::Notify` 发布 Create/Update/Delete。runtime 的 `createWatchTimerChan` 在后端支持 Watch 时订阅，否则使用永不就绪的通道；`batchHandleWatchResponses` 对 Create/Update 刷新缓存，对 Delete 移除缓存（`pkg/timer/runtime/runtime.rs`）。

## 数据与状态

`OptionalVal<T>` 的核心不变量是“存在性独立于值”。例如 `OptionalVal<bool>` 能区分不更新与更新为 `false`，`OptionalVal<Vec<_>>` 能区分不更新与更新为空集合，`OptionalVal<Option<Timestamp>>` 能进一步区分不更新、设置时间、显式清空时间。`Get` 返回借用，调用方需要在写入记录时按类型复制或克隆。

`TimerCond::FieldsSet` 和 `TimerUpdate::FieldsSet` 使用硬编码字段表而非 Rust 反射；返回顺序稳定等于源码中的表顺序，`excludes` 按字段名过滤。新增可选字段若没有同步加入该表，会导致 SQL 构造或诊断遗漏。`TimerCond::KeyPrefix` 本身不计入 FieldsSet，只有 Key Present 时才影响匹配。

`TimerUpdate::apply` 不修改输入记录，而是返回克隆后的新记录。`CheckVersion`/`CheckEventID` 只做前置条件检查，不写入结果；记录版本递增是具体后端职责。时区更新先尝试 `parse_location(value)`，失败则尝试默认位置，最后回退到当前本地固定偏移，因此 `apply` 本身不会因无效时区返回错误；后端后续的 `TimerRecord::Validate` 或 SQL 路径可能施加额外约束。

Watch 响应只传 ID，不传完整记录；消费方必须重新读取 Create/Update 对应记录。事件批次是拥有所有权的 `Vec`，接收端为多生产者/消费者通道的 `Receiver`。

## 依赖与调用关系

- 上游 API：`pkg/timer/api/client.rs` 的 `DefaultTimerClient` 是直接业务调用者；它也利用 `ErrVersionNotMatch` 实现手动触发重试。
- 上游 runtime：`pkg/timer/runtime/runtime.rs` 调用 `WatchSupported`/`Watch`，并根据本文件的事件类型刷新或删除缓存；`worker.rs` 多次用 `GetByID` 在更新后重新读取状态。
- 内存实现：`pkg/timer/api/mem_store.rs` 的 `MemoryStoreCore` 实现 `TimerStoreCore`，以互斥锁保护双索引；其 `MemTimerWatchEventNotifier` 实现通知接口，并为订阅者启动转发线程。
- 持久化实现：`pkg/timer/tablestore/store.rs` 的 `TableTimerStoreCore` 实现同一契约，使用系统会话和 SQL；通知器由 `NewTableTimerStore` 在 etcd 与内存实现间选择。
- 分布式通知：`pkg/timer/tablestore/notifier.rs` 的 `EtcdNotifier` 实现通知接口，把事件入队后由后台线程写入 etcd，并将调用方 Context 取消传播给底层 watch Context。
- 领域依赖：`TimerRecord`、`ManualRequest`、`EventExtra`、`Timestamp`、`TimerLocation` 与 `parse_location` 来自 `pkg/timer/api/timer.rs`；统一结果和四类哨兵错误来自 `error.rs`。
- crate 依赖：`crossbeam-channel` 形成 Watch API，`chrono` 提供本地偏移；`std::sync::{Arc,Weak}` 和原子布尔值承载共享所有权与取消状态。

## 错误处理与边界

`TimerUpdate::apply` 明确返回 `ErrVersionNotMatch` 或 `ErrEventIDNotMatch`，且失败发生在克隆和写字段之前。字段赋值阶段没有独立错误；后端负责检查 `None` 参数、记录存在性、领域校验、唯一性、SQL 错误和版本递增。

`TimerStore::getOneRecord` 原样传播 `List` 错误；空列表映射为 `ErrTimerNotExist`；多条结果只取第一条。因此唯一性依赖 ID 或 `(Namespace, Key)` 的后端约束，而不是门面强制验证。

`TimerCond` 的 Tags 是“全部包含”语义；Present 的空标签列表对任意记录都匹配。`KeyPrefix` 在 Key 未设置时不起作用。条件树允许零个孩子，且 `Not` 对已有 Operator 只翻转复制品，不修改原 `Arc` 指向的对象。

`Context` 不是完整的 Go context：没有 deadline、错误原因、值传播或父子取消树；CRUD trait 接收它，但当前内存和表存储实现均以 `_ctx` 忽略取消，仅 Watch 实现主动观察它。调用者不能假定取消会中断正在执行的 CRUD/SQL。

## 并发与资源生命周期

`Context` 克隆共享同一 `AtomicBool`，Acquire/Release 保证取消标志跨线程可见；`CancelContext` 使用弱引用，所以所有 Context 被释放后调用 `cancel` 是安全空操作。取消不可逆，也没有等待原语，Watch worker 采用轮询或桥接线程观察状态。

`TimerStore` 克隆只增加核心实现的 `Arc` 引用计数，允许多个客户端/runtime 共享后端。`TimerStoreCore` 与 `Cond` 均要求 `Send + Sync`，但具体锁、事务及关闭幂等性由实现保证。本文件没有 `Drop` 自动调用 `Close`；资源拥有者必须显式关闭 store/notifier。

内存通知器为每个订阅启动线程，Context 取消或 notifier 关闭后注销订阅；`Close` 清理订阅并等待 worker。etcd 通知器也维护发送线程和 watch 桥接线程，并在 `Close` 时 join。`WatchTimerChan` 的关闭时机因此是接口契约与具体实现共同决定的；本文件自身只暴露接收端。

## 与 Go 版本的对应关系

总体结构逐项对应 `pkg/timer/api/store.go`：OptionalVal、TimerCond、TimerUpdate、组合条件、Watch 数据、TimerStoreCore、TimerStore 便利查询和通知接口均保留；`pkg/timer/api/store_test.rs` 也复现 `store_test.go` 对存在性、字段枚举、匹配真值表、并发检查及不修改原记录的测试意图。

明确差异如下：

- Go `OptionalVal<T>` 以“零值 + present bool”实现，Rust 以 `Option<T>` 实现；Rust 的 `Get` 返回 `Option<&T>`。两者都能表达“present 的空/假值”。
- Go 的 `FieldsSet` 通过反射和字段地址排除，Rust 使用字段名切片与固定表。Rust 更静态，但新增字段必须手工同步，且排除接口不是地址身份。
- Go `SchedPolicyType`/`SchedEventStatus` 是命名字符串类型；Rust `TimerUpdate` 对应字段存 `String`。类型约束更弱，最终合法性依赖领域校验/后端。
- Rust 的 `EventStart`、`Watermark` 是 `OptionalVal<Option<Timestamp>>`，能够显式写入 `None`；Go 对应 `OptionalVal<time.Time>` 用零时间表达清空，表示方式不同。
- Go `apply` 将空 Tags 规范化为 nil；Rust 保留空 `Vec`。过滤语义相同，但内存表示与直接相等比较可能不同。
- Go 用真正的 `context.Context`；Rust `Context` 只有布尔取消标志，且 CRUD 实现当前不消费取消。
- Go 内存时区辅助函数对非法值回退系统时区；Rust `apply` 依次尝试目标时区、默认位置和当前本地固定偏移。合法 IANA 时区会保留为 `TimerLocation::Named`，只有回退路径固定当前偏移，与 Go 持有系统 `Location` 的动态规则不完全相同。
- Go `Operator::Match` 对未知枚举值返回 false；Rust `OperatorTp` 是封闭 enum，安全 Rust 代码不能构造未知值。

## 扩展指南

新增查询字段时，应同时修改 `TimerCond`、`Match`、`FieldsSet` 和 `Clear`，再同步内存 List 与 `pkg/timer/tablestore/sql.rs` 的 SQL 条件翻译；测试应放在独立的 `pkg/timer/api/store_test.rs`，并补表存储 SQL 测试，不能把测试内嵌进生产文件。

新增可更新字段时，应同步 `TimerUpdate`、`apply`、`FieldsSet`，并检查 `MemoryStoreCore::Update`、表存储的约束检查及 SQL UPDATE 构造。特别确认“未设置/设置默认值/显式清空”三种状态、版本递增、领域校验和 Go 对照语义；时间字段应明确是否需要 `Option<Timestamp>`。

新增组合运算符需要调整封闭的 `OperatorTp` 与 `Operator::Match`，并定义零子节点、短路和 Not 的真值表。新增 Watch 类型时还必须更新 runtime 的事件分派；若希望支持位组合，不能只增加常量，消费端也要从等值匹配改为位测试。

实现新存储后端时，通过 `TimerStore::from_core` 接入，并保证 ID/命名空间键唯一性、更新前置条件、版本策略、Watch 成功通知时机、Context/Close 资源语义与现有实现一致。若后端不支持 Watch，应让 `WatchSupported` 返回 false，runtime 会选择永不触发的通道。

兼容风险主要是 Go/Rust 可选值表示、空 Tags、字符串枚举和时区回退；正确性风险集中于漏同步 FieldsSet/SQL 翻译、错误的通知时机和 CAS 检查；性能风险集中于大条件树递归动态分派、List 后取一条以及 Watch 线程/通道背压。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；目标目录的 Rust/Go 源与测试均已索引。
- 目标源码：`pkg/timer/api/store.rs`（501 行），核对了全部公开类型、trait、常量、函数、impl 与私有 `TimerUpdate::apply`/`TimerStore::getOneRecord`；文件没有条件编译项。
- crate 与入口：`pkg/timer/api/Cargo.toml`、`pkg/timer/api/lib.rs`；确认 crate 名、直接依赖、Go 包元数据、模块声明和公共再导出。
- Go 对照：`pkg/timer/api/store.go`；逐段核对 OptionalVal、条件、更新、组合运算、Watch 和存储接口。
- 独立测试：`pkg/timer/api/store_test.rs` 与 `pkg/timer/api/store_test.go`；核对显式 None/空值、字段顺序与排除、Key 前缀、Tags 全包含、And/Or/Not、CAS 错误、全字段更新及输入不变性。
- 直接调用和实现：RustCodeGraph 查询以及图未精确解析 trait 实现时的 `rg` 复核，定位到 `pkg/timer/api/client.rs`、`pkg/timer/api/mem_store.rs`、`pkg/timer/tablestore/store.rs`、`pkg/timer/tablestore/notifier.rs`、`pkg/timer/runtime/runtime.rs`、`pkg/timer/runtime/worker.rs`。
- 领域与错误：`pkg/timer/api/timer.rs`、`pkg/timer/api/error.rs`；核对 Timestamp、记录字段、时区解析及错误常量。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证固定的 11 个二级章节，并人工复查仅新增本说明和删除完成后的任务文件。
