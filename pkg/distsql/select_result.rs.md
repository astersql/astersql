# `pkg/distsql/select_result.rs`

## 文件定位

本文件属于 `astersql-distsql` crate，由 [`pkg/distsql/lib.rs`](lib.rs) 以公开模块 `select_result` 导出。它位于 DistSQL 请求发送与上层执行消费之间：[`pkg/distsql/distsql.rs`](distsql.rs) 的 `Select`、`SelectWithRuntimeStats`、`Analyze`、`Checksum` 和 `GenSelectResultFromMPPResponse` 先从 `KvClient` 得到 `ResponseSource`，再构造本文件的 `selectResult`；调用方随后通过 `SelectResult` trait 拉取原始字节或行，并在关闭时归集协处理器运行时统计。

这是一个明确标注为“精简版”的 Rust 移植。它已经接入 Rust DistSQL 发送入口和 `astersql-util-execdetails` 统计集合，但并不等同于 Go [`pkg/distsql/select_result.go`](select_result.go) 的完整实现：Rust 使用仓库内简化的 `SelectResponse`/`ResponseSource`，没有在本文件中完成 protobuf 解码、真实 TiDB `chunk.Chunk` 解码、SQL killer、内存 tracker、telemetry、backoff/RPC 明细和完整 intermediate-output 通道处理。

crate 边界由 [`pkg/distsql/Cargo.toml`](Cargo.toml) 确认：本文件直接依赖本 crate 的响应/错误类型、`astersql-config` 的全局 copr-cache 配置，以及 `astersql-util-execdetails` 的执行统计类型；这些依赖均为工作区路径依赖。文件没有条件编译项。

## 核心职责

1. 定义结果消费协议：`SelectResult` 支持 `NextRaw`、批量 `Next`、所有权转换 `IntoIter`、`Close` 和可选并发度；`SelectResultIter` 支持逐行 `Next` 与 `Close`。
2. 用泛型 `selectResult<S: ResponseSource>` 把响应源转换成两种互斥消费视图：`VecDeque<Vec<u8>>` 原始数据队列和 `VecDeque<Row>` 标量行队列，并保证同一行只被其中一种接口消费一次。
3. 在消费响应和关闭响应源时，把扫描、耗时、read-pool、execution summary、限流等待及 Analyze 扫描字节等证据合并进 `SyncExecDetails` 与 `RuntimeStatsColl`。
4. 提供两种组合结果：`serialSelectResults` 按输入顺序串接结果，`sortedSelectResults` 假设每路输入已经有序并执行多路归并。
5. 提供 `selectResultRuntimeStats` 的合并、缓存命中率计算、文本格式化和 `execdetails::RuntimeStats` trait 适配。

本文件不发送 KV 请求；传输边界是 `ResponseSource`。它也不拥有 SQL 计划或表达式求值器；排序只操作已经物化为 `Scalar` 的行。

## 主要符号

- `ResultStatsContext`：统计写入上下文。`exec_details` 接收语句级 cop/read-pool 明细，`runtime_stats` 是加锁共享的 plan 统计集合；`root_plan_id`、`cop_plan_ids`、`store_type` 决定归属；`is_analyze`/`collect_raw_details` 选择 Analyze 关闭路径；`mpp_reports_directly` 用于避免 TiFlash MPP 双重上报。
- `Scalar` 与 `Row`：当前 Rust 行模型。`Scalar::compare` 对同类型值做自然比较，浮点使用 `total_cmp`，`Null` 小于非空；不同类型通过 Debug 字符串比较。这是简化语义，不是 TiDB 完整类型系统或排序规则。
- `ByItem`：排序列下标和降序标志。越界列被 `compare_rows` 当作“缺失值”，其顺序类似空值。
- `SelectResult`：面向批量/原始读取的公开 trait。`concurrency` 默认返回 `None`，因此包装实现可明确表示是否暴露并发度。
- `SelectResultIter` / `SelectResultRow`：逐行结果协议；`channel` 保留 Go intermediate-output 通道概念。当前通用 `selectResultIter` 固定返回通道 `0`。
- `GetSelectResultConcurrency`：只转发 trait 的 `concurrency`。`selectResult` 返回 `(concurrency, extra_concurrency)`，其中当前构造函数只设置主并发度，额外并发度初始为 `0`。
- `selectResult<S>`：核心状态机。`source` 保存响应源；`buffered`/`raw` 保存行和字节的双视图；`closed` 与 `close_error` 保证显式关闭幂等并重放第一次关闭错误；`runtime_stats_observed` 控制是否注册 plan 统计。
- `fetchResp` / `consume_response`：拉取和解释一份 `SelectResponse`。前者处理“响应与错误同时存在”的边界，后者累计计数、统计和数据缓冲。
- `record_cop_evidence`：将一份 `CopRuntimeEvidence` 归入 TiKV、TiFlash、Analyze 或未消费响应对应的统计路径，并检查 execution summaries 是否完整。
- `selectResultIter`：核心结果的逐行适配器，每次用容量 `1` 调用 `SelectResult::Next`。
- `serialSelectResults` / `NewSerialSelectResults`：串行聚合器。当前输入耗尽后移动到下一个输入；`Close` 关闭全部输入并返回最后一个关闭错误。
- `sortedSelectResults` / `NewSortedSelectResults`：有序多路归并器。构造时把每个输入转换为 `SelectResultIter`，`initialize` 每路预取一行，`next_row` 线性扫描所有 head 选最小值并补充对应输入。
- `selectResultRuntimeStats`：响应数、告警数、扫描 key、cop 响应耗时、processed keys、处理/等待时间、缓存命中、store batching、limiter wait 和简化 RPC 命令计数的聚合体。
- `FillDummySummariesForTiFlashTasks`：返回尚未出现在 `recorded` 中的 plan ID 列表。与 Go 同名函数直接向统计集合写入零值 summary 的行为不同，Rust 调用点的补写实际由 `record_cop_evidence` 内部完成；该公开辅助函数自身只做集合差集。

## 执行流程

普通读取主链如下：

1. [`pkg/distsql/distsql.rs`](distsql.rs) 的发送函数通过 `KvClient::send` 或 `send_with_options` 得到 `Box<dyn ResponseSource>`，调用 `selectResult::new`，需要统计时再调用 `with_stats_context`。
2. `Next` 或 `NextRaw` 先消费对应队列；队列为空时调用 `fetchResp`。
3. `fetchResp` 调用 `ResponseSource::next_response_with_error`。如果同时收到响应和传输错误，先以 `include_data = false` 调用 `consume_response`，保留计数和 cop 证据但不暴露响应数据，再返回传输错误。
4. 正常响应进入 `consume_response`：先增加 `response_count`、`scanned_keys`、`warning_count`，按 MPP 路由记录 TiFlash summaries，再通过 `record_cop_evidence` 合并 cop 统计；随后写入 `raw_data`，检查嵌入式 `response.error`，最后把字符串行同时编码为制表符分隔字节并转换为 `Scalar::String` 行。
5. `NextRaw` 弹出一个 raw item 时同步丢弃一个 `buffered` 行；`Next` 弹出一行时同步丢弃一个 `raw` item。因此交替调用两种接口不会重复返回同一行。独立 `raw_data` 没有对应的行队列项。
6. 响应源返回空，或 `consume_response` 表示没有消费到响应时，`fetchResp` 调用 `Close` 并返回流结束。

统计分支由 `record_cop_evidence` 决定：

- 没有 `ResultStatsContext`，或 Analyze 未开启 `collect_raw_details` 时，不写 plan 统计。
- 始终可将 cop details/read-pool details 合入 `SyncExecDetails`；有 `RuntimeStatsColl` 时还累计本地 `selectResultRuntimeStats`。
- Analyze 把统计记到 `root_plan_id`，并累计 `raw_scan`，供关闭时估算扫描字节。
- 未消费的响应只把通用 cop 统计归到最后一个 cop plan，不伪造 execution-summary 期望，也不重放最后一次已消费响应的 summary。
- TiKV 消费响应先登记每个 cop plan 的 summary 期望；空 summary 仍保留根 cop 的扫描/时间证据，长度或必填字段不合法则把本次计划摘要标为无效。
- 带 `ExecutorId` 的 TiFlash summary 走稀疏 ID 路径：先记录有效 summary，再为缺失 plan 写零值占位；无 ID 时按 `cop_plan_ids` 的位置配对。

组合器流程：`serialSelectResults` 只有当当前子结果未产生数据才递增 `current`；`sortedSelectResults` 惰性预取每路 head，每输出一行只推进获胜输入。后者每行执行一次对所有 head 的 `min_by`，复杂度约为 `O(输出行数 × 输入路数 × 排序键数)`，并非 Go 版本的堆式 `O(log 输入路数)` 推进。

## 数据与状态

`selectResult` 的关键不变量是 `source` 在关闭前存在，`closed` 一旦置位不再恢复，且 `buffered` 与由行派生的 `raw` 保持相同消费进度。`fetchResp` 因此可以安全地用 `source.as_mut().expect("source exists until close")`；当前 `Close` 并不把 `source` 置为 `None`，但关闭后 `fetchResp` 会在访问源之前返回 `false`。

`runtime_stats` 是结果对象本地的可克隆累计值；共享 `RuntimeStatsColl` 通过 `Arc<Mutex<_>>` 更新。`cop_response_times` 与 `processed_keys` 按同一次证据同步追加，`mergeCopRuntimeStatsWithDetails` 先新增响应槽位，再覆写刚追加的 processed-key 值，所以格式化时两者长度应一致。`Merge` 会逐字段累加，并按 RPC 命令名合并 `request_stats`。

`calcCacheHit` 的分母是 `response_count + store_batched_num`，分子是 `cop_cache_hit_num`；分母为零时返回 `0.0`。`Display` 会复制并排序耗时/key 样本来求 min/max/平均/p95，因此格式化本身不改变原统计，但会产生与样本数成正比的临时内存和 `O(n log n)` 排序成本。只有存在 cop 响应样本时才输出 `cop_task` 主体和 limiter wait；全局配置决定显示命中率还是 `copr_cache: disabled`。

`sortedSelectResults` 保存每路迭代器和一个 `Option<Row>` head，空间规模约为输入路数乘单行大小。它假设每个输入已经按相同 `order` 排好；本文件不验证该前置条件。相等 key 没有额外稳定性键，`min_by` 的平局选择不应被依赖为跨输入稳定顺序。

## 依赖与调用关系

上游生产调用关系由 RustCodeGraph 与源码共同确认：

- `pkg/distsql/distsql.rs::new_select_result` 构造 `selectResult<Box<dyn ResponseSource>>`，`DistSQLSelectResult` 委托实现本文件的 `SelectResult`。
- `pkg/distsql/distsql.rs::{Select, SelectWithRuntimeStats, Analyze, Checksum, GenSelectResultFromMPPResponse}` 是当前 Rust 发送侧入口；其中带统计的入口填充 `ResultStatsContext`。
- `pkg/distsql/distsql_test.rs` 通过 `GetSelectResultConcurrency` 验证这些包装器保留并发度。

下游依赖包括：

- `ResponseSource::{next_response_with_error, close, collect_unconsumed_cop_stats, limiter_wait_stats}`：数据、关闭与尾部统计的唯一传输接口，类型定义位于 [`pkg/distsql/lib.rs`](lib.rs)。
- `SelectResponse` 与 `CopRuntimeEvidence`：简化响应内容和 cop 证据，亦定义于 crate 根。
- `astersql_util_execdetails::execdetails`：`SyncExecDetails`、`RuntimeStatsColl`、`ScanDetail`、`ExecutorExecutionSummary`、统计格式化及 `RuntimeStats` trait。
- `astersql_config::get_global_config`：仅在统计字符串格式化时读取 copr-cache 开关。
- 标准库 `Arc<Mutex<_>>`、`VecDeque`、`HashMap`、`HashSet`、`Duration`：共享统计、FIFO 缓冲、summary 去重和时长累计。

RustCodeGraph 对公开串行/排序构造函数没有找到生产调用者；它们的直接事实证据主要来自 [`pkg/distsql/select_result_test.rs`](select_result_test.rs)。因此应表述为“已实现并有独立测试”，不能据此断言当前 Rust SQL 主链已经使用分区归并。图索引最初把本文件列为由测试和 `pkg/expression/aggregation/base_func.rs` 使用，但源码搜索进一步确认核心 `selectResult` 的真实生产接线位于 `pkg/distsql/distsql.rs`；这里以精确导入和构造代码为准。

## 错误处理与边界

- 传输错误：`fetchResp` 返回 `next_response_with_error` 的错误。若同时存在响应，只记录统计而不加入数据队列，避免错误响应中的数据被上层消费。
- 嵌入式响应错误：`consume_response` 在已保存独立 `raw_data` 后检查 `response.error`，调用 `Close` 再返回 `DistSqlError`；行数据不会入队。调用者不应假定错误后 raw 缓冲可继续读取，因为 `Close` 会清空队列。
- 空响应源：触发关闭并表现为 `NextRaw -> None` 或 `Next` 不再添加行。
- 锁中毒：对 `RuntimeStatsColl` 的 `Mutex::lock` 使用 `expect`，会 panic，而非转换为 `DistSqlError`。
- `serialSelectResults::IntoIter` 与 `sortedSelectResults::IntoIter` 明确返回 `"not implemented"`；`sortedSelectResults::NextRaw` 明确返回不支持错误。Go 的 sorted `NextRaw` 是 panic，Rust 选择了可传播错误。
- `serialSelectResults::Close` 尝试关闭所有输入并返回最后一个错误；`sortedSelectResults::Close` 在首个输入关闭错误处提前返回，后续输入不会被关闭。这一点与串行聚合器不同，扩展时必须保留或有意识地修改其契约。
- `sortedSelectResults::compare_rows` 对列越界不报错，并允许跨类型 Debug 字符串比较；这与 Go 版本基于 schema/type compare function 的 SQL 排序语义不等价。
- `Scalar::Float` 可表示 NaN，使用 `total_cmp` 提供总序；跨类型顺序依赖 Rust Debug 表示，不应作为持久兼容协议。
- `capacity` 小于或等于当前 `rows.len()` 时 `Next` 不做工作；调用方负责提供合理容量并判断是否有新增行。

## 并发与资源生命周期

`SelectResult` 和 `SelectResultIter` 都要求 `Send`，允许所有权在线程间移动，但接口方法需要 `&mut self`，单个结果不支持无锁并发消费。共享统计用 `Arc<Mutex<RuntimeStatsColl>>` 串行化更新；`mpp_reports_directly` 回调要求 `Send + Sync`，消费响应时同步调用。

显式 `Close` 是主要资源回收点：它只执行一次源关闭，清空缓冲，关闭源，关闭后收集未消费 cop 统计与 limiter wait，按上下文注册 runtime stats，并缓存源关闭错误。后续调用返回相同结果。测试 `go_merge_42_close_returns_first_close_error_on_every_call` 验证源只关闭一次且错误被重放。

`Drop for selectResult` 是兜底：若对象未显式关闭，它会调用 `source.close()`，但忽略错误，也不会执行 `Close` 中的未消费统计收集、limiter wait 合并、Analyze 扫描字节估算和 runtime stats 注册。因此需要完整统计的调用方必须显式 `Close`；不能依赖析构替代正常关闭。

串行聚合器没有独立 `closed` 标志，重复 `Close` 会再次调用每个子结果的 `Close`，是否真正重复释放由子实现保证。排序聚合器自身用 `closed` 保证只关闭一次。构造排序结果时若某个 `IntoIter` 失败，已经成功转换并移入局部 `inputs` 的迭代器会被丢弃；是否充分释放仍取决于具体迭代器的析构实现。

## 与 Go 版本的对应关系

对照文件是 [`pkg/distsql/select_result.go`](select_result.go)，Rust 保留了主要名称和高层意图：`SelectResult`/`SelectResultIter`、串行拼接、有序归并、`selectResult` 拉取与关闭、execution summaries 归集、runtime stats 合并/显示及 cache-hit 计算。独立 Rust 测试中的 `go_merge_42_*` 用例特别覆盖了缺失/畸形 summary、TiFlash executor ID、read-pool details、limiter wait 和关闭错误重放。

仍存在重要语义差异：

- Go 消费真实 `kv.Response`/protobuf `tipb.SelectResponse`，并按 Default/Chunk 两种编码解码到 TiDB `chunk.Chunk`；Rust 接收已简化的字符串行或 `raw_data`，所有行转为 `Scalar::String`。
- Go API 接受 `context.Context`，会检查 SQL killer，并记录 fetch duration、metrics、telemetry、内存占用、backoff、region request stats、CPU/RU 和跨可用区流量；Rust API 无取消上下文，统计字段和副作用均较少。
- Go 的 `IntoIter` 支持多个 intermediate outputs，并倒序消费通道以优先返回更完整的最终数据；Rust 的核心迭代器只有固定通道 `0`，不解析 intermediate outputs，也没有 Go 的“转换后原结果失效”状态检查。
- Go 排序结果使用带类型比较函数的 heap 和内存 tracker；Rust 用 `Scalar` 的简化比较与线性 head 扫描。Go 构造器声明主要用于分区表；Rust 当前没有生产调用证据。
- Go `GetSelectResultConcurrency` 仅对底层 `*selectResult` 且响应实现 `copr.CopInfo` 时成功；Rust 把能力放进 trait，因此包装器可以委托暴露构造时保存的并发度。
- Go `FillDummySummariesForTiFlashTasks` 直接向 `RuntimeStatsColl` 写零 summary；Rust 同名函数只返回缺失 ID。Rust 的真实补写发生在 `record_cop_evidence` 的 executor-ID 分支。
- Go sorted close 遇到第一个错误即返回，并回收已遍历输入的内存；Rust同样首错返回，但没有内存 tracker。Go serial close 与 Rust一致，遍历全部输入并保留最后错误。
- Go runtime stats 还包含并发度、build/fetch duration、backoff 和更丰富 RPC 统计；Rust `request_stats` 只是命令到次数的 map，且本文件没有填充路径，只支持克隆/合并已提供的数据。

因此，本文件适合被描述为 Go 结果消费与统计主干的可运行子集，而不是功能等价完成版。

## 扩展指南

- 增加真实类型/编码支持时，优先扩展 `Scalar`、响应模型及 `consume_response`，并避免用 Debug 字符串定义跨类型 SQL 顺序；若接入 TiDB 类型系统，应同步替换 `compare_rows` 的比较依据。
- 增加 intermediate outputs 时，应扩展 `SelectResultRow::channel` 的产生路径和 `selectResultIter` 状态机，明确主结果与中间通道顺序，并参考 Go `selRespChannelIter`/`selectResultIter` 的倒序通道规则。相关测试必须放在独立的 [`pkg/distsql/select_result_test.rs`](select_result_test.rs)，不要内嵌回生产文件。
- 修改统计归属时，集中审查 `record_cop_evidence` 与 `Close`：TiKV summary 期望、畸形失效、TiFlash ID/位置两条路径、未消费响应不重放 summary、Analyze 扫描字节都是必须保留的不变量。
- 新增 `selectResultRuntimeStats` 字段时，必须同步更新 `Clone` 派生适用性、固有 `Merge`、`Display`、`execdetails::RuntimeStats::{Merge, CloneBox}` 以及独立测试；计数字段需明确取最大值还是累加，不能机械照搬。
- 修改关闭逻辑时，验证显式关闭幂等、第一次错误重放、源只关闭一次、未消费统计在源关闭后收集、Drop 仅作为兜底。若希望组合器全部执行 best-effort close，应分别处理 sorted 的首错提前返回和 serial 的末错返回差异。
- 优化排序性能时，可把 `heads` 的线性选择替换为堆，但必须保留降序、多键、空/缺列和相等 key 语义，并为每路补充/关闭失败编写回归测试。
- 若新增上层使用点，应从 `pkg/distsql/distsql.rs` 的 `DistSQLSelectResult` 委托或明确的组合器构造入口接线，并用 RustCodeGraph/源码搜索确认调用边；不要仅因 Go 版本有调用就宣称 Rust 已接线。
- 对齐 Go 完整语义时要把未移植项拆成独立工作：上下文取消、protobuf/chunk 解码、内存/metrics/telemetry、intermediate outputs、完整请求统计。一次局部修改不应把这些差异隐式视作已解决。

## 验证依据

- RustCodeGraph `status`：索引覆盖 11,467 个文件，其中 Rust 7,032 个；本文件被识别为 854 行、75 个符号。
- RustCodeGraph `node --file pkg/distsql/select_result.rs`：逐段读取全部 1–854 行，核对 traits、结构体、所有 impl、排序/串行流程、统计与关闭实现；未发现条件编译项。
- RustCodeGraph `callees NewSortedSelectResults`：Rust 构造器调用各输入的 `IntoIter` 并实例化 `sortedSelectResults`；同名 Go 构造器还初始化比较函数、key columns 和 heap。
- RustCodeGraph `callees fetchResp` / `callees consume_response`：确认 `fetchResp -> next_response_with_error -> consume_response`，以及 `consume_response -> record_cop_evidence` 的核心下游边。泛型/trait 动态分派使若干 `callers` 查询没有返回结果，因此又以精确导入和构造源码核对上游。
- [`pkg/distsql/distsql.rs`](distsql.rs)：核对 `DistSQLSelectResult` 的 trait 委托，以及 `Select`、`SelectWithRuntimeStats`、`Analyze`、`Checksum`、`GenSelectResultFromMPPResponse` 的生产构造与统计上下文接线。
- [`pkg/distsql/lib.rs`](lib.rs)：核对公开模块边界及 `DistSqlError`、`DistSqlResult`、`StoreType` 等 crate 根类型；[`pkg/distsql/Cargo.toml`](Cargo.toml) 核对 crate 名称、lib 入口和直接工作区依赖。
- [`pkg/distsql/select_result.go`](select_result.go)：读取接口、sorted/serial、核心 fetch/Next/NextRaw、summary 归集、关闭、通道迭代和 runtime stats 实现，逐项区分 Rust 已有行为与未移植能力。
- [`pkg/distsql/select_result_test.rs`](select_result_test.rs)：核对 raw/row 共享消费、串行推进、升序归并、通道 0、cache ratio、summary 校验、read-pool 合并、Close 幂等/错误重放及组合器 `IntoIter` 不支持等边界。
- [`pkg/distsql/select_result_test.go`](select_result_test.go)：核对 Go 的未消费统计关闭顺序、limiter wait、intermediate-output 多通道、转换后失效和 MPP summary 路由等对照语义。
- 未运行 Cargo 或代码测试：本任务是纯文档分析，任务计划明确禁止 Cargo；交付只执行文档结构验证和人工事实复核。
