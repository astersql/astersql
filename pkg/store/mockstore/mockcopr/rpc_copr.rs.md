# `pkg/store/mockstore/mockcopr/rpc_copr.rs`

## 文件定位

本文件是 `astersql-store-mockstore-mockcopr` crate 的 RPC 适配层：它把 mockstore 调用方看到的单次 Coprocessor 和 BatchCop 接口，接到同 crate 的 `coprHandler` 上，并负责 BatchCop 流的首包预取、超时租约和后台监控线程。模块由 `lib.rs` 的 `mod rpc_copr` 纳入，并通过 `pub use rpc_copr::*` 对 crate 使用者公开。

它服务于内存 mock Coprocessor，而不是网络 gRPC 服务端。默认构造函数 `NewCoprRPCHandler` 使用 `MemoryReader`；需要注入自定义存储视图时使用 `NewCoprRPCHandlerWithReader`。当前仓库中的直接 Rust 使用证据主要在独立测试：`rpc_copr_test.rs` 验证 BatchCop Region 错误协议，`executor_test.rs` 构造默认处理器以对齐 Go mock TiKV 测试入口。RustCodeGraph 将本文件识别为含 38 个符号的生产文件，并显示它被 7 个文件引用，但精确 `callers` 查询未给出可用调用边，因此不能据此声称它已接入真实 TiDB 请求主链。

crate 边界由 `pkg/store/mockstore/mockcopr/Cargo.toml` 确定：库入口是 `lib.rs`，Go 对照包是 `pkg/store/mockstore/mockcopr`。清单中的大量 AsterSQL 依赖均为 optional；本文件本身只使用标准库并依赖同 crate 的 `copr_handler` 类型。

## 核心职责

1. `RPCSession` 将一次 mock RPC 所需的 `Arc<dyn KvReader>` 与可注入的 `CopError` Region 错误绑定，并在派发前统一执行 `CheckRequestContext`。
2. `CoprRPCHandler` 定义流式旧接口、单次 `CmdCop`、`BatchCop` 和关闭协议；`coprRPCHandler` 给出默认实现。
3. `HandleCmdCop` 完成上下文校验，把请求交给 `coprHandler::handle_request`，并将其单元素批响应折叠为普通 `Response`。
4. `HandleBatchCop` 完成上下文校验、构造数据流客户端、注册超时租约、预取首个 `BatchResponse`，随后把可继续 `Recv` 的包装对象交给调用方。
5. `check_stream_timeout_loop` 在独立线程中维护租约集合，到期时原子取消租约；`Close`/`Drop` 负责停止并回收线程。

本文件不解析 DAG、不执行算子，也不编码行。相关工作位于 `copr_handler.rs`：`handle_request` 按 `RequestPayload` 分发 DAG、Analyze、Checksum，`handleBatchCopRequest` 构建 DAG 执行器、调用 `drainRowsFromExecutor`，再生成 `mockBatchCopDataClient`。

## 主要符号

- `RPCSession { reader, region_error }`：一次调用的读取上下文。`new` 初始化读取器并令 `region_error = None`；`CheckRequestContext` 克隆并返回预置错误，否则返回 `Ok(())`。
- `Lease { deadline, cancelled }`：BatchCop 超时状态。`new(timeout)` 以 `Instant::now() + timeout` 固定截止时间；`Cancel`/`IsCancelled` 用 `AtomicBool` 的 `SeqCst` 顺序在线程间发布与读取取消状态。
- `BatchCopClient`：流客户端的二态枚举。`Data(mockBatchCopDataClient)` 逐条读取预计算结果；`RegionError(CopError)` 每次 `Recv` 都构造一个 `other_error` 响应，不会自然耗尽。
- `BatchCopStreamResponse { client, BatchResponse, Lease, Timeout }`：对外的 BatchCop 流包装。`BatchResponse` 保存预取首包，`Recv` 在访问客户端前检查租约是否已经取消。
- `CoprRPCHandler: Send`：公开抽象，约束处理器可在线程间转移。其 `HandleCopStream` 保留旧接口形状但当前必定 panic。
- `coprRPCHandler`：默认实现，持有一个 `RPCSession`、容量 1024 的租约 `SyncSender`、一次性关闭 sender 和后台 `JoinHandle`。字段 `session` 是构造器绑定的会话，但当前处理方法实际接收显式 `session` 参数；两者不要混淆。
- `NewCoprRPCHandler()`：以空的 `MemoryReader` 创建并装箱为 `Box<dyn CoprRPCHandler>`。
- `NewCoprRPCHandlerWithReader(reader)`：注入读取器并返回具体 `coprRPCHandler`，便于测试访问实现方法。
- `check_stream_timeout_loop(...)`：私有工作循环，每轮收取新租约、取消到期租约，并以 10ms `recv_timeout` 同时承担退出等待与轮询节流。

## 执行流程

`coprRPCHandler::new` 首先创建容量 1024 的同步租约通道及无界退出通道，然后启动一个线程运行 `check_stream_timeout_loop`，最后构造内部 `RPCSession` 并保存两端资源。

单次请求路径如下：

1. `HandleCmdCop(session, request)` 调用 `session.CheckRequestContext()`。
2. 若存在 Region 错误，立即返回只设置 `region_error` 的默认响应，不执行下游逻辑。
3. 否则克隆 `session.reader` 与 `session.region_error`，临时构造 `coprHandler`。
4. 调用 `coprHandler::handle_request`。该函数依据 `RequestPayload` 执行 DAG、Analyze 或 Checksum，并包装成包含一个普通响应的 `BatchResponse`。
5. RPC 层取 `responses` 的首项；若下游意外返回空列表，则以 `Response::default()` 兜底。

批量请求正常路径如下：

1. `HandleBatchCop` 先校验 `RPCSession`，再构造相同读取上下文的 `coprHandler`。
2. `handleBatchCopRequest` 遍历 `BatchRequest.requests`；每个请求均构造 DAG 执行器、排空行并生成一个 `BatchResponse`。任何构建或执行错误通过 `?` 返回，尚不创建租约。
3. RPC 层创建 `Lease::new(timeout)`，将克隆值发送到后台线程。通道已断开时返回 `CopError::InvalidRequest("stream timeout loop has stopped")`。
4. 组装 `BatchCopStreamResponse`，立即调用一次 `Recv` 预取首包，并存入 `BatchResponse` 字段。预取失败时整个 `HandleBatchCop` 返回错误。因此空 `BatchRequest` 会在首包阶段得到下游 `EndOfStream`，而不是返回一个空流包装。
5. 调用方读取预取字段后，可继续调用 `Recv`。数据客户端按偏移消费剩余的预计算响应，耗尽后返回 `CopError::EndOfStream`；若后台线程先取消租约，则包装层先返回超时 `InvalidRequest`。

Region 错误的 BatchCop 路径不进入 `coprHandler`，也不注册后台租约：它创建 `BatchCopClient::RegionError`、零超时字段并立即预取错误响应。由于该客户端每次都重新生成同一 `other_error`，后续 `Recv` 仍重复返回该错误响应。这一协议由 `rpc_copr_test.rs::batch_cop_region_error_is_returned_in_the_stream_first_response` 直接断言。

## 数据与状态

- 读取状态通过 `Arc<dyn KvReader>` 共享。RPC 层只克隆 `Arc`，不复制底层数据，也不持有显式读锁；具体一致性与并发能力由 `KvReader` 实现负责。
- Region 错误是 `RPCSession.region_error: Option<CopError>` 中的预置状态。`CheckRequestContext` 返回克隆值，因此原会话可被后续调用复用。
- 每个正常 BatchCop 流各有一个 `Lease`。后台线程和返回给调用方的流持有同一个 `Arc<AtomicBool>`，但各自保存同一截止时刻的值。
- 数据流并非边执行边生产：`copr_handler.rs::handleBatchCopRequest` 在返回前把所有请求执行为 `Vec<BatchResponse>`，`mockBatchCopDataClient.offset` 仅控制读取位置。因此内存占用随批量请求及结果规模增长，超时也不能中断已经发生的预计算阶段。
- `BatchCopStreamResponse.BatchResponse` 是首包缓存而非当前游标；后续 `Recv` 不会自动更新该字段。
- `coprRPCHandler.session` 保存构造时读取器，但 `HandleCmdCop` 和 `HandleBatchCop` 使用调用参数中的 `session`。当前实现没有校验两者是否对应同一读取器。

## 依赖与调用关系

上游装配关系为 `lib.rs -> mod rpc_copr -> pub use rpc_copr::*`。已核实的 Rust 调用包括 `rpc_copr_test.rs -> NewCoprRPCHandlerWithReader -> HandleBatchCop`，以及 `executor_test.rs::test_resolved_large_txn_locks -> NewCoprRPCHandler`。RustCodeGraph 的文件级关系还列出其他测试及 `pkg/dxf/importinto/conflictedkv/collector.rs` 等引用文件，但精确符号调用边查询无输出；文档不把文件级“used by”误写成已经验证的运行时调用。

下游关系分为两条：

- `HandleCmdCop -> RPCSession::CheckRequestContext -> coprHandler::handle_request -> handleCopDAGRequest / handleCopAnalyzeRequest / handleCopChecksumRequest`。
- `HandleBatchCop -> RPCSession::CheckRequestContext -> coprHandler::handleBatchCopRequest -> buildDAGExecutor -> drainRowsFromExecutor -> mockBatchCopDataClient::Recv`；正常流还经过 `Lease` 与 `check_stream_timeout_loop`。

所有本文件直接导入均来自标准库或 `crate::copr_handler`。`Cargo.toml` 没有为 RPC 层声明专门 feature；crate 的外部 AsterSQL 依赖多为 optional，不能仅凭清单推断本文件在某一 feature 下启用。模块本身也没有 `#[cfg(...)]` 条件编译项。

## 错误处理与边界

- `HandleCopStream` 是显式弃用边界，调用必定 panic；不能把 trait 中存在该方法理解为功能已支持。
- CmdCop 的 Region 错误写入 `Response.region_error`，不是 Rust `Err`。BatchCop 的 Region 错误写入首包与后续包的 `other_error`，同时外层返回 `Ok(stream)`。两者是协议响应，不是传输层失败。
- 正常 BatchCop 的 DAG 构建、执行、行投影或读取失败由 `handleBatchCopRequest` 原样传播为 `Err(CopError)`；租约通道关闭被转换为带固定消息的 `InvalidRequest`；首包读取失败也阻止返回流对象。
- 超时只在每次 `BatchCopStreamResponse::Recv` 入口检查，已进入底层 `client.Recv` 的操作不会被异步打断。后台轮询粒度约为 10ms，因此实际观察到取消可能略晚于 deadline。
- `SyncSender::send` 在 1024 容量耗尽时会阻塞；当前没有 `try_send`、发送超时或背压错误分支。
- `Instant::now() + timeout` 使用标准库时间加法；本文件未对极端大 `Duration` 的溢出单独处理。
- `Close` 忽略退出通知发送和线程 join 的错误。它通过 `Option::take` 保证重复调用安全；`Drop` 再次调用时不会重复发送或 join。
- 后台循环收到退出消息或退出 sender 断开即结束。租约 sender 断开本身不会结束循环；正常生命周期依赖 `done` 通道。

## 并发与资源生命周期

每个 `coprRPCHandler::new` 都创建一个后台线程，线程拥有两个 receiver，处理器拥有对应 sender 和 `JoinHandle`。新租约经容量 1024 的 `sync_channel` 进入线程；线程把租约保存在本地 `Vec` 中，取消或到期后通过 `retain` 移除。其扫描成本与同时存活租约数线性相关。

租约取消状态使用 `Arc<AtomicBool>`，并以 `SeqCst` 读写，因此不依赖互斥锁即可跨线程可见。文件导入了 `Mutex` 但当前未使用；不能据此推断存在受锁保护的共享集合。`BatchCopClient` 由 `&mut self` 驱动，单个流游标不支持并发 `Recv`；trait 只要求处理器 `Send`，没有要求 `Sync`。

显式 `Close` 先发送退出信号，再 `join` 等待线程结束。线程的 `recv_timeout` 最长等待 10ms，因而关闭通常至多等待该轮超时；退出信号也可让等待立即返回。若调用方忘记关闭，`Drop` 会执行同一清理逻辑。Region 错误流没有注册租约；正常流即使被调用方提前丢弃，其后台租约仍保留到 deadline 后的下一轮清理，因为没有流析构时主动 `Cancel` 的实现。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/mockcopr/rpc_copr.go`。两版保留了同一高层协议：默认构造器创建容量 1024 的租约通道和后台超时任务；旧 `HandleCopStream` panic；CmdCop 先检查 Region 上下文再派发；BatchCop 在正常流上安装超时、预取首包；Region 错误以流响应而非外层 RPC 错误返回；`Close` 通知后台任务退出。

Rust 为适配当前移植状态做了以下结构性变化：

- Go 使用 client-go 的 `testutils.RPCSession`、protobuf 请求/响应和 `tikvrpc.Lease`；Rust 在本 crate 定义 `RPCSession`、请求响应模型、`Lease` 与流包装。
- Go `HandleCmdCop` 按数值请求类型 switch，未知类型 panic；Rust 的 `RequestPayload` 枚举把 DAG、Analyze、Checksum 限定为封闭集合，由 `coprHandler::handle_request` 分发。
- Go BatchCop 派生 `context.WithCancel`，把 cancel 函数装入 lease，出错时取消上下文；Rust 没有上下文对象，lease 仅设置原子标志。因此 Rust 超时不能取消下游计算，只会令之后的包装层 `Recv` 报错。
- Go 的 BatchCop 数据客户端体现流接口；Rust 下游先计算完整 `Vec<BatchResponse>` 再按 offset 返回，语义更接近预计算序列。
- Go `Close` 只关闭 `done` 通道；Rust `Close` 还持有并 join 后台线程，并用 `Option` 实现幂等关闭。
- Go Region 错误首包直接填入 `OtherError`，其错误客户端继续返回错误；Rust 通过 `BatchCopClient::RegionError` 预取并重复生成同样的 `other_error`。独立 Rust 测试覆盖了此行为及 `Timeout == Duration::ZERO`。
- Go 方法带 `context.Context` 与 `kvrpcpb.Context` 参数；Rust 接口未移植这两个上下文，Region 校验只检查会话中的预设错误。

因此，本文件是 Go RPC mock 语义的局部移植，但并非 client-go 网络类型的一比一替代。尤其是取消传播、真正流式执行和完整请求上下文仍有明确差异。

## 扩展指南

- 新增普通 Coprocessor 请求种类时，优先扩展 `copr_handler.rs` 的 `RequestPayload` 与 `coprHandler::handle_request`，RPC 层通常无需新增分支；同时在对应独立 `*_test.rs` 中验证成功与错误响应形状。
- 新增或改变 BatchCop 行为时，需联合修改 `HandleBatchCop`、`BatchCopClient::Recv` 与 `copr_handler.rs::handleBatchCopRequest`，并在 `rpc_copr_test.rs` 增加独立测试。至少覆盖空批次、首包错误、流耗尽、Region 错误重复读取和超时后读取。
- 若要对齐 Go 的取消语义，应把可取消令牌传入 DAG 构建和执行循环，而不只是修改 `Lease::Recv` 检查；还要验证提前丢弃流是否及时释放后台租约与下游资源。
- 若调整租约机制，应评估 1024 容量阻塞、10ms 轮询误差、活动租约线性扫描和 `Close` join 行为，避免在 RPC 调用线程制造无限等待。
- 若让构造器内的 `self.session` 成为实际默认会话，需要先决定 trait 方法是否仍接受外部 `session`，避免两个读取器来源产生含糊行为。
- 不要恢复内嵌测试。仓库约束要求 Rust 源逻辑与测试分文件；本模块测试应继续放在 `rpc_copr_test.rs`，并由 `lib.rs` 的 `#[cfg(test)] #[path = "rpc_copr_test.rs"]` 接线。
- 保持 Go 兼容时应同步核对 `rpc_copr.go`，特别关注 Region 错误属于响应而非外层错误、BatchCop 首包预取和旧 CopStream panic 这三个外部可见契约。性能敏感修改还应核对是否继续全量预计算响应。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录列表显示 `rpc_copr.rs`、`rpc_copr.go`、`rpc_copr_test.rs` 等均已索引。
- RustCodeGraph `node --file pkg/store/mockstore/mockcopr/rpc_copr.rs --offset 1 --limit 500`：读取目标文件完整 304 行，核对 38 个符号、公开 API、超时循环和文件级引用关系。
- RustCodeGraph `query HandleBatchCop`、`query NewCoprRPCHandler`、`query check_stream_timeout_loop`：核对 Rust/Go 对应符号及位置；对 `HandleBatchCop`、trait 转发和下游 `handleBatchCopRequest` 执行的精确 `callers/callees` 查询未返回边，因此调用关系又由模块入口和直接引用搜索补证。
- RustCodeGraph `node` 读取 `pkg/store/mockstore/mockcopr/copr_handler.rs` 第 520 至 653 行：核对 `Response`、`BatchRequest`、`BatchResponse`、`coprHandler`、`handleBatchCopRequest`、`handle_request`、`drainRowsFromExecutor` 与 `mockBatchCopDataClient::Recv`。
- RustCodeGraph `node` 读取 `pkg/store/mockstore/mockcopr/lib.rs`：核对模块声明、公开再导出和独立测试接线；该目录不存在 `doc.go`。
- `pkg/store/mockstore/mockcopr/Cargo.toml`：核对 crate 名、`lib.rs` 入口、Go 包元数据及 optional/dev 依赖边界。
- `pkg/store/mockstore/mockcopr/rpc_copr.go`：逐段核对构造、三类 RPC 方法、Go context/lease 取消和关闭语义。
- `pkg/store/mockstore/mockcopr/rpc_copr_test.rs`：核对 Region 错误在首包及后续 `Recv` 中重复返回、外层调用成功以及零超时字段。
- `pkg/store/mockstore/mockcopr/executor_test.rs` 第 105 至 149 行：核对 `NewCoprRPCHandler` 的现有测试侧构造入口；该测试没有直接验证本文件的 RPC 数据路径。
- 未运行 Cargo 或代码测试：任务是纯文档分析，任务计划明确禁止运行 Cargo。交付结构通过任务指定的 11 章节命令验证；行为结论来自上述源码、图查询、Go 对照和独立测试的静态核验。
