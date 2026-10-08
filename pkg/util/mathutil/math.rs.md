# `pkg/util/mathutil/math.rs`

## 文件定位

本文件是 `astersql-util-mathutil` crate 的基础数值工具实现，源码入口由 [`pkg/util/mathutil/Cargo.toml`](Cargo.toml) 指向 [`lib.rs`](lib.rs)，再由 `lib.rs` 的 `mod math` 装入，并在 crate 根公开再导出全部常量和函数。workspace 根还通过 `facade_util_mathutil` 依赖并在 `pkg/lib.rs` 的 `pkg::util::mathutil` 门面中再导出，因此仓库内调用者通常写作 `mathutil::...`，不直接引用私有模块 `math`。

它对应 Go 包中的 [`math.go`](math.go)，只提供无状态、小粒度的整数与浮点辅助操作，不参与 SQL 请求调度、存储访问或事务管理。当前 Rust 生产调用集中在类型宽度推导和 planner 基数估算；其他 API 已移植并公开，但检索到的 Rust 使用点目前主要是测试。

## 核心职责

- 用 `MaxInt`、`MinInt`、`MaxUint`、`IntBits` 暴露当前目标平台的指针宽度整数边界。
- 用 `Abs` 保留 Go 二进制补码公式，包括 `i64::MIN` 无法表示正绝对值时仍返回原位模式的特殊行为。
- 用 `uintSizeTable`、`StrLenOfUint64Fast` 和 `StrLenOfInt64Fast` 在不反复除以 10 的情况下计算十进制文本宽度。
- 用 `IsFinite` 判断 `f64` 是否既非 NaN 也非正负无穷，并用泛型 `Clamp` 实现闭区间钳制。
- 用 `NextPowerOfTwo` 计算不小于输入的最小 2 的幂；输入合法性由调用方负责。
- 用泛型 `Divide2Batches` 把总量尽量均匀地拆为正数批次，余数优先分配给前面的批次。

## 主要符号

- `pub const MaxInt: isize`、`MinInt: isize`、`MaxUint: usize`、`IntBits: usize`：直接取 Rust 当前目标平台的 `isize`/`usize` 边界和 `usize::BITS`。这些是编译目标相关常量，不固定为 64 位。
- `pub fn Abs(n: i64) -> i64`：令符号掩码 `y = n >> 63`，返回 `(n ^ y).wrapping_sub(y)`。`wrapping_sub` 是保留 Go 补码溢出结果的关键，不能替换成会在调试构建检查溢出的普通减法。
- `static uintSizeTable: [u64; 21]`：索引 0 是哨兵；索引 1 至 19 存放对应十进制位数的最大值，索引 20 为 `u64::MAX`，保证所有 `u64` 都能命中。
- `pub fn StrLenOfUint64Fast(x: u64) -> usize`：从表下标 1 起顺序比较并返回首个满足 `x <= limit` 的下标；末项覆盖全域，因此正常输入不会到达 `unreachable!`。
- `pub fn StrLenOfInt64Fast(x: i64) -> usize`：负数先贡献一个负号字符，再把 `Abs(x)` 的补码结果转换为 `u64` 并复用无符号位数函数；因此 `i64::MIN` 得到 19 位数字加一个负号，共 20 个字符。
- `pub fn IsFinite(f: f64) -> bool`：利用有限数的 `f - f` 不是 NaN，而 NaN 和正负无穷的自减结果为 NaN，返回其否定。
- `pub fn Clamp<T: PartialOrd>(n, minv, maxv) -> T`：先判断上界，再判断下界，否则原样返回。它按值取得并返回 `T`，不要求 `Copy`，但调用者会交出三个实参的所有权。
- `pub fn NextPowerOfTwo(mut i: i64) -> i64`：已是 2 的幂时直接返回；否则先乘 2，再反复执行 `i &= i - 1` 清除最低置位，直到只剩一个置位比特。
- `pub fn Divide2Batches<T>(total, batches) -> Vec<T>`：要求 `T` 支持复制、默认零值、比较、加减赋值、除法和取余。它没有单独构造字面量 1，而用 `batches / batches` 得到同类型的 1，从而支持 `i8` 等不实现 `From<u8>` 的整数类型。

本文件没有类型、trait 或 `impl` 定义。条件编译只用于选择 `Assert`：`formal-crate` 决定 crate 内路径还是 workspace 门面路径，`test`/`intest`/`enableassert` 决定启用真实断言还是 `no_assert` 实现。

## 执行流程

十进制宽度路径从 `StrLenOfInt64Fast` 开始：先根据符号决定是否计入 `-`，调用 `Abs` 得到与 Go 相同的补码绝对值位模式，转换为 `u64`，然后由 `StrLenOfUint64Fast` 从小到大扫描 `uintSizeTable`。生产调用 `pkg/types/field_type.rs::DefaultTypeForValue` 在识别 `i32`、`i64` 或 `u64` 后使用这些结果设置 `FieldType.flen`。

planner 的钳制路径直接调用 `Clamp`。`pkg/planner/cardinality/selectivity.rs` 将多值索引路径的 `CountAfterAccess / realtimeCount` 限制到 `[0.0, 1.0]` 后参与交集乘积或并集概率合成；`pkg/planner/cardinality/row_count_column.rs` 将组合范围估算的 `totalCount` 限制到 `[1.0, tableRowCount]` 后返回。

`Divide2Batches` 先计算固定商 `quotient` 和余数 `remainder`。循环每轮以商作为大小；仍有余数时给当前批次加一并扣减余数，因此结果前部至多比后部大一。随后通过条件断言检查大小为正、压入 `Vec`，再从剩余总量扣除，直到总量为零。若 `total < batches`，商为零且前 `total` 轮各消费一个余数，得到 `total` 个 1；若 `total == 0`，循环不执行并返回空向量。

## 数据与状态

唯一静态数据是不可变的 `uintSizeTable`，没有可变全局变量、缓存或惰性初始化。全部函数的结果只取决于参数和编译目标的整数位宽。

`Divide2Batches` 在调用内拥有一个动态 `Vec<T>`，输出长度为 `min(total, batches)`（限于文档契约要求的非负 `total`、正 `batches` 和常规整数语义），所有元素均为正、总和保持为原始 `total`，任意两批大小相差不超过 1，较大的批次位于前部。其他函数只使用标量或不可变表，不保留跨调用状态。

## 依赖与调用关系

下游依赖仅包括 Rust 标准库：`std::ops::{AddAssign, Div, Rem, SubAssign}` 为 `Divide2Batches` 的泛型约束；其余整数位运算、浮点 `is_nan`、平台常量和 `Vec` 均来自标准库。断言接口来自仓库内 `pkg/util/intest` 的 `Assert`/`AssertArg`，具体实现由 feature 条件选择。

内部调用边为 `StrLenOfInt64Fast -> Abs` 和 `StrLenOfInt64Fast -> StrLenOfUint64Fast`；`Divide2Batches -> Assert`。其余函数相互独立。

上游生产调用经 RustCodeGraph 与精确文本检索核对为：

- `pkg/types/field_type.rs::DefaultTypeForValue -> StrLenOfInt64Fast/StrLenOfUint64Fast`，用于整数默认字段显示宽度。
- `pkg/planner/cardinality/selectivity.rs::CalcTotalSelectivityForMVIdxPath -> Clamp`，用于约束多值索引选择率。
- `pkg/planner/cardinality/row_count_column.rs::getPseudoRowCountWithPartialStats -> Clamp`，用于约束部分统计信息下的总行数估算。

在当前 Rust 源集合中，`IsFinite`、`NextPowerOfTwo` 和 `Divide2Batches` 未发现测试之外的直接调用；`Abs` 的生产调用只经 `StrLenOfInt64Fast` 间接发生。公开 API 仍由 `lib.rs` 导出，不能据此认定它们未接线或可删除。

## 错误处理与边界

这些函数均不返回 `Result`。合法输入下没有 I/O 或可恢复错误；边界依赖调用契约：

- `Abs(i64::MIN) == i64::MIN` 是有意兼容 Go 的补码结果，不是数学意义上的非负绝对值。下游转换成 `u64` 后仍能正确计算 19 位数值部分。
- `StrLenOfUint64Fast(0) == 1`，`u64::MAX` 命中表中第 20 项。表若被修改而不再以 `u64::MAX` 收尾，会触发 `unreachable!`。
- `IsFinite` 对 NaN 和正负无穷均返回 `false`。`Clamp<f64>` 使用 `PartialOrd`；若 `n` 是 NaN，两次比较都为 `false`，因此返回 NaN。它也不验证 `minv <= maxv`，调用者必须提供有效闭区间。
- `NextPowerOfTwo` 的公开契约要求 `i > 0` 且结果不溢出。实现对 0 会直接返回 0，但这不构成受支持语义；负数或过大输入可能在检查溢出的构建中 panic，不能依赖 release 模式的环绕结果。
- `Divide2Batches` 要求 `batches > 0`，零会在除法或取余时 panic。其算法也以非负 `total` 和普通整数算术为前提：负总量会返回空结果，负批次数可能产生非正大小甚至不终止。真实断言仅在 `test`、`intest` 或 `enableassert` 配置启用，正式无断言配置不能替代调用方校验。
- `Divide2Batches` 的 `Vec` 分配可能因资源耗尽而终止进程；实现没有预分配容量，也没有显式处理算术溢出。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。不可变静态表可被并发只读访问，函数没有共享可变状态，因此并发调用之间不会相互影响；具体泛型值是否可在线程间传递仍由 `T` 自身的 `Send`/`Sync` 性质和调用上下文决定，本文件未增加这些约束。

资源生命周期局限于函数栈帧和返回值所有权。`Clamp` 消费并返回一个输入值；未返回的值在函数退出时析构。`Divide2Batches` 在函数内逐步构造 `Vec<T>` 并把所有权交给调用方；发生 panic 时由 Rust 展开规则清理已构造元素（若构建配置选择 abort，则由进程终止策略接管）。

## 与 Go 版本的对应关系

Rust [`math.rs`](math.rs) 逐项对应 Go [`math.go`](math.go)：平台常量、补码 `Abs`、21 项十进制表、有限性表达式、`Clamp` 分支顺序、位清除算法和批次余数前置分配均保持一致。Go 的 `int`/`uint` 平台宽度映射为 Rust `isize`/`usize`；Go 返回的位数 `int` 映射为 `usize`。

主要语言层差异如下：

- Go `Abs` 的有符号溢出自然按补码运算；Rust 使用 `wrapping_sub` 显式保留该语义。
- Go `Clamp` 约束为 `cmp.Ordered`，Rust 使用更宽的 `PartialOrd`，因此可实例化为含 NaN 的浮点类型；分支顺序与比较行为仍与 Go 当前实现一致。
- Go `Divide2Batches` 约束为 `constraints.Integer`，Rust 通过操作 trait 组合表达需求，类型范围在类型系统层面更宽；实际契约仍是整数。Go 用 `size++`，Rust 用 `batches / batches` 构造同类型的 1。
- Go 用 `intest.Assert`；Rust 根据 `formal-crate`、`test`、`intest`、`enableassert` 选择等价路径或无断言实现。
- Rust 独立测试 [`math_test.rs`](math_test.rs) 除复刻 Go [`math_test.go`](math_test.go) 的位数、钳制、2 的幂和批次案例外，还覆盖 `Abs(i64::MIN)`、平台常量和 `IsFinite`。[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 进一步覆盖所有常见有符号/无符号整数宽度的批次泛型行为。

## 扩展指南

新增或修改基础数值 API 时，应在 [`math.rs`](math.rs) 实现，并在 [`lib.rs`](lib.rs) 的 `pub use math::{...}` 中接入公开接口；若目标是 Go 对齐，还应逐分支核对 [`math.go`](math.go)，尤其关注溢出、平台位宽、NaN 比较和整数泛型差异。不要把单元测试内嵌回生产文件：常规对应测试放在独立的 [`math_test.rs`](math_test.rs)，跨模块移植契约可补充在 [`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)，并同步检查 Go [`math_test.go`](math_test.go) 的原始意图。

修改 `uintSizeTable` 或位数算法时必须覆盖 0、每个 10 的幂边界、`i64::MIN` 和 `u64::MAX`；修改 `NextPowerOfTwo` 时必须明确 0、负数和溢出策略，不能悄然把当前“调用方负责”的契约改成另一种行为；修改 `Divide2Batches` 时必须保持总和、正数元素、前部余数分配和跨整数宽度行为，并评估是否需要容量预分配。修改 `Clamp` 时应回归 planner 的选择率和行数上下界，避免 NaN 或倒置区间语义发生未经设计的变化。

兼容风险主要是 Go/Rust 边界行为漂移和公开返回类型变化；性能风险主要是把位数查表改为除法循环、在 planner 热路径增加分配，或让批次划分产生过多小向量元素。由于 crate 通过 workspace 门面广泛可见，即使当前没有生产调用的公开函数也应先做全仓调用检索再变更或删除。

## 验证依据

- RustCodeGraph `status`：索引有效，包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/util/mathutil` 列出本目录 16 个已索引 Go/Rust 文件。
- RustCodeGraph `node --file` 已读取：`pkg/util/mathutil/math.rs`（完整 173 行）、`lib.rs`、`math_test.rs`、`migration_aster_unit_test.rs`、Go `math.go`、Go `math_test.go`，以及三个生产调用片段 `pkg/types/field_type.rs`、`pkg/planner/cardinality/selectivity.rs`、`pkg/planner/cardinality/row_count_column.rs`。
- RustCodeGraph `explore`/`query` 核对了七个公开函数及调用关系；其中图的宽泛查询准确给出 `Clamp` 的两个 planner 调用和本 crate 测试，但遗漏了 `pkg/types/field_type.rs` 的位数函数调用，因此又用限定 `*.rs` 的精确 `rg` 检索全仓补证。文档只把两种证据共同确认的直接调用写为当前事实。
- crate 边界由 [`Cargo.toml`](Cargo.toml) 核对：包名为 `astersql-util-mathutil`，库入口是 `lib.rs`，默认启用 `formal-crate`，另有 `intest` 和 `enableassert` feature；porting 元数据指向 Go `pkg/util/mathutil`。
- 行为边界由独立 Rust/Go 测试核对：百万次位数样本与手工边界、`i64::MIN`、NaN/无穷、数值和字符串钳制、2 的幂案例、零总量及不整除批次、`i8` 与 `u64` 泛型实例。
- 本任务是纯文档分析，按计划不运行 Cargo；最终只执行任务指定的 11 章节结构检查，并人工复核“文件为何存在、如何运行、如何安全扩展”。
