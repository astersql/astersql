# `pkg/executor/inspection_result.rs`

## 文件定位

本文件属于 `astersql-executor` crate；crate 根在 `pkg/executor/lib.rs`，其中以 `pub mod inspection_result;` 公开本模块。`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 与 `[package.metadata.porting] go-package = "pkg/executor"` 表明它是 Go `pkg/executor` 的 Rust 移植组成部分。文件自身只使用 `std` 的有序集合、格式化和 `Arc`，没有直接引用 Cargo 中的其他 crate。

它描述 `INFORMATION_SCHEMA.INSPECTION_RESULT` 背后的巡检规则引擎：从 `information_schema.cluster_config`、`cluster_info`、`cluster_log`、`cluster_systeminfo` 以及 `metrics_schema` 查询集群状态，生成固定九列的巡检结果。需要注意当前接线状态：Rust 仓库搜索只找到 `lib.rs` 的模块声明和两个独立测试模块，没有发现测试之外构造 `inspectionResultRetriever` 的 Rust 调用点；可确认的完整生产入口仍在 Go `pkg/executor/builder.go` 的 `TableInspectionResult` 分支。因此本文件应视为已经承载主要规则语义、但 Rust 生产执行主链接线尚未由本任务证实的移植模块，而不能据此宣称 Rust 服务已经实际使用它。

## 核心职责

- `inspectionResultRetriever::retrieve` 负责一次性执行：建立巡检表缓存、合并测试快照、获取实例地址映射、按规则和检查项过滤、运行五类规则、稳定排序、补齐 `instance`/`statusAddress`，最终物化为 `Vec<Vec<datum>>`。
- `configInspection` 检查同组件配置不一致、三项已知不合理配置，以及同机 TiKV block cache 总量超过物理内存 45% 的情况。
- `versionInspection` 检查同类型组件存在多个 `git_hash`。
- `nodeLoadInspection` 检查 load1/load5/load15 相对 CPU 核数、虚拟内存、swap 和磁盘使用率。
- `criticalErrorInspection` 汇总 panic、busy、write stall 等错误指标，并从 Prometheus `up` 波动或 Welcome 日志识别断连/重启。
- `thresholdCheckInspection` 检查 TiKV 线程 CPU、延迟与 pending 数、block cache 命中率、store 均衡、Region 健康/数量和 leader 骤降。

这些职责由 `inspectionRule::run` 和 `ruleChecker::{sql,result,item}` 两层分发统一组织；前者分发五大规则，后者复用“生成 SQL—执行—映射结果”的检查项模板。

## 主要符号

- `InspectionError` / `InspectionResultValue<T>`：本模块的字符串错误与统一 `Result`。错误主要来自数据源 SQL、缓存操作和容量解析。
- `datum` / `queryRow`：简化的数据单元与行适配器。`string`、`float`、`unsigned` 会对类型进行宽松转换；越界、`Null` 或解析失败会得到空串/零，而不是错误。
- `queryTimeRange::condition`：生成闭区间时间条件 `where time >= ... and time <= ...`。字符串直接嵌入 SQL，调用方必须提供可信且已格式化的时间值。
- `inspectionDataSource: Send + Sync`：运行时边界，抽象 SQL 执行、指标标签、warning 记录及巡检表缓存的 begin/merge/end 生命周期。`Arc<dyn inspectionDataSource>` 让 retriever 持有可共享的数据源。
- `inspectionResult`：内部发现记录。`degree` 仅参与排序，不进入最终九列；其余字段映射为 rule、item、type、instance、status address、actual、expected、severity、detail。
- `inspectionFilter`：有序集合为空时表示全部启用，否则只允许集合内名称；`rules` 过滤五大类，`items` 过滤具体检查项。
- `inspectionRule`：`Config`、`Version`、`NodeLoad`、`CriticalError`、`ThresholdCheck` 五个变体，提供统一的 `rule_name` 与 `run`。
- `inspectionResultRetriever`：顶层检索状态，包含一次性标志、跳过标志、两级过滤、时间窗口、双向地址映射和数据源。
- `ruleChecker`：封装 `inspectVirtualMemUsage`、`inspectSwapMemoryUsed`、`inspectDiskUsage`、`inspectCPULoad`、`compareStoreStatus`、`checkRegionHealth`、`checkStoreRegionTooMuch`；`checkRules` 统一运行它们。
- `configInspection::convertReadableSizeToByteSize`：识别 `KiB` 至 `PiB` 以及可选 `B` 后缀，先按 `i64` 解析，再转为 `u64` 并使用 wrapping 乘法，以保持现有 Go 移植测试所要求的有符号解析/溢出语义。

## 执行流程

1. 调用者构造 `inspectionResultRetriever`，传入规则集合、检查项集合、查询时间范围和实现了 `inspectionDataSource` 的共享数据源。
2. `retrieve` 在 `retrieved` 或 `skipInspection` 为真时返回空结果；否则立即设置 `retrieved = true`，保证同一 retriever 只物化一次。
3. 它调用 `begin_table_cache`，随后在受控闭包内调用 `merge_mock_table_cache`。当地址映射为空时，查询 `information_schema.cluster_info` 建立 `instance ↔ status_address` 双向映射；该查询失败只追加 warning，不终止巡检。
4. 它创建规则过滤器和检查项过滤器，按 config、version、node-load、critical-error、threshold-check 的固定顺序遍历 `inspectionRule`。未启用的规则直接跳过。
5. 各规则通过 `inspectionDataSource::execute_sql` 读取信息表或指标表。通用检查走 `checkRules`；配置、严重错误和阈值规则还包含需要二次查询或动态指标标签的专用路径。
6. 每类规则的结果先按 `degree` 降序，再按 `item`、`actual`、`tp`、`instance` 升序，保证同一数据输入得到稳定顺序。之后用双向映射补齐缺失地址，生成九列 `datum::String` 行。
7. 闭包无论成功或失败都会继续调用 `end_table_cache`。若业务阶段失败则优先返回业务错误；业务成功但清理失败时返回清理错误；两者都成功才返回行。

规则内部的主要分支包括：配置差异会按值聚合并排序实例；block cache 按去端口后的 IP 聚合同机 TiKV；critical error 根据指标表标签动态组成 `GROUP BY`；server-down 同时检查 `up` 指标和重启日志；threshold-check 分为线程 CPU、通用指标阈值、store/Region 状态与 leader-drop 四组。

## 数据与状态

`inspectionResultRetriever` 有三类可变状态：`retrieved` 控制一次性读取；两个 `BTreeMap` 缓存实例与 status address 的双向关系；规则和检查项 `BTreeSet` 提供确定性过滤。地址映射只在空时查询，允许调用者预填充或在对象生命期内复用。

`inspectionResult.degree` 的量纲并不统一：大部分阈值使用相对偏差，错误计数使用总数，server-down 使用 `10000 + 序号`，leader-drop 使用绝对下降量。它只保证同一规则类别内部的优先顺序，不应被解释成跨规则可比较的业务指标。

时间窗口保存在 `queryTimeRange { from, to }`，由规则拼入 metrics SQL。配置和版本检查基本不依赖时间范围；节点负载、错误与阈值检查依赖它。所有查询行均经 `queryRow` 的宽松访问器读取，缺列或类型异常可能静默变为零值，因此数据源实现和 SQL 列顺序构成重要隐含契约。

## 依赖与调用关系

RustCodeGraph 对本文件确认的核心下游边包括：`inspectionResultRetriever::retrieve → inspectionDataSource::{begin_table_cache,merge_mock_table_cache,execute_sql,end_table_cache}`、`inspectionRule::run → 各规则 inspect`、`nodeLoadInspection::inspect/thresholdCheckInspection::inspectThreshold3 → checkRules`、`checkRules → inspectionFilter::enable + ruleChecker::{item,sql,result} + execute_sql/append_warning`。`configInspection`、`criticalErrorInspection` 和 `thresholdCheckInspection` 的专用方法也直接调用数据源。

上游方面，`pkg/executor/lib.rs` 公开模块，`pkg/executor/inspection_result_test.rs` 与 `inspection_result_internal_test.rs` 直接使用过滤器和容量解析函数；仓库 Rust 搜索没有发现其他构造 `inspectionResultRetriever` 或调用其 `retrieve` 的位置。作为 Go 对照，`pkg/executor/builder.go` 在 `infoschema.TableInspectionResult` 分支构造 Go retriever，随后 Go 内存表执行器调用其 `retrieve`。这条 Go 调用链只能证明原实现位置，不能替代 Rust 生产接线证据。

模块的数据库依赖是协议性的而非 Cargo 类型依赖：SQL 假定存在 `information_schema.cluster_*` 和多个 `metrics_schema.*` 表，并假定第一列、标签列及聚合列具有约定顺序。指标错误检查还依赖 `metric_labels(table)` 返回以实例地址为首标签的非空标签列表。

## 错误处理与边界

- `begin_table_cache`、`merge_mock_table_cache`、`end_table_cache` 返回硬错误；开始缓存失败时函数立即退出且不会调用 end，开始成功后则无论闭包结果如何都会尝试 end。
- 大多数巡检 SQL 失败采用降级策略：`query_or_warn` 或各专用分支调用 `append_warning` 并跳过该项，使其他规则仍能产生结果。获取 cluster info 失败同样只 warning，最终地址可能留空。
- 配置差异的明细查询失败时仍产生一条发现，并在 detail 中给出建议 SQL；block cache 容量任一值解析失败则 warning 并放弃整个 block-cache 检查。
- `datum` 的宽松转换、`queryRow` 的越界零值、浮点转无符号整数会隐藏坏数据。扩展 SQL 时必须同步核对列数、类型和索引，不能依赖零值判断查询是否正确。
- 多处 SQL 通过 `format!` 嵌入时间、表名、配置键或地址；当前值来自固定规则、内部元数据或调用方时间范围。若以后接收用户自由文本，必须改为参数化或严格转义。
- 相对偏差公式使用 `value.max(threshold)` 作分母；现有阈值均为正，新增零阈值规则应先处理零除和 NaN 排序。
- `convertReadableSizeToByteSize` 特意保留负数转 `u64` 和 wrapping 乘法行为；不要在没有同步 Go 语义与测试的情况下改为饱和或拒绝负数。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道。`inspectionDataSource` 要求 `Send + Sync`，并通过 `Arc` 保存在 retriever 中，但 `retrieve(&mut self)` 及内部可变映射意味着同一 retriever 需要由调用方串行使用；该 API 本身不提供并发读取保证。

主要资源是巡检表缓存。生命周期为 `begin_table_cache → merge_mock_table_cache/查询规则 → end_table_cache`，用于复用昂贵的集群内存表读取并保持一次巡检内的一致视图。当前代码显式在闭包后清理，能够覆盖 merge、规则执行和物化阶段的错误；但若 `begin_table_cache` 已产生部分副作用后返回错误，接口没有补偿调用，数据源实现需要保证 begin 的失败原子性。

查询结果、规则数组和中间结果均在调用栈内拥有；最终结果一次性收集到内存，没有流式输出或背压。检查项和集群规模增加会线性增加结果与查询行的内存占用，配置差异及 leader-drop 还会触发逐项二次查询。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/executor/inspection_result.go`。Rust 保留了 Go 的五类 `inspectionRules`、过滤语义、稳定排序字段、地址补全、配置/版本/节点负载/严重错误/阈值规则、block-cache 45% 规则、Region 与 leader-drop 规则，以及九列最终输出。Go 的 `ruleChecker` interface 在 Rust 中改为 `ruleChecker` enum；Go 的 `sessionctx.Context`、restricted SQL executor、statement warning 和 session cache被收敛到 `inspectionDataSource` trait，便于独立测试和后续接线。

Go `retrieve` 由 builder 的 Inspection Result 虚拟表分支实际构造，并通过 session variables 建立/释放 `InspectionTableCache`；Rust 将这些动作抽象为 begin/merge/end，但目前未找到与 Rust executor builder 的生产连接。Go 使用 `plannerutil.QueryTimeRange` 的格式化时间，Rust 使用字符串 `queryTimeRange`。Go 从 `infoschema.MetricTableMap` 获取标签，Rust由数据源的 `metric_labels` 提供。这些抽象必须由未来适配层保持同样的内部事务类型、warning 归属、时间格式和缓存一致性。

测试覆盖也不对称：Go `inspection_result_test.go` 包含 `TestInspectionResult`、三组 threshold、critical error、node load 和 storage block cache 场景；Rust `inspection_result_test.rs` 与 `inspection_result_internal_test.rs` 目前只覆盖过滤/时间条件和容量解析，包括负数及 wrapping 乘法。由此只能确认局部辅助语义，不能将 Go 的端到端行为视为已由 Rust 测试验证。

## 扩展指南

- 新增一类顶层规则：增加规则结构体及 `inspect`，扩展 `inspectionRule` 变体、`rule_name`/`run` 分发和 `retrieve` 中固定注册数组；同步 Go 版本或明确迁移差异，并在独立的 `*_test.rs` 文件中覆盖过滤、输出九列和稳定排序。
- 新增可套用“SQL + 行映射”的检查项：扩展 `ruleChecker` 变体及 `sql`/`result`/`item` 三处分发，然后在 `nodeLoadInspection::inspect` 或 `thresholdCheckInspection::inspectThreshold3` 注册。新增简单阈值则优先扩展 `inspectThreshold1`/`inspectThreshold2` 的规则表。
- 新增数据源能力或生产接线：实现 `inspectionDataSource` 时必须保持 warning 不打断其他规则、缓存 begin/end 配对、指标标签顺序和 cluster info 地址映射语义；还应补一份独立集成测试验证 Rust builder/虚拟表确实调用 `retrieve`。
- 修改 SQL 或输出字段：同步检查 `queryRow` 索引、`degree` 计算、instance/status address 所属列、detail 文案和 Go `inspection_result_test.go` 的期望。最终九列顺序是对外兼容面，不能只改单个规则。
- 修改容量解析：同时更新 `inspection_result_test.rs` 和 `inspection_result_internal_test.rs`；若意图偏离负数转换或 wrapping 溢出，先确认并记录与 Go `strconv.ParseInt` 后 `uint64` 乘法的兼容影响。
- 性能上重点关注配置差异的逐项明细查询、leader-drop 的逐地址明细查询、全量结果内存物化和动态标签聚合；新增规则应尽量复用一次巡检缓存，避免为相同表重复拉取数据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、索引时间戳 `1791342965170`；通过 `node --file pkg/executor/inspection_result.rs` 分段读取 1–1621 行，并查询 `inspectionResultRetriever`、`retrieve`、`checkRules` 的定义与调用边。图明确给出 Rust `checkRules` 对 `execute_sql`、`append_warning`、`enable`、`sql`、`result`、`item` 的调用。
- Rust 源与模块：`pkg/executor/inspection_result.rs`、`pkg/executor/lib.rs`；仓库搜索确认模块声明和两个测试模块之外没有 Rust 使用 `inspectionResultRetriever`/`checkRules` 的生产位置。
- crate 边界：`pkg/executor/Cargo.toml`，确认 crate 名 `astersql-executor`、根文件 `lib.rs`、Go 包映射 `pkg/executor`；本文件的 import 仅来自 `std`。
- Go 对照与入口：`pkg/executor/inspection_result.go` 的类型、五类 inspect、`checkRules` 和 `retrieve`；`pkg/executor/builder.go` 的 `infoschema.TableInspectionResult` 分支。
- 独立测试：Rust `pkg/executor/inspection_result_test.rs`、`pkg/executor/inspection_result_internal_test.rs`；Go `pkg/executor/inspection_result_test.go` 的七个顶层测试覆盖总规则、三组阈值、严重错误、节点负载和 block cache。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令检查目标文件存在且恰有十一个固定二级标题，并人工复核当前接线限制、资源清理、错误降级和测试覆盖差异均有源码依据。
