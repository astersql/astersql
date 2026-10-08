# `pkg/planner/core/operator/physicalop/enforce.rs`

## 文件定位

`enforce.rs` 位于 `astersql-planner-core-operator-physicalop` crate，crate 根在同目录的 `lib.rs` 中以 `pub mod enforce` 公开它。该文件处在物理计划候选已经形成、但仍可能缺少调用方要求的分布或排序属性这一阶段：它接收一个简化的 `PhysicalTask`，必要时在计划树顶端补 `ExchangeSender`/`ExchangeReceiver` 或 `Sort`。

当前 Rust 接线必须与 Go 主线区分开来。全仓 Rust 搜索和 RustCodeGraph 调用图没有发现 `enforce_property`、`enforce_exchanger` 的生产调用者，只有 `enforce_test.rs` 使用这些 API；Rust 主选优文件 `pkg/planner/core/find_best_task.rs` 另有一个私有的同名 `enforce_property`，操作的是另一套 `Task` 类型。因此本文件是由 crate 公开、可独立执行和测试的 Go 语义移植骨架，尚未接入 Rust 的完整选优主链。Go 版本则由 `pkg/planner/core/find_best_task.go`、`physical_cte.go`、`physical_mem_table.go` 等真实调用。

本文件直接使用的计划数据模型来自 `physical_common_plans.rs`，没有条件编译项、模块级常量或 trait 实现。`Cargo.toml` 声明 crate 名、`lib.rs` 入口和 `autotests = false`；本文件自身只依赖同 crate 模块及标准库集合类型，不直接使用清单中的外部 crate。

## 核心职责

- `enforce_property` 是属性补偿总入口。对于 MPP 请求，它先校验候选任务与会话开关，再验证排序是否能表示为分区内排序，随后委托 `enforce_exchanger`；只要仍有排序要求且任务有效，它就在树顶包一层 `PhysicalKind::Sort`。
- `enforce_exchanger` 只在当前分区不满足请求时插入成对的 Exchange 节点，并同步任务的分区元数据；它也负责新排序规则下字符串 Hash Exchange 的兼容性拒绝与 MPP 压缩协商。
- `need_enforce_exchanger` 实现 `Any`、`Broadcast`、`Single`、`Hash` 四类分区的满足性判断；有等价关系时，Hash 分支允许已有键是所需键等价闭包中的子集。
- `sort_items_all_for_partition` 和 `equivalent` 是内部判定函数，分别处理简化的分区内排序匹配与列等价图的传递闭包。

职责边界是“改写一个已存在的简化计划候选”，而不是搜索候选、计算代价、执行 Exchange/Sort 或维护真实会话变量。它也不返回 `Result`：不可用候选通过 `PhysicalTask.invalid` 表达，面向用户的兼容性原因写入 `warnings`。

## 主要符号

- `pub struct PhysicalTask`：属性强制阶段的值对象。`plan` 是当前计划树根；`task_type` 区分 Root/Cop/MPP；`invalid` 是短路标记；`partition_type` 与 `hash_columns` 描述现有 MPP 分布；`warnings` 累积诊断。该类型通过值传递，函数返回更新后的新值。
- `pub struct EnforceContext`：从完整会话状态中抽出的最小输入。`allow_mpp` 控制总入口是否接受 MPP；`new_collation` 与 `hash_exchange_with_new_collation` 控制字符串 Hash Exchange；`string_columns` 是字符串列 ID 集合；`mpp_version` 和 `compression` 决定 Sender 的压缩字段。派生的 `Default` 会令所有布尔值为假、版本为 0、集合和字符串为空。
- `pub fn enforce_property(&PhysicalProperty, PhysicalTask, &EnforceContext, &BTreeMap<i64, i64>) -> PhysicalTask`：公开总入口。它可能令任务失效、改变任务类型、更新计划根，或原样返回。
- `pub fn enforce_exchanger(PhysicalTask, &PhysicalProperty, &EnforceContext, &BTreeMap<i64, i64>) -> PhysicalTask`：公开 Exchange 专用入口。直接调用它不会检查 `allow_mpp`、任务类型或任务既有的 `invalid`，这些前置约束只由 `enforce_property` 保证。
- `fn sort_items_all_for_partition(&PhysicalProperty) -> bool`：私有排序判定。空排序直接满足；非空时要求 `sort_items` 与 `partition_columns` 等长且逐项列 ID 相同。
- `fn need_enforce_exchanger(&PhysicalTask, &PhysicalProperty, &BTreeMap<i64, i64>) -> bool`：私有分区满足性判定。
- `fn equivalent(i64, i64, &BTreeMap<i64, i64>) -> bool`：把 map 的每个键值对视为无向边，用 `BTreeSet` 去重、`Vec` 作待访问栈，查找两个列 ID 是否连通。

相关数据定义在 `physical_common_plans.rs`：`PhysicalProperty` 携带任务类型、排序项、分区类型和分区列；`PhysicalPlanNode` 携带节点 ID、种类、schema、孩子、统计和子属性；`PhysicalKind` 提供本文件创建的 `Sort`、`ExchangeSender`、`ExchangeReceiver` 变体。

## 执行流程

调用 `enforce_property` 时的顺序如下：

1. 仅当请求的 `property.task_type` 为 `Mpp` 时进入 MPP 校验。候选不是 MPP、候选已失效或 `context.allow_mpp` 为假，都会设置 `invalid = true` 并立即返回。
2. `sort_items_all_for_partition` 若判定 MPP 排序不能由分区键承载，就追加与 Go 一致的 Sort 不支持警告，令任务失效并返回。
3. 调用 `enforce_exchanger`。若 `need_enforce_exchanger` 判定当前分区已经满足请求，任务保持原样；否则继续兼容性检查和树改写。
4. 若请求没有排序项，或前述步骤已令任务失效，总入口不会再创建 Sort。
5. 非 MPP 排序请求把 `task.task_type` 改为 `Root`。随后保存旧根的 `schema` 和 `stats`，构造 ID 为旧根 ID 加一的 `Sort`，以旧计划为唯一孩子。Sort 的 `by` 复制请求排序项，`partial` 复用分区内排序判定，子属性要求固定为 Root 和同一排序序列。

`enforce_exchanger` 的改写流程如下：

1. 按 `need_enforce_exchanger` 检查是否需要新边界。`Any` 永不强制，`Broadcast` 总是强制，`Single` 只接受已有 Single；Hash 先要求已有 Hash，再按等价闭包或精确列序比较。
2. 当启用新排序规则、未允许新排序规则 Hash Exchange、请求 Hash 分区，且任一分区列在 `string_columns` 中时，追加 HashJoin/HashAgg 字符串键警告，令任务失效，不改写计划树。
3. 保存旧根的 schema 与 stats，创建 `ExchangeSender`。Sender 的分区类型和列来自请求；`mpp_version >= 1` 才复制 `context.compression`，旧版本写空字符串；旧计划成为 Sender 唯一孩子。
4. 创建 `ExchangeReceiver` 作为新根，ID 为 Sender ID 加一，Sender 成为其唯一孩子；Receiver 与 Sender 沿用旧根的 schema/stats，二者的 `required_properties` 均为空。
5. 把任务的 `partition_type`、`hash_columns` 更新成请求值。既有 `warnings` 没有被替换，因而随按值改写自然保留。

## 数据与状态

所有状态都封装在入参和返回值中，没有全局可变状态。`PhysicalTask` 的计划、分区元数据和警告是本文件的主要可变数据；`PhysicalProperty`、`EnforceContext` 和等价边表仅借用读取。

计划改写保留旧根的 `schema` 与 `stats`：Sort 直接移动其克隆值，Exchange Sender/Receiver 各持有相同内容的克隆。旧计划整体移动到新节点的 `children`，不会与原任务共享可变引用。新节点 ID 仅按旧根 `+1`、`+2` 推导；文件没有全局 ID 分配器或冲突检测，因此调用方必须保证这种简化编号策略可接受。

Hash 等价关系以 `BTreeMap<i64, i64>` 表达边集，而非完整的 Go `FDSet`。`equivalent` 把边视为双向并计算传递闭包，例如 `1→2`、`2→3` 会令 1 与 3 等价。由于 map 的键唯一，它不能直接表达同一键发出的多条独立边；复杂函数依赖语义也不在此模型中。

Hash 满足性有两种模式：等价表和已有 Hash 键都非空时，逐个检查“已有键”能否匹配任一“请求键”，不要求两边等长；否则要求两个列 ID 向量完全相等，包含长度与顺序。成功插入 Exchange 后，任务元数据与请求分区完全同步。

## 依赖与调用关系

上游接线证据如下：

- `lib.rs` 通过 `pub mod enforce` 暴露本模块，并在 `#[cfg(test)]` 下装配 `enforce_test`。
- RustCodeGraph 对 `enforce_property` 的解析显示其调用 `sort_items_all_for_partition`、`enforce_exchanger` 并构造 `PhysicalKind::Sort`、`PhysicalPlanNode`；对当前 Rust 仓库的调用者查询为空。
- `rg` 只在 `enforce_test.rs` 找到本模块公开类型和函数的外部 Rust 使用；`find_best_task.rs` 中的同名私有函数属于另一套实现，不是调用边。

下游依赖集中在两处：

- `crate::physical_common_plans::{PartitionType, PhysicalKind, PhysicalPlanNode, PhysicalProperty, TaskType}` 提供简化计划和属性类型。
- `std::collections::{BTreeMap, BTreeSet}` 分别承载等价边与闭包遍历的已访问集合。

Go 应用主链是对照证据而非 Rust 现有调用边：`pkg/planner/core/find_best_task.go` 在候选算子附着完成并决定添加 enforcer 后调用 `physicalop.EnforceProperty`，DataSource 的强制属性路径也调用它；`physical_cte.go` 和 `physical_mem_table.go` 还有直接调用。Go 的 MPP 任务与属性实现继续依赖 `base.Task`、`MppTask`、会话变量、`funcdep.FDSet` 及真实 PhysicalSort/Exchange 算子。

## 错误处理与边界

本文件没有可恢复错误类型，也不显式 `panic`。属性不兼容通过 `invalid` 和 `warnings` 表达：MPP 类型/开关错误只置失效、不附加警告；MPP Sort 不兼容和新排序规则字符串 Hash Exchange 会附加固定警告后置失效。调用方应先检查 `invalid`，不能把返回的计划形状本身当作成功标志。

重要边界包括：

- 空 `sort_items` 总被视为分区内排序满足，因此不会触发 MPP Sort 拒绝，也不会创建 Sort。
- `sort_items_all_for_partition` 只比较列 ID，不比较 `SortItem.descending`；因为简化模型用 `partition_columns` 代替 Go 的 `SortItemsForPartition`，无法表达 Go 对升降序也必须一致的检查。
- `enforce_property` 才校验 `allow_mpp`、任务类型和既有失效状态；直接调用 `enforce_exchanger` 时调用者必须自行满足这些前置条件。
- 非 MPP 排序路径只是把简化任务标签改为 Root 并包 Sort，不等价于 Go `ConvertToRootTask` 的完整执行层转换。
- 字符串兼容检查依赖调用者正确填充 `string_columns`；它只按列 ID 查找，不读取字段类型。
- ID 加法没有溢出或全树唯一性检查；等价图遍历虽有 `visited` 防环，但复杂度会随边数和访问节点数增长。
- `Broadcast` 请求即使当前元数据已是 Broadcast 仍会插入新 Exchange，这是与 Go `NeedEnforceExchanger` 一致的规则，不是幂等行为。

## 并发与资源生命周期

模块不创建线程、异步任务、锁、通道、事务、网络流或文件句柄，因此没有跨线程同步和显式清理阶段。所有输入要么按值取得所有权，要么在调用期间不可变借用；返回后，新的 `PhysicalTask` 独占改写后的计划树。

资源成本主要来自克隆与遍历：Sort 会克隆排序项及旧根 schema/stats；Exchange 会克隆分区列、schema、stats 和可选压缩字符串；等价检查对每个待匹配 Hash 键执行图遍历，并在每次弹出节点时扫描 `BTreeMap` 全部边。当前实现面向小型简化元数据，若接入更大的真实 FD 图，应重新评估数据结构和复杂度。

警告的生命周期与任务一致。Exchange 成功改写不会清空警告；兼容性失败在原列表尾部追加一条。计划孩子通过所有权嵌套，任务离开作用域时由 Rust 自动递归释放，无需手工回收。

## 与 Go 版本的对应关系

Rust `enforce_property` 对应 `enforce.go` 的 `EnforceProperty`：两者都先处理 MPP/Exchange，再在需要时转换到 Root 语义并附着 Sort；都在 MPP Sort 不受支持时返回无效候选并发出相同文本警告。Rust 的 `sort_items_all_for_partition` 是 Go `PhysicalProperty.IsSortItemAllForPartition` 的压缩表示，但 Go 比较 `SortItems` 与独立的 `SortItemsForPartition`，同时检查列和升降序，Rust 仅将排序列与 `partition_columns` 对齐。

Rust `enforce_exchanger` 合并了 Go `MppTask.EnforceExchanger` 与 `EnforceExchangerImpl` 的主要分支：先调用满足性判定，再检查新排序规则字符串键，最后构造 Sender/Receiver 并在 MPP v1 起设置压缩。Go 会先 `Copy` 任务、从真实计划上下文取版本和压缩、使用字段类型判断字符串列，并复制 `Warnings`；Rust 使用按值任务和显式 `EnforceContext` 达成简化等价效果。

Rust `need_enforce_exchanger` 对应 `property.NeedEnforceExchanger`。Any/Broadcast/Single/Hash 的基本规则一致；Go 在有 `FDSet` 时调用 `NeedMPPExchangeByEquivalence`，Rust 用无向整数边闭包近似。`pkg/planner/property/physical_property_test.go` 与 Rust 的 `physical_property_test.rs` 覆盖了更完整的等价类用例；本模块的 `enforce_test.rs` 另外验证了等价键子集能避免重复 Exchange。

目前仍有明确迁移差距：Rust 模块没有生产调用者；`PhysicalTask` 不是真实 `MppTask`/`base.Task`；非 MPP Root 转换、QueryBlockOffset、`ExpectedCnt = MaxFloat64`、真实 ByItems 表达式及代价/统计接线均被简化。因此扩展时应以 Go 行为为规范，并避免把这份骨架直接描述为已替代 Go 主线。

## 扩展指南

若新增属性强制规则，首先判断它属于总入口顺序、Exchange 满足性还是计划节点构造：总入口校验放在 `enforce_property`；分区是否已满足放在 `need_enforce_exchanger`；会话兼容性和 Exchange 节点字段放在 `enforce_exchanger`。不要把分区判断散落到调用者，否则会破坏“先判定、后改写”的单一入口。

修改时应保持以下不变量：失效任务不再附着 Sort；Exchange Receiver 位于 Sender 之上；新节点保留输入 schema/stats；成功 Exchange 同步任务分区元数据；已有警告不丢失；MPP v1 之前不写压缩；Any/Broadcast/Single 的规则与 Go 一致。若引入真实任务类型或 FDSet，应优先复用 `pkg/planner/property` 与现有物理算子，而不是继续扩大平行的简化模型。

测试应继续放在独立文件 `pkg/planner/core/operator/physicalop/enforce_test.rs`，不要内嵌到生产源文件。新增分支至少覆盖成功、不需要改写、拒绝并告警三类结果；排序变更应覆盖列序和升降序；Hash 变更应与 `pkg/planner/property/physical_property_test.rs` 及 Go 的 `physical_property_test.go` 等价用例同步。若将模块接入生产主链，还需要在真正调用点增加集成级计划形状测试，证明执行的是本模块而非 `find_best_task.rs` 的私有同名函数。

兼容性风险集中在 Go/Rust 属性模型差异、会话开关默认值和警告文本；正确性风险集中在 Hash 等价方向、键子集规则及树节点次序；性能风险集中在等价闭包的重复全边扫描与计划元数据克隆。任何扩展都应对照 `enforce.go`、`physical_property.go` 和两侧独立测试逐项核验。

## 验证依据

本说明基于以下直接证据：

- Rust 源码：`pkg/planner/core/operator/physicalop/enforce.rs` 的 `PhysicalTask`、`EnforceContext`、`enforce_property`、`enforce_exchanger`、`sort_items_all_for_partition`、`need_enforce_exchanger`、`equivalent`。
- Rust 数据模型与装配：`physical_common_plans.rs` 的 `TaskType`、`PartitionType`、`SortItem`、`PhysicalProperty`、`PhysicalKind`、`PhysicalPlanNode`，以及 `lib.rs` 的 `pub mod enforce`/`mod enforce_test`。
- crate 边界：`pkg/planner/core/operator/physicalop/Cargo.toml` 的 package、lib、dependencies、dev-dependencies 和 `package.metadata.porting.go-package`。
- Rust 测试：`enforce_test.rs` 覆盖 Any 不强制、Broadcast 总强制、Single 忽略残留 Hash 列、等价键子集、MPP Sort 拒绝、新排序规则字符串 Hash 拒绝、警告保留和 MPP v1 压缩；`pkg/planner/property/physical_property_test.rs` 覆盖更完整的 Go 对齐等价类规则。
- Go 对照：`enforce.go` 的 `EnforceProperty`、`MppTask.EnforceExchanger`、`EnforceExchangerImpl`；`pkg/planner/property/physical_property.go` 的 `IsSortItemAllForPartition`、`NeedEnforceExchanger`；`physical_property_test.go` 的等价类回归；`pkg/planner/core/find_best_task.go` 等调用点。
- RustCodeGraph：状态显示项目已索引；`node --file .../enforce.rs` 读取到完整 214 行；`query enforce_property` 区分出本模块入口和 `find_best_task.rs` 私有同名函数；callees 显示本入口调用 `enforce_exchanger`、`sort_items_all_for_partition` 并构造 Sort/计划节点；callers 对本模块入口无生产调用结果。补充 `rg` 搜索确认模块外 Rust 使用仅来自独立测试。

本任务是纯文档分析，按计划未运行 Cargo。结构验收另以任务指定命令确认文件存在且恰有十一个固定二级章节；人工复核重点是当前未接线事实、Go/Rust 差异、直接调用前置条件和扩展风险均有源码或测试依据。
