# `pkg/executor/stmtsummary.rs`

## 文件定位

`pkg/executor/stmtsummary.rs` 属于 `astersql-executor` crate；crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/lib.rs` 通过 `pub mod stmtsummary` 公开本模块，并在测试配置下把独立的 `pkg/executor/stmtsummary_test.rs` 装配为测试模块。

本文件是 information schema 语句摘要表的 Rust 检索骨架，覆盖 `STATEMENTS_SUMMARY`、`STATEMENTS_SUMMARY_HISTORY`、`STATEMENTS_SUMMARY_EVICTED`、`TIDB_STATEMENTS_STATS` 及其 `CLUSTER_*` 变体。它负责选择 Legacy/Persistent/Dummy 路径、按需创建行读取器、分页返回数据，以及把权限、实例地址、列投影和实际数据源操作委托给运行时适配器。

当前接线状态必须与实现能力分开理解：RustCodeGraph 显示该文件由 `pkg/executor/stmtsummary_test.rs` 等测试文件引用；对 `buildStmtSummaryRetriever` 的调用者查询只命中该独立 Rust 测试，仓库 Rust 源码搜索也未发现生产侧构造调用。因此它目前是已公开、可测试的通用实现，但未验证已经接入 Rust 生产执行主链。相对地，Go 版本由 `pkg/executor/builder.go:2885` 构造并接入内存表执行。

## 核心职责

- 用八个表名常量和 `isClusterTable`、`isCumulativeTable`、`isCurrentTable`、`isHistoryTable`、`isEvictedTable` 对请求表分类；比较是精确、区分大小写的字符串匹配。
- `buildStmtSummaryRetriever` 规范化 planner 提取条件：空 digest 集合被视为无过滤，粗时间范围经 `buildTimeRanges` 变成一段精确 Unix 秒范围，并根据 `skip_request` 与 `StatementSummaryRuntime::persistent_enabled` 选择模式。
- `StmtSummaryRetriever::ensureRowsReader` 惰性建立读取状态；驱逐表走 `initEvictedRowsReader`，其余表走 `initSummaryRowsReader`。
- `RowsReader` 统一“预装内存行”和“后续批量 puller”，由 `read` 每次最多返回指定数量；检索器固定以 `DEFAULT_RETRIEVE_COUNT`（1024）为页大小。
- 用 `StatementSummaryRuntime` 隔离真实上下文、行、列、digest、错误、权限检查、实例地址、Legacy 数据和 Persistent v2 数据源。本文件只编排流程，不持有全局摘要存储。

## 主要符号

- `DEFAULT_RETRIEVE_COUNT: usize = 1024`：`StmtSummaryRetriever::retrieve` 的固定单批上限。
- `StmtTimeRange { begin, end }` 与 `CoarseTimeRange { start_unix, end_unix }`：分别表示数据源消费的精确范围和 planner 侧输入。本文件不重排、不裁剪，也不校验 `begin <= end`。
- `StatementsSummaryExtractor<D>`：携带可选 digest、可选时间范围及 `skip_request`；与 Go 的 `plannercore.StatementsSummaryExtractor` 对应，但通过泛型避免直接绑定 planner 类型。
- `RowsPuller<Row, E>`：历史数据源的最小接口，只有 `rows` 与 `close`。`RowsReader<Row, E>` 持有可选 boxed puller 和尚未消费的行缓冲。
- `newSimpleRowsReader`：只包装已有行；`newRowsReader`：包装首批内存行并连接历史 puller。
- `RetrieverMode::{Dummy, Legacy, Persistent}`：Dummy 跳过请求，Legacy 使用旧内存摘要接口，Persistent 使用内存窗口加可选历史 puller。
- `StatementSummaryRuntime`：本文件最重要的适配边界。其关联类型决定上下文和数据表示；方法分别提供模式开关、digest 判空、PROCESS 权限、错误适配、集群实例地址、host 列附加、列投影及 Legacy/Persistent 数据读取。
- `StmtSummaryRetriever<R>`：保存 runtime、模式、目标表、列投影、过滤条件和惰性 `rows_reader`。所有字段公开，便于当前移植层和测试装配，但调用方必须维护字段之间的一致性。
- `checkPrivilege`、`clusterTableInstanceAddr`：小型边界函数，分别把 PROCESS 权限失败映射为 runtime 错误，并仅为集群表查询实例地址。
- `buildTimeRanges`：把一个 `CoarseTimeRange` 一对一映射为单元素 `Vec<StmtTimeRange>`；`None` 保持为 `None`。

## 执行流程

1. 调用方用 `buildStmtSummaryRetriever` 传入 runtime、表名、投影列和可选 extractor。函数补默认 extractor，去掉空 digest，计算时间范围，再按 `skip_request > persistent_enabled > Legacy` 的优先级确定模式。
2. 首次调用 `StmtSummaryRetriever::retrieve` 时，Dummy 直接返回空向量；其他模式调用 `ensureRowsReader`。后续 retrieve 复用同一个 reader，不会重新读取内存摘要快照。
3. `ensureRowsReader` 按 `isEvictedTable` 分流。初始化成功后才把 reader 写入 `self.rows_reader`；初始化错误会向上传播并保留 `None`，所以以后可以重试。
4. 驱逐表路径先执行 `checkPrivilege`。Legacy 调 `legacy_evicted_rows`，Persistent 取至多一个 `persistent_evicted_row`；集群表再经 `append_host_info` 加实例信息，最后统一 `adjust_columns` 并建立简单 reader。
5. 普通摘要路径先拒绝 Persistent 模式下的累计表。随后计算当前用户是否具备 PROCESS 权限，并由 `clusterTableInstanceAddr` 为集群表取得实例地址；这里的权限布尔值传给数据源用于内容过滤，不像驱逐表那样直接拒绝。
6. Legacy 模式一次调用 `legacy_summary_rows` 取得完整结果并建立简单 reader。Persistent 模式先调用 `persistent_memory_rows`；当前表只返回这些内存行，历史表则再建立 `persistent_history_puller`，使内存行先于持久化历史行输出。无法识别的非驱逐表得到空 reader。
7. `RowsReader::read` 先调用 `pull`。只要缓冲非空，puller 不会被访问；缓冲清空后一次拉取一批。若请求上限覆盖全部缓冲，则用 `mem::take` 转移所有行；否则用 `split_off` 留下后缀、返回前缀。
8. puller 返回空批次表示 EOF：`pull` 调用其 `close`，成功后清除 puller。调用方也可通过 `StmtSummaryRetriever::close` 主动转发关闭。

## 数据与状态

检索器的稳定配置是 `mode`、`table_name`、`columns`、`digests` 与 `time_ranges`；可变运行状态是 `runtime` 和 `rows_reader`。`rows_reader == None` 同时表示“尚未初始化”或“最近一次初始化失败”，而成功初始化后的空结果由 `Some(RowsReader { rows: [], puller: None })` 表示。

`RowsReader.rows` 是待返回队列。分页时前缀被交给调用方、后缀留存；历史模式的顺序不变量是先耗尽构造时的内存行，再逐批消费持久化行。它不会跨 puller 批次凑满 `maximum_count`：每次 `pull` 最多取一批，然后 `read` 从当前批次截页。因此返回行数不超过上限，但可能少于上限且不代表 EOF；只有返回空批次且 puller 已耗尽才表示结束。

`StatementSummaryRuntime` 的关联类型让本模块不依赖具体 `Datum`、session context 或 digest set。`pkg/executor/Cargo.toml` 同时声明了 `astersql-util-stmtsummary`、`astersql-util-stmtsummary-v2`，但本文件没有直接调用它们；预期真实适配器才把 trait 方法接到这两个数据源。文件直接依赖的 workspace crate 是 `astersql-errors` 和 `astersql-util-dbterror-plannererrors`，用于 Persistent 累计表的标准错误构造与适配。

## 依赖与调用关系

上游模块装配是 `pkg/executor/lib.rs -> pub mod stmtsummary`。当前已验证的 Rust 调用者主要是 `pkg/executor/stmtsummary_test.rs`：辅助函数 `retriever` 和测试 `builder_and_evicted_privilege_match_go_contract` 调用 `buildStmtSummaryRetriever`；其他测试直接驱动 `RowsReader` 与 `StmtSummaryRetriever::retrieve`。RustCodeGraph 对目标文件报告的引用还包括恢复和 planner 测试，但这些属于测试依赖，不能作为生产接线证据。

内部调用主链为 `buildStmtSummaryRetriever -> buildTimeRanges`，以及 `retrieve -> ensureRowsReader -> initEvictedRowsReader/initSummaryRowsReader -> StatementSummaryRuntime`。`RowsReader::read -> pull -> RowsPuller::rows/close` 形成历史读取链。图查询对同名 Go/Rust 符号存在混合结果，因此文档中的内部边同时以本文件源码顺序核验。

Go 生产上游位于 `pkg/executor/builder.go:2870-2885`：识别语句摘要相关 information schema 表，取得 `StatementsSummaryExtractor`，调用 Go `buildStmtSummaryRetriever`。Rust 当前没有对应的生产构造边，扩展时不能仅实现 trait 适配器，还必须在 Rust 的内存表 builder/retriever 分派处完成接线并增加集成级测试。

下游抽象包括权限系统、集群实例地址、列调整、Legacy 摘要读取器、Persistent 内存读取器和历史读取器；具体实现均经 `StatementSummaryRuntime` 注入。直接错误下游 `astersql_util_dbterror_plannererrors::ErrNotSupportedYet` 仅用于 Persistent 累计表。

## 错误处理与边界

- `RowsPuller::rows`、`RowsPuller::close`、runtime 数据访问、host 信息追加与实例地址获取的错误均用 `?` 原样传播；本文件不记录日志也不重试。
- `RowsReader::pull` 只有在 EOF 的 `close` 成功后才把 puller 置空。如果 close 失败，puller 被保留，后续调用可能再次尝试关闭。
- `RowsReader::close` 不清除 puller，因此显式重复 close 是否安全由具体 puller 决定；测试只证明 EOF 自动关闭一次。检索器没有实现 `Drop`，资源所有者必须显式调用 `close` 或持续读取到 EOF。
- 驱逐表要求 PROCESS 权限，失败通过 `process_privilege_denied` 返回；普通摘要表只把 `process_privilege` 布尔值传给数据源，符合 Go 版本由 reader 控制可见内容的设计。
- Persistent + `TIDB_STATEMENTS_STATS`/`CLUSTER_TIDB_STATEMENTS_STATS` 返回 `ErrNotSupportedYet`，错误文本为 `cumulative statement summary table with persistent mode (v2)`。错误发生在任何内存读取前，且 reader 保持未初始化；Rust 和 Go 回归测试都覆盖这一点。
- 表名识别只接受八个大写常量。未知表在 Persistent/Legacy 初始化路径上的行为不完全对称：Persistent 最终为空；Legacy 是否为空由 `legacy_summary_rows` 适配器决定。因此生产接线应传 canonical information schema 表名。
- `maximum_count == 0` 时 `RowsReader::read` 返回空前缀而保留所有行；检索器自身固定传 1024，不会触发此边界。时间范围也不做合法性校验，依赖 planner/数据源保证语义。

## 并发与资源生命周期

`StmtSummaryRetriever::retrieve`、`ensureRowsReader` 和初始化方法都要求 `&mut self`，`RowsReader` 也以 `&mut self` 消费缓冲，因此单个实例不会被无同步地并发读取。trait 没有要求 `Send` 或 `Sync`；是否能跨线程移动或共享完全取决于具体 `R`、关联类型和外部同步，不能从本文件推断线程安全。

历史资源由 `Box<dyn RowsPuller<...>>` 独占。生命周期从 `persistent_history_puller` 成功返回开始，经过内存行和持久化批次消费，在 puller 首次返回空批次时自动关闭；若调用方提前停止，则应调用 `StmtSummaryRetriever::close`。初始化历史 puller 之前若 `persistent_memory_rows` 失败，不会创建历史资源；创建 puller 失败也不会安装 reader。

测试中的 `Arc<Mutex<PullState>>` 仅用于观察批次和关闭次数，不表示生产实现内部一定使用锁。`DEFAULT_RETRIEVE_COUNT` 限制单次向上游移交的缓冲行数，但 puller 自己的批次大小由数据源决定，本模块不会限制一次 `rows()` 获取所占内存。

## 与 Go 版本的对应关系

Rust 文件按 `pkg/executor/stmtsummary.go` 的结构移植：常量、builder、Legacy/Persistent 两种检索策略、`rowsReader`、表分类、权限检查、实例地址及时间范围转换均有对应物。Rust 用 `RetrieverMode` 合并 Go 的 `dummyRetriever`、`stmtSummaryRetriever`、`stmtSummaryRetrieverV2`，再用 `StatementSummaryRuntime` 把 Go 中的 session、infoschema 与两代 stmtsummary 直接调用抽象出去。

关键语义保持一致：空 digest 等于无过滤；skip 优先于持久化开关；每批最多 1024 行；驱逐表要求 PROCESS；集群驱逐表追加 INSTANCE；Persistent 当前表只读内存，历史表按“内存后磁盘”顺序读取；Persistent 累计表明确返回不支持；粗时间范围转成单个 begin/end 范围。

可见差异与迁移限制包括：

- Go builder 已在 `pkg/executor/builder.go` 生产接线，Rust builder 当前只见测试调用。
- Go 直接读取全局配置和 `GlobalStmtSummary`，Rust 由 runtime 提供 `persistent_enabled` 及数据源，更利于测试但需要真实适配器才能工作。
- Go v2 历史读取器接收 context 和 `DistSQLScanConcurrency`；Rust trait 方法没有显式 context/并发参数，只能由 `R::Context` 或 runtime 自身间接提供。若生产移植依赖取消或并发控制，需要在适配层验证没有语义丢失。
- Go `initSummaryRowsReader` 对意外表名可能留下 nil reader，旧实现随后有 panic 风险；Rust 对 Persistent 未识别表构造空 reader，属于更安全但可观察行为不同的兜底。
- Go 的 `rowsReader.close` 与 Rust 一样不会主动清空 puller；两者都依赖调用纪律或底层 close 幂等性。

## 扩展指南

新增或调整表类型时，应同步修改表名常量及所有相关分类谓词，并审查 `ensureRowsReader` 的分流、`initSummaryRowsReader` 的模式矩阵和 `clusterTableInstanceAddr`。若新增表返回完整列，驱逐路径还需确认 `append_host_info` 与 `adjust_columns` 的先后顺序。同步扩展 `pkg/executor/stmtsummary_test.rs` 的表分类、权限、当前/历史/驱逐及错误测试；测试逻辑必须继续放在独立文件，不内嵌到生产源文件。

接入真实 Rust 执行链时，最可能新增的是 `StatementSummaryRuntime` 的生产实现以及 executor builder 的 `buildStmtSummaryRetriever` 调用。适配器需分别映射 `legacy_summary_rows`、`persistent_memory_rows`、`persistent_history_puller`，并验证用户/PROCESS 权限、时区、digest、时间范围、实例地址、列投影、查询取消和历史并发度都与 Go 一致。由于当前 trait 未显式表达时区、取消和并发参数，若 `R::Context` 无法完整承载这些信息，应先调整接口并补回归测试，不能静默省略。

调整分页或 puller 生命周期时，必须保持“内存先于历史”“非空短页不表示 EOF”“空批次触发关闭”“错误不丢失资源句柄”等不变量。建议在 `pkg/executor/stmtsummary_test.rs` 增加 pull 错误、close 错误、零上限、多页大批次与提前 close 用例；若改变用户可见 SQL 结果，还应同步 Go 对照测试 `pkg/executor/stmtsummary_test.go` 的意图，并增加 Rust 侧 executor/information-schema 集成验证。

性能风险主要是 runtime 一次性返回过大的 `Vec<Row>`，以及 `split_off` 分页时前缀保留在原 Vec、随后 `mem::replace` 移出造成的内存行为；兼容风险主要来自表名、列顺序、实例列、权限可见性和 Legacy/Persistent 结果差异。任何优化都应先用相同数据顺序和错误时机的测试固定契约。

## 验证依据

- RustCodeGraph 索引状态：项目索引可用，包含 11,467 个文件；通过 `node --file pkg/executor/stmtsummary.rs` 阅读完整 438 行源码，并取得“由测试文件引用”的文件关系。
- RustCodeGraph 符号/边：查询 `buildStmtSummaryRetriever` 同时定位 Go 与 Rust 定义；Rust 调用者结果为 `stmtsummary_test.rs::retriever` 与 `builder_and_evicted_privilege_match_go_contract`；callees 包含 Rust `StatementSummaryRuntime::persistent_enabled`、`digests_empty` 和 `buildTimeRanges`。同名查询会混入 Go 节点，故内部调用链另以目标源码核验。
- 已读 Rust 路径：`pkg/executor/stmtsummary.rs`、`pkg/executor/stmtsummary_test.rs`、`pkg/executor/lib.rs`、`pkg/executor/Cargo.toml`。目标 package 无 `pkg/executor/doc.go`。
- 已读 Go 对照：`pkg/executor/stmtsummary.go`、`pkg/executor/stmtsummary_test.go`，并由 `pkg/executor/builder.go:2870-2885` 核验生产构造入口。
- 独立 Rust 测试证明：简单 reader 分页和表分类；内存行与历史批次的顺序及 EOF 自动关闭一次；Persistent 当前/驱逐/历史路径；空 digest 与 Dummy 模式；驱逐权限拒绝与集群 host 附加；Persistent 累计表错误、reader 不初始化且不读取内存；Legacy 累计表仍可返回行。
- Go 测试补充证明 v2 当前表 digest 聚合、驱逐计数、历史内存加磁盘结果，以及两种累计表返回标准 `ErrNotSupportedYet`。本任务按计划不运行 Cargo，所有结论来自结构、源码、调用图和现有测试意图核验。
