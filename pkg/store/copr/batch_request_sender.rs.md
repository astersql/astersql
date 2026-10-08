# `pkg/store/copr/batch_request_sender.rs`

## 文件定位

本文件属于 `astersql-store-copr` crate（`pkg/store/copr/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/store/copr/lib.rs` 声明为 `batch_request_sender` 模块，并把其中的发送器、请求/响应、Region 元数据、退避和 RPC 抽象统一再导出。它是 Rust 侧 Batch Coprocessor 请求发送边界：上层应先把键范围整理为 `RegionInfo`，再通过 `RegionBatchRequestSender` 将一个 `BatchRequest` 发往 `RpcContext.address` 指定的 TiFlash/TiKV 节点。

需要区分实现状态与接线状态：发送器和失败重试决策已经实现，`pkg/executor/executor_failpoint_test.rs` 也直接执行了真实发送器失败路径；但仓库内 Rust 生产源码没有构造 `RegionBatchRequestSender` 或调用 `send_req_to_addr`。当前 Rust 的 `pkg/store/copr/mpp.rs` 只复用本文件的 `Backoffer`、错误和 Region 数据类型，尚未像 Go 的 `pkg/store/copr/batch_coprocessor.go`、`pkg/store/copr/mpp.go` 那样把该发送器接入生产发送主链。

## 核心职责

- 定义 Batch Cop 发送边界所需的数据模型：`RegionVerId`、`KeyRange(s)`、`RegionInfo`、`RpcContext`、`BatchRequest`、`BatchResponse` 与 `RpcResponse`。
- 用 `RpcClient` 隔离具体传输，用 `RegionFailureHandler` 隔离 Region 缓存失效/重载，使发送逻辑可由真实后端或测试替身驱动。
- `RegionBatchRequestSender::send_req_to_addr` 负责填充请求上下文、检查地址与父取消状态、调用 RPC、记录耗时，并把传输失败转换为“重建整批后重试”或终止错误。
- `RegionBatchRequestSender::on_send_fail_for_batch_regions` 负责取消、关停、存算分离拓扑和退避耗尽等分支；普通 TiFlash 拓扑下先通知 Region 缓存，再执行退避。
- 提供 Go 迁移兼容入口，如保留字段名的 `RegionInfo`、`toCoprocessorRegionInfo` 和 `NewRegionBatchRequestSender`。

## 主要符号

- `BatchError` / `BatchResult<T>`：发送路径统一错误域。`Cancelled`、`ShuttingDown`、`Transport`、`BackoffExhausted` 是本文件发送逻辑的主要终态；其余变体也供同 crate 的 coprocessor、store 和 network backend 共用。`Display` 给出稳定的面向日志/上层错误文本。
- `RegionVerId { id, conf_ver, version }`：Region 身份与 epoch 版本；实现 `Hash`/`Ord`，可作为缓存键并按版本比较。
- `KeyRange` 与 `KeyRanges`：表示半开键区间 `[start, end)` 及集合；`into_sorted` 只按 `start` 排序，不合并、去重或校验端点。
- `RegionInfo`：与 Go 模型对齐，含 `Region`、可选 `Meta`、`Ranges`、候选 `AllStores` 和 `PartitionIndex`。`to_coprocessor_region_info` 仅复制 Region 版本及 ranges；`AllStores`、`PartitionIndex` 不进入该线格式对象。
- `RpcContext`：单次发送目标，包含 Region 版本、地址以及可选 store/meta/peer。`send_req_to_addr` 将其中的 `meta`、`peer` 写入可变 `BatchRequest.context`。
- `RpcClient::send_request`：同步传输 trait，参数含目标地址、请求、超时和子 `CancellationToken`，返回一组流式/批量响应项 `RpcResponse`。
- `RegionFailureHandler::on_send_fail_for_batch_regions`：失败通知 trait。真实实现 `pkg/store/copr/region_cache.rs::RegionCache` 只处理带 `engine=tiflash` 标签的 store，并只向后端通知含 `Meta` 的 Region。
- `Backoffer`：以尝试次数为界的轻量退避状态。`backoff` 先处理取消，再记录错误文本并递增次数；只有 `attempts > max_attempts` 时返回 `BackoffExhausted`，因此 `max_attempts = 1` 允许第一次失败进入重试、第二次失败才耗尽。
- `CancellationToken`：`Arc<AtomicBool>` 包装，`cancel` 使用 `Release` 写，`is_cancelled` 使用 `Acquire` 读，可跨线程克隆共享。
- `RegionBatchRequestSender`：持有 RPC 客户端、失败处理器、执行信息开关、存算分离开关、关停标志、可选统计容器及最后一次 RPC 错误。
- `SendResult`：把响应、是否重试、子取消令牌和终止错误同时返回；私有构造器 `failed` 统一生成不可重试失败结果。
- `NewRegionBatchRequestSender`：Go 风格兼容构造器，调用 Rust 风格 `RegionBatchRequestSender::new` 并返回 `Box`。

## 执行流程

`send_req_to_addr` 的流程如下：

1. 从 `RpcContext.meta/peer` 重建 `request.context`；该修改发生在地址和取消检查之前。
2. 地址为空时立即返回 `BatchError::Transport("RPC context has no target address")`，不调用传输、不通知 Region 缓存，也不消耗退避次数。
3. 若 `backoffer.cancellation()` 已取消，立即返回 `Cancelled`。否则为本次 I/O 创建独立的子 `CancellationToken`；父级退避令牌不会在 I/O 失败时被取消，因此上层仍可重建整批。
4. 记录开始时间。若 failpoint `github.com/pingcap/tidb/pkg/store/copr/mockBatchCopResponseError` 生效，则直接生成 `OtherResponse`；否则调用 `RpcClient::send_request(address, request, timeout, child_token)`。
5. 当 `enable_collect_execution_info` 为真且 `stats` 已设置时，不论 RPC 成功或失败，都把 `(request.command, elapsed)` 追加到统计列表。
6. 成功时返回原 `RpcResponse`、`retry=false` 和仍可由调用方取消的本次子令牌。
7. 失败时先取消子令牌并把错误克隆到 `last_rpc_error`，随后调用 `on_send_fail_for_batch_regions`。若失败处理与退避成功，丢弃失败子令牌，返回 `retry=true`、空响应和新的未取消令牌；若处理失败，则通过 `SendResult::failed` 返回终止错误。

失败处理依次执行：原错误为 `Cancelled` 时直接终止；全局 `shutting_down` 为真时返回 `ShuttingDown`；非存算分离 TiFlash 拓扑调用缓存失败处理器且 `reload_region` 恒为 `true`；最后由 `Backoffer::backoff` 决定本轮是否仍可重试。

## 数据与状态

发送器的可变状态只有 `last_rpc_error` 和 `stats` 指向的统计列表。成功请求不会清空旧的 `last_rpc_error`，因此该字段表示“最近一次已观察到的 RPC 错误”，不能当作当前调用结果；当前结果必须读取 `SendResult`。`stats` 默认是 `None`，即使启用收集开关也必须由上层显式安装 `Arc<Mutex<RpcRuntimeStats>>` 才会记录。

`Backoffer` 独立保存 `max_attempts`、当前 `attempts`、父取消令牌和公开的字符串 `history`。本文件没有睡眠、指数延迟或抖动，名称中的 backoff 在这里主要表达重试预算与失败历史；真实时间策略若需要加入，应避免改变现有 `attempts > max_attempts` 边界。

`RpcResponse.responses` 是 `VecDeque<BatchResult<BatchResponse>>`，允许响应流中每项独立成功或失败；本文件只负责把整个队列交还上层，不解释 `BatchResponse.other_error` 或 `retry_regions`。`BatchResponse::encoded_size` 是数据、错误字符串字节数与 Region 标识内存尺寸的估算和，不是 protobuf/网络线格式的精确编码长度。

## 依赖与调用关系

crate 边界由 `pkg/store/copr/Cargo.toml` 确认。本文件直接使用标准库并调用 `fail::eval`；`fail = { version = "0.5.1", features = ["failpoints"] }` 是对应外部依赖。文件定义的类型被 `batch_coprocessor.rs`、`coprocessor.rs`、`region_cache.rs`、`network_backend.rs`、`store.rs` 和 `mpp.rs` 广泛复用，并由 `lib.rs` 再导出给其他 crate。

关键下游边为：`send_req_to_addr -> RpcClient::send_request`、`send_req_to_addr -> on_send_fail_for_batch_regions`、后者再到 `RegionFailureHandler::on_send_fail_for_batch_regions` 与 `Backoffer::backoff`。真实失败处理器是 `pkg/store/copr/region_cache.rs` 的 `RegionCache` 实现；它进一步调用 backend 的 `on_send_fail_tiflash`。直接 Rust 调用证据目前仅见 `pkg/executor/executor_failpoint_test.rs::batch_cop_response_failpoint_uses_the_real_sender_retry_path`。

Go 生产上游清晰存在：`pkg/store/copr/batch_coprocessor.go` 构造 sender 后发送 batch cop task，`pkg/store/copr/mpp.go` 也通过 sender 发送 MPP 相关请求。Rust 侧相同生产上游尚未接线，所以不能把这些 Go 调用边声称为 Rust 已运行路径。

## 错误处理与边界

- 空地址和父级预取消均在 RPC 前终止；它们不会设置 `last_rpc_error`，不会触发缓存回调，也不会进入退避历史。
- failpoint 错误与真实传输错误走完全相同的发送失败分支；测试据此验证 transport 被绕过、`reload_region=true`、重试标志和 `last_rpc_error`。
- 一旦 RPC 返回错误，本次子取消令牌先被置为取消。若允许整批重试，返回值改用新的默认令牌，防止上一次失败 I/O 的取消状态污染下一批。
- `Cancelled` 优先于关停检查；其他错误遇到关停时转为 `ShuttingDown`，且不通知 Region 缓存、不消耗退避次数。
- `disaggregated_tiflash=true` 时跳过 Region 缓存失败通知，但仍执行退避预算。这与独立计算节点拓扑不应按普通 TiFlash Region 副本做失效处理的边界一致。
- `Mutex` 统计锁若中毒会由 `expect("RPC statistics lock poisoned")` 触发 panic，而不是返回 `BatchError`；这是当前明确的失败策略。
- 本文件不检查成功 `RpcResponse` 内各项错误、`other_error` 或 `retry_regions`，也不处理 Region/lock error；这些属于响应消费层职责。

## 并发与资源生命周期

`RpcClient` 与 `RegionFailureHandler` 都要求 `Send + Sync` 并存放在 `Arc` 中，可由多个发送器共享。`shutting_down` 和所有 `CancellationToken` 使用原子标志：Acquire/Release 配对保证观察取消/关停状态时具备基本跨线程可见性。运行统计由 `Arc<Mutex<_>>` 串行追加；锁覆盖范围仅为一次 `push`。

`send_req_to_addr` 自身接收 `&mut self`、`&mut Backoffer` 和 `&mut BatchRequest`，因此同一个发送器实例、退避器或请求不能在安全 Rust 中被多个调用并发修改。RPC trait 当前是同步函数，没有在此文件中创建线程、异步任务或 channel。

成功返回的 `SendResult.cancellation` 属于本次 RPC，由调用方持有并可继续取消相关流；失败 I/O 的令牌已取消并被丢弃。传输对象和失败处理器在发送器销毁时由 `Arc` 自动释放；本文件没有显式 close。`SendResult::failed` 返回独立、未取消的默认令牌，它只是返回形状占位，错误字段才是终态依据。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/copr/batch_request_sender.go`。`RegionInfo` 字段、Region epoch/ranges 转换、`RegionBatchRequestSender`、构造器、`SendReqToAddr` 和失败处理顺序均对应 Go 实现。两边共同保持的关键语义包括：发送前写入 Region/peer context；RPC 失败后取消当前调用、记录错误、通知缓存并要求上层重建整批；普通 TiFlash 每次 I/O 失败都用 `reload=true`；取消和 TiDB 关停不继续重试；执行信息开关控制 RPC 耗时采集。

Rust 当前有若干显式适配差异：

- Go 嵌入 `tikv.RegionRequestSender` 并直接使用 client/cache/oracle；Rust 以 `RpcClient`、`RegionFailureHandler` trait 和原子关停标志注入依赖，没有 oracle 字段。
- Go 使用 context/RPC canceller 和返回的 `cancel func()`；Rust 使用克隆式 `CancellationToken`，并明确分离父退避取消与单次 I/O 子取消。
- Go 通过 `tikv.BoTiFlashRPC()` 执行真实退避策略；Rust `Backoffer` 仅计次、存历史且不睡眠，是行为模型而非完整 client-go backoff。
- Go 的 `SetContext` 可返回错误；Rust 对自有 `BatchRequest.context` 直接赋值，不存在对应编码失败分支。
- Go 生产路径已在 `batch_coprocessor.go` 与 `mpp.go` 使用发送器；Rust 生产路径尚未调用该发送器。这是当前迁移/接线差距，不应由本文件文档推断为已完成。

## 扩展指南

若接入 Rust 生产发送主链，应优先复用 `RegionBatchRequestSender::new`、`send_req_to_addr` 和现有 trait，不在调用方复制失败分支；同时明确谁安装 `stats`、谁消费 `RpcResponse` 项级错误、谁根据 `retry=true` 重建全部 Region/ranges。接线测试应放在独立 `*_test.rs`，不要内嵌到本文件；现有最直接样板是 `pkg/executor/executor_failpoint_test.rs`。

若扩展错误分类或重试规则，应同步检查 `BatchError::Display`、`Backoffer::backoff`、`RegionCache` 的 `RegionFailureHandler` 实现及上层匹配分支。改变取消/关停优先级、`reload_region=true` 或存算分离跳过缓存的规则会影响兼容性；改变重试计数边界可能产生额外 RPC 和延迟。对应独立测试至少覆盖：空地址、父取消、关停、存算分离、退避耗尽、统计开关、成功响应和真实 transport 错误。

若更换为异步 RPC 或增加并行发送，需重新设计 `&mut self`/`&mut Backoffer` 所表达的串行所有权，并评估统计锁竞争、子取消令牌传播以及 `last_rpc_error` 的竞态语义。若修改 `BatchResponse::encoded_size` 用于限流或内存核算，必须先确认所需的是 Rust 内存估算还是实际协议编码大小。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 `pkg/store/copr/batch_request_sender.rs` 已索引，共 511 行、47 个符号。
- RustCodeGraph 源码/符号查询：读取目标文件全部两段（1–260、255–511），查询 `RegionBatchRequestSender`、`send_req_to_addr`、`on_send_fail_for_batch_regions`、`NewRegionBatchRequestSender`；精确 callers/callees 查询未返回额外静态边，因此用仓库引用搜索确认实际接线情况。
- crate 与模块证据：`pkg/store/copr/Cargo.toml`、`pkg/store/copr/lib.rs`；前者确认 crate、`fail` 依赖和 Go package 元数据，后者确认模块声明、公开再导出及测试分文件组织。
- 直接实现证据：`pkg/store/copr/batch_request_sender.rs`；下游失败处理证据：`pkg/store/copr/region_cache.rs::RegionFailureHandler for RegionCache`；共享类型的生产使用证据：`pkg/store/copr/mpp.rs` 等同 crate 文件。
- Go 对照：`pkg/store/copr/batch_request_sender.go` 全文，以及其生产调用点 `pkg/store/copr/batch_coprocessor.go`、`pkg/store/copr/mpp.go`。
- Rust 独立测试：`pkg/executor/executor_failpoint_test.rs::batch_cop_response_failpoint_uses_the_real_sender_retry_path`，验证 failpoint 绕过 transport、调用失败处理器、设置 `retry` 和保存 `last_rpc_error`。同目录不存在同名独立 sender 测试；`pkg/store/copr/region_cache_test.rs` 另有失败处理器相关覆盖。
- 本任务是纯文档分析，按计划未运行 Cargo。结构校验要求目标文档存在且恰有十一个固定二级标题。
