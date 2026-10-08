# `pkg/store/mockstore/unistore/cophandler/mpp.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-unistore-cophandler` crate；crate 边界由同目录 `Cargo.toml` 定义，`lib.rs` 通过 `pub mod mpp` 将其公开。它位于 UniStore mock coprocessor 的 MPP 层，使用 `cop_handler.rs` 中的请求、执行器树、行和值模型，并把非 Exchange 算子交给 `mpp_exec.rs::execute_executor`。因此它不是网络协议服务端，而是一个进程内、以 Rust 标准库同步通道模拟任务间 Exchange 的实现。

仓库范围的精确引用搜索表明，当前生产 Rust 文件没有调用 `handle_mpp_dag_request`、`MppTaskHandler` 或 `ExchangerTunnel`；直接消费者是 `mpp_test.rs` 与 `cop_handler_test.rs`。这意味着本文件目前已由 crate 公开并具备可执行逻辑，但生产 RPC/调度接线尚未在 Rust 侧得到验证，不能等同于 Go `mpp.go` 已接入的完整 MPP 请求链。

文件没有 feature gate 或其他条件编译项。`MPP_VERSION = 1` 和 `TUNNEL_BUFFER = 10` 是公开常量；前者在当前 Rust 仓库中没有其他引用，后者决定每条内存隧道的背压容量。

## 核心职责

1. 用 `TaskMeta`、`TunnelKey`、`EstablishRequest` 和 `MppDataPacket` 表示最小任务身份、隧道身份、建连请求及交换载荷。
2. 用 `ExchangerTunnel` 封装容量为 10 的 `sync_channel`，提供连接等待、单次激活、发送、接收和关闭语义。
3. 用 `MppTaskHandler` 维护任务侧隧道表，拒绝重复键，完成连接握手，并在取消时关闭全部隧道。
4. 用 `MppExecBuilder` 递归解释 `Executor`：`ExchangeSender` 负责 Broadcast、PassThrough 或 Hash 分发，`ExchangeReceiver` 合并源任务数据，其余算子委托 `mpp_exec.rs::execute_executor`。
5. 用 `handle_mpp_dag_request` 把带根执行器的 DAG 请求转成 `Response`；无法匹配该形态时回退 `cop_handler.rs::handle_cop_request`。

该实现的职责边界是“本地 mock 行集 + 进程内隧道”。它不解析 protobuf task meta，不创建远程 RPC，不管理 Go 版的 gather ID、任务状态或 RPC client，也不实现 Go `mppExecBuilder` 的全部协议构建细节。

## 主要符号

- `MPP_VERSION: i64`：当前值为 1，表示模块声明的 MPP 协议版本；当前没有 Rust 调用方读取它。
- `TUNNEL_BUFFER: usize`：同步通道容量 10，与 Go `buildMPPExchangeSender` 创建 `DataCh` 时的容量一致。
- `TaskMeta { task_id, address }`：任务标识。当前隧道查找只使用 `task_id`，`address` 被保存但不参与本文件的执行或路由。
- `MppDataPacket { data, error }`：一包已解码的 `Vec<Row>` 或字符串错误；它不是 Go `mpp.MPPDataPacket` 的 protobuf 字节载荷。
- `TunnelKey { sender_task_id, receiver_task_id }`：Rust 隧道表的复合键，允许同一 handler 区分不同发送方到接收方的通道。
- `EstablishRequest { sender, receiver }`：建连查找所需的两端任务元数据。
- `ExchangerTunnel`：核心通道对象。公开方法为 `new`、`connect`、`wait_connected`、`send`、`recv_chunk`、`activate`、`close`；内部状态由两个 `Mutex`、一个 `Condvar` 和两个 `AtomicBool` 组成。
- `MppTaskHandler`：任务级隧道注册表。`register_tunnel`、`establish_conn`、`tunnel`、`cancel`、`cancelled` 分别处理注册、单次激活建连、查找、整体取消和取消查询。
- `MppContext<'a>`：把当前 `TaskMeta` 与借用的 `MppTaskHandler` 绑定给一次执行。
- `MppExecBuilder<'a>`：持有 `KvReader`、键范围、MVCC `start_ts` 和可选 MPP 上下文；`build_and_execute` 是公开递归入口，`send` 与 `receive` 是内部 Exchange 实现。
- `handle_mpp_dag_request`：本文件最高层公开入口，构造 builder、执行 DAG 根节点并组装响应。
- `row_hash`：内部 FNV-1a 64 位哈希。它依次调用 `Datum::encode`，使全部分区表达式结果共同决定 Hash Exchange 槽位。

## 执行流程

隧道生命周期从 `ExchangerTunnel::new` 开始：创建一个容量为 `TUNNEL_BUFFER` 的同步通道，初始状态为未连接、未激活、未关闭。发送端调用 `send` 时先进入 `wait_connected`；后者在条件变量上循环等待，直到 `connect` 设置连接位，或 `close` 设置关闭位并唤醒等待者。连接建立后才向通道写入数据。接收端 `recv_chunk` 阻塞读取；数据包携带 `error` 时将其转成 `CopError::Tunnel`，已关闭且通道断开时返回 `Ok(None)`。

任务侧先以 `(sender_task_id, receiver_task_id)` 调用 `MppTaskHandler::register_tunnel`。`establish_conn` 从 `EstablishRequest` 重建同一个键，查到隧道后执行 `activate`；原子 compare-and-exchange 保证每条隧道只能成功激活一次，然后 `connect` 完成握手。重复注册、找不到键或重复激活都返回 `CopError::Tunnel`。

`MppExecBuilder::build_and_execute` 的顺序如下：

1. 若上下文中的 handler 已取消，立即返回 `CopError::Cancelled`。
2. 根节点为 `ExchangeReceiver` 时，`receive` 逐个源 task ID 查找 `(source, current_task)` 隧道，主动 `connect`，持续 `recv_chunk` 直到关闭，并按 `sources` 顺序拼接全部行。
3. 根节点为 `ExchangeSender` 时，先递归执行 child，再调用 `send`。成功后把最终行集的克隆追加到 `ExecutionOutput::intermediate`，同时保留 `output.rows`。
4. 其他节点直接交给 `mpp_exec.rs::execute_executor`，后者负责扫描、过滤、Limit、TopN、投影、Expand、聚合和 Join 等本地算子。

`MppExecBuilder::send` 只选择 `sender_task_id == context.task.task_id` 的隧道。Broadcast 向每条隧道发送完整行集；PassThrough 只使用收集结果中的第一条隧道；Hash 先求每行所有 `partition_keys` 的表达式结果，再以 `row_hash % tunnel_count` 分桶。每条被使用的隧道只发送一个包，随后立即关闭。没有 MPP 上下文或没有发送隧道时，该方法直接成功且不传输数据。

`handle_mpp_dag_request` 只对 `RequestPayload::Dag` 且 `root` 存在的请求走上述路径。其他请求调用普通 coprocessor 入口。成功时把行按 64 行切成 `Chunk`，并透传 range counts、NDV、执行摘要和 scan detail；错误时只设置 `Response::other_error`。

## 数据与状态

隧道数据是已经物化、已经解码的 `Vec<Row>`。Hash 分区以 `Expr::eval` 的结果而不是整行作为键；`mpp_test.rs::hash_exchange_uses_only_partition_keys_like_go` 用八行相同第一列、不同第二列的数据验证相同分区键不会被拆到两条隧道。`row_hash` 在每个 `Datum` 编码中包含类型标签与载荷，因此 Rust 当前的分区一致性以 `cop_handler.rs::Datum::encode` 为准。

`MppTaskHandler::tunnels` 是整个 handler 的可变注册表，键不可重复，但条目在成功接收、关闭或取消后不会移除。`cancelled` 是单向状态；`cancel` 后没有恢复 API。`ExchangerTunnel::active` 同样是单向状态；关闭后不能重开或再次激活。

`MppExecBuilder` 只借用 reader、ranges 和 handler，自身不拥有长期资源。`start_ts` 传给普通执行器以限定 MVCC 可见性。`context: None` 时普通执行器仍可运行，而 ExchangeSender 静默跳过发送；ExchangeReceiver 则明确返回 `Unsupported("exchange receiver without MPP context")`，两者的无上下文行为并不对称。

## 依赖与调用关系

上游入口在 crate 层是 `lib.rs::mpp`。RustCodeGraph 对目标文件识别出 36 个符号，并确认关键内部边：`ExchangerTunnel::send -> wait_connected`，`MppTaskHandler::establish_conn -> activate/connect`，`MppTaskHandler::cancel -> close`，`MppExecBuilder::build_and_execute -> cancelled/send/receive/execute_executor`，`MppExecBuilder::send -> Expr::eval/row_hash/ExchangerTunnel::send/close`，`receive -> tunnel/connect/recv_chunk`。RustCodeGraph 未找到 `handle_mpp_dag_request` 的外部调用者；全仓精确引用搜索也只找到目标文件本身。

主要下游依赖如下：

- `cop_handler.rs`：提供 `Request`、`RequestPayload`、`DagRequest`、`Executor`、`ExchangeType`、`Expr`、`Datum`、`Row`、`KvReader`、`Response`、`CopError` 与普通请求回退入口。
- `mpp_exec.rs`：提供 `ExecutionOutput` 和 `execute_executor`。Exchange 只在本文件特殊处理，其余执行树逻辑由该文件承担。
- Rust 标准库：`HashMap` 保存隧道，`Arc` 共享隧道，`Mutex` 保护 sender、receiver 与注册表，`Condvar` 实现建连等待，`AtomicBool` 保存激活、关闭与取消位，`sync_channel` 提供有界背压。

`Cargo.toml` 没有为本文件声明额外第三方依赖；crate 的大量工作区依赖均为 optional，而本文件直接使用的是同 crate 模块和标准库。`lib.rs` 以独立 `mpp_test.rs` 挂载 MPP 测试，符合测试不内嵌生产文件的约束。

## 错误处理与边界

可恢复的协议/执行错误统一使用 `CopError`：关闭或通道断开通常映射为 `Cancelled`，包内错误、重复注册、重复激活和隧道不存在映射为 `Tunnel(String)`，接收端缺少上下文映射为 `Unsupported`，表达式求值与普通执行器错误使用原错误向上传播。最高层 `handle_mpp_dag_request` 将错误字符串放入 `Response::other_error`，不会 panic。

锁中毒使用 `expect`，因此持锁线程 panic 会使后续操作 panic，而不是生成 `CopError`。`send` 在同步通道写满时会阻塞并持有 sender mutex；此时另一个线程调用 `close` 也需要该 mutex，不能保证立即打断满缓冲发送。现有取消回归只覆盖“连接前等待被关闭唤醒”，没有覆盖“通道已连接但缓冲已满”的取消情形。

还需注意以下当前行为：空隧道集合使发送静默成功；PassThrough 依赖 `HashMap` 迭代所得的“第一条”隧道，目标选择没有稳定顺序保证；Hash 只在隧道非空后取模，因此不会除零；`partition_keys` 为空时所有行的哈希相同；接收按源列表串行读取，如果较早源没有关闭，后续源即使已有数据也不会被消费；`MppDataPacket.error` 存在发送模型但本文件的 `send` 只构造 `error: None`。

## 并发与资源生命周期

`Arc<ExchangerTunnel>` 允许 handler、执行端和接收端共享同一隧道。`connected` 的布尔谓词与条件变量共用 mutex，`wait_connected` 使用循环抵抗虚假唤醒；`close` 设置 Release 关闭位、丢弃唯一保存在对象中的 sender，并 `notify_all`，等待者用 Acquire 读取关闭位。`active` 的 AcqRel compare-and-exchange 提供一次性占用保证。

`receiver: Mutex<Receiver<_>>` 使同一隧道的接收操作串行化；`sender: Mutex<Option<SyncSender<_>>>` 使发送与关闭互斥。`MppTaskHandler::cancel` 持有隧道表锁并逐条关闭，期间注册、查找和建连都会等待。取消位先于关闭动作发布，因此新的 `build_and_execute` 会拒绝启动；但 `register_tunnel` 和 `establish_conn` 自身不检查取消位，调用方若在取消后直接操作它们，仍可能注册或激活隧道。

正常发送路径在最后一包后调用 `close`，接收端在耗尽通道后得到 `None`。对象没有 `Drop` 定制、后台线程或异步运行时；资源释放依赖 `Arc` 引用计数和标准通道端点析构。`cop_handler_test.rs::TestExchSenderExecNextReturnsWhenCtxCanceledBeforeTunnelConnected` 以线程验证关闭能在一秒内唤醒尚未连接的等待者，并验证 handler 取消位。

## 与 Go 版本的对应关系

直接对照文件是同目录 `mpp.go`，运行期 Exchange 还依赖 `mpp_exec.go`。名称上的对应关系是 `MppExecBuilder` 对 `mppExecBuilder`、`MppTaskHandler` 对 `MPPTaskHandler`、`ExchangerTunnel` 对同名 Go 类型、`handle_mpp_dag_request` 对 `HandleMPPDAGReq`。

已对齐的核心意图包括：ExchangeSender 先执行 child；Hash 只依据配置的分区键；相同键必须落到同一 tunnel；每条发送通道容量为 10；任务 handler 注册并在连接到来时激活隧道；接收端合并多个上游的数据。Rust 独立测试 `hash_exchange_uses_only_partition_keys_like_go` 固化了分区键语义，Rust/Go 同名取消回归共同要求连接前取消不能永久阻塞。

Rust 不是 Go 实现的逐字段完整复刻，重要差异必须保留在扩展判断中：

- Go 处理 protobuf `tipb.Executor`、编码 task meta、TiKV RPC 流、timezone/session context、gather ID 和任务状态；Rust 使用本地 enum、`Vec<Row>` 与进程内通道。
- Go `mppExecBuilder` 可构建扫描、Join、聚合等多类具体 executor；Rust builder 只拦截 Exchange，其他算子统一委托 `execute_executor`。
- Go handler 以目标 task ID 保存 tunnel，校验 gather ID，并在建连找不到任务时最多重试 10 次；Rust 使用发送/接收复合键，立即报告找不到，且没有 gather ID 字段。
- Go receiver 为每个源启动 worker 并通过 RPC 收流；Rust `receive` 按源顺序同步读取本地通道。
- Go Hash 使用带字段类型信息的 `codec.HashChunkRow`；Rust使用自定义 `Datum::encode` + FNV-1a。当前测试证明“相等分区键同槽”，未证明跨 Go/Rust 的字节级槽位一致。
- Go `HandleMPPDAGReq` 通过流式 tunnel 返回数据并返回空 cop response；Rust入口把最终行放入普通 `Response.chunks`，ExchangeSender 还保留本地 `output.rows`。
- Go 非 Hash 发送路径会遍历全部 tunnels；Rust显式区分 Broadcast（全部）和 PassThrough（首条）。因此不能仅凭枚举名称宣称两端的 PassThrough 路由完全等价。

## 扩展指南

新增协议或生产接线时，首先决定边界属于 `mpp.rs` 的隧道/Exchange，还是 `mpp_exec.rs` 的普通算子。新增 Exchange 类型应修改 `cop_handler.rs::ExchangeType` 和 `MppExecBuilder::send`，并在独立 `mpp_test.rs` 增加多隧道、空输入和错误传播测试；新增普通算子应优先接入 `mpp_exec.rs::execute_executor`，不要在本文件复制执行逻辑。

若接入真实任务调度/RPC，需要围绕 `handle_mpp_dag_request`、`TaskMeta`、`EstablishRequest` 和 `MppDataPacket` 设计明确的编码边界，并补齐生产调用者。不要假定当前 `address` 或 `MPP_VERSION` 已生效。若追求 Go 兼容，还需逐项决定 gather ID 校验、建连重试、任务状态、远程取消、流式多包、错误包和 Hash 编码规则，而不是仅复用当前本地类型名称。

并发修改需重点验证：重复 establish、cancel 与 establish/register 竞争、关闭前后 send/recv、满缓冲取消、多个接收者、源隧道永久不关闭，以及锁中毒策略。PassThrough 若要求确定性目标，应显式保存有序目标或按 task ID 选择，不应依赖 `HashMap` 顺序。若隧道条目需要长期运行，应增加清理策略，避免 handler 的 map 永久保留已关闭隧道。

测试应继续放在独立文件。首选扩展 `mpp_test.rs` 验证路由和 handler 状态；涉及连接前取消语义时同步 `cop_handler_test.rs` 的 Rust 回归，并参考 Go `cop_handler_test.go::TestExchSenderExecNextReturnsWhenCtxCanceledBeforeTunnelConnected`。涉及 Go 语义对齐时，还应对照 `mpp.go::buildMPPExchangeSender`、`MPPTaskHandler`、`HandleEstablishConn` 以及 `mpp_exec.go` 的 `exchSenderExec`/`exchRecvExec`。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被识别为 436 行、36 个符号。
- RustCodeGraph `node --file pkg/store/mockstore/unistore/cophandler/mpp.rs`：通读目标源文件并核对全部常量、结构体、方法、公开入口与内部哈希函数。
- RustCodeGraph `query`：查询 `ExchangerTunnel`、`MppTaskHandler`、`MppExecBuilder`、`handle_mpp_dag_request`、`row_hash`，确认 Rust/Go 对应符号和测试引用。
- RustCodeGraph 调用边：确认 `send -> wait_connected`、`establish_conn -> activate/connect`、`cancel -> close`、`build_and_execute -> send/receive/execute_executor`、`send -> Expr::eval/row_hash`、`receive -> tunnel/connect/recv_chunk`；精确 caller 查询未找到 `handle_mpp_dag_request` 的外部调用方。
- 读取 `pkg/store/mockstore/unistore/cophandler/Cargo.toml` 与 `lib.rs`：核对 crate 名称、Go 包迁移元数据、模块公开方式、独立测试挂载和依赖边界；目录中不存在 `doc.go`。
- 读取 `cop_handler.rs` 的 `Datum`、`ExchangeType`、`Executor`、请求/响应和 `CopError` 定义，以及 `mpp_exec.rs::ExecutionOutput`/`execute_executor`：核对数据模型、错误传播和下游执行边。
- 读取 `mpp_test.rs::hash_exchange_uses_only_partition_keys_like_go` 与 `cop_handler_test.rs::TestExchSenderExecNextReturnsWhenCtxCanceledBeforeTunnelConnected`：核对相同分区键同槽、建连前关闭唤醒及 handler 取消行为。
- 读取 Go `mpp.go` 的 builder、`HandleMPPDAGReq`、`MPPTaskHandler`、`ExchangerTunnel`，`mpp_exec.go` 的 Exchange 发送/接收逻辑，以及 `cop_handler_test.go` 的 MPP 冒烟与取消回归：核对迁移意图及已知差异。
- 全仓 `rg` 精确引用检查：排除目标文件和两个 Rust 测试后，没有找到上述 Rust MPP 公共入口/类型的调用，故文档将其生产接线状态明确记为尚未验证。

本任务只做静态分析和文档结构验证，按计划不运行 Cargo，也不以编译或运行测试作为本次完成证据。
