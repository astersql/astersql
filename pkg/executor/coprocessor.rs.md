# `pkg/executor/coprocessor.rs`

## 文件定位

本文件属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/executor/lib.rs` 以 `pub mod coprocessor` 公开。它是 TiDB/AsterSQL 进程内的本地 Coprocessor DAG 处理器：将序列化 DAG 请求转换为物理执行器，拉取结果，再以 unary 或 stream 响应形式编码。

实现不直接依赖 protobuf、会话、权限、规划器或具体 chunk 类型，而是通过 `CoprocessorBackend`、`DagExecutor`、`CopChunk` 和 `CoprocessorStream` 四个 trait 定义边界。这使文件内保留了 Go 主流程，但实际类型适配与系统接线必须由其他模块提供。当前代码搜索和 RustCodeGraph 均未找到 `CoprocessorDAGHandler` 的 Rust 外部调用者，因此它是已公开但尚未证明接入 Rust RPC 主链的移植实现，不应解读为已在生产路径运行。

## 核心职责

- `copHandlerCtx` 把请求中的 `SourceStatement` 转换为 `TraceInfo`，同时注入 trace 与日志字段；无来源语句时原样返回上下文。
- `buildDAGExecutor` 完成 DAG 请求类型校验、解码、可选身份匹配与默认角色设置、时区和 statement flags 初始化，再从 ranges 与 executor specs 构建物理计划和执行器。
- `HandleRequest` 以 Open/Next/Close 模式拉取全部数据，记账序列化 chunk 内存，最后 marshal 一个 `SelectResponse`。
- `HandleStreamRequest` 每次 Next 后立即编码并发送帧；遇到空 chunk 仍调用发送路径，以保留 Go 版的终止帧语义。
- `buildChunk`、`encodeDefault` 与 `encodeChunk` 按 DAG 声明的编码类型和 `output_offsets` 投影输出列；Default 编码每 64 行切分一个 `TipbChunk`，Chunk 编码则把整个内部 chunk 编为一个 `TipbChunk`。
- 所有可恢复错误统一转成 `CoprocessorResponse.other_error`，但 stream 传输自身的发送失败通过 `Result` 向上传播。

## 主要符号

- 常量 `REQ_TYPE_DAG: i64 = 103` 是本处唯一支持的请求类型；`rowsPerChunk: usize = 64` 只控制 Default 编码的分块行数。
- `SourceStatement` / `TraceInfo` 保存 `connection_id` 与 `session_alias`；`CopHandlerContext` 要求上下文支持 trace 信息和日志字段的链式注入。
- `CoprocessorRequest<R>` 包含请求类型、DAG 字节、扫描 ranges 和可选来源语句。
- `DAGRequest<X>` 是解码后的请求模型，包含用户、时区、flags、执行器描述、输出列偏移、编码类型与执行摘要开关。`EncodeType::Unknown(i32)` 显式保留不支持的 protobuf 枚举值。
- `CopChunk` 提供重置、行数和按行列取 datum；`DagExecutor<C>` 提供 `open/new_cache_chunk/return_field_types/next/close`。
- `CoprocessorBackend` 是最大的生产边界，以关联类型连接 Context、Range、ExecutorSpec、Plan、Executor、Chunk、Datum 与 FieldType，并封装解码、权限、时区、计划、编码、marshal、内存记账和错误栈。
- `CoprocessorDAGHandler<B>` 持有可变后端 `sctx` 与最近一次解码的 `dagReq`。`NewCoprocessorDAGHandler` 只设置后端，`dagReq` 初始为 `None`。
- 公开入口为 `HandleRequest` 和 `HandleStreamRequest`；其余 handler 方法均为内部实现细节。
- `ExecutorExecutionSummary` 当前是空占位类型；开启收集时只会生成与 DAG executor 数相同的空摘要，不包含真实运行指标。

## 执行流程

1. 入口先调用 `copHandlerCtx`；只有 `source_statement` 存在时才克隆 alias 并注入两类追踪上下文。
2. `buildDAGExecutor` 要求 `request_type == REQ_TYPE_DAG`，然后通过 `decode_dag_request` 解码。
3. 若 DAG 带 `DagUser` 且权限管理器可用，先设置请求用户，再尝试 `match_identity`；仅匹配成功且允许无密码验证时，才设置认证用户与默认角色。匹配不成功本身不会在本文件中返回错误。
4. 根据名称和 offset 构造时区，将时区与 DAG flags 写入后端会话状态，再把完整 `DAGRequest` 克隆到 `self.dagReq`。
5. 用 request ranges 和 DAG executors 构建物理计划，注入 extra projection，然后构建可执行对象。
6. unary 路径 Open 执行器，循环 Reset/Next；非空结果通过 `buildChunk` 编码，对每个序列化 chunk 调用 `consume_statement_memory`，直到 Next 产生零行。随后 Close，并通过 `buildUnaryResponse` 一次性 marshal 全部 chunks。
7. stream 路径同样 Open 并循环 Reset/Next，但每个内部 chunk 都经 `buildResponseAndSendToStream` 立即发送；零行 chunk 发送完成后返回。
8. `buildChunk` 对 Default 编码逐行、逐 `output_offsets` 取 datum 并调用 `encode_value`，再由 `appendRow` 每 64 行切块；对 Chunk 编码先按 offsets 构造返回列类型，然后一次调用 `encode_chunk`。

## 数据与状态

`CoprocessorDAGHandler` 是有状态的：`sctx` 在构建执行器时可被修改（请求用户、认证用户、活动角色、时区、flags、内存计账），`dagReq` 则保存当前/最近请求的解码结果，为后续编码取得 `encode_type`、`output_offsets`、`executors` 和摘要开关。因为入口需要 `&mut self`，单个 handler 不能在无外部同步下并发处理多个请求。

`DAGRequest` 在赋值给 `dagReq` 时整体克隆；这是为了让后续构建计划与编码都能继续借用请求信息，代价是 executors、offsets 和字符串的克隆。unary 路径还会把所有 `TipbChunk` 留在 `total_chunks` 中直到整个响应 marshal；stream 路径没有此累积，也没有调用 `consume_statement_memory`。

`output_offsets` 是列投影的唯一来源。`encodeDefault` 和 `encodeChunk` 都会将其直接转为 `usize` 并索引 field types/chunk，本文件不做边界检查；其合法性是解码或计划构建边界必须保持的前置不变量。

## 依赖与调用关系

- 模块上游：`pkg/executor/lib.rs` 公开此模块。RustCodeGraph 对 `pkg/executor/coprocessor.rs:252:function:HandleRequest` 的 callers 查询返回空集，全库 `rg` 也只在本文件找到 Rust `CoprocessorDAGHandler`/`NewCoprocessorDAGHandler`，所以当前没有可证实的 Rust 生产调用边。
- Go 主链：`pkg/server/rpc_server.go` 的 Coprocessor 与 CoprocessorStream RPC 会创建 `executor.NewCoprocessorDAGHandler(se)`，分别调用 `HandleRequest` 和 `HandleStreamRequest`；这只能证明 Go 版的系统位置。
- Rust RPC 边界：`pkg/server/rpc_server.rs` 使用另一组 `CoprocessorExecutor`/`CoprocessorRequest`/`CoprocessorResponse` 抽象，其测试 `pkg/server/rpc_server_test.rs` 用 `TestExecutor` 验证 RPC 会话生命周期，但未实例化本文件的 handler，因而不是本文件的行为测试。
- 文件内下游：公开入口调用 `CopHandlerContext`、`CoprocessorBackend`、`DagExecutor`、`CopChunk` 和 `CoprocessorStream` 的方法；具体 protobuf 与数据库类型被抽象掉，文件本身唯一直接外部依赖是 `std::fmt::Display`。
- RustCodeGraph 的宽泛 explore 查询识别出 `HandleRequest`/`HandleStreamRequest -> copHandlerCtx/reset/num_rows/open/new_cache_chunk/return_field_types/next` 等文件内边；精确 `callees` 对该 Rust 符号返回空集，说明当前索引不能完整表达这些泛型 trait 调用，因而不以空集推断“无下游”。

## 错误处理与边界

`CoprocessorBackend::Error: Display` 是统一错误类型。请求类型不是 103、DAG 解码失败、时区无效、计划/执行器构建失败、Open/Next/Close 失败、datum 编码失败、响应 marshal 失败，均在相应边界被捕获。unary 入口总是返回 `CoprocessorResponse`，并通过 `error_stack` 把错误放入 `other_error`；stream 入口通常也发送错误响应，但发送本身失败会成为函数的 `Err`。

`buildChunk`、`buildUnaryResponse`、`encodeChunk` 和 `encodeDefault` 都假定 `buildDAGExecutor` 已成功写入 `dagReq`；违反调用顺序会由 `expect("DAG request must be built first")` panic。`output_offsets` 越界也可能 panic。这些是内部契约而非可恢复错误，新后端必须在 decode/build 阶段验证。

需特别注意资源清理边界：unary 只在正常 Next 结束后调用 `close`，Next 或编码中途失败会直接返回；stream 路径完全没有调用 `close`。这与当前 Go 对照文件一致，但不能据此假设所有 `DagExecutor` 实现都无需清理；实现需要具有自管理或外层保底语义。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁或内部通道。同步执行的边界由调用者和 trait 实现决定。`HandleStreamRequest` 接收 `&mut S`，每帧顺序调用 `send`，当前代码没有并行发送、缓冲队列或显式背压策略；`send` 不返回前不会继续 Next，因此同步 trait 边界自然形成顺序背压。

unary 执行器的理想生命周期是 build -> open -> 多次 next -> close，但如上所述，错误早退不会 close。stream 是 build -> open -> 多次 next/send -> return，本文件无 close 阶段。chunk 缓存只创建一次并在每次 Next 前 reset，避免每批重新分配容器；unary 的序列化结果则持续累积到响应完成。

`sctx` 和 `dagReq` 使 handler 本身不适合跨请求并发共享；外层应为每个请求创建独立 handler，这也是 `pkg/server/rpc_server.go` 中 Go 路径的做法。Rust 主链当前尚无该接线证据。

## 与 Go 版本的对应关系

Rust 文件直接对应 `pkg/executor/coprocessor.go`，名称和流程高度一致：`copHandlerCtx`、`CoprocessorDAGHandler`、`NewCoprocessorDAGHandler`、`HandleRequest`、`HandleStreamRequest`、`buildDAGExecutor`、`buildChunk`、两种 encode 和 `appendRow` 均可逐段对照。Rust 以 trait 方法替代 Go 的具体 API：

- Go `proto.Unmarshal` / `proto.Marshal` 对应 backend 的 decode/marshal 方法。
- Go `privilege.GetPrivilegeManager`、`MatchIdentity`、`GetDefaultRoles` 对应 backend 权限方法组。
- Go `timeutil.ConstructTimeZone`、`StmtCtx.InitFromPBFlagAndTz` 对应 `construct_time_zone` 与 `set_time_zone_and_statement_flags`。
- Go `core.NewPBPlanBuilder(...).Build`、`core.InjectExtraProjection`、`newExecutorBuilder(...).build` 对应三个计划/执行器 backend 边界。
- Go `exec.Open/Next/Close`、`TryNewCacheChunk`、`RetFieldTypes` 对应 `DagExecutor` trait。
- Go `codec.EncodeValue` 会通过 statement error context 处理编码错误；Rust 将这一语义整体交给 `CoprocessorBackend::encode_value`，因而 backend 实现必须保留时区与 error-context 行为才能达到完整对齐。

已保留的关键语义包括：只支持 DAG 类型、权限管理器不存在时跳过认证填充、extra projection 注入、unary 内存记账、Default 每 64 行分块、Chunk 编码单块输出、空执行摘要占位、stream 终止空 chunk 和 `OtherError` 转换。差异是 Rust 把生产类型抽象掉，而且当前没有 Go `pkg/server/rpc_server.go` 那样的真实调用接线。

## 扩展指南

- 接入生产 Rust RPC 时，应在独立模块实现 `CoprocessorBackend`、`DagExecutor`、`CopChunk`、`CopHandlerContext` 与 stream 适配，再让 `pkg/server/rpc_server.rs` 的 `CoprocessorExecutor` 委托本 handler；不应把 protobuf/会话具体逻辑再次硬编码进本文件。
- 新增 encode type 时修改 `EncodeType`、`buildChunk` 及 backend 编码边界，并与 Go `buildChunk` 保持枚举值、投影列与错误文本一致。
- 改变 Default 分块时同步检查 `rowsPerChunk`、`appendRow` 与 Go 常量；这会改变响应帧边界、内存峰值和客户端观察到的 chunk 数。
- 增加真实 execution summary 时修改 `ExecutorExecutionSummary`、`DagExecutor`/后端指标接口和 `buildUnaryResponse`，不能只填充空结构。
- 改进执行器清理时，首先明确与 Go 的兼容目标，然后为 unary Next/编码失败和 stream 成功/失败各增独立测试，避免 close 错误覆盖原始错误。
- 建议新建同目录独立 `coprocessor_test.rs`（并在测试模块装配处引入），不要把测试写入生产文件。最小覆盖应包括：非 DAG 错误，解码/时区/计划失败，权限管理器缺失和身份匹配分支，两种编码，63/64/65 行分块，offset 投影，摘要数量，unary 内存记账，stream 终止帧和发送失败，Open/Next/Close/marshal 错误。
- 与 RPC 层集成时还应扩展 `pkg/server/rpc_server_test.rs`，用真实适配而非 `TestExecutor` 证明请求类型转换、会话状态和响应编码已连通。

## 验证依据

- Rust 源码：`pkg/executor/coprocessor.rs` 全部 525 行，重点符号为 `copHandlerCtx`、`CoprocessorBackend`、`CoprocessorDAGHandler`、`HandleRequest`、`HandleStreamRequest`、`buildDAGExecutor`、`buildChunk`、`encodeDefault`、`encodeChunk` 和 `appendRow`。
- crate 与模块：`pkg/executor/Cargo.toml` 确认 crate 名为 `astersql-executor` 且 library 入口为 `lib.rs`；`pkg/executor/lib.rs:86` 以 `pub mod coprocessor` 公开模块。
- Go 对照：`pkg/executor/coprocessor.go` 全部 305 行；`pkg/server/rpc_server.go:131-132` 和 `:215-216` 提供 Go stream/unary 生产调用边。
- Rust 相邻边界：`pkg/server/rpc_server.rs` 定义独立 `CoprocessorExecutor` 抽象；`pkg/server/rpc_server_test.rs` 只测试该 RPC 抽象和会话生命周期，不直接覆盖本 handler。全库搜索未找到本文件的独立 Rust 测试或外部实例化。
- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/executor/coprocessor.rs` 报告该文件有 64 个符号。`query` 定位 Rust/Go 同名入口；`node --file` 用于阅读 Rust 与 Go 全文；`explore` 找到 handler 到上下文、executor 和 chunk 方法的内部边；精确 callers 对 Rust `HandleRequest` 返回空集，精确 callees 因泛型 trait 调用未给出边，本文档因而同时使用索引源码与 `rg` 接线搜索交叉核验。
- 本任务是纯文档分析，按计划不运行 Cargo；验收以十一个固定章节的结构检查与上述代码证据复核为准。
