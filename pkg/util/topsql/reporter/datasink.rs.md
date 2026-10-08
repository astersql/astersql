# `pkg/util/topsql/reporter/datasink.rs`

## 文件定位

本文件位于 `astersql-util-topsql-reporter` crate 的数据输出边界，定义 reporter 与具体输出实现之间的共享载荷、接口、注册器和生命周期规则。crate 根 `pkg/util/topsql/reporter/lib.rs` 以 `pub mod datasink` 装入本文件并 `pub use datasink::*`，因此这里的公开类型也是 crate 级 API。`RemoteTopSQLReporter` 在 `reporter.rs::NewRemoteTopSQLReporter` 中持有一个 `DefaultDataSinkRegisterer`，把整理完成的 TopSQL/TopRU 数据广播给已注册的数据汇；具体数据汇由 `pubsub.rs::PubSubDataSink` 和 `single_target.rs::SingleTargetDataSink` 实现。

该文件不负责采集、聚合、网络编码或阻塞发送。它负责规定“什么可以被发送”“输出端如何接入”“订阅如何影响全局采集开关”，并提供并发安全的 sink 集合。直接依赖边界可由 `pkg/util/topsql/reporter/Cargo.toml` 核对：tipb protobuf 载荷来自 `tipb`，错误派生来自 `thiserror`，TopSQL/TopRU 开关由同 workspace 的 `astersql-util-topsql-state`（代码中 `crate::topsql_state`）管理。

## 核心职责

1. `ReportData` 统一携带 TopSQL CPU 记录、TopRU 记录、SQL 元数据和 Plan 元数据，并由 `ReportData::has_data` 给出“任一分量非空”的批次判定。
2. `DataSink` 抽象非阻塞/限时投递和 reporter 关闭通知；`subscription_config` 让注册器无需识别具体实现类型即可决定 TopSQL/TopRU 全局开关。
3. `DataSinkRegisterer` 抽象注册与注销，供 PubSub 服务、SingleTarget 生命周期和 `RemoteTopSQLReporter` 共同使用。
4. `DefaultDataSinkRegisterer` 对同一 sink 做幂等去重、限制最多 10 个 sink，并在注册/注销/关闭时维护采集开关。
5. `DataSinkError` 汇总注册边界以及具体输出实现会返回的背压、关闭、超时、配置和流发送错误，使同一 trait 链使用统一错误类型。

## 主要符号

- `MAX_DATA_SINKS: usize = 10`：单个注册器的硬上限。达到上限后，新且不同的 sink 注册返回 `DataSinkError::TooManyDataSinks`；重复注册会先命中幂等分支，不占新槽位。
- `DataSinkError`：`RegistererClosed`、`TooManyDataSinks` 在本文件的注册路径产生；`InvalidTopRuInterval(i32)` 包装 `topsql_state::SetTopRUItemInterval` 的非法间隔；`ChannelFull`、`Closed`、`DeadlineExceeded`、`Stream(String)` 主要供 `pubsub.rs`、`single_target.rs` 的发送路径使用。`TopRuConfigEmpty` 是共享枚举的一部分，本文件不直接构造它。
- `ReportData`：四个公开 `Vec<tipb::...>` 字段分别是 `data_records`、`ru_records`、`sql_metas`、`plan_metas`。类型实现 `Clone + Debug + Default`；`has_data(&self) -> bool` 只检查是否至少一个向量非空，不校验记录内容。
- `SubscriptionConfig`：按值复制的订阅快照，包含 `enable_top_sql`、`enable_top_ru` 和秒单位 `item_interval`。
- `DataSink`：要求实现者满足 `Send + Sync + 'static`。`try_send(Arc<ReportData>, Instant)` 允许多个 sink 共享同一不可变批次；`on_reporter_closing` 接收关闭通知；默认 `subscription_config() -> None` 表示 SingleTarget 语义，即启用 TopSQL、不计入 TopRU。
- `DataSinkRegisterer`：同样要求 `Send + Sync + 'static`；`register` 可失败，`deregister` 幂等且无返回值。
- `RegistererState`：锁内状态，`data_sinks` 以指针键保存 trait object，`top_sql_sink_count` 仅统计要求 TopSQL 的 sink。
- `DefaultDataSinkRegisterer`：`closed: AtomicBool` 与 `state: Mutex<RegistererState>` 组合实现关闭门闩和集合串行化；`new/default` 创建空实例，`sink_count` 与 `sinks` 提供数量和 `Arc` 快照，`close` 完成全量清理，`Drop::drop` 兜底调用 `close`。
- `data_sink_key`：把 `Arc<dyn DataSink>` 的数据指针擦除为 `usize`，用于按同一 Arc 分配对象身份去重，而不是按业务字段相等性去重。

## 执行流程

正常主链如下：

1. `reporter.rs::NewRemoteTopSQLReporter` 构造 `DefaultDataSinkRegisterer::new()`；`RemoteTopSQLReporter::Register/Deregister` 和它对 `DataSinkRegisterer` 的实现都转发到该注册器。
2. PubSub 的 `TopSqlPubSubService::subscribe` 构造 `PubSubDataSink`、注册、运行发送循环，并在循环结束后注销；SingleTarget 的 `Start/trySwitchRegistration/Close` 随接收地址有无注册或注销自身。
3. `DefaultDataSinkRegisterer::register` 先在锁外读一次 `closed`，随后加锁并再次读取，阻止与 `close` 交错时插入新 sink。它再按 `data_sink_key` 检查重复、检查 10 个 sink 上限、读取订阅配置。
4. 对启用 TopRU 的 sink，注册器先调用 `SetTopRUItemInterval`；只有间隔合法时才 `EnableTopRU` 并继续插入。后注册订阅的合法间隔覆盖先前值。对无配置或显式启用 TopSQL 的 sink，插入后调用 `EnableTopSQL` 并增加 `top_sql_sink_count`。
5. `RemoteTopSQLReporter::reportWorker` 取得报告后调用 `doReport`；空批次被 `hasData` 过滤，非空批次转换成 sink 载荷后进入 `trySend`。`trySend` 调用 `sinks()` 获取快照，逐个执行 `DataSink::try_send(data.clone(), deadline)`。单个 sink 失败只记录 warning，不中断其他 sink，方法最终返回 `Ok(())`。
6. `deregister` 在未关闭时加锁，按指针键移除；不存在则直接返回。要求 TopSQL 的 sink 会使本地计数饱和减一，计数归零才 `DisableTopSQL`。要求 TopRU 的 sink 每次移除都调用一次 `DisableTopRU`，实际引用计数由 `pkg/util/topsql/state/state.rs` 维护。
7. `RemoteTopSQLReporter::Close` 停止并 join 自身 worker 后调用注册器 `close`。注册器用原子 `swap` 保证只执行一次，在锁内 drain 全部 sink、清理 TopSQL 计数并为每个 TopRU sink释放一次全局引用；随后在锁外逐个调用 `on_reporter_closing`。

## 数据与状态

`ReportData` 是本文件唯一的业务载荷。四个向量相互独立：因此只有元数据、只有 TopRU 或只有 TopSQL 的批次都属于有效数据。`Arc<ReportData>` 使广播无需为每个 sink 深拷贝 protobuf 向量；各 sink 只获得共享只读引用。

注册器有两层状态：`closed` 是一次性生命周期门闩，`RegistererState` 是受互斥锁保护的可变集合。`HashMap<usize, Arc<dyn DataSink>>` 同时拥有 sink 生命周期并支持按对象身份 O(1) 查找；`sinks()` 克隆 Arc 后立即释放锁，所以实际发送期间注册/注销不持有注册器锁。快照语义意味着已经进入某次广播的 sink 即使紧接着被注销，仍可能收到该次 `try_send`。

`top_sql_sink_count` 是注册器本地引用计数，因为 `topsql_state::EnableTopSQL/DisableTopSQL` 本身只是布尔开关。TopRU 不在本文件重复保存计数：`topsql_state::EnableTopRU` 原子加一，`DisableTopRU` 以 CAS 防下溢并在最后一个消费者离开时把 interval 重置为默认值。多个 TopRU sink 的 interval 采用“后写覆盖”，注销非最后一个 sink 不恢复它之前的 interval；这由 `datasink_test.rs::test_default_data_sink_registerer_top_ru_two_sinks_ref_count_and_reset` 明确验证。

## 依赖与调用关系

上游调用者与入口：

- `reporter.rs::NewRemoteTopSQLReporter` 创建注册器；`RemoteTopSQLReporter::{Register,Deregister}` 暴露注册入口，`RemoteTopSQLReporter::trySend` 读取 sink 快照，`RemoteTopSQLReporter::Close` 关闭注册器。
- `pubsub.rs::TopSqlPubSubService::subscribe` 使用 `DataSinkRegisterer::{register,deregister}` 包围订阅运行期。`PubSubDataSink` 的 `subscription_config` 返回显式三字段配置。
- `single_target.rs::SingleTargetDataSink` 根据动态接收地址注册/注销。其 `DataSink` 实现不覆盖 `subscription_config`，因此使用默认 SingleTarget 语义。

下游调用与实现：

- 注册路径调用 `topsql_state::{SetTopRUItemInterval,EnableTopRU,EnableTopSQL}`；注销/关闭调用 `DisableTopSQL/DisableTopRU`。这些函数位于依赖 crate `astersql-util-topsql-state`。
- `DataSink::try_send` 的两个生产实现都使用容量为 1 的有界通道：PubSub 满时累计 `PUBSUB_METRICS.ignored_channel_full` 并返回 `ChannelFull`；SingleTarget 把自身错误映射成同一 `DataSinkError`。
- tipb 的 `TopSqlRecord`、`TopRuRecord`、`SqlMeta`、`PlanMeta` 由 `Cargo.toml` 中固定 revision 的 `tipb` Git 依赖提供，并由 `lib.rs` 以 `tipb_protobuf` 别名导出。

RustCodeGraph 的文件索引显示本文件有 23 个符号；其探索结果给出 `on_reporter_closing <- close`、`subscription_config <- register/deregister/close`、`DefaultDataSinkRegisterer::new <- reporter.rs::NewRemoteTopSQLReporter` 等直接关系。对通用名 `close/register` 的全仓搜索会混入大量同名符号，因此本文的主链同时由上述精确源码节点复核。

## 错误处理与边界

- 注册器已关闭时返回 `RegistererClosed`。双重检查保证“第一次检查后、加锁前发生 close”的线程也不会插入。
- 不同 sink 超过 10 个时返回 `TooManyDataSinks`；相同 Arc 重复注册为 `Ok(())`，不会重复启用开关或增加计数。
- TopRU interval 非法时映射为 `InvalidTopRuInterval(original_value)`；错误发生在 `EnableTopRU` 和插入 map 之前，因此该次注册不留下 sink 或开关引用。允许值由 state crate 定义为 0（归一为默认 60）、15、30、60。
- 注销未知 sink、注册器关闭后的注销，以及重复 `close` 都是无操作。TopSQL 计数使用 `saturating_sub` 防止下溢；TopRU state 也以 CAS 防止全局计数下溢。
- 锁中毒不会直接 panic：本文件统一以 `unwrap_or_else(|error| error.into_inner())` 取回内部状态。这维持可用性，但也意味着先前 panic 可能留下的状态仍会继续被使用。
- `data_sink_key` 依赖对象仍由 map 中的 Arc 强持有，因此注册期间地址不会被回收复用；注销后旧键随条目移除。调用者必须传回指向同一对象的 Arc，重新构造内容相同的 sink 不会命中。
- `trySend` 有意吞掉每个 sink 的错误并继续广播；因此调用方不能从其 `Ok(())` 推断所有目标都已接收，只能结合日志和各 sink 指标判断。

## 并发与资源生命周期

`DefaultDataSinkRegisterer` 可跨线程共享：trait 边界要求 sink/registerer 为 `Send + Sync`，map 和 TopSQL 计数由同一 `Mutex` 串行化，关闭标志用 `SeqCst` 原子访问。`register` 在持锁后重查关闭状态，建立“close 已取得锁并清理后不能新增成员”的约束；`close` 用 `swap(true)` 提前封门，再取得状态锁清空集合。

关闭回调特意移到锁外执行。这样 `DataSink::on_reporter_closing` 内部即使触发自身取消、join 或间接进入注册相关路径，也不会因注册器锁重入而死锁。`sinks()` 同样返回 Arc 快照，不在未知的 sink 代码执行期间持锁。

`Drop` 为未显式关闭的注册器提供兜底清理；显式 `close` 与析构可以安全叠加。`RemoteTopSQLReporter::Close` 先停止生产数据的两个 worker，再关闭 sinks，避免常规关闭顺序中继续生成新广播。具体 sink 还拥有自己的通道和 worker：PubSub 的关闭回调调用 `cancel`，SingleTarget 的关闭回调调用 `cancelWorker`；完整 join 由 SingleTarget 的显式 `Close` 负责，而注册器只发出 reporter 关闭通知。

全局开关带来跨注册器共享状态的约束：TopRU 在 state crate 内是进程级引用计数，能够跨 sink 配对；TopSQL 是进程级布尔值而本文件只维护单个注册器内计数。因此若将来同时创建多个独立 `DefaultDataSinkRegisterer`，不能假定一个实例关闭 TopSQL 不会影响另一个实例，这不是本文件当前模型保证的场景。

## 与 Go 版本的对应关系

对应实现是 `pkg/util/topsql/reporter/datasink.go`，行为主干保持一致：四类 `ReportData` 载荷、`hasData` 判空、最多 10 个 sink、重复注册幂等、SingleTarget 默认启用 TopSQL、PubSub 按订阅启用 TopSQL/TopRU、TopSQL 本地计数归零关闭、TopRU 借助 state 层引用计数，以及后注册 TopRU interval 覆盖先前值。

Rust 为所有权和 trait object 做了几处等价适配：

- Go 用 `map[DataSink]struct{}` 以接口值作键；Rust 用 `Arc<dyn DataSink>` 加擦除后的数据指针键，表达同一实例身份。
- Go 通过对 `*pubSubDataSink` 的具体类型断言获取订阅字段；Rust 把它提升为 `DataSink::subscription_config`，无配置的实现自然采用 SingleTarget 默认值。
- Go 用 `context.Context.Done()` 判定注册器关闭；Rust 用 `AtomicBool`、显式 `close` 和 `Drop` 表达生命周期，并额外负责 drain sink 和调用 `on_reporter_closing`。Go 的 reporter 关闭通知逻辑分布在 reporter 侧，Rust 将这部分收拢进注册器。
- Go 载荷通过指针共享；Rust 用 `Arc<ReportData>` 明确并发共享所有权。Go 的错误是字符串/包装错误，Rust 用可比较的 `DataSinkError` 枚举保留稳定分类。

独立测试 `datasink_test.rs` 与 `datasink_test.go` 一一覆盖基本注册、两个 TopRU sink、重复注册、并发交错、TopSQL/TopRU-only 隔离和 SingleTarget 混合行为。Rust 测试用 `serial_test::serial` 与 `StateGuard` 隔离进程级状态；该测试必须继续保留在独立文件中，不应内嵌回生产源文件。

## 扩展指南

- 新增一种 sink 时实现 `DataSink`，将发送工作留在自身模块；`try_send` 应遵守 deadline/背压约定并避免在 reporter 广播线程长期阻塞。若它具有订阅选择，覆盖 `subscription_config`；若省略该方法，就明确接受“启用 TopSQL、永不启用 TopRU”的 SingleTarget 语义。
- 增加 `ReportData` 载荷类别时，需要同时修改 `ReportData` 字段与 `has_data`，再同步 reporter 的组包/转换、PubSub 和 SingleTarget 的发送顺序及 protobuf 依赖；仅加字段而忘记 `has_data` 会让只含新类别的批次被丢弃。
- 改动注册/注销规则时，以 `register`、`deregister`、`close` 三条路径成对审查开关引用。尤其要保证配置验证失败不产生半注册状态、重复注册不重复计数、关闭对每个已注册 TopRU sink恰好释放一次引用。
- 若提高 `MAX_DATA_SINKS`，应评估 `RemoteTopSQLReporter::trySend` 的串行扇出时延和每个 sink 的背压日志量；当前上限也用于 HashMap 初始容量。
- 若改变 sink 身份规则，必须先定义 clone Arc、不同 trait-object 视图和对象重建的等价性，避免出现无法注销或错误合并。当前 `data_sink_key` 的对象身份语义应由专门回归测试锁定。
- 测试应优先扩展同目录独立文件 `pkg/util/topsql/reporter/datasink_test.rs`，并同步核对 `datasink_test.go` 的原始意图；涉及发送行为时再扩展 `pubsub_test.rs` 或 `single_target_test.rs`。共享全局开关的用例继续使用串行执行和 RAII 复位，避免测试间污染。
- 兼容风险集中在 Go/Rust 注册语义、错误分类及 interval 归一化；性能风险集中在锁内调用 state 函数、HashMap 快照克隆 Arc 和逐 sink 串行 `try_send`。未知 sink 回调不得移入锁内。

## 验证依据

- 源文件：`pkg/util/topsql/reporter/datasink.rs`，RustCodeGraph `node --file ... --offset 1 --limit 260` 完整读取 248 行，并确认常量、错误枚举、载荷、两个 trait、注册器实现、Drop 和指针键函数。
- crate 边界：`pkg/util/topsql/reporter/Cargo.toml` 与 `pkg/util/topsql/reporter/lib.rs`，确认 crate 名、`autotests = false`、`datasink` 的公开再导出、tipb/thiserror/topsql_state 依赖和独立 `mod datasink_test` 接线。
- RustCodeGraph 调用证据：`status` 显示本地索引含 11,467 个文件且 `datasink.rs` 已索引；`files --filter pkg/util/topsql/reporter` 确认 Rust/Go 实现与测试；`query`/`explore`/`node` 核对 `DefaultDataSinkRegisterer`、`ReportData`、`data_sink_key`、`NewRemoteTopSQLReporter`、`PubSubDataSink`、`SingleTargetDataSink` 及调用链。
- 直接 Rust 邻接证据：`reporter.rs::{NewRemoteTopSQLReporter,Register,Deregister,reportWorker,doReport,trySend,Close}`，`pubsub.rs::{TopSqlPubSubService::subscribe,DataSink for PubSubDataSink}`，`single_target.rs::{Start,trySwitchRegistration,Close,DataSink for SingleTargetDataSink}`，`state/state.rs::{EnableTopRU,DisableTopRU,SetTopRUItemInterval}`。
- Go 对照：`pkg/util/topsql/reporter/datasink.go`；独立测试：`pkg/util/topsql/reporter/datasink_test.rs` 与 `pkg/util/topsql/reporter/datasink_test.go`。测试证据确认基本生命周期、幂等、上限相关状态模型、TopRU 引用计数/interval、并发交错及混合订阅语义。本任务按计划只做文档分析，未运行 Cargo 或代码测试。
- 人工复核结论：本文区分了本文件承担的注册/状态职责与具体 sink 的发送职责，列出了真实符号、上下游、锁和资源生命周期、Go 差异及安全扩展点；未把未执行的代码测试描述为已验证。
