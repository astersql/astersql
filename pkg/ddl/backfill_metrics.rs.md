# `pkg/ddl/backfill_metrics.rs`

## 文件定位

该文件属于 `astersql-ddl` crate：`pkg/ddl/Cargo.toml` 以 `lib.rs` 为库入口，`pkg/ddl/lib.rs` 通过 `pub mod backfill_metrics;` 暴露本模块，并在 `cfg(test)` 下把独立测试文件 `backfill_metrics_test.rs` 挂入 crate。它位于 DDL 回填观测层，描述回填动作的指标标签、指标应归属的表 ID，以及一个确定性遍历的进程内指标容器；它不驱动 DDL job、schema state、checkpoint 或回填扫描。

当前接线状态需要特别区分：仓库内 Rust 生产文件没有引用本模块的公开符号，直接使用者只有 `pkg/ddl/backfill_metrics_test.rs`。因此，这里已经移植了可测试的标签与表 ID 选择语义，但尚不能据此认定 Rust DDL 主链已实际发布 Prometheus 指标。Go 生产链的对应调用位于 `pkg/ddl/reorg.go:updateBackfillProgress` 和 `pkg/ddl/backfilling.go:newBackfillCtx`。

## 核心职责

1. 六个 `LABEL_*` 常量固定与 Go 指标系统兼容的标签文本，避免调用方各自拼写。
2. `BackfillAction` 把与指标路由有关的 DDL action 收敛为九个变体；`backfill_progress_label` 决定哪些 action 有进度序列，以及加索引是否处于临时索引合并阶段。
3. `is_partition_reorganization`、`is_partition_drop_or_truncate` 和 `is_partition_reorganization_label` 表达表 ID 路由所需的分类规则。
4. `backfill_metrics_table_id` 在逻辑表 ID 与物理分区 ID 之间选择，使分区重组和分区清理指标能以 DDL 完成后仍可访问的 ID 清理。
5. `BackfillMetrics` 以 `MetricKey` 为键，提供累计量、百分比和按表清理的纯内存模型，供独立 Rust 测试验证基本生命周期。

本文件不负责 Go 版本中的 Prometheus collector 获取、注册或全局清理；Rust 的 `BackfillMetrics` 也没有与 `astersql-metrics` 建立类型或调用关系。

## 主要符号

- `LABEL_ADD_INDEX`、`LABEL_ADD_INDEX_MERGE`、`LABEL_MODIFY_COLUMN`、`LABEL_REORG_PARTITION`、`LABEL_REORG_PARTITION_RATE`、`LABEL_CLEANUP_INDEX_RATE`：分别对应普通加索引、临时索引合并、改列、分区重组进度、分区重组速率前缀和索引清理速率。值由 `backfill_metrics_test.rs:backfill_progress_labels_match_go_actions` 锁定。
- `pub enum BackfillAction`：指标决策使用的动作枚举。`Other` 是显式兜底；删除/截断分区参与清理指标路由，但不会从 `backfill_progress_label` 获得进度标签。
- `pub fn backfill_progress_label(action, merging_temporary_index) -> &'static str`：加索引/主键根据阶段返回两个不同标签；改列和三类分区重组返回固定标签；其他动作返回空串。返回空串是“调用方不应上报进度”的协议，而不是错误。
- `pub fn is_partition_reorganization(BackfillAction) -> bool`：只识别 `ReorganizePartition`、`AlterTablePartitioning` 和 `RemovePartitioning`。
- `pub fn is_partition_drop_or_truncate(BackfillAction) -> bool`：只识别 `DropTablePartition` 和 `TruncateTablePartition`。
- `pub fn is_partition_reorganization_label(&str) -> bool`：接受精确的 `reorg_partition`，也接受以 `reorg_partition_rate` 开头的派生序列，例如测试中的 `reorg_partition_rate-conflict`。
- `pub struct ReorganizationMetricInfo`：携带 action、可选逻辑表 ID 和必有物理表 ID。`Option<i64>` 对应 Go 中 `reorgInfo.Job` 是否存在这一决策条件，而不是完整复制 Go job。
- `pub fn backfill_metrics_table_id(Option<&ReorganizationMetricInfo>, &str) -> i64`：指标归属 ID 的集中路由函数；无上下文用 `0` 表示缺失。
- `pub struct MetricKey`：由 `table_id`、标签、schema、table 和对象名组成完整序列键；派生 `Ord`/`PartialOrd` 以支持 `BTreeMap`。
- `pub struct BackfillMetrics`：内部持有 `totals` 与 `progress` 两张私有 `BTreeMap`。`add_total` 累加，`set_progress` 截断常规数值到 `[0, 100]`，`total`/`progress` 缺键返回 `0.0`，`clear_table` 同时删除指定表的两类序列。

## 执行流程

进度标签路径如下：调用方先把 DDL 动作映射为 `BackfillAction`，再调用 `backfill_progress_label`。加索引和加主键继续检查 `merging_temporary_index`；改列及三种分区重组直接返回固定标签；其他动作得到空串并应停止进度上报。Go 的实际流程可在 `updateBackfillProgress` 看到：计算并防止进度回退后取标签，空标签立即返回，然后选择表 ID 并把百分比写入 gauge。

表 ID 路由先处理缺失信息：`None` 返回 `0`。对于非分区重组动作，默认返回 `physical_table_id`；唯一例外是删除/截断分区且标签恰为 `cleanup_idx_rate`，此时优先返回逻辑表 ID，因为旧物理分区会从分区定义中移除。对于分区重组动作，进度标签或 `reorg_partition_rate*` 标签优先使用逻辑表 ID，其他标签仍使用物理 ID。两处“优先逻辑 ID”在缺失时都会退回物理 ID。

内存容器路径彼此独立：`add_total` 通过 map entry 从零累计；`set_progress` 覆盖同键旧值；读取器不创建条目；`clear_table` 分别扫描两张 map 并保留其他 `table_id`。`backfill_metrics_test.rs:production_metrics_store_accumulates_and_cleans_by_table_id` 验证两次累计、覆盖进度、缺键读零、幂等清理和其他表隔离。

## 数据与状态

模块没有全局可变状态。标签是静态字符串；动作和路由信息是按值传递的小型数据；`MetricKey` 拥有四个 `String`，所以 map 不借用调用方缓冲区。`BackfillMetrics` 的两张 map 是唯一可变状态，并保持同一种键模型但不强制两者同时存在。

`BTreeMap` 让键遍历顺序由 `MetricKey` 的派生字典序确定，便于确定性检查；本文件当前没有公开迭代接口，因此这一性质主要影响调试、未来导出或派生比较。累计量没有单调性校验：负数、无穷或 `NaN` 都可传给 `add_total`。`set_progress` 对普通有限值执行 `[0, 100]` 截断，但没有显式拒绝 `NaN`。调用方若需要 Prometheus counter 的非负约束或严格数值验证，必须在接线层补充并测试。

`0` 是 `backfill_metrics_table_id(None, ...)` 的缺失哨兵；函数不返回 `Option` 或 `Result`。逻辑表 ID 缺失不是错误，会退回物理表 ID。这两条是兼容现有 Go 分支行为的重要边界。

## 依赖与调用关系

直接 Rust 依赖只有标准库 `std::collections::BTreeMap`，目标文件本身不消费 `pkg/ddl/Cargo.toml` 中的外部 crate。crate 根通过 `pkg/ddl/lib.rs:pub mod backfill_metrics` 导出该模块，独立测试通过 `crate::backfill_metrics::{...}` 调用全部核心路径。

RustCodeGraph 对目标文件给出的使用集合包含 `pkg/ddl/backfill_metrics_test.rs` 等文件，但精确符号搜索和仓库引用复核表明，本模块符号目前只在该 Rust 测试中出现；图的宽泛 file-level “used by” 不能解释为所有列出文件都调用这些 Rust API。精确 `callers backfill_metrics_table_id` 查询在本次会话中长时间无结果而被中止，因此调用结论以精确符号查询加 `rg` 引用复核为准。

Go 对照的真实生产边为：`reorg.go:updateBackfillProgress` → `backfillProgressLabel` → `backfillMetricsTableID` → `getBackfillProgressByTableID(...).Set(progress*100)`；`backfilling.go:newBackfillCtx` → `backfillMetricsTableID` → 两次 `getBackfillTotalByTableID`，分别创建普通与 `-conflict` counter。Go 包装函数再下沉到 `pkg/metrics` 的全局 Prometheus 实现。

## 错误处理与边界

所有公开函数都是无失败返回的纯同步 API。未知动作通过空标签降级；缺失重组信息通过表 ID `0` 降级；缺失逻辑 ID回退到物理 ID；缺失 map 键读取为 `0.0`；重复清理无副作用。调用方必须理解这些哨兵，否则可能把无效 ID 或空标签注册成真实指标。

标签前缀匹配是有意行为，不是精确枚举：任何以 `reorg_partition_rate` 开头的字符串都会路由到逻辑表，包括 `-conflict` 派生标签。相反，`cleanup_idx_rate` 必须精确相等才触发删除/截断分区特例。

本模块不验证 schema/table/object 名，也不限制累计值；`MetricKey` 的所有字符串都参与键身份，任一名字差异都会产生新序列。`clear_table` 只按 `table_id` 清理，因此同一 ID 的所有标签和名字都会一起删除。

## 并发与资源生命周期

`BackfillMetrics` 没有锁、原子变量、任务或通道；变更方法要求 `&mut self`，Rust 借用规则阻止同一实例未经同步的并发写。若未来成为共享生产指标存储，调用方需要在外层选择 `Mutex`、分片容器或单线程所有权，并定义锁粒度；本文件当前没有作出这一架构承诺。

资源完全驻留内存，随 `BackfillMetrics` 所有者析构释放。单条指标由首次 `add_total` 或 `set_progress` 创建，后续累计或覆盖，DDL 完成/回滚时应由接线层调用 `clear_table`。若漏清理，字符串键和数值会持续占用内存；如果使用了错误的物理分区 ID，分区元数据删除后也可能失去可达的清理键，这正是逻辑表 ID 路由特例要避免的问题。

## 与 Go 版本的对应关系

`backfill_progress_label` 逐分支对应 `pkg/ddl/backfill_metrics.go:backfillProgressLabel`；三个分类函数对应 `isPartitionReorgDDL`、`isPartitionDropOrTruncateDDL` 和 `isPartitionReorgBackfillMetricLabel`；`backfill_metrics_table_id` 对应 `backfillMetricsTableID`。Rust 用 `BackfillAction` 代替 `model.ActionType`，用 `logical_table_id: Option<i64>` 压缩表达 Go 的 `rInfo.Job != nil` 与 `Job.TableID`，测试覆盖了 job/逻辑 ID 存在和缺失的分支。

关键差异是 Go 文件还提供 `getBackfillTotalByTableID` 与 `getBackfillProgressByTableID`，直接返回 `pkg/metrics` 的 Prometheus counter/gauge，并由生产回填代码调用；Rust 文件没有这两个包装器，也没有生产调用边。Rust 新增的 `MetricKey`/`BackfillMetrics` 是本地内存抽象，并非 Go 文件中的类型，也未复刻 Prometheus registry 的 label 追踪与 `DDLClearBackfillMetrics` 全局删除行为。

Go 测试 `pkg/ddl/backfill_metrics_test.go` 验证真实 collector 中序列注册/删除、分区逐个清理、逻辑表路由和派生标签；Rust 测试只验证标签文本、表 ID 决策和内存容器。因此，Rust 当前保持了决策逻辑，但生产可观测性与 Go 尚不等价。

## 扩展指南

新增一种回填 action 时，应先扩展 `BackfillAction`，再明确它是否产生进度标签、是否属于分区重组或删除/截断分类，并同步更新 `backfill_progress_label` 与表 ID 路由。相应测试应继续放在独立文件 `pkg/ddl/backfill_metrics_test.rs`，至少覆盖普通阶段、特殊阶段、逻辑 ID 存在/缺失以及非目标标签，不能把测试嵌入生产源文件。

新增指标标签时，需要确认它是精确标签还是前缀族，并与 Go `pkg/metrics` 的字符串保持一致。改变 `LABEL_REORG_PARTITION_RATE` 或其前缀判定会影响 `-conflict` 等派生序列的清理归属；改变清理标签的精确匹配可能造成残留序列。任何键字段变更都应评估指标 cardinality 和 `clear_table` 是否仍能完整回收。

若要把模块接入 Rust 生产链，最小必要工作不是简单实例化 `BackfillMetrics`：需要在 Rust 的回填进度更新与 worker/context 创建点映射真实 action 和 job/table 信息，并对接 `astersql-metrics` 的 collector/清理 API，保证 DDL 成功、失败、取消与回滚路径都清理相同 ID。还应补独立测试验证 collector 可见性与清理，而不是以当前内存模型测试替代 Go 的生产语义；并发共享方案及 counter 非负约束也必须在该接线设计中确定。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`node --file pkg/ddl/backfill_metrics.rs` 核对目标文件 203 行的全部符号；`query backfill_metrics_table_id --kind function` 精确定位到第 136 行；`explore "pkg/ddl/backfill_metrics.rs metrics backfill"` 找到 Rust 测试符号及 Go 对应符号。精确 callers 查询超过约 90 秒无输出后中止，随后用精确仓库引用搜索补证。
- Rust 源与装配：`pkg/ddl/backfill_metrics.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`；Cargo 证明 crate 名为 `astersql-ddl`、库入口为 `lib.rs`，目标模块没有条件编译，测试模块受 `cfg(test)` 控制。
- Rust 独立测试：`pkg/ddl/backfill_metrics_test.rs` 的 `backfill_progress_labels_match_go_actions`、`production_metrics_store_accumulates_and_cleans_by_table_id`、`metric_table_id_selection_matches_go_job_presence_rules`。
- Go 对照与调用方：`pkg/ddl/backfill_metrics.go`、`pkg/ddl/reorg.go:updateBackfillProgress`、`pkg/ddl/backfilling.go:newBackfillCtx`；Go 独立测试为 `pkg/ddl/backfill_metrics_test.go`。
- DDL 包契约与阅读入口：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`。本文件只涉及回填观测辅助逻辑，不直接更改 job、schema version、schema state、持久化 checkpoint、取消/回滚或 MDL 行为。
- 交付验证按任务要求只执行 Markdown 结构与人工事实检查，不运行 Cargo；目标结构命令应确认文件存在且恰有十一个固定二级标题。
