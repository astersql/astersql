# `pkg/executor/join/index_lookup_join.rs`

## 文件定位

本文件属于 `astersql-executor-join` crate，是 Index Lookup Join（索引查找连接）的 Rust 行为模型。模块由 [`pkg/executor/join/lib.rs`](lib.rs) 以 `pub mod index_lookup_join` 导出；crate 边界和依赖记录在 [`pkg/executor/join/Cargo.toml`](Cargo.toml)。当前可执行代码从源文件第 675 行开始，依赖同 crate 的 `joiner::{Joiner, NaajType, Predicate, Row}`、`row_table_builder::Value`，以及 `astersql-executor-internal-exec` 的自适应 LIMIT 控制器类型。

文件第 22–674 行是一整段注释化的 Go 迁移草图，保留了 Go 版 worker、通道、内存 tracker、range/collation 等控制流以便对照，但不参与 Rust 编译。分析当前行为时必须以第 675 行之后的声明为准，不能把草图中的并发 worker、取消通道或内存跟踪器当成已经实现的 Rust 能力。

在完整构建链中，物理 `IndexJoin` 由 [`pkg/executor/builder.rs`](../builder.rs) 的 `executorBuilder::buildIndexLookUpJoin` 进入 `build_index_join_kind`，后者校验 inner condition、创建 `DataReaderBuilder`，再调用 `ExecutorBuilderDependencies::build_index_join_executor`。该生产接线没有在本文件中直接展开；仓库内对 `IndexLookUpJoin::new` 的明确直接调用主要位于独立测试和 benchmark。因此本文件提供可执行 join 核心与 builder 抽象，但“物理计划一定实例化为本类型”不能仅凭本文件断言。

## 核心职责

当前可执行实现承担五项职责：

1. `OuterWorker` 把 `OuterCtx.rows` 按指数增长的批大小切成 `LookUpJoinTask`，并执行外表过滤器。
2. `InnerWorker::construct_lookup_content` 从通过过滤的外表行提取 lookup key，按 `compare_row` 排序并去重；普通等值语义下跳过含 `NULL` 的外键。
3. `IndexJoinExecutorBuilder::build` 把去重后的 lookup 内容交给实际内表读取实现；本文件只定义边界，不负责真实 KV range 构造或索引回表。
4. `InnerWorker::build_lookup_map` 把内表结果按 key 编码到 `HashMap<Vec<u8>, Vec<Row>>`，随后 `IndexLookUpJoin::execute` 按外表原始顺序调用 `Joiner` 产出匹配行或未匹配补行。
5. `IndexLookUpJoinRuntimeStats` 累计任务、阶段耗时并保存一次执行生命周期的自适应 LIMIT 快照。

当前实现是“分批但同步”的：`IndexLookUpJoin::execute` 在调用线程内创建一个借用 `InnerCtx` 的 `InnerWorker` 并立即执行 `handle_task`。`keep_outer_order` 会设置 `adaptive_limit_eligible`，输出也确实按批次和外表行顺序生成，但代码没有根据该字段切换乱序模式。

## 主要符号

- `RowFilter = Predicate`：行级过滤器别名。只有谓词返回 `Some(true)` 才通过；`None` 与 `Some(false)` 均视为不通过，错误直接传播。
- `IndexJoinLookupContent`：一次 lookup 的去重单位，携带外表提取的 `keys`、原始 `row`、内表 key 下标 `key_columns` 和列 ID `key_column_ids`。后两者由 builder 用于解释内表键；本文件不消费列 ID。
- `IndexJoinExecutorBuilder`：`Send + Sync` trait，唯一方法 `build(&[IndexJoinLookupContent]) -> Result<Vec<Row>, String>`。它隔离真实索引读取、分区裁剪或测试内存数据源。
- `OuterCtx`：外表全量行、外表 join key 列和过滤器。它是当前内存模型，而非 Go 版流式 child executor。
- `InnerCtx`：持有 boxed builder、内表 key 列及列 ID，并以单个 `null_safe: bool` 控制全部 key 的 `<=>` 语义。
- `LookUpJoinTask`：批次状态，包含外表行、过滤标记、去重 lookup 内容、内表结果、lookup map、保留的 `cursor` 和完成标志。`new` 默认所有外表行均可 lookup。
- `OuterWorker`：维护初始/当前/最大批大小与外表游标；`new`、`build_task`、`increase_batch_size`、`reset` 组成生命周期。
- `InnerWorker<'a>`：借用 `InnerCtx`；`construct_lookup_content`、`fetch_inner_results`、`build_lookup_map` 和 `handle_task` 组成内表阶段。
- `IndexLookUpJoin`：对外执行器对象；`new` 组装上下文、`Joiner` 与 worker，`open` 重置周期，`next` 惰性触发 `execute` 并分页返回缓冲结果，`close` 清理当前周期。
- `InnerWorkerRuntimeStats` / `IndexLookUpJoinRuntimeStats`：保存耗时、任务数、并发显示字段和自适应 LIMIT 快照；`merge` 累加计数/耗时但不覆盖接收者的 `concurrency`，快照只保留第一份。
- `compare_row`、`extract_key`、`encode_key`：分别负责 key 排序、按列抽取和 map 键编码。`encode_key` 当前使用 `Value` 的 `Debug` 字符串字节，是迁移期表示，不是 Go 的类型感知 `codec.EncodeKey`。
- `filters_match`、`compare_value`：私有辅助函数；后者对同型值自然排序、令 `NULL` 最小，异型值回退到 `Debug` 字符串排序。

文件没有条件编译项、模块级常量或 `unsafe` 可执行代码。`IndexLookUpJoinRuntimeStats::TYPE = 6` 是关联常量，对齐运行统计类型编号。

## 执行流程

1. 构造：`IndexLookUpJoin::new` 先调用 `OuterWorker::new`。初始批或最大批为 0 会返回 `batch sizes must be positive`；初始值大于上限时被截到上限。`adaptive_limit_eligible` 初值等于 `keep_outer_order`，控制器默认为空。
2. 打开：显式 `open` 会重置外表游标和批大小，清空结果与读取游标，并把 `opened=true`、`closed=false`、`executed=false`。首次 `next` 若尚未打开也会隐式调用 `open`；但 `close` 后的 `next` 明确报错，必须先显式 `open`。
3. 建外表任务：`OuterWorker::build_task` 在切片前先调用 `increase_batch_size`，所以初始批 1、上限 2 时首批读取 2 行。每行依次通过全部 `filters`；过滤失败只禁止索引 lookup，不从外连接输出中删除该外表行。
4. 构造 lookup：`InnerWorker::construct_lookup_content` 跳过过滤未通过的行；逐列检查下标；普通等值连接遇到任一 `NULL` 时跳过，`null_safe=true` 时保留。内容按 key 排序并按 key 去重，减少 builder 的重复查找。
5. 拉取与建表：`fetch_inner_results` 用整个去重内容切片调用 builder；`build_lookup_map` 清空旧 map、抽取内表 key，并在普通等值模式跳过含 `NULL` 的内表行。相同 key 的多行按 builder 返回顺序保存在 `Vec<Row>` 中。
6. Probe：`IndexLookUpJoin::execute` 逐批同步执行 `InnerWorker::handle_task`，再按 `task.outer_rows` 顺序探测。过滤未通过的行直接调用 `Joiner::on_miss_match(false, ...)`；其余行先编码外键，再用 `Joiner::try_to_match_inners(..., NaajType::Unknown)` 应用 join 类型与附加条件，未匹配时携带 `has_null` 调用 `on_miss_match`。
7. 返回：整个输入在首次非零 `next` 中一次性执行并缓存在 `output`，之后按 `required_rows` 切片复制返回；`required_rows == 0` 不触发执行。耗尽后稳定返回空向量，不重复执行。
8. 关闭与统计：`close` 若存在自适应控制器则获取一次 `Snapshot`，随后清空输出、复位游标和执行标志并设置 `closed=true`。再次执行需显式 `open`。

## 数据与状态

`IndexLookUpJoin` 的状态机由 `opened`、`closed`、`executed` 和结果 `cursor` 共同表达。新对象既未打开也未关闭；首次 `next` 可隐式打开。`execute` 成功后才把 `executed` 设为 true，因此构造 key、builder 或 joiner 报错时不会提交完成状态。当前代码也不会回滚已经写入 `output` 的部分结果；调用者在错误后继续 `next` 可能再次执行，所以错误后的复用语义应谨慎，相关语义测试专门覆盖重复错误路径。

任务级数据遵循“先清空再重建”：`construct_lookup_content` 清空 `lookup_contents`，`fetch_inner_results` 覆盖 `inner_rows`，`build_lookup_map` 清空 `lookup_map`。同一 `LookUpJoinTask` 被复用时不会保留上一批 key/map。`done` 只在 `handle_task` 三阶段全部成功后置 true；当前同步执行器不读取这个标记，它主要保留 Go task 完成语义。

键一致性依赖同一对列配置：外表由 `OuterCtx.key_columns` 提取，内表由 `InnerCtx.key_columns` 提取，builder 又接收内表 key 元数据。任何一侧下标越界都返回字符串错误。当前 `InnerCtx.null_safe` 是全键布尔值，不能表达 Go `HashIsNullEQ` 的逐 key 混合普通等值和 NULL-safe 等值。

`output` 保存整个执行结果，所以内存规模与完整连接结果成正比，而不是与单个 `next` 批次成正比。`lookup_map` 和 `inner_rows` 是每个外表批次的临时值，批次结束后随局部 task 释放。统计中的阶段字段虽然齐全，但当前实现只更新 `inner_worker.total_time`、`inner_worker.tasks` 和整体 `probe`；`construct/fetch/build/join` 仍保持零值。

## 依赖与调用关系

上游与装配关系：

- [`pkg/executor/join/lib.rs`](lib.rs) 公开 `index_lookup_join`，并通过独立的 `index_lookup_join_test.rs` 挂接 crate 内单测，符合测试不内嵌源文件的约束。
- [`pkg/executor/builder.rs`](../builder.rs) 的 `buildIndexLookUpJoin -> build_index_join_kind -> ExecutorBuilderDependencies::build_index_join_executor` 是物理计划侧入口。最后一步属于注入边界，当前搜索未证明它必然直接调用 `IndexLookUpJoin::new`。
- `IndexLookUpJoin::new` 的已确认直接 Rust 调用者包括 [`pkg/executor/benchmark_test.rs`](../benchmark_test.rs)、[`pkg/executor/join/index_lookup_join_test.rs`](index_lookup_join_test.rs) 和 [`pkg/executor/join/test/indexjoin/index_lookup_join_test.rs`](test/indexjoin/index_lookup_join_test.rs)。
- `index_lookup_hash_join.rs` 与 `index_lookup_merge_join.rs` 复用本文件的 `IndexJoinExecutorBuilder`、`IndexJoinLookupContent`、`encode_key`、`extract_key` 或 `compare_row`，因此这些公共 helper 的表示与错误契约会影响另外两种索引连接变体。

下游调用链为：

`IndexLookUpJoin::next -> open（按需） -> execute -> OuterWorker::build_task -> filters_match -> InnerWorker::handle_task -> construct_lookup_content / fetch_inner_results / build_lookup_map -> IndexJoinExecutorBuilder::build / extract_key / encode_key -> Joiner::try_to_match_inners 或 Joiner::on_miss_match`。

`close -> AdaptiveLimitController::Snapshot` 形成统计侧依赖；`IndexLookUpJoinRuntimeStats::fmt` 读取快照字段并生成 `adaptive:{...}` 诊断文本。Cargo 中非 Windows 的直接依赖恰为 `astersql-executor-internal-exec` 和 `astersql-util-execdetails`；本文件实际使用前者，后者由 crate 内其他模块或条件路径使用。大量完整执行器依赖被放在 `cfg(windows)` 目标段，说明该 crate 的平台装配仍有迁移边界，不能从 manifest 推断当前文件具有 Go 版所有运行时设施。

## 错误处理与边界

- 构造边界：零批大小由 `OuterWorker::new` 拒绝；所有公开执行路径统一使用 `Result<_, String>`，没有结构化错误类型。
- 列边界：外表或内表 key 下标越界由 `extract_key`/`construct_lookup_content` 返回包含列号的错误，不会 panic；builder 自己访问行列的安全性由其实现负责。
- 过滤边界：任一过滤器返回错误即终止当前任务；`None` 按 SQL 三值逻辑视为不通过。对左外连接，过滤未通过仍进入 `on_miss_match`，保留外表行。
- NULL 边界：普通等值模式同时跳过 outer 与 inner 的 NULL key；NULL-safe 模式两侧都保留。多列时当前开关作用于全部列，弱于 Go 的逐列标记。
- builder/joiner 错误：`IndexJoinExecutorBuilder::build` 和 `Joiner::try_to_match_inners` 的错误原样向 `next` 传播。独立测试覆盖立即/延迟 builder 错误和 probe 条件错误。
- 生命周期边界：耗尽后返回空；`required_rows=0` 返回空但不执行；`close` 后隐式重开被禁止并返回 `cannot reopen closed index lookup join`，显式 `open` 可重新执行保留的输入。
- 表示边界：`encode_key` 使用 `Debug` 文本，异型 `compare_value` 也回退到 `Debug` 文本。这不是稳定的存储编码或完整 SQL 类型/排序规则语义；新增类型、浮点特殊值、collation 或跨版本持久化时不可依赖它。
- 迁移缺口：可执行代码没有 Go 版的类型转换后等值检查、invalid datetime 特判、前缀索引裁剪、collator、动态 range、内存 tracker、context 取消、panic 恢复、真实并发 worker 和流式 chunk 生命周期。注释化草图只能作为对照证据。

## 并发与资源生命周期

尽管类型注释仍使用“outer worker/inner worker”术语，当前 `IndexLookUpJoin::execute` 没有创建线程、任务、通道、锁或原子变量；一个 `InnerWorker` 在调用线程内同步处理每个批次。`IndexJoinExecutorBuilder: Send + Sync` 允许 builder 被并发环境持有，但本文件不会并发调用同一个 builder。集成语义测试中的 `ConcurrentBuilder` 和外部线程用于验证共享 builder/storage 场景，不能反证本执行器内部已经并行。

资源生命周期如下：`new` 持有 outer/inner 上下文和 builder；`open` 重置可重复执行状态；首次有效 `next` 物化完整输出；后续 `next` 只移动游标；`close` 清空结果但保留输入、builder 和 joiner，以便显式 `open` 后重跑。没有显式 drop、inner executor close、worker join 或取消信号。

自适应 LIMIT 控制器以 `Option<Arc<AdaptiveLimitController>>` 共享，但本文件不调用 admission/commit，也不根据 controller 限制外表抓取或 lookup 数量；只在 `close` 拍快照。因此 `adaptive_limit_eligible` 和 controller 当前主要承担资格/诊断接线，而非本文件内的背压实现。

与 Go 相比，资源风险集中在全量 `output` 物化与每批 `Vec<Row>` 克隆：`next(required_rows)` 的返回粒度不会限制首次执行内存。扩展并发前必须先定义 task 所有权、builder 共享方式、错误取消、顺序恢复与统计同步，不能仅把同步循环包进线程。

## 与 Go 版本的对应关系

Go 对照文件是 [`pkg/executor/join/index_lookup_join.go`](index_lookup_join.go)。两者保留的主干语义包括：外表批大小先增长再读取、过滤结果不参与 lookup、lookup key 排序去重、builder 拉取内表、key 到多行的 map、按外表顺序 probe、普通等值跳过 NULL、NULL-safe 等值允许 NULL、未匹配由 `Joiner` 处理，以及运行统计合并时不覆盖接收者 concurrency、快照保留首份。

主要差异如下：

| 方面 | Go 实现 | 当前可执行 Rust 实现 |
| --- | --- | --- |
| 输入 | child executor + chunk/list，按需读外表 | `OuterCtx.rows: Vec<Row>` 全量驻留 |
| worker | 一个 outer goroutine、多个 inner goroutine、通道、WaitGroup、context cancel | 同一线程内顺序 `build_task` 和 `handle_task` |
| key 语义 | Datum 转换、collator、逐列 `HashIsNullEQ`、前缀索引、codec 编码 | `Value` 克隆、全局 `null_safe`、Debug 字节编码 |
| 内表 | 构造并驱动真实 inner executor，可分批 fetch/close | builder 一次返回 `Vec<Row>` |
| 内存 | tracker 按 task/chunk/worker 记账 | 普通 Vec/HashMap，无 tracker |
| 错误/取消 | context、panic recover、done channel、failpoint | 同步 `Result<String>` 传播 |
| 输出 | `Next` 按 chunk 增量 probe | 首次有效 `next` 先物化全部输出，再切片 |
| 自适应 LIMIT | 外表 reservation/commit 与读取协调 | 仅保存 eligibility/controller，并在 close 拍统计快照 |

Go SQL 回归 [`pkg/executor/join/test/indexjoin/index_lookup_join_test.go`](test/indexjoin/index_lookup_join_test.go) 覆盖 SQL 计划选择、NULL-safe 单/多键与外连接、分区数据、panic/取消、延迟 worker 错误和 CTE 并发构建。Rust 对应语义测试 [`pkg/executor/join/test/indexjoin/index_lookup_join_test.rs`](test/indexjoin/index_lookup_join_test.rs) 直接驱动当前类型，覆盖 NULL-safe、外表顺序、分批、builder 错误、重复读取、大量重复 key、共享存储和空 inner 等场景；它验证行为子集，不代表上述 Go 并发与类型系统细节已完整移植。

## 扩展指南

- 增加或修改 join 主流程时，优先改 `IndexLookUpJoin::execute`，并同步检查 `Joiner::try_to_match_inners` / `on_miss_match` 对 inner、outer、semi/anti/NAAJ 各类型的契约。回归应放在独立的 `index_lookup_join_test.rs` 或 `test/indexjoin/index_lookup_join_test.rs`，不要嵌入源文件。
- 增加 key 类型、collation、前缀索引或逐列 NULL-safe 语义时，应替换/扩展 `InnerCtx`、`construct_lookup_content`、`compare_row`、`compare_value` 和 `encode_key`，并同步复用这些 helper 的 index hash/merge join。兼容风险是排序与相等关系不一致导致 lookup 去重或 map probe 漏行。
- 接入真实 KV/index reader 时，在 `IndexJoinExecutorBuilder` 实现层保持 `lookup_contents` 去重契约，并明确 `key_column_ids` 的分区裁剪用途。builder 返回行必须与 `InnerCtx.key_columns` 对齐，否则 map 建立会报错或错误匹配。
- 实现 Go 等价并发时，需要同时引入 outer/inner task 队列、错误与取消传播、保序结果队列、close 等待、panic 隔离、每 worker 独立可变状态和内存追踪；同时更新 `stats.concurrency` 及各阶段计时。不得只并行 builder 调用而忽略输出次序和关闭路径。
- 改变 `next` 的流式策略时要保持：`required_rows=0`、耗尽稳定空、错误后状态、close 后显式 reopen、外表过滤未命中仍保留外连接行。当前全量物化是主要性能风险，也是最值得独立设计的扩展点。
- 修改自适应 LIMIT 时，不能只更新 close 快照；应与 `AdaptiveLimitController` 的 outer/lookup admission、consume、commit 协议一起接线，并增加提前停止和 outstanding 统计的测试。
- 修改统计格式或合并规则时，同步 [`pkg/executor/join/join_stats_test.rs`](join_stats_test.rs)；修改批处理、去重、NULL、过滤或重开行为时同步 [`pkg/executor/join/index_lookup_join_test.rs`](index_lookup_join_test.rs)；SQL/Go 对齐场景同步 `test/indexjoin` 下独立 Rust 测试并对照同名 Go 测试。

## 验证依据

本说明基于以下可复核证据：

- RustCodeGraph `status`：索引存在，覆盖 7,032 个 Rust 文件和 4,415 个 Go 文件；目标文件被识别为 1,171 行、41 个符号。
- RustCodeGraph `files --filter pkg/executor/join`：确认同目录模块、Go 对照、crate 内测试和 `test/indexjoin` 语义测试的位置。
- RustCodeGraph `node --file pkg/executor/join/index_lookup_join.rs`：完整核对注释化迁移草图（22–674）与可执行实现（675–1171），包括所有公开类型、方法、私有 helper 和统计格式。
- RustCodeGraph `explore`、`query`：确认 `IndexLookUpJoin`、Go 同名符号、`startWorkers/newInnerWorker/Next/handleTask` 等 Go 调用链；Rust 方法级 `callers` 查询未在可接受时间内返回，因此用精确源码引用搜索补齐 Rust 直接调用者，并未把缺失图边当成“无调用者”。
- RustCodeGraph `node`：读取 [`pkg/executor/join/lib.rs`](lib.rs)、[`pkg/executor/builder.rs`](../builder.rs)、[`pkg/executor/join/index_lookup_join.go`](index_lookup_join.go)、[`pkg/executor/join/index_lookup_join_test.rs`](index_lookup_join_test.rs)、[`pkg/executor/join/test/indexjoin/index_lookup_join_test.rs`](test/indexjoin/index_lookup_join_test.rs)、[`pkg/executor/join/test/indexjoin/index_lookup_join_test.go`](test/indexjoin/index_lookup_join_test.go) 和 [`pkg/executor/join/join_stats_test.rs`](join_stats_test.rs)。
- 直接读取 [`pkg/executor/join/Cargo.toml`](Cargo.toml)：确认 crate 名、`lib.rs` 入口、porting 元数据、非 Windows 直接依赖和 Windows 条件依赖边界。
- 测试事实：crate 内单测覆盖批增长/过滤、过滤错误、去重与 map 重建、普通等值 NULL、统计 merge、外表保序、左连接未命中和 close/reopen；`test/indexjoin` 的 Rust 语义测试进一步覆盖 NULL-safe、多键、分区式 workload、立即/延迟 builder 错误、probe 错误、重复 key 和空 inner；Go 测试提供 SQL 计划及并发/取消/故障注入对照。

本任务是纯文档分析，按计划不运行 Cargo，也不声称这些测试在本次会话中执行通过。结构验证用于确认本文存在且恰好包含规定的十一个二级章节。
