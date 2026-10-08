# `pkg/util/intset/fast_int_set.rs`

## 文件定位

本文件实现 `astersql-util-intset` crate 的核心整数集合 `FastIntSet`。crate 边界由 [`pkg/util/intset/Cargo.toml`](Cargo.toml) 定义，入口 [`pkg/util/intset/lib.rs`](lib.rs) 通过 `pub mod fast_int_set` 声明模块并用 `pub use fast_int_set::*` 扁平重导出这里的公开 API；该 crate 没有声明第三方 Rust 依赖，生产实现只使用标准库的 `BTreeSet`、格式化和错误 trait。

它位于通用工具层，不承载 SQL 语义，而为上层保存小型或稀疏的 `i32` 标识集合。实际接线包括：[`pkg/expression/util.rs`](../../expression/util.rs) 收集表达式中的列 `UniqueID`；[`pkg/planner/funcdep/fd_graph.rs`](../../planner/funcdep/fd_graph.rs) 表示函数依赖的决定列、被决定列、非空列与等价类；逻辑聚合、连接、投影、选择等算子构造列集合；[`pkg/sessionctx/stmtctx/stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs) 保存更新计划引用的列。因此它处在“表达式/逻辑计划提取标识 -> 集合运算 -> 规划性质或语句状态判断”的基础数据结构位置，而不是请求入口或存储访问层。

## 核心职责

- 对 `[0, 64)` 的整数用一个 `u64` 位图完成无分配的插入、删除、包含判断及部分集合运算；阈值由 `smallCutOff = 64` 固定。
- 出现负数或 `>= 64` 的整数时，把现有小值展开到有序的 `BTreeSet<i32>`，由 `large` 保存全集，同时继续维护 `small` 作为前 64 个非负值的缓存。
- 提供确定性的升序遍历、复制、相等/子集/相交判断、差并交、整体平移、闭区间加入与压缩字符串表示。
- 保持 Go [`pkg/util/intset/fast_int_set.go`](fast_int_set.go) 的公开方法命名、双表示状态迁移和主要边界语义，供 Rust 迁移代码按相同调用模型使用。

核心不变量是：当 `large` 为 `Some` 时，它必须包含集合的全部元素，而 `small` 必须准确反映其中 `[0, 64)` 的成员；当 `large` 为 `None` 时，全集完全由 `small` 表示。`Insert`、`Remove`、`Clear`、`CopyFrom` 和三个原地集合运算共同维护该不变量。

## 主要符号

- `smallCutOff: i32 = 64`：位图表示的右开上界。
- `FastIntSet { small: u64, large: Option<BTreeSet<i32>> }`：公开类型，内部字段私有；派生 `Clone`、`Debug` 和空集合 `Default`。
- `SmallValueError`：`GetSmallUInt64` 无法返回纯位图时的零尺寸错误类型，实现 `Display` 和 `std::error::Error`。
- `NewFastIntSet(Vec<i32>)`：逐项调用 `Insert` 的构造函数。与 Go 的可变参数不同，Rust 接收向量。
- 基础查询和更新：`Len`、`Only1Zero`、`Insert`、`Next`、`Remove`、`Clear`、`Has`、`IsEmpty`。
- 观察和复制：`SortedArray`、`ForEach`、`Copy`、`CopyFrom`、`Equals`、`GetSmallUInt64`。
- 集合运算：返回新集合的 `Difference`、`Union`、`Intersection`，以及修改接收者的 `DifferenceWith`、`UnionWith`、`IntersectionWith`；另有 `Intersects` 和 `SubsetOf`。
- 派生操作：`Shift`、`AddRange`、`String`，以及将 `String` 接到 Rust 格式化体系的 `Display for FastIntSet`。
- 私有辅助：`toLarge` 将当前全集克隆或从位图展开；`largeToSmall` 判断已分配的大表示中是否仍有位图范围外成员。

文件没有 trait 定义、条件编译项、异步函数或 `unsafe` 代码。所有公开方法保留 Go 风格的大写名称，文件级 `allow` 为此关闭相应命名 lint。

## 执行流程

1. `NewFastIntSet` 从 `Default` 空集开始调用 `Insert`。小值只置 `small` 的对应 bit；第一次遇到负数或 `>= 64` 的值时，`toLarge` 枚举已有位图，构造 `BTreeSet`，随后插入越界值。此后每次插入小值会同时更新位图和全集，大值只更新全集。
2. 点查询优先走位图：`Has` 对小值直接按位与；`Len`、`ForEach`、`SortedArray` 在无 `large` 时使用 `count_ones`/`trailing_zeros`，在有 `large` 时以有序树为权威全集。`Next` 若起点小于 64，先把负起点钳为 0 并扫描位图，再从 `large.range(startVal..)` 搜索；无结果返回 `(i32::MAX, false)`。
3. `Remove` 同步清除小值 bit 和 `large` 成员；`Clear` 清零位图并清空已分配的树，但不把 `large` 恢复为 `None`。因此表示状态记录了集合是否曾提升，不能仅由当前成员推断。
4. `Copy` 深克隆两种表示；`CopyFrom` 覆盖 `small`，并尽量用 `clone_from` 或 `clear` 复用接收者已有的树分配。`Equals` 还处理“一侧纯位图、另一侧曾提升但越界成员已删完”的等价情形。
5. 非原地差/并/交先 `Copy` 左操作数，再调用对应 `*With`。原地操作总先做位图按位运算；需要处理全集时，用 `toLarge` 得到一致表示，再用 `retain` 或 `extend` 完成树集合运算。`IntersectionWith` 在右侧为纯位图时可直接丢弃左侧 `large`，因为交集不可能含越界值。
6. `Shift` 对空集直接返回空集；纯位图且移动后仍在 `[0, 64)` 时用整词移位，否则升序遍历并对每个 `value + delta` 重新 `Insert`。`AddRange` 对纯位图内的闭区间一次构造掩码，否则逐值插入。
7. `String` 用 `ForEach` 的升序保证组织输出：负数逐个打印，非负连续段压缩；单点写 `a`，两个连续值写 `a,b`，三个及以上写 `a-b`，整体包在括号内。

## 数据与状态

`small` 的第 `n` 位表示整数 `n`，只覆盖 `0..=63`。纯小集合的主要操作是常数时间位运算且不分配堆内存。`large` 使用 `BTreeSet<i32>`，所以支持负数和所有 `i32` 值，遍历天然升序；树操作通常为对数时间，展开、克隆及集合遍历与元素数量相关。

表示有三种值得区分的状态：`large == None` 的纯位图；`large == Some` 且确有越界成员的全集；`large == Some` 但越界成员已被删除、当前只剩小值或为空的“已提升”状态。最后一种状态仍满足数据不变量，但会影响 `GetSmallUInt64`、内存复用和后续操作路径。`Clear`、`Remove`、`DifferenceWith` 不主动降级；`IntersectionWith` 在右操作数没有大表示时是明确的例外，会把结果降为纯位图。

`FastIntSet` 没有内部版本号、缓存失效标志或共享指针。返回新集合的操作和 `Copy` 都拥有独立的 `BTreeSet`；`SortedArray` 也返回新分配的 `Vec<i32>`。

## 依赖与调用关系

下游依赖仅为 Rust 标准库：`BTreeSet` 负责大表示，`fmt` 负责错误与集合文本，`Error` 使 `SmallValueError` 可作为标准错误使用。内部调用关系中，`NewFastIntSet -> Insert`；`Only1Zero -> Len + Has`；`SortedArray -> Len + ForEach`（纯位图路径）；`Difference/Union/Intersection -> Copy + 对应 *With`；`Shift -> IsEmpty/ForEach/Insert`；`Display::fmt -> String`。`Insert`、集合运算与若干转换共同依赖 `toLarge`。

RustCodeGraph 对目标文件的文件节点报告 34 个符号，并标识 17 个使用文件，代表性生产调用者与仓库搜索一致：

- [`pkg/expression/util.rs`](../../expression/util.rs) 的 `ExtractColumnSet`/内部提取流程把列 ID 插入集合。
- [`pkg/planner/funcdep/fd_graph.rs`](../../planner/funcdep/fd_graph.rs) 大量使用 `NewFastIntSet`、并交差、子集与闭包相关操作，是主要消费者。
- [`pkg/planner/core/operator/logicalop/`](../../planner/core/operator/logicalop/) 下的聚合、连接、投影、选择、展开、`UNION ALL` 等实现构造并传递列集合。
- [`pkg/planner/util/funcdep_misc.rs`](../../planner/util/funcdep_misc.rs) 和 planner property/cascades 代码构造非空列、等价键等集合。
- [`pkg/sessionctx/stmtctx/stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs) 将其作为 statement context 中的列引用状态。

这些消费者通过各自 `Cargo.toml` 中对 `astersql-util-intset` 的路径依赖接入。RustCodeGraph 的精确 `callers/callees` 查询因 `Insert` 等同名方法无法消歧而未返回可用的单符号边，因此上述跨 crate 边以图的文件级 `used by` 结果和局部静态引用共同核验，不推断运行时调用频次。

## 错误处理与边界

- `GetSmallUInt64` 只在 `large.is_none()` 时成功。即使越界成员已全部删除，只要大表示仍存在，它也返回 `SmallValueError`；这是表示状态契约，不是“当前所有成员是否位于 0..63”的动态判断。
- `AddRange(from, to)` 要求 `to >= from`，否则以固定消息 panic。它没有返回 `Result`，调用方必须先保证区间有效。
- `Next` 的语义不是完整的“从任意 `i32` 起点找成员”：当 `startVal < 64` 时会把负数钳到 0，因此不会返回负成员；测试明确称其为“only seek non-neg”。完整枚举应使用 `ForEach` 或 `SortedArray`。
- `Next` 无结果用 `i32::MAX` 作哨兵并令布尔值为 `false`；调用者必须检查布尔值，不能把哨兵当成员。相应地，如果集合实际包含 `i32::MAX`，`BTreeSet::range` 会返回 `(i32::MAX, true)`，仍可由布尔值区分。
- `largeToSmall` 是私有函数且要求 `large` 存在，内部用 `expect("set contains no large")` 保护前置条件；当前调用点先按表示状态分支。
- `Shift` 慢路径直接计算 `value + delta`，没有 `checked_add`；超出 `i32` 可表示范围时的整数溢出行为取决于构建的溢出检查设置，调用者不应传入会越界的组合。
- 重复插入、删除不存在的元素和清空空集都是幂等操作。集合 API 没有 I/O 或可恢复的运行时错误；除上述显式 error/panic 与算术边界外，操作通过返回值表达结果。

## 并发与资源生命周期

本类型不包含锁、原子、通道、任务或外部资源，也没有自定义 `Drop`。只读方法可通过共享引用调用；更新方法要求 `&mut self`，并发访问策略完全由持有者负责。`FastIntSet` 的字段类型本身可在线程间移动/共享，但文件没有建立任何并发协议。

资源生命周期主要是内存分配：纯位图状态不为集合内容分配堆内存；第一次插入越界值时分配并填充 `BTreeSet`。`Clear` 和 `CopyFrom` 的部分分支保留并复用树容器，避免反复创建表示；`IntersectionWith` 可能将 `large` 设为 `None` 并释放该树。`Copy`、`toLarge` 在已有大表示时、`SortedArray` 以及非原地集合运算会产生独立分配，热路径扩展时应留意这些复制成本。

## 与 Go 版本的对应关系

Rust 文件直接对应 [`pkg/util/intset/fast_int_set.go`](fast_int_set.go)。两者共享 `smallCutOff = 64`、位图加大表示的双层设计、首次越界时提升全集、删除越界值后不自动降级、`Next` 跳过负数、集合运算和字符串压缩规则。Rust 的 [`fast_int_set_test.rs`](fast_int_set_test.rs) 与 Go [`fast_int_set_test.go`](fast_int_set_test.go) 也保留了相同的基础、随机往返、双集合、区间、位掩码和字符串测试主题；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 额外点名迁移契约。

实现载体存在有意差异：Go 的大表示是 `golang.org/x/tools/container/intsets.Sparse`，Rust 使用标准库 `BTreeSet<i32>`；Go 整数是平台宽度 `int`，Rust API 固定为 `i32`；Go 构造器是 `values ...int`，Rust 是 `Vec<i32>`；Go `GetSmallUInt64` 返回普通 `error`，Rust 返回具体的 `SmallValueError`。Go 的 `SortedArray` 对空集返回 `nil`，Rust 返回空 `Vec`，在各自语言中都表示零元素结果，但容器表现并非逐字等同。

Go `toLarge` 在已有大表示时返回原指针，Rust `toLarge` 返回拥有所有权的 `BTreeSet`，因此已有大表示会被克隆；Rust 的集合运算据此避免别名，但成本模型不同。Go 使用 `intsets.MaxInt` 哨兵，Rust 使用 `i32::MAX`。这些差异不应被误写为数据结构完全同构，扩展时须同时核对可观察语义和复制/整数范围成本。

## 扩展指南

- 新增基础更新或集合运算时，必须同时维护“`large` 为全集、`small` 为小值缓存”的不变量；尤其不要只改树而遗漏小值 bit，或在提升时遗漏原位图成员。
- 新增遍历 API 前先明确是否需要包含负数。若需要完整集合，复用 `ForEach`/`BTreeSet` 顺序；不要基于 `Next(i32::MIN)`，因为 `Next` 明确从 0 开始搜索。
- 若调整阈值或位图类型，应同步审查 `Insert`、`Next`、`largeToSmall`、`Shift`、`AddRange` 的边界和移位宽度；当前 `u64` 与阈值 64 是成对设计。
- 若希望已提升集合自动降级，需先决定 `GetSmallUInt64`、分配复用以及与 Go 的兼容语义；这不是局部性能优化，而是可观察状态变化。
- 若增加可失败 API，优先返回明确的 Rust 错误类型；不要把新的普通输入错误改成 panic。`AddRange` 的 panic 是既有 Go 兼容契约。
- 行为修改应同步更新独立测试文件 [`fast_int_set_test.rs`](fast_int_set_test.rs)，而不是把测试写进生产源文件；涉及 Go 对齐状态时也更新 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并对照 [`fast_int_set_test.go`](fast_int_set_test.go)。性能路径变化可扩展 [`fast_int_set_bench_test.rs`](fast_int_set_bench_test.rs) 的差集与插入场景。
- 上层接线变化至少复核 expression 列集合提取和 planner 函数依赖/逻辑算子消费者；固定 `i32` ID、复制成本、排序顺序和字符串格式都可能构成兼容或性能风险。

## 验证依据

- RustCodeGraph：`status` 显示索引包含本仓库 Rust/Go 文件；`files --filter pkg/util/intset` 找到目标、Go 对照及独立测试；`node --file pkg/util/intset/fast_int_set.rs --offset 1 --limit 400` 读取目标的 34 个符号并报告 17 个使用文件。对常见方法名执行的精确 `callers/callees` 没有产生可消歧输出，随后以文件级使用关系和局部引用搜索补证，未将噪声结果当作调用边。
- 生产实现与边界：[`fast_int_set.rs`](fast_int_set.rs)；crate 名、入口与无外部依赖事实：[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)。目标目录没有 `doc.go`。
- Go 对照：[`fast_int_set.go`](fast_int_set.go)，核对双表示、状态迁移、全部公开操作、`Next`、panic、错误和字符串规则。
- Rust 测试：[`fast_int_set_test.rs`](fast_int_set_test.rs) 覆盖基础/随机操作、正负及 64 边界、复制、集合运算、区间、位掩码和文本；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 验证提升后不降级、Go 对齐的运算/平移/字符串及非法区间 panic；[`fast_int_set_bench_test.rs`](fast_int_set_bench_test.rs) 提供差集和插入路径的成本对照。
- Go 测试：[`fast_int_set_test.go`](fast_int_set_test.go) 与 [`fast_int_set_bench_test.go`](fast_int_set_bench_test.go)，用于确认测试意图和原实现性能动机。
- 代表性上游：[`pkg/expression/util.rs`](../../expression/util.rs)、[`pkg/planner/funcdep/fd_graph.rs`](../../planner/funcdep/fd_graph.rs)、[`pkg/planner/core/operator/logicalop/`](../../planner/core/operator/logicalop/)、[`pkg/sessionctx/stmtctx/stmtctx.rs`](../../sessionctx/stmtctx/stmtctx.rs)，并核对其相邻 `Cargo.toml` 对 `astersql-util-intset` 的路径依赖。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务给定命令验证目标文档存在且恰含 11 个固定二级章节，并人工复核未建议把 Rust 测试内嵌到生产文件。
