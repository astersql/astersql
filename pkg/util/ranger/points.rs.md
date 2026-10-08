# `pkg/util/ranger/points.rs`

## 文件定位

`points.rs` 是 `astersql-util-ranger` crate 内“SQL 谓词 → 有序区间端点”的实现文件。crate 根 `pkg/util/ranger/lib.rs` 以 `points_impl` 私有模块装入本文件，并重新导出其中的公开全范围工厂；内部的 `builder`、`point` 则由同 crate 的 `ranger.rs` 和 `detacher.rs` 直接使用。它位于表达式分析和最终 `Range` 物化之间：上游交给它单列比较、`IN`、`LIKE`、真假判断及其否定形式，下游 `ranger.rs` 再把成对端点转换为表范围或索引/列范围。

该 crate 的边界由 `pkg/util/ranger/Cargo.toml` 定义，包名为 `astersql-util-ranger`，Go 对照包元数据为 `pkg/util/ranger`。本文件直接使用 crate 根重导出的 `expression` 能力，以及 `rangerctx`、`plannererrors`、`types`、collation/MySQL 类型常量；它不是独立入口，也不负责选择哪些谓词能成为 access condition，那部分主要由 `checker.rs`、`detacher.rs` 完成。

## 核心职责

1. 用 `point { value, excl, start }` 表示一个范围的左/右端点，并定义相同值下开闭端点的稳定排序规则（`rangePointCmp`、`rangePointEqualValueCmp`）。
2. 由 `builder::build` 按表达式动态类型和标量函数名分派，将常量、裸列、二元比较、`AND`/`OR`、`IN`、`LIKE`、`IS NULL`、真假谓词及部分 `NOT` 形式变成偶数个端点。
3. 在构造过程中保持 MySQL/TiDB 语义：处理 NULL、unsigned 负常量、整数/float 越界、YEAR 修正、ENUM 枚举值、字符串 collation、前缀索引以及新 collation sort key。
4. 用扫描线算法对端点序列求交集或并集，并通过 `builder.err` 把比较、求值或不支持形式的错误传给调用方。
5. 提供 `FullIntRange`、`FullRange`、`FullNotNullRange`、`NullRange` 等已经物化为 `Ranges` 的公共边界工厂，以及供 crate 内构造流程使用的 `getFullRange`。

## 主要符号

- `RangeType = i32` 及 `IntRangeType`、`ColumnRangeType`、`IndexRangeType`：保持 Go `iota` 的判别值顺序。当前文件只声明这些值；具体消费方应通过仓库搜索确认，不能假定它们构成 Rust enum。
- `point`：crate 内部端点。`value` 是 `types::Datum`；`start=true` 表示左端点；`excl=true` 表示不包含端点值。`point::String` 仅提供 Go 风格调试文本，把 `MinNotNull`/`MaxValue` 显示为 `-inf`/`+inf`。
- `rangePointCmp`、`rangePointEnumCmp`、`rangePointEqualValueCmp`：先按 Datum（ENUM 特判为整数值）排序，再以端点方向和开闭性消除相同值歧义。这一全序是排序、去重和扫描线合并的共同前提。
- `convertPointToSortKeyInPlace`、`convertPointsToSortKeyInPlace`：先调用 `ranger_impl::convertPointInPlace` 做目标类型转换，再在启用新 collation 的普通字符串上生成 sort key；ENUM/SET 不走这一通用路径。
- `getFullRange`、`getNotNullFullRange`：每次创建新的端点向量，避免共享可变端点。前者从 NULL 默认 Datum 到 `MaxValue`，后者从 `MinNotNull` 到 `MaxValue`。
- `FullIntRange`、`FullRange`、`FullNotNullRange`、`NullRange`：公开的 `Ranges` 工厂。整型全范围使用真实 `i64`/`u64` 边界，而非 `MaxValueDatum`。
- `builder<'a, 'ctx>`：保存 `RangerContext` 引用和首个可传播错误。`build` 是核心入口，`buildFromScalarFunc` 是标量函数分派中心。
- `buildFromBinOp`：识别列位于比较式哪一侧，必要时反转比较方向；随后依次调用 `refineValueAndOp`、`handleUnsignedCol`、`handleBoundCol`，最后生成 EQ/NE/LT/LE/GT/GE/NullEQ 端点。
- `buildFromIn`：求值常量列表，跳过 NULL 并记录 `hasNull`，处理 ENUM/YEAR/字符串类型，生成点范围，按端点规则排序去重，再按需裁剪前缀和转换 sort key。
- `newBuildFromPatternLike`：提取第一个未转义 `%`/`_` 之前的前缀；根据 collation、PAD SPACE、前缀长度和 sort key 能力选择精确点、前缀区间或保守非 NULL 全范围。
- `buildFromNot`：支持真假谓词、`NOT IN`、`NOT LIKE`、`IS NOT NULL`。其中 `NOT IN` 必须先对完整值集合取补集，再裁剪前缀/生成 sort key。
- `intersection`、`union`、`mergeSorted`、`merge`：合并两个已经排序的端点序列，并按覆盖计数分别选择同时覆盖（AND）或任一覆盖（OR）的边界。
- `handleUnsignedCol`、`handleBoundCol`、`handleEnumFromBinOp`、`refineValueAndOp`：集中承载 unsigned、数值边界、ENUM 和字符串/YEAR 的比较修正。

本文件没有条件编译项；测试装配发生在 `lib.rs` 的 `#[cfg(test)] #[path = "points_test.rs"]`，符合源文件与测试文件分离要求。

## 执行流程

典型调用链是 `ranger.rs::buildColumnRange` 或 `detacher.rs::ExtractEqAndInCondition` 创建 `builder`，对每个单列条件调用 `builder::build`，再用 `intersection` 合并 CNF 条件。`ranger.rs::buildColumnRange` 从 `getFullRange()` 开始，字符串路径采用二进制 collator 合并已经转换的 sort key；之后由 `points2TableRanges` 或 `points2Ranges` 把每两个端点物化为一个 `Range`。

`builder::build` 的分派规则如下：裸列走 `buildFromColumn`，语义等价于“列为真”，产生 `[-inf,0)` 与 `(0,+inf]`；常量走 `buildFromConstant`，NULL、false 或求值失败产生空端点，true 产生全范围；标量函数交给 `buildFromScalarFunc`；未知表达式类型保守返回全范围。

二元比较流程为：确定列和常量 → 列在右侧时反转 GE/GT/LT/LE → 非 NullEQ 与 NULL 比较直接为空 → 统一字符串 collation 和 YEAR 值/操作符 → 修正 unsigned 与数值越界 → ENUM 字符串列枚举所有合法值，否则按操作符生成端点 → 调用 `cutPrefixForPoints` → 可选转换 sort key。NE 会生成常量两侧的两个区间；NullEQ(NULL) 生成 `[NULL,NULL]`。

`IN` 为每个非 NULL 合法常量产生 `[v,v]`，端点排序后利用 start/end 交替关系去重。`NOT IN` 如果列表含 NULL，按 SQL 三值逻辑返回空范围；否则把去重后的点集合补成各值之间的开区间，对 unsigned 整数先丢弃负值点。前缀裁剪延后到补集生成之后，避免例如前缀索引把不同完整值过早折叠并漏扫中间数据。

`LIKE` 先验证表达式与列 collation 兼容。空模式形成空字符串点范围；通配符前没有字符时返回非 NULL 全范围；没有通配符时形成精确点。存在固定前缀时，只有二进制 collation 或允许生成 sort key 才能形成窄范围；起点按 PAD SPACE 规则决定是否裁剪尾随空格，终点通过未裁剪 sort key 的最后一个可进位字节加一得到，全部溢出则使用 `MaxValue`。

逻辑 `AND`/`OR` 递归构造左右端点，再分别调用 `intersection`/`union`。`mergeSorted` 先按统一端点顺序归并；`merge` 扫描 start/end 事件并维护覆盖计数：交集要求计数达到 2，并集要求达到 1，仅在进入或离开所需覆盖层数时输出端点。

## 数据与状态

端点向量的关键不变量是：长度应为偶数，`[0,1]`、`[2,3]` 等分别是一段范围；每段先左端点后右端点；用于合并前必须按 `rangePointCmp` 排序。`start` 与 `excl` 共同决定相同 Datum 的事件次序，因此不能只比较 `value`，否则相接的开/闭区间会被错误合并或切断。

特殊 Datum 承担哨兵语义：默认 Datum 表示 NULL 边界，`MinNotNullDatum` 表示最小非 NULL 值，`MaxValueDatum` 表示正无穷式上界。表 handle 范围不能把 `MaxValueDatum` 当真实整数，因此 `FullIntRange` 和 `points2TableRanges` 使用具体整数边界。

`builder` 唯一的可变跨步骤状态是 `err: Option<errors::Error>`。多数需要向外传播的失败会写入该字段；`ranger.rs::buildColumnRange` 每次构造/求交后取出并返回错误。端点本身和值在构造、类型转换、前缀裁剪和 sort key 转换中会原地修改，所以 `getFullRange` 每次分配新向量。

`prefixLen` 等于 `types::UnspecifiedLength` 时不做前缀索引裁剪；`convertToSortKey` 决定字符串端点是否转为新 collation 的排序键。转换后比较必须使用 `charset::CollationBin`，`buildFromScalarFunc` 在 AND/OR 分支显式遵守这一约束。

## 依赖与调用关系

直接上游有两类。`pkg/util/ranger/ranger.rs` 导入 `builder`、`getFullRange`、`point`，在 `buildColumnRange` 中把 access conditions 构造成端点，并在 `points2TableRanges`/`points2Ranges` 中物化；该文件也消费 `FullIntRange`、`FullRange`、`FullNotNullRange` 处理全范围和内存回退。`pkg/util/ranger/detacher.rs` 导入 `builder`、`point`，在 `ExtractEqAndInCondition` 中对同一列的多个 EQ/IN 条件求交，之后还能把点恢复成简化表达式。

主要下游依赖包括：`expression::Expression`/`ScalarFunction`/`Constant`/`Column` 的类型识别和常量求值；`types::Datum`、`FieldType` 与转换/比较 API；`collate::Collator` 和 sort-key API；`rangerctx::RangerContext` 提供表达式求值上下文和类型上下文；`ranger_impl::convertPointInPlace`、`cutPrefixForPoints` 完成跨文件的类型转换与前缀修正；`plannererrors::ErrUnsupportedType` 表示 IN 非常量和 NOT LIKE 等不支持情况。

RustCodeGraph 将 `points.rs` 识别为 39 个符号，并显示文件被包括 `pkg/planner/cardinality/selectivity.rs`、`pkg/planner/core/operator/logicalop/logical_datasource.rs`、`pkg/types/datum.rs` 在内的文件关联使用；但针对 Rust 方法 `buildFromScalarFunc` 及公开工厂的 callers/callees 图查询返回空集合。因此本节的精确调用边以 `ranger.rs`、`detacher.rs` 的显式 import 和调用点为准，不把图的文件级 “used by” 误写成直接函数调用。

## 错误处理与边界

`rangePointCmp` 和类型/sort-key 转换返回 `Result`；归并失败时 `mergeSorted` 写入 `builder.err` 并返回空向量。`buildFromConstant` 的求值或布尔转换失败也写入 `err`。`buildFromIn` 遇到非常量或不能求值的列表元素会设置 `ErrUnsupportedType` 并保守返回全范围；单个非法 ENUM/YEAR 值则跳过。

`buildFromBinOp` 有一个经独立 Rust 测试固定的细节：比较式常量求值失败时直接返回空端点，但不设置 `builder.err`，且列在左、右两种排列都如此（`points_test.rs::binary_comparison_eval_error_does_not_set_builder_error`）。这与其他构造分支的错误通道不同，修改时不能顺手统一。

为避免漏行，无法精确构造的情况通常退回全范围或非 NULL 全范围：未知表达式返回全范围；不兼容 collation 的 LIKE 返回全范围；非二进制 collation 且不生成 sort key 的通配符 LIKE 返回非 NULL 全范围；尚未支持的 NOT 子形式返回全范围。`NOT LIKE` 还会设置不支持错误。这里“保守”意味着可能多扫并由残留条件过滤，不能改成空范围。

unsigned 列与负常量比较时，GT/GE/NE 可归约为从 0 开始，其余比较为空。signed 整数或 float 遇到超上/下界时，某些方向为空，另一些方向夹到最大/最小值并调整操作符。YEAR 转换若发生范围修正，GT/LT 可能相应变为 GE/LE；EQ/NE 保留转换错误，由调用分支决定空范围或非 NULL 全范围。

本实现对若干结构前提使用 `unwrap`/`expect`：LIKE 假定 pattern/escape 已由上游检查为常量，UnaryNot 假定参数为 scalar function，列类型假定已解析。安全扩展应先确认 `checker.rs`/`detacher.rs` 仍保证这些前置条件，否则需要把 panic 路径改成可传播错误并与 Go 行为核对。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或全局可变状态。`builder` 持有一个借用的 `RangerContext`，生命周期受单次 range 构建调用约束；端点 `Vec<point>` 拥有 Datum 值并在函数间按值移动或显式 clone。

主要资源成本来自端点向量分配、Datum clone、字符串 sort key 分配、ENUM 全枚举以及 `merge` 为遍历创建的快照。`buildFromIn` 预留 `2 * 常量数`，`handleEnumFromBinOp` 预留 `2 * enum 元素数`，归并预留两输入长度之和。范围数量的内存上限并不在本文件执行，而由 `ranger.rs` 在端点物化阶段估算并回退到全范围。

collator 以 trait object 借用传递；`RangerContext` 的表达式与类型上下文也只借用或 clone 轻量上下文句柄。因为 `builder.err` 和端点向量会被原地修改，同一个 builder 不应被并发共享；当前调用路径是在单次规划构造中顺序使用。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/util/ranger/points.go`。Rust 文件保留了 Go 的主要符号划分与流程：`point`、`builder`、全范围工厂、二元比较/IN/LIKE/NOT 构造、端点比较和扫描线交并均可逐名对应；`RangeType` 常量也保持 Go `iota` 的 0/1/2 值。

语言层差异主要是所有权表达，而非算法简化：Go 使用 `[]*point`，Rust 使用 `Vec<point>` 并在需要保留值时 clone；Go 的 `builder.err error` 对应 `Option<errors::Error>`；Go 在 `buildFromBinOp` 内的 `refineValueAndOp` 闭包被 Rust 提升为文件级函数；Go 的 comparator 返回 `(int,error)`，Rust 排序闭包把错误记录到 builder 后暂以 Equal 继续，随后由上层检查 `err`。

Go 测试 `ranger_test.go` 提供广泛的行为基线，包括 `TestTableRange` 的左右操作数与 NE/大小比较、`TestIndexRangeForUnsignedAndOverflow`、`TestIndexRangeForYear`、`TestPrefixIndexRangeScan` 和 `TestBinCollationRangeForIndex`。Rust 对应测试分散在独立文件：`points_test.rs` 固定二元比较求值失败的错误状态，`ranger_test.rs::test_index_range_for_unsigned_and_overflow` 等覆盖全范围和部分构造链。目前不能仅凭 Rust 测试数量宣称 Go 的所有边界都已有一一回归覆盖。

## 扩展指南

新增一种可构造谓词时，先在上游 checker/detacher 确认它能被安全识别为单列 access condition，再在 `buildFromScalarFunc` 增加分派，并用独立的 `points_test.rs` 或 `ranger_test.rs` 覆盖端点与最终 Range；不要把测试嵌入本源文件。若谓词存在 NOT 形式，还需同步检查 `buildFromNot`、NULL 三值逻辑和前缀索引处理顺序。

修改端点排序或交并逻辑时，应同时审查 `rangePointEqualValueCmp`、`mergeSorted`、`merge`，并覆盖同值端点的四种 start/end 与 excl 组合、相接区间、空交集和嵌套区间。排序规则是扫描线正确性的基础，局部改变 comparator 会影响 AND、OR 和 IN 去重。

扩展类型支持时，优先接入 `refineValueAndOp`、`handleUnsignedCol`、`handleBoundCol` 或独立的类型专用函数，而不是在每个操作符分支复制逻辑。必须与 `points.go` 同步核对 ENUM、YEAR、unsigned、overflow、NULL 和 collation 行为；新增字符串路径还要验证 binary/non-binary、PAD SPACE、尾随空格、无固定 LIKE 前缀及 sort-key 全字节溢出。

改变前缀裁剪或 sort-key 时，重点保持两个顺序约束：普通比较/IN 在产生点后裁剪和转换；`NOT IN` 必须先取完整值补集再裁剪/转换。相关回归应放在独立测试文件，并参考 Go 的 `TestPrefixIndexRangeScan` 与 `TestBinCollationRangeForIndex`。

性能方面应避免无界复制或把 ENUM/IN 的点数进一步放大；正确性方面始终允许保守扩大扫描范围，但不能缩窄到漏数据。任何新增错误路径都要决定是写入 `builder.err`、返回空范围还是保守全范围，并用上游残留条件行为证明选择与 Go 一致。

## 验证依据

- 源码全貌：`pkg/util/ranger/points.rs`，核对了 1591 行中的类型、常量、结构体、全部函数/impl 和无条件编译项事实。
- RustCodeGraph：`status` 显示索引包含 11467 文件、307296 节点、1848419 边；`files --filter pkg/util/ranger` 显示本模块 27 个 Go/Rust 文件；`node --file pkg/util/ranger/points.rs` 显示本文件 39 个符号及文件级使用关系；`query` 核对了 `rangePointCmp`、`buildFromScalarFunc`、`FullIntRange` 等 Rust/Go 对应符号。对 Rust 方法/工厂运行 callers/callees 未得到函数边，因此改以直接源码引用验证调用链。
- crate 与模块边界：`pkg/util/ranger/Cargo.toml`、`pkg/util/ranger/lib.rs`；目标包没有 `doc.go`。
- 直接调用证据：`pkg/util/ranger/ranger.rs` 的 `points2TableRanges`、`buildColumnRange`，以及 `pkg/util/ranger/detacher.rs` 的 `ExtractEqAndInCondition`。
- Go 对照：`pkg/util/ranger/points.go`，逐项核对主要符号和构造/交并流程；行为测试参考 `pkg/util/ranger/ranger_test.go` 中的 `TestTableRange`、`TestIndexRangeForUnsignedAndOverflow`、`TestIndexRangeForYear`、`TestPrefixIndexRangeScan`、`TestBinCollationRangeForIndex`。
- Rust 独立测试：`pkg/util/ranger/points_test.rs::binary_comparison_eval_error_does_not_set_builder_error`；补充参考 `pkg/util/ranger/ranger_test.rs::test_index_range_for_unsigned_and_overflow` 及同文件的列/索引范围测试。
- 本任务是只读分析加 Markdown 产出，按任务约束未运行 Cargo。交付结构以任务指定命令验证文档存在且恰有 11 个固定二级标题；人工复核覆盖“为何存在、如何运行、如何安全扩展”，并避免把未获得的图调用边或未运行的测试写成已验证事实。
