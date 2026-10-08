# `pkg/util/ranger/ranger.rs`

## 文件定位

本文件是 `astersql-util-ranger` crate 中把 SQL 谓词端点落实为可供表扫描、索引扫描和代价估算使用的 `Range`/`Ranges` 的核心实现。crate 根 `pkg/util/ranger/lib.rs` 以 `ranger_impl` 装入本文件并公开再导出其导出项；`pkg/util/ranger/Cargo.toml` 的 `package.metadata.porting.go-package` 指向 `pkg/util/ranger`，说明其直接移植目标是同目录 Go 包，具体行为基线为 `pkg/util/ranger/ranger.go`。

它位于“表达式条件拆分”与“规划器访问路径”之间：`points.rs` 的 `builder` 先把表达式变为成对端点，本文件完成类型转换、有效性过滤、单列/多列组合、内存配额回退和区间合并；规划器随后把结果写入逻辑或物理扫描。直接生产调用可见于 `pkg/planner/core/operator/logicalop/logical_table_scan.rs`、`logical_datasource.rs`、`pkg/planner/core/operator/physicalop/physical_table_scan.rs`、`physical_index_join.rs`、`index_join_probe.rs`，估算侧调用见 `pkg/planner/cardinality/selectivity.rs`、`cross_estimation.rs` 和 `row_count_index.rs`。

## 核心职责

1. **端点标准化与区间有效性**：`convertPointInPlace` 按目标 `FieldType` 转换 `Datum` 并修正开闭边界，`convertPointsInPlace` 成对处理端点、过滤空区间，`validInterval` 以 KV 编码后的字节序判断区间是否仍非空。
2. **构造扫描范围**：`points2Ranges` 构造普通索引/列范围，`points2TableRanges` 构造整数 handle 表范围；`BuildColumnRange` 与 `BuildTableRange` 是对应公开入口。
3. **构造多列索引范围**：`appendPoints2Ranges`、`appendPoints2IndexRange` 和 `AppendRanges2PointRanges` 只在已有前缀为点范围时追加下一列，并形成笛卡尔积；`rangeDetacher::buildRangeOnColsByCNFCond`、`buildCNFIndexRange` 把它们接入 CNF 条件拆分流程。
4. **控制范围膨胀**：三组 `estimateMemUsage*` 在分配前估算结果内存；超过 `rangeMaxSize` 时返回较宽范围，并把未使用谓词保留为 remained conditions，不能把回退误解为丢弃过滤语义。
5. **区间规范化与前缀索引处理**：`UnionRanges` 对编码边界排序并合并重叠/相邻范围；`cutPrefixForPoints`、`CutDatumByPrefixLen`、`ReachPrefixLen` 处理前缀长度及开闭边界。
6. **辅助表达与诊断**：`points2EqOrInCond` 从点集合恢复 `EQ`/`IN`/`IS NULL` 条件，`RangesToString` 和 `RangeSingleColToString` 把范围恢复为 SQL 条件文本。

## 主要符号

- `validInterval(ec, loc, low, high) -> Result<bool, Error>`：分别编码左右端点；低端开区间和高端闭区间通过 `kv::Key::PrefixNext` 转成半开 KV 边界，最后要求编码后的左边界严格小于右边界。
- `convertPointInPlace(sctx, p, newTp)`：调用 `Datum::ConvertTo`；转换失败时设置跳过 plan cache 的原因，并仅对 Year、Bit、数值溢出、非法时间到 Timestamp、Enum 截断等 Go 已定义场景采用容忍/钳制策略。转换值相对原值发生移动时，按起点/终点和开/闭属性调整 `point.excl`。
- `convertPointsInPlace(...)`：以两个 `point` 为一个区间；表范围将 `NULL`/`MinNotNull`/`MaxValue` 映射为有符号或无符号整数边界，`skipNull` 会丢弃上界为 `NULL` 的区间，并将有效端点压紧到返回向量前部。
- `points2Ranges`、`points2TableRanges`：从端点对创建 `Range`。前者在 NOT NULL 类型回退到 `FullNotNullRange`，否则回退到 `FullRange`；后者回退到 `FullIntRange(unsigned)`。
- `AppendRanges2PointRanges(pointRanges, ranges, rangeMaxSize)`：公开的多列范围拼接入口。追加范围的开闭属性来自后缀 `ranges`，collator 列表按前缀、后缀顺序拼接；超配额时原样返回 `pointRanges` 并令布尔值为 `true`。
- `BuildTableRange`、`BuildColumnRange`：公开构造入口，返回 `(Ranges, used conditions, remained conditions)`；`BuildColumnRange` 对空条件直接返回 `FullRange`。
- `rangeDetacher::buildRangeOnColsByCNFCond`：先逐列构造等值/IN 前缀，再把第一个非等值列追加为范围后缀；一旦发生内存回退，准确切分已使用与剩余谓词。
- `rangeDetacher::buildCNFIndexRange`：完成上述构造后，如果任一索引列是前缀索引，则调用 `UnionRanges` 消除裁剪导致的重叠。
- `sortRange`、`UnionRanges`：内部排序对象保存原始 `Range` 与编码后的左右边界；`mergeConsecutive=true` 时 `end >= next.start` 即合并，否则仅 `end > next.start` 的实际重叠会合并。
- `CutDatumByPrefixLen`、`ReachPrefixLen`：binary/ASCII 以字节计数，其他字符集以 Unicode 字符计数。Rust 对 binary 字符串使用 `SetBytesAsString`，允许裁剪点落在 UTF-8 码点中间但仍保持 String Datum 语义。
- `newFieldType`：整数统一放宽到 `TypeLonglong`；浮点和字符串族取消声明长度限制，避免常量在构造范围阶段过早溢出或截断。
- `RangesToString`、`RangeSingleColToString`：支持点范围、普通区间、`NULL`、`MinNotNull`、`MaxValue`；多列范围只允许最后一列是非点区间，列间以 `AND`、范围间以 `OR` 连接。

本文件没有模块级常量、trait 或条件编译项；条件测试装配位于 `lib.rs`，状态载体 `Range`、`Ranges`、`point` 与 `rangeDetacher` 分别定义在同 crate 的 `types.rs`、`points.rs` 和 `detacher.rs`。

## 执行流程

表/普通列入口的主流程如下：

1. `BuildTableRange` 或 `BuildColumnRange` 进入 `buildColumnRange`；空列条件在公开入口直接返回全范围。
2. 为每个 access condition 调用 `points_impl::builder::build`，再通过 `builder::intersection` 累积端点交集；builder 记录的错误会立即向上传播。
3. `newFieldType` 放宽字段类型；字符串端点在完成表达式比较后由 `convertStringFTToBinaryCollate` 切到 binary collation，以编码键顺序构造最终范围。
4. 表路径调用 `points2TableRanges`，普通列路径调用 `points2Ranges`。两者先转换端点、过滤无效区间，再检查预计内存。
5. 若超配额，`RangerContext::RecordRangeFallback` 记录回退，返回全范围、空 used conditions 和完整 remained conditions；否则返回精确范围和完整 used conditions。指定前缀长度的普通列结果还会执行 `UnionRanges(..., true)`。

多列 CNF 索引路径由 `rangeDetacher::buildRangeOnColsByCNFCond` 驱动：第 1 个等值/IN 列经 `points2Ranges` 建立点范围，后续等值/IN 列经 `appendPoints2Ranges` 扩展；第一个非等值列的所有条件先求交集，再作为最后一列追加。非点范围不会继续追加后续列，这是 B-Tree 联合索引“等值前缀后最多接一段范围”的约束。最终 `buildCNFIndexRange` 只在前缀索引存在时额外合并范围。

区间合并流程由 `UnionRanges` 完成：对 LowVal/HighVal 编码，依据开闭区间调整成实际 KV 扫描边界，按 `encodedStart` 排序，然后线性扫描。发生覆盖时仅在新右端更远时替换原始 HighVal/HighExclude；不重叠时提交上一段。这使排序依据与存储键顺序一致，同时保留可供上层解释的原始 Datum。

## 数据与状态

- `Range` 持有 `LowVal`、`HighVal`、`LowExclude`、`HighExclude` 和逐列 `Collators`；`Ranges` 是其集合。多列范围中，前缀列必须为点，开闭属性只属于最后一列。
- `point` 保存单个端点的 `Datum`、是否起点和是否排除；调用方必须向端点转范围函数传入偶数个、按低高成对排列的端点。本文件多处使用 `step_by(2)` 和 `j + 1`，该配对是内部前置不变量。
- `RangerContext` 提供类型/时区/错误上下文，并保存 plan-cache 跳过原因及 range fallback 记录。文件本身不保存全局可变状态。
- `rangeMaxSize == 0` 表示不限制；正数限制的是构造结果的估算内存。估算包含 `Range` 固定开销、Datum 内存和 collator 槽位，但不是运行时分配器的精确统计。
- 构造函数预分配 `Vec` 容量及临时 Datum/Range 缓冲区，随后为每个结果生成独立 Vec。Rust 版本用克隆保持结果间所有权隔离；`ranger_test.rs::test_range_clone_does_not_share_collators` 与 Go 回归用例共同约束“修改一个范围不得污染相邻范围”。
- `sortRange` 只活跃于一次 `UnionRanges` 调用；编码缓冲区随函数返回释放，不会缓存到 context。

## 依赖与调用关系

上游主要分为三类：

- 扫描路径构造：`logical_table_scan.rs`、`logical_datasource.rs`、`physical_table_scan.rs` 调用 `BuildTableRange`；`logical_datasource.rs`、`physical_index_join.rs` 调用 `BuildColumnRange`；`index_join_probe.rs` 调用 `AppendRanges2PointRanges`。
- 条件拆分：同 crate 的 `detacher.rs` 调用 `AppendRanges2PointRanges` 和 `UnionRanges`，而本文件为 `rangeDetacher` 补充 CNF range 构造方法。
- 估算与规范化：`selectivity.rs`、`cross_estimation.rs` 调用 `BuildColumnRange`，`row_count_index.rs` 和 `logical_datasource.rs` 调用 `UnionRanges`。

下游依赖来自 `Cargo.toml`：`astersql-expression` 提供表达式、Datum、类型、collator 和 codec 门面；`astersql-kv` 提供 `Key::PrefixNext`；`astersql-util-ranger-context` 提供错误、时区和 fallback 状态；`astersql-sessionctx-stmtctx`、`astersql-types-parser_driver` 与 `astersql-parser-format` 支持 SQL 字符串恢复；`regex` 仅用于把只含括号的 `true` 结果简化。crate 还声明 planner/model/fixcontrol 等依赖，但本文件的直接路径主要经 crate 根再导出的上述模块使用。

关键内部调用边是：`Build*Range -> buildColumnRange -> builder::{build, intersection} -> points2*Ranges -> convertPointsInPlace -> convertPointInPlace/validInterval`；多列路径是 `rangeDetacher::build* -> points2Ranges/appendPoints2Ranges -> appendPoints2IndexRange`；规范化路径是 `buildColumnRange` 或 `buildCNFIndexRange -> UnionRanges -> codec::EncodeKey/kv::Key::PrefixNext`。

## 错误处理与边界

- `codec::EncodeKey`、Datum 转换/比较、表达式构造和 SQL Restore 的错误统一包装为 `errors::Error` 向上传播；`validInterval` 先交由 `ErrCtx::HandleError` 判定可忽略还是返回。
- `convertPointInPlace` 的容错名单来自 Go 语义，并非所有转换错误都忽略。未列入名单的错误立即返回；任何转换错误都会令 plan cache 被标记为不安全。非法字符字符串被跳过转换，让后续区间过滤决定有效性。
- `RangeSingleColToString` 对特殊边界产生 `IS NULL`、`IS NOT NULL`、`true` 或 `false`；普通相等闭区间输出 `=`，其余输出上下界组合。
- `RangesToString` 显式拒绝 LowVal/HighVal 列数不一致，以及除最后一列外出现非等值前缀的范围。它按 `colNames[j]` 和 `Collators[j]` 索引，因此调用方仍必须保证列名、collator 数量与范围宽度一致；本函数没有额外长度检查。
- 空 `Ranges` 传给 `UnionRanges` 返回空集合；空普通列条件返回全范围；空追加后缀返回原点范围且不报告 fallback。
- 前缀索引裁剪后，实际发生裁剪或起点正好达到前缀长度时必须把端点改为闭区间，否则 `col > 'xx'` 在长度 2 的前缀索引上会漏掉 `'xxx'`。
- 内存回退是保守扩大扫描范围，不是成功构造精确范围；上层必须使用返回的 remained conditions 继续过滤。任务测试 `test_range_fallback_for_build_table_range`、`test_range_fallback_for_build_column_range` 和 Go 同名测试覆盖此契约。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、通道、事务或外部 I/O。所有范围和编码缓冲区均是调用栈内拥有的 `Vec`，结果通过所有权返回；共享输入 context 仅在调用期间借用。`Build*Range` 和 detacher 路径需要可变 `RangerContext`，因为会记录错误、plan-cache 跳过原因和 fallback；`UnionRanges` 只读借用 context。

资源风险主要来自组合爆炸：多列 IN/点范围拼接会产生 `前缀数 × 后缀数` 个 Range。实现先调用 `estimateMemUsageForPoints2Ranges`、`estimateMemUsageForAppendPoints2Ranges` 或 `estimateMemUsageForAppendRanges2PointRanges`，超限即在大规模分配前回退。新代码若绕过这些入口或改变 `Range` 内存布局，必须同步校准估算公式并验证相邻结果的 Vec/collator 不共享可变后备存储。

## 与 Go 版本的对应关系

Rust 文件按函数结构直接对应 `pkg/util/ranger/ranger.go`：从 `validInterval`、端点转换、三类内存估算与 fanout，到 `BuildTableRange`/`BuildColumnRange`、`rangeDetacher` 方法、`UnionRanges`、前缀裁剪以及 SQL 字符串化，名称和分支顺序基本一致。`Cargo.toml` 的 porting 元数据也明确指向该 Go 包。

需要注意的语言表达差异：

- Go 使用 `[]*point`、`[]*Range` 和三下标切片限制容量；Rust 使用值语义的 `Vec<point>`/`Vec<Range>`、克隆和独立结果 Vec 保证追加时不会覆盖邻居。
- Go 的 `ConvertTo` 可随错误返回钳制后的 Datum；Rust 在可容忍错误后用忽略截断标志再次转换以恢复等价边界，并对 Enum 上溢补到最大枚举值。
- Go 字符串是任意字节序列；Rust binary/ASCII 前缀裁剪可能落在 UTF-8 中间，因此用 `SetBytesAsString` 保存原始字节而不是要求有效 UTF-8。`test_cut_binary_string_prefix_inside_utf8_code_point` 专门验证这一移植差异保持了 Go 行为。
- Go 的 `UnionRanges` 复用原 slice 容量；Rust 新建 `Ranges`。两者均按编码起点排序，并以相同的 `mergeConsecutive` 比较规则合并。
- Go 的核心回归覆盖更广，包括真实 session/planner 产生的表达式和精确 access/remained 字符串；Rust 独立测试覆盖主路径、配额回退、前缀、并集和所有权隔离，但不能据此声称所有 Go 用例已经逐项移植。

## 扩展指南

- 新增类型转换规则时修改 `convertPointInPlace`，必须保持“转换值移动方向决定 excl”的四类分支，并在独立的 `pkg/util/ranger/ranger_test.rs` 增加溢出、截断、无效时间或排序规则边界测试；不要把测试内嵌进 `ranger.rs`。
- 新增 Range 构造入口应优先复用 `builder`、`convertPointsInPlace` 和既有配额估算；若产生新的组合维度，需先给出可保守覆盖结果内存的估算，并定义 fallback 时 used/remained conditions 的切分。
- 修改多列 fanout 时重点检查：只有点前缀可追加、最终开闭属性来自最后一列、collator 顺序与 Datum 顺序一致、结果之间无可变存储别名。应同步 Rust 的 `test_index_range`、所有权隔离测试以及 Go `ranger_test.go` 对应长 IN/配额用例。
- 修改 `UnionRanges` 时同时验证 `mergeConsecutive=true/false`、开闭端点、包含关系、相同起点和编码错误；规划器分区路径依赖 `false` 不合并不同相邻点，见 `test_partition_path_does_not_merge_distinct_points`。
- 修改前缀裁剪时同时覆盖 binary、ASCII、多字节字符、正好达到前缀长度和起点开区间，并核对 Go `CutDatumByPrefixLen`/`ReachPrefixLen` 的字节与 rune 规则。
- 修改字符串化函数时必须保持 SQL literal 的 `Restore` 路径，避免手工拼接转义值；还要补充列宽/特殊 Datum/非点前缀的错误测试。
- 性能风险集中在长 IN 与多列笛卡尔积；兼容风险集中在 Datum 转换、collation/编码顺序、NULL/无穷边界和前缀索引。任何行为调整都应先与 `ranger.go` 的对应增量核对，不能为通过 Rust 测试删减 Go 分支。

## 验证依据

- RustCodeGraph：`status` 确认索引包含目标文件；`files --filter pkg/util/ranger` 确认 Rust/Go 源与独立测试；`node --file pkg/util/ranger/ranger.rs` 阅读完整 1332 行实现；`query` 核对 `BuildTableRange`、`BuildColumnRange`、`AppendRanges2PointRanges`、`UnionRanges`、`CutDatumByPrefixLen`、`RangesToString` 等 Rust/Go 对应符号。图查询还报告本文件被 10 个规划器文件使用；`callers/callees` 未返回文本，因此调用点以精确符号搜索补齐。
- 已读生产与装配文件：`pkg/util/ranger/ranger.rs`、`pkg/util/ranger/Cargo.toml`、`pkg/util/ranger/lib.rs`、`pkg/util/ranger/ranger.go`；目标包无 `doc.go`。
- 已读测试：`pkg/util/ranger/ranger_test.rs`；并核对 `pkg/util/ranger/ranger_test.go` 中 `TestRangeFallbackForBuildTableRange`、`TestRangeFallbackForBuildColumnRange` 及相关配额/前缀行为。Rust 测试还直接覆盖表/列范围、多列范围、前缀索引、并集、空条件、分区不合并、Clone 隔离和 binary 字节裁剪。
- 精确调用证据：规划器文件对 `BuildTableRange`、`BuildColumnRange`、`AppendRanges2PointRanges`、`UnionRanges` 的调用，以及同 crate `detacher.rs` 对追加和并集函数的调用，均通过限定符号搜索核对。
- 本任务只生成文档，按计划不运行 Cargo。交付结构检查要求文档存在且恰有 11 个固定二级标题；其命令与退出状态在任务交付时记录。
