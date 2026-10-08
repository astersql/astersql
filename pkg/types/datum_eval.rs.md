# `pkg/types/datum_eval.rs`

## 文件定位

`pkg/types/datum_eval.rs` 是 `Datum` 加法求值的类型分派实现，当前只定义一个公开入口 `ComputePlus` 和两个私有辅助函数 `trace_overflow`、`add_integer`。它本身不是 `pkg/types/Cargo.toml` 中声明的独立模块：`pkg/types/internal/core_time/lib.rs:647-652` 在 `types::datum_eval` 子模块中通过 `include!("../../datum_eval.rs")` 纳入源码，再以 `pub use datum_eval::*` 导出；顶层 `pkg/types/lib.rs:11` 又把 `types-core-time` 依赖再导出为 `core_time`。

因此，本文件当前处于 `astersql-types-core-time` 内部 crate 的 `types` 命名空间，使用包含点作用域里的 `Datum`、Kind 常量、`MyDecimal`、`DecimalAdd`，并直接依赖父 crate 的 `errors` 模块。`pkg/types/Cargo.toml` 通过路径依赖 `types-core-time = { package = "astersql-types-core-time", path = "internal/core_time" }` 把它接入 `astersql-types`。

## 核心职责

- `ComputePlus(a, b)` 依据左右 `Datum::Kind()` 选择加法规则，只接受 `int64 + int64`、两个方向的 `int64/uint64` 混合、`uint64 + uint64`、`float64 + float64` 和 `decimal + decimal`（`pkg/types/datum_eval.rs:29-75`）。
- 整数路径保证结果不越过目标 SQL 整数域：纯有符号与纯无符号路径复用 `overflow.rs` 的检查函数，混合路径始终产生 `KindUint64` 并对负数减法和正数加法做边界判断。
- 十进制路径保留操作数中较大的显示小数位 `Frac`；浮点路径使用 Rust `f64` 原生加法。
- 所有未列出的 Kind 组合统一返回非法二元操作错误，不执行隐式类型转换。本文件负责的是底层 `Datum` 运算原语，而不是 SQL 表达式层的类型推断或强制转换。

## 主要符号

- `pub fn ComputePlus(a: Datum, b: Datum) -> Result<Datum, errors::SharedError>`：唯一公开 API。先创建默认 `Datum`，按左值 Kind 进入分支，校验右值 Kind，写入对应值与 Kind 后返回。成功结果的具体 Kind 由所调用的 `SetInt64`、`SetUint64`、`SetFloat64` 或 `SetMysqlDecimal` 决定。
- `fn trace_overflow(err: OverflowError) -> errors::SharedError`：把 `pkg/types/overflow.rs` 的结构化 `OverflowError` 格式化成字符串，再由 `errors::New` 归一为此 crate 的共享错误类型；转换后不再保留可向下转型的原始错误类型。
- `fn add_integer(a: u64, b: i64) -> Result<u64, errors::SharedError>`：本文件私有的混合整数加法。`b >= 0` 时转为 `u64` 后调用 `AddUint64`；`b < 0` 时用 `i64::unsigned_abs()` 取得无符号绝对值，先检查其是否大于 `a`，否则计算 `a - magnitude`。使用 `unsigned_abs` 使 `i64::MIN` 也无需对其作不可表示的有符号取反。

文件不声明常量、类型、trait、impl 或条件编译项。

## 执行流程

1. `ComputePlus` 初始化 `Datum::default()`，随后读取 `a.Kind()`（`datum_eval.rs:29-31`）。
2. `KindInt64`：右侧为 `KindInt64` 时调用 `AddInt64`，成功后 `SetInt64`；右侧为 `KindUint64` 时交换参数形态为 `add_integer(b_uint, a_int)`，成功后 `SetUint64`。
3. `KindUint64`：右侧为 `KindInt64` 时调用 `add_integer(a_uint, b_int)`；右侧为 `KindUint64` 时调用 `AddUint64`；两条路径均以 `SetUint64` 写回。
4. `KindFloat64`：仅当右侧也为 `KindFloat64` 时直接相加，并以 `SetFloat64` 写回。IEEE-754 的无穷大、NaN 和舍入行为没有额外拦截。
5. `KindMysqlDecimal`：仅当右侧同类时创建默认 `MyDecimal`，调用 `DecimalAdd` 写入结果，随后 `SetMysqlDecimal`，并把结果 `Frac` 设置为两个输入 `Frac` 的最大值（`datum_eval.rs:65-71`）。错误会在写入结果前由 `?` 返回。
6. 左右 Kind 不匹配或左值属于其他 Kind 时，退出分派并构造包含两侧值和 Kind 的 `SharedError`（`datum_eval.rs:77-84`）。

上述匹配顺序不会改变加法交换律的数值结果，但混合整数分支会把有符号操作数规范化为 `add_integer` 的第二参数，确保两个方向采用同一套无符号结果与溢出规则。

## 数据与状态

函数只处理按值传入的两个 `Datum`，并创建一个局部结果；没有全局状态、缓存或外部副作用。成功路径通过 setter 同时建立结果值和 Kind：纯 `int64` 保持 `KindInt64`，任一 `uint64` 参与的整数混合结果为 `KindUint64`，同类浮点和十进制分别保持原 Kind。

十进制结果还有独立的 `Frac` 元数据不变量：其值等于 `max(a.Frac(), b.Frac())`。当前实际包含环境 `pkg/types/internal/core_time/lib.rs:608-615` 将 `MyDecimal` 定义为 `BigDecimal` 并用 `*result = lhs + rhs` 实现 `DecimalAdd`；这与 `pkg/types/mydecimal.rs:1487-1493` 的完整 `MyDecimal` 算法不是同一个被调用符号，扩展时不能混淆。

混合整数的负数分支把运算视为无符号减法。只有 `|b| <= a` 才能返回 `a - |b|`；例如 `0u64 + (-1i64)` 必须报错。正数分支则受 `u64::MAX` 上界限制。

## 依赖与调用关系

- 装配上游：`pkg/types/internal/core_time/lib.rs:648-652` 包含并再导出本文件；`pkg/types/lib.rs:11` 再导出 `types_core_time`。`pkg/types/Cargo.toml` 声明了相应路径依赖和 `go-package = "pkg/types"` 的移植元数据。
- 直接 Rust 调用者：RustCodeGraph 的 `callers ComputePlus` 只解析到独立测试 `core_time_2_compute_plus_matches_go_dispatch`（`pkg/types/core_time_2_aster_unit_test.rs:194`）。仓库文本检索还显示 `pkg/types/datum_test.rs:381-418` 通过 `core::ComputePlus` 覆盖同一再导出 API。
- 下游整数依赖：`AddInt64`、`AddUint64` 和 `OverflowError` 来自 `pkg/types/overflow.rs:18-68`；`trace_overflow` 将其错误适配到 `errors::SharedError`。
- 下游 Datum/十进制依赖：`Datum`、Kind 常量、getter/setter、`MyDecimal` 和实际调用的 `DecimalAdd` 由包含点 `pkg/types/internal/core_time/lib.rs` 的 `types` 模块提供。
- Go 对照入口：`pkg/types/datum_eval.go:23-64`；Go 的整数检查来自 `pkg/types/overflow.go`，非法操作由 `InvOp2(..., opcode.Plus)` 生成。

当前图中没有生产 Rust 调用者，说明该 API 至少在已索引代码里仍主要由迁移期测试验证；这不等价于 API 无效，因为它经内部 crate 和顶层 crate 公开再导出，但也不能据此宣称已接入完整 SQL 执行主链。

## 错误处理与边界

- `i64 + i64` 通过 `checked_add` 检测正、负溢出；`u64 + u64` 及混合整数非负分支通过无符号 `checked_add` 检测上溢（`pkg/types/overflow.rs:58-68`）。
- 混合整数负分支若 `b.unsigned_abs() > a`，返回 `value ({a}, {b}) overflows BIGINT UNSIGNED`；相等时合法返回零，且 `i64::MIN` 被安全覆盖。
- 整数溢出经 `trace_overflow` 只保留 `OverflowError::to_string()` 文本。当前错误类型兼容性依赖文本而非结构化错误身份。
- `DecimalAdd` 的错误由 `?` 原样传播；只有其成功后才写入结果和 Frac。当前 `types-core-time` 实现恒定返回 `Ok(())`，但签名保留了未来失败能力。
- 浮点加法不检查溢出、NaN 或 infinity，这是原生 `f64` 行为；调用者若要求 SQL 层诊断，需要在更高层处理。
- 不支持相异的浮点/整数、字符串/整数、NULL 或其他 Kind。错误文本包含 `GetValue()` 的显示值和双方 Kind；相比 Go 的 `InvOp2`，文本格式并非完全相同。
- `Datum::default()` 只在函数内部暂存；所有错误分支返回 `Err`，不会把部分初始化的默认 Datum 暴露给调用者。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务、I/O 或长期资源。输入按值移动，结果与十进制临时值均由当前栈帧拥有；错误值按 `SharedError` 的定义返回。函数不修改输入，也不共享可变状态，因此其并发安全性只取决于 `Datum`、`MyDecimal` 和 `errors::SharedError` 自身的类型属性；本文件没有额外同步要求。

资源生命周期止于一次同步调用：整数和浮点路径仅创建标量，十进制路径创建一个局部 `MyDecimal` 并在成功时移动进结果 `Datum`。不存在需要显式释放或回滚的资源。

## 与 Go 版本的对应关系

Rust `ComputePlus` 的分派矩阵、整数混合参数归一方式、浮点同类限制、十进制 Frac 取最大值均直接对应 `pkg/types/datum_eval.go:23-64`。Go 独立测试 `pkg/types/datum_test.go:314-340` 覆盖 int、uint、float、decimal 和非法组合；Rust 的 `pkg/types/core_time_2_aster_unit_test.rs:194-232` 进一步显式覆盖三类整数溢出和负混合下溢，`pkg/types/datum_test.rs:381-418` 覆盖基础分派。

存在以下当前差异：

- Go 非法分支调用 `InvOp2(a.GetValue(), b.GetValue(), opcode.Plus)`，Rust 直接用 `errors::New(format!(...))`，且额外写出 Kind；错误类别/文本可能不完全一致。
- Go 的完整 `Datum` 使用 `pkg/types/mydecimal.go` 中的 `MyDecimal` 与可能失败的 `DecimalAdd`；当前 Rust 包含环境使用 `BigDecimal` 简单相加。`pkg/types/mydecimal.rs` 虽有完整 `DecimalAdd`，但不是本文件在当前装配下解析到的调用目标。
- Go 的 `int64 + uint64` 实现也写入 `SetUint64`。Go 测试表中该用例的期望构造为 `NewIntDatum(100)`，但断言使用 `Datum.Compare` 比较数值而未检查 Kind；Rust 的迁移期测试明确断言实际结果为 `KindUint64`，与实现写入行为一致。
- Go 用 `errors.Trace` 保留错误链语义；Rust 的整数适配将结构化溢出错误转换为新建的共享错误文本。

## 扩展指南

- 新增可接受的 Kind 组合应修改 `ComputePlus` 的分派矩阵，并明确结果 Kind、是否允许隐式转换、溢出目标类型和错误语义；不要只让测试数值相等而忽略结果 Kind。
- 修改混合整数规则时优先复用并统一 `pkg/types/overflow.rs::AddInteger`，或说明为何保留本文件私有 `add_integer`；必须覆盖 `i64::MIN`、`u64::MAX`、零减一、绝对值等于无符号操作数等边界。
- 增强十进制实现前应先确认 `internal/core_time` 的 `Datum/MyDecimal` 迁移边界，避免错误地修改未被本文件调用的 `pkg/types/mydecimal.rs::DecimalAdd`。还需验证精度、scale/Frac、舍入和完整 Go 错误语义。
- 错误兼容要求提高时，应使不匹配分支对齐 Go `InvOp2` 的错误类别，并考虑保留 `OverflowError` 的来源链，而非只复制显示文本。
- 测试逻辑必须保持在独立文件。同步更新 `pkg/types/core_time_2_aster_unit_test.rs` 或 `pkg/types/datum_test.rs`；若改变 Go 对齐预期，同时核对 `pkg/types/datum_test.go::TestComputePlusAndMinus`。本仓库规则禁止把单元测试内嵌到 `datum_eval.rs`。
- 性能上，该函数位于求值原语层；新增分配、字符串格式化或通用动态分派前应评估热路径成本。当前成功整数/浮点路径不构造错误字符串，十进制路径的成本主要来自 `BigDecimal` 加法。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件；`files --filter pkg/types/datum_eval.rs` 确认目标文件已索引并识别 4 个节点。
- RustCodeGraph 源码与调用查询：`node --file pkg/types/datum_eval.rs --offset 1 --limit 240`；`query ComputePlus --limit 20`；`node ComputePlus`；`callers ComputePlus --limit 50`；`callees ComputePlus --limit 50`；`query/callers/callees add_integer`。查询确认目标实现及直接测试调用边；同名 `pkg/expression/aggregation/lib.rs::ComputePlus` 已排除出本文件调用链。
- RustCodeGraph 下游查询：`node DecimalAdd`、`node AddInt64`、`node AddUint64`，并读取索引中的 `pkg/types/internal/core_time/lib.rs:560-667`，确认实际包含环境、再导出和依赖符号。
- 已读 Rust/Cargo 路径：`pkg/types/datum_eval.rs`、`pkg/types/overflow.rs`、`pkg/types/internal/core_time/lib.rs`、`pkg/types/lib.rs`、`pkg/types/Cargo.toml`、`pkg/types/core_time_2_aster_unit_test.rs`、`pkg/types/datum_test.rs`。目标目录不存在 `doc.go`，因此没有额外包契约文件可读。
- 已读 Go 对照与测试：`pkg/types/datum_eval.go`、`pkg/types/datum_test.go::TestComputePlusAndMinus`；RustCodeGraph 同时核对 `pkg/types/overflow.go::{AddInt64,AddUint64}` 与 `pkg/types/mydecimal.go::DecimalAdd`。
- 本任务为纯文档分析，按计划未运行 Cargo。结构校验要求目标文档存在且恰有 11 个固定二级章节；交付前另行执行任务文件指定命令并人工复核链接、事实边界与扩展说明。
