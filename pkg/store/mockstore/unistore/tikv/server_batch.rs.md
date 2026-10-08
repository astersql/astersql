# `pkg/store/mockstore/unistore/tikv/server_batch.rs`

## 文件定位

本文件位于 UniStore 的内存 mock TiKV crate `astersql-store-mockstore-unistore-tikv` 中，说明对象是同目录的 [`server_batch.rs`](./server_batch.rs)。crate 入口 `pkg/store/mockstore/unistore/tikv/lib.rs` 通过 `pub mod server_batch` 公开此模块；`Cargo.toml` 的 `[package.metadata.porting]` 又将该 crate 对应到 Go 包 `pkg/store/mockstore/unistore/tikv`。它不是网络协议实现，而是把一组 Rust 内存命令并发转调到同 crate 的 `server::Server`，再组装为便于测试的批响应。

仓库搜索只发现测试代码调用 `BatchRequestHandler::dispatch_batch`，没有生产 Rust 调用方或 gRPC 流接线。因此当前事实是“可公开调用的 mock 批处理门面”，不能把它描述成已经替代 Go `Server.BatchCommands` 的线上流式入口。目标源文件没有条件编译项；crate 的大量内部路径依赖在 `Cargo.toml` 中仅声明于 `cfg(target_os = "windows")` 段，这是 crate 级构建边界，不改变本文件自身的控制流。

## 核心职责

文件承担四项职责：

1. 用 `BatchCommand` 表示 12 类事务 KV、Raw KV 请求及其完整参数。
2. 由 `handle_batch_request` 将每个枚举分支映射到相应的 `Server` 方法，并把 `RpcResponse<T>` 压缩成统一的 `BatchCommandResponse`。
3. 由 `BatchRequestHandler::dispatch_batch` 为每条 `(request_id, command)` 启动一个作用域线程，通过有界同步通道回收结果。
4. 由 `ResponseIdPair::append_to` 和 `collect_batch_response` 同步追加 ID 与响应，使两个向量在完成顺序下仍保持逐位置对应。

它不负责 protobuf 编解码、gRPC stream 的收发/取消、请求分批读取、响应分批发送，也不实现底层 Region 校验、MVCC 锁存取或 Raw KV 存储；这些语义分别属于 Go 传输层和 Rust `server.rs`/`mvcc`/`mock_region`。

## 主要符号

- `REQUEST_CHANNEL_SIZE: usize = 1024`：公开常量，但本文件和仓库内 Rust 调用点均未使用；不能据此声称存在请求通道。
- `RESPONSE_CHANNEL_SIZE: usize = 1024`：`dispatch_batch` 创建 `mpsc::sync_channel` 时采用的响应缓冲容量。
- `BatchCommand`：公开请求枚举。事务读包括 `Get`、`Scan`、`BatchGet`；事务写/锁操作包括 `PessimisticLock`、`PessimisticRollback`、`Prewrite`、`Commit`、`BatchRollback`、`ResolveLock`；Raw KV 包括 `RawGet`、`RawPut`、`RawDelete`。需要 Region/MVCC 语义的分支携带 `RpcContext`，Raw 分支不携带它。
- `BatchCommandResponse`：公开统一响应枚举。`Value` 表示点查（外层 `Option` 是响应种类，内层值可为空），`Pairs` 表示扫描/批量读，`Empty` 表示无载荷成功，`Error(String)` 表示被压缩成文本的键级或 Region 错误。
- `ResponseIdPair`：公开关联对象，字段 `request_id` 与 `response` 共同移动，避免异步完成后失去请求身份。
- `BatchResponse`：公开批结果，包含并行的 `request_ids` 与 `responses` 两个向量；正确性不变量是长度相同且同一下标属于同一请求。
- `ResponseIdPair::append_to`：以相同顺序分别向两个向量追加 ID 和响应，是上述对应关系的唯一写入原语。
- `BatchRequestHandler<'a>`：只借用一个 `&'a Server`。`new` 建立轻量包装，`dispatch_batch` 执行整批并等待所有发送端结束。
- `collect_batch_response`：crate 内可见的顺序收集器，也是独立单元测试的测试缝。
- `handle_batch_request`：公开的单命令分派函数。
- `unit_rpc`、`error`：私有响应归一化函数，分别处理无载荷 RPC 和错误文本选择。

## 执行流程

`BatchRequestHandler::dispatch_batch` 的流程如下：

1. 创建容量为 `RESPONSE_CHANNEL_SIZE` 的同步通道。
2. 进入 `std::thread::scope`，逐个消费传入的请求向量；每个请求克隆一个发送端，并借用同一个 `Server`。
3. 每个作用域线程调用 `handle_batch_request`，随后发送含原始 ID 的 `ResponseIdPair`。发送结果以 `let _ = ...` 丢弃。
4. 主线程完成派生后丢弃原始发送端，再由 `collect_batch_response(receiver)` 按通道到达顺序持续接收，直到所有线程持有的发送端均被释放。
5. 每个 pair 通过 `append_to` 同步追加到两个结果向量。`thread::scope` 返回前会等待作用域线程结束，因此函数返回的 `BatchResponse` 是整批结果，而非流式中间结果。

`handle_batch_request` 的分派映射为：`Get`→`kv_get`，`Scan`→`kv_scan`，`PessimisticLock`→`kv_pessimistic_lock`，`PessimisticRollback`→`kv_pessimistic_rollback`，`Prewrite`→`kv_prewrite`，`Commit`→`kv_commit`，`BatchGet`→`kv_batch_get`，`BatchRollback`→`kv_batch_rollback`，`ResolveLock`→`kv_resolve_lock`，三类 Raw 命令分别转到 `raw_get`、`raw_put`、`raw_delete`。读请求把成功载荷转换为 `Value`/`Pairs`；事务型无载荷操作通过 `unit_rpc` 判断错误；Raw 写删调用完成后直接返回 `Empty`。

## 数据与状态

本文件自身不持有可变业务状态。`BatchRequestHandler` 只借用 `Server`，每次调用临时创建通道、线程和 `BatchResponse`。实际状态位于 `server.rs` 的 `Server`：事务数据由共享 `MvccStore` 管理，Raw 数据是 `RwLock<BTreeMap<Vec<u8>, (Vec<u8>, Option<u64>)>>`，Region 路由由 `RegionManager` 管理。

关键数据约束如下：

- `request_id` 只用于关联，不参与排序，也不要求唯一；输出顺序是完成/到达顺序，不是输入顺序或 ID 顺序。
- `BatchResponse` 的两个字段公开，类型系统不能阻止外部代码构造长度不一致的值；本文件内只有 `append_to` 同步更新两者。
- `RawPut` 的 `ttl` 与 `now_ts` 原样传给 `Server::raw_put`，后者用饱和加法计算过期时间；`RawGet` 由 `now_ts` 过滤过期项。
- `RpcContext` 携带 Region、隔离级别及 resolved/committed lock 信息；具体校验和锁语义发生在 `Server` 下游，而不在批分派层复制。

## 依赖与调用关系

上游接线证据：`lib.rs` 公开模块；`main_test.rs::batch_handler_preserves_response_id_association` 构造 handler 并调用 `dispatch_batch`；`server_batch_test.rs::collected_responses_preserve_channel_arrival_order_like_go` 直接验证收集器。仓库搜索未发现生产 Rust 调用点，这一限制应在新增传输层之前保持可见。

内部调用链是 `dispatch_batch` → `handle_batch_request` → `Server::{kv_*,raw_*}`，同时 `dispatch_batch` → `collect_batch_response` → `ResponseIdPair::append_to`。RustCodeGraph 能识别 `dispatch_batch` 到 `collect_batch_response`、`collect_batch_response` 到 `append_to`，以及 `handle_batch_request` 到 `unit_rpc`/`error` 的边；其同名方法解析有误归属，故具体 `Server` 方法映射以上述源码 match 分支为准。

下游依赖来自 `crate::server::{RpcContext, Server, KeyError}`、`crate::mvcc::{KvPair, PessimisticLockRequest, PrewriteRequest}`、`crate::mock_region::RegionError` 和标准库 `std::sync::mpsc`/`std::thread`。`Cargo.toml` 表明这些模块同属 `astersql-store-mockstore-unistore-tikv` crate；MVCC 又是相邻路径 crate/模块边界的一部分，但本文件只通过 `Server` 调用它，不直接操作存储。

## 错误处理与边界

事务读分支以 `RpcResponse.value` 是否为 `Some` 判断成功；无值时调用 `error`。无载荷事务分支只有在 `key_error` 与 `region_error` 同时为空时返回 `Empty`。`error` 优先采用 `KeyError.message`，其次采用 `RegionError` 的 `Display` 文本，二者都缺失时产生 `"empty response"`。因此批层会丢失 `KeyError` 的 deadlock、locked、conflict、retryable、abort 等结构化字段；若键错误和 Region 错误同时出现，也只保留键错误消息。这与 `server.rs` 当前构造响应时通常二选一相符，但不是类型层强制的不变量。

Raw 命令没有 Region 错误通道；Raw 写删也没有可返回错误的接口。同步通道发送失败被忽略，正常路径中接收端一直存活到发送端耗尽，因此这主要是防御性处理。若工作线程在发送前 panic，作用域线程机制会在退出 scope 时传播 panic，而不会将其转换成 `BatchCommandResponse::Error`。

当前命令集合比 Go 对照少，未覆盖事务心跳、事务状态检查、cleanup、scan lock、GC、delete range、Raw batch/scan/delete-range、coprocessor 和 empty 等 Go 分支。该差异属于当前移植边界，调用方不能假定完整 TiKV BatchCommands 协议覆盖。

## 并发与资源生命周期

`dispatch_batch` 为每条请求创建一个操作系统作用域线程，没有线程池、并发上限、取消令牌或背压到请求入口。响应通道容量固定为 1024；超过容量时发送线程可能暂时阻塞，待主线程派生完整批次并开始收集后继续。大批次的主要风险是“一请求一线程”的栈与调度成本，而不是响应向量重排。

所有线程共享借用的 `&Server`，安全性依赖 `Server` 内部的 `Arc`、`RwLock`、`Mutex`、原子量以及 MVCC/Region 层的锁存器；作用域保证线程不会越过该借用生命周期。原始 sender 在派生完成后显式 drop，每个克隆 sender 在线程退出时释放；receiver 以通道关闭作为整批结束信号。返回前 scope 等待全部子线程，因此不存在本函数遗留的后台线程。

完成顺序具有非确定性。独立单元测试人为给出 `[20, 10]` 的 pair 序列，证明收集器不重排；集成式测试把实际并发结果按 ID 排序后断言关联内容，证明它只要求 ID/响应配对正确，不要求某种调度顺序。

## 与 Go 版本的对应关系

Go `server_batch.go` 的 `respIDPair.appendTo` 对应 Rust `ResponseIdPair::append_to`，`batchRequestHandler.handleRequest`/`Server.handleBatchRequest` 对应 Rust 的线程闭包/`handle_batch_request`，`respChanSize = 1024` 对应 `RESPONSE_CHANNEL_SIZE`。两版都允许子请求乱序完成，并依靠 ID 与响应一起传递来恢复关联。

差异同样重要：Go `Server.BatchCommands` 接收真正的双向 gRPC stream，`start` 建立取消上下文，`dispatchBatchRequest` 循环 `Recv`，`collectAndSendResponse` 把当前通道积压聚成多个批次并逐次 `Send`；Rust 接受一个已物化 `Vec`，只返回一个完整 `BatchResponse`，没有流、取消和 I/O 错误。Go 单请求处理失败时记录日志且不发送该响应，Rust 则把服务端业务错误折叠成 `Error(String)`；Rust 线程 panic 仍会传播。Go 分支覆盖范围也明显更广。

仓库内没有搜索到直接覆盖这些 Go batch 函数的 `*_test.go`；Rust 的直接证据来自 `server_batch_test.rs` 与 `main_test.rs`。因此文档只声明控制流对应关系，不宣称两版协议能力完全等价。

## 扩展指南

新增命令时，应同时修改 `BatchCommand` 和 `handle_batch_request` 的穷尽 match，并选择不会丢失必要语义的 `BatchCommandResponse` 表达；若下游返回新结构，不应为了复用 `Empty`/字符串错误而删减 Go 版本行为。同步扩展独立的 `server_batch_test.rs`，需要真实 `Server` 状态时扩展 `main_test.rs`，不要把测试嵌入生产源文件。

若要接近 Go 的流式协议，应在独立传输/适配层增加 protobuf 转换、Recv/Send 循环、取消和 I/O 错误策略，再复用单命令分派；不能仅把 `dispatch_batch` 命名成 stream handler。若要提高大批次可伸缩性，最可能修改 `BatchRequestHandler::dispatch_batch`，需明确并发上限、背压、公平性、panic 隔离及完成顺序约定，并增加超过 1024 条请求、慢请求与 panic/关闭场景测试。

若需要保留 TiKV 错误兼容性，应优先扩展 `BatchCommandResponse` 的结构化错误表示并检查 `unit_rpc`/`error`，同时验证 KeyError 与 RegionError 的优先级。修改 ID/响应存储方式时必须维持逐项关联不变量，并继续覆盖乱序完成。`REQUEST_CHANNEL_SIZE` 当前未接线；使用或删除前需先确认未来传输层设计，不能把它误当现有容量控制。

## 验证依据

- 源码：`pkg/store/mockstore/unistore/tikv/server_batch.rs`（全部 292 行），核对所有常量、类型、impl、函数、12 个命令分支及无条件编译事实。
- crate 边界：`pkg/store/mockstore/unistore/tikv/lib.rs` 的 `pub mod server_batch` 和独立测试模块声明；`pkg/store/mockstore/unistore/tikv/Cargo.toml` 的包名、lib 路径、Go porting 元数据及目标条件依赖。
- 下游实现：`pkg/store/mockstore/unistore/tikv/server.rs` 的 `RpcContext`、`KeyError`、`RpcResponse`、`Server` 及被调用的 `kv_*`/`raw_*` 方法；`mock_region.rs::RegionError` 的显示实现。
- Rust 测试：`pkg/store/mockstore/unistore/tikv/server_batch_test.rs::collected_responses_preserve_channel_arrival_order_like_go`；`pkg/store/mockstore/unistore/tikv/main_test.rs::batch_handler_preserves_response_id_association`。
- Go 对照：`pkg/store/mockstore/unistore/tikv/server_batch.go` 的 `Server.BatchCommands`、`batchRequestHandler::{start,handleRequest,dispatchBatchRequest,collectAndSendResponse}`、`Server.handleBatchRequest`；仓库范围未找到相关 Go `*_test.go`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query` 找到 `BatchRequestHandler`、`dispatch_batch`、`handle_batch_request`、`collect_batch_response`；`callees` 确认 `dispatch_batch`→`collect_batch_response`、`collect_batch_response`→`append_to`、`handle_batch_request`→`unit_rpc`/`error`。调用者和部分同名方法边缺失或误归属，已用精确仓库搜索与源码分支补证。
- 本任务只新增说明文档，不改变 Rust/Go/Cargo 行为，按计划不运行 Cargo；最终以固定 11 个二级标题的结构命令验证。
