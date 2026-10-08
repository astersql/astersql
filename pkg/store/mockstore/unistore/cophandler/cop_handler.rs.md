# `pkg/store/mockstore/unistore/cophandler/cop_handler.rs`

## 文件定位

本文件是 Rust UniStore mock coprocessor crate 的公共数据模型与请求分派门面。crate 入口 `pkg/store/mockstore/unistore/cophandler/lib.rs` 以 `pub mod cop_handler` 导出它；同一 crate 的 `analyze.rs`、`mpp_exec.rs`、`mpp.rs`、`topn.rs` 和 `closure_exec.rs` 都复用这里定义的 `Datum`、`Expr`、`Executor`、`KvReader`、`Response` 与 `CopError`。

对外的主入口是 `handle_cop_request(&dyn KvReader, &Request) -> Response`（第 761 行）。它接收 Rust 自定义请求，而不是 Go 版本的 `coprocessor.Request` protobuf；仓库搜索到的直接调用目前集中在同 crate 的独立测试 `cop_handler_test.rs`，没有发现 Rust 服务端把真实 RPC 解码后接到该入口。因此它当前主要承担可测试的 mock 执行内核和移植语义载体，而不是 Go `HandleCopRequest` 的线协议替代品。

`pkg/store/mockstore/unistore/cophandler/Cargo.toml` 将该目录定义为 `astersql-store-mockstore-unistore-cophandler` crate，`[lib]` 指向 `lib.rs`，并用 `package.metadata.porting.go-package` 标明对应 Go 包 `pkg/store/mockstore/unistore/cophandler`。清单列出的 TiDB 子 crate 依赖均为 optional；本文件自身只直接使用标准库及本 crate 的 `analyze`、`mpp_exec` 模块。

## 核心职责

1. 定义 mock 执行所需的值、行、键范围、请求、响应、错误和执行器树：`Datum`、`Row`、`KeyRange`、`Request`、`Response`、`Executor` 等。
2. 用 `KvReader` 抽象范围扫描；以 `MemoryReader` 提供按 `commit_ts <= start_ts` 过滤的内存实现，并提供默认 checksum 算法。
3. 通过 `Expr::eval` 实现列引用、常量、比较、三值逻辑、`IS NULL` 和同类型数值加法。
4. 通过 `executor_list_to_tree` 把自底向上的一元算子列表组装为执行树，通过 `handle_cop_request` 分派 DAG、Analyze 和 Checksum 请求。
5. 把 `mpp_exec::ExecutionOutput` 转换为固定最多 64 行的 `Chunk`，可选把 datum 编码进 `Response.data`，并携带 range 计数、NDV、执行摘要和扫描明细。
6. 提供 Region 范围求交、chunk 追加、锁可见性判断、耗时摘要及进程级时区偏移缓存等辅助能力。

这些职责是轻量 mock 语义。它没有直接解析 tipb/protobuf、构造 TiDB session context、解码真实行格式或访问 Badger/MVCC DB；相关完整行为仍在同路径 Go 实现中。

## 主要符号

- `ROWS_PER_CHUNK: usize = 64`：响应分块上限，与 Go `rowsPerChunk` 一致。
- `Datum` / `Row`：五种简化 SQL 值和行别名。`Datum::cmp` 允许整数、无符号整数和浮点数跨类型比较；`Bytes` 字典序比较；`Datum::truthy` 把 NULL、数值零、空字节串和字节串 `"0"` 视为假；`Datum::encode` 产生本 mock 私有的“类型标签 + 定长或长度前缀载荷”。
- `KeyRange` / `KvPair` / `KvReader`：半开区间、带提交时间戳的扫描结果及存储读取接口。`KvReader::checksum` 是可覆盖的默认实现；它扫描可见行，对 key 和编码 value 的 FNV-1a 哈希做异或并统计 KV 数及字节。
- `MemoryReader`：`BTreeMap<Vec<u8>, (Row, u64)>` 后端。`scan` 对每个输入 range 顺序遍历，过滤范围及快照可见性；倒序请求对最终合并结果整体 `reverse`。
- `Expr` / `Expr::eval`：简化表达式 AST。`compare_datum`、`eval_and`、`eval_or` 保留 SQL NULL/三值逻辑；`add` 只接受相同数值类型，整数加法使用饱和语义。
- `ByItem`、`AggKind`、`AggCall`、`JoinType`、`ExchangeType`、`Executor`：定义扫描、选择、Limit、TopN、投影、Expand、聚合、Join 和 Exchange 的计划结构；实际算子语义由 `mpp_exec.rs` 的 `execute_executor` 实现。
- `DagRequest` / `RequestPayload` / `Request`：DAG 根或扁平执行器列表、输出列、范围、快照时间戳、分页和缓存字段。根存在时优先于 `executors`。
- `Chunk`、`ExecutionSummary`、`ScanDetail`、`LockInfo`、`Response`：返回行、编码数据、执行/扫描统计、锁错误、分页范围和缓存信息。`ScanDetail::merge` 与 `record` 均以饱和加法维护计数。
- `CopError`：区分不支持、非法请求、列越界、类型错误、锁、取消和 tunnel 错误；`handle_cop_request` 将 `Locked` 单独映射到 `Response.locked`，其余错误映射到 `other_error`。
- `LocationMap` / `global_location_map`：`OnceLock<LocationMap>` 管理的进程级单例，内部用 `RwLock<HashMap<String, i32>>` 缓存“时区名 -> UTC 偏移秒数”。本 crate 当前未发现生产调用。
- `executor_list_to_tree` / `attach_child`：首元素作为叶子，后续元素逐个包裹为父节点；仅接受 Selection、Limit、TopN、Projection、Expand、Aggregation、ExchangeSender 作为一元父节点。
- `handle_cop_request` / `handle_dag` / `response_from_output`：请求分派、DAG 执行和响应组装主链。
- `extract_kv_ranges`、`append_row`、`check_lock`、`duration_summary`：范围裁剪、分块、锁判断和摘要辅助函数；当前 Rust 生产调用链未把前三者接入 `handle_cop_request`。

## 执行流程

主链可按以下顺序理解：

1. 调用者构造实现 `KvReader` 的读取器和 `Request`，进入 `handle_cop_request`。
2. 若 `cache_enabled` 且 `cache_if_match_version == start_ts`，立即返回 `cache_hit = true`、`cache_last_version = start_ts` 的空响应；这里没有读取存储或执行 payload。
3. 否则记录 `Instant`，按 `RequestPayload` 分派：
   - `Dag` 调用私有 `handle_dag`；
   - `Analyze` 调用 `analyze::analyze(reader, ranges, start_ts, analyze_request)`，将所得字节放入 `Response.data`；
   - `Checksum` 不调用 `KvReader::checksum`，而是按 Go UniStore mock 的固定行为返回三个大端序 `u64(1)`。
4. DAG 路径先拒绝空 ranges，再选择 `DagRequest.root`，或用 `executor_list_to_tree` 组装 `executors`。随后调用 `mpp_exec::execute_executor(reader, ranges, start_ts, root)`。
5. 若 `output_offsets` 非空，逐行按偏移重新投影；任何越界产生 `CopError::ColumnOffset`，整个请求失败。
6. 若 `paging_size > 0` 且总行数严格大于该值，截断结果，并把请求的最后一个 range 原样作为 `last_range`。此提示不是精确到最后一行键的继续游标。
7. `response_from_output` 每 64 行生成一个 `Chunk`；`encode_chunk` 为真时按行、按列连续调用 `Datum::encode` 写入 `data`，同时转移执行器返回的 range counts、NDV、summary 与 scan detail。
8. 成功响应若没有 summary，入口补一条 `iterations = 1` 的摘要，行数来自 chunks，耗时来自入口 `Instant`。锁错误放到 `locked`；其他错误的显示文本放到 `other_error`。

辅助路径中，`extract_kv_ranges` 逐个校验请求范围必须满足 `start < end`，与 Region 半开区间求交，丢弃空交集，最后可整体倒序；`check_lock` 仅当 key 相同、锁事务 `start_ts` 不晚于读时间戳且锁版本不在 resolved 列表时返回 `CopError::Locked`。

## 数据与状态

- `MemoryReader.rows` 是按 key 排序的单版本映射；它不是完整 MVCC 数据库，同一 key 不能同时保存多个提交版本。快照规则只有 `commit_ts <= start_ts`。
- `Request` 与 `Response` 都是拥有数据的普通 Rust 值。执行过程中会克隆执行器树、范围、行和值，适合 mock 和测试，但大结果集会产生额外内存及复制成本。
- `Executor` 是递归拥有的树；一元算子通过 `Box<Executor>` 持有子节点，Join 持有左右子树。`DagRequest.executors` 的约定是“首元素叶子、后续元素依次向上”，不同于 Go `ParentIdx` 可表达的非线性父子关系。
- `ScanDetail::record` 以 `Datum::encode` 后的长度计算 value 字节，并让 processed 与 total 同步增加；它表达 mock 成功扫描数据量，不含被跳过/解码失败版本的独立记录入口。
- 唯一的进程级可变状态是 `global_location_map` 的 `OnceLock + RwLock<HashMap<...>>`。缓存无淘汰机制，值只是固定偏移秒数而非 Go 的 `*time.Location` 完整时区规则。
- `resolved_locks` 存在于 `Request`，但 `handle_dag`/`execute_executor` 当前没有调用 `check_lock`；所以仅设置该字段不会改变主链扫描行为。`collect_range_counts` 同样未在本文件分派逻辑中用于开关输出。

## 依赖与调用关系

RustCodeGraph 将目标文件识别为 115 个符号。关键静态关系及源码核验如下：

- `handle_cop_request` -> `handle_dag` -> `mpp_exec::execute_executor` -> 各执行器实现；`execute_executor` 的索引签名位于 `mpp_exec.rs:118`。
- `handle_cop_request` -> `analyze::analyze`，并传递同一 reader、ranges 和 start_ts。
- `handle_dag` -> `executor_list_to_tree`（仅当无显式 root）以及 `response_from_output`。
- `KvReader::checksum` -> `KvReader::scan`、`Datum::encode`、私有 `fnv64`；但 Checksum payload 主链刻意不走这条默认方法。
- `Expr::eval` 递归调用自身及 `compare_datum`、`eval_and`、`eval_or`、`add`。
- `mpp_exec.rs`、`analyze.rs`、`mpp.rs`、`topn.rs`、`closure_exec.rs` 从本模块导入公共类型；`lib.rs` 负责模块装配，独立测试通过 `#[path = "cop_handler_test.rs"]` 挂载。

RustCodeGraph 的 `query` 精确定位了 `handle_cop_request`（`cop_handler.rs:761`）、`executor_list_to_tree`（`:695`）、`extract_kv_ranges`（`:888`）、`check_lock`（`:937`）和 `mpp_exec::execute_executor`（`mpp_exec.rs:118`）。`callers` 命令在本地索引上未在 30 秒内返回结果，因此调用者集合又用仓库级 Rust 引用搜索交叉核验；未发现 crate 外对 `handle_cop_request` 的生产调用。

## 错误处理与边界

- 空 DAG ranges 返回 `InvalidRequest("request range is null")`；空扁平执行器列表、不能接收 child 的父节点和非法范围也返回 `InvalidRequest`。
- 输出列或表达式列引用越界返回 `ColumnOffset`；异类型加法返回 `Type`。整数加法不会溢出报错，而是饱和到类型边界；浮点加法遵循 IEEE 行为。
- 比较遇到 NULL 返回 NULL；AND 中 false 优先于 NULL，OR 中 true 优先于 NULL，NOT NULL 仍为 NULL。`Datum::Ord` 对浮点使用 `total_cmp`，因此包含 NaN 时仍有全序；这不等同于完整 MySQL 比较/类型转换规则。
- `extract_kv_ranges` 要求每个请求 range 的 `start < end`，所以请求端空 end 会被视为非法；Region 的空 end 才表示无上界。独立 Rust 测试覆盖了开放 request end 和反向 range 的拒绝行为。
- 缓存命中条件是请求版本直接等于 `start_ts`，与 Go 通过 `mockCopCacheInUnistore` failpoint 提供独立 cache version 的机制不同；未命中时 Rust 也没有设置 Go 的 `CanBeCached` 或模拟 500ms 处理时间。
- `LocationMap::{get,set}` 在锁中毒时使用 `expect`，会 panic，而不是返回 `CopError`。
- 分页只截断最终行向量，`last_range` 取请求最后一个范围；不能据此假设已经实现 Go 扫描器级精确续扫。
- `handle_cop_request` 把执行错误编码进 `Response`，不向调用者返回 `Result`。只有锁错误保留结构化信息，其他错误仅保留字符串。

## 并发与资源生命周期

- `KvReader: Send + Sync` 允许执行层通过共享引用在多线程环境使用 reader；本文件自身不创建线程或异步任务。
- `MemoryReader::scan` 只通过 `&self` 读取不可变 `BTreeMap`，克隆返回行，不持有跨调用锁或借用。
- `global_location_map` 由 `OnceLock` 延迟初始化且只初始化一次；读写分别取得 `RwLock` 的共享/独占锁，guard 在 `get`/`set` 返回前释放。
- 每个请求的 `Instant`、执行树克隆、`ExecutionOutput`、chunks 和编码缓冲都局限于同步调用栈；成功或错误返回后自动释放。
- 本文件没有 Go `mppExecute` 的 `open`/`next`/`stop` 生命周期；该生命周期位于 Rust `mpp_exec.rs` 的执行器实现。扩展需要保证所有提前返回仍能完成对应资源清理，并在独立的 `mpp_exec_test.rs` 验证。
- `ScanDetail` 的合并和记录不是原子操作，只适用于调用者独占的可变引用；若未来并行汇总，应在上层聚合局部统计，而不是共享同一个实例无锁写入。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/store/mockstore/unistore/cophandler/cop_handler.go`，测试是同目录 `cop_handler_test.go`。已对齐的核心意图包括：请求按 DAG/Analyze/Checksum 分派、执行器列表转树、每 chunk 64 行、Region 范围裁剪与倒序、锁信息结构、时区缓存的读写锁，以及 checksum mock 固定返回 `(1,1,1)`。Rust 测试 `ChecksumResponseMatchesGoMock`、`ExtractKVRangesRejectsOpenEndedRequestRange` 和 `ExtractKVRangesRejectsReversedRange` 固化了其中的边界。

当前 Rust 实现有以下明确差异，扩展时不能忽略：

- Go 接口读取/写入 `coprocessor`、`kvrpcpb` 和 `tipb` protobuf，Rust 使用本地结构和私有编码，尚无线协议互操作证据。
- Go `HandleCopRequestWithMPPCtx` 可把 DAG 分派到 MPP task handler；Rust 入口没有 MPP context 参数。Rust 的 Exchange 类型只是执行器模型的一部分。
- Go `buildDAG` 解析 flags、时区名/偏移、除法精度、keyspace ID、resolved locks，并建立 session/statement context；Rust 没有这些上下文构建步骤。
- Go `ExecutorListsToTree` 支持 `ParentIdx` 和 IndexLookup 子列表，遇到非法结构会 panic；Rust 仅线性包裹一元算子，返回 `CopError`，且 Join 不能由扁平列表的 `attach_child` 组装。
- Go 通过 DBReader、lockstore、rowcodec 和真实 TiDB datum/field type 执行；Rust `MemoryReader` 与简化 `Datum` 不覆盖字符集、collation、decimal、时间、JSON 等语义。
- Go 响应包含 protobuf SelectResponse、warnings、SQL 错误码、ExecDetails/V2、intermediate outputs 和精确执行器摘要；Rust 响应模型只覆盖其中一个子集。
- Go 缓存语义由 failpoint 控制 cache version；Rust 用 `start_ts` 代替。Go checksum 固定值经 protobuf marshal，Rust 是三个连续的大端 `u64`，只保证本地测试所断言的值序列。
- Go 范围裁剪假定输入 ranges 已排序，越过 region end 时可 `break`；Rust 遍历所有 range 并跳过空交集。两者正常有序输入结果一致，但异常无序输入行为不应默认等价。

因此，本文件是“行为意图的轻量 Rust 移植”，不是 Go 文件逐 API、逐协议的完整替换。

## 扩展指南

- 增加新请求类型：扩展 `RequestPayload` 和 `handle_cop_request` 分派，同时定义成功数据编码与错误映射；在 `cop_handler_test.rs` 添加独立回归测试，并逐项对照 Go `HandleCopRequestWithMPPCtx`。
- 增加表达式或值类型：修改 `Datum` 的 `cmp`、`truthy`、`encode` 和 `datum_tag`，以及 `Expr::eval`；同步检查 `topn.rs`、`mpp_exec.rs`、`analyze.rs` 的假设。必须增加 NULL、跨类型、溢出、NaN/排序和编码边界测试。
- 增加执行器：在 `Executor` 建模，在 `mpp_exec.rs::execute_executor` 实现；若是一元算子，还要加入 `attach_child`。Join 或多子节点执行器不能套用当前线性列表约定，应明确树构造格式并测试非法拓扑。
- 接入真实锁检查：扫描层必须在返回 KV 前获得相应 `LockInfo` 并调用 `check_lock(request.start_ts, request.resolved_locks)` 语义，而不能只保留字段；测试至少覆盖未来锁、已 resolved 锁、不同 key 和可见未解决锁。
- 改进分页：在扫描/执行器层追踪最后消费键并构造准确的剩余 `KeyRange`，避免继续使用请求最后 range 作为近似提示；需覆盖升序、降序、多 range、恰好等于 page size 和过滤后分页。
- 接入线协议或真实 TiDB 类型时，应优先复用 Cargo 清单中的现有 crate，而不是继续扩大本地简化模型；需要验证 protobuf 编码、collation、时区、warning/SQL error code 与 Go 一致性。
- 修改响应统计时，同步 `mpp_exec.rs` 的 `ExecutionOutput`/`ScanDetail` 产生逻辑，并扩展 `dag_response_reports_scanned_versions_and_bytes`。计数必须保持饱和、明确 processed 与 total 的定义。
- Rust 测试逻辑继续放在同目录独立文件 `cop_handler_test.rs`（由 `lib.rs` 挂载），不要内嵌进生产源文件；Go 对照回归位于 `cop_handler_test.go`。

兼容风险主要是私有编码与未来 protobuf 的差异、简化类型转换与 MySQL 语义的差异、缓存/分页提示被上层误当成完整实现；性能风险主要是整批扫描、执行器树和行的多次克隆，以及 `encode_chunk` 额外构造完整 data 缓冲。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/store/mockstore/unistore/cophandler` 列出该目录 22 个 Go/Rust 文件。
- RustCodeGraph 源码读取：`node --file pkg/store/mockstore/unistore/cophandler/cop_handler.rs --offset 1 --limit 500` 与 `--offset 480 --limit 520` 覆盖目标文件全部 967 行。
- RustCodeGraph 符号查询：`handle_cop_request`（`:761`）、`executor_list_to_tree`（`:695`）、`extract_kv_ranges`（`:888`）、`check_lock`（`:937`）、`mpp_exec.rs::execute_executor`（`:118`）。`callers handle_cop_request` 在 30 秒窗口内未返回，调用者结论另由 Rust 引用搜索核验。
- 读取的 crate/入口证据：`pkg/store/mockstore/unistore/cophandler/Cargo.toml`、`pkg/store/mockstore/unistore/cophandler/lib.rs`。
- 读取的 Go 对照证据：`pkg/store/mockstore/unistore/cophandler/cop_handler.go`，重点核对 `HandleCopRequestWithMPPCtx`、`ExecutorListsToTree`、`buildDAG`、`genRespWithMPPExec`、`extractKVRanges`、`appendRow` 与 `handleCopChecksumRequest`。
- 读取的独立测试：`pkg/store/mockstore/unistore/cophandler/cop_handler_test.rs`；并参考 Go `pkg/store/mockstore/unistore/cophandler/cop_handler_test.go` 的数据、点查、执行器和范围测试架构。
- 测试事实：Rust 测试覆盖 range 非法边界、固定 checksum、SQL NULL、点查、Selection、TopN、取消、DAG 冒烟及扫描字节统计；没有发现缓存、`check_lock`、`LocationMap`、`append_row` 或精确分页的直接 Rust 回归。
- 结构验证使用任务指定命令，要求目标文件存在且固定二级标题恰好为 11 个。本任务为纯文档分析，按计划不运行 Cargo。
