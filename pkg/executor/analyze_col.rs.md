# `pkg/executor/analyze_col.rs`

源文件：[analyze_col.rs](analyze_col.rs)

## 文件定位

`pkg/executor/analyze_col.rs` 属于 `astersql-executor` crate；crate 根由 `pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`pkg/executor/lib.rs` 以 `pub mod analyze_col;` 对外公开本模块。文件移植自同目录的 `analyze_col.go`，负责表达列统计 ANALYZE 下推请求的参数、结果流生命周期边界和作业描述文本。

当前 Rust 接线必须与 Go 生产链区分开看。RustCodeGraph 显示本文件被 `pkg/executor/analyze.rs`、`pkg/executor/analyze_col_test.rs`、`pkg/executor/analyze_test.rs` 和一个 mockstore 测试文件引用，但精确文本检索只找到本文件的 `AnalyzeColumnsExec` 在 `pkg/executor/analyze_col_test.rs` 中被构造；Rust 主调度 `pkg/executor/analyze.rs::analyzeWorker` 使用的是该文件自己定义的 `analyzeColumnsExec`，采样实现 `pkg/executor/analyze_col_sampling.rs` 也定义了另一个泛型 `AnalyzeColumnsExec<B>`。因此，本文件现阶段是公开的、可单测的列下推移植边界，不应表述为已完整接入 Rust ANALYZE 主链。

## 核心职责

- 用 `analyzeRequestSpec` 和 `analyzeTransportSpec` 将逻辑请求参数与传输参数分开，并通过 `analyzeColumnRuntime` 抽象内存追踪、范围拆分、请求编码和 DistSQL 执行。
- 由 `AnalyzeColumnsExec::open` 安装语句级内存追踪器，按有符号 `int64` 边界处理 handle 范围，并用 `tableResultHandler` 保存一段或两段结果流。
- 由 `AnalyzeColumnsExec::buildResp` 根据快照开关、store batch 配置、资源组和 DistSQL 上下文组装一次列统计请求。
- 判断列或索引是否被 Public、单列、无前缀、无条件的唯一索引覆盖，为采样统计避免无意义 TopN 提供判定函数。
- 生成 `analyze table ... with ...` 作业描述，过滤隐藏 row handle、变更中列和删除中列，并在 v2 选项存在时优先使用已填充选项。

本文件不负责消费 `selectResult`、构建直方图/TopN、合并采样、持久化统计或调度 ANALYZE worker；这些能力在 Go 中延伸到 `analyze_col_sampling.go` 和 `analyze.go`，Rust 当前则由其他尚未与本类型统一的模块承担。

## 主要符号

### 请求、元数据与错误

- `AnalyzeColumnError(String)` / `AnalyzeColumnResult<T>`：本模块运行时边界的统一错误和值类型；错误只保存文本并实现 `Display` 与 `Error`。
- `analyzeContext { requestID }`：传给运行时 `analyze` 的最小请求上下文。
- `schemaState`、`indexColumn`、`indexInfo`、`columnInfo`、`tableInfo`：本地化的表结构快照。`indexInfo::{hasPrefixIndex, hasCondition}` 支撑唯一索引资格判断；`tableInfo::nonTemporaryColumnCount` 计算作业文案中的有效列总数。
- `handleCols::{Int, Common}` 与 `hasPkHist`：区分整型主键 handle 和 common handle；只有 `Int` 具有独立 PK 直方图。
- `keyRange { low, high }`：半开扫描范围 `[low, high)` 的字节表示。
- `columnAnalyzeRequest`、`analyzeRequest`：列采样、桶、TopN、CMS/FM sketch、扩展统计和 NDV 参数；`opaqueFields` 保留本模块不理解的序列化字段。
- `analyzeOptionType`、`v2AnalyzeOptions`、`analyzeInfo`、`analyzeJob`：选项标签、v2 已填充选项及最终 `jobInfo` 容器。

### 运行时接口与执行器

- `memoryTracker`：要求运行时实现 `attach_to_statement` 和 `detach`。
- `selectResult`：要求下推结果流实现可失败的 `close`。
- `analyzeColumnRuntime`：依次抽象 `new_memory_tracker`、`split_ranges_across_int64_boundary`、`build_request` 和 `analyze`，是本文件与真实内存系统、range 编码和 DistSQL 客户端之间的注入点。
- `baseAnalyzeExec`：保存表 ID、并发度、请求、选项、快照、资源组、客户端/KV 变量、DistSQL context ID 及运行时对象。
- `AnalyzeColumnsExec`：在基础配置上增加表/列/索引信息、handle 类型、结果处理器、采样 wait group、虚拟列 schema、基线计数和内存追踪器。
- `tableResultHandler`：保存可选第一段结果和必需第二段结果；`set_results` 只转移所有权，不读取或关闭流。

### 判定与文案函数

- `isSingleColNonPrefixUniqueIndex`：要求索引为 `Public`，是 unique 或 primary，恰有一列，且没有前缀和条件。
- `isColumnCoveredBySingleColUniqueIndex`：在表的索引集合中复用上述判定，并比较唯一索引列的 `offset`。
- `prepareColumns`、`prepareIndexes`：把选定对象写入作业字符串，或在覆盖全量时写 `all columns` / `all indexes`。
- `prepareAnalyzeColumnsJobInfo`：处理 auto-analyze 前缀、索引/列组合、桶数、TopN、样本数或采样率，并写回可选的 `baseAnalyzeExec.job`。

模块没有条件编译项；顶部 `#![allow(...)]` 只是允许沿用 Go 命名，不改变行为。

## 执行流程

### 打开列下推执行器

1. `AnalyzeColumnsExec::open` 调用 `runtime.new_memory_tracker(planID, -1)`；`-1` 表示本文件不设置固定字节上限。
2. 调用 tracker 的 `attach_to_statement`，成功后把 `Arc` 存入 `self.memTracker`，并初始化空的 `tableResultHandler`。
3. 调用 `split_ranges_across_int64_boundary(ranges, true, false, !hasPkHist(...))`。最后一个参数在没有整型 PK 直方图时为真；具体如何编码和拆分由运行时实现决定。
4. 对第一组范围调用 `buildResp`。若第二组为空，将第一组结果保存为 handler 的 `secondResult`，`firstResult` 留空；这是 handler 对“单流”和“双流”的统一布局。
5. 若第二组非空，再调用一次 `buildResp`，随后把两段结果分别保存为 `firstResult` 和 `secondResult`。

### 构建并发送单次请求

1. `buildResp` 由 `handleCols` 推导 `commonHandle`：存在且不是 `Int` 时为真。
2. `enableAnalyzeSnapshot` 为真时使用 `snapshot` 和 `SnapshotIsolation`；否则使用 `u64::MAX` 和 `ReadCommitted`。
3. `analyzeStoreBatchSize > 0` 同时开启 `allowBatchTaskDataMerge` 与 `executeBatchTasksSerially`；`keepOrder` 固定为 `false`，因为全采样统计会在采样后恢复 handle 顺序。
4. `runtime.build_request` 接收表 ID、范围、分析参数、并发度、资源组信息和已经安装的 tracker。若调用方绕过 `open` 直接调用 `buildResp` 且未填充 `memTracker`，这里会因 `expect` 触发 panic。
5. 编码成功后，`runtime.analyze` 接收上下文、请求字节以及 client ID、KV 变量、restricted SQL 标志和 DistSQL context ID，返回一个所有权交给 handler/调用方的 `selectResult`。

### 生成作业描述

`prepareAnalyzeColumnsJobInfo` 对 `None` 立即返回；否则先选取 `analyzeInfo.v2Options.filledOptions`，没有 v2 选项时才用基础 `options`。restricted SQL 添加 `auto ` 前缀，然后依次拼接索引、逗号、列和 `with` 选项。桶数和 TopN 仅在映射含键时输出；非零样本数优先输出 `samples`，否则输出请求中的浮点 `sampleRate`。只有 `job` 存在时才写回字符串。

## 数据与状态

- `AnalyzeColumnsExec` 的可变生命周期状态主要是 `memTracker` 和 `resultHandler`；其余字段是建请求、采样和描述作业所需的配置或表结构快照。
- `Arc<dyn analyzeColumnRuntime>` 和 `Arc<dyn memoryTracker>` 允许执行器及外部调度共享运行时对象；本文件自身不修改原子 worker 计数，也不启动线程。
- `notifyErrorWaitGroupWrapper` 与 `waitGroupWrapper` 仅持有 `Arc<AtomicUsize>`。本文件没有增减或等待逻辑，它们只是为了与完整执行器形状对齐。
- `tableInfo::nonTemporaryColumnCount` 先排除 temporary/removing 列并按小写名称去重。对于 changing 列，它会从 `_Col$_<原名>_<后缀>` 推回原名并从集合移除，避免在线 modify-column 阶段把原列和过渡列重复计入；`analyze_col_test.rs::non_temporary_column_count_matches_go_modify_column_semantics` 覆盖该规则。
- `prepareColumns` 假定 `ExtraHandleID == -1` 的伪 `_row_id` 若存在则位于 `colsInfo` 最后；它只剔除末项，不扫描中间位置。
- 选项用 `BTreeMap` 保存，因此状态确定且可比较；实际输出顺序不是映射迭代顺序，而是函数明确规定的 buckets、topn、samples/samplerate。

## 依赖与调用关系

### 上游

- crate 装配：`pkg/executor/lib.rs` 公开 `analyze_col`，测试模块 `analyze_col_test` 独立声明，符合源码与单元测试分文件约束。
- 当前 Rust 直接行为验证：`pkg/executor/analyze_col_test.rs::analyze_request_is_unordered_for_store_batching` 构造本文件的 `AnalyzeColumnsExec`，调用 `open` 和 `buildResp`。
- RustCodeGraph 对 `buildResp` 给出的调用者包括本文件的 `open` 和上述测试；对 `prepareColumns`/`prepareIndexes` 给出的有效边是 `prepareAnalyzeColumnsJobInfo -> prepareColumns/prepareIndexes`。
- Go 生产上游：`pkg/executor/builder.go` 构造 Go `AnalyzeColumnsExec`，`pkg/executor/analyze.go` 在投递任务前调用 `prepareAnalyzeColumnsJobInfo`，`pkg/executor/analyze_col_sampling.go` 使用两个唯一索引判定函数决定将 TopN 数量置零。

### 下游

- 本文件直接使用的标准库依赖仅有集合、格式化、`Arc` 与 `AtomicUsize`。
- 实际系统依赖均被 `analyzeColumnRuntime` 和两个 trait 隔离：范围拆分、请求序列化、DistSQL 调用、语句内存树以及结果关闭都不在本文件实现。
- `pkg/executor/Cargo.toml` 证明所属 crate 直接声明了 `astersql-distsql`、`astersql-distsql-context`、`astersql-kv`、`astersql-statistics*`、`astersql-util-memory`、`astersql-util-ranger*` 等完整执行链依赖；但本文件没有直接 `use` 这些 crate，不能仅凭 manifest 推断 trait 已绑定到生产实现。

Rust 生产主链的相关但不同类型是 `pkg/executor/analyze.rs::analyzeColumnsExec`：`AnalyzeExec::analyzeWorker` 经 `analyzeRuntime::analyze_columns` 调用它。另一个 `pkg/executor/analyze_col_sampling.rs::AnalyzeColumnsExec<B>` 承担 Rust 采样逻辑。新增接线前必须先决定统一、适配还是保留这些同名类型，避免误连。

## 错误处理与边界

- `new_memory_tracker`、`attach_to_statement`、`build_request` 和 `analyze` 的错误都通过 `?` 原样传播为 `AnalyzeColumnError`；本文件不增加阶段或表 ID 上下文。
- `open` 在 tracker attach 成功后若后续失败，不会在本文件内调用 `detach`；`memoryTracker::detach` 仅是接口能力。调用者或未来的 guard 必须负责释放。
- `selectResult::close` 同样没有被本文件调用。尤其第二段 `buildResp` 失败时，Rust 会直接返回错误并丢弃局部 `first_result`；除非具体结果类型的析构自行关闭资源，否则没有显式 close。Go `AnalyzeColumnsExec.open` 在相同分支使用 `errors.Join(err, firstResult.Close())`，这是当前 Rust 与 Go 的明确语义缺口。
- `buildResp` 对缺失 `memTracker` 使用 `expect`，而不是返回错误；安全调用顺序是不绕过 `open`，或在测试/适配层预先安装 tracker。
- `resultHandler` 初始化后的 `as_mut().expect(...)` 依赖本函数前面刚写入的内部不变量，外部无法在两句之间并发清空该字段，因为 `open` 持有 `&mut self`。
- `isColumnCoveredBySingleColUniqueIndex` 仅在 `isSingleColNonPrefixUniqueIndex` 已保证 `columns.len() == 1` 后访问 `columns[0]`，因此空索引列不会越界。
- 作业文案函数不会验证名称是否需要 SQL 转义，也不会在 `job == None` 时报告错误；其输出是展示文本，不是可重新执行的 SQL。

## 并发与资源生命周期

本文件没有创建线程、异步任务或通道。`open(&mut self)` 串行地改变执行器状态，`buildResp(&self)` 只读取执行器并调用要求 `Send + Sync` 的共享 runtime。并发度字段只是传给下游请求的数值。

资源生命周期如下：运行时创建 tracker → tracker attach 到语句 → tracker 存入执行器 → runtime 构建请求并返回结果流 → 结果流存入 handler。文件定义了 `detach` 和 `close`，却没有对应的 `Drop`、`close` 或回滚路径；因此资源收尾是当前边界最需要调用者保证的约束。`tableResultHandler::set_results` 覆盖旧字段时也不会先显式关闭旧流，执行器不应在未清理旧 handler 的情况下重复 `open`。

`samplingBuilderWg`、`samplingMergeWg` 和 `samplingStatsConcurrency` 反映 Go 完整采样执行器的并发形状，但这里没有消费逻辑。Go 源码还注明 `samplingStatsConcurrency` 必须在主 goroutine 读取，因为 session variables map 不支持分区 worker 并发查找；Rust 本文件只保存该值，尚未实现这一并发约束。

## 与 Go 版本的对应关系

### 已保持的语义

- 唯一索引资格条件与 `pkg/executor/analyze_col.go::{isColumnCoveredBySingleColUniqueIndex,isSingleColNonPrefixUniqueIndex}` 一致：Public、unique/primary、单列、无前缀、无条件。
- `open` 都安装不限额的语句级 tracker、拆分整型边界、先建第一段请求，并在有第二段时保存双结果。
- `buildResp` 都以快照开关选择 SI/RC，以 ANALYZE 专用 store batch size 同时控制 merge/serial 标志，并固定无序扫描。
- 作业文案的 auto 前缀、索引/列全量或子集、选项顺序以及 samples 与 samplerate 二选一规则与 Go 一致。
- Rust 测试 `analyze_request_is_unordered_for_store_batching` 对应 Go `pkg/executor/analyze_test.go::TestAnalyzeBuildsRequest` 的核心请求断言：`keepOrder=false`、并发度传播、batch size 传播以及两个 batch 布尔标志随零值关闭。

### 已知差异与迁移状态

- Go 文件直接使用 TiDB 的 `model.TableInfo`、`plannerutil.HandleCols`、`memory.Tracker`、`distsql.RequestBuilder` 和 `distsql.Analyze`；Rust 使用轻量本地结构和 trait 注入，目前未见这些 trait 到真实 crate 类型的生产适配。
- Go 的 range split 调用传入的 keep-order 参数为 `false`，Rust 调用传入 `true`；两者随后都把最终请求 `keepOrder` 设为 `false`。这一参数影响拆分函数本身，接入真实 runtime 前必须核对其约定，不能假定等价。
- Go 在第二段请求失败时显式关闭第一段并合并错误，Rust 没有该清理。
- Go 在请求构建后提供 `analyzeColumnsRequestBuilt` failpoint，并将 `planID` 传给 `distsql.Analyze`；Rust spec/transport 没有等价 failpoint，`planID` 只用于创建 tracker。
- Go 生产 builder 会填充 protobuf 列信息、common PK ID、默认值、基线计数和虚拟列 schema；本文件只定义其数据形状，未实现 builder 接线。
- Rust 的 `tableInfo::nonTemporaryColumnCount` 是对 Go `GetNonTempColumns` 相关 modify-column 语义的本地近似，并有独立单测；后续元数据模型接入后应改为复用权威实现而非继续复制规则。

## 扩展指南

1. 接入生产主链时，先梳理并统一 `analyze_col.rs::AnalyzeColumnsExec`、`analyze.rs::analyzeColumnsExec` 与 `analyze_col_sampling.rs::AnalyzeColumnsExec<B>`，再实现 `analyzeColumnRuntime` 的真实 adapter。不可仅把测试 runtime 换成 DistSQL 调用便宣称完成。
2. 修改请求字段时，同步更新 `analyzeRequestSpec`、`analyzeTransportSpec`、`buildResp` 和 runtime 实现，并在独立的 `pkg/executor/analyze_col_test.rs` 增加捕获 spec/transport 的断言；不要把测试嵌回源文件。
3. 修改 handle 范围逻辑时，重点覆盖：有符号/无符号边界、空的第一或第二半区、Int handle、common handle 和无 handle。还应先消除 keep-order 拆分参数与 Go 的差异。
4. 修改资源生命周期时，优先引入显式 close/guard，保证第二段构建失败会关闭第一段、所有失败路径 detach tracker、重复 open 不覆盖未关闭结果；测试应使用记录 close/detach 次数的独立 fake。
5. 修改作业文案时，同步 Go `prepareAnalyzeColumnsJobInfo` 的选项顺序与 auto-analyze 规则，并覆盖空列、末尾 `ExtraHandleID`、changing/removing 列、全量/部分索引、v2 options 和 `job=None`。
6. 修改唯一索引优化时，同步 `analyze_col_sampling` 中 TopN 抑制的调用点，覆盖非 Public、复合、前缀、条件、普通非唯一和 primary index。

兼容风险主要是 job 文案成为可观测状态、隔离级别或 start timestamp 改变读取一致性、range split 改变无符号主键覆盖面。性能风险主要是错误开启有序扫描、batch flags、错误并发度，以及未关闭结果流或未 detach tracker 造成资源积累。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件 `node` 覆盖 `pkg/executor/analyze_col.rs` 全部 616 行。
- RustCodeGraph 符号查询：定位了 Rust/Go 两份 `AnalyzeColumnsExec`、`isColumnCoveredBySingleColUniqueIndex`、`isSingleColNonPrefixUniqueIndex`、`hasPkHist` 和 `prepareAnalyzeColumnsJobInfo`；流程查询确认 Rust `open -> buildResp`、`prepareAnalyzeColumnsJobInfo -> prepareIndexes/prepareColumns`，以及测试对 `buildResp` 的调用。
- 读取的 Rust 生产文件：`pkg/executor/analyze_col.rs`、`pkg/executor/analyze.rs`、`pkg/executor/analyze_col_sampling.rs` 的直接引用，以及 `pkg/executor/lib.rs` 的模块声明。
- 读取的 crate 配置：`pkg/executor/Cargo.toml`；确认 package 为 `astersql-executor`、lib 根为 `lib.rs`、`nextgen` 是唯一声明 feature，且完整执行链依赖位于 crate 层。本文件没有 feature gate。
- 读取的 Rust 测试：`pkg/executor/analyze_col_test.rs`；覆盖 modify-column 列计数和无序/batch 请求参数。`pkg/executor/analyze_test.rs` 被图索引列为模块使用者，但其中的 `analyze_columns` 针对 `analyze.rs` 的 runtime 接口，不是本文件执行器的直接行为测试。
- 读取的 Go 对照：`pkg/executor/analyze_col.go` 全文件，以及直接上游 `pkg/executor/builder.go`、`pkg/executor/analyze.go`、`pkg/executor/analyze_col_sampling.go` 的相关片段。
- 读取的 Go 测试：`pkg/executor/analyze_test.go::TestAnalyzeBuildsRequest`，验证无序请求、unsigned handle 边界、专用并发度、store batch 参数及其零值关闭行为。
- 包契约检查：仓库中不存在 `pkg/executor/doc.go`，因此没有可读取的包级 `doc.go`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证只执行任务指定的 11 章节结构检查并人工核对上述事实链。
