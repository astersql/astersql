# `br/pkg/mock/stubs.rs`

## 文件定位

`stubs.rs` 是 `astersql-br-pkg-mock` crate 的本地边界替身层，由 [`lib.rs`](lib.rs) 通过 `#[path = "stubs.rs"] pub mod stubs` 装入，并把其中大部分公共类型平铺再导出。该 crate 的 [`Cargo.toml`](Cargo.toml) 没有依赖项，元数据将其映射到 Go 包 `br/pkg/mock`；注释明确这是为 darwin/arm64 等不便链接 `kvproto`、`grpcio`、TiDB domain/server 与 Lightning 重依赖的环境准备的瘦身实现。

它服务于 mock crate 的 `backend.rs`、`encode.rs`、`importer.rs`、`mock_cluster.rs` 和 `task_register.rs`，也被其他 Rust 移植代码当作测试边界类型使用。它不执行真实备份恢复、行编码、ImportKV RPC、PD/TiKV 通信或端口监听，因此不能视为这些生产子系统的实现。

## 核心职责

文件包含四组职责：

1. `Error`、`Result<T>`、`Context` 提供 Go `error`、`context.Context` 的最小签名外形。
2. `Controller`、`Call` 与 `take_error`/`take_pair`/`take_one` 支撑 Rust 版 mockgen 文件的 `EXPECT → Return → 实际调用` 流程。
3. `UUID`、`EngineWriter`、ImportKV 请求/响应、`WriteEngineClient` 等类型替代 Lightning、模型、protobuf 和 gRPC 类型。
4. `Storage`、`Domain`、`Server`、`HttpServer`、`Config`、`MysqlConfig` 及可注入探测钩子支撑 `mock_cluster.rs` 的 `NewCluster → Start → Stop` 生命周期。

这些实现刻意只保持测试所需的接口形状和少量可观察状态。比如 `Datum`、`EngineConfig` 是空结构体，`NilEngineWriter` 不保存行，`NilWriteEngineClient` 不发送消息，`HttpServer::ListenAndServe` 也不绑定套接字。

## 主要符号

- `Error { msg }`、`Result<T>`：统一轻量错误；`Error::Trace` 原样返回错误，不增加栈信息。`Context { cancelled }` 仅保留取消标志，`Context::background()` 创建未取消上下文。
- `Controller`：以 `Arc<Mutex<ControllerInner>>` 共享 `VecDeque<ExpectedCall>`。`RecordCallWithMethodType` 只记录方法名，`Call` 查找并移除首个同名期望，`remaining` 返回未消费数量。
- `Call`：通过 `Return`、`ReturnError`、`Return1`、`Return2` 向对应 `ExpectedCall` 写入 `Any + Send` 返回槽。`take_error`、`take_pair`、`take_one` 负责将动态槽还原为 Rust 返回值。
- Lightning/模型外形：`UUID`、`EngineConfig`、`LocalWriterConfig`、`CheckCtx`、`EncodingConfig`、`Datum`、`KVChecksum`、`DBInfo`、`TableInfo`、`RowsHandle` 和 `RowHandle`。多数只保留测试实际读取的字段。
- `ChunkFlushStatus`、`EngineWriter`：定义刷盘状态与引擎写入对象安全接口；`SimpleChunkFlushStatus` 和 `NilEngineWriter` 是最小实现。
- ImportKV/gRPC 外形：各 `*Request`/`*Response`、`CallOption`、`Metadata`、`WriteEngineClient` 与 `NilWriteEngineClient`。请求主要保留 `uuid` 或 `mode`，响应主要保留 `metrics`、`version`、`error`。
- 集群资源：`TiKVCluster`、`Storage`、`RegionCache`、`PDClient`、`PDHTTPClient`、`Domain`、`TiDBDriver`、`Server`、`HttpServer`。关闭状态和运行状态使用共享原子布尔值，使测试能在句柄克隆后观察生命周期。
- 构造与配置：`Config::NewConfig`/`NewConfig`、`NewTiDBDriver`、`NewServer`、`NewMockStoreWithoutBootstrap`、`BootstrapWithSingleStore`、`BootstrapSession`、`DisableStats4Test`、`view_Stop`。
- 测试协调与探测：`RUN_IN_GO_TEST`、`make_run_in_go_test_chan`、`notify_run_in_go_test`、`MysqlConfig::FormatDSN`、`retry_time`、`sleep_retry`、`sql_open`、`http_get`、各 `set_*` 函数及 `reset_test_hooks`。

## 执行流程

mockgen 调用链从生成式模块的 recorder 开始。例如 `backend.rs` 或 `importer.rs` 的 `EXPECT()` 调用 `Controller::RecordCallWithMethodType`，创建共享的 `ExpectedCall`；测试再用 `Call::Return*` 写入返回槽。实际 mock 方法调用 `Controller::Call`，控制器按方法名搜索、移除期望并转移返回槽，随后由 `take_error`、`take_pair`、`take_one` 或 `importer.rs::take_response` 解包。参数会装箱传入，但本控制器不比较参数。

集群链由 `mock_cluster.rs::NewCluster` 驱动：`NewMockStoreWithoutBootstrap` 创建共享 `TiKVCluster`，inspector 调用 `BootstrapWithSingleStore` 将其设为一个 store 且已 bootstrap；之后 `BootstrapSession` 返回 `Domain`，`Storage` 暴露 PD 与 PD HTTP 客户端。`Cluster::Start` 用 `NewTiDBDriver` 和 `NewServer` 组装服务，`Server::Run` 设置 running/ready、发送就绪通知并等待关闭；`waitUntilServerOnline` 通过本文件的 `sql_open`、`http_get`、`retry_time` 与 `sleep_retry` 探测，再形成 DSN。`Cluster::Stop` 最终调用资源的 `Close` 并翻转状态。

探测函数按“注入钩子优先、线程局部 online 标志次之、否则返回错误”的顺序工作。`reset_test_hooks` 恢复重试次数、休眠、online 状态、两个闭包钩子和进程级 `RUN_IN_GO_TEST`，供测试在用例边界清理状态。

## 数据与状态

期望调用队列位于 `ControllerInner.expected`；每个 `ExpectedCall.rets` 有独立 `Mutex`，而 `Call` 与 `Controller` 克隆都共享底层 `Arc`。同名期望按队列中首次出现的位置消费，不同方法可以交错消费；一条期望只会被移除一次。

集群对象的共享状态包括 `Storage.cluster: Arc<Mutex<TiKVCluster>>`，以及 `Storage`、`Domain`、`Server`、`HttpServer` 的 `Arc<AtomicBool>` 标志。`Server.ready` 是 `Mutex<bool> + Condvar`，用于在 `Run` 和 `wait_ready` 之间传递就绪事件。`GO_TEST_NOTIFY` 是进程级 `Mutex<Option<Sender<()>>>`，而 `RUN_IN_GO_TEST` 是进程级原子值。

重试次数、休眠毫秒数、跳过休眠、SQL/HTTP 闭包和 `CLUSTER_ONLINE` 都是 `thread_local!` 状态；它们不会自动传播到新线程。`MysqlConfig` 持有 DSN 五个字段，`FormatDSN` 只处理空/非空密码差异。ImportKV 与 Lightning 替身数据大多是拥有所有权的 `String`、`Vec<u8>` 或数值，不维护真实协议状态。

## 依赖与调用关系

直接依赖仅来自标准库：`Any` 用于动态返回槽，`VecDeque` 保存期望，`Arc`/`Mutex`/`AtomicBool`/`Condvar` 支撑共享状态，`Cell`/`RefCell` 保存线程局部钩子，`Duration` 与线程休眠模拟等待。`Cargo.toml` 的空 `[dependencies]` 验证该文件没有外部 crate 依赖。

上游方面，`lib.rs` 导出本模块；`backend.rs`、`encode.rs`、`importer.rs`、`task_register.rs` 依赖 `Controller`、数据外形和解包函数生成 GoMock 风格 API；`mock_cluster.rs` 依赖集群资源、构造函数与探测钩子。RustCodeGraph 将该文件标记为被 35 个文件使用，并明确给出 `mock_cluster.rs::NewCluster/Start/Stop`、`importer.rs` 多个 RPC 方法及本目录测试对这些符号的调用边。

下游方面，本文件不调用仓库业务 crate，只操作标准库状态。最重要的跨文件链为 `mock_cluster.rs::NewCluster → NewMockStoreWithoutBootstrap/BootstrapWithSingleStore/BootstrapSession`、`Cluster::Start → NewTiDBDriver/NewServer/make_run_in_go_test_chan`，以及 `importer.rs::call_unary → Controller::Call → take_response/take_error`。

## 错误处理与边界

`Error` 只有消息，没有错误码、来源链或堆栈；`Error::Trace` 不增强错误。`Controller::Call` 在不存在同名期望时 panic，锁中毒也通过 `expect`/`unwrap` panic。控制器不校验 receiver、参数值、调用次数范围、`Times`、`After`、`DoAndReturn` 或显式顺序约束，因此它不是完整 gomock 实现。

返回值解包有意宽松：`take_error` 对空槽和未知类型返回成功；`take_pair`、`take_one` 在缺槽或类型不匹配时回退 `Default`。这便于机械移植的 mock 可运行，但也可能掩盖错误的 EXPECT 返回类型；测试需要用非默认探针值和 `remaining() == 0` 约束契约。

空实现不代表真实成功：`NilEngineWriter::AppendRows` 不写入数据，`NilWriteEngineClient` 不维护 gRPC 半关闭或消息状态，`NewServer` 用固定非零端口替代真实临时端口，SQL/HTTP 探测不会建立真实连接，`DisableStats4Test` 和 `view_Stop` 无副作用。`Context.cancelled` 也不会中断等待或 I/O。

## 并发与资源生命周期

`Controller` 可跨线程克隆，队列和返回槽分别受互斥锁保护；匹配与移除在同一队列锁临界区完成。集群关闭标志采用 `SeqCst`，便于测试从克隆句柄稳定观察。`Server::Run` 先设置 running 和 ready、唤醒条件变量并发送 channel 通知，然后每毫秒轮询 `running/closed`；`Server::Close` 翻转标志并唤醒等待者。

`GO_TEST_NOTIFY` 和 `RUN_IN_GO_TEST` 属于进程共享状态，但探测钩子属于线程局部状态；跨线程启动服务时不能假定调用线程设置的 `CLUSTER_ONLINE` 或闭包可见。`reset_test_hooks` 只清理当前线程的 thread-local 值和全局 `RUN_IN_GO_TEST`，不会清除 `GO_TEST_NOTIFY` 中已有 sender，也不会替调用者关闭 `Storage`、`Domain`、`Server` 或 `HttpServer`。

资源所有权由 `mock_cluster.rs::Cluster` 聚合并显式 `Stop`。`Storage::Close`、`Domain::Close`、`Server::Close`、`HttpServer::Close` 都只更新可观察标志；没有真实连接、线程句柄或套接字需要回收。`br/pkg/mock/mock_cluster_test.rs` 会在 `Stop` 后检查这些标志，形成当前生命周期契约。

## 与 Go 版本的对应关系

本文件没有同名 Go 文件，而是把多个 Go/外部依赖边界集中成本地替身。`Controller`/`Call` 对应 `go.uber.org/mock/gomock`；`backend.go`、`encode.go`、`importer.go` 与 `task_register.go` 是 MockGen 生成物，Rust 对应文件调用这里的控制器和类型外形。Go `importer.go` 的 ImportKV protobuf、gRPC metadata 与 stream client 被本文件的请求/响应、`Metadata` 和 `WriteEngineClient` 取代。

集群部分直接服务于 Rust `mock_cluster.rs` 对 Go `mock_cluster.go` 的移植。Go 版本会创建真实 mockstore、domain、server、SQL 连接和 HTTP 请求；Rust 替身只保留 bootstrap 标志、资源关闭状态、ready 通知和可注入探测。`MysqlConfig::FormatDSN` 对齐 Go MySQL 配置的常用格式，但不是完整 go-sql-driver 编码器。

主要已知差异是：参数不匹配、返回类型错误和缺失返回值在 Rust 轻量控制器中更宽松；服务/RPC/I/O 均不真实发生；配置与模型字段只是测试所需子集；`Context` 不实现 deadline/value/cancellation 传播。因此 Go 文件仍是行为权威，本文件只保证当前 Rust 测试契约。

## 扩展指南

新增 mock 方法时，应先确认 Go 接口或 MockGen 文件中的签名，再在相应独立 Rust 模块增加实际方法和 recorder；只有缺少共享数据外形或解包能力时才修改本文件。返回形状应复用 `ReturnError`、`Return1`、`Return2` 或明确新增解包器，并在独立测试中使用非默认值验证，避免 `Default` 回退造成假阳性。

新增 ImportKV 字段或 RPC 时，应同步请求/响应结构、`importer.rs` 的 client/recorder 路径和 `importer_test.rs`；若需要真实 protobuf 编解码、metadata、deadline 或流状态机，应引入独立真实实现，而不是继续扩张这些空桩。新增 Lightning 类型字段也应以 `backend.go`/`encode.go` 的实际接口使用为依据。

修改集群生命周期或探测逻辑时，应同步 `mock_cluster.rs`，并扩展 `mock_cluster_test.rs` 与 `parity_test.rs`；测试必须在结尾调用 `reset_test_hooks`，且并发测试要考虑 thread-local 与进程级状态的差异。测试逻辑继续放在独立 `*_test.rs` 文件，不应嵌入 `stubs.rs`。

兼容风险主要来自 Go 接口签名漂移和宽松解包掩盖类型错误；正确性风险来自把空操作误当成真实 I/O；性能风险目前仅限锁竞争和 `Server::Run` 的 1ms 轮询。若未来让桩承载大量并发，应重新评估队列线性搜索、全局通知槽和轮询退出机制。

## 验证依据

- 源文件：`br/pkg/mock/stubs.rs`，核对了 996 行全部实现，包括 139 个索引符号、四组职责、共享状态和全部公开函数。
- crate 与入口：`br/pkg/mock/Cargo.toml`、`br/pkg/mock/lib.rs`，确认 crate 名、Go 包映射、空依赖集合、模块装配及公开再导出。
- 直接 Rust 调用：`br/pkg/mock/backend.rs`、`encode.rs`、`importer.rs`、`mock_cluster.rs`、`task_register.rs`；RustCodeGraph 的 `explore`/`callers`/`callees` 结果确认 `Controller::Call`、集群构造与探测钩子的实际调用边。
- Go 对照：`br/pkg/mock/backend.go`、`encode.go`、`importer.go`、`task_register.go`、`mock_cluster.go`；其中生成文件定义 gomock/API 形状，`mock_cluster.go` 定义真实 `NewCluster`、`Start`、`Stop`、DSN 与在线探测语义。
- 独立测试：`br/pkg/mock/parity_test.rs` 覆盖 EXPECT/Return、错误传播、非默认返回值、ImportKV、DSN 和探测边界；`br/pkg/mock/importer_test.rs` 覆盖流创建与响应错误；`br/pkg/mock/mock_cluster_test.rs` 覆盖 bootstrap、Start/Stop 和关闭状态。
- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/mock` 找到本目录 27 个索引文件；`node --file br/pkg/mock/stubs.rs` 显示该文件被 35 个文件使用；针对文件、`Controller`、`reset_test_hooks`、`NewMockStoreWithoutBootstrap` 与 `sql_open` 的查询用于复核上下游关系。
- 本任务是纯文档分析，未运行 Cargo。交付前使用任务规定的命令检查文档存在且恰含 11 个固定二级标题，并人工复核没有把空桩描述为生产实现。
