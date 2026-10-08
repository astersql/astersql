# `pkg/store/mockstore/mockcopr/copr_handler.rs`

## 文件定位

本文件是 `astersql-store-mockstore-mockcopr` crate 的核心协议模型与请求分发层。crate 入口 [`lib.rs`](lib.rs) 将其声明为私有模块 `copr_handler`，再通过 `pub use copr_handler::*` 导出其中的类型和函数；相邻的 `cop_handler_dag.rs`、`analyze.rs`、`checksum.rs` 则继续为这里定义的 `coprHandler` 增加具体请求处理方法。

它位于 mockstore 的存储侧协处理器边界：上游 [`rpc_copr.rs`](rpc_copr.rs) 把单次或批量 RPC 请求交给 `coprHandler`，下游 DAG 路径通过 [`executor.rs`](executor.rs) 中的 `executor` trait 拉取扫描、过滤、聚合、TopN 和 Limit 的结果。这里的协议结构是仓库内 mock 实现，不是 kvproto/tipb 生成类型的完整替代品。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认，包名为 `astersql-store-mockstore-mockcopr`，Go 对照包为 `pkg/store/mockstore/mockcopr`。清单中的 TiDB 子 crate 依赖均为 optional；本文件自身只直接使用 Rust 标准库，具体执行能力由同 crate 模块提供。

## 核心职责

1. 定义 mock Coprocessor 共用的数据面：`Datum`、`Row`、`KeyRange`、`KvPair`、请求/响应、DAG 算子规格以及执行摘要。
2. 通过 `KvReader` 抽象存储读取，并提供 `MemoryReader`，让独立测试无需真实 TiKV 即可按键范围和 `start_ts` 读取数据。
3. 提供最小 SQL 值语义：跨数值类型排序、NULL 三值逻辑、真假判断、加法和稳定的行编码。
4. 由 `coprHandler::handle_request` 按 `RequestPayload` 分发单请求，由 `handleBatchCopRequest` 构造并排空批量 DAG 执行器。
5. 用 `drainRowsFromExecutor` 把执行器行投影到 `output_offsets`，并用 `mockBatchCopDataClient::Recv` 模拟逐包接收和流结束。

## 主要符号

- `CopError`：统一错误枚举。`Region`、`Locked`、`InvalidRequest`、`ColumnOffset`、`Type`、`Codec` 和 `EndOfStream` 分别覆盖 RPC/存储上下文、锁、请求校验、投影越界、值运算、编码以及流耗尽。
- `Datum` 与 `Row`：支持 `Null`、`Int`、`Uint`、`Real`、`Bytes` 五类值；`Row = Vec<Datum>`。`Datum::cmp` 令 NULL 最小，数值类型按数值交叉比较，字节串按字典序比较；`Datum::encode` 使用一字节类型标签及大端载荷。
- `KeyRange`、`next_key`、`KvPair`：表达半开区间 `[start, end)`，空 `end` 表示无上界；`KeyRange::is_point` 以 `end == next_key(start)` 判定点范围。
- `KvReader`：要求实现线程安全的 `scan`；默认 `checksum` 扫描可见行，对键和编码行分别做 `fnv64` 后异或，返回校验值、行数和总字节数。
- `MemoryReader`：以 `BTreeMap<Vec<u8>, (Row, u64)>` 保存每键一个行版本及提交时间，只返回 `commit_ts <= start_ts` 的记录，并支持正序/倒序范围扫描。
- `Expr`：列、常量、六种比较、`And`/`Or`/`Not`、`IsNull` 和同类型数值 `Add`；`eval` 在行上递归求值。
- `ExecutorSpec`、`DagRequest`：描述 Table/Index Scan、Selection、Hash/Stream Agg、TopN、Limit，以及输出列偏移、编码类型和执行摘要开关。
- `RequestPayload`、`Request`、`Response`：单请求及响应模型；`BatchRequest`、`BatchResponse` 是批包装；`Chunk` 同时保留编码字节 `rows_data` 和便于测试断言的结构化 `rows`。
- `coprHandler`：持有 `Arc<dyn KvReader>` 及可注入的 `region_error`。`new` 构造正常处理器，其他模块基于同一类型实现 DAG、ANALYZE 和 Checksum 分支。
- `drainRowsFromExecutor`：持续调用 `executor::Next`，按 `DagRequest::output_offsets` 选择列并编码到一个 `Chunk`。
- `mockBatchCopDataClient`：保存预先算好的 `BatchResponse` 列表及 `offset`；`Recv` 每次克隆一项并推进游标，耗尽后返回 `CopError::EndOfStream`。

## 执行流程

单请求从 `coprRPCHandler::HandleCmdCop` 进入。RPC 层先执行 `RPCSession::CheckRequestContext`，再用会话中的 reader 和 region error 构造 `coprHandler`，调用 `handle_request`。后者匹配 `RequestPayload`：DAG 进入 `handleCopDAGRequest`，ANALYZE 进入 `handleCopAnalyzeRequest`，Checksum 进入 `handleCopChecksumRequest`，最后统一包成仅含一个 `Response` 的 `BatchResponse`。

DAG 的实际树构建不在本文件：`cop_handler_dag.rs::buildDAGExecutor` 校验 region error、非空 ranges、DAG 载荷和非空 executors，创建 `dagContext`，再由 `buildDAG`/`buildExec` 串起执行器。执行器共同实现 `executor.rs::executor`，以 `Next() -> Result<Option<Row>, CopError>` 进行拉取。

批请求从 `coprRPCHandler::HandleBatchCop` 进入，调用 `handleBatchCopRequest`：

1. 若 `coprHandler.region_error` 存在，立即返回该错误，不创建数据客户端。
2. 依次处理 `BatchRequest.requests`；每项都调用 `buildDAGExecutor`，因此当前 Rust 批路径只接受 DAG 请求。
3. `drainRowsFromExecutor` 不断拉取行；对每个输出偏移先边界检查，再编码并保存结构化值。
4. 每个输入请求生成一个仅含一个 `Response`、一个 `Chunk` 的 `BatchResponse`，全部预计算后放入 `mockBatchCopDataClient`。
5. RPC 层注册超时租约，并立即通过流包装预取首个响应；后续调用最终落到 `mockBatchCopDataClient::Recv`。

`MemoryReader::scan` 先拒绝 `start >= end` 的非开放区间，再根据方向排列 ranges 和各 range 内的键；它按请求顺序拼接结果，不做重叠区间去重。每个命中的键只在其单一保存版本的 `commit_ts` 不晚于 `start_ts` 时返回。

## 数据与状态

`Datum` 是本文件最基础的值状态。比较遵循本地约定而非完整 MySQL 类型系统：NULL 与任意值比较的表达式结果是 NULL；`And`/`Or` 实现短路和三值逻辑；`Add` 只接受完全相同的数值变体，整数使用饱和加法，混合数值类型返回 `CopError::Type`。`Bytes` 的真值仅将空值和字节串 `"0"` 判为假。

`MemoryReader.rows` 是有序、只读扫描所依赖的状态。克隆行和值使返回的 `KvPair` 不借用 reader；代价是扫描和结果传递会分配、复制。该模型每键只保存一个 `(Row, commit_ts)`，所以只模拟时间戳可见性筛选，不模拟同一键的完整 MVCC 版本链或锁解析。

批量响应的流状态只有 `mockBatchCopDataClient.offset`。响应在客户端构造前已全部执行并缓存在 `responses` 中，因此它模拟“逐条接收”接口，但不是边执行边产生数据的惰性流。`Response` 中的 `region_error`、`other_error`、`locked` 是不同错误通道；DAG 单请求还可携带 `counts` 与 `execution_summaries`。

## 依赖与调用关系

直接上游调用边由源码引用确认：

- `rpc_copr.rs::HandleCmdCop -> coprHandler::handle_request`，用于普通 CmdCop。
- `rpc_copr.rs::HandleBatchCop -> coprHandler::handleBatchCopRequest`，用于 BatchCop，并在成功后包装租约和预取首包。
- `copr_handler_test.rs` 直接构造 `coprHandler`，验证批 DAG 排空和构建失败传播；`main_test.rs`、`cop_handler_dag_test.rs`、`analyze_test.rs`、`checksum_test.rs` 也复用本文件的类型或构造函数。

主要下游调用边为：

- `handle_request -> handleCopDAGRequest | handleCopAnalyzeRequest | handleCopChecksumRequest`；实现分别位于 `cop_handler_dag.rs`、`analyze.rs`、`checksum.rs`。
- `handleBatchCopRequest -> buildDAGExecutor -> buildDAG/buildExec`，随后 `drainRowsFromExecutor -> executor::Next`。
- 扫描执行器 `tableScanExec`/`indexScanExec -> KvReader::scan`；`KvReader::checksum -> scan + Datum::encode + fnv64`。
- `Expr::eval` 递归调用自身，并通过 `compare_datums`、`add_datums` 与 `Datum::truthy/cmp` 落实值语义。

RustCodeGraph 已索引本文件 97 个符号及同目录 27 个 Go/Rust 文件。其文件节点能完整读取本文件，但本次精确 `callers/callees` 命令没有产出可用边，所以上述边均由同 crate 的直接调用表达式补证，不把模糊的全仓“used by”列表当作精确调用关系。

## 错误处理与边界

- `MemoryReader::scan` 对非空结束键要求 `start < end`；空结束键允许开放扫描。空 ranges 返回空结果而不是错误，DAG 请求层会更早拒绝空 ranges。
- `Expr::Column`、`drainRowsFromExecutor` 的投影和 `evalContext` 的列解码都以 `CopError::ColumnOffset` 报告越界，不发生直接索引 panic。
- `add_datums` 的 NULL 传播优先于类型检查；整数溢出采用饱和而非报错或环绕。
- `handleBatchCopRequest` 对 region error、DAG 构建错误、执行器 `Next` 错误和输出偏移错误均直接返回 `Err`。由于响应全部预计算，任一请求失败会使整个客户端构造失败，之前生成的响应不会返回给调用者。
- `handle_request` 自身不返回 `Result`；具体分支把错误编码进 `Response`。未知载荷在 Rust 枚举模型中不可表示，不存在 Go `switch` 的 default panic 分支。
- `mockBatchCopDataClient::Recv` 以 `EndOfStream` 表示耗尽；这对应 Go 客户端的 `io.EOF`，但错误类型和响应编码形式不同。
- `Datum::Real` 使用 `f64::total_cmp`，因此 NaN 也有确定全序；这种顺序是 Rust mock 的确定性实现细节，不应无证据外推为完整 SQL 浮点排序契约。

## 并发与资源生命周期

`KvReader: Send + Sync` 且由 `Arc<dyn KvReader>` 共享，使同一 reader 可安全交给多个 handler/RPC 会话。`MemoryReader` 扫描只取得不可变借用，没有内部锁或后台任务；并发一致性取决于注入的其他 `KvReader` 实现遵守 trait 的线程安全约束。

`executor: Send` 允许执行器对象跨线程所有权转移，但本文件中的排空循环是同步、单线程的。`handleBatchCopRequest` 在返回前完成所有扫描、执行、投影和编码，客户端之后只读取自身的 `Vec<BatchResponse>`；它没有通道、锁、取消检查或背压机制。

真正的批 RPC 生命周期位于 `rpc_copr.rs`：`HandleBatchCop` 创建 `Lease`，通过同步通道交给超时监控线程，并预取第一包；`coprRPCHandler::Close` 通知线程退出并 join。`coprHandler` 和 `mockBatchCopDataClient` 自身没有显式清理逻辑，最后一个 `Arc` 和拥有的 Vec 在离开作用域时由 Rust 自动释放。

## 与 Go 版本的对应关系

Go 基准文件 [`copr_handler.go`](copr_handler.go) 中，`coprHandler` 嵌入 `*testutils.RPCSession`；Rust 将其所需能力拆成 `Arc<dyn KvReader>` 和可注入 `region_error`。两版批路径都逐项构建 DAG、持续调用执行器 `Next`、按 `OutputOffsets` 追加行数据，并在构建或执行失败时立即返回错误。

两版请求形态存在结构差异：Go `BatchRequest` 按 `Regions` 遍历，并为每个 region 用公共 `Data`、`StartTs` 和该 region 的 ranges 临时构造 DAG `coprocessor.Request`；Rust `BatchRequest` 直接持有完整 `Vec<Request>`。因此 Rust 复现的是逐请求/逐 region 的执行意图，而不是 kvproto 对象布局。

Go 的 `drainRowsFromExecutor` 只填 `tipb.Chunk.RowsData`；Rust `Chunk` 额外保留 `rows` 供测试检查，并在投影前显式验证偏移。Go `mockBatchCopDataClient` 持有 `[]tipb.Chunk`，每次 `Recv` 把一个 chunk marshal 为 `SelectResponse.Data`，耗尽返回 `io.EOF`；Rust 客户端持有已包装的 `BatchResponse`，返回结构化 `Response.chunks`，耗尽返回 `CopError::EndOfStream`。

单请求 Go RPC 直接按请求类型调用三个 handler，未知类型 panic；Rust 先映射为封闭的 `RequestPayload` 枚举，再由 `handle_request` 分发。Go 的 context 会传入 `Next(ctx)` 并由 Batch RPC 创建可取消 context；Rust 的 `executor::Next` 没有 context 参数，取消/超时租约只存在于外层 RPC 包装，不能中断本文件同步预计算中的执行。

## 扩展指南

- 新增值类型时，应同时更新 `Datum`、`datum_tag`、`Ord`、`truthy`、`encode`，并检查 `Expr`、聚合、TopN、ANALYZE 编码及 checksum 是否仍一致；测试应放在独立的 `*_test.rs`，不要内嵌到生产源文件。
- 新增表达式应扩展 `Expr::eval`，明确 NULL、类型转换、溢出和错误语义，并在 `main_test.rs` 或新的独立表达式测试文件中覆盖正常、NULL、类型错误和列越界。
- 新增请求种类需要同时扩展 `RequestPayload`、`coprHandler::handle_request`、RPC 适配和响应错误映射；若批路径也支持，必须明确它是否仍只走 `buildDAGExecutor`。
- 改变批流行为时，重点修改 `handleBatchCopRequest`、`drainRowsFromExecutor` 和 `mockBatchCopDataClient::Recv`，并同步 `copr_handler_test.rs` 与 `rpc_copr_test.rs`。若改为惰性执行，需要重新设计错误出现时机、取消传播、背压及 reader/executor 生命周期。
- 扩展存储模型优先实现新的 `KvReader`，不要把真实 TiKV 细节塞入 `MemoryReader`。若需要多版本/锁语义，应为相同 key 的版本选择、锁错误映射、重叠 ranges 和正反向扫描各加独立回归测试。
- 与 Go 对齐时要分别核对 `copr_handler.go` 的排空逻辑、`cop_handler_dag.go` 的流客户端及 `rpc_copr.go` 的 RPC/租约逻辑，避免只凭同名文件判断完整行为。兼容风险主要是协议编码和错误出现时机；性能风险主要是全量预计算、行克隆及 `rows_data`/`rows` 双份保存。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点；`files --filter pkg/store/mockstore/mockcopr` 确认同目录 27 个文件；`node --file ... --offset ...` 读取了 `copr_handler.rs` 全部 653 行；`query` 定位 `coprHandler` 与 Rust/Go 两个 `drainRowsFromExecutor`。精确 callers/callees 查询未产生结果，调用边改由直接源码引用验证。
- 目标源码：[`copr_handler.rs`](copr_handler.rs)，核对错误/值/范围/reader/表达式/协议结构、批处理入口、排空函数和流客户端。
- crate 与模块：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)，核对包名、Go 对照元数据、optional 依赖、公开再导出和独立测试模块。
- Rust 直接依赖与调用者：[`cop_handler_dag.rs`](cop_handler_dag.rs)、[`executor.rs`](executor.rs)、[`analyze.rs`](analyze.rs)、[`checksum.rs`](checksum.rs)、[`rpc_copr.rs`](rpc_copr.rs)。
- Rust 测试：[`copr_handler_test.rs`](copr_handler_test.rs) 验证 65 行批 DAG 被排入单 chunk，并验证空执行器错误在返回客户端前传播；[`main_test.rs`](main_test.rs) 覆盖 NULL 语义、开放倒序范围和摘要开关；[`cop_handler_dag_test.rs`](cop_handler_dag_test.rs) 覆盖空 ranges 拒绝。
- Go 对照：[`copr_handler.go`](copr_handler.go)、[`cop_handler_dag.go`](cop_handler_dag.go)、[`rpc_copr.go`](rpc_copr.go)，分别核对批 DAG 排空、数据流 `Recv`/EOF、单请求分发和租约生命周期。
- 本任务是纯文档分析，未运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工检查所有关键行为均能回指上述符号或文件。
