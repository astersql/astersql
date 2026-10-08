# [`pkg/util/execdetails/tiflash_stats.rs`](./tiflash_stats.rs)

## 文件定位

本文件是 TiFlash 执行统计的 Rust 实现，对应 Go 文件 `pkg/util/execdetails/tiflash_stats.go`。它不是顶层 crate 入口：`pkg/util/execdetails/internal/ruv2/lib.rs` 在私有模块 `tiflash_stats` 中以 `include!("../../tiflash_stats.rs")` 编入源码，再用 `pub use tiflash_stats::*` 导出。因此这些类型实际属于 `astersql-util-execdetails-ruv2` crate；顶层 `astersql-util-execdetails` 又通过 `lib.rs` 的 `execdetails_ruv2::*` 间接提供相关 API。

该文件位于 TiFlash 返回的 `tipb::ExecutorExecutionSummary` 与 TiDB 侧运行时统计/资源计量之间：它把 protobuf 中的扫描、等待、网络和 RU 字段解码为可合并的本地结构，并提供诊断字符串。直接消费方可见于 `pkg/util/execdetails/runtime_stats.rs`；TiFlash MPP report 的 RU 汇总入口则沿 `pkg/executor/internal/mpp/local_mpp_coordinator.rs::handleAllReports` 进入 `MppReportSink::MergeTiFlashRUConsumption`。

## 核心职责

1. 用 `TiflashStats` 聚合四组统计：传统 TiFlash 扫描、列存扫描、等待摘要和网络流量。
2. 将 `tipb` protobuf 摘要逐字段合并到本地结构；`None` 摘要是无操作，不制造“已观测”证据。
3. 合并多个 task/response 的统计：大多数字段求和，列存宽度类字段取最大值，流式耗时维护最小值/最大值，等待摘要保留执行时间最长的一条。
4. 生成与 Go 版本兼容的诊断文本，包括向量索引、全文索引、倒排索引、DMFile、列存、等待和网络信息。
5. 将 TiFlash 网络统计原子累加到 `tikvutil::ExecDetails`，并将序列化 RU consumption 解码后合并到 `tikvutil::RUDetails`。

本文件只负责统计数据的表达、归并和格式化，不负责发起 TiFlash 请求、调度 MPP task 或维护运行时统计集合；这些职责分别位于调用方，例如 `runtime_stats.rs` 和 MPP coordinator。

## 主要符号

- `TiflashStats`：四个上下文的容器，字段为 `scanContext`、`columnarScanContext`、`waitSummary`、`networkSummary`。自身没有合并方法，调用方 `basicCopRuntimeStats::Merge` 分别调用四个子结构的 `Merge`。
- `TiFlashScanContext`：传统 TiFlash/DeltaMerge 扫描统计。字段覆盖 DMFile 扫描与跳过行数、MVCC 输入输出、Region/segment/task、snapshot/bitmap/input-stream 和 local/remote stream 耗时、分离式缓存命中，以及 vector/FTS/inverted index 指标。`regionsOfInstance: HashMap<String, u64>` 保存每个 TiFlash 实例的 Region 数。
  - `Clone(&self) -> TiFlashScanContext`：深复制；Rust `HashMap::clone` 与 Go 的 `make` 加 `maps.Copy` 同义。
  - `String(&self) -> String`：按非零索引类别追加 `vector_idx`、`inverted_idx`、`fts`，最终总会追加 `tiflash_scan`。
  - `Merge(&mut self, other)`：归并另一份本地统计。
  - `mergeExecSummary(&mut self, Option<&tipb::TiFlashScanContext>)`：从 wire summary 归并。
  - `Empty(&self) -> bool`：按 Go 的有限“有效扫描证据”字段判断为空，并非检查结构内每个字段是否为零。
- `TiFlashColumnarScanContext`：列存读路径统计，含 `hasStats` presence 位、Region/task/table/column、读字节、MVCC、各阶段耗时、rough-check pack 分类和 segment 数。
  - `Merge` 对计数/耗时求和，但 `physicalTables`、`columns` 取最大值，`hasStats` 做逻辑或。
  - `mergeExecSummary` 在收到非空 protobuf 后先置 `hasStats = true`，即使所有数值为零也保留“已上报”语义。
- `TiFlashWaitSummary`：记录 `executionTime` 与三种纳秒等待时间。`Merge`/`mergeExecSummary` 只在新 execution time 严格更大时整体替换等待值；相等时保留先到值。`CanBeIgnored` 在三类等待均小于 1 ms 时返回 true。
- `TiFlashNetworkTrafficSummary`：区内/跨区的发送与接收字节。
  - `UpdateTiKVExecDetails` 将跨区和总量写入四个 `AtomicI64`。
  - `GetInterZoneTrafficBytes` 只返回跨区发送字节，避免发送、接收两侧对同一流量重复计数。
- `MergeTiFlashRUConsumption(executionSummaries, ruDetails) -> Result<(), error::Error>`：逐条解析非空 `ru_consumption` 为 `resource_manager::Consumption`，先聚合进临时 `RUDetails`，全部成功后才合并进调用者提供的 `ruDetails`。

文件没有 trait、模块级常量或条件编译项；公开 API 使用了与 Go 对齐的 CamelCase 名称，所在 crate 在 `lib.rs` 中统一允许 `non_snake_case`。

## 执行流程

运行时统计主链如下：

1. TiFlash 返回 `tipb::ExecutorExecutionSummary`。
2. `pkg/util/execdetails/runtime_stats.rs::basicCopRuntimeStats::mergeExecSummary` 累加通用的迭代、行数、并发和处理时间。
3. 该函数仅在对应 protobuf 子消息存在时惰性创建 `TiflashStats`，随后分别调用扫描、列存、等待和网络结构的 `mergeExecSummary`。
4. 多份 `basicCopRuntimeStats` 合并时，`basicCopRuntimeStats::Merge` 调用各子结构的 `Clone` 与 `Merge`。等待统计不是相加，而是选择处理时间最长的 task；扫描与网络主要相加。
5. `CopRuntimeStats::String` 仅对 TiFlash store 输出这些信息：先输出 wait/network；列存上下文非空时优先输出 `columnar_scan`，否则输出非空的传统 `tiflash_scan`。`basicCopRuntimeStats::String` 的兼容路径则直接输出传统 scan。

MPP RU 主链与普通 runtime 字段独立：`local_mpp_coordinator.rs::handleAllReports` 等待全部 report，遍历每份 report 后调用 `MppReportSink::MergeTiFlashRUConsumption`。本文件的自由函数对每条 summary 的二进制 `ru_consumption` 调用 protobuf `merge_from_bytes`，把 `Consumption` 交给临时 `RUDetails::UpdateTiFlash`，最后一次性 `ruDetails.Merge`。

格式化流程中的重要分支包括：索引 load 来源总数为零时不显示相应索引块；FTS 平均搜索耗时在次数为零时显式为 0；remote stream 与 disaggregated cache 只在存在非零数据时显示；Region balance 从 map 计算实例数、最大值、最小正值和固定六位小数的比值。

## 数据与状态

这些结构都实现 `Clone + Default`，默认值是全零/空 map。归并会原地修改接收者，整数运算使用普通 `+=`，未做饱和或显式溢出检查，因此应保持与 protobuf 计数和 Go 无符号累加的约束一致。

关键不变量如下：

- `minLocalStreamMs`/`minRemoteStreamMs` 的 0 表示“尚未设置”；归并逻辑以另一侧值初始化，否则取更小值。`max*` 始终取较大值。
- `regionsOfInstance` 按 instance id 求和；`Clone` 后 map 不共享可变状态。
- 列存的 `physicalTables` 和 `columns` 表示一次聚合中的最大并行宽度/规模，不可改为求和；其他列存计数和耗时才求和。
- `TiFlashWaitSummary.executionTime` 是选择 wait sample 的比较键，不参与输出；只有更长 execution time 的 sample 能替换当前等待值。
- `Empty` 是展示策略而非逐字段零值等价：例如传统扫描上下文的部分耗时或字节字段本身不参与空判断，而列存上下文同时检查 presence 位和全部字段。
- 网络字段存为 `u64`，但 `String` 为兼容 Go 的 `uint64 -> int64` 转换按 `i64` 显示，因此大于 `i64::MAX` 的值显示为负数。
- RU 合并具有批次原子性：任一非空 payload 解码失败时立即返回，临时聚合结果不会写入目标 `ruDetails`。

## 依赖与调用关系

crate 边界由两层 Cargo 清单确定：顶层 `pkg/util/execdetails/Cargo.toml` 的包名是 `astersql-util-execdetails`，依赖内部包 `astersql-util-execdetails-ruv2`；真正编译本文件的 `pkg/util/execdetails/internal/ruv2/Cargo.toml` 声明 `protobuf = 2.8.0`、`prometheus` 和 `astersql-util-resourcegrouptag`，并运行 `build.rs`。

下游依赖：

- `std::collections::HashMap`：实例到 Region 数的聚合和 balance 展示。
- `std::time::Duration`：1 ms 忽略阈值与纳秒到毫秒转换。
- `std::sync::atomic::Ordering`：更新 `tikvutil::ExecDetails` 时使用 `SeqCst` 原子加。
- `protobuf::Message`：为 `resource_manager::Consumption::merge_from_bytes` 提供解码方法。
- `tipb::*`：`TiFlashScanContext`、`ColumnarScanContext`、`TiFlashWaitSummary`、`TiFlashNetWorkSummary` 与 `ExecutorExecutionSummary`。绑定由 `internal/ruv2/build.rs` 从官方 tipb proto 生成，并为 `get_min_tso_wait_ns` 补稳定拼写。
- `tikvutil::{ExecDetails, RUDetails}` 与 `resource_manager::Consumption`：由 `internal/ruv2/lib.rs` 定义/再导出。

直接上游：`runtime_stats.rs::basicCopRuntimeStats::{mergeExecSummary, Merge, String}`、`StmtCopRuntimeStats::mergeExecSummary` 和 `CopRuntimeStats::String`；MPP RU 通过 `local_mpp_coordinator.rs::handleAllReports` 调用 report sink。图索引还显示本文件被 `runtime_stats.rs`、`ruv2_metrics.rs`、`internal/ruv2/lib.rs` 及相关测试使用。

## 错误处理与边界

多数统计方法是纯本地、无返回错误的归并操作。所有 `mergeExecSummary(Option<...>)` 和 `UpdateTiKVExecDetails(Option<...>)` 对 `None` 静默返回，符合 Go nil 输入不做事的语义。

唯一显式错误路径是 `MergeTiFlashRUConsumption`：无效 protobuf 字节由 `merge_from_bytes` 产生 `protobuf::ProtobufError`（通过 `error::Error` 别名）并直接向上传播。空 summary、`None` summary 或空 `ru_consumption` 被跳过。调用者必须处理错误，不能把部分 RU 当作完整批次。

需留意的数值边界：普通 `u64/u32` 归并在 release 构建可能回绕；Region balance 若 map 非空却没有正值，则最小值保持 `u64::MAX`，比值按浮点规则展示；传统 `GetInterZoneTrafficBytes` 的 Rust 接收者不能像 Go nil 指针方法那样表示 nil，因此 Rust API 没有 Go 版本的 nil 返回 0 分支。字符串中的网络字节有意按 `i64` 重解释，以匹配 Go 边界行为。

## 并发与资源生命周期

扫描、列存、等待和网络 summary 本身不含锁，也不提供内部同步；调用者必须独占 `&mut self` 才能执行 `Merge`/`mergeExecSummary`，Rust 借用规则阻止同一对象的并发可变访问。`Clone` 用于把一个统计快照安全合并进另一个对象。

并发共享发生在边界对象中：`TiFlashNetworkTrafficSummary::UpdateTiKVExecDetails` 对 `ExecDetails` 的四个 `AtomicI64` 使用 `fetch_add(..., Ordering::SeqCst)`，保证多个汇报者并发累加不会丢失更新；`RUDetails` 的实际字段由 `internal/ruv2/lib.rs` 中的 `Mutex` 保护。本文件先创建临时 RUDetails，生命周期仅覆盖一次函数调用，解码全部成功后再并入长期对象。

本文件不创建线程、任务、通道、文件句柄或事务。protobuf `Consumption` 是循环内临时值；`HashMap` 和格式化 `Vec<String>` 均由所有者自动释放。

## 与 Go 版本的对应关系

Rust 的类型、字段分组和方法基本逐项对应 `pkg/util/execdetails/tiflash_stats.go`：`Clone`、`String`、`Merge`、`mergeExecSummary`、`Empty`、`CanBeIgnored`、`UpdateTiKVExecDetails`、`GetInterZoneTrafficBytes` 和 `MergeTiFlashRUConsumption` 保留同一职责。

已核对的关键同义点：

- Go 用 `maps.Copy` 深复制 `regionsOfInstance`，Rust 用派生 `Clone` 深复制 map。
- Go/Rust 的 scan、columnar、wait、network 合并规则一致；尤其是列存 table/column 取最大值、wait 取最大 execution time 对应样本、stream min 的 0 哨兵规则。
- Go `atomic.AddInt64` 对应 Rust `AtomicI64::fetch_add(SeqCst)`。
- Go `time.Millisecond` 阈值对应 Rust `Duration::from_millis(1).as_nanos()`。
- Go 网络字符串先转 `int64`，Rust 显式 `as i64`；`tiflash_stats_test.rs::network_string_matches_go_int64_conversion` 锁定 `u64::MAX -> -1`。
- Go `%f` 的 Region 比例默认六位小数，Rust使用 `{:.6}`；`region_balance_uses_go_fixed_point_format` 锁定该格式。
- Go `Unmarshal` 对应 protobuf 2.8 的 `merge_from_bytes`；两者在首个坏 payload 处返回错误，并在最终成功后才合并临时 RU。

可见接口差异主要来自语言模型：Go 结构字段为包内私有，而 Rust 字段为 `pub` 以跨内部模块/测试使用；Go 方法可接收 nil 指针，Rust 引用不可为空，只有显式 `Option` 参数保留 nil 语义；Go summary slice 是指针切片，Rust 是 `Option<ExecutorExecutionSummary>` slice。

## 扩展指南

新增 TiFlash 统计字段时，应从 wire 到展示完整接线，避免只改结构体：

1. 在对应本地结构增加字段，并核对官方 tipb schema/build 生成绑定是否已有 getter。
2. 同步更新 `Merge` 和 `mergeExecSummary`；根据字段语义明确选择求和、最大/最小、presence 或“最长 execution time 对应样本”，不要默认全部求和。
3. 若字段影响“是否应显示”，更新相应 `Empty`/`CanBeIgnored`；传统 scan 的 `Empty` 有意不是全字段检查，修改前须核对 Go 行为。
4. 若字段需要诊断输出，更新 `String` 并保持既有标签、顺序、空格和单位兼容。网络边界、纳秒/毫秒转换和浮点格式尤其容易造成 Go/Rust 差异。
5. 若字段属于 MPP 流量或 RU，检查 `UpdateTiKVExecDetails`、`GetInterZoneTrafficBytes` 或 `MergeTiFlashRUConsumption` 的双计数和批次错误语义。
6. 同步修改独立 Rust 测试，优先放在 `pkg/util/execdetails/tiflash_stats_test.rs`；跨 runtime 聚合可扩展 `execdetails_test.rs`，execution-unit presence/overflow 可扩展 `tiflash_execution_units_test.rs`。同时参照 Go 的 `execdetails_test.go`、`tiflash_execution_units_test.go`，不得把测试内嵌到生产源文件。

兼容性风险主要是慢查询/Explain 文本格式变化、空判断导致字段被隐藏、计数溢出以及收发流量重复计费；性能风险主要来自在热路径扩大 clone/map 合并和字符串分配。若加入共享可变状态，应沿现有边界使用原子或 `RUDetails` 的锁，而不是在这些值对象内引入隐式全局同步。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、307296 个节点、1848419 条边；目标 `pkg/util/execdetails/tiflash_stats.rs` 已索引，共 790 行。
- RustCodeGraph 源码/结构查询：
  - `rustcodegraph files --filter pkg/util/execdetails`
  - `rustcodegraph node --file pkg/util/execdetails/tiflash_stats.rs --offset 1 --limit 260`
  - 同文件 offsets `260/540`，覆盖全部类型和方法。
  - `rustcodegraph query TiFlashScanContext --kind struct --limit 20`
  - `rustcodegraph callers/callees` 与聚焦 `explore` 查询；确认 `MergeTiFlashRUConsumption` 的 MPP report 调用链，并用精确文件节点消除同名符号歧义。
  - `runtime_stats.rs` offsets `195`、`local_mpp_coordinator.rs` offsets `35/675`，核对解码、合并、展示和 report 汇总入口。
- crate/生成边界：`pkg/util/execdetails/Cargo.toml`、`pkg/util/execdetails/internal/ruv2/Cargo.toml`、`internal/ruv2/lib.rs` 第 317–327 行、`internal/ruv2/build.rs`。
- Go 对照：`pkg/util/execdetails/tiflash_stats.go` 全部 918 行；聚合展示测试 `pkg/util/execdetails/execdetails_test.go::TestCopRuntimeStatsForTiFlash`、`TestVectorSearchStats`、`TestColumnarScanContextStats`。
- Rust 独立测试：`pkg/util/execdetails/tiflash_stats_test.rs`；补充行为证据来自 `execdetails_test.rs::test_vector_search_stats`、`test_columnar_scan_context_stats` 和 `tiflash_execution_units_test.rs` 的 presence、重复、显式零及 overflow 用例。
- 本任务是纯文档分析，按计划未运行 Cargo。交付验证使用任务规定的 11 章节结构检查，并人工复核唯一生产物、源码链接、Go 对齐、边界和扩展位置。
