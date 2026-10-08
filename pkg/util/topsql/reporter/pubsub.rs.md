# `pkg/util/topsql/reporter/pubsub.rs`

## 文件定位

本文件是 `astersql-util-topsql-reporter` crate 的订阅发送端实现：把 reporter 产生的 `ReportData` 变成面向单个订阅者的 `PubSubResponse` 流。crate 根 `pkg/util/topsql/reporter/lib.rs` 以 `pub mod pubsub` 装载本文件，并再导出其公开项；上层 `pkg/util/topsql/topsql.rs` 在 `TopSQLReporter for RemoteTopSQLReporter` 的 `RegisterPubSubServer` 中调用 `NewTopSQLPubSubService`，再把服务交给抽象的 `PubSubServer` 注册。因此它位于“远程 reporter → DataSink 注册器 → 订阅流”的边界，不负责生成 TopSQL/TopRU 数据，也不直接实现具体 gRPC server。

crate 边界由 `pkg/util/topsql/reporter/Cargo.toml` 确认：本文件直接使用 `crossbeam-channel`，使用 `tipb` protobuf 类型，并通过路径依赖 `topsql_state` 读取全局 TopRU 开关。该 crate 没有为本模块设置条件编译；测试由 `lib.rs` 中独立的 `mod pubsub_test` 挂载，未把测试写入生产源文件。

## 核心职责

1. `parse_top_sql_subscription` 与 `parse_top_ru_subscription` 把 `tipb::TopSqlSubRequest` 解释为 TopSQL/TopRU 开关和 TopRU 间隔。空请求或空 collector 列表默认启用 TopSQL，以保留旧客户端兼容行为；请求 TopRU 却缺少 `TopRuConfig` 时拒绝订阅。
2. `PubSubDataSink` 实现 `DataSink`：reporter 线程通过容量为 1 的有界通道非阻塞投递整批 `ReportData`，订阅运行循环取出后按固定顺序逐项写入 `PubSubStream`。
3. `TopSqlPubSubService` 管理单次订阅的注册、运行、注销和取消，确保发送路径正常返回或 panic 后都执行清理。
4. `PUBSUB_METRICS` 记录通道满丢弃、四类响应发送成功数和批次发送失败数。这是本地原子计数结构，与 Go 文件使用的 reporter Prometheus 指标并非同一实现。

本文件不校验 TopRU 间隔是否合法，也不维护全局 TopSQL/TopRU 引用计数；这些职责在 `pkg/util/topsql/reporter/datasink.rs` 的 `DefaultDataSinkRegisterer::register`/`deregister` 中完成。

## 主要符号

- `parse_top_sql_subscription(Option<&TopSqlSubRequest>) -> bool`：`None`、空 collectors、包含 `CollectorTypeTopsql` 或 `CollectorTypeUnspecified` 时返回 `true`；仅 TopRU 等其他 collector 时返回 `false`。
- `parse_top_ru_subscription(...) -> Result<(bool, i32), DataSinkError>`：只有 collectors 包含 `CollectorTypeTopru` 才启用；启用时要求 `has_topru()`，并把 protobuf `item_interval_seconds` 转为 `i32` 原样返回。未知值不在这里归一化或拒绝。
- `PubSubResponse`：发送边界的四种载荷枚举，分别封装 `TopSqlRecord`、`TopRuRecord`、`SqlMeta` 和 `PlanMeta`。
- `PubSubStream`：要求 `Send + Sync + 'static` 的流抽象；唯一方法 `send` 把传输层失败转换为 `DataSinkError`。具体协议适配由外层集成提供。
- `PubSubMetrics` / `PUBSUB_METRICS`：进程级无锁原子计数器；成功数在每条流发送成功后递增，`send_failures` 在异步任务超时或发送失败时按任务递增。
- `SendTask`：内部队列元素，持有共享的 `Arc<ReportData>` 和绝对 `Instant` 截止时间。
- `PubSubDataSink`：保存流、数据/取消通道两端、取消原子位、两类订阅开关和 TopRU 间隔。`new`、`from_request`、`run`、`run_one`、`do_send`、`cancel` 是主要生命周期入口。
- `TopSqlPubSubService`：持有 `Arc<dyn DataSinkRegisterer>`；`subscribe` 创建并注册 sink，运行至结束，然后注销并取消。
- `NewTopSQLPubSubService`：保留 Go 风格命名的公开构造函数，供 `pkg/util/topsql/topsql.rs` 的包级接线使用。
- `impl DataSink for PubSubDataSink`：`try_send` 提供非阻塞背压，`on_reporter_closing` 转为取消，`subscription_config` 把本订阅配置暴露给注册器。

## 执行流程

订阅建立流程如下：

1. 上层通过 `NewTopSQLPubSubService` 构造服务；具体入口见 `pkg/util/topsql/topsql.rs::RegisterPubSubServer`。
2. `TopSqlPubSubService::subscribe` 调用 `PubSubDataSink::from_request`。后者分别解析 TopSQL 与 TopRU 配置，再调用 `new` 创建容量为 1 的数据通道和容量为 1 的取消通道。
3. 服务把同一个 sink 以 `Arc<dyn DataSink>` 注册到 `DataSinkRegisterer`。默认注册器据 `subscription_config` 调整全局采集开关和 TopRU 间隔，并把 sink 放入 reporter 可分发集合。
4. reporter 通过 `DataSink::try_send` 投递 `Arc<ReportData>` 与 deadline。队列空时立即成功；容量已占用时立即返回 `ChannelFull` 并增加 `ignored_channel_full`，不会阻塞 reporter。
5. `run` 用 `crossbeam_channel::select!` 等待取消或任务。任务到达后，`execute_task` 先检查是否已逾时，再调用 `do_send_until`。
6. `do_send_until` 严格按 TopSQL records → TopRU records → SQL metas → plan metas 的顺序发送。TopSQL records 受订阅开关控制；TopRU records 同时要求订阅启用和 `topsql_state::TopRUEnabled()`；两类 metadata 无条件发送。
7. `send` 在调用 `stream.send` 前后都检查取消和 deadline。后置检查保证最后一条底层发送虽然返回成功、但期间发生取消或超时时，整批仍返回正确错误。
8. `run` 遇到取消、通道断开、deadline 或流错误后结束。`subscribe` 随后调用 `deregister` 和 `cancel`；若运行闭包 panic，也先完成相同清理，再按与 Go `recover` 对齐的契约返回 `Ok(())`。

`run_one` 是同一接收/发送逻辑的非阻塞单步入口：最多处理一个任务，空队列返回 `Ok(false)`，主要便于确定性测试或外部驱动；生产订阅主路径使用 `run`。

## 数据与状态

`ReportData` 定义在 `datasink.rs`，四个 `Vec` 分别存放 TopSQL、TopRU、SQL 元数据和 plan 元数据。本文件不修改批次内容，而是在发送时克隆单条 protobuf 消息，因而共享批次所有权可安全留在 `Arc` 中，但发送成本包含每条消息克隆。

每个 `PubSubDataSink` 的配置在构造后不可变：`enable_top_sql`、`enable_top_ru`、`item_interval` 只被读取。`item_interval` 通过 `subscription_config` 交给注册器，用于全局 TopRU 状态；它不参与本文件的发送节拍。队列容量固定为 1，这是显式背压不变量：一个待处理批次已排队时，新批次会被丢弃并返回错误。

`cancelled: AtomicBool` 是快速可见的终止状态，使用 `SeqCst` 保证跨线程一致顺序；取消通知通道用于唤醒阻塞在 `run` 的接收循环。指标原子使用 `Relaxed`，因为它们只累计统计值，不承载生命周期同步。

## 依赖与调用关系

上游链路由源码接线和精确搜索确认：

- `pkg/util/topsql/topsql.rs::RegisterPubSubServer` → `NewTopSQLPubSubService` → 外部 `PubSubServer::RegisterTopSQLPubSubService`。
- `pkg/util/topsql/reporter/reporter.rs` 通过 `DataSinkRegisterer` 持有并向已注册 sink 分发 `ReportData`；本文件以 `DataSink` trait 接入该路径。
- `TopSqlPubSubService::subscribe` → `PubSubDataSink::from_request` → `DataSinkRegisterer::register` → `PubSubDataSink::run` → `execute_task` → `do_send_until` → `PubSubStream::send`，结束后调用 `DataSinkRegisterer::deregister` 和 `cancel`。

直接下游包括：

- `crate::datasink::{DataSink, DataSinkError, DataSinkRegisterer, ReportData, SubscriptionConfig}`：定义批次、错误和注册协议。
- `crate::tipb_protobuf`：订阅请求及四类响应载荷。
- `crate::topsql_state::TopRUEnabled`：发送 TopRU 前的第二层全局门控。
- `crossbeam_channel::{bounded, select, TryRecvError, TrySendError}`：同步有界队列、取消通知与阻塞选择。

RustCodeGraph `files --filter pkg/util/topsql/reporter` 确认目标、Go 对照与测试均已索引；`node --file ...pubsub.rs` 给出了完整实现及文件级使用关系。由于常见的 `subscribe`/`send` 名称使图查询混入大量无关 pubsub 符号，具体上游调用点以 `rg` 对目标类型和构造函数的精确命中复核，没有把歧义结果当作调用事实。

## 错误处理与边界

- 请求 TopRU 但没有配置：`parse_top_ru_subscription` 返回 `TopRuConfigEmpty`，尚未注册 sink，也不会改变全局开关。
- TopRU interval 未知：解析层保留原始整数；默认注册器随后返回 `InvalidTopRuInterval`。这是职责分离，不是漏校验。
- 队列已满：`try_send` 增加 `ignored_channel_full` 并返回 `ChannelFull`；调用方可记录丢弃，但本文件不重试。
- 已取消或本文件的数据通道断开：返回 `Closed` 表示订阅不可继续；registerer 自身关闭则由 `datasink.rs` 返回独立的 `RegistererClosed`。
- deadline：取任务时已经到期，或任一单条发送前后到期，均返回 `DeadlineExceeded`；由 `execute_task` 增加一次 `send_failures`。
- 流发送失败：错误由 `PubSubStream::send` 原样传播，当前批次立即停止，后续记录和 metadata 不再发送；异步任务路径增加 `send_failures`。
- 注册失败：`subscribe` 直接返回注册错误，不进入 `run`，也不会调用本服务的注销分支，因为注册并未成功。
- panic：仅 `subscribe` 包裹的 `sink.run()` 被 `catch_unwind` 捕获；清理后返回成功以对齐 Go 的 `recover` 行为。直接调用 `do_send`/`run_one` 并没有该保护。

边界上，`PubSubStream::send` 是同步调用；deadline 检查不能中断一个永久阻塞的具体 stream 实现，只能在它返回后识别已经超时。Rust 实现因此不像 Go 对照的 `run` 那样为一次批次另启 goroutine 并由 context 提前结束等待，这是扩展传输层时必须注意的资源风险。

## 并发与资源生命周期

`PubSubDataSink` 可跨线程共享：流、通道和注册器均通过 `Arc`/线程安全 trait 边界持有。生产者只执行 `try_send`，消费者由单个 `run` 循环顺序发送，因此一个 sink 内不会并行写同一流，且四类载荷顺序稳定。实现没有显式阻止多个线程同时调用 `run` 或 `run_one`；安全扩展应保持“每个 sink 单消费者”这一实际使用约束，否则不同批次可能被不同消费者交错处理。

`cancel` 通过 `AtomicBool::swap` 实现幂等：只有首次取消尝试向取消通道投递通知，通道满时忽略也安全，因为取消位已经可见。`run` 收到通知后返回 `Closed`；`send` 的前后检查使批次能在记录边界观察取消。正在执行的同步 `stream.send` 无法被本文件强制抢占。

注册成功后，`TopSqlPubSubService::subscribe` 拥有完整清理责任：无论 `run` 返回错误还是 panic，都先注销同一个 trait object，再取消 sink。默认注册器的注销进一步维护全局 TopSQL 引用计数与 TopRU 启用状态。`Arc<ReportData>` 让排队任务在 reporter 原始引用释放后仍有效；任务处理或通道销毁后自动释放。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/util/topsql/reporter/pubsub.go`，测试是同目录 `pubsub_test.go`。主要一致点如下：

- Rust `TopSqlPubSubService`、`PubSubDataSink`、解析函数和容量 1 的任务通道分别对应 Go `TopSQLPubSubService`、`pubSubDataSink`、`parseTopSQLSubscription`/`parseTopRUSubscription` 和 `sendTaskCh`。
- 两版都把空 collector 视为 TopSQL 订阅，只有显式 TopRU collector 才启用 TopRU，并在缺少 TopRU 配置时报错。
- 两版都按 TopSQL → TopRU → SQL meta → plan meta 顺序发送，且 TopRU 还受进程级开关门控；单条失败立即中止后续发送。
- 两版都在订阅结束时注销和取消，并将发送路径 panic 恢复为订阅成功返回。

需要明确的实现差异：Go sink 自带 context/cancel 和 registerer，`run` 为每批发送启动恢复保护的 goroutine，并可由 deadline context 先结束等待；Rust 将注册/注销放在服务层，使用原子位加 crossbeam 取消通道，并同步调用 stream。Go 记录细分的 Prometheus histogram/counter，Rust 当前仅维护 `PUBSUB_METRICS` 原子计数。Rust `PubSubResponse` 是本地枚举，而 Go 直接组装 `TopSQLSubResponse` oneof；具体编码/传输由 Rust 的 `PubSubStream` 适配层承担。

## 扩展指南

- 新增响应载荷时，应同时扩展 `PubSubResponse`、`do_send_until` 的确定发送位置、具体 `PubSubStream` 适配器以及成功/失败指标；同步更新 `pkg/util/topsql/reporter/pubsub_test.rs` 的 mock 分类和顺序断言，并核对 Go `doSend` 及对应 send 方法，避免两版顺序漂移。
- 修改订阅字段或默认值时，优先改 `parse_top_sql_subscription`/`parse_top_ru_subscription`，并同步 Rust/Go 的 nil、空 collectors、未知枚举、缺配置测试。保持旧客户端“空 collectors 启用 TopSQL”的兼容约束，除非协议明确变更。
- 修改背压策略时，重点评估 reporter 延迟、内存上限和丢弃可观测性。容量 1 与非阻塞 `try_send` 是当前 Go 对齐语义；不要仅为了减少 `ChannelFull` 而引入无界队列。
- 引入可取消的异步传输时，应在 `PubSubStream` 边界设计 deadline/cancellation，而不是只增加发送前检查；同时验证阻塞 send、最后一条发送跨 deadline、取消中途截断和 panic 清理。
- TopRU interval 校验和全局引用计数属于 `datasink.rs::DefaultDataSinkRegisterer`。若变更 interval 策略，必须连同该文件及其独立测试评估，不能在 pubsub 解析层重复维护另一套状态。
- 测试继续放在独立的 `pkg/util/topsql/reporter/pubsub_test.rs`；跨 crate 包级接线可在 `pkg/util/topsql/topsql_test.rs` 覆盖，不应把测试模块嵌回生产文件。

兼容风险集中在订阅默认值、消息顺序和 panic 返回契约；性能风险集中在 protobuf 逐条克隆、同步 stream 阻塞和队列容量；正确性风险集中在 register/deregister 配对、TopRU 双重门控以及取消/deadline 的发送后检查。

## 验证依据

- RustCodeGraph：运行 `status`，索引包含 11,467 个文件；运行 `files --filter pkg/util/topsql/reporter`，确认目标 Rust、Go 对照和独立测试均在索引中；运行 `node --file pkg/util/topsql/reporter/pubsub.rs --offset 1 --limit 260` 及 `--offset 240 --limit 140`，读取完整 340 行实现；并查询 `PubSubDataSink`、`TopSqlPubSubService`、`parse_top_ru_subscription`。歧义的通用方法调用结果未用作结论。
- 生产源码：`pkg/util/topsql/reporter/pubsub.rs`（目标实现）、`pkg/util/topsql/reporter/datasink.rs`（trait、错误、注册器与全局开关生命周期）、`pkg/util/topsql/reporter/lib.rs`（模块装载/再导出）、`pkg/util/topsql/topsql.rs`（`RegisterPubSubServer` 上游接线）。
- crate 配置：`pkg/util/topsql/reporter/Cargo.toml`（crate 名、`crossbeam-channel`、`tipb`、`topsql_state` 依赖及无本模块条件 feature），以及 `pkg/util/topsql/Cargo.toml`（上层对 reporter crate 的路径依赖）。
- Go 对照：`pkg/util/topsql/reporter/pubsub.go`；入口对照由 `pkg/util/topsql/topsql.go` 的 `NewTopSQLPubSubService` 调用精确搜索确认。
- 独立测试：`pkg/util/topsql/reporter/pubsub_test.rs` 覆盖解析、顺序、TopSQL/TopRU 门控、队列满、超时、取消、流错误、注册失败、非法 interval 和 panic 注销；`pkg/util/topsql/reporter/pubsub_test.go` 提供 Go 语义对照；`pkg/util/topsql/topsql_test.rs` 覆盖包级 sink/服务集成。
- 本任务是纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文件存在且恰好具有 11 个固定二级章节，并人工复核以上关键陈述均指向实际符号或文件。
