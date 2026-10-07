# `pkg/ddl/backfilling.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-ddl`，由 `pkg/ddl/lib.rs` 以 `pub mod backfilling` 暴露。它位于 Online DDL 的数据重组（reorg）基础层：定义回填任务、批次结果和索引回填请求的数据模型，并提供范围校验、范围切分、警告归并、顺序进度归并等纯算法工具。这里不负责 DDL job 的持久化、schema 状态转换、owner 调度或真实 KV 事务；这些职责分别由 `job_worker.rs`、`reorg.rs` 及 session/存储适配层承担。

当前 Rust 接线需要分开理解：`IndexBackfillBatch`、`ReorgBackfillTask`、`BackfillTaskContext`、`BackfillResult` 和 `merge_warnings_and_counts` 已被生产代码使用；其余若干公共工具目前只有独立测试或尚无直接 Rust 生产调用。因而本文件是“基础模型与算法集合”，不是 Go `backfilling.go` 中完整 worker master/worker pool 的等价实现。

## 核心职责

1. 用 `BackfillerType` 区分加索引、改列、清理索引、合并临时索引和重组分区五类回填，并提供稳定的显示名称和 `COUNT`。
2. 用 `ReorgBackfillTask` 表达物理表上的左闭右开键区间，用 `BackfillTaskContext` 表达单批结果，用 `BackfillResult` 汇总一个任务的累计进度。
3. 用 `handle_backfill_task` 驱动实现了 `Backfiller` trait 的处理器逐批推进，并防止“成功返回但键未前移”造成死循环。
4. 用 `validate_and_fill_ranges` 与 `split_ranges_by_keys` 修正、验证和细分存储 Region 范围；用 `get_batch_tasks` 把范围映射为连续编号的任务。
5. 用 `iterate_snapshot_keys` 在一个已经物化为有序行集合的简化快照上遍历目标前缀与范围。
6. 用 `DoneTaskKeeper` 将并发乱序完成的任务归并成可安全持久化的连续前缀；用 `LocalRowCountCollector` 记录本地吞吐统计。
7. 用 `IndexBackfillBatch` 把真实 session adapter 执行一次索引转换所需的 schema、表、索引、范围、批大小、资源组和 SQL mode 聚合为单个请求。

## 主要符号

- `type Key = Vec<u8>`：本文件内部任务使用的原始有序字节键。`KvKey`/`KvKeyRange` 则直接再导出自 `astersql-kv`，保留存储层的 `StartKey`、`EndKey` 和比较方法。
- `BackfillerType`：五成员枚举；`as_str`/`Display` 产生监控或诊断名称。注意 Rust 的 `CleanupIndex` 文本为 `"cleanup index"`，Go 对照是 `"clean up index"`。
- `KeyRange`：本地的 `[start_key, end_key)` 范围值，主要供 `get_batch_tasks` 使用，不等同于存储层 `KvKeyRange`。
- `ReorgBackfillTask`：携带 `physical_table_id`、任务 `id`、`job_id`、起止键和事务 `priority`。生产调用者会把它嵌入 `IndexBackfillBatch`。
- `BackfillTaskContext`：一次批处理返回 `next_key`、`done`、新增/扫描计数、警告及次数、`finish_ts`。
- `BackfillResult`：整个任务的累计结果，包含 `task_id`、最终 `next_key`、累计计数、累计警告和字符串错误。
- `Backfiller`：同步 trait，核心方法是 `backfill_data`；`add_metric_info` 默认为空，`name` 用于无进展错误。
- `merge_warnings_and_counts`：只遍历 `task.warnings` 中出现的错误码；既有警告保留首次文本并累加计数，新警告同时写入文本和计数。仅存在于 `warning_counts` 而不在 `warnings` 的“孤立计数”会被忽略。
- `handle_backfill_task`：通用批次循环。目前 `rg` 未发现 Rust 生产调用者，不能把它当成实际 `JobWorker` 主链；生产事务式索引回填在 `job_worker.rs::run_transactional_index_backfill` 中另有显式事务循环。
- `validate_and_fill_ranges`：裁剪首尾无界/越界 Region，拒绝空集合、开头缺口、中间无界边界和不连续范围，错误类型为共享 `ErrInvalidSplitRegionRanges`。
- `split_ranges_by_keys`：按已排序 split key 线性细分范围；等于边界或落在范围外的键不产生空子范围。
- `get_batch_tasks`：连续分配任务 ID，复制范围边界，优先级固定为 `0`。它没有 Go 版本 `getActualEndKey`、物理表对象、reorg 元数据和 worker 类型判断。
- `iterate_snapshot_keys`：在传入的 `(Key, Vec<u8>)` 切片中过滤 `[start_key, end_key)` 与前缀，逐条调用回调；继续扫描时在当前键后追加 `0` 作为返回进度。
- `DoneTaskKeeper`：以 `BTreeMap<任务 ID, next key>` 缓存提前完成的任务，仅当 `current` 连续时推进 `next_key`。
- `LocalRowCountCollector`：分别累加 accepted bytes、processed bytes 和 processed rows；当前未发现生产调用。
- `IndexBackfillBatch`：已接入 `job_worker.rs`、`persistent_modify_column.rs` 和 `partition.rs` 的真实批次请求 DTO。

## 执行流程

通用 `handle_backfill_task` 流程如下：先复制任务并把结果的初始进度设为任务起点；当当前起点小于终点时，先调用 `runnable` 检查取消/暂停条件，再调用 `Backfiller::backfill_data`。成功批次会累加 added/scan 计数、合并警告、上报 metric，并把结果进度更新到 `context.next_key`。`context.done` 为真时直接结束；否则要求 `next_key` 严格大于本批起点，再把它作为下一批起点。不可运行、下游错误或无进展都写入 `BackfillResult.error` 后返回已积累的部分结果。

实际已接线的事务式加索引路径是 `JobWorker::run_transactional_index_backfill`：它验证 owner lease、job/reorg 状态与持久化 job 元数据，构造 `IndexBackfillBatch`，通过 `DurableJobSession::backfill_index_batch` 在事务中处理一批，提交后使用本文件的 `merge_warnings_and_counts` 汇总警告，再由 `PersistentReorgHandler::stage_update` 单独发布 checkpoint。修改列索引路径和分区全局索引路径也构造相同 DTO，但具体执行由各自 context adapter 提供。

范围准备工具的顺序是：Region 查询结果先交给 `validate_and_fill_ranges` 收缩并验证连续覆盖，再由 `split_ranges_by_keys` 加入额外边界，最后可由 `get_batch_tasks` 生成任务。当前 Rust 生产代码未直接串联这三个函数；这是来自 Go 流程的工具级移植，而不是已验证的 Rust 生产调用链。

## 数据与状态

所有键区间都采用左闭右开语义 `[start, end)`，正常任务只有在 `start < end` 时才执行。`handle_backfill_task` 的累计状态存在返回值中，不自行写入 `mysql.tidb_ddl_reorg`；持久化 checkpoint 由调用层负责。`finish_ts` 被数据结构保留，但本文件不消费它。

警告由两个 `BTreeMap<String, ...>` 共同表达：一个保存每个错误码首次遇到的文本，另一个累计次数。使用 `BTreeMap` 使迭代顺序稳定，但本文件没有跨线程共享或落盘行为。

`DoneTaskKeeper` 的关键不变量是：`next_key` 只代表从任务 0 开始连续完成的最大前缀。任务 3 即使先完成，也必须等任务 0、1、2 全部完成才能推动 checkpoint；该约束避免重启后跳过尚未完成的键区间。它没有检测重复 ID、过期 ID 或相互冲突的 next key，调用方必须保证 ID 唯一且结果可信。

## 依赖与调用关系

直接外部依赖很窄：`astersql-kv` 提供 `Key`/`KeyRange`，`astersql-util-dbterror` 提供共享错误 `ErrInvalidSplitRegionRanges`；两者都在 `pkg/ddl/Cargo.toml` 的普通依赖中。标准库只使用 `BTreeMap` 和格式化接口。

上游生产调用证据：

- `pkg/ddl/job_worker.rs::run_transactional_index_backfill` 使用 `BackfillResult`、`IndexBackfillBatch`、`ReorgBackfillTask` 和 `merge_warnings_and_counts`。
- `pkg/ddl/persistent_modify_column.rs` 为 changing indexes 构造 `IndexBackfillBatch`，在事务、ingest 或 merge adapter 间分派。
- `pkg/ddl/partition.rs` 为重组分区期间未触及分区的全局索引构造 `IndexBackfillBatch`。
- `pkg/ddl/job_worker.rs` 中的 `DurableJobSession` 相关接口以 `IndexBackfillBatch` 为 session adapter 边界。

RustCodeGraph 报告本文件被 31 个文件使用，但精确 `callers/callees` 查询在本次分析中未返回；因此调用结论以图的 used-by 集合与上述直接引用搜索交叉核对。`handle_backfill_task`、`validate_and_fill_ranges`、`split_ranges_by_keys`、`get_batch_tasks`、`iterate_snapshot_keys`、`DoneTaskKeeper` 和 `LocalRowCountCollector` 均不应仅因是 `pub` 就推断为生产接线。

## 错误处理与边界

`handle_backfill_task` 不返回 `Result`，而把失败保存在 `BackfillResult.error: Option<String>` 中；失败前已累计的计数、警告和 `next_key` 会保留。它只在 `done == false` 时检查严格进展，因此 `done == true` 的上下文可以携带未前移的 `next_key`，调用方需按具体协议解释。该函数也没有验证 `next_key <= end_key`；真实生产事务路径在 `job_worker.rs` 额外拒绝超出 reorg 终点的进度。

`validate_and_fill_ranges` 使用十六进制键构造诊断信息。空 Region 列表、首范围起点晚于请求起点、任何裁剪后空边界、相邻范围不连续都会生成 `ErrInvalidSplitRegionRanges`。与 Go 一致，最后一个范围若短于请求终点并不报错，因为上游 Region 扫描可能主动设置了 limit；只有无界或越过终点时才向内裁剪。

`split_ranges_by_keys` 假定 ranges 与 split keys 都已排序，不在函数内验证排序或范围之间的连续性。`iterate_snapshot_keys` 同样假定输入 rows 的顺序代表扫描顺序；它传播回调错误，并用 `Ok(false)` 提前结束，但不配置真实 snapshot 的 priority、request source、resource group，也不解码 row handle。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务或存储快照，所有类型均为调用方持有的普通值。`Backfiller` 接收 `&mut self`，`handle_backfill_task` 因而是单 worker 内同步执行；多 worker 的调度与伸缩不在此文件。

并发正确性主要体现在数据约束而非同步原语：`DoneTaskKeeper` 将乱序结果转换为连续 checkpoint，但自身不是线程安全容器，必须由单一协调者串行更新或由外部锁保护。`LocalRowCountCollector` 也是非原子计数器。`IndexBackfillBatch: Clone` 允许事务重试时重建同一请求，但真正的 begin/commit/rollback、lease epoch 检查与 checkpoint 发布位于 `job_worker.rs`。

本文件不会获取或释放真实 KV snapshot。`iterate_snapshot_keys` 只遍历借用的内存切片，借用期结束即释放访问；与 Go 版创建 snapshot iterator 并 `defer Close` 的资源生命周期不同。

## 与 Go 版本的对应关系

`BackfillerType`、`BackfillTaskContext`、`BackfillResult`、`ReorgBackfillTask`、范围辅助函数与 `DoneTaskKeeper` 均能在 `pkg/ddl/backfilling.go` 找到同名或同职责原型。Rust 的 `validate_and_fill_ranges` 和 `split_ranges_by_keys` 与 Go 算法及 `pkg/ddl/backfilling_test.go` 的边界用例基本一致；Rust 独立测试也覆盖同样的裁剪、无界键、缺口和边界 split key 场景。

差异必须保留：Go `backfilling.go` 同时包含真实 `backfillWorker` channel、worker master/伸缩、物理表对象、事务和 snapshot 访问。Rust 本文件没有这些设施。Rust `get_batch_tasks` 是简化映射，缺少 Go `getBatchTasks` 调用 `getActualEndKey` 的限幅策略，也固定优先级为 0。Rust `iterate_snapshot_keys` 接收内存 rows，Go `iterateSnapshotKeys` 则从指定版本存储 snapshot 迭代、设置请求来源/资源组/优先级、解码 row handle 并关闭 iterator。Rust 的 warning key/text 被简化为字符串，Go 使用结构化 `ErrorID` 与 `terror.Error`。Rust `BackfillResult` 保存 warning 和字符串错误，而 Go 同名结构主要保存计数、next key 与 `error`。

`IndexBackfillBatch` 是 Rust session adapter 的边界结构，在 Go 文件中没有同名类型；它是 Rust 为真实事务接线新增的聚合 DTO，不应声称逐字段复刻 Go。总体状态应描述为“若干核心算法与生产请求边界已移植，完整 Go worker 子系统不在本文件中”。

## 扩展指南

新增回填类型时，应同步修改 `BackfillerType` 的成员、`COUNT`、`as_str`，并确认上层 action/reorg mode 的选择逻辑与独立测试；不要只扩枚举而遗漏监控名称。改变批次协议时，应同时审查 `BackfillTaskContext`、`BackfillResult`、`IndexBackfillBatch` 以及 `job_worker.rs::run_transactional_index_backfill` 的进度上界、事务重试、lease 和 checkpoint 顺序。

扩展 Region 范围逻辑时，优先修改 `validate_and_fill_ranges`/`split_ranges_by_keys`，并在独立的 `pkg/ddl/backfilling_test.rs` 增加回归用例；测试不要嵌入生产源文件。若要让 `get_batch_tasks` 达到 Go 语义，必须先补齐 `getActualEndKey` 所需的表/reorg/worker 类型上下文，不能静默沿用固定优先级和原始 end key。

若将 `iterate_snapshot_keys` 接入真实存储，需保留 Go 版的 snapshot version、priority、内部请求来源、resource group、row-handle 解码、iterator close 和错误传播语义；不应把当前内存辅助函数直接描述为完整替代。若让 `DoneTaskKeeper` 跨线程共享，应在外层建立单协调者或明确同步方案，并补充重复/陈旧 task ID 的策略。

兼容风险集中在 checkpoint 单调性、警告聚合、键边界和 Go job 元数据语义；性能风险集中在范围粒度、批大小、无谓 clone 以及把内存扫描误用于真实大表。任何生产接线修改都应同步 Rust 独立测试，并与 `pkg/ddl/backfilling_test.go` 的既有意图核对。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`node --file pkg/ddl/backfilling.rs` 读取了完整 481 行并报告 31 个 used-by 文件；`query` 精确定位 `handle_backfill_task`、`validate_and_fill_ranges` 和 `DoneTaskKeeper`。`callers/callees` 两次查询未在 30 秒内返回，故没有把缺失图边当成“无调用者”。
- 源码与包边界：`pkg/ddl/backfilling.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`、`pkg/ddl/doc.rs`，以及包契约 `pkg/ddl/doc.go`。
- 生产接线：`pkg/ddl/job_worker.rs::run_transactional_index_backfill`、`pkg/ddl/persistent_modify_column.rs`、`pkg/ddl/partition.rs`。
- Rust 独立测试：`pkg/ddl/backfilling_test.rs`，直接验证 warning 首值/计数合并、乱序任务连续推进、Region 范围校验和 split key 边界。该文件中的其他测试涉及相邻模块，不能作为本文件全部符号已接线的证据。
- Go 对照：`pkg/ddl/backfilling.go` 的 `backfillerType`、`backfillTaskContext`、`backfillResult`、`reorgBackfillTask`、`splitRangesByKeys`、`validateAndFillRanges`、`getBatchTasks`、`iterateSnapshotKeys`、`doneTaskKeeper`；`pkg/ddl/backfilling_test.go` 的范围与切分用例。
- DDL 架构约束：`docs/agents/ddl/README.md`、`docs/agents/ddl/03-reorg-backfill.md`。这些文档只作为导航，结论已用上述代码和测试核实。
- 本任务为纯文档分析，按任务要求未运行 Cargo。结构验证应确认本文恰有十一个规定的二级标题。
