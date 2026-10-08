# `pkg/store/mockstore/unistore/rpc.rs`

## 文件定位

`rpc.rs` 是 `astersql-store-mockstore-unistore` crate 的进程内 RPC 边界。crate 入口 `pkg/store/mockstore/unistore/lib.rs` 以 `pub mod rpc` 声明本模块并通过 `pub use rpc::*` 导出其 API；`pkg/store/mockstore/unistore/Cargo.toml` 则表明该 crate 直接依赖配置、server、tikv 三个拆分 crate，以及启用 failpoint 的 `fail`。

它不负责网络编解码，也不启动 gRPC 服务。调用方构造强类型的 `Request`，`RPCClient::send_request` 验证客户端状态和目标地址后，在同一进程内把请求转交给 `tikv::server::Server`、`Cluster` 或独立的 `RawHandler`，再用 `Response` 返回结果。`pkg/store/mockstore/unistore/mock.rs::New` 创建 server 和 cluster 后构造 `Arc<RPCClient>`；上层 `pkg/store/mockstore/mockstorage/embedded_rpc.rs::EmbeddedRpcStore` 将它用于 mock 事务读写，`pkg/store/mockstore/redirector.rs` 又把它适配为 `KvClient`。

因此，本文件是“上层 mock 存储请求”与“嵌入式 UniStore 实现”之间的协议适配和分派层，而不是 MVCC、Region 或 Coprocessor 算法本身的实现位置。

## 核心职责

1. 用 `CommandType`、`Request` 和 `Response` 描述嵌入式客户端支持的命令及强类型载荷，并由 `Request::command_type` 保持请求变体到命令类型的一一映射。
2. 由 `RPCClient::send_request` 提供统一同步入口：拒绝已关闭客户端、模拟短超时 failpoint、运行可选请求拦截器、校验地址对应的 store，最后调用 `dispatch`。
3. 由 `RPCClient::dispatch` 将事务/MVCC 请求交给 `Server::kv_*`，Raw KV 请求交给 `RawHandler`，Coprocessor/MPP/调试请求交给各自 server 或 cluster 接口。
4. 用 `MockStream<T>` 模拟 Cop、Batch Cop 和 MPP 的响应流；这里的流是预先收集到 `VecDeque` 的有限序列，不是网络背压流。
5. 管理共享客户端的测试钩子和生命周期：`RequestInterceptor` 可在真实分派前拒绝请求；请求 marker 以线程局部、客户端局部方式作用于闭包；`close`/`Drop` 停止 server 并在易失模式下清理目录。
6. 为 `DebugGetRegionProperties` 解码 Region 的 memcomparable 边界并扫描 MVCC 数据，计算 `mvcc.num_rows`。

本文件同时包含若干有意的 mock 行为：`MppAlive` 恒为 `true`，`StoreSafeTs` 恒为 `0`，`UnsafeDestroyRange` 直接成功，`close_addr` 不做任何工作；这些不应被解释为真实 TiKV 的完整实现。

## 主要符号

- `CommandType`：无载荷的命令分类枚举，共覆盖普通 KV、两阶段提交、悲观事务、Raw KV、Coprocessor、MPP、调试及占位命令。`RpcError::Unsupported` 携带它报告不支持的请求。
- `Request`：统一请求枚举。事务类变体通常携带 `RpcContext`；Raw 请求只携带键值；MPP 请求携带 store/task 标识和可选 payload。`Request::context()` 只返回当前明确列出的 Get/Scan/事务写、Cop/BatchCop 等变体的上下文；即使 `MvccGetByKey`、`MvccGetByStartTs`、`SplitRegion`、`Flush`、`BufferBatchGet` 自身也带 `context`，当前方法仍对它们返回 `None`，扩展调用方不能假设“有字段就一定可由 `context()` 取得”。
- `Request::command_type(&self) -> CommandType`：穷尽匹配全部 `Request` 变体，是日志、错误和测试按命令分类的稳定入口。
- `Response`：统一响应枚举。事务/server 路径保留 `RpcResponse<T>` 中的 region/key error；Raw 路径返回 `RawGetResponse` 或 `Vec<RawKvPair>`；流式路径返回 `MockStream`；若命令仅表示动作完成则用 `Empty`。
- `RpcError` 与 `Result<T>`：RPC 边界错误模型，区分 `Cancelled`、`Unsupported`、`Region`、`Server`、`Io`、`EndOfStream`。其 `Display` 当前直接输出 Debug 形式，没有额外错误链。
- `RequestInterceptor = Arc<dyn Fn(&Request) -> Result<()> + Send + Sync>`：共享的、可失败的请求前置检查器。`RPCClient::set_request_interceptor` 原子地以写锁替换它，`send_request` 在分派前克隆并调用。
- `REQUEST_MARKERS`：线程局部 `RefCell<HashMap<usize, u64>>`，以客户端地址作为键。`request_marker` 查询当前线程上的值；`with_request_marker` 支持嵌套，并借助局部 `Restore` 的 `Drop` 在正常返回或 panic 展开时恢复旧值。
- `RPCClient`：核心客户端，拥有 `Arc<Server>`、`Arc<Cluster>`、数据路径、独立 `RawHandler`、持久化标记、关闭原子位和拦截器锁。它可安全地放入 `Arc` 并被多个上层组件共享。
- `RPCClient::send_request` / `send_request_async`：同步入口及线程派生式异步包装。异步版本固定使用 2 秒超时，并在新 OS 线程完成后调用一次 callback。
- `RPCClient::dispatch`：私有的穷尽请求路由函数，是本文件行为的主体。
- `decode_bytes`：解码 TiDB memcomparable bytes。每组为 8 个数据字节加 1 个 marker；它验证总长度、marker、零填充及终止组，任何格式错误都返回 `RpcError::Server`。
- `MockStream<T>`：以 `VecDeque<T>` 保存预制响应；`recv` 每次弹出队首，耗尽后返回 `RpcError::EndOfStream`。

## 执行流程

典型创建与调用链如下：

1. `mock.rs::New` 选择或创建数据目录，构造配置，通过 `server::new_mock` 得到 server/region manager/mock PD，然后构造 `Cluster` 与 `Arc<RPCClient>`。
2. 上层通过 `EmbeddedRpcStore::send`、`KvClient for RPCClient`，或直接持有的 `Arc<RPCClient>` 调用 `send_request(address, request, timeout)`。
3. `send_request` 以 acquire 读取 `closed`；已关闭时立即返回 `Cancelled`。
4. 当超时小于 1 秒且 `unistoreRPCDeadlineExceeded` failpoint 命中时，返回 `RpcError::Server("Deadline is exceeded")`，不进入 server。
5. 方法从 `interceptor` 的读锁中克隆当前拦截器，释放锁后调用它；拦截器返回错误会短路请求。
6. `Server::get_store_id_by_address` 验证地址存在。返回的 store ID 本身不参与后续分派；失败被映射为 `RpcError::Region`。
7. `dispatch` 消费 `Request` 并构造匹配的 `Response`：
   - Get/Scan、Prewrite/Commit、悲观锁、事务状态、GC、DeleteRange 等调用 `Server::kv_*`；Prewrite、PessimisticLock 和 Flush 在调用 server 前以 start timestamp 和 region ID 执行 `Cluster::handle_delay`。
   - `BatchGet` 与 `BufferBatchGet` 当前共同调用 `Server::kv_batch_get`。
   - Raw Get/Put/Delete/Scan 等调用客户端自有的 `RawHandler`，与 MVCC store 隔离。
   - `Cop` 直接调用一次 `Server::coprocessor`；`CopStream` 把一次调用结果包装为单元素流；`BatchCop` 对输入逐个同步执行后组成流。
   - MPP 建连先取得全部 packet 再构造流；创建和取消任务调用 server 并把字符串错误映射为 `RpcError::Server`；存活检查恒真。
   - MVCC 调试查询和 Region 拆分转交 server；Region properties 则从 cluster 查 Region、解码边界、以 `MAX_SYSTEM_TS` 全范围扫描，再返回行数属性。
   - `StoreSafeTs`、`UnsafeDestroyRange` 是固定 mock 响应；`Empty` 是唯一明确返回 `Unsupported` 的变体。
8. 调用者按请求类型解包 `Response`；server 的 region/key 错误通常仍位于 `RpcResponse<T>` 内，而不是提升为外层 `RpcError`。

`send_request_async` 只是在新线程中调用上述同步链；它没有任务池、取消句柄或 join handle。`MockStream::recv` 也只是队列弹出，不会继续驱动 server。

## 数据与状态

`RPCClient` 的状态分成四类：

- 共享服务状态：`server: Arc<Server>` 和 `cluster: Arc<Cluster>`。事务、MVCC、Coprocessor、MPP 与 Region 元数据最终由它们管理；客户端只负责路由。
- 客户端私有 Raw 状态：`raw_handler: RawHandler`。相邻 `raw_handler.rs` 证明其底层是 `RwLock<BTreeMap<Vec<u8>, Vec<u8>>>`，因而 Raw 请求具有有序扫描和并发读/独占写语义，但不进入 MVCC。
- 生命周期状态：`path`、`persistent` 和 `closed: AtomicBool`。`persistent == false` 时首次成功关闭会删除 `path`；`closed` 只从 `false` 转为 `true`，使关闭和 Drop 幂等。
- 测试/观测状态：`interceptor: RwLock<Option<RequestInterceptor>>` 在客户端实例间隔离；`REQUEST_MARKERS` 在执行线程间隔离，并在同一线程内按客户端指针区分。

`Request` 和 `Response` 通过所有权传递可变长度载荷，避免 dispatch 中长期借用上层缓冲区。server 返回的 `RpcResponse<T>` 保留“外层调用成功但内部带 Region/Key 错误”的协议形态。`MockStream` 拥有全部响应，因此其生命周期独立于原始请求，但内存用量与结果总大小线性相关。

## 依赖与调用关系

上游直接证据：

- `pkg/store/mockstore/unistore/mock.rs::New` 调用 `RPCClient::new`，并返回 `Arc<RPCClient>`、PD facade 与 cluster。
- `pkg/store/mockstore/mockstorage/embedded_rpc.rs::EmbeddedRpcStore::send` 调用 `RPCClient::send_request`；同文件的 get/scan/rollback/commit 将 canonical mock 事务翻译成 `Request`。
- `pkg/store/mockstore/redirector.rs` 为 `RPCClient` 实现 `KvClient`，同步入口直接委托给本文件的方法；`ClientRedirector` 根据 TiKV/TiFlash/TiDB 目标选择 mock 或网络客户端。
- 精确搜索还显示 `pkg/store/mockstore/mockstorage/canonical_storage_test.rs`、`pkg/executor/test/loaddatatest/*` 使用拦截器、marker 或直接请求，说明这些测试钩子已进入更高层 SQL/事务测试链。

下游直接证据：

- `crate::tikv::server::Server`：地址校验、全部 `kv_*`、coprocessor、MPP、MVCC 调试、split region、stop 和 MVCC store 暴露。
- `crate::cluster::Cluster`：一次性事务延迟注入和 Region manager 查询。
- `crate::raw_handler::RawHandler`：独立 Raw KV 内存实现。
- `crate::tikv::mvcc` 与 `crate::tikv::mock_region`：构成请求/响应载荷的事务类型、锁、MVCC 信息和 Region。
- 标准库同步/资源设施：`Arc`、`RwLock`、`AtomicBool`、`thread_local!`、`std::thread::spawn`、`fs::remove_dir_all` 和 `VecDeque`。
- `fail` crate：只在本文件的 deadline 模拟点使用。

RustCodeGraph 将本文件标记为被 24 个文件使用，并能解析 160 个符号；精确 `callers` 命令在本次环境中挂起，因此上述具体调用边由索引的 “used by” 信息、精确符号查询和 `rg` 调用点共同核验，而没有把超时查询当作成功证据。

## 错误处理与边界

- 客户端关闭、短超时 failpoint 和不支持的 `Empty` 请求分别产生 `Cancelled`、`Server`、`Unsupported`。
- 地址不存在由 `get_store_id_by_address` 报错并映射为 `Region(String)`；MPP server 的字符串错误映射为 `Server(String)`；关闭时 server stop 和目录删除错误分别映射为 `Server` 与 `Io`。
- 大部分 KV 方法返回 `RpcResponse<T>`，Region/Key 错误保留在响应内部。调用者必须像 `embedded_rpc.rs::value` 一样检查这些字段，不能只检查外层 `Result`。
- `DebugGetRegionProperties` 在 Region 不存在时返回 `Region`；编码边界长度不是 9 的倍数、marker 非法、填充非零或没有终止组时返回 `Server`。空边界被视为无界并解码为空字节串。
- `MockStream::recv` 用 `EndOfStream` 表示正常耗尽；它没有区分 EOF 与失败响应，响应内部错误仍由元素本身表达。
- `interceptor` 和 `RawHandler` 的锁均以 `expect` 处理 poisoned lock，因此持锁 panic 后后续访问会再次 panic，而不是返回 `RpcError`。
- `close` 先以 `swap(true)` 标记关闭，再调用 `server.stop`。如果 stop 失败，客户端仍保持关闭且本次不会删除临时目录；后续 `close` 因幂等早退也不会重试清理。这是扩展生命周期逻辑时需要保留或显式修正的边界。
- `dispatch` 本身没有外部取消 token 或 deadline 检查；传入的 `timeout` 只参与 send 入口的特定 failpoint，且不会限制实际同步执行时间。

## 并发与资源生命周期

`RPCClient` 面向 `Arc` 共享。`closed` 使用 acquire 读取和 acq-rel 交换，确保首次关闭对后续请求可见；但“检查 closed”与进入 dispatch 之间没有全局互斥，因此与 close 竞态的已通过检查请求仍可能和 server stop 并行。`close_addr` 是进程内客户端的空操作，不管理真实连接。

拦截器更新受 `RwLock` 保护。`send_request` 在锁内只克隆 `Arc`，随后在锁外执行用户闭包，避免拦截器回调阻塞配置写锁或因回调重入造成同一锁死锁。Raw 数据的并发安全由 `RawHandler` 自身的读写锁提供；server/cluster 的内部并发约束属于各自模块。

`with_request_marker` 的 marker 只在当前线程生效。局部 `Restore` 在闭包结束或 panic 展开时恢复嵌套前的值；另一个线程即使持有同一 `RPCClient` 也看不到 marker。`rpc_test.rs::request_marker_is_client_local_nested_and_restored_after_panics` 同时验证了客户端隔离、嵌套恢复、panic 恢复和线程隔离。

`send_request_async` 每次调用创建一个非托管 OS 线程，并把 `Arc<RPCClient>` 保活至 callback 完成；API 不返回 join handle，调用者不能等待或取消线程。`MockStream` 则在构造时已经拥有所有响应，无后台任务、通道或锁。

`close` 负责显式停止 server 和条件删除目录；`Drop` 再调用一次 `close` 并忽略错误，保证忘记显式关闭时尽力回收。若仍有其他 `Arc<RPCClient>`，只有最后一个强引用释放时才触发 Drop；显式 `close` 会立即影响所有共享引用。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/rpc.go`。两版共同保留的主干语义包括：RPCClient 持有 server/cluster/path/raw handler/persistent/closed；异步发送包装同步发送；地址先解析；Prewrite/PessimisticLock/Flush 前调用 cluster delay；事务、Raw、Cop、MPP 与调试命令按类型分派；非持久模式关闭时清理目录；按地址关闭为空操作；Debug Region Properties 解码边界并报告 `mvcc.num_rows`。

Rust 版不是 Go 协议层的逐类型机械翻译，而是内部强类型模型，当前差异必须在扩展时显式评估：

- Go 接收 `context.Context` 和 `tikvrpc.Request`，处理 context 取消、附加请求上下文、响应 RegionError 修正以及大量 failpoint；Rust 入口没有 context 取消，只实现 deadline failpoint 和通用 `RequestInterceptor`。
- Go 有 send/response hook、TopSQL resource tag 检查、server-busy/epoch/事务结果/BatchCop/MPP 等多种 failpoint；Rust 本文件未实现这些行为，不能仅凭命令枚举宣称完全兼容。
- Go 的流响应带 gRPC 风格 client、lease cancel 和 timeout，并可能在 `Recv` 时产生错误；Rust `CopStream`、`BatchCop`、`MppStream` 都是预先物化的 `MockStream`。尤其 Rust `CopStream` 只有一个结果，BatchCop 顺序执行全部子请求。
- Go `CmdStoreSafeTS` 调 server，Rust `StoreSafeTs` 固定返回 0；Go `CmdBufferBatchGet` 调 `KvBufferBatchGet`，Rust 与普通 BatchGet 共用 `kv_batch_get`；两版都把 UnsafeDestroyRange 当作无需真实销毁的成功操作。
- Go `Close` 忽略目录删除错误且每次写 closed；Rust `close` 幂等并传播 stop/IO 错误，同时由 Drop 尽力调用。
- Rust 新增了 `RequestInterceptor` 和线程局部 request marker，用于 Rust 上层测试中的请求检查；对应行为不是 `rpc.go` 同名 API。

相关 Rust 独立测试位于 `pkg/store/mockstore/unistore/rpc_test.rs`，没有把测试内嵌到生产源文件。Go 侧本目录没有独立 `rpc_test.go`；Go 语义证据来自 `rpc.go` 本身以及使用 RPCClient 的相邻 Go 测试/实现。

## 扩展指南

新增命令时至少同步检查以下位置：

1. 在 `CommandType`、`Request`、`Request::command_type` 和 `Response` 中补齐类型；若请求携带上下文，明确决定是否加入 `Request::context`，不要沿用当前少数带 context 但返回 `None` 的遗漏而不加说明。
2. 在 `RPCClient::dispatch` 增加穷尽分支，并确认下游应是 `Server`、`Cluster` 还是独立 `RawHandler`；保留 `RpcResponse` 内部错误与外层 `RpcError` 的边界。
3. 若通过重定向或 canonical mock storage 暴露，更新 `pkg/store/mockstore/redirector.rs` 或 `pkg/store/mockstore/mockstorage/embedded_rpc.rs` 的适配和响应类型检查。
4. 与 `rpc.go` 对照请求的 failpoint、取消、流式首包、RegionError 和超时语义。确实不支持的差异应记录并测试，不应以恒定成功占位冒充完整实现。
5. 在独立的 `pkg/store/mockstore/unistore/rpc_test.rs` 增加最小回归测试；跨层行为可同步更新现有 `mockstorage/*_test.rs` 或调用该钩子的 executor 测试，仍不要把测试写进 `rpc.rs`。

修改生命周期时要特别测试：重复 close、stop 失败、目录删除失败、close 与在途请求竞态、最后一个 `Arc` 的 Drop。修改 marker/interceptor 时要覆盖嵌套、panic、跨客户端、跨线程以及 poisoned-lock 策略。

修改流式实现时的主要兼容风险是把当前 eager、有序、有限队列改成真正异步流后，首包时机、错误出现位置、内存占用和取消行为都会变化。修改 BatchCop/MPP 路由还需关注执行顺序和 store ID；修改 Region properties 解码则必须保持 memcomparable 分组、marker、填充和空边界规则。

## 验证依据

- RustCodeGraph 状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `rpc.rs` 被完整索引为 839 行、160 个符号，并标记为被 24 个文件使用。
- RustCodeGraph 精确节点/源码：`rpc.rs::RPCClient`（391 行）、`rpc.rs::send_request`（459 行）、`rpc.rs::send_request_async`（493 行）、`rpc.rs::decode_bytes`（791 行）、`rpc.rs::MockStream`（823 行）；另通过 `node --file` 阅读了目标文件全貌。
- RustCodeGraph 调用图限制：对 `send_request` 的精确 `callers` 查询连续 60 秒没有返回，已中止；随后使用精确文本调用点补证，没有伪造图结果。
- crate 与装配证据：`pkg/store/mockstore/unistore/Cargo.toml`、`pkg/store/mockstore/unistore/lib.rs`、`pkg/store/mockstore/unistore/mock.rs`。
- 上下游证据：`pkg/store/mockstore/mockstorage/embedded_rpc.rs`、`pkg/store/mockstore/redirector.rs`、`pkg/store/mockstore/unistore/raw_handler.rs`，以及目标文件引用的 `cluster`、`tikv::server`、`tikv::mvcc` 接口。
- Go 对照：`pkg/store/mockstore/unistore/rpc.go`（完整 612 行），重点核对 `SendRequest`、流处理、`handleDebugGetRegionProperties`、`Close` 与 mock stream。
- 测试证据：`pkg/store/mockstore/unistore/rpc_test.rs` 的 `debug_region_properties_reports_mvcc_row_count_like_go` 验证两条已提交 MVCC 记录得到行数 2；`request_marker_is_client_local_nested_and_restored_after_panics` 验证 marker 的客户端、嵌套、panic 和线程边界。
- 精确搜索证据：`rg` 定位 `RPCClient::new`、`send_request`、`set_request_interceptor`、`with_request_marker` 及主要 `Request` 变体的直接使用点，弥补本次 RustCodeGraph callers 查询挂起。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前运行固定章节结构检查，并人工复核本文只描述当前源码可证实的行为、明确列出 mock/占位与 Go 差异。
