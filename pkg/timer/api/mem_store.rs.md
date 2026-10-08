# `pkg/timer/api/mem_store.rs`

## 文件定位

本文件是 `astersql-timer-api` crate 的内存版定时器存储和内存 Watch 通知器实现。crate 根模块在 [`lib.rs`](lib.rs) 中公开 `mem_store` 并再导出其公共项；[`Cargo.toml`](Cargo.toml) 的 `package.metadata.porting.go-package` 指向 `pkg/timer/api`，对应的 Go 来源是 [`mem_store.go`](mem_store.go)。

`NewMemoryTimerStore() -> TimerStore` 把 `MemoryStoreCore` 装进 [`TimerStore`](store.rs) 门面。它适合测试、运行时夹具和不需要持久化的本地场景，不提供跨进程共享或重启恢复。`NewMemTimerWatchEventNotifier()` 则是可独立复用的通知器；表存储 [`../tablestore/store.rs`](../tablestore/store.rs) 在没有外部通知器时也会采用它，因此通知器并不只服务于内存 CRUD。

## 核心职责

- 维护两套指向同一逻辑记录的内存索引：`(Namespace, Key) -> TimerRecord` 用于枚举和业务键唯一性，`ID -> TimerRecord` 用于按 ID 更新、删除；写操作必须同步更新二者。
- 实现 [`TimerStoreCore`](store.rs) 的 Create、List、Update、Delete、Watch 和 Close 契约，并把创建、更新、删除转换为单事件 `WatchTimerResponse`。
- 在创建和更新时执行记录校验、版本推进、时区解析以及时间字段归一化。条件匹配和更新补丁本身分别委托给 [`Cond::Match`](store.rs) 与 [`TimerUpdate::apply`](store.rs)。
- 管理 Watch 订阅的注册、广播、取消、关闭及线程回收；慢订阅者不会让 `Notify` 永久占住通知器状态锁。

## 主要符号

- `MemoryStoreData`：私有数据容器。`namespaces: HashMap<String, HashMap<String, TimerRecord>>` 是命名空间/Key 索引，`id2Timers: HashMap<String, TimerRecord>` 是 ID 索引。
- `MemoryStoreCore`：`TimerStoreCore` 的具体实现；`data` 由 `parking_lot::Mutex` 串行保护，`notifier` 由 `Arc<MemTimerWatchEventNotifier>` 共享。
- `NewMemoryTimerStore() -> TimerStore`：创建空索引、创建通知器，并经 `TimerStore::from_core` 返回类型擦除门面。
- `Watcher`：单个订阅的上下文和内部发送端。其 `Context` 决定订阅何时退出。
- `NotifierState`：在一把锁下保存 `closed`、单调递增的 `next_id` 和 `watchers` 表。
- `NotifierInner`：共享可变状态、一次性 shutdown 发送端和所有转发/补发线程的 `JoinHandle`。
- `MemTimerWatchEventNotifier`：`TimerWatchEventNotifier` 实现；自身保留可克隆的 shutdown 接收端。
- `NewMemTimerWatchEventNotifier() -> Arc<dyn TimerWatchEventNotifier>`：向表存储或测试公开通知器 trait object。
- `getMemStoreTimeZoneLoc(tz) -> TimerLocation`：解析时区；失败时退回系统时区。
- `normalizeTimeFields(record)`：若记录有 `Location`，将 `Watermark`、`EventStart`、`CreateTime` 转换到该位置；它改变显示位置/偏移，不改变时间瞬间。

本文件没有条件编译项，也没有内嵌测试模块；测试保持在独立文件中。

## 执行流程

1. **构造**：`NewMemoryTimerStore` 创建空的双索引和通知器，再返回 `TimerStore`。调用者通过门面的同名方法动态分派到 `MemoryStoreCore`。
2. **创建**：`Create` 先拒绝 `None` 以及预填的 ID、Version、CreateTime，再调用 `TimerRecord::Validate`。随后生成无连字符 UUID、解析时区、设置 Version=1/CreateTime/默认 `SchedEventIdle` 并归一化时间。在 `data` 锁内同时检查 ID 与 `(Namespace, Key)` 冲突并写入两套索引，解锁后发出 Create 事件。
3. **查询**：`List` 持锁遍历所有命名空间；没有条件时克隆全部记录，有条件时只克隆 `Cond::Match` 为真的记录。`HashMap` 不承诺返回顺序。`TimerStore::GetByID`/`GetByKey` 在门面层构造条件并复用该流程。
4. **更新**：`Update` 拒绝空补丁，在锁内按 ID 克隆旧记录，调用 `TimerUpdate::apply`（其中包含版本与事件 ID 的乐观并发校验），归一化并重新 `Validate`，版本加一，再覆盖两套索引。成功解锁后发出 Update 事件；任何前置错误都不会写入或通知。
5. **删除**：`Delete` 在锁内从 ID 索引移除记录，再用旧记录的 Namespace/Key 清理另一索引，空命名空间也被移除。不存在返回 `Ok(false)`；实际删除后才发 Delete 事件并返回 `Ok(true)`。
6. **订阅**：`Watch` 建立容量为 8 的内部通道和零容量的外部通道，登记 watcher 后启动转发线程。线程轮询订阅取消、全局 shutdown 和内部消息；收到消息后继续以可取消方式把它交给外部接收者，退出时注销自身。
7. **广播与关闭**：`Notify` 构造只含一条事件的响应。内部通道可立即写入时同步完成；已断开或已取消的 watcher 被移除；容量已满时记录补发任务，释放状态锁后为其启动可取消的发送线程。`Close` 幂等地标记关闭、清空订阅、丢弃 shutdown 发送端使所有接收端获知断开，并 join 已登记线程。

## 数据与状态

双索引的不变量是：每个 `id2Timers[ID]` 都应在 `namespaces[Namespace][Key]` 有一份值相同的克隆，且同一命名空间内 Key 唯一。创建和删除同时维护两边；更新补丁不允许修改 ID、Namespace 或 Key，因此可安全沿用旧记录的业务键更新命名空间索引。

记录由值语义保存和返回：创建时接收所有权，List/Get 返回克隆，Update 先克隆再应用补丁。调用方不能绕过锁直接改变存储内容。版本在创建时固定为 1，每次成功更新恰加 1；失败更新不推进版本。

通知器有独立生命周期状态。`next_id` 只递增、不复用；`closed=true` 后拒绝新订阅和新通知。每个事件响应当前只包含一个 `WatchTimerEvent`，顺序由对同一内部通道的发送顺序决定，但不同 watcher 各自拥有缓冲和转发线程。

## 依赖与调用关系

上游公开路径是 `lib.rs -> mem_store::*`。RustCodeGraph 的文件关系显示本文件被 10 个文件引用，并将 `NewMemoryTimerStore` 的使用定位到 [`client_test.rs`](client_test.rs)、[`client_1_aster_unit_test.rs`](client_1_aster_unit_test.rs)、timer runtime 的独立测试以及 [`../store_intergartion_test.rs`](../store_intergartion_test.rs)；当前仓库中该构造器主要承担真实内存实现的测试/夹具角色。`NewMemTimerWatchEventNotifier` 还有生产下游 [`../tablestore/store.rs`](../tablestore/store.rs)，用于表存储的默认 Watch 通知。

主要下游依赖如下：

- [`store.rs`](store.rs)：`TimerStoreCore`、`TimerStore`、`Cond`、`TimerUpdate`、Watch 类型、`Context` 和通知器 trait。
- [`timer.rs`](timer.rs)：`TimerRecord`、校验、调度状态、时间戳及时区转换。
- [`error.rs`](error.rs)：`TimerResult`、`TimerError`、`ErrTimerExists`、`ErrTimerNotExist`。
- `parking_lot::Mutex`：存储和通知器状态同步；`crossbeam-channel`：内部缓冲、外部交付和 shutdown；`uuid`：创建时生成 ID。

RustCodeGraph 对 `Create`/`Update`/`Delete` 的被调逻辑与源码相符：它们分别下沉到校验/时间处理、`TimerUpdate::apply`、双索引操作和 `Notify`。精确 `callers`/`callees` 命令在本次检查时受共享索引上的长时间查询阻塞，没有返回额外结果；因此调用点由 RustCodeGraph `explore` 的 blast-radius 结果和针对性 `rg` 交叉确认，未把未返回的图边写成结论。

## 错误处理与边界

- `Create(None)`、`Update(..., None)` 返回带明确文本的 `TimerError`；创建时预填 ID、非零 Version 或 CreateTime 也立即失败。
- `TimerRecord::Validate` 负责 Namespace、Key、调度表达式、时区等业务校验。创建在落库前校验；更新在补丁应用和时间归一化后校验，因此失败不会留下部分修改。
- `(Namespace, Key)` 或生成 ID 冲突返回 `ErrTimerExists`；更新未知 ID 返回 `ErrTimerNotExist`；删除未知 ID不是错误，而是 `Ok(false)`。
- `TimerUpdate::apply` 的 `CheckVersion`、`CheckEventID` 分别返回 `ErrVersionNotMatch`、`ErrEventIDNotMatch`，形成条件更新护栏。
- `getMemStoreTimeZoneLoc` 对无法解析的字符串回退系统时区，但正常 Create 会先调用 `Validate`，所以非法非空时区通常已被拒绝；空时区使用系统位置。系统时区解析被视为进程不变量，失败会 `expect` panic。
- CRUD 的 `Context` 参数本身未参与取消判断；取消语义只用于 Watch。List 是全量扫描，数据量增长后时间和克隆成本均为 O(n)。
- 通知发生在数据锁释放后，因此订阅者看到事件时相应写入已可见；但通知不是事务日志，没有持久化、重放、快照或“先订阅再补历史”的保证。

## 并发与资源生命周期

所有存储读写共用一把 `Mutex<MemoryStoreData>`：双索引更新具有进程内原子性，但 List 也使用独占锁而非读锁。通知在该锁外执行，避免慢 watcher 延长 CRUD 临界区。

Watch 有两级背压：容量 8 的内部通道吸收短暂突发，零容量外部通道要求消费者实际接收。内部通道满时，`Notify` 不在状态锁内阻塞，而是创建补发线程；这保持通知入口可前进，但持续慢消费者会增加线程和 `workers` 向量，属于需要监控的资源风险。转发和补发线程每 10ms 检查一次取消/shutdown，取消不是即时唤醒式的。

`Close` 是幂等的资源终点：首次调用清空 watcher、关闭 shutdown 信号并等待所有登记线程；之后调用直接返回。关闭后的 `Watch` 返回一个发送端已释放的 receiver，关闭后的 `Notify` 是 no-op。正常使用者应显式调用 `TimerStore::Close` 或通知器 `Close`；本类型没有 `Drop` 自动 join 逻辑。

## 与 Go 版本的对应关系

Rust 逐项保留了 [`mem_store.go`](mem_store.go) 的主要契约：双索引、创建输入限制、UUID、初始版本/状态、条件 List、补丁更新与版本递增、删除布尔值、三类 Watch 事件、每订阅者容量 8 的内部缓冲、慢订阅异步补发、取消清理、Close 等待，以及时区/时间字段转换。

实现层差异主要来自语言运行时：Go 使用 `sync.RWMutex`、goroutine、context channel 和 `WaitGroup`；Rust 使用 `parking_lot::Mutex`、OS thread、轮询式 `Context`、crossbeam channel 和 `JoinHandle`。Go 的记录以指针保存并在边界 Clone，Rust 直接保存拥有所有权的值并显式 clone。Go UUID 经 `hex.EncodeToString`，Rust 使用 `Uuid::simple()`，二者都产生 32 位小写十六进制字符串。

Rust 的关闭信号通过丢弃零容量 channel 的唯一 sender 来广播断开；Go 通过取消 notifier context。Rust 还显式用 `closed` 阻止关闭后的注册。两版都不承诺 List 顺序，也都把 Notify 放在成功变更之后。对应 Go 集成测试在 [`../store_intergartion_test.go`](../store_intergartion_test.go)，Rust 可执行对照测试集中在 [`../store_intergartion_test.rs`](../store_intergartion_test.rs)。

## 扩展指南

- 新增查询能力时，优先扩展 [`Cond`](store.rs)/`TimerCond` 并让 `List` 继续只负责遍历与调用 `Match`；若数据规模要求索引化，必须同时定义新索引与 Create/Update/Delete 的一致性规则。
- 新增可更新字段时，在 [`TimerUpdate::apply`](store.rs) 接线，并确认字段是否会改变 ID、Namespace 或 Key。若允许改变业务键，必须在 `MemoryStoreCore::Update` 中原子迁移 `namespaces` 条目并处理唯一性冲突，不能只覆盖旧键。
- 新增事件类型或批量事件时，同步 `WatchTimerEventType`/`WatchTimerResponse`、三个写方法和通知器测试；明确事件顺序、丢失、背压和关闭语义。
- 调整并发模型时，保持“数据变更完成后再通知”“不持有数据锁等待消费者”“Close 可终止并回收所有 worker”三项约束。若把每次满缓冲创建线程改为固定 worker 或异步 runtime，应增加慢消费者和高频通知压力测试。
- 测试不要嵌入本文件。存储契约应扩展 [`../store_intergartion_test.rs`](../store_intergartion_test.rs)；客户端行为放在 `client_test.rs` 或 `client_1_aster_unit_test.rs`；表存储复用通知器的变化还需同步 `../tablestore/sql_test.rs`。同时核对 Go 的 `mem_store.go` 与 `store_intergartion_test.go`，避免移植语义漂移。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 7,032 个 Rust 文件；`files --filter pkg/timer/api` 找到本文件、Go 对照及相邻独立测试。
- RustCodeGraph `node --file pkg/timer/api/mem_store.rs`：核对 378 行完整源码和 22 个符号；`explore` 核对 `NewMemoryTimerStore`、`NewMemTimerWatchEventNotifier`、CRUD/Watch/Notify 的调用范围。精确 `callers`/`callees` 查询已尝试，但因共享图查询长时间占用而未返回，调用点另用定向文本检索复核。
- 源码：[`mem_store.rs`](mem_store.rs)、[`store.rs`](store.rs)、[`timer.rs`](timer.rs)、[`error.rs`](error.rs)、[`lib.rs`](lib.rs)。
- crate/config：[`Cargo.toml`](Cargo.toml)，确认 crate 名、根文件和 `chrono`、`chrono-tz`、`crossbeam-channel`、`parking_lot`、`uuid` 等依赖及 Go 包映射。
- Go 对照：[`mem_store.go`](mem_store.go)；相关 Go 集成测试为 [`../store_intergartion_test.go`](../store_intergartion_test.go)。
- Rust 独立测试：[`../store_intergartion_test.rs`](../store_intergartion_test.rs) 的 `test_mem_timer_store_crud_update_delete_and_validation`、`test_mem_timer_store_list_conditions_match_go`、`test_mem_timer_store_watch_and_context_cleanup`、`test_mem_notifier_broadcast_order_close_and_cancel`、`test_mem_timer_store_timezone_and_time_fields`，分别验证 CRUD/错误与乐观并发、条件组合、事件、广播/取消/关闭及时区字段。
- 本任务是只读行为分析加 Markdown 文档，不运行 Cargo；最终以固定 11 章节结构命令、链接检查和人工事实复核验收。
