# `pkg/executor/checksum.rs`

## 文件定位

本文件属于 `astersql-executor` crate。crate 根在 `pkg/executor/Cargo.toml` 中以 `lib.rs` 为入口，`pkg/executor/lib.rs:77-78` 通过 `pub mod checksum` 公开本模块；独立测试由 `pkg/executor/lib.rs:414-416` 的 `#[path = "checksum_test.rs"]` 接入。

它移植了 Go 版 `ADMIN CHECKSUM TABLE` 执行器的核心算法：把逻辑表展开成表数据与 Public 索引的 DistSQL checksum 请求，并发执行，最后按逻辑表汇总 CRC64-XOR、KV 数和字节数。与 Go 版不同，Rust 文件不直接依赖具体的 session、KV、tipb 或 chunk 类型，而是用 `ChecksumBackend`、`ChecksumResultStream` 和 `ChecksumOutputChunk` 三个 trait 隔离生产设施（`pkg/executor/checksum.rs:126-192`）。

当前接线状态必须谨慎理解：Rust 模块已由 crate 根导出，算法也有独立单元测试，但全仓 Rust 引用搜索只找到 `pkg/executor/checksum_test.rs` 对 `ChecksumTableExec` 和 `newChecksumContext` 的使用，没有找到生产 `ChecksumBackend` 实现或 builder 对该执行器的实例化。因此，本文件目前是可测试的移植核心，不应描述为已经接入 Rust SQL 执行主链。Go 生产主链则在 `pkg/executor/builder.go:763-777` 构造 `ChecksumTableExec`。

## 核心职责

1. 用轻量元数据类型 `DatabaseInfo`、`TableInfo`、`IndexInfo`、`PartitionInfo` 描述待校验对象，并用 `ChecksumContext` 保存同一快照时间戳和累计结果（`pkg/executor/checksum.rs:40-92,381-401`）。
2. 对每个逻辑表 ID 本身以及每个分区物理表 ID，各生成一个表扫描请求；再为每个 Public 索引生成一个索引扫描请求。非 Public 索引被跳过（`ChecksumContext::buildTasks`、`appendRequest4PhysicalTable`）。
3. 固定使用 `ChecksumAlgorithm::Crc64Xor`；表扫描根据 common handle 选择 full-not-null 或 signed-int handle 范围，索引扫描使用 full range（`buildTableRequest`、`buildIndexRequest`）。
4. 从后端读取会话 checksum 并发度，建立 scoped worker 池，执行所有任务并收集每个任务的一份结果（`ChecksumTableExec::Open`、`checksumWorker`）。
5. 流式读取一次 DistSQL 请求的所有原始响应块，逐块解码并合并；无论读取成功或失败，只要结果流创建成功，就关闭流并执行后置钩子（`handleChecksumRequest`）。
6. `Next` 把每张逻辑表的库名、表名、checksum、KV 数、字节数写成五列，并通过 `done` 保证只输出一轮（`pkg/executor/checksum.rs:263-282`）。

## 主要符号

- `ChecksumResponse { checksum, total_kvs, total_bytes }`：单次扫描或逻辑表累计值。默认值全为零；合并规则由 `updateChecksumResponse` 定义。
- `DatabaseInfo`、`TableInfo`、`IndexInfo`、`PartitionInfo`、`PartitionDefinition`、`SchemaState`：为移植逻辑提供的本地元数据模型。`SchemaState` 只区分 `Public` 与 `Other`，不是完整 schema 状态枚举。
- `ChecksumScanOn::{Table, Index}`、`ChecksumAlgorithm::Crc64Xor`：请求的扫描对象和算法；当前没有其他算法分支。
- `ChecksumRequestSpec<R,T,G,S>`：传给后端 request builder 的完整规格，包含物理表/索引、范围、快照、DistSQL 扫描并发度以及资源组标签/名称和显式请求来源（`pkg/executor/checksum.rs:110-124`）。
- `ChecksumResultStream`：抽象流的 `next_raw` 与 `close` 生命周期。
- `ChecksumBackend`：生产适配边界，涵盖 BaseExecutor 打开、会话变量、范围构造、请求构造、kill signal、DistSQL 调用、响应解码、日志及后置钩子（`pkg/executor/checksum.rs:138-185`）。关联类型要求 `Context: Clone`、`Error: Display + Send`、`Request: Send`，trait 本身要求 `Sync`，以便 worker 线程共享后端引用。
- `ChecksumOutputChunk`：`Next` 所需的最小输出接口。
- `ChecksumTableExec<B>`：顶层执行器，持有后端、`table_id -> ChecksumContext` 映射和单次输出标志；公开入口是 `Open` 与 `Next`。
- `ChecksumTask<R>` / `ChecksumResult<E>`：worker 队列输入与返回值。表扫描以 `indexID = -1` 标识，索引扫描保存真实索引 ID（`pkg/executor/checksum.rs:365-379,442-457`）。
- `ChecksumContext` / `newChecksumContext`：单张逻辑表的库表元数据、快照 `startTs` 和累计响应。
- `checksumRequestCount`：按“物理表数 ×（Public 索引数 + 1）”估算请求数；分区表的物理表数在这里包含逻辑表 ID 本身以及所有分区 ID（`pkg/executor/checksum.rs:516-528`）。
- `getChecksumTableConcurrency`：把后端提供的字符串解析成 `usize`。
- `updateChecksumResponse`：checksum 按位异或，`total_kvs` 和 `total_bytes` 相加（`pkg/executor/checksum.rs:538-543`）。

文件没有条件编译项、模块级常量或本地 type alias。公开类型较多是为了让具体后端和独立测试在模块外组合；`buildTasks`、请求构造、worker 与单请求处理函数保持私有。

## 执行流程

`Open(context)` 的主流程如下（`pkg/executor/checksum.rs:206-261`）：

1. 调用 `ChecksumBackend::open_base` 打开基础执行器。
2. `getChecksumTableConcurrency` 读取并解析 worker 数；解析失败映射为后端错误，零值显式返回 `zero_checksum_concurrency`。
3. `ChecksumTableExec::buildTasks` 遍历 `tables`，让每个 `ChecksumContext` 生成任务并扁平化为一个向量。
4. `ChecksumContext::buildTasks` 先为逻辑表 ID 生成请求，再为每个分区 ID 生成请求。每个物理 ID 都先追加表扫描，再追加所有 Public 索引扫描。
5. 表请求使用当前 `startTs`、后端的 DistSQL scan concurrency 和资源组字段；common handle 表使用 `full_not_null_range`，否则使用 `full_int_range(false)`。索引请求使用 `full_range`，并把 `common_handle` 固定为 `false`。
6. 所有任务进入 `Arc<Mutex<VecDeque<_>>>`；`std::thread::scope` 启动会话变量指定数量的 worker。worker 每次只在 `pop_front` 时持锁，随后在锁外执行 I/O，并把结果写入容量为任务数（空任务时为 1）的同步通道。
7. scoped threads 全部结束后，主线程按任务数接收结果。成功结果记录信息并通过 `handleResult` 合并到对应逻辑表；失败结果告警并覆盖 `last_error`。收集完全部结果后，如发生过错误则返回最后接收到的错误。

一次任务在 `handleChecksumRequest` 中执行（`pkg/executor/checksum.rs:334-362`）：先检查 kill signal，取得 session context，调用后端 checksum 创建结果流；随后循环 `next_raw -> decode_checksum_response -> updateChecksumResponse -> 再检查 kill signal`。循环结束或中途出错后都执行 `close` 和 `after_handle_checksum_request`。若 `close` 失败，其错误优先于此前的读取、解码或 kill 错误。

调用方随后使用 `Next` 输出结果。第一次调用先 reset chunk，再遍历 `tables` 写五列并置 `done = true`；以后调用仍会 reset chunk，但立即返回空结果。由于底层是 `HashMap`，多表结果行顺序没有稳定保证，调用方若要求顺序应显式排序。

## 数据与状态

- `ChecksumTableExec::tables` 以逻辑表 ID 为唯一键。每个 `ChecksumResult.tableID` 必须能在该映射中找到，否则 `handleResult` 会 panic；这是 task 构造与收集阶段之间的不变量。
- `ChecksumContext::startTs` 被复制进该表的所有表/索引请求，保证一次逻辑表校验使用同一快照。不同 context 理论上可持有不同时间戳；本文件不负责统一它们。
- `ChecksumContext::response` 从全零开始。CRC64-XOR 的结合律和交换律使并发完成顺序不影响 checksum；KV 数与字节数也按无符号加法累计。
- 分区表会同时对逻辑表 ID 和每个分区 ID 构造请求，这一行为与 Go `checksumContext.buildTasks` 一致。测试中的“两分区 + 一 Public 索引”得到 `3 × 2 = 6` 个请求（`pkg/executor/checksum_test.rs:32-55,298-379`）。
- `ChecksumRequestSpec::concurrency` 是单个 DistSQL 请求内部的扫描并发度；`Open` 解析出的 checksum table concurrency 是外层同时工作的 worker 数，两者不是同一个设置。
- `done` 只控制输出，不控制 `Open` 是否可重复调用。本文件没有重置累计 response 或 done 的 reopen 逻辑，正常使用应遵循执行器生命周期只打开一次、消费一次。
- `u64` 累加使用普通 Rust 加法；极端溢出在 debug/release 配置下行为不同（panic 或回绕），代码没有显式 checked/saturating 处理。实际可接受范围由下游协议和数据规模约束，本文件未声明更强保证。

## 依赖与调用关系

上游关系：

- Rust：`pkg/executor/lib.rs` 公开 `checksum` 模块；`pkg/executor/checksum_test.rs` 实例化 `ChecksumTableExec<TestBackend>` 并调用 `Open`/`Next`。RustCodeGraph 对 `ChecksumTableExec` 的 node trail 同样只显示测试导入和 `partitioned_executor` 实例化。全仓 Rust 搜索未发现生产构造方或 `ChecksumBackend` 的生产实现，因此 Rust 生产上游当前未接线。
- Go 对照：`pkg/executor/builder.go:763-777` 的 `buildChecksumTable` 获取 snapshot TS，按计划中的表构造 `checksumContext`，并返回实现 `exec.Executor` 的 `ChecksumTableExec`。这是完整应用中该功能应处的位置，但不能据此宣称 Rust builder 已经接线。

文件内调用链可概括为：

`Open -> getChecksumTableConcurrency -> buildTasks -> ChecksumContext::buildTasks -> appendRequest4PhysicalTable -> buildTableRequest/buildIndexRequest -> ChecksumBackend::build_request`

以及：

`Open -> checksumWorker -> handleChecksumRequest -> ChecksumBackend::checksum -> ChecksumResultStream::next_raw -> decode_checksum_response -> updateChecksumResponse -> close -> after_handle_checksum_request -> handleResult -> ChecksumContext::handleResponse`。

直接代码依赖只有 Rust 标准库的 `HashMap`、`VecDeque`、`Arc`、`Mutex`、同步通道和 scoped threads。crate 层面 `pkg/executor/Cargo.toml` 声明了 `astersql-distsql`、`astersql-kv`、`astersql-sessionctx`、`astersql-sessionctx-vardef` 等生产依赖，但本文件没有直接引用它们；未来的生产 `ChecksumBackend` 适配器才应把这些具体组件接入 trait。`Cargo.toml` 唯一 feature `nextgen` 也不改变本文件。

## 错误处理与边界

- `open_base`、读取/解析并发度、构建任一请求、kill signal、发起 checksum、拉取原始块、解码、关闭流等错误都通过 `B::Error` 返回。
- 并发度字符串按 `usize` 解析：负数、非数字和超出平台 `usize` 的数值会走 `invalid_checksum_concurrency`；零虽能解析，但被 `Open` 单独拒绝。Go 对照使用 `strconv.ParseInt(..., 64)`，因此在数值域和负数处理上存在语言层差异，Rust 的显式零值错误也是额外防护。
- `Open` 不在首个任务错误处短路，而是等待、收集所有任务。多个错误时返回接收顺序中的最后一个，同时每个错误都会调用 `warn_checksum_failed`；成功任务仍会被合并。这与 Go 循环中不断覆盖命名错误值的意图一致。
- 只有 `ChecksumBackend::checksum` 成功创建结果流后，代码才执行 `close` 和 `after_handle_checksum_request`。初始 kill 检查或创建流失败时没有可关闭资源，也不会运行该后置钩子，与 Go defer 的安装位置一致。
- `close` 错误覆盖此前处理错误；`pkg/executor/checksum_test.rs:384-395` 验证六个流都执行后置钩子，且六个 close 错误都被告警。
- 三个内部不变量用 `expect` 表达而不是可恢复错误：任务队列 mutex 不得 poisoned、每个任务必须恰好发送一个结果、结果 table ID 必须存在。worker 自身 panic 也会在 scoped thread 汇合时传播 panic。这些属于实现一致性失败，不是 SQL 用户输入错误。
- 非 Public 索引被静默排除；`SchemaState::Other` 不区分具体过渡状态。空 `tables` 会正常打开并在首次 `Next` 输出零行。
- 本文件不进行重试、限流、超时或上下文取消策略；这些能力只能由 `ChecksumBackend` 的 kill signal、请求构造和 checksum 实现提供。

## 并发与资源生命周期

- `Open` 使用 scoped OS threads，因此 worker 可以安全借用 `&self.BaseExecutor`，且 `Open` 返回前所有 worker 必须退出；没有脱离执行器生命周期的后台线程。
- 任务队列是 `Arc<Mutex<VecDeque<_>>>`。锁只保护出队，不覆盖网络/流处理，避免把所有 checksum I/O 串行化。任务只会被弹出一次。
- `SyncSender` 的容量为 `task_count.max(1)`。每个任务产生且只产生一个 `ChecksumResult`；主 sender 在 worker 完成后被 drop。当前实现先等待 scoped workers 全部结束、再读取 channel，之所以不会因回传阻塞，是因为容量恰好容纳全部任务结果；若未来改小容量，必须同时把接收移动到 worker 运行期间，否则会死锁。
- worker 数完全来自会话变量，代码没有把它限制到任务数；过大值会创建许多空闲 OS threads，生产适配层或变量校验需要保证合理上限。
- 每个结果流按“创建成功 -> 读取零到多个块 -> close -> 后置钩子”的顺序终结。即使读取、解码或中途 kill 检查失败，`close` 仍执行；`after_handle_checksum_request` 严格在 close 之后执行。
- 累计表结果只在主线程接收阶段修改，worker 不共享 `ChecksumContext`，因此响应合并不需要额外锁。后端自身被多个 worker 以共享引用调用，所以 `ChecksumBackend: Sync`，其内部可变状态必须自行同步。
- `Next` 没有并发保护，签名要求 `&mut self`；执行器预期由单一消费方串行调用。

## 与 Go 版本的对应关系

Rust 主要逐项对应 `pkg/executor/checksum.go`：

- `ChecksumTableExec::Open/Next/buildTasks/handleResult/checksumWorker/handleChecksumRequest` 对应同名 Go 方法（Go `pkg/executor/checksum.go:45-186`，Rust `pkg/executor/checksum.rs:204-363`）。
- `ChecksumTask`、`ChecksumResult`、`ChecksumContext` 和 `newChecksumContext` 对应 Go 的小写类型/构造函数（Go `:188-217`，Rust `:365-401`）。
- `ChecksumContext` 的任务展开、表/索引请求构建和响应合并对应 Go `:219-320`；算法和范围选择保持一致。
- `getChecksumTableConcurrency` 与 `updateChecksumResponse` 对应 Go `:322-336`。XOR 与两项求和完全一致。
- Go 直接调用 `distsql.RequestBuilder`、`distsql.Checksum`、session vars、SQLKiller、日志、failpoint 和 `chunk.Chunk`；Rust 把这些具体行为移到三个 trait，因而当前文件本身没有真实 TiKV 请求类型、protobuf 解码或生产日志实现。
- Go worker 通过 goroutine/channel 消费预填充任务 channel；Rust 用 scoped threads + mutex deque。两者都允许任务完成乱序并在主线程合并，但 Rust 的线程创建成本和 channel 容量不变量不同。
- Go 的 `res.Close` defer 会覆盖此前错误并随后触发 `afterHandleChecksumRequest` failpoint；Rust `handleChecksumRequest` 显式保留这一顺序。
- Go builder 已接线，Rust builder 未找到对应生产构造；这是当前最重要的迁移差距。
- Go `TestChecksum`（`pkg/executor/checksum_test.go:23-42`）验证两分区表的六个请求汇总为 `test t 0 6 6`。Rust `checksum_executor_matches_go_partitioned_table_result_and_request_contract` 不仅验证相同输出，还核对请求类型、范围、start TS、两级并发字段、资源组信息、kill 检查次数和后置钩子次数。

## 扩展指南

- 接入 Rust 生产主链时，应在合适的执行器 builder/adapter 中实现 `ChecksumBackend` 和 `ChecksumOutputChunk`，把真实 BaseExecutor、session vars、SQLKiller、DistSQL RequestBuilder、tipb 解码、日志及 failpoint 映射进来；不要把这些逻辑塞回算法核心。必须同时新增独立测试文件中的生产接线/集成覆盖，不能把测试内嵌到 `checksum.rs`。
- 新增 schema 状态时，先判断是否仍能归并为 `Other`；若状态会改变参与规则，应修改 `SchemaState`、`appendRequest4PhysicalTable`、`checksumRequestCount`，并同步 `pkg/executor/checksum_test.rs` 的计数和请求集合测试。
- 新增 checksum 算法或扫描目标时，应扩展 `ChecksumAlgorithm`/`ChecksumScanOn` 与 `ChecksumRequestSpec` 的构造分支，并确认响应合并规则是否仍是 XOR；不得仅新增枚举而继续无条件调用 `updateChecksumResponse`。
- 修改分区展开时必须同步维护 `checksumRequestCount` 与 `buildTasks`，保证预估容量和真实任务集合采用同一“逻辑表 ID + 分区 ID”规则，并对照 Go `checksumContext.buildTasks`。
- 调整通道容量或把接收逻辑移动位置时，要保住“每任务一个结果”及无死锁不变量。若改为提前失败，还需明确是否取消剩余请求、是否仍关闭所有已创建的流，以及与 Go 的“收集全部结果、返回最后错误”兼容性。
- 修改结果流处理时必须保留 close 覆盖错误和后置钩子顺序，并扩展 `checksum_close_error_is_reported_after_every_stream_is_finalized`；还建议分别覆盖 `next_raw`、解码和 kill signal 失败。
- 修改并发解析时要考虑 Rust `usize` 与 Go signed 64-bit 的差异，并增加非法、零值、超大值测试；生产代码还应在 session variable 层保证合理上限，避免无界创建 OS threads。
- 修改输出列或顺序时需同步 SQL 层 schema、`ChecksumOutputChunk` 适配器和 `Next` 测试。多表顺序若需要稳定，必须显式排序，不能依赖 `HashMap` 遍历。
- 性能风险集中在任务数量（物理表数 × Public 索引数）、外层线程数、每请求 DistSQL 并发度及全量响应聚合；兼容风险集中在 request metadata、snapshot TS、range 类型、错误优先级和 Go parity。

## 验证依据

本说明基于以下直接证据：

- Rust 实现：`pkg/executor/checksum.rs` 全部 543 行；重点核对 `ChecksumBackend`、`ChecksumTableExec::{Open,Next,buildTasks,checksumWorker,handleChecksumRequest}`、`ChecksumContext` 的请求构造方法、`checksumRequestCount`、`getChecksumTableConcurrency` 与 `updateChecksumResponse`。
- Rust 独立测试：`pkg/executor/checksum_test.rs`，尤其是 `partitioned_table_checksum_builds_table_and_index_requests_per_physical_table`、`checksum_responses_xor_crc_and_sum_kv_totals`、`checksum_executor_matches_go_partitioned_table_result_and_request_contract`、`checksum_close_error_is_reported_after_every_stream_is_finalized`。
- crate 边界：`pkg/executor/Cargo.toml:1-17`、其中的 porting metadata 与依赖表；模块/测试接线：`pkg/executor/lib.rs:77-78,414-416`。
- Go 对照实现：`pkg/executor/checksum.go`；Go 生产入口：`pkg/executor/builder.go:763-777`；Go 回归：`pkg/executor/checksum_test.go:23-42`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query` 找到 Rust `ChecksumTableExec`（`:195`）、`ChecksumBackend`（`:138`）、`ChecksumContext`（`:382`）、`newChecksumContext`（`:390`）、`handleChecksumRequest`（`:335`）、`checksumRequestCount`（`:517`）和 `updateChecksumResponse`（`:539`）；`node pkg/executor/checksum.rs::ChecksumTableExec` 的 trail 显示测试导入及 `partitioned_executor` 实例化。精确 `callers/callees` 查询在 30 秒内未返回，因此调用边另由全仓 `rg` 复核，并未把超时结果当作“没有调用者”的依据。
- 全仓引用核对：Rust 侧除 `pkg/executor/checksum_test.rs` 外未发现本文件公开执行器/构造函数的使用；同名 `br/pkg/checksum/executor.rs::updateChecksumResponse` 属于另一模块，不能视为调用本文件。Go 侧确认 builder 的生产构造边。

本任务是纯文档分析，按计划未运行 Cargo 或代码测试。结构验收应使用任务指定命令，确认文件存在且恰好包含以上十一个固定二级标题；行为描述通过逐符号与 Go/Rust 独立测试人工交叉核对。
