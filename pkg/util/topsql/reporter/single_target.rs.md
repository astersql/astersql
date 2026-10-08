# `pkg/util/topsql/reporter/single_target.rs`

## 文件定位

本文件实现 TopSQL Reporter 的“单一接收端”数据汇：把 Reporter 生成的一批 TopSQL 记录、SQL 元数据和执行计划元数据，通过 tonic gRPC 发给当前配置的唯一接收端。模块由 [`lib.rs`](./lib.rs) 以 `pub mod single_target` 挂载并整体再导出；crate 名为 `astersql-util-topsql-reporter`，其边界、构建脚本和依赖见 [`Cargo.toml`](./Cargo.toml)。

生产主链在 `pkg/util/topsql/topsql.rs`：`default_pipeline` 用 `NewSingleTargetDataSink(reporter.clone())` 创建实例，`SetupTopProfiling` 在启动 `RemoteTopSQLReporter` 后调用 sink 的 `Start`。Reporter 的 `trySend` 再通过 `DataSink::try_send` 将同一个 `Arc<ReportData>` 分发给所有已注册 sink。因此，本文件位于“周期聚合结果”与“外部 TopSQL Agent gRPC 服务”之间，而不是采样、聚合或生成摘要的入口。

## 核心职责

- 管理 sink 是否已向 `DataSinkRegisterer` 注册，并每隔 `pollInterval` 检查接收地址是否从空变为非空或反向变化（`Start`、`run`、`trySwitchRegistration`）。
- 提供容量为 1 的非阻塞发送队列；忙时立即返回 `SingleTargetError::ChannelFull`，关闭后返回 `SingleTargetError::Closed`，不让 Reporter 阻塞（`NewSingleTargetDataSinkWithReceiver`、`try_send_inner`）。
- 在独立后台线程内持有单线程 Tokio runtime 和 gRPC 连接状态；同一地址复用连接，地址变化时丢弃旧 client 并重新连接（`recoverRun`、`connectionState`、`tryEstablishConnection`）。
- 将一批数据转换到 tonic 使用的 prost 类型，并行发送 SQL 元数据、Plan 元数据和 TopSQL 记录；将分支 panic 转为普通错误并记录整批及各子流指标（`doSend`、`recoverSendPanic`、三个 `sendBatch*` 方法及 `observe*`）。
- 明确保持当前兼容边界：`sendBatchTopRURecord` 是成功返回的空操作，SingleTarget 当前不发送 TopRU；`sendTopRURecords` 只是保留并可独立测试的流兼容辅助函数，并未接入 `doSend` 的实际 TopRU 发送路径。

## 主要符号

- `SingleTargetDataSink`：公开主体。持有注册器、动态地址提供者、发送/取消通道、三个原子状态、轮询周期和 worker 句柄。类型实现 `DataSink`，因此可注册到 `RemoteTopSQLReporter`。
- `NewSingleTargetDataSink`：生产构造入口，使用进程级 `globalReceiverAddress`；`SetGlobalReceiverAddress` 更新这个默认地址。`NewSingleTargetDataSinkWithReceiver` 是可注入地址提供者的构造入口，主要用于测试和其它显式集成。
- `ReceiverAddressProvider` / `MutableReceiverAddress`：把地址读取与 sink 解耦。后者以 `RwLock<String>` 提供读写；空字符串的协议含义是“禁用该 sink 的注册和发送”。
- `SingleTargetError::{ChannelFull, Closed}`：公开非阻塞 API `TrySend` 的精确错误；实现 `DataSink` 时映射为共享的 `DataSinkError` 对应变体。
- `sendTask`：内部队列元素，包含共享载荷 `Arc<ReportData>` 和绝对截止时间 `Instant`。
- `connectionState`：仅由 worker 使用，保存 `Option<TopSqlAgentClient<Channel>>` 及其地址，避免把异步 client 放进跨线程锁。
- `recoverSendPanic`：包裹单个异步发送分支，捕获 panic payload 并转成 `anyhow::Error`，使一个分支 panic 不会直接破坏 worker。
- `TopRURecordStream` / `sendTopRURecords`：对可写/可关闭的 TopRU 流做抽象；`Unimplemented` 被视为向旧服务端兼容成功，其它错误返回调用者。它们当前不被 `sendBatchTopRURecord` 调用。
- `remaining`、`protobuf_to_prost_vec`：分别计算严格为正的剩余 deadline，以及利用 protobuf wire format 把 protobuf-codec 消息转换为 prost 消息。
- `incrementIgnoreReportChannelFull`、`observeAll`、`observeRecords`、`observeSQL`、`observePlans`：更新 reporter-metrics 中可选的进程级指标。

## 执行流程

1. `default_pipeline` 构造 `RemoteTopSQLReporter`，再把它作为 `Arc<dyn DataSinkRegisterer>` 传给 `NewSingleTargetDataSink`；构造器创建容量 1 的发送通道和取消通道，所有状态初始为未启动、未注册、未取消。
2. `SetupTopProfiling` 调用 `Start`。`Start` 以 `started.swap(true)` 保证最多启动一次；若已经取消也直接返回。当前地址非空时先同步注册，随后创建 OS worker 线程。
3. `recoverRun` 在线程中创建 current-thread Tokio runtime，并初始化 worker 私有的 `connectionState`。它以 `catch_unwind` 包住 `run`；panic 会记录后重进循环，正常取消或通道断开则退出。runtime 创建失败时只记录错误并结束 worker。
4. `run` 在取消通知、发送任务和地址轮询 tick 三者之间选择。收到任务后重新读取地址：空地址直接丢弃该任务；非空地址则用 runtime 执行 `doSend`。每轮之后都调用 `trySwitchRegistration`，地址空时注销，地址非空且未注册时注册。
5. Reporter 的 `RemoteTopSQLReporter::trySend` 调用 trait 方法 `DataSink::try_send`；本文件的实现转到 `try_send_inner`。它先检查 `cancelled`，再 `try_send` 到容量 1 的通道，不等待 worker 腾空。
6. `doSend` 先以批次 deadline 建立或复用连接，再把 `ReportData` 的四组 protobuf-codec 消息转为 prost。随后用 `tokio::join!` 同时等待 SQL 元数据、Plan 元数据、TopSQL 和 TopRU 四个分支；前三个进行 gRPC 调用，TopRU 分支当前为空操作。任一分支错误都会使整批失败，但 `join!` 会等待四个分支都完成。
7. 三个真实发送方法对空输入立即成功；非空输入构造迭代流、给 tonic request 设置剩余 deadline，并调用相应 client-streaming RPC。流每产出一条才增加 `sentCount`，完成后记录条数和成功/失败耗时。
8. `Close` 幂等取消 worker、在仍注册时注销，并在非 worker 当前线程中 join；Reporter 自身关闭时，`DefaultDataSinkRegisterer::close` 先移除所有 sink，再调用 `on_reporter_closing`，后者只触发取消，避免回调重入注册器。

## 数据与状态

`ReportData` 定义在 `datasink.rs`，包含 `data_records`、`ru_records`、`sql_metas`、`plan_metas`。队列中使用 `Arc<ReportData>`，trait 分发时各 sink 共享载荷；公开便捷方法 `TrySend` 接收所有权并封装成 `Arc`。进入 gRPC 边界时，`protobuf_to_prost_vec` 会为四类消息各做一次编码和解码，因此批次内存峰值同时包含原始消息、wire bytes 的逐条临时值和转换后的向量。

状态不变量如下：`started` 使 `Start` 只创建一个 worker；`cancelled` 使取消只发送一次并阻止新任务/重新注册；`registered` 表示该 sink 是否应存在于注册器中；`worker` 至多保存一个 `JoinHandle`。这三个布尔状态使用 `SeqCst`，而每个发送流的局部计数只用 `Relaxed`，因为计数在对应 future 完成后才读取，不承担同步职责。

`connectionState` 的 `curRPCAddr` 与 `conn` 必须配套判断：只有地址相同且 `conn.is_some()` 才复用；换址或无连接时先 `take` 旧 client，再连接新地址，成功后才同时写回 client 和地址。连接失败时旧连接已被丢弃，下次任务会重试。

## 依赖与调用关系

上游直接关系：

- `pkg/util/topsql/topsql.rs::default_pipeline` → `NewSingleTargetDataSink`；`SetupTopProfiling` → `SingleTargetDataSink::Start`。
- `pkg/util/topsql/reporter/reporter.rs::trySend` → `DataSink::try_send` → `try_send_inner`。
- `pkg/util/topsql/reporter/reporter.rs::Close` → `DefaultDataSinkRegisterer::close` → `DataSink::on_reporter_closing` → `OnReporterClosing`。

文件内主调用链是 `Start → recoverRun → run → doSend → tryEstablishConnection / sendBatchSQLMeta / sendBatchPlanMeta / sendBatchTopSQLRecord / sendBatchTopRURecord`。注册链是 `Start/run → DataSinkRegisterer::{register,deregister}`。

下游 crate 依赖由 `Cargo.toml` 证实：`crossbeam-channel` 提供同步非阻塞队列与 ticker；`tokio`/`tokio-stream` 提供 runtime、并发和流；`tonic`/`prost` 提供 gRPC client 与消息；固定的 `protobuf = 2.8.0` 和 git `tipb` 的 `protobuf-codec` feature 提供 Reporter 侧消息，二者在边界转换；`reporter_metrics` 提供观测指标；`anyhow`、`thiserror` 分别承载内部上下文错误和公开枚举错误。

RustCodeGraph 能索引目标文件、构造函数和 `sendTopRURecords`，但本次索引没有为该 Rust 文件多数 `impl` 方法建立可查询的独立调用节点，且文件级报告为 `used by 0 files`；因此上面的 Rust 上游边由索引源码中的实际调用点及 `rg` 补证，不将该文件级数字解释为“生产未接线”。

## 错误处理与边界

- 背压和关闭是调用者可区分的同步错误；队列满还递增 `IgnoreReportChannelFullCounter`。Reporter 的广播层会逐 sink 记录错误但继续发送给其它 sink。
- deadline 同时限制拨号和每个真实 RPC。`remaining` 在截止时间已到或剩余为零时失败；拨号 timeout 取剩余时间与 5 秒的较小值。
- 地址会在任务出队时读取，而不是入队时固定；配置变化期间，任务发送到“处理时”的地址。地址为空时任务被静默丢弃，且不会计入整批成功/失败直方图。
- 注册失败：`Start` 的首次失败只记录日志，worker 仍启动并可在后续 tick 重试；`run` 中的后续注册失败会让该 worker 正常退出，当前实现不会为这种非 panic 退出自动再建线程。
- gRPC URI 没有 scheme 时自动补 `http://`，即当前连接是明文 transport；非法 URI、连接超时、RPC status、protobuf 转换和 deadline 都向 `doSend` 汇总为失败。
- `recoverSendPanic` 可处理 `&str`、`String` 和未知 panic payload。外层 `recoverRun` 还能捕获同步 worker panic并继续，但锁中毒处广泛使用 `expect`；持续的确定性 panic 可能形成反复重启。
- 指标静态量通过 `unsafe` 读取 `Option`；指标未初始化时跳过记录，不影响发送结果。
- `sendTopRURecords` 对 `Send` 或 `CloseAndRecv` 返回 `Unimplemented` 均兼容为成功；若已有发送错误，非 `Unimplemented` 的 close 错误不会覆盖原错误。当前真实 SingleTarget 路径不调用它，不能据此宣称 TopRU 已发送。

## 并发与资源生命周期

每个 sink 最多拥有一个 OS worker。worker 内只有一个 current-thread Tokio runtime；每批内部的四个 future 由 `tokio::join!` 并发轮询，不会创建四个额外 OS 线程。发送队列容量为 1，因此“worker 正处理一批 + 队列等待一批”是常见的最大在途形态，之后的入队立即失败。

`Close` 是完整拥有者清理入口：触发取消、注销、取出句柄并 join。若从 worker 自身调用则跳过 join，避免自锁。`OnReporterClosing` 只取消，因为调用它的注册器已经先清空 sink 表；它不 join，实际线程会在下一次 select 观察到取消后退出。worker 退出时局部 `connectionState` 和 runtime 被 drop；tonic `Channel` 采用引用计数，旧连接在最后一个在途 clone 完成后释放。

地址由 `RwLock` 并发读写；轮询间隔由 `Mutex` 保护，但 `SetPollInterval` 强制只能在 `Start` 前调用，worker 启动后会复制一次 interval。发送状态指标计数使用每个 future 独立的 `Arc<AtomicUsize>`，避免共享可变集合。

## 与 Go 版本的对应关系

主要语义与同目录 `single_target.go` 对齐：5 秒拨号超时、1 GiB stream/connection 初始窗口、容量 1 的非阻塞队列、地址轮询注册、同地址连接复用、三路真实 client-streaming 上报、四分支并行、panic 恢复、TopRU 空操作，以及 `sendTopRURecords` 对 `Unimplemented` 的兼容。Rust 的 `single_target_test.rs` 对照 Go 主测试和 TopRU 兼容测试，`single_target_3_aster_unit_test.rs` 另补容量/关闭和地址切换覆盖。

实现机制存在以下可见差异：

- Go 直接读取 `config.GetGlobalConfig().TopSQL.ReceiverAddress`；Rust 通过 `ReceiverAddressProvider`，生产默认值必须由 `SetGlobalReceiverAddress` 写入进程级对象。
- Go 使用 goroutine、`context.Context` 和 `grpc.ClientConn`；Rust 使用 OS 线程、current-thread Tokio runtime、`Instant` deadline 和 tonic `Channel`。Rust `Close` 会 join worker，生命周期收束比 Go 的仅 cancel 更显式。
- Go 在 `grpc.DialContext` 配置了特定 connect backoff；Rust 当前只设置 connect timeout 和窗口大小，没有对应的自定义 backoff 配置。
- Go 的消息类型统一来自 go-tipb；Rust Reporter 数据是 protobuf-codec，而 tonic client 是 prost，因此增加 wire-format 转换步骤。
- Go 的流式发送显式逐条 `Send` 后 `CloseAndRecv`；tonic client-streaming 接收一个迭代 stream 并在 RPC future 完成时取得响应，语义对应但 API 形态不同。

## 扩展指南

- 要真正启用 SingleTarget TopRU，应修改 `sendBatchTopRURecord` 并接入 tonic RPC，而不是只改目前未接线的 `sendTopRURecords`。必须同步独立测试 `single_target_test.rs` 和 `single_target_3_aster_unit_test.rs`，并重新评估旧 Agent 的 `Unimplemented` 兼容、TopRU 开关语义和指标；同时对照 Go `single_target.go`，避免两端行为漂移。
- 要改变队列容量或丢弃策略，应从 `NewSingleTargetDataSinkWithReceiver` 和 `try_send_inner` 入手，并保留非阻塞契约；同步容量满、关闭后发送和 Reporter 广播不中断的测试。增大容量会增加陈旧数据和内存占用。
- 要改变动态地址来源，实现新的 `ReceiverAddressProvider` 即可；必须明确何处写入默认全局地址，并测试空地址、换址、注册失败和关闭竞态。若要求换址立即生效，应重新设计 ticker/通知机制，当前最多延迟一个 `pollInterval`。
- 要调整连接安全性、backoff 或消息大小，从 `tryEstablishConnection` 修改 `Endpoint`/client 配置，并覆盖 URI 带/不带 scheme、deadline、连接失败和换址重连。启用 TLS 会改变当前 `http://` 默认兼容行为。
- 要增加新的批次数据种类，应同步 `datasink.rs::ReportData`、protobuf→prost 转换、`doSend` 并发汇总、子流指标、mock server 和两个独立 Rust 测试文件；注意 `tokio::join!` 的任一错误会使整批报告失败。
- 所有 Rust 测试继续放在独立的 `single_target_test.rs` 或迁移补充测试文件中，不把测试内嵌回生产源文件。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 Rust/Go 文件均在索引中。
- RustCodeGraph `node --file pkg/util/topsql/reporter/single_target.rs`：读取目标文件 1–698 行，核对全部常量、类型、trait、函数、`impl`、状态机、发送路径和指标逻辑。
- RustCodeGraph `query`：确认 Rust `SingleTargetDataSink`、`NewSingleTargetDataSink`、`NewSingleTargetDataSinkWithReceiver` 和 `sendTopRURecords` 的符号位置；调用图未暴露多数 Rust `impl` 方法节点，此限制已在“依赖与调用关系”中说明。
- RustCodeGraph 文件节点：读取 `lib.rs`、`datasink.rs`、`pkg/util/topsql/topsql.rs`、`reporter.rs`、`single_target.go`、`single_target_test.go`、`single_target_test.rs`、`single_target_3_aster_unit_test.rs` 的相关范围，核对模块挂载、生产入口、分发/关闭链、Go 语义及测试断言。
- 直接读取未由代码图覆盖的 `Cargo.toml`，核对 crate、feature、build 脚本及外部/本地依赖；用 `rg` 定位 Rust 构造、启动和 trait 分发调用点以补足图索引缺口。
- 人工复核结论：该文件存在是为了把 Reporter 批次非阻塞地送往一个动态配置的 gRPC 接收端；其运行依赖注册、worker、连接复用和并发子流；安全扩展必须同时维护背压、deadline、关闭、旧 Agent 兼容及独立测试边界。

本任务为纯文档分析，按计划不运行 Cargo 或运行时代码测试；交付验证仅检查文档存在且恰含规定的十一个二级章节。
