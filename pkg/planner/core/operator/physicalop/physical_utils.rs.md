# `pkg/planner/core/operator/physicalop/physical_utils.rs`

## 文件定位

本文件属于 Cargo 包 `astersql-planner-core-operator-physicalop`，由同目录 `lib.rs` 以 `mod physical_utils` 纳入并通过 `pub use physical_utils::*` 对外重导出。它不是单一算子的实现，而是物理计划层的横向辅助模块：围绕 `base::PhysicalPlan` 提供计划树遍历、克隆、存储引擎判定、统计信息定位、动态分区访问描述、唯一索引键编码、虚拟列展开和子节点期望行数估算。

该 crate 的 `Cargo.toml` 将 `base`、`expression`、`model`、`property`、`types`、`kv`、`vardef` 等声明为直接依赖，正好覆盖本文件使用的计划接口、表达式、元数据、物理属性、Datum、存储类型与优化器变量。全仓调用搜索显示，它位于规划后的多个消费点：`pkg/planner/optimize.rs` 用其检查混合 TiKV/TiFlash 计划，`pkg/planner/core/plan_cost_ver1.rs` 与 `plan_cost_ver2.rs` 用其查找表统计，`pkg/session/runtime/relational_scan.rs` 用其把下推树线性化后构造扫描请求。

## 核心职责

- 检查物理计划树使用的存储引擎，以及索引连接内侧是否是单次扫描，从而为优化器的跨引擎安全检查提供事实（`StorageEngineUsage`、`HasSingleScanIndexJoin`）。
- 克隆一组物理计划，并提供 Plan Cache 专用的可缓存性检查（`ClonePhysicalPlan`、`ClonePhysicalPlansForPlanCache`、`CloneConstant2DForPlanCache`）。
- 将下推计划按协议需要转为叶到根的一元列表，或转为后序列表并记录非相邻父子边（`FlattenListPushDownPlan`、`FlattenTreePushDownPlan`）。
- 从计划子树取得原始表直方图，解析虚拟列表达式的列下标，并补入虚拟列依赖的基列（`GetTblStats`、`ResolveIndicesForVirtualColumn`、`ExpandVirtualColumn`）。
- 通过小型适配 trait 隔离尚未固化的 infoschema/分区剪枝与 session/table 编码依赖（`DynamicPartitionResolver`、`UniqueIndexValueEncoder`）。
- 按父节点 `ExpectedCnt`、子节点基数和有序索引选择率计算下推期望行数（`CalcChildExpectedCnt`）。

## 主要符号

- `StorageEngineUsage(&dyn PhysicalPlan) -> (bool, bool)`：返回 `(uses_tikv, uses_tiflash)`。Reader 节点是边界；普通节点递归汇总孩子，`PhysicalCTE` 还显式遍历 seed/recur 计划。
- `HasSingleScanIndexJoin(&dyn PhysicalPlan) -> bool`：递归寻找索引连接；只检查 `InnerChildIdx` 指向的内侧，允许中间有一元算子。`PhysicalIndexReader` 或 TiKV `PhysicalTableReader` 是单扫，IndexLookup/IndexMerge 是多扫。
- `ClonePhysicalPlan`：逐项调用 `clone_physical`，首个错误通过 `Result` 返回。
- `FlattenListPushDownPlan`：先前序遍历，再整体反转；设计前提是一元下推链，结果为叶到根。
- `FlattenTreePushDownPlan`：后序遍历；返回节点数组和 `HashMap<child_index, parent_index>`。只有多孩子节点中“不紧邻父节点”的直接孩子才进入映射。
- `GetTblStats`：只认 `PhysicalTableScan.TblColHists` 与 `PhysicalIndexScan.TblColHists`，其他节点沿第一个孩子继续查找。
- `DynamicPartitionAccessObject`：Explain 所需的数据库名、表名/别名、分区名、全分区标志与字符串错误。
- `PartitionPruningResult`、`DynamicPartitionResolver`、`GetDynamicAccessPartition`：把动态剪枝开关、库名查询和剪枝算法抽象到 resolver，再把结果映射为访问对象。
- `ResolveIndicesForVirtualColumn`：仅处理 `VirtualExpr` 非空的列，以给定 `Schema` 替换为已解析下标的新表达式。
- `ClonePhysicalPlansForPlanCache`：先用 `clone_for_plan_cache` 判断每个计划是否可缓存，再执行 `clone_physical`；任一步失败即返回 `(None, false)`。
- `UniqueIndexValueEncoder`、`EncodeUniqueIndexValuesForKey`、`EncodeUniqueIndexKey`：按索引列顺序查表列、归一化 Datum、编码值，并加上表/索引 ID 前缀。
- `CloneConstant2DForPlanCache`：逐行克隆二维常量数组。
- `ExpandVirtualColumn`：追加虚拟表达式依赖但 Schema 中尚无的基列，同时保持尾部 ExtraHandle/ExtraPhysTblID 的位置。
- `CalcChildExpectedCnt`：在父期望行数收紧或有序扫描需要额外探测时返回有限值，否则返回 `f64::MAX`。

## 执行流程

存储检查从规划结果根节点进入。`StorageEngineUsage` 遇到 TableReader 后按 `StoreType`立即返回，遇到 IndexReader、IndexLookupReader、IndexMergeReader、PointGet 或 BatchPointGet 则判定为 TiKV；其余节点累计 CTE seed/recur 和普通 children 的结果。`pkg/planner/optimize.rs` 随后只在 TiKV/TiFlash 同时出现且不存在单扫索引连接时触发对应约束；`HasSingleScanIndexJoin` 因此沿树寻找索引连接，只沿其内侧的一元链判断最终 Reader 类型。

下推计划编码有两条路径。一元路径由 `flatten_preorder` 收集根到叶，再反转为叶到根，`pkg/session/runtime/relational_scan.rs` 和 `physical_indexmerge_reader.rs` 依次消费该顺序。树形路径由 `flatten_postorder` 递归完成所有孩子后加入父节点；若父节点有多个孩子，它记录那些在数组中没有紧贴父节点的孩子下标，以便消费者恢复树边而不把扁平数组误当作单链。

动态分区流程先要求表确有 `Partition` 且 resolver 报告启用动态剪枝。随后选择 alias 或真实表名，取得数据库名，再调用 `partition_indices`：`FullRange` 设置 `AllPartitions`；下标列表逐一映射到分区定义名；剪枝错误或越界下标写入 `Err`。返回 `None` 仅表示不适用/未启用，不代表剪枝失败。

唯一索引编码先检查值数与索引列数一致，再把每个 `IndexColumn.Offset` 转成 `usize` 并从 `TableInfo.Columns` 找到列定义，交给 encoder 做类型归一化，最后统一编码。完整键在编码值前写入 `t`、可比较序 table ID、`_i` 和可比较序 index ID；`encode_comparable_i64` 通过翻转符号位并使用大端字节保持有符号整数排序。

虚拟列展开先统计 Schema 尾部连续的 ExtraHandle/ExtraPhysTblID。若普通列之后存在这些额外列，先同时从 Schema 和 `ColumnInfo` 副本中摘除，追加虚拟表达式依赖，再原序接回；内部 `expand_virtual_column` 去除 Schema 已包含或本轮已收集的重复依赖，并只追加能在 `all_columns` 中找到元数据的列。

## 数据与状态

大多数函数是无共享状态的纯遍历或转换。计划树通过借用的 `&dyn PhysicalPlan` 读取，扁平结果保存原节点引用而非克隆节点；调用者必须保证原树在结果使用期间存活。`ClonePhysicalPlan` 和 Plan Cache 克隆则创建新的 boxed trait objects，并把同一个 `ContextRef` 的克隆传给各节点。

`DynamicPartitionAccessObject.Err` 是面向访问描述的内嵌错误状态，和函数级 `Result` 不同；分区对象即使剪枝失败也会返回，供 Explain/上层展示原因。`PartitionPruningResult::FullRange` 与空的 `Partitions` 语义不同，前者显式表示所有分区。

`ExpandVirtualColumn` 会原地改变 `schema.Columns`，同时返回与之对应的新 `Vec<ColumnInfo>`；两者的追加与 Extra 列重排必须保持同步。`CalcChildExpectedCnt` 读取 SessionVars；在属性包含排序项时还调用 `RecordRelevantOptVar(TiDBOptOrderingIdxSelRatio)`，因此除了数值返回外还会记录优化结果依赖的会话变量。

## 依赖与调用关系

上游已确认的 Rust 生产调用包括：

- `pkg/planner/optimize.rs` → `StorageEngineUsage`、`HasSingleScanIndexJoin`。
- `pkg/session/runtime/relational_scan.rs`、`physical_indexmerge_reader.rs` → `FlattenListPushDownPlan`。
- `pkg/planner/core/plan_cost_ver1.rs`、`plan_cost_ver2.rs` → `GetTblStats`。
- `physical_batch_point_get.rs` → `ResolveIndicesForVirtualColumn`。
- `base_physical_plan.rs` 的 Apply/Join 期望行数计算 → `CalcChildExpectedCnt`。

`lib.rs` 的重导出是这些跨 crate 调用的公开入口。下游共同依赖是 `base::PhysicalPlan::children`、运行时 downcast、克隆接口和 `PlanContext`；具体数据依赖来自 `expression::{Column, Constant, Schema}`、`model::{ColumnInfo, IndexInfo, TableInfo}`、`property::{HistCollRef, PhysicalProperty}`、`types::datum::Datum` 与 `kv::StoreType`。

全仓 Rust 搜索未发现 `ClonePhysicalPlan`、`GetDynamicAccessPartition`、唯一索引编码函数、`ExpandVirtualColumn` 的直接生产调用。Plan Cache 生成器会生成同名 Go 风格调用文本，`plan_clone_generated.rs` 与 `task_base.rs` 中也有注释化引用；这些只能证明迁移意图，不能证明 Rust 当前已接线。`DynamicPartitionAccessObject` 另在 `pkg/planner/core/access/access_obj.rs` 有不同类型，当前没有证据表明两者已桥接。

## 错误处理与边界

- 所有递归遍历都假定计划结构无环；文件没有环检测。深树使用递归，极端深度存在栈压力。
- `FlattenListPushDownPlan` 不验证“一元链”前提；若传入分支树，它仍会反转完整前序结果，但该顺序不再表示简单叶到根链。分支计划应使用树形版本。
- `GetTblStats(None)`、无孩子且非 Scan 的节点，以及 Scan 未携带直方图时均返回 `None`；它只沿第一个孩子，不搜索兄弟。
- `GetDynamicAccessPartition` 对无分区表或未启用动态剪枝返回 `None`；数据库名缺失降级为空串。剪枝错误和非法分区下标写入 `Err`，非法下标前已经追加的分区名会保留。
- `ResolveIndicesForVirtualColumn` 和批量克隆均为短路语义：首个错误终止，之前已经修改/克隆的项不会回滚，但局部克隆会随返回释放。
- 唯一索引编码显式拒绝值数不等、负 offset 和越界 offset，并传播 encoder 的归一化/编码错误。与 Go 版本不同，Rust 不在本函数中直接把特定转换错误映射为 `kv.ErrNotExist`，而是把策略交给 encoder。
- `ExpandVirtualColumn` 对 `all_columns` 中找不到的依赖静默跳过；调用方若要求完备 Schema，应在上游保证元数据齐全。
- `CalcChildExpectedCnt` 仅在会进入公式的有序分支检查 `estimated_row_count > 0`；若第一分支因 `ExpectedCnt < estimated_row_count` 命中，公式会除以 `estimated_row_count`。正常统计应为正值，新增调用点仍需守住该前提。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。计划引用和 Schema 可变借用均由 Rust 生命周期/借用规则限定；返回的计划引用不能脱离输入树。克隆函数在错误时由 RAII 释放已经构造的局部对象，不泄漏部分结果。

并发安全性主要由传入 trait object 和 `ContextRef` 的实现决定，本文件不额外同步。`CalcChildExpectedCnt` 对 SessionVars 的相关变量记录可能使用内部可变性，因此并发调用能否安全共享同一 context 应服从 `PlanContext`/SessionVars 自身契约，不能从本文件单独推导。动态分区 resolver 与唯一索引 encoder 同理：调用是同步借用，资源所有权仍归调用者。

## 与 Go 版本的对应关系

同目录 `physical_utils.go` 是直接语义基线。计划克隆、两种 flatten、首孩子统计查找、虚拟列下标解析、Plan Cache 克隆、常量二维克隆、虚拟列展开和 `CalcChildExpectedCnt` 均保留了 Go 的主要流程。独立的 `physical_utils_test.rs` 也用与 `physical_utils_test.go` 相同的单链和分支树形状验证 flatten 顺序；Rust 用已接入 `PhysicalPlan` 的 `PhysicalIndexLookUpReader` 替代 Go fixture 的 `PhysicalLocalIndexLookUp`，测试关注点仍是二元节点遍历。

差异需要明确：

- Go 仓库把存储引擎和单扫索引连接辅助实现拆到了 `storage_engine_usage.go`、`single_scan_index_join.go`，Rust 将其集中在本文件；Rust 测试覆盖 TiKV/TiFlash 汇总和单扫/双扫基本分支，Go 测试覆盖的 TiDB reader、包装一元节点、IndexMerge、Hash/Merge 变体更广。
- Go 的动态分区函数直接依赖 InfoSchema、PartitionedTable 与 `partitionpruning.PartitionPruning`；Rust 通过 `DynamicPartitionResolver` 抽象这些边界，并增加非法下标错误字符串，当前未发现 resolver 实现或生产调用。
- Go 的唯一索引编码直接执行字符串/枚举/`table.CastValue` 特例并通过 `codec.EncodeKey`、`tablecodec.EncodeIndexSeekKey` 编码；Rust 把归一化和 Datum 编码委托给 `UniqueIndexValueEncoder`，只在本地构造 table/index 前缀。两者的字节兼容性需要具体 encoder 的独立测试才能证明，当前文件及测试没有该证据。
- Go 的常量克隆调用 Plan Cache 专用克隆辅助；Rust 直接 `Clone` 每行。是否等价取决于 `Constant::clone` 的语义，当前未见针对本函数的测试。
- Rust 的虚拟列展开在本轮 dependent 中按 `UniqueID` 去重，并在元数据缺失时跳过；Go 只用 Schema 包含检查并直接追加 `FindColumnInfoByID` 结果。

## 扩展指南

新增 Reader 或存储引擎时，应同步检查 `StorageEngineUsage` 的边界分类和 `is_single_scan_read` 的单扫含义，并在 `physical_utils_test.rs` 增加对应节点、包装层和 CTE 分支用例。新增复合索引连接变体时，还要确认 `crate::index_join_base` 能识别它，而不是只修改本文件的递归逻辑。

修改下推树序列协议时，应同时评估 `pkg/session/runtime/relational_scan.rs`、`physical_indexmerge_reader.rs` 以及生成器产生的 Plan Cache 克隆代码。列表版必须继续限定一元链；树形版若改变遍历顺序或映射含义，需要同步 Rust/Go 的 flatten fixture，测试必须保留在独立 `physical_utils_test.rs`，不要嵌回源文件。

接通动态分区或唯一索引编码前，应先提供真实的 resolver/encoder 适配实现，并用独立测试覆盖全分区、空结果、剪枝失败、非法下标、字符串/枚举/溢出以及与 Go `tablecodec` 的字节级一致性。当前 trait 是边界，不应把缺失的生产实现误认为已完成接线。

扩展虚拟列逻辑时需保持 `schema.Columns` 和返回 `ColumnInfo` 一一对应，特别验证只有 Extra 列、普通列后带多个 Extra 列、重复依赖和元数据缺失。调整期望行数公式时，应同步 `base_physical_plan.rs` 的 Apply/Join 调用语义，并覆盖无排序、零/负统计、ratio 为零及父期望不收紧等边界。

## 验证依据

- 源码全量阅读：`pkg/planner/core/operator/physicalop/physical_utils.rs`（449 行）；模块入口：同目录 `lib.rs` 的 `mod physical_utils`、`pub use physical_utils::*`；crate 边界：同目录 `Cargo.toml`。
- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点与 1,848,419 条边；`files --filter` 确认目标文件已索引且含 38 个符号；`query` 精确定位了 `StorageEngineUsage`、`HasSingleScanIndexJoin`、`ClonePhysicalPlan`、两种 flatten 的 Rust/Go 定义。`callers`/`callees` 在本次命令时限内未返回，因此调用点另以全仓 `rg` 补证，未把图查询超时解释为“无调用”。
- 生产调用证据：`pkg/planner/optimize.rs`、`pkg/session/runtime/relational_scan.rs`、`pkg/planner/core/plan_cost_ver1.rs`、`plan_cost_ver2.rs`、`physical_batch_point_get.rs`、`physical_indexmerge_reader.rs`、`base_physical_plan.rs`。
- Go 对照：`pkg/planner/core/operator/physicalop/physical_utils.go`、`storage_engine_usage.go`、`single_scan_index_join.go`；独立测试：`physical_utils_test.rs` 与 `physical_utils_test.go`。
- 本任务是纯文档分析，按计划未运行 Cargo。结构验收以目标文件存在且固定的十一个二级标题各出现一次为准；事实复核同时区分了已接入调用、生成器字符串、注释化迁移代码和未发现调用的 API。
