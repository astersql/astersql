# `pkg/store/mockstore/mockcopr/cop_handler_dag.rs`

## 文件定位

本文件属于 `astersql-store-mockstore-mockcopr` crate，是 mockstore 中模拟 TiKV Coprocessor DAG 请求的组装与响应层。crate 入口 `pkg/store/mockstore/mockcopr/lib.rs` 将本模块声明为私有模块后再公开重导出其符号；`pkg/store/mockstore/mockcopr/Cargo.toml` 的 `[package.metadata.porting]` 将该 crate 对应到 Go 包 `pkg/store/mockstore/mockcopr`。

单请求的真实入口链为 `rpc_copr.rs::coprRPCHandler::HandleCmdCop` → `copr_handler.rs::coprHandler::handle_request` → 本文件的 `coprHandler::handleCopDAGRequest`。本文件不负责 RPC 超时线程、KV 数据的具体扫描或各算子的逐行算法；这些分别位于 `rpc_copr.rs`、`executor.rs`、`aggregate.rs` 和 `topn.rs`。批量请求由 `copr_handler.rs::handleBatchCopRequest` 直接复用 `buildDAGExecutor`，但使用 `drainRowsFromExecutor` 形成单个 `Chunk`，不经过 `handleCopDAGRequest` 的响应组装流程。

## 核心职责

1. 校验 `Request`，把 `DagRequest`、键范围和 `start_ts` 固化到 `dagContext`，并构造执行器链（`buildDAGExecutor`、`buildDAG`、`buildExec`）。
2. 将 `TableScan`/`IndexScan` 作为叶子，将 `Selection`、聚合、`TopN`、`Limit` 依次包在上游执行器之外，再通过 `executor::Next` 排空结果（`handleCopDAGRequest`）。
3. 根据 `output_offsets` 选择输出列，将行编码并按最多 64 行分块；同时带回扫描计数、可选执行摘要和分类后的错误（`fillUpData4SelectResponse`、`encodeDefault`、`appendRow`、`buildResp`）。
4. 提供 Go 接口形状所需的辅助类型和函数，包括求值列元数据、键范围辅助、表达式列偏移收集、mock gRPC 流及固定错误客户端。

它是可执行的 mock 实现，但不是 Go 版本的完整等价替代。当前 Rust 协议类型是 crate 内的简化结构，并未解析 protobuf；`EncodeType::Chunk` 也暂时复用 Default 编码。

## 主要符号

- `dummySlice: &[u8]`、`rowsPerChunk: usize = 64`：行编码初始缓冲和响应分块上限。
- `dagContext`：持有克隆后的 `DagRequest`、`Vec<KeyRange>`、MVCC 可见性时间戳 `startTS` 及 `evalContext`。构建执行器期间可修改，执行和编码阶段由当前请求独占。
- `FieldType`、`ColumnInfo`、`evalContext`：简化的列类型与列信息。`setColumnInfo` 同步生成 `fieldTps` 和列 ID；`decodeRelatedColumnVals` 只按偏移复制已经解码的 `Datum`，偏移越界返回 `CopError::ColumnOffset`。
- `EvalSessionContext`、`flagsAndTzToSessionContext`：仅保存 flags 和时区秒偏移的会话上下文；不是 Go `sessionctx.Context` 的完整实现。
- `coprHandler::handleCopDAGRequest`：DAG 单请求总控入口，构建执行器、循环 `Next`、读取 `Counts`/`ExecDetails`、编码数据并生成最终 `Response`。
- `buildDAGExecutor`：拒绝 Region 注入错误、空范围、非 DAG 载荷及空执行器列表，然后创建上下文和根执行器。
- `buildDAG`、`buildDAGForTiFlash`、`buildExec`：执行器拓扑构建 API。线性 `executors` 按列表顺序自底向上连接；`buildDAGForTiFlash` 当前只是调用 `buildDAG`，没有 Go 版递归 `RootExecutor.Child` 语义。
- `buildTableScan`、`buildIndexScan`、`buildSelection`、`buildHashAgg`、`buildStreamAgg`、`buildTopN`：把简化规范转换为 `executor.rs`/`aggregate.rs` 的具体执行器。扫描器共享克隆后的 `Arc<dyn KvReader>`。
- `extractKVRanges`：复制范围，降序时反转列表。当前构建扫描器并未调用它；扫描方向直接传给扫描执行器。
- `initSelectResponse`、`fillUpData4SelectResponse`、`constructRespSchema`、`encodeDefault`、`encodeChunk`：响应骨架、编码分发和 schema 辅助。`constructRespSchema` 当前没有进入编码路径。
- `buildResp`、`toPBError`、`PbError`：响应和简易 PB 风格错误转换。`buildResp` 将 Locked、Region 和其他错误放入不同字段；`toPBError` 当前固定 `code = 1`，且未被本文件主流程调用。
- `reverseKVRanges`、`appendRow`、`maxStartKey`、`minEndKey`、`isDuplicated`、`extractOffsetsInExpr`、`fieldTypeFromPBColumn`：范围、分块、表达式和列类型辅助函数。
- `mockClientStream`/`MockGRPCClientStream`：保持 Go mock gRPC 客户端流的接口形状，所有通信方法均为空操作或成功返回。
- `mockBathCopErrClient::Recv`：每次返回一个无数据且 `other_error` 固定为所持 `CopError` 文本的 `BatchResponse`；不会推进或耗尽状态。

## 执行流程

单请求流程如下：

1. `HandleCmdCop` 校验 `RPCSession` 后创建 `coprHandler`；`handle_request` 依据 `RequestPayload::Dag` 分发到 `handleCopDAGRequest`。
2. `buildDAGExecutor` 先返回已注入的 `region_error`，再要求范围非空、载荷确为 DAG、执行器列表非空；随后克隆请求状态到 `dagContext`。
3. `buildDAG` 遍历 `DagRequest.executors`。第一个扫描规范创建叶子；后续规范由 `buildExec` 创建并通过 `SetSrcExec` 连接原有根。扫描若出现在已有 `source` 之后会被拒绝，因此扫描必须位于列表首端。反之，本文件没有显式禁止非扫描算子成为第一个元素，错误可能延后到该算子读取缺失 source 时暴露。
4. `handleCopDAGRequest` 重复调用根执行器的 `Next()`：`Some(row)` 被缓存，`None` 表示结束，首个错误停止排空。之后无论成功与否都读取 `Counts()`；只有 `collect_execution_summaries` 为真时才读取 `ExecDetails()`。
5. `initSelectResponse` 写入 counts，并可先把执行错误写入 `other_error`。没有执行错误时，`fillUpData4SelectResponse` 按编码类型处理所有缓存行。
6. `encodeDefault` 对每行按 `output_offsets` 取值，调用 `Datum::encode` 追加字节，同时保存结构化的 selected row；`appendRow` 保证每个 `Chunk` 最多 64 行。不存在的输出偏移会被静默忽略，而不是报错。
7. `buildResp` 附加执行摘要，并再次按照 `CopError` 变体设置 `locked`、`region_error` 或 `other_error`。编码失败时会保留已创建的响应和摘要并把编码错误分类写入响应。

批量路径只共享步骤 2～3：`handleBatchCopRequest` 对每个请求调用 `buildDAGExecutor`，然后由 `drainRowsFromExecutor` 排空执行器。后者对越界 `output_offsets` 返回 `ColumnOffset`，且把所有行放入一个块；因此批量与单请求在越界和分块行为上并不完全一致。

## 数据与状态

- 请求状态：`buildDAGExecutor` 克隆 `DagRequest` 和范围，避免执行期借用调用者；`startTS` 传入扫描器，由 `MemoryReader::scan` 等读取器据此筛选可见版本。
- 执行器状态：根对象为 `Box<dyn executor>`。每个非叶子通过 `SetSrcExec(Option<Box<dyn executor>>)` 获得所有权，形成单所有者链。计数和执行摘要由具体执行器维护，本文件仅在排空后读取。
- 行状态：`handleCopDAGRequest` 先把全部 `Row` 收集到 `Vec<Row>`，再编码；内存占用随结果集线性增长，不是边读边发。
- 分块状态：`appendRow` 在没有块或末块已有 64 行时创建新块，同时向 `rows_data` 和 `rows` 追加同一行的两种表示。空结果不会产生空块。
- 求值状态：`evalContext` 能保存列元信息，但当前 `buildTableScan`/`buildIndexScan` 并未调用 `setColumnInfo`，所以 schema 相关字段默认可为空；实际简化执行器直接处理 `Row = Vec<Datum>`。
- mock 流状态：`mockClientStream` 无内部状态；`mockBathCopErrClient` 只有一个错误字段，其 `Recv` 可无限重复相同响应。

## 依赖与调用关系

上游调用者（RustCodeGraph 与源码核对）：

- `copr_handler.rs::handle_request` 调用 `handleCopDAGRequest` 处理单个 DAG 载荷。
- `copr_handler.rs::handleBatchCopRequest` 调用 `buildDAGExecutor`，证明 DAG 构建逻辑也服务于 BatchCop。
- `rpc_copr.rs::HandleCmdCop` 是单请求 RPC 侧入口，通过 `handle_request` 间接进入本文件。
- `cop_handler_dag_test.rs` 直接调用 `buildDAGExecutor` 和 `mockBathCopErrClient::Recv`；`main_test.rs` 直接调用 `handleCopDAGRequest`。

下游依赖：

- `copr_handler.rs` 提供请求/响应协议、`CopError`、`Datum`/`Expr`、`KvReader` 与 `coprHandler`。
- `executor.rs` 提供 `executor` trait 以及扫描、过滤、TopN、Limit 实现；本文件依赖其 `Next`、`SetSrcExec`、`Counts`、`ExecDetails` 合约。
- `aggregate.rs` 提供 Hash/Stream 聚合执行器。
- Rust 标准库提供 `Box` 动态分派、`Vec` 所有权、切片检查和字典序 `min`/`max`。

Cargo 清单虽声明了 expression、kv、sessionctx、types、rowcodec 等大量 optional 端口依赖，但本文件当前没有直接引用外部 crate；它只通过同 crate 模块和标准库工作。这与 Go 文件直接依赖 protobuf、TiDB expression/sessionctx/types/codec 形成鲜明差异。

## 错误处理与边界

- 请求前置错误：注入的 Region 错误优先于其他检查；空范围、错误载荷、空执行器和扫描位置错误均返回 `CopError::InvalidRequest`。
- 执行错误：`Next` 的首个错误终止读取。`initSelectResponse` 和 `buildResp` 都可能写 `other_error`，最终文本保持该错误的 `Display` 结果；Locked 和 Region 还会分别进入专用字段。
- 编码边界：单请求 `encodeDefault` 对越界输出列静默跳过；批量 `drainRowsFromExecutor` 则显式返回 `ColumnOffset`。扩展时应先决定是否保留这一现状，避免无意改变测试兼容性。
- `decodeRelatedColumnVals` 同时检查输入 values 与目标 row 的偏移；任何一侧越界都返回 `ColumnOffset`，已写入的较早列不会回滚。
- `minEndKey` 把空 end 视为正无穷；两端都为空时返回空。`maxStartKey` 使用字节向量的字典序。不同于 Go 的 `extractKVRanges`，Rust 当前不验证 `start < end`，也不按 Region 原始边界裁剪范围。
- `extractOffsetsInExpr` 支持本 crate 当前所有 `Expr` 变体并去重；遇到重复列只保留第一次出现的位置。表达式树通过递归遍历，极深的人造树可能消耗调用栈。
- `appendRow` 中的 `expect` 由同一分支刚插入块的不变量保证；正常输入不会触发 panic。
- 未知执行器类型在 Rust 中无法出现，因为 `ExecutorSpec` 是封闭枚举；这与 Go 版运行时 default 分支返回“不支持”错误不同。

## 并发与资源生命周期

本文件自身不创建线程、任务、锁、通道或事务。`coprHandler.reader` 是共享的 `Arc<dyn KvReader>`，构造扫描器时克隆 `Arc`，因此 reader 的生命周期至少覆盖执行器链；`KvReader: Send + Sync` 允许其被 RPC 层并发共享。具体读取并发安全由 reader 实现承担。

单次调用中 `dagContext`、执行器树、缓存行和响应都由当前栈帧独占；执行器通过 `Box` 链式拥有 source，函数返回后按 Rust RAII 自动释放。该处理函数同步排空整个结果集，没有取消信号或背压。BatchCop 的租约监控线程和 channel 位于 `rpc_copr.rs`，不是本文件的生命周期职责；本文件提供的两个 mock client 也不管理网络连接。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/store/mockstore/mockcopr/cop_handler_dag.go`。Rust 保留了大多数同名入口、执行器种类、64 行分块常量、响应组装辅助和 mock 客户端外形，但存在以下已验证差异：

- Go 从 protobuf 解码 `tipb.DAGRequest`，构造真实时区/statement context、除法精度和警告；Rust 直接接收强类型简化 `DagRequest`，`EvalSessionContext` 只有 flags/时区偏移，主流程也未使用它。
- Go 在 `Executors` 为空时递归处理 `RootExecutor` 的 TiFlash 树；Rust 明确拒绝空列表，`buildDAGForTiFlash` 只是线性 `buildDAG` 的别名。
- Go 扫描构造会建立 rowcodec decoder、列信息、隔离级别、resolved locks、start-ts fallback、range counts，并调用会校验和裁剪 Region 边界的 `extractKVRanges`；Rust 只把 reader、范围、startTS、方向和 unique 标记交给简化扫描器。
- Go `getAggInfo` 会转换表达式、创建聚合函数并收集相关列；Rust 只克隆已是内部类型的 `AggCall`/`Expr`，且 `buildHashAgg`/`buildStreamAgg` 直接调用构造器。
- Go Chunk 编码会按响应 schema 用 chunk codec 真正编码；Rust `encodeChunk` 当前调用 `encodeDefault`。Rust `Response` 也直接保存结构化 rows，不进行 protobuf marshal。
- Go `buildResp` 主要把 SelectResponse 序列化到 `Response.Data`，锁错误另填 `Locked`；Rust 直接在一个 `Response` 中保存 chunks、摘要和三类错误字段。
- Go `appendRow` 依赖全局 row index 每 64 行开块；Rust 根据末块当前行数开块，正常顺序追加时效果等价，并额外保存结构化行。
- Go `mockBathCopErrClient::Recv` 使用嵌入的 `errorpb.Error.Message`；Rust 使用 `CopError::to_string()`。独立测试确认两次调用都返回相同错误响应。

因此，维护时应以 Go 文件作为行为目标和差距来源，但不能假设 Go 侧细节已经在 Rust 中实现。

## 扩展指南

- 新增执行器：先扩展 `copr_handler.rs::ExecutorSpec`，在 `buildExec` 增加构造分支，并在相应独立测试文件新增“拓扑构建 + 正常结果 + source 缺失/非法位置”覆盖；具体算法应放入 `executor.rs`、`aggregate.rs` 或新的生产模块，不要把测试或完整算法堆入本文件。
- 补齐 TiFlash 树：需要先扩充请求模型以表达 root/child，再实现区别于线性列表的递归构建；同步对照 Go `buildDAGForTiFlash`，并在 `cop_handler_dag_test.rs` 添加多层树与子节点错误测试。
- 补齐列/schema/Chunk：应让扫描构造更新 `evalContext`，使 `constructRespSchema` 能区分聚合结果 schema，再实现真实 Chunk codec。必须测试 AVG 双列、聚合 group-by、空结果、恰好 64/65 行及非法 output offset；当前 Default/Chunk 同形的兼容行为会改变，风险较高。
- 调整范围语义：若接入 Region 边界裁剪，应把 `extractKVRanges` 真正接入扫描构造，并覆盖空上界、相离范围、非法范围、降序多范围。不要只反转列表而忽略 reader 自身的 descending 行序。
- 改进错误模型：统一单请求和批请求的 output offset 行为，并决定 Select 内错误与顶层 Region/Locked 错误的映射；需同步 `cop_handler_dag_test.rs`、`copr_handler_test.rs` 和 `rpc_copr_test.rs`。
- 性能：当前“全部收集后编码”对大结果集有双份行/字节内存。若改为流式分块，应保留 `Counts`、执行摘要只在排空后完整可用的约束，并验证中途错误时已产生块如何处理。
- Go 对齐修改必须保持本仓库规则：Rust 生产逻辑与测试分文件，修改本文件时同步同目录独立测试；不要删除 PingCAP license，真正可用的 Rust 文件保留顶部 `// Copyright 2026 AsterSQL.`。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/store/mockstore/mockcopr` 确认目标模块、Go 对照和独立测试均在图中。
- RustCodeGraph `node --file pkg/store/mockstore/mockcopr/cop_handler_dag.rs`：完整读取 517 行，核对 52 个索引符号及主流程、辅助函数和 mock 类型。
- RustCodeGraph `explore "handleCopDAGRequest coprHandler pkg/store/mockstore/mockcopr"`：确认 Rust 调用边 `copr_handler.rs::handle_request` → `cop_handler_dag.rs::handleCopDAGRequest`，并确认 Go 侧的 DAG 构建、编码和辅助调用关系。对常见名称执行单独 `callers/callees` 未返回可消歧输出，因此没有用其空结果推导结论。
- 已读 Rust 直接证据：`pkg/store/mockstore/mockcopr/lib.rs`、`copr_handler.rs`、`rpc_copr.rs`、`cop_handler_dag_test.rs`、`copr_handler_test.rs`、`main_test.rs`。
- 已读边界与依赖：`pkg/store/mockstore/mockcopr/Cargo.toml`；该清单确认 crate 名、`lib.rs` 入口、Go 包映射及 optional 端口依赖。
- 已读 Go 对照：`pkg/store/mockstore/mockcopr/cop_handler_dag.go` 全部 803 行，逐项核对 DAG 构建、扫描配置、求值上下文、编码、错误和范围辅助语义。该目录没有独立的 `cop_handler_dag_test.go`；Rust 独立测试及同 crate 的 Go 测试面是可用的最近证据。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前执行固定章节结构命令，并人工检查本文只陈述上述源码和调用图可支持的事实。
