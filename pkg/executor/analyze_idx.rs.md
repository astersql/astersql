# `pkg/executor/analyze_idx.rs` 逻辑说明

## 文件定位

本文件是 `astersql-executor` crate 中的索引统计收集实现，源码由 [`lib.rs`](lib.rs) 以公开模块 `analyze_idx` 导出（`lib.rs:47-50`），crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[package] name = "astersql-executor"` 与 `[lib] path = "lib.rs"` 确定（`Cargo.toml:1-8`）。它把“打开索引分析结果流、逐响应合并直方图和 Sketch、整理 v2 统计结果、关闭流”抽象成同步 Rust API。

当前接线状态需要与设计目标分开看：仓库内 Rust 生产代码没有调用 `analyzeIndexPushdown` 或 `analyzeIndexNDVPushDown`，直接引用仅见独立测试 [`analyze_idx_test.rs`](analyze_idx_test.rs)；因此它目前是公开、可测试的移植模块，但尚不能据此断言 Rust SQL 执行主链已经调用它。完整应用中的对应调度位置仍可在 Go 侧看到：普通索引任务由 [`analyze.go`](analyze.go) 的 `analyzeWorker` 调用 `analyzeIndexPushdown`（`analyze.go:914-933`），特殊索引 NDV 子任务由 [`analyze_col_sampling.go`](analyze_col_sampling.go) 的工作循环调用 `analyzeIndexNDVPushDown`（`analyze_col_sampling.go:505-527`）。

## 核心职责

1. 用 `AnalyzeIndexBackend`/`AnalyzeResultStream` 隔离真实存储请求与响应流，使核心合并逻辑不依赖具体 TiKV 或 DistSQL 类型（`analyze_idx.rs:281-300`）。
2. 对单列索引把非 NULL 主范围与 NULL 专用范围拆成两个流；对多列索引直接扫描包含 NULL 的全范围（`analyzeIndexPushdown`，`analyze_idx.rs:348-356`；`AnalyzeIndexExec::open`，`analyze_idx.rs:431-438`）。
3. 逐响应合并累计直方图、CMS、FM Sketch 和 TopN，限制桶数和 FM 集合大小，并把 TopN 频次从直方图中扣除（`buildStatsFromResult`、`updateIndexResult`、`merge_histograms`，`analyze_idx.rs:463-505,591-685`）。
4. 输出两种结果：完整 v2 索引统计 `analyzeIndexPushdown`，以及只保留 FM Sketch 和索引 ID 占位直方图的轻量 NDV 结果 `analyzeIndexNDVPushDown`（`analyze_idx.rs:347-393,548-588`）。
5. 将结果流读取、kill 检查、作业进度和流关闭错误显式纳入错误传播（`analyze_idx.rs:397-429,480-496,606-619`）。

本文件不负责构造真实 KV 请求、protobuf 解码、会话隔离级别、资源组标签或任务线程调度；这些能力在 Rust 中被压缩到后端 trait 边界，且当前仓库没有该 trait 的生产实现。Go 对照实现的真实请求构造位于 `AnalyzeIndexExec.fetchAnalyzeResult`（`analyze_idx.go:147-186`）。

## 主要符号

- 常量 `STATS_VERSION_2`、`MAX_SKETCH_SIZE`：前者把当前实现限定为统计版本 2；后者将 FM 哈希集合限制为 10,000 个元素（`analyze_idx.rs:26-29`）。
- 数据模型 `Datum`、`Range`、`IndexInfo`：分别表示可排序的统计值、扫描范围意图和索引元数据。`Range::{full,full_not_null,null}` 是三种范围构造器（`analyze_idx.rs:31-79`）。这些是本地简化模型，不是 Go `types.Datum`、`ranger.Range` 或 `model.IndexInfo` 的完整等价类型。
- `AnalyzeIndexError`：公开错误枚举，覆盖后端、取消、解码、版本、缺失结果和合并失败；本文件自身实际构造 `InvalidStatsVersion`、`MissingResult`、`Merge`，其余通常由后端返回（`analyze_idx.rs:81-96,202-205,403,469-470`）。
- `Bucket`、`Histogram`：桶的 `count` 是累计计数；`Histogram::total_row_count` 返回末桶累计数加 NULL 数，`remove_topn` 防止 TopN 与桶重复计数，`standardize_v2` 删除空桶（`analyze_idx.rs:98-163`）。
- `CMSketch`、`FMSketch`、`TopN`：CMS 只允许相同深宽合并；FM 合并后保留排序最小的至多 10,000 个哈希；TopN 按“频次降序、值升序”确定保留项，溢出项回写 CMS（`analyze_idx.rs:192-269`）。
- `AnalyzeIndexResponse`：一个后端响应的已解码统计载荷（`analyze_idx.rs:272-279`）。
- trait `AnalyzeResultStream`：同步的 `next_response`/`close` 生命周期接口，要求实现可在线程间转移（`Send`）（`analyze_idx.rs:281-285`）。
- trait `AnalyzeIndexBackend`：打开索引流、检查 kill、更新作业进度的注入点；要求 `Send + Sync + 'static`（`analyze_idx.rs:287-300`）。
- `AnalyzeIndexOptions`、`AnalyzeResult`、`AnalyzeResults`：分别承载合并参数、单组统计与顶层输出（`analyze_idx.rs:302-330`）。
- `AnalyzeIndexExec<B>`：有状态执行器，持有后端、表/索引信息、主/NULL 结果流、统计选项、版本、快照和并发度（`analyze_idx.rs:332-345`）。
- 公开入口 `analyzeIndexPushdown`、`analyzeIndexNDVPushDown` 和公开合并函数 `updateIndexResult`（`analyze_idx.rs:348,549,591`）。
- 内部辅助：`datum_cmp_bytes`、`merge_bucket_pairs`、`merge_histograms`，以及各统计结构的私有合并/整理方法（`analyze_idx.rs:130-269,624-685`）。

## 执行流程

完整统计路径如下：

1. `analyzeIndexPushdown` 根据 `idxInfo.column_count` 选择单列的 `full_not_null` 或多列的 `full`，然后调用 `buildStats(..., true)`（`analyze_idx.rs:348-356`）。
2. `buildStats` 调用 `open`。`open` 总是先开主流；单列且 `consider_null` 为真时，再用 `Range::null()` 开 NULL 流（`analyze_idx.rs:397-404,431-438`）。
3. `fetchAnalyzeResult` 决定是否传入快照，以 `isCommonHandle && idxInfo.primary` 标记公共句柄主索引，保证并发度至少为 1，并按 `null_range` 把流存入 `result` 或 `countNullRes`（`analyze_idx.rs:440-460`）。
4. `buildStatsFromResult` 每轮先调用 `backend.killed()`，再从流取一个已解码响应；每个响应交给 `updateIndexResult`（`analyze_idx.rs:463-497`）。
5. `updateIndexResult` 先按响应直方图行数更新作业进度，再合并直方图；完整路径还合并 CMS 与 TopN，所有路径都在存在 FM 数据时合并 FM（`analyze_idx.rs:591-619`）。
6. 流结束后，完整路径从直方图扣除 TopN、执行 v2 桶标准化并计算 CMS 默认值（`analyze_idx.rs:498-505`）。NULL 流只取末桶累计数写入主直方图的 `null_count`，最后写入真实索引 ID（`analyze_idx.rs:405-415`）。
7. `buildStats` 尝试关闭主流与 NULL 流；成功时返回统计，入口再构造 `AnalyzeResults`，其 `count` 为整理后的直方图总行数加 TopN 总频次（`analyze_idx.rs:417-428,368-385`）。

轻量 NDV 路径 `analyzeIndexNDVPushDown -> buildSimpleStats` 复用相同开流和逐响应循环，但传入 `need_cms = false`，因而跳过 CMS/TopN 合并、TopN 扣除与 CMS 默认值计算，只返回 FM Sketch 和可选 NULL 直方图。顶层结果放入一个只有索引 ID 的空直方图，并以 NULL 直方图末桶计数作为 `count`（`analyze_idx.rs:508-540,549-580`）。

`merge_histograms` 保持累计计数语义：相邻响应边界相同则合并接壤桶并把 NDV 减一；右侧桶追加前先用左侧末桶累计数做偏移；两侧平均桶大小相差至少两倍时先折叠较细一侧；最终持续两两折叠直至不超过 `max_buckets`（`analyze_idx.rs:624-685`）。

## 数据与状态

- `AnalyzeIndexExec.result` 与 `countNullRes` 是临时拥有的流。`buildStats`/`buildSimpleStats` 用 `take()` 把所有权移到局部变量，执行结束后 executor 字段回到 `None`；复用同一 executor 会重新开流（`analyze_idx.rs:403-404,515-516`）。
- `Histogram.buckets[*].count` 是累计值而非单桶值。合并右侧响应时必须加入左侧总数；同边界合并时还要扣除已吸收的右侧首桶偏移（`merge_histograms`，`analyze_idx.rs:634-679`）。
- 单列索引主流排除 NULL，NULL 行数从第二个流的末桶 `count` 回填；多列索引不定义独立 NULL 行，因而只使用一个全范围流（`analyze_idx.rs:351-355,407-413,434-436`）。
- `Histogram::remove_topn` 假设 TopN 的 `BTreeMap` 键顺序与 `datum_cmp_bytes` 的桶边界顺序兼容，使用饱和减法维护非负累计计数，并在 TopN 值等于桶上界时清零重复数（`analyze_idx.rs:130-152`）。
- CMS 合并要求维度完全一致；TopN 超容量的数据进入 CMS。FM 使用 `BTreeSet` 去重，超限时删除最大哈希（`analyze_idx.rs:200-215,223-237,250-269`）。
- `enable_snapshot` 仅控制是否把 `snapshot` 传给后端；`snapshot` 值仍会原样写入顶层结果（`analyze_idx.rs:342-344,446-453,382,577`）。
- `for_mv_or_global` 在完整路径对多值索引或全局索引置真；NDV 轻量路径始终为假（`analyze_idx.rs:383,578`）。

## 依赖与调用关系

RustCodeGraph 对索引中的关键边给出的直接证据包括：

- `analyzeIndexPushdown -> AnalyzeIndexExec::buildStats -> open -> fetchAnalyzeResult/open_index_result`；结果整理还调用 `Histogram::total_row_count` 与 `TopN::total_count`（图中入口位于 `analyze_idx.rs:348`）。
- `AnalyzeIndexExec::buildStatsFromResult -> updateIndexResult`；`updateIndexResult -> update_job_progress/merge_histograms/CMSketch::merge/TopN::merge/FMSketch::merge`（图中被调边位于 `analyze_idx.rs:464,591`）。
- `analyzeIndexNDVPushDown -> AnalyzeIndexExec::buildSimpleStats`（图中入口位于 `analyze_idx.rs:549`）。

源文件的直接 Rust 库依赖只有标准库 `BTreeMap`、`BTreeSet` 与格式化 trait（`analyze_idx.rs:23-24`）。`Cargo.toml` 表明它归属依赖面很广的 `astersql-executor` crate，但本文件没有直接引用这些 workspace crate；不能把 crate 的全部依赖误认为该模块的实际调用依赖。`nextgen` 是 crate 级 feature，`lib.rs` 对 `analyze_idx` 的声明没有 `cfg(feature = ...)`，因此该模块不受该 feature 门控（`Cargo.toml:10-11`；`lib.rs:47-50`）。

上游方面，RustCodeGraph 与 `rg` 都未发现 Rust 生产调用者，只有测试模块通过 `crate::analyze_idx` 使用类型和 `buildStats`。对应 Go 主链则是 `analyzeWorker -> analyzeIndexPushdown` 和特殊索引子任务 worker `-> analyzeIndexNDVPushDown`。下游真实 DistSQL 请求在 Go 的 `fetchAnalyzeResult -> distsql.Analyze`；Rust 目前只通过 `AnalyzeIndexBackend::open_index_result` 描述该边界，生产适配器尚未验证存在。

## 错误处理与边界

- 非 v2 版本在 `buildStatsFromResult` 和 `updateIndexResult` 立即返回 `InvalidStatsVersion`；顶层入口把执行错误放进 `AnalyzeResults.error`，而不是返回外层 `Result`（`analyze_idx.rs:359-390,469-470,582-585,603-605`）。
- 主流缺失会返回 `MissingResult`。后端开流、kill、取响应与关闭错误都用 `?` 传播；CMS 深宽不一致返回 `Merge("CMS dimensions differ")`（`analyze_idx.rs:202-205,403,447-454,481-483`）。
- `buildStats` 和 `buildSimpleStats` 即使统计构建失败也会调用两个流的 `close`，但处理错误优先，关闭错误在该分支被忽略；构建成功时主流关闭错误优先于 NULL 流关闭错误（`analyze_idx.rs:405-428,517-539`）。独立测试 `build_stats_closes_all_results_when_main_close_fails` 验证主流关闭报错时 NULL 流仍被关闭（`analyze_idx_test.rs:135-172`）。
- 与 Go 的一个明确差异是：Go `open` 在第二次 NULL 请求失败时会立即关闭并清空已打开的主流，并用 `errors.Join` 合并错误（`analyze_idx.go:130-143`）；Rust `open` 对第二次调用直接 `?` 返回，主流仍留在 `self.result`，此路径的及时关闭尚无测试证据。扩展生产后端前应补齐并验证这一资源错误路径。
- `saturating_add`/`saturating_sub` 防止计数整数溢出或下溢，但这也意味着异常大或不一致输入会被钳位而非报错（例如 `total_row_count`、TopN 扣除和合并偏移，`analyze_idx.rs:122-126,144-151,632-678`）。
- `bucket_count` 被提升到至少 1，并发度也被提升到至少 1；`topn_count = 0` 合法，所有 TopN 值会溢出到 CMS（`analyze_idx.rs:453,607,251-268`）。
- 文件定义了 `Cancelled`、`Decode` 等错误，但没有在核心代码中自行完成字节解码；响应已经是 `AnalyzeIndexResponse`，解码失败只能由后端边界表达。

## 并发与资源生命周期

本模块没有创建线程、异步任务、锁或通道，处理循环是同步的。并发只表现为传给后端的请求参数 `concurrency.max(1)`；后端是否并行发请求、如何取消和等待，由 trait 实现负责（`analyze_idx.rs:441-454`）。

`AnalyzeIndexBackend: Send + Sync + 'static` 允许后端被并发执行环境安全持有，`AnalyzeResultStream: Send` 允许结果流在线程之间转移，但 `&mut AnalyzeIndexExec` 和同步 `next_response` 保证单次合并过程串行修改执行器与累积统计。每取一条响应前检查 `backend.killed()`，因此取消粒度受 `next_response` 阻塞时间约束；本文件没有上下文对象去中断一个正在阻塞的读取（`analyze_idx.rs:281-300,480-484`）。

资源生命周期是“打开主流，可能再打开 NULL 流，取走字段所有权，读取，关闭”。当前关闭不是 RAII `Drop` 守卫：它依赖 `buildStats`/`buildSimpleStats` 的显式尾部逻辑；尤其是第二个流打开失败时的主流清理差异需要生产接线前处理。测试后端通过 `Arc<AtomicBool>` 证明正常尾部逻辑会尝试关闭两条流（`analyze_idx_test.rs:13-17,87-132,135-172`）。

## 与 Go 版本的对应关系

Rust 的符号基本按 [`analyze_idx.go`](analyze_idx.go) 同名移植：`AnalyzeIndexExec`、`analyzeIndexPushdown`、`buildStats`、`open`、`fetchAnalyzeResult`、`buildStatsFromResult`、`buildSimpleStats`、`analyzeIndexNDVPushDown`、`updateIndexResult` 均能逐一对应（Go `analyze_idx.go:42-366`；Rust `analyze_idx.rs:332-620`）。主要保持的语义包括：

- 单列索引主扫描排除 NULL，另开 NULL 范围；多列索引走全范围。
- 只支持 v2 统计，合并后扣除 TopN、标准化 v2 直方图、计算 CMS 默认值。
- 每个响应更新作业进度，合并直方图、CMS/TopN 与 FM Sketch。
- NDV 路径只需要 FM，并用空直方图携带索引 ID。
- 构建结束关闭主/NULL 两个结果流。Rust 独立测试专门验证了 Go `closeAll` 的“两者都尝试关闭”意图（`analyze_idx_test.rs:135-172`）。

已确认的简化或差异如下：

- Go 构造完整 `kv.Request`，选择 RC/SI、startTS、keep-order、资源组和请求来源并调用 `distsql.Analyze`（`analyze_idx.go:147-186`）；Rust 把这些行为缩成 `open_index_result` 的索引、范围、公共句柄、NULL 标志、可选快照和并发参数，无法表达其余请求属性。
- Go 从 protobuf 原始字节解码 `tipb.AnalyzeIndexResp`，并使用共享 `statistics` 实现合并（`analyze_idx.go:188-256,321-365`）；Rust 使用本文件的简化统计结构和已解码响应。Rust 的 `FMSketch::ndv` 直接返回集合大小，只是本地模型，不应解读为完整 FM 估计算法。
- Go 对缺失 CMS 响应记录警告后继续（`analyze_idx.go:351-360`）；Rust 对 `None` 静默跳过。Go 的统计合并接收 statement context，并可能返回更丰富的合并错误；Rust 是本地确定性桶折叠。
- Go 在 NULL 流打开失败时关闭主流；Rust 尚未对齐该路径。Go 还包含 failpoint、基于 context cause 的取消归一化和慢任务注入（`analyze_idx.go:188-240`），Rust trait 只提供 `killed()`。
- Go 的全局索引标志条件显式要求 v2；Rust 因入口已拒绝非 v2，成功分支直接使用 `multi_valued || global`，成功语义等价（Go `analyze_idx.go:97-99`；Rust `analyze_idx.rs:359-367,383`）。

相关 Go 回归 `TestFailedAnalyzeRequestV2` 通过 `buildStatsFromResult` failpoint 验证错误能从 `ANALYZE TABLE ... INDEX` 返回（`test/analyzetest/analyze_test.go:280-304`）；Rust 没有端到端 SQL 接线测试，只有模块级独立测试。

## 扩展指南

- 接入 Rust 生产主链时，应在 executor 的 ANALYZE worker 或等价调度处构造 `AnalyzeIndexExec`，为 `AnalyzeIndexBackend` 提供真实 DistSQL/KV 适配器，并分别接入完整统计与特殊索引 NDV 两个入口。必须同步验证会话取消原因、快照隔离、资源组、keep-order、请求来源、公共句柄范围和作业对象，不能仅凭 trait 现有参数假定已对齐 Go。
- 扩展请求参数优先修改 `AnalyzeIndexBackend::open_index_result` 与 `fetchAnalyzeResult`，并在独立 [`analyze_idx_test.rs`](analyze_idx_test.rs) 增加记录参数的 fake backend；不要把测试嵌入生产 `.rs` 文件。
- 修改直方图算法时，以 `merge_histograms`、`merge_bucket_pairs`、`Histogram::{remove_topn,standardize_v2}` 为主要入口。需覆盖相同边界桶、累计计数偏移、奇数桶、桶上限、空输入、TopN 恰等于桶上界和饱和算术，并与 Go `statistics.MergeHistograms`/`StandardizeForV2AnalyzeIndex` 的实际结果对照。
- 修改 Sketch/TopN 时，以 `CMSketch::merge`、`FMSketch::merge`、`TopN::merge` 为入口，保持 CMS 维度不变量、稳定的 TopN 排序规则和 FM 容量上限；注意当前结构是移植模型而非共享统计 crate 类型。
- 修改资源处理时，应先补“NULL 流打开失败仍关闭主流”“读取/kill/合并失败仍关闭两流”“两次关闭均失败时错误优先级”的回归测试，再调整 `open`、`buildStats` 和 `buildSimpleStats`。Go `open` 与 `closeAll` 是直接对照依据。
- 增加错误或取消行为时，后端返回值应映射到 `AnalyzeIndexError`；如需对齐 Go context cause 或阻塞读取取消，应扩展流接口，而不是只在循环顶部增加检查。
- 任何 Rust 行为修改都应同步更新独立测试文件；本文件不应内嵌 `#[cfg(test)]` 测试。生产接线完成后还需要 SQL 层测试证明 Rust 路径真实可达。

兼容风险主要来自统计格式和累计计数错误（会影响优化器估算）、NULL 范围差异、TopN 双计数、全局/多值索引标志，以及取消/关闭泄漏；性能风险主要来自响应数乘以桶合并成本、TopN 全量排序、FM `BTreeSet` 操作和后端并发度。

## 验证依据

- 源码全量阅读：[`analyze_idx.rs`](analyze_idx.rs)（685 行）。公开 API、内部实现、常量和无条件编译状态均按源码逐项核对。
- crate 与模块边界：[`Cargo.toml`](Cargo.toml)（`package/lib/features`，`Cargo.toml:1-15`）和 [`lib.rs`](lib.rs)（`lib.rs:35-52`）。目标包未发现 `doc.go`，因此没有额外的 Go 包契约文件可读。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`explore "pkg/executor/analyze_idx.rs analyzeIndexExec AnalyzeIndexExec"`；`query analyzeIndexPushdown/analyzeIndexNDVPushDown/updateIndexResult`；以及对 Rust `analyzeIndexPushdown`、`analyzeIndexNDVPushDown`、`updateIndexResult` 的 `node` 查询。图确认 `buildStatsFromResult -> updateIndexResult` 等内部边；路径 `files --filter pkg/executor/analyze_idx` 未命中，故按技能规则用源码和 `rg` 补核文件级引用。
- Rust 上游引用检查：`rg` 只找到 `lib.rs` 模块声明与 [`analyze_idx_test.rs`](analyze_idx_test.rs) 测试引用，没有找到 Rust 生产调用者。
- Go 对照与调用边：[`analyze_idx.go`](analyze_idx.go)（完整实现），[`analyze.go`](analyze.go) 的 `analyzeWorker`（`analyze.go:910-951`），[`analyze_col_sampling.go`](analyze_col_sampling.go) 的特殊索引 worker（`analyze_col_sampling.go:505-542`）。
- 独立 Rust 测试：[`analyze_idx_test.rs`](analyze_idx_test.rs) 共三项，验证关闭两流、TopN 扣除与 v2 空桶标准化、跨响应累计计数偏移（`analyze_idx_test.rs:135-242`）。本任务按计划不运行 Cargo，因此这些是测试源码证据，不是本次动态测试结果。
- Go 相关测试：[`test/analyzetest/analyze_test.go`](test/analyzetest/analyze_test.go) 的 `TestFailedAnalyzeRequestV2`（`analyze_test.go:280-304`）验证 Go 端合并错误传播到 SQL 调用者。
- 结构验证使用任务指定命令，要求文件存在且恰有 11 个固定二级标题；最终结果及退出码在任务交付时记录。
