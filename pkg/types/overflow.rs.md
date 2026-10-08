# `pkg/types/overflow.rs`

## 文件定位

`pkg/types/overflow.rs` 是 MySQL 整数算术边界检查的 Rust 实现，负责在 `i64`、`u64` 以及二者混合运算时保持 `BIGINT` / `BIGINT UNSIGNED` 的可表示范围。它不定义通用任意精度算术，也不处理 SQL 的 NULL、warning 模式或表达式类型推导；调用方应先完成这些工作，再调用本文件的窄接口。

该文件由 `pkg/types/internal/file_group/lib.rs` 通过 `#[path = "../../overflow.rs"] pub mod overflow` 编入 `astersql-types-file-group` 子 crate。根 crate `astersql-types` 在 `pkg/types/Cargo.toml` 中依赖该子 crate，并由 `pkg/types/lib.rs` 以 `pub use types_file_group as file_group` 暴露，因此稳定访问路径是 `astersql_types::file_group::overflow`。同一源码还被 `pkg/types/internal/core_time/lib.rs` 的局部兼容模块以 `include!("../../overflow.rs")` 使用；这意味着修改本文件时必须同时留意两个编译上下文。

真实应用接线可见于 `pkg/expression/aggregation/lib.rs`（再导出 `AddInt64`、`AddUint64`）、`pkg/expression/aggregation/sum_int.rs`（整数 SUM 累加）和 `pkg/expression/builtin_arithmetic_vec.rs`（三种整数符号组合的 `DIV`）。RustCodeGraph 的文件节点也把 `pkg/expression/aggregation/lib.rs` 标为该文件的使用者。

## 核心职责

- 对无符号、有符号和混合符号的加、减、乘、除执行边界判断，成功时返回数学结果，越界时返回 `OverflowError`。
- 将 SQL 目标类型固定为 `"BIGINT"` 或 `"BIGINT UNSIGNED"`，并保存形如 `"(a, b)"` 的操作数文本，供上层转换为 MySQL 风格错误。
- 对 `Duration` 提供与 Go `time.Duration` 底层 `int64` 纳秒表示一致的加减入口。
- 通过复用基础运算减少规则分叉：`AddDuration -> AddInt64`、`SubDuration -> SubInt64`，混合加减及有符号乘法也委托给无符号基础函数。

本文件的职责止于“结果是否能由目标整数类型表示”。除法函数明确不把除数为零转换成 `OverflowError`：`DivInt64`、`DivUintWithInt`、`DivIntWithUint` 最终仍使用 Rust 整数除法，零除会 panic。生产调用者 `BuiltinArithmeticIntDivideIntSig::vec_eval_int` 在进入这些函数前先处理零除（`pkg/expression/builtin_arithmetic_vec.rs`）。

## 主要符号

- `OverflowError { target_type: &'static str, expression: String }`：公开错误载体。`target_type` 只借用静态类型名，`expression` 拥有格式化后的操作数文本；派生 `Clone`、`Debug`、`Eq`、`PartialEq`，并实现 `Display` 与 `std::error::Error`。私有构造器 `OverflowError::new` 统一创建错误。
- `type Duration = i64`：Go `time.Duration` 的兼容表示；没有单位类型保护，调用者需保证数值语义为纳秒。
- 加法：`AddUint64(u64, u64)`、`AddInt64(i64, i64)`、`AddDuration(Duration, Duration)`、`AddInteger(u64, i64)`。
- 减法：`SubUint64(u64, u64)`、`SubInt64(i64, i64)`、`SubDuration(Duration, Duration)`、`SubUintWithInt(u64, i64)`、`SubIntWithUint(i64, u64)`。
- 乘法：`MulUint64(u64, u64)`、`MulInt64(i64, i64)`、`MulInteger(u64, i64)`。
- 除法：`DivInt64(i64, i64)`、`DivUintWithInt(u64, i64)`、`DivIntWithUint(i64, u64)`。

全部运算函数均为公开 API，并保留 Go 风格的 PascalCase 名称。文件没有 trait、宏、模块级常量或条件编译项。

## 执行流程

1. 无符号加减直接调用 `checked_add` / `checked_sub`；有符号加减调用对应的 `i64` checked 运算。这同时覆盖普通边界和 `0 - i64::MIN`，避免先对 `MIN` 取负。
2. `AddInteger` 将非负 `i64` 转为 `u64` 后走 `AddUint64`；负数用 `unsigned_abs()` 得到可表示的幅值（包括 `i64::MIN`），若幅值大于左操作数则下溢，否则相减。
3. `SubUintWithInt` 在减数为负时把操作改写为无符号加法，否则走无符号减法。`SubIntWithUint` 要求左值非负且其无符号值不小于右值，成功结果以 `u64` 返回。
4. `MulUint64` 在 `b > 0` 时用 `a > u64::MAX / b` 预判越界，避免先执行溢出乘法。`MulInteger` 先处理零，再拒绝任何会产生负的非零无符号结果。
5. `MulInt64` 先处理零，再拆分符号，并用 `unsigned_abs()` 把幅值交给 `MulUint64`。正结果上限是 `i64::MAX`；负结果幅值可到 `i64::MAX + 1`，恰好表示 `i64::MIN`，最终用 `wrapping_neg()` 恢复符号。
6. `DivInt64` 只拦截唯一的有符号除法溢出 `i64::MIN / -1`。混合除法先判断负操作数对应的数学结果是否为负且绝对值至少为 1：若是，则无法表示为 `u64` 并报错；若绝对值小于 1，则整数截断结果为 0；其余情况执行普通除法。
7. 任一越界分支调用 `OverflowError::new`；成功路径返回 `Ok(result)`。上层通常将错误的 `Display` 文本映射进自身错误类型，例如聚合代码的 `errors::New(error.to_string())` 和向量化算术的 `map_integer_overflow`。

## 数据与状态

所有运算都是纯函数：输入按值传递，不访问全局变量，不缓存结果，也不修改调用者状态。唯一的堆所有权来自错误路径上的 `String` 表达式；成功路径只处理固定宽度整数。

核心不变量如下：

- `Ok` 中的值必定可由函数返回类型表示，且与无限精度整数运算后按 Rust/Go 整数除法向零截断的结果一致。
- `AddInteger`、`SubUintWithInt`、`SubIntWithUint`、`MulInteger` 和两个混合除法入口的返回类型为 `u64`，因此非零负结果属于 `BIGINT UNSIGNED` 溢出，而不是二进制补码回绕。
- `MulInt64` 对负结果单独允许幅值 `2^63`，但正结果只允许到 `2^63-1`。
- 每个 `OverflowError.expression` 在本文件中都用 `format!("({}, {})", a, b)` 生成；测试依赖其中的逗号与空格。

## 依赖与调用关系

本文件的下游依赖仅为 Rust 标准库：整数 `checked_*` / `unsigned_abs` / `wrapping_neg`，`std::fmt::{Display, Formatter}`，以及 `std::error::Error`。它没有第三方 crate 依赖；`pkg/types/internal/file_group/Cargo.toml` 中的 `collate`、`astersql_errors` 是同一文件组内其他模块所需，并非本文件直接使用。

RustCodeGraph 对目标节点给出的内部调用边包括：`AddDuration -> AddInt64`、`SubDuration -> SubInt64`、`AddInteger -> AddUint64`、`SubUintWithInt -> AddUint64/SubUint64`、`MulInt64 -> MulUint64`、`MulInteger -> MulUint64`，以及各错误分支到 `OverflowError::new`。由于当前索引的 `callers` 命令未返回这些同名跨 crate 调用者，外部边又用源码引用核验：

- `pkg/expression/aggregation/sum_int.rs` 的整数聚合累加调用 `types::AddUint64` / `types::AddInt64`；`pkg/expression/aggregation/lib.rs` 从 `file_dependency::overflow` 再导出二者。
- `pkg/expression/builtin_arithmetic_vec.rs` 的 `BuiltinArithmeticIntDivideIntSig::vec_eval_int` 按左右值的 unsigned 标志调用 `DivUintWithInt`、`DivIntWithUint` 或 `DivInt64`，并先处理 NULL 与零除。
- `pkg/types/datum_eval.rs` 调用 `AddInt64` / `AddUint64`；它当前由 `pkg/types/internal/core_time/lib.rs` 的兼容模块包含，RustCodeGraph 显示其直接文件使用者是相应独立测试，而非已确认的 SQL 主链入口，因此这里不扩大声称其运行时覆盖范围。

## 错误处理与边界

溢出错误的展示格式为 ``{target_type} value is out of range in '{expression}'``。`OverflowError` 不含堆栈、错误码或操作符；上层若需要 SQL 错误码、具体 `DIV`/`+` 上下文或 statement warning，必须在映射时补充。

重点边界包括：

- `u64::MAX + 1`、`0_u64 - 1`、`u64::MAX * 2` 返回 `BIGINT UNSIGNED` 错误。
- `i64::MAX + 1`、`i64::MIN - 1`、`0 - i64::MIN`、`i64::MIN * -1`、`i64::MIN / -1` 返回 `BIGINT` 错误。
- 使用 `unsigned_abs()` 使 `i64::MIN` 的幅值 `2^63` 能安全参与混合运算；不得改回普通取负。
- 零乘任何值先返回 0，所以 `MulInteger(0, negative)` 合法；非零无符号数乘负数报错。
- 混合除法中，负的真值若向零截断为 0，则返回 `Ok(0)`；若幅值至少为 1，则因目标是无符号而报错。
- 三个除法入口都要求调用方先排除零除。直接传入零除数会 panic，这与 Go 文件注释所述 panic 契约一致，但不是 `Result` 的错误分支。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、文件句柄、网络连接或事务。函数只操作局部标量；`OverflowError` 拥有自己的 `expression`，因而跨线程传递时不会借用操作数或临时格式化缓冲区。`target_type` 是 `&'static str`，生命周期覆盖整个进程。

没有共享可变状态，因此函数本身可并发调用。资源成本是常数级计算；错误路径会为表达式分配一个 `String`，成功路径无此分配。若上层在大批量向量化循环中频繁触发溢出，错误格式化成本仍由第一次失败路径承担，当前文件没有批量聚合或延迟格式化机制。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/overflow.go`，独立 Go 测试是 `pkg/types/overflow_test.go`。Rust 的 15 个公开函数与 Go 同名函数逐一对应，测试表覆盖相同的加、减、乘、除及混合符号边界。

实现手段存在语言适配，但目标行为一致：Go 以显式 `math.Max*` / `math.Min*` 比较判断，Rust 的基础加减使用 `checked_add` / `checked_sub`；Go 用 `ErrOverflow.GenWithStackByArgs` 返回带 TiDB 错误体系和堆栈的错误，Rust 用轻量 `OverflowError` 保存类型与表达式，再由调用方映射。Go 的 `time.Duration` 是独立命名类型，Rust 当前只是 `i64` 别名，因此 Rust 编译器不能阻止把普通整数误当 duration。

Rust 使用 `unsigned_abs()` 明确安全处理 `i64::MIN`，并在 `MulInt64` 中用 `wrapping_neg()` 生成 `i64::MIN`。这些写法表达了 Go 代码依赖的两补码边界意图，避免 Rust debug 构建中的普通取负溢出。除法的零除 panic 契约与 Go 注释一致；生产向量化调用点在调用前处理零除。

Rust 回归证据分布在 `pkg/types/overflow_test.rs`（完整 Go 表和错误文案）、`pkg/types/overflow_duration_test.rs`（duration 加减）、`pkg/types/overflow_add_sub_test.rs`（基础边界）以及由 file-group 子 crate 挂载的 `pkg/types/overflow_10_aster_unit_test.rs`（完整表）。测试逻辑与生产文件保持分离。

## 扩展指南

- 增加新的整数运算时，优先复用已有 checked 基础函数，并先确定结果的 SQL 目标类型；不要让 Rust 的 `as` 转换或 release 模式回绕替代显式范围检查。
- 修改混合符号逻辑时，必须单独覆盖 `i64::MIN`、`i64::MAX`、`u64::MAX`、0、结果绝对值小于 1 的负除法、恰好边界和越界一单位。特别保留 `unsigned_abs()` 与 `MulInt64` 的 `i64::MAX + 1` 负结果许可。
- 修改错误格式时，要同步检查 `OverflowError::fmt`、四个 Rust 测试文件以及 `pkg/types/overflow_test.go`；聚合和向量化调用方会把该文本包装到自己的错误中，文案变化可能影响兼容性测试。
- 新增函数若应成为根类型 API，接入点仍是 `pkg/types/internal/file_group/lib.rs` 的 `overflow` 模块与 `pkg/types/lib.rs` 的 `file_group` 再导出；若表达式子系统要直接使用，还需在相应 Cargo 依赖和局部门面中显式导入或再导出。
- 若改变除零契约，应同时修改三个除法函数和所有调用前置检查；这是用户可见 SQL 行为，不能只把 panic 改成任意 `OverflowError`。
- 性能方面保持成功路径无分配；兼容性方面保持 Go 表、MySQL 类型名和整数截断规则；测试应继续放在独立 `*_test.rs` 文件，不嵌入 `overflow.rs`。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件节点完整显示 240 行源码，并报告 `pkg/expression/aggregation/lib.rs` 与 `pkg/types/overflow_duration_test.rs` 的文件级使用关系。
- RustCodeGraph 精确符号查询确认 `AddInteger`、`MulInt64`、`DivUintWithInt` 在 Go/Rust 中均存在对应定义；目标文件 `callees` 结果确认 15 个公开运算函数的内部委托和 `OverflowError::new` 错误边。
- 已读 Rust 源与装配：`pkg/types/overflow.rs`、`pkg/types/internal/file_group/lib.rs`、`pkg/types/internal/core_time/lib.rs`、`pkg/types/lib.rs`、`pkg/types/datum_eval.rs`、`pkg/expression/aggregation/lib.rs`、`pkg/expression/aggregation/sum_int.rs`、`pkg/expression/builtin_arithmetic_vec.rs`。
- 已读 crate 声明：`pkg/types/Cargo.toml`、`pkg/types/internal/file_group/Cargo.toml`；前者确认根 crate 到 file-group 子 crate 的依赖与再导出边界，后者确认本模块没有额外直接依赖。
- 已读对照与测试：`pkg/types/overflow.go`、`pkg/types/overflow_test.go`、`pkg/types/overflow_test.rs`、`pkg/types/overflow_duration_test.rs`、`pkg/types/overflow_add_sub_test.rs`、`pkg/types/overflow_10_aster_unit_test.rs`。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付结构检查要求本文恰有“文件定位”至“验证依据”11 个固定二级标题；最终以任务文件给定的 `test` + `rg -c` 命令验证。
