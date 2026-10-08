# `pkg/util/chunk/compare.rs`

## 文件定位

本文件是 `astersql-util-chunk` crate 的行值排序语义实现，源码由 [`lib.rs`](./lib.rs) 的私有 `compare_impl` 模块通过 `include!("compare.rs")` 注入，再以 `pub use compare_impl::*` 从 crate 根导出。它位于列式 `Chunk` 存储与上层排序、连接、统计搜索之间：一方面按 `FieldType` 生成“两行两列”的比较函数，另一方面将 `Row` 中的列值与 `Datum` 比较，并为有序 `Chunk` 提供上下界搜索。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，包名为 `astersql-util-chunk`。本文件直接使用 crate 根导入的 `Row`、`Chunk`、`mysql` 与 `types`；其中 MySQL 类型码来自 `astersql-parser-mysql`，`FieldType`、`Datum` 及具体标量比较能力来自 `astersql-types-datum` 等类型 crate。当前文件没有条件编译项，也不定义持久化格式。

## 核心职责

1. `GetCompareFunc` 把列的 `FieldType` 映射为适合该物理表示的 `CompareFunc`，并处理整数有无符号标志及字符串 collation。
2. 所有行对行比较器统一实现 NULL 顺序：两个 NULL 相等，NULL 小于非 NULL；非 NULL 值再交给相应标量类型比较。
3. `Compare` 实现 `Row[colIdx]` 对 `Datum` 的异构入口，包含 `KindNull`、`KindMinNotNull`、`KindMaxValue` 三种边界哨兵。
4. `Chunk::LowerBound` 与 `Chunk::UpperBound` 在已按目标列非递减排序的行批上执行二分查找，供范围定位使用。

返回值沿用 Go 比较约定：负数、零、正数分别表示左值小于、等于、大于右值。当前各内部实现实际规范化为 `-1/0/1`，但调用者应依赖符号而不是具体绝对值。

## 主要符号

- `pub type CompareFunc = Box<dyn Fn(Row, usize, Row, usize) -> i32 + Send + Sync>`：可跨线程传递、共享的拥有型比较闭包。四个参数依次为左右 `Row` 及列下标；行类型必须与选择该闭包时的 `FieldType` 一致。
- `GetCompareFunc(&types::FieldType) -> Option<CompareFunc>`：公开工厂。整数族按 `HasUnsignedFlag` 分流；Float/Double、字符串与 Blob 族、时间、Duration、Decimal、Set/Enum、Bit、JSON、VectorFloat32 和 Null 均有实现；未知类型返回 `None`。
- `ordering_to_i32`、`cmp_i64`、`cmp_u64`、`cmp_f64`：基础序关系适配。`cmp_f64` 显式规定 NaN 小于所有非 NaN，两个 NaN 相等，以匹配 Go `cmp.Compare`。
- `cmpNull`：所有常规行比较器共享的 NULL 前置处理。`cmpNullConst` 则专供 `TypeNull`，无条件返回相等。
- `cmpInt64`、`cmpUint64`、`cmpFloat32`、`cmpFloat64`、`cmpMyDecimal`、`cmpTime`、`cmpDuration`、`cmpNameValue`、`cmpBit`、`cmpJSON`、`cmpVectorFloat32`：各物理类型的私有行比较器。
- `genCmpStringFunc` / `cmpStringWithCollationInfo`：前者把 `FieldType.GetCollate()` 的字符串复制进闭包，后者调用 `types::CompareString` 执行规则相关比较。
- `Compare(Row, usize, &types::Datum) -> i32`：公开的行列对 Datum 比较入口，按 `Datum.Kind()` 读取对应的行表示。
- `Chunk::LowerBound`：返回最小的 `i`，使 `row[i] >= d`，并返回搜索过程中是否遇到相等值。
- `Chunk::UpperBound`：返回最小的 `i`，使 `row[i] > d`。

文件没有模块级常量、结构体、枚举或 trait；唯一 `impl` 是对既有 `Chunk` 添加两个搜索方法。

## 执行流程

行对行比较从 `GetCompareFunc` 开始。工厂读取 `FieldType.GetType()`，对整数额外读取 flag，对字符类型额外捕获 collation，然后返回相应闭包。闭包先对左右行调用 `IsNull`；只要任一侧为 NULL 就进入 `cmpNull`，否则通过 `Row.Get*` 读取与列类型匹配的值并执行类型专属比较。Set/Enum 只比较数值 `Value`，不比较显示名称；Duration 固定以 FSP 参数 `0` 读取后比较底层时长；Bit 将原始字节包装成 `BinaryLiteral`。

`Compare` 不经过 `FieldType`。它先按右侧 `Datum.Kind()` 分派：三种哨兵直接决定顺序，其余分支用相应 `Row.Get*` 与 `Datum.Get*` 取值后比较。字符串比较使用 Datum 自带的 collation；源码注释明确假设列与 Datum 的 collation 相同。Bytes、BinaryLiteral 与 MysqlBit 在此入口统一按字节字典序处理。未识别 Kind 的兜底分支返回 `0`。

`LowerBound` 先比较末行：若末行仍小于目标，立即返回 `(NumRows(), false)`。否则在 `[0, NumRows())` 上二分；中点值大于等于目标时收缩右边界，小于时推进左边界，遇到相等值便记录 `found_match`。因此重复值 `[1,2,2,2,5]` 对目标 `2` 返回 `(1,true)`。`UpperBound` 使用相同区间，但只有中点严格大于目标才收缩右界，故同一示例返回 `4`。

## 数据与状态

比较过程本身不修改 `Row`、`Chunk` 或 `Datum`。`CompareFunc` 拥有闭包对象；只有字符串比较闭包保存一份 collation `String`，其余闭包无捕获。每次调用按值接收 `Row`，当前 `Row` 是对既有 Chunk 数据的轻量行视图语义，实际值仍由 `Get*` 访问器读取。

二分搜索只维护 `index`、`hi`，以及 `LowerBound` 的 `found_match`。它不缓存搜索结果，也不验证列是否有序。正确性不变量是：目标列必须按与 `Compare` 完全相同的比较规则非递减排列；行列的物理类型必须和 `Datum.Kind()` 或 `FieldType` 对应；列下标必须有效。

## 依赖与调用关系

下游依赖集中在三类接口：`Row.IsNull/Get*` 与 `Chunk.NumRows/GetRow` 提供存储访问；`mysql` 提供类型码和 unsigned flag；`types` 提供字符串、Decimal、Time、BinaryLiteral、JSON、Vector 等比较语义。`lib.rs` 负责把这些名称带入 `include!` 模块，并把公开符号重新导出。

RustCodeGraph 对 `compare.rs` 的文件节点显示它被规划器的 `physical_index_join.rs`、`physical_merge_join.rs` 及两个 codec 测试文件关联使用。精确源码检索进一步确认：

- `physical_merge_join.rs` 在构造 `PhysicalMergeJoin.CompareFuncs` 时，按每个左连接键的 `RetType` 调用 `ranger::chunk::GetCompareFunc`，再转为 `Arc`；不支持的键类型会触发明确的 `expect`。
- `physical_index_join.rs::ColWithCmpFuncManager::rebuild_compare_funcs` 为受影响列建立比较器，`CompareRow` 按列顺序执行，首个非零结果即为字典序结果。
- `ranger::chunk` 经 `expression::chunk::*` 间接再导出本 crate 的 API，因此上述规划器调用最终落到本文件。
- 当前 Rust 生产源码检索未发现 `Compare`、`Chunk::LowerBound` 或 `Chunk::UpperBound` 的直接业务调用；三者已有独立 Rust 测试覆盖。Go 版本则在 `pkg/statistics/histogram.go` 直接使用这三者处理直方图边界，说明 Rust 对应统计链尚不能仅凭本文件认定为已接线。

## 错误处理与边界

本文件没有 `Result` 返回和显式错误传播。未知 `FieldType` 由 `GetCompareFunc` 返回 `None`，调用者必须拒绝该类型或提供替代语义；Merge Join 当前选择显式 panic，Index Join 的 `filter_map` 则会跳过无法构造比较器的列，这一差异是上游策略而非本文件行为。

调用契约被违反时，底层 `Get*`/`GetRow` 可能因列下标、行下标或物理类型不匹配而 panic。尤其 `LowerBound` 在二分前无条件读取 `NumRows() - 1`，因此要求 Chunk 至少有一行；空 Chunk 会发生 `usize` 下溢或越界。`UpperBound` 对空 Chunk 会自然返回 `0`。

`Compare` 对未知 Datum Kind 返回相等而不是报错，新增 Kind 时若忘记扩充分支，可能静默破坏排序。普通数值分支也不统一先判断行值 NULL；调用者必须保证 Datum Kind 与行列值/哨兵语义匹配。字符串分支依赖“列与 Datum collation 相同”的显式假设。二分方法不检查排序性，未排序输入的结果没有意义。

## 并发与资源生命周期

代码不创建线程、任务、锁、通道、事务或 I/O 资源。比较函数要求 `Send + Sync`，因此无捕获比较器以及仅捕获拥有型 collation 字符串的比较器可被上层安全放入并发结构；例如 Merge Join 将其转换为 `Arc`。本文件不提供可变共享状态，单次比较和搜索只使用栈上局部变量。

资源生命周期主要是闭包所有权：`GetCompareFunc` 每次调用分配一个 `Box`；字符串分支还复制并持有 collation，直到闭包释放。热路径若重复构建比较器会产生分配开销，上层应像当前规划器结构一样按列缓存并复用。`Row` 的底层数据有效期仍受拥有它的 Chunk 约束，本文件不延长或管理该存储生命周期。

## 与 Go 版本的对应关系

直接对照文件是 [`compare.go`](./compare.go)。Rust 保留了 Go 的 API 分层、类型分派、NULL/NaN 顺序、Set/Enum 按 Value 比较、Datum Kind switch，以及 `sort.Search` 的上下界定义。Rust 用 `Box<dyn Fn + Send + Sync>` 代替 Go 函数值，以 `Option<CompareFunc>` 表达 Go 的 `nil`，并把 `sort.Search` 闭包展开为显式 `while` 二分循环。

已核对的细节差异包括：Rust 下标为 `usize`，Go 为 `int`；Rust 字符串闭包拥有 collation 副本；Rust 字节比较通过 slice 的 `cmp` 再转成 `i32`；Rust Decimal 分支因底层返回类型进行了 `as i32`。这些改变没有意图改变顺序语义。两版 `LowerBound` 都先读取末行，因而共享非空前置条件；两版 `Compare` 都对未知 Kind 返回 `0`。

Go `pkg/util/chunk/chunk_test.go` 的比较测试覆盖完整类型集合、NULL 与小/大值的七种相对关系，并在 Chunk 复制后验证逐列相等。Rust 当前最近的独立测试 `codec_2_aster_unit_test.rs::comparison_handles_null_nan_and_binary_search_like_go` 聚焦 NaN、NULL 和重复整数的上下界，覆盖面小于 Go 测试；扩展类型或修改比较语义时应保持两边测试意图同步，而不是仅满足现有 Rust 样例。

## 扩展指南

新增 MySQL 列类型时，首先在 `GetCompareFunc` 增加准确的 `FieldType` 分支和专用比较器；如果该值也可作为 Datum 搜索键，还必须同步扩展 `Compare` 的 Kind 分支。比较器应沿用“先 NULL、后具体值”的模板，并验证所用 `Row.Get*` 与列物理布局一致。新增字符串或复合类型时还要明确 collation、NaN/哨兵、名称与数值、字节与逻辑值之间的排序约定。

修改 `Compare` 后必须重新审查 `LowerBound`/`UpperBound` 的单调性前提及 Go `pkg/statistics/histogram.go` 的边界语义。若要支持空 Chunk，最小安全接入点是 `LowerBound` 的末行快速路径之前，并应添加空输入回归测试；这属于行为修改，不应只靠文档假设。若把未知 Kind 的兜底改为错误，需要重新设计公开签名并迁移所有搜索调用者，兼容风险较高。

测试应放在独立文件而非 `compare.rs` 内：Rust 优先扩展 `pkg/util/chunk/codec_2_aster_unit_test.rs` 或新建由 `lib.rs` 在 `#[cfg(test)]` 下挂接的独立测试文件；Go 对照更新 `pkg/util/chunk/chunk_test.go`。至少覆盖所有支持类型、signed/unsigned、不同 collation、左右 NULL、NaN、特殊 Datum 哨兵、重复元素、目标位于首尾之外，以及空 Chunk 契约。性能评估应关注每列闭包分配、字符串 collation 捕获和二分过程中 `GetRow/Get*` 的调用次数。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/util/chunk` 确认目标与同目录测试；`node --file pkg/util/chunk/compare.rs --offset 1 --limit 260` 及 `--offset 239 --limit 180` 覆盖全部 350 行并给出关联使用文件；`query` 分别确认 Rust/Go 的 `GetCompareFunc`、`LowerBound`、`UpperBound` 定义。精确 `callers/callees` 查询在 30 秒内未返回，故用下述局部源码检索补证。
- 已读生产源码与配置：`pkg/util/chunk/compare.rs`、`pkg/util/chunk/lib.rs`、`pkg/util/chunk/Cargo.toml`、`pkg/util/ranger/lib.rs`、`pkg/planner/core/operator/physicalop/physical_merge_join.rs`、`physical_index_join.rs` 及该 crate 的 `Cargo.toml`。
- 已读对照与测试：`pkg/util/chunk/compare.go`、`pkg/util/chunk/chunk_test.go`、`pkg/util/chunk/codec_2_aster_unit_test.rs`；并检索 `pkg/statistics/histogram.go` 以确认 Go 上层边界搜索用法。
- 源码检索证据：Rust 生产调用点目前集中在两个物理 Join 文件的 `GetCompareFunc`；Rust 独立测试对 `[1,2,2,2,5]` 验证下界 `(1,true)`、上界 `4`，并验证 NaN 和 NULL 顺序。未运行 Cargo，符合本纯文档任务约束。
