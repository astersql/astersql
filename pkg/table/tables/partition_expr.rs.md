# `pkg/table/tables/partition_expr.rs`

## 文件定位

`ForRangePruning::Compare` 提供二分定位所需的三值比较，并处理无符号解释与末尾 `MAXVALUE`；`ForRangeColumnsPruning` 保存多列上界，其中 `None` 表示 `MAXVALUE`。

## 核心职责

1. `NewPartitionExprBuildCtx` 创建分区表达式构建上下文：SQL mode 固定为 `ModeAllowInvalidDates`，忽略截断、零日期、日期含零以及无效日期错误，并把截断错误组降为 `LevelIgnore`；同时从所属 `TableCommon` 绑定新排序规则开关。其目的与 Go `NewPartitionExprBuildCtx` 一致，是让分区定位所需的常量折叠在非严格日期环境下完成。
2. `PartitionExpr` 聚合不同分区类型共用或专用的数据：RANGE 定位表达式、HASH/LIST 表达式、原始 AST、分区列偏移，以及 KEY/RANGE/RANGE COLUMNS/LIST 裁剪状态。
3. `ForKeyPruning::LocateKeyPartition` 按列顺序构造 IEEE CRC32 字节流并取模，保持 KEY 分区定位与 Go 一致；`PartitionExpr::GetPartColumnsForKeyPartition` 把全表列投影成紧凑分区键行布局。
4. `ForRangePruning::Compare` 提供二分定位所需的三值比较，并处理无符号解释与末尾 `MAXVALUE`；`ForRangeColumnsPruning` 保存多列上界，其中 `None` 表示 `MAXVALUE`。
5. `ForListPruning` 保存单表达式 LIST 的值到分区映射、NULL/DEFAULT 回退以及 LIST COLUMNS 的每列裁剪器；`ForListColumnPruning::GenKey` 负责类型转换和排序规则敏感的键编码。
6. `ListPartitionGroup`、`ListPartitionLocation` 与 `ListPartitionLocationHelper` 表达 LIST COLUMNS 谓词的候选分区/值组集合，并提供析取的并集及合取的交集操作。

## 主要符号

- `NewPartitionExprBuildCtx(&TableCommon) -> ExprContext`：公开构造函数。与 Go 无参版本相比，Rust 显式接收表实例，以便把 `TableCommon::use_new_collation()` 写入上下文。
- `PartitionExpr`：公开、可克隆、可默认构造的汇总结构。`UpperBounds` 用于 RANGE 的逐分区上界表达式；`OrigExpr` 是 point-get 所需 AST；`Expr` 承载 HASH 或普通 RANGE/LIST 表达式；四个 `For*Pruning` 字段以 `Option` 对应 Go 的可空嵌入指针；`ColumnOffset` 是分区列在表 schema 中的位置。
- `PartitionExpr::ForTable`：用表实例的排序规则模式覆盖 `ForKeyPruning.UseNewCollate` 以及全部 `ForListPruning.ColPrunes[*].UseNewCollate`。它返回更新后的值，不改变传入表。
- `PartitionExpr::GetPartColumnsForKeyPartition`：按 `ColumnOffset` 选择 `Column`，把被选源列的 `Index` 原地改成紧凑行下标，克隆后返回，并同步返回各列 `FieldType.GetFlen()`。
- `ForKeyPruning { KeyPartCols, UseNewCollate }` 与 `LocateKeyPartition`：用各列的 `Index` 从输入行取值；NULL 写入单字节 `0`，非 NULL 先 `ToString`，再按 datum 自带 collation 和实例模式取得 collator key，最后计算 `crc32 % (num_parts as u32)`。
- `ForRangePruning { LessThan, MaxValue, Unsigned }` 与 `Compare`：保存整数上界。方法的 `unsigned` 参数决定本次比较方式；结构字段 `Unsigned` 本身仅作为元数据保存，不会被 `Compare` 自动读取。
- `ForRangeColumnsPruning { LessThan }`：二维上界表，外层对应分区、内层对应分区列，`Option::None` 表示从该列开始的 `MAXVALUE`。
- `ForListPruning`：`LocateExpr` 面向行定位，`PruneExpr`/`PruneExprCols` 面向裁剪；`ValueToPartitionIdx` 是已编码整数到分区下标的只读映射；`NullPartitionIdx` 和 `DefaultPartitionIdx` 以负数表示不存在；`ColPrunes` 用于 LIST COLUMNS。
- `ForListPruning::LocatePartition` / `GetDefaultIdx`：前者按 NULL、精确值、DEFAULT 的顺序返回下标，后者直接暴露默认分区下标。
- `ForListColumnPruning`：保存目标表达式列、目标类型、排序规则模式、精确值映射 `ValueMap`、有序映射 `Sorted` 和 `DefaultPartID`。`GenKey` 执行 `Datum::ConvertTo` 后用 `codec::NewEncoder(UseNewCollate)` 编码；`HasDefault` 仅在 `DefaultPartID > 0` 时为真。
- `ListPartitionGroup { PartIdx, GroupIdxs }`：一个分区及该分区内匹配的值元组组号；私有 `intersect` 取同分区组号交集，私有 `union` 按 Go 语义直接追加、不去重。
- `ListPartitionLocation(Vec<ListPartitionGroup>)`：候选位置集合；`IsEmpty` 只有在所有组的 `GroupIdxs` 都为空时才为真，空向量也为真。
- `ListPartitionLocationHelper` / `NewListPartitionLocationHelper`：内部保存 `initialized` 和累计 `location`；公开方法为 `GetLocation`、`UnionPartitionGroup`、`Union`、`Intersect`。

## 执行流程

元数据构建流程如下：

1. `table_from_meta_for_validation` 从完整 `model_dependency::TableInfo` 取得 `PartitionInfo`，先拒绝没有定义的分区元数据。
2. 在 `expression-runtime` 下，它调用 `canonical_partition_expr::build`。后者创建表达式上下文，把 catalog 列转成 `expression::Column`，解析分区表达式并填充 `PartitionExpr.ColumnOffset`。
3. HASH 分支保存原始 AST 和已计算 hash code 的 `Expr`；KEY 分支构造 `ForKeyPruning`；RANGE 分支构造 `UpperBounds`，再按普通 RANGE 或 RANGE COLUMNS 分别填充 `ForRangePruning` / `ForRangeColumnsPruning`；LIST 分支按是否有列列表构造 `ForListPruning` 的整数映射或 `ForListColumnPruning` 的编码映射。
4. 若 DDL 正处于重组、移除分区或改变分区方式阶段，加载器还针对 adding/dropping definitions 构造 `reorganization_expression`。两份表达式随后进入 `ValidatedTableMetadata`，供校验和测试观察。

KEY 定位流程为：调用者先用 `GetPartColumnsForKeyPartition` 将选中列重编号为紧凑输入行位置；`LocateKeyPartition` 再依 `KeyPartCols` 顺序读取 datum。NULL 贡献 `[0]`，其他值经字符串化与 collation key 规范化后写入 CRC32，最终按 Go 的规则先把分区数截成 `u32` 再取余。

普通 LIST 定位流程为：NULL 优先返回 `NullPartitionIdx`，若没有专用 NULL 分区则返回 `DefaultPartitionIdx`；非 NULL 根据 `PruneExpr` 类型的 unsigned flag 选择原始 `u64` 或 `EncodeIntToCmpUint`，查询 `ValueToPartitionIdx`，未命中时回退 DEFAULT。

LIST COLUMNS 的组合流程为：各列值先由 `GenKey` 转换并编码，映射为 `ListPartitionLocation`。CNF/多列约束通过 `Intersect` 组合：第一次调用只克隆输入并标记已初始化，后续按 `PartIdx` 配对并对 `GroupIdxs` 取交集；DNF/范围结果通过 `Union` 合并，同分区只追加组号。当前 Rust planner 文件 `pkg/planner/core/rule/rule_partition_processor.rs` 中可见的是对应 Go 流程的移植注释，并非已执行的 Rust 调用。

## 数据与状态

所有结构都实现 `Clone` 和 `Default`。表达式由 `ExprBox`（trait object）拥有；列、边界向量和 location 在 clone 时各自复制。较大的只读查找结构使用 `Arc<BTreeMap<...>>`：`ForListPruning.ValueToPartitionIdx`、`ForListColumnPruning.ValueMap` 和 `Sorted` 的 clone 共享底层树，独立测试 `list_pruning_clone_deep_clones_columns_and_shares_lookup_trees` 明确验证了“列可独立修改、映射仍为同一 Arc”的不变量。

下标/哨兵约定必须保持一致：`ColumnOffset`、`PartIdx`、`GroupIdxs` 使用 `usize`；LIST 的 NULL/DEFAULT 分区下标使用 `isize`，负值表示缺失；`DefaultPartID` 是 catalog 物理分区 ID，只有正值代表已初始化；RANGE COLUMNS 用 `None` 表示 `MAXVALUE`；普通 RANGE 则用末元素配合 `MaxValue` 标志表示无穷上界。

`ListPartitionLocationHelper.initialized` 与 `location` 是唯一会在操作中变化的内部状态。值得注意的是，先调用 `Union` 不会设置 `initialized`；之后第一次 `Intersect` 仍会用传入 location 覆盖已有并集。这与 Go `listPartitionLocationHelper` 的状态机一致，调用方应按谓词组合语义选择操作顺序。

`ForRangePruning.Unsigned` 与 `Compare` 的 `unsigned` 参数并存：构造器记录元数据，实际比较由调用者显式传参。这一接口允许调用点选择解释方式，但也意味着错误参数会产生错误的分区边界判断。

## 依赖与调用关系

上游直接证据：

- `pkg/table/tables/lib.rs` 在默认 `expression-runtime` feature 下编译并重导出本模块。
- `pkg/table/tables/canonical_partition_expr.rs::build` 是生产代码中明确构造所有 `PartitionExpr` 分支的入口，并调用 `ForListColumnPruning::GenKey` 建立 LIST COLUMNS 映射。
- `pkg/table/tables/tables.rs::table_from_meta_for_validation` 调用上述 builder，保存正常与 DDL 重组期表达式。
- `pkg/table/tables/partition_expr_test.rs` 直接覆盖上下文、排序规则绑定、KEY 哈希、RANGE 比较、LIST 键、集合交并及 clone 语义；`pkg/session/runtime/normal_ddl_create_table_test.rs:998-1028` 和 `pkg/table/tables/tables_test.rs:1169-1179` 从元数据加载入口检查构造结果。
- `pkg/planner/core/base/misc_base.rs::PartitionedTable::partition_expr` 声明返回 `tables::PartitionExpr` 的接口，但本次搜索未找到其生产实现；planner 的 Go 对照调用仍主要保留在注释中。

下游依赖：

- `expression::Column`、`ExprBox`、AST、Datum、FieldType、Build/Eval/Type/Error Context、collator 和 codec 提供表达式表示、类型转换、排序键与错误策略。
- `exprstatic_dependency` 构造静态 build/eval context。
- `crc32fast::Hasher` 对齐 Go `crc32.NewIEEE`。
- `std::collections::BTreeMap` 提供稳定有序的查找结构，`Arc` 让已构建映射可廉价共享。
- `crate::tables::TableCommon` 提供表实例级的新排序规则开关。

运行链边界：当前 `pkg/session/runtime/dml.rs` 使用 `canonical_partition::CanonicalPartitionedTable::locate` 直接根据 catalog 元数据定位物理分区，并不读取 `ValidatedTableMetadata.partition_expression`。所以修改本文件的定位方法不会自动改变现有 DML 路由；若要接线，必须同时核对 DML、planner trait 实现及 `canonical_partition.rs`，但这些修改不属于本分析任务。

## 错误处理与边界

`LocateKeyPartition` 只传播 `Datum::ToString` 失败。其他前置条件通过索引或 `expect` 表达：列 `Index` 必须非负且落在输入行内，`num_parts as u32` 必须非零；否则转换、索引或取模会 panic。由于先转 `u32`，`num_parts == 0` 或低 32 位为零同样非法。`GetPartColumnsForKeyPartition` 要求每个 offset 有效且目标列存在 `RetType`，违约时索引或 `expect("partition column must have a return type")` panic。

`ForRangePruning::Compare` 要求 `LessThan` 非空且 `index` 有效；它会计算 `len() - 1`，空向量会下溢/越界。若末下标且 `MaxValue` 为真，无论输入值均返回 `1`；其他情况严格返回 `-1/0/1`。调用者必须把构造时的 unsigned 语义正确传入方法。

`ForListPruning::LocatePartition` 在非 NULL 路径要求 `PruneExpr` 已初始化，否则以 `expect` panic。没有 NULL、DEFAULT 或精确映射时可能合法返回负数，调用者负责将其转成“给定值无分区”的用户错误；Go 的 `locateListPartitionByRow` 正是在外层完成该转换，本文件不生成表级错误。

`ForListColumnPruning::GenKey` 在 `ValueType` 缺失时返回显式 expression error；`ConvertTo` 失败直接传播。编码失败交给语句 `error_context.HandleError`：策略保留错误时重新包装并返回，策略忽略错误时返回空字节键。空键因此不能简单解释为“原值确实编码为空”，调用方必须沿用同一语句错误策略。`HasDefault` 严格使用 `DefaultPartID > 0`，零和负数都视为未初始化。

location 交并只比较 `PartIdx` 与组号，不排序也不去重；`Union` 的重复组号是与 Go 对齐的可观察行为。`Intersect` 第一次即使输入为空也返回 `true`，因为首次调用只负责初始化；第二次及以后才以剩余分区是否非空决定返回值。`IsEmpty` 则检查所有组号向量，而非只检查外层向量长度。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件或网络资源。表达式上下文和所有裁剪对象由调用方同步构造并按普通 Rust 所有权释放；没有显式清理阶段。

`Arc<BTreeMap<...>>` 的用途是共享不可变、构建完成的查找树，而不是提供可变并发协议。本文件没有通过 `Arc::make_mut`、锁或内部可变性更新这些映射。`ListPartitionLocationHelper` 通过 `&mut self` 串行累积位置，不适合作为无同步的共享可变对象。是否能跨线程传递整个 `PartitionExpr` 还取决于其中 `ExprBox` 的 trait 约束；本文件没有声明或验证额外的 `Send`/`Sync` 保证，不应仅凭 `Arc` 推断线程安全。

clone 生命周期中，列/向量/位置集合独立，三个 Arc 映射共享引用计数；最后一个拥有者释放后底层树自动回收。`NewPartitionExprBuildCtx` 内部把 eval context 放入 `Arc` 后交给 expr context 持有，生命周期同样由引用计数管理。

## 与 Go 版本的对应关系

- Rust `NewPartitionExprBuildCtx(&TableCommon)` 保留 Go 的非严格日期/截断策略，并额外在构造时绑定表实例的 new-collation；Go 是先无参构造、再在 `newPartitionExpr` 中 `Apply(WithNewCollationEnabled(...))`。
- `GetPartColumnsForKeyPartition` 保留 Go 指针共享导致的源列 `Index` 原地修改语义，同时返回 Rust clone。测试 `key_partition_columns_are_cloned_and_reindexed` 固化了这一点。
- KEY 哈希保留 NULL 的单零字节、按列顺序追加、collation key 和 IEEE CRC32；取模前把分区数转为 `u32`。Rust 对所有非 NULL datum 统一 `ToString`，而 Go 对 string/bytes 直接 `GetString`、其他类型 `ToString`；当前测试覆盖普通字符串、NULL 和新旧排序规则，但未证明所有 bytes/非字符串 datum 的逐字节等价。
- 普通 LIST 定位保留 NULL 优先、DEFAULT 回退、有符号整数经 `EncodeIntToCmpUint` 的规则；Rust 用 `Arc<BTreeMap<u64, usize>>` 代替 Go 泛型 B-tree。
- LIST COLUMNS 的 `ValueMap`/`Sorted` 都由 `BTreeMap<String, ListPartitionLocation>` 表示；Go 分别使用 map 与 B-tree。Rust `GenKey` 对齐 Go `genKey` 的 ConvertTo、encoder 和 error-context 处理，但延迟构建/重建字段与方法（Go 中的 `ctx`、`tblInfo`、`schema`、`names`、`colIdx`、`RebuildPartitionValueMapAndSorted`、范围查询等）不在本文件中。Rust builder 当前一次性构建映射。
- Rust `PartitionExpr::ForTable` 是实例级 collation 绑定辅助，属于 Rust 接线方式；Go 的相同效果分散在 `newPartitionExpr` 及各裁剪器构造过程。

迁移状态并非“一比一完整替代”：完整 catalog builder 已能构造各类结构，相关元数据校验测试可观察结果；实际 DML 路由由 `canonical_partition.rs` 的另一套逻辑完成，planner 裁剪文件中的对应流程仍是 Go 移植注释。

## 扩展指南

- 新增或改变分区表达式字段时，应同时修改 `PartitionExpr`、`canonical_partition_expr::build`、`ValidatedTableMetadata` 使用点，并核对 DDL 重组表达式是否也需要填充；测试应放在独立的 `pkg/table/tables/partition_expr_test.rs` 或加载入口的现有独立测试文件中，不能内嵌到生产源文件。
- 修改 KEY 算法时，优先扩展 `key_partition_uses_ieee_crc32_and_null_marker` 和 `go_merge_49_key_partition_uses_instance_collation_mode`，覆盖 bytes、数值、日期、NULL、多列次序、异常 `num_parts` 与 Go 逐字节结果。任何 hash 输入变化都会改变物理分区选择，属于数据兼容性高风险变更。
- 修改 collation 绑定时，必须同步检查 `NewPartitionExprBuildCtx`、`PartitionExpr::ForTable`、`canonical_partition_expr::context/build` 和 `canonical_partition::CanonicalPartitionedTable`；新旧排序规则不一致会使建表时的映射键、裁剪键和执行路由分歧。
- 扩展 RANGE 时应在 `ForRangePruning::Compare` 或 `ForRangeColumnsPruning` 附近实现，并补齐 signed/unsigned、`MAXVALUE`、空/越界前置条件和多列字典序测试。若改变 `Unsigned` 的使用方式，要清楚决定是否移除当前“字段记录、参数控制”的双重来源。
- 扩展普通 LIST 时应保持构建键与查询键完全相同；负下标哨兵不能转换成 `usize`。若增加范围查询，需要参考 Go `LocatePartitionByRange` 的端点、NULL、DEFAULT 和有序遍历语义，而不能只复用精确查找。
- 扩展 LIST COLUMNS 时，`GenKey`、`ValueMap` 和 `Sorted` 必须使用同一类型上下文、error context 与 new-collation 模式。实现 Go 的延迟重建或范围定位时，应明确映射何时发布、是否仍保持不可变 Arc，以及 DEFAULT 特殊组号语义。
- 修改 `Union`/`Intersect` 时不要擅自去重或排序；这些细节已由 Go 行为和 Rust 回归测试锁定。若要改变首次 `Intersect` 的返回约定，也必须先核对 planner 调用点。
- 若要把本文件真正接入 planner 或 DML 主链，应先实现/定位 `PartitionedTable::partition_expr` 的生产实现，再决定与 `canonical_partition.rs` 统一还是替换；同时需要 SQL 集成测试证明构造、裁剪与物理路由三者选择相同分区。该工作超出本纯文档任务范围。
- 性能上应保留映射的共享只读特性，避免每行 clone B-tree 或重复创建 collator/表达式上下文；正确性上重点防范 CRC32 输入、编码排序、NULL/DEFAULT 哨兵及 DDL 重组两套表达式漂移。
