# `pkg/planner/property/physical_property.rs`

## 文件定位

本文件属于 `astersql-planner-property` crate，定义父物理算子对子计划提出的物理要求，以及比较、变换和编码这些要求所需的辅助类型。crate 根 `pkg/planner/property/lib.rs` 将本模块私有声明为 `physical_property`，再通过 `pub use physical_property::*` 对外导出；因此其他 planner crate 通常以 `property::PhysicalProperty`、`property::SortItem` 等名字使用这里的 API。

它位于逻辑属性和物理计划之间：逻辑算子枚举物理实现时创建或改写属性，物理算子把属性继续传给子节点，Cascades/memo 和物理计划缓存用属性指纹区分候选。RustCodeGraph 的文件关系显示该文件被 21 个 Rust 文件使用；直接搜索可见主要使用点集中在 `pkg/planner/core/operator/logicalop/logical_projection.rs`、`pkg/planner/core/operator/physicalop/base_physical_plan.rs`、`physical_topn.rs`、`physical_window.rs`、`base_physical_agg.rs` 以及 `pkg/planner/cascades/old/implementation_rules.rs`。

## 核心职责

1. 用 `SortItem`、`PartialOrderInfo` 和 `PartialOrderMatchResult` 表达全局排序、分区内排序和前缀索引提供的部分有序性。
2. 用 `MPPPartitionType`、`MPPPartitionColumn`、`NeedMPPExchangeByEquivalence` 和 `NeedEnforceExchanger` 描述并判断 MPP Exchange 的广播、哈希和单分区要求。
3. 用 `PhysicalProperty` 汇总任务类型、期望行数、排序、MPP、CTE、向量 TopK、IndexJoin 运行时信息、禁止下推和 TiFlash 偏好。
4. 生成稳定的属性指纹 `HashCode`，供 memo/实现缓存按物理要求查找候选；例如 `pkg/planner/memo/group.rs` 和 `pkg/planner/cascades/memo/group.rs` 都以该值作为映射键。
5. 提供 Explain 文本、collation ID 转换、内存估算、关键字段克隆和任务类型枚举等配套操作。

本文件不负责成本计算、物理算子实例化或真正执行 Exchange；它只承载要求和匹配规则。真正的属性传播和物理计划构造发生在 `pkg/planner/core/operator/**` 与 Cascades 实现代码中。

## 主要符号

- `wholeTaskTypes`：Root 属性可接受的子任务集合，依次为 `CopSingleReadTaskType`、`CopMultiReadTaskType`、`RootTaskType`；不包含 MPP。
- `SortItem { Col, Desc }`：列级排序要求。`Hash64`/`EqualsSortItem` 同时考虑列与方向，`String` 生成 `{列 asc|desc}`，`Clone` 深拷贝列，`MemoryUsage` 汇总布尔值和列占用。
- `ExplainPartitionBy`：为空时不写内容；否则写入 `partition by` 及逗号分隔列，按 `normalized` 选择规范化或带上下文的 Explain 表达。
- `MPPPartitionType` 与 `AnyType`、`BroadcastType`、`HashType`、`SinglePartitionType`：透明 `i32` 新类型。`ToExchangeType` 将后三者映射到 tipb；`AnyType` 或未知值会记录 warning 并回退 `PassThrough`。
- `MPPPartitionColumn { Col, CollateID }`：MPP 哈希键。`ResolveIndices` 将列绑定到目标 schema；`Equal` 在负 collation ID（新 collation 编码）下要求 ID 一致，再比较列；`hashCode` 采用相同兼容规则编码 collation。
- `ChoosePartitionKeys`：按下标顺序深拷贝分区键子集。调用者必须保证所有下标有效。
- `PhysicalPropMatchResult`：`PropNotMatched`、`PropMatched`、`PropMatchedNeedMergeSort`；`Matched` 将后两者都视作可满足要求。
- `PartialOrderInfo`：记录 TopN 等场景可由前缀索引提供的排序项；`AllSameOrder` 判断方向是否一致。
- `PartialOrderMatchResult`：记录某条访问路径是否匹配、最后一个前缀列与前缀字节长度，供路径级匹配结果保存。
- `IndexJoinRuntimeProp`：携带非等值条件、内外连接键、平均内表行数及偏好 table range scan；`CloneEssentialFields` 当前克隆完整结构。
- `VectorSearchInfo` / `VectorProperty`：保存向量距离函数签名、查询向量、目标列和 TopK。查询向量由 `Arc<VectorFloat32>` 共享。
- `PhysicalProperty`：本文件核心类型。公开字段组合排序、任务类型、预计行数、是否允许 enforcer、MPP 分区、CTE 状态、向量/IndexJoin 信息、`NoCopPushDown`、部分有序、建议排序和 `PreferTiFlash`；`hashcode: OnceLock<Vec<u8>>` 是 crate 内可见缓存。
- `NewPhysicalProperty` / `SortItemsFromCols`：从任务类型和统一排序方向创建常见属性。
- `NeedEnforceExchanger`：按目标分区类型判断已有 MPP 分区能否直接满足要求。

## 执行流程

典型属性传播流程如下：

1. 物理计划枚举代码以 `PhysicalProperty::default`、`NewPhysicalProperty` 或 `CloneEssentialFields` 创建要求。默认值表示 Root 任务、无排序、任意 MPP 分区、不可加 enforcer；`ExpectedCnt` 默认为 `0.0`，具体枚举点通常会显式覆盖。
2. 中间逻辑/物理算子根据自身语义改写要求。`logical_projection.rs` 会把排序、部分有序和哈希分区列穿过投影；`physical_topn.rs` 设置 `PartialOrderInfo`、`AdvisorySortItems` 或向量 `VectorProp`；`physical_window.rs` 设置 `SortItemsForPartition` 和 MPP 分区；`base_physical_agg.rs` 构造聚合子属性并可能设置 `PreferTiFlash`。
3. 候选访问路径使用 `IsPrefix`、`AllColsFromSchema`、`NeedKeepOrder`、`GetSortDescForKeepOrder` 和 `GetSortItemsForKeepOrder` 判断能否保持要求的顺序。部分有序信息非空时，keep-order 辅助方法优先采用它而不是全局 `SortItems`。
4. MPP 路径比较已有分区和要求。`NeedEnforceExchanger` 对 `Any` 总是不强制、对 `Broadcast` 总是强制、对 `SinglePartition` 比较分区类型；哈希场景先要求已有类型为 `HashType`。若提供 FD 且已有哈希列非空，则进入 `NeedMPPExchangeByEquivalence`，否则要求列数、顺序、列身份和 collation 逐项一致。
5. `NeedMPPExchangeByEquivalence` 对每个要求列用 `FDSet::ClosureOfEquivalence` 求等价闭包；每个已有哈希键只要能在任一要求列的闭包中找到且满足 collation 规则即可消除 Exchange。其语义是“已有键集合不得引入要求等价集合以外的键”，不是要求两边等长。
6. memo 或实现缓存调用 `HashCode`。首次调用通过 `buildHashCode` 顺序编码关键字段并放入 `OnceLock`，之后克隆返回缓存字节；`pkg/planner/memo/group.rs`、`pkg/planner/cascades/memo/group.rs`、`pkg/planner/cascades/old/optimize.rs` 都以该指纹查重或查找实现。

## 数据与状态

`PhysicalProperty::default` 初始化为空排序、`RootTaskType`、`ExpectedCnt = 0.0`、`CanAddEnforcer = false`、`AnyType` 分区、无 CTE/向量/IndexJoin/部分有序信息。空排序的 `AllSameOrder` 和 `PartialOrderInfo::AllSameOrder` 都返回 `(true, false)`，把“没有冲突方向”视为同向且默认升序。

`HashCode` 编码 `CanAddEnforcer`、任务类型、期望行数和全局排序；仅 MPP 任务编码分区类型、分区列及向量列/函数签名；随后编码 CTE 状态、可选 IndexJoin 内容、`NoCopPushDown`、`PreferTiFlash`、部分有序存在标记和排序项、建议排序项。向量常量本身和 `VectorProp.TopK` 未进入指纹，这与 Go 中“只接收 DataSource 直接上方 TopN 的向量信息，因此不哈希向量常量”的约束一致，但扩展向量属性时必须重新审视缓存区分度。

`CloneEssentialFields` 会复制排序、任务、预计行数、MPP、CTE、禁止下推、部分有序、建议排序和 `PreferTiFlash`，并通过默认值刻意清空哈希缓存、`CanAddEnforcer`、向量属性和 IndexJoin 属性。需要传递 IndexJoin 的少数算子会另行调用 `IndexJoinRuntimeProp::CloneEssentialFields`，见 `pkg/planner/core/operator/physicalop/base_physical_plan.rs`。

`MemoryUsage` 是估算值：包含结构本体、已初始化哈希缓存容量、排序项/分区列/建议排序和部分有序信息；它没有累计 `VectorProp`、`IndexJoinProp` 内部动态数据。`Vec` 容量也并非所有字段都单独计入，因此不应把结果视为精确堆分配审计。

## 依赖与调用关系

- crate 边界由 `pkg/planner/property/Cargo.toml` 定义：本文件通过 crate 根再导出的 `base`、`codec`、`collate`、`expression`、`funcdep`、`intset`、`size` 使用多个工作区 crate，并直接依赖 `log` 与启用 `protobuf-codec` 的 `tipb` Git revision。
- `expression` 提供 `Column`、`Schema`、表达式哈希、Explain 上下文与向量类型；`funcdep::FDSet` 和 `intset::FastIntSet` 提供等价闭包；`codec` 负责稳定编码；`collate` 负责新旧 collation ID 重写；`tipb` 提供 Exchange 和标量函数枚举。
- 上游构造/变换：`logical_projection.rs` 投影属性列，`physical_topn.rs` 产生部分有序、建议排序和向量属性，`physical_window.rs` 产生分区内排序，`base_physical_agg.rs` 产生聚合子属性，`base_physical_plan.rs` 广泛克隆、放宽和比较属性。
- 下游消费：`pkg/planner/memo/group.rs` 和 `pkg/planner/cascades/memo/group.rs` 用 `HashCode` 索引实现；`base_physical_plan.rs` 根据 MPP 字段构造或复用 Exchange；`cache_snapshot.rs` 捕获并恢复几乎全部属性字段。
- RustCodeGraph 对精确 `callers`/`callees` 查询没有返回函数边，因此调用证据以索引的文件级 `used by` 关系和 `rg` 的精确调用点为准。当前 Rust 生产代码未直接调用本文件的 `NeedEnforceExchanger`；`base_physical_plan.rs` 和 `physicalop/enforce.rs` 含有本地等价判定/接线，而本函数由 `physical_property_test.rs` 直接验证。不能据此宣称生产路径已经统一复用该函数。

## 错误处理与边界

- `MPPPartitionColumn::ResolveIndices` 是本文件主要的显式错误通道：`Column::ResolveIndices` 失败时以 `expression::Error` 原样向上传播，不在本层恢复。
- `ChoosePartitionKeys` 直接用 `keys[index]` 取值，越界会 panic；`matches` 必须来自已验证的匹配结果（例如 `IsSubsetOf`），不能接受未经检查的外部下标。
- `ToExchangeType` 对 `AnyType` 和未知整数不会返回错误，而是 warning 后回退 `PassThrough`。调用者若需要拒绝非法状态，必须在调用前验证分区类型。
- `IsSubsetOf` 允许同一 `keys` 位置匹配多个重复要求列，因为每个要求列独立执行 `position`；它返回的是可匹配下标而非严格的集合多重性证明。
- `NeedMPPExchangeByEquivalence` 在“当前哈希列为空”时自身会返回 `false`；公共入口 `NeedEnforceExchanger` 只有在依赖存在且已有列非空时才调用它，否则走列数/逐项比较。扩展或直接调用前应保留这一入口条件。
- `GetSortDescForKeepOrder` 忽略 `AllSameOrder` 的第一个返回值；若排序方向混合，它得到的方向是 `false`。调用方必须先保证方向一致，或只把返回值用于已满足该不变量的路径。
- `HashCode` 是惰性快照。由于多数属性字段公开可变，一旦首次计算后再修改任何参与编码的字段，缓存不会失效，会产生陈旧键。安全用法是在属性定型后才哈希；若要继续改写，应从 `CloneEssentialFields` 等创建未初始化缓存的新值。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。绝大多数值由 planner 在单次优化期间拥有并按值/`Vec` 传递，生命周期随候选计划结束。

`PhysicalProperty::hashcode` 使用 `std::sync::OnceLock`，允许多个只读调用者并发初始化同一个缓存且只发布一个结果；`HashCode` 返回缓存内容的克隆，不暴露内部 `Vec`。不过 `PhysicalProperty` 的其他字段公开可变，`OnceLock` 只解决初始化同步，不解决“哈希后修改属性”造成的逻辑失配。

`VectorSearchInfo::Vec` 使用 `Arc` 共享查询向量，克隆属性只增加引用计数而不复制向量数据。其他列、排序项和分区列的显式 `Clone` 会复制其值；没有需要显式关闭的资源。

## 与 Go 版本的对应关系

同路径 `pkg/planner/property/physical_property.go` 也是 799 行，Rust 文件按同名类型和方法移植了主要语义：任务集合、排序项、MPP 类型/列、FD 等价判断、属性前缀/方向/keep-order、哈希字段、关键字段克隆和 Exchange 判定均可逐项对应。`pkg/planner/property/physical_property_test.go::TestNeedEnforceExchangerWithHashByEquivalence` 的六组 FD 用例由 Rust 测试 `need_enforce_exchanger_with_hash_by_equivalence_matches_go_cases` 对齐。

语言层面的差异包括：Go 用指针和 `nil` 表示可选值，Rust 对必需列使用值类型、对可选结构使用 `Option`；Go 的 `HashCode` 原地填充 `[]byte`，Rust 用线程安全的 `OnceLock<Vec<u8>>`；Go `ChoosePartitionKeys` 复用键指针，Rust 返回深拷贝；Go Explain 接收 `bytes.Buffer` 并返回该 buffer，Rust直接写入 `&mut String`。

当前 Rust 还包含 Go 文件中没有的 `PhysicalProperty::PreferTiFlash`，并将其编码进哈希、保留在关键字段克隆中；其生产使用点见 `base_physical_agg.rs` 和 `base_physical_plan.rs`。这是仓库当前 Rust 接线事实，不应误写成同路径 Go 的现有字段。另一方面，Go 生产代码直接在物理规划中调用 `property.NeedEnforceExchanger`，而 Rust 精确搜索只发现测试直接调用本文件函数；Rust 的物理计划文件目前还保留本地判定逻辑，因此这部分接线并非一比一。

## 扩展指南

- 新增会影响物理候选等价性的字段时，应同时更新 `PhysicalProperty::default`、`buildHashCode`、`CloneEssentialFields`、`MemoryUsage`（若含动态内存）和 `pkg/planner/core/operator/physicalop/cache_snapshot.rs` 的捕获/恢复逻辑。
- 新增排序或部分有序语义时，检查 `IsPrefix`、`NeedKeepOrder`、两个 `AllSameOrder`、keep-order getter，并同步独立测试 `pkg/planner/property/physical_property_test.rs`；测试不得内嵌回生产源文件。
- 修改 MPP 分区匹配时，必须同步 `MPPPartitionColumn::Equal/hashCode`、`checkEquivalence`、`NeedMPPExchangeByEquivalence` 和 `NeedEnforceExchanger`，并保留 collation 负 ID 兼容规则及“FD 路径只在已有列非空时启用”的边界。
- 修改哈希字段后，应增加“字段变化导致指纹变化”和“重复调用缓存稳定”的回归用例，并审查所有哈希后仍可能修改属性的路径。
- 扩展 `CloneEssentialFields` 前先确认该字段是必须跨算子传播，还是像 `CanAddEnforcer`、`VectorProp`、`IndexJoinProp` 一样应由特定路径显式接线；盲目复制会改变候选枚举范围。
- 对照 Go 迁移时，应以 `physical_property.go` 和 `physical_property_test.go` 的具体分支为依据，同时记录 Rust 专有字段/接线，避免为了表面一致删除 Rust 已被生产路径消费的行为。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/planner/property` 确认同目录 11 个 Go/Rust/Cargo 相关文件；`query` 定位 `PhysicalProperty`（第 413 行）、`SortItem`（第 38 行）、`MPPPartitionColumn`（第 170 行）、`NewPhysicalProperty`（第 469 行）、`NeedMPPExchangeByEquivalence`（第 512 行）和 `NeedEnforceExchanger`（第 771 行）；`node --file` 完整读取目标 799 行并报告 21 个 Rust 使用文件。精确 callers/callees 查询无输出，未将其当作“无调用”的证明。
- 源与 crate：完整核对 `pkg/planner/property/physical_property.rs`、`pkg/planner/property/lib.rs`、`pkg/planner/property/Cargo.toml`。
- Go 对照：完整核对 `pkg/planner/property/physical_property.go` 和 `pkg/planner/property/physical_property_test.go`。
- Rust 测试：完整核对独立文件 `pkg/planner/property/physical_property_test.rs`；其中覆盖六组 FD 等价用例、Any/Broadcast/Single/Hash 分区规则、排序前缀与部分有序、指纹缓存及区分字段。该文件还包含 stats/task-type 测试，但不作为本文件行为的依据。
- 调用点搜索：以 `rg` 核对 `NewPhysicalProperty`、`CloneEssentialFields`、`HashCode` 以及各物理属性字段在 `pkg/planner`、`pkg/executor`、`pkg/expression` 中的使用；确认当前 Rust 生产文件没有直接调用本文件 `NeedEnforceExchanger`。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验证要求是目标文件存在且恰有十一个规定的二级标题。
