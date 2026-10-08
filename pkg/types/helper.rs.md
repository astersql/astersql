# `pkg/types/helper.rs`

## 文件定位

`pkg/types/helper.rs` 是 MySQL 数值类型辅助逻辑的共享实现，覆盖浮点舍入/截断、按字段元数据限幅、字符串到有符号整数的 best-effort 解析，以及 DECIMAL 显示长度与 precision 的换算。它自身不是 `astersql-types` 根 crate 的普通 `mod`：`pkg/types/internal/field/lib.rs` 的 `helper` 模块通过 `include!("../../helper.rs")` 编译本文件，再以 `pub use helper::*` 从 `astersql-types-field` 导出；根 crate 的 `pkg/types/lib.rs` 则将这个内部 crate 暴露为 `field`。因此生产代码通常从 `types_dependency::field::*` 或内部 field crate 路径访问这些 API。

所属依赖边界可由 `pkg/types/Cargo.toml` 与 `pkg/types/internal/field/Cargo.toml` 复核：根 crate 依赖名为 `types-field-group` 的 `astersql-types-field`，本文件使用的 `ErrTruncated`、`ErrOverflow`、`ErrBadNumber` 与 `errors` 均由该内部 crate 的 `lib.rs` 提供。本文件不声明 feature 或条件编译项。

## 核心职责

1. `RoundFloat`、`Round` 和 `Truncate` 提供与 Go `pkg/types/helper.go` 对齐的二进制浮点舍入/截断原语；其中舍入采用 ties-to-even，截断采用向零方向。
2. `GetMaxFloat` 与 `TruncateFloat` 根据 MySQL 字段的 `flen`/`decimal` 计算形如 `999.99` 的边界，先舍入后饱和到正负边界，并用 `ErrOverflow` 报告越界。
3. `TruncateFloatToString` 先按指定小数位截断，再输出最短十进制显示串。
4. `strToInt` 解析可选符号和十进制数字前缀；发生尾随非数字、无数字或溢出时保留尽可能有用的数值并返回类型错误。`StrToIntForTest` 只为外部测试提供公开入口。
5. `DecimalLength2Precision` 与 `Precision2LengthNoTruncation` 在 DECIMAL 显示长度、scale、符号位和小数点占位之间换算。

这些函数没有 SQL 执行上下文，也不决定 warning/error 的最终处理策略；它们只产生数值与 `SharedError`，上层再决定传播、降级或记录。

## 主要符号

- `pub fn RoundFloat(f: f64) -> f64`：调用 Rust `f64::round_ties_even`，实现最近偶数舍入。例如独立测试 `pkg/types/etc_test.rs::TestRoundFloat` 覆盖正负 `.5`。
- `pub fn Round(f: f64, dec: i32) -> f64`：用 `10^dec` 移动小数点后调用 `RoundFloat`。中间值为无穷时返回原值；最终结果为 NaN 时返回 `0.0`。
- `pub fn Truncate(f: f64, dec: i32) -> f64`：以同样的十进制移位方式调用 `trunc`。移位乘积为 Inf/NaN 时保留原值；`10^dec` 下溢到零时，NaN 保持 NaN，其他值返回零。
- `pub fn GetMaxFloat(flen: i32, decimal: i32) -> f64`：计算 `10^(flen-decimal) - 10^(-decimal)`。
- `pub fn TruncateFloat(...) -> (f64, Option<errors::SharedError>)`：名称沿用 Go，但实际先调用 `Round`，再按 `GetMaxFloat` 限幅。NaN 直接变为零并返回 `ErrOverflow`；有限值先舍入；正负溢出钳位并返回经 `errors::Trace` 包装的错误。
- `pub fn TruncateFloatToString(f: f64, dec: i32) -> String`：调用 `Truncate` 后使用 Rust `format!` 输出；测试覆盖不补尾零、负数和负精度。
- `fn isSpace`、`fn isDigit`、`fn isPunctuation`：私有 ASCII 分类辅助函数。当前 `strToInt` 直接使用标准库 ASCII digit 判断，这三个函数在本文件当前流程中没有调用者，属于与 Go 同文件保持的辅助实现。
- `maxUint`、`uintCutOff`、`intCutOff`：分别表示 `u64::MAX`、十进制累乘的提前溢出阈值，以及 `|i64::MIN|`；它们约束 `strToInt` 的无符号累加过程。
- `pub(crate) fn strToInt(...)`：内部整数解析器；`pub fn StrToIntForTest(...)` 是薄测试出口，直接转发给它。
- `pub fn DecimalLength2Precision(...)` 与 `pub fn Precision2LengthNoTruncation(...)`：互为方向相反的显示元数据换算，但是否完全可逆取决于输入是否满足合法字段约束。

## 执行流程

浮点舍入链为：调用者传入 `f64` 与小数位 `dec`，`Round` 计算 `shift = 10^dec`，形成 `tmp = f * shift`；若 `tmp` 溢出为 Inf 就返回原值，否则用 `RoundFloat(tmp)` 做 ties-to-even 后除回。`TruncateFloat` 在此基础上先由 `GetMaxFloat` 得到字段边界；有限输入经 `Round`，随后与 `±maxF` 比较，超出时钳位并附带 `ErrOverflow`。`pkg/types/datum.rs` 的浮点转换路径调用 `TruncateFloat`，同文件的字段最大/最小值构造调用 `GetMaxFloat`；`pkg/expression/builtin_math.rs` 则通过 `types_dependency::field::Round`/`Truncate` 实现实数 SQL 函数分支。

截断链为：`TruncateFloatToString` 调用 `Truncate`；`Truncate` 将小数点移位后向零取整再除回。极大的正 `dec` 会使乘积 Inf/NaN，从而保留原输入；极小的负 `dec` 会使 `shift` 下溢为零，从而对非 NaN 输入返回 `0.0`。这两条边界由 `pkg/types/helper_test.rs::TestTruncate` 与 Go 对照测试覆盖。

整数解析链为：先对输入做 Unicode `str::trim`；空串立即返回 `(0, ErrTruncated)`；识别一个可选的 `+`/`-`；随后按字节逐位读取 ASCII 数字并在 `u64` 中累积。遇到非数字时停止并保留已解析前缀、返回 `ErrTruncated`；无数字同样截断。累乘或累加越界时置零并返回 `ErrBadNumber`。循环后再按符号检查 `i64` 边界：正数达到 `2^63` 即钳到 `i64::MAX`，负数绝对值大于 `2^63` 即钳到 `i64::MIN`，恰好 `2^63` 则安全表示为 `i64::MIN`。

长度换算链没有外部状态：当 `scale > 0` 时，小数点占一个显示字符；有符号数再占一个符号字符。`DecimalLength2Precision` 扣除这些位置，`Precision2LengthNoTruncation` 加回这些位置。

## 数据与状态

本文件没有结构体、trait、全局可变状态或缓存。所有生产函数均为同步纯计算；唯一的模块级数据是三个编译期 `u64` 常量。错误值不是在本文件初始化，而是来自 `pkg/types/internal/field/lib.rs` 中的 `LazyLock` 标准错误：`ErrTruncated`、`ErrOverflow` 和 `ErrBadNumber`。

主要数据不变量如下：

- `strToInt` 用无符号幅值保存数字，最后才应用符号，因而能够无未定义溢出地表示 `i64::MIN` 的绝对值 `2^63`。
- `TruncateFloat` 的正常有限输出位于闭区间 `[-GetMaxFloat(flen, decimal), GetMaxFloat(flen, decimal)]`；超界时数值和错误同时返回。
- 舍入与截断均基于 `f64` 和十进制幂，继承二进制浮点的表示误差；这不是任意精度 DECIMAL 运算。
- 长度换算不校验 `length`、`scale` 的合法范围，调用者必须提供符合字段元数据约束的值。

## 依赖与调用关系

下游依赖很小：`RoundFloat` 使用标准库 `f64::round_ties_even`，`Round`/`Truncate` 使用 `powi` 与浮点分类，格式化使用 `format!`；错误构造依赖 field crate 提供的三类标准错误与 `errors::Trace`。源码中 `Round -> RoundFloat`、`TruncateFloat -> GetMaxFloat + Round`、`TruncateFloatToString -> Truncate`、`StrToIntForTest -> strToInt` 是明确的内部调用边。

已核实的生产上游包括：

- `pkg/expression/builtin_math.rs` 通过 `types_dependency::field::Round` 与 `Truncate` 处理实数 ROUND/TRUNCATE；DECIMAL 分支使用另一套 `MyDecimal::Round`，不可与本文件的 `f64` 路径混为一谈。
- `pkg/types/datum.rs` 在浮点字段转换时调用 `TruncateFloat`，并用 `GetMaxFloat` 构造浮点字段允许的正负边界。
- `pkg/types/internal/datum/lib.rs` 从 field crate 导入 `GetMaxFloat` 和 `TruncateFloat`，说明这些函数是内部类型 crate 之间的正式接口。

RustCodeGraph `node --file pkg/types/helper.rs` 报告本文件被 10 个文件使用；精确 `callers/callees` 查询未返回可用边，因此上述跨文件关系又以精确符号检索核实。根 `pkg/types/lib.rs` 没有直接挂载生产 `helper` 模块，切勿绕过 `pkg/types/internal/field/lib.rs` 的 include/re-export 边界理解其归属。

## 错误处理与边界

- `Round`：移位乘积为 Inf 时返回原值；除回后为 NaN 时返回零。负 `dec` 会对小数点左侧舍入。
- `Truncate`：乘积为 Inf/NaN 时返回原值；shift 下溢为零时，NaN 原样返回，其他输入返回零。它不返回错误。
- `TruncateFloat`：NaN 返回零加 `ErrOverflow`；有限或无限值超过字段边界时钳位并返回 `ErrOverflow`。错误参数固定为 `"DOUBLE"` 与空值文本，与 Go 文件一致。
- `strToInt`：仅数字前缀成功参与结果；尾随非数字产生 `ErrTruncated`。空串、只有符号或首字符非数字都没有数字结果。超出有符号范围时饱和；更早发生 `u64` 累加溢出时当前实现把累积值清零并返回 `ErrBadNumber`，扩展时不能只验证返回错误而忽略数值契约。
- `strToInt` 先用 Rust Unicode 空白规则裁剪两端，但数字循环只接受 ASCII 数字；Go 版本用 `strings.TrimSpace`，循环里的 `unicode.IsDigit(rune(str[i]))` 实际也逐字节喂入 rune。两边的非 ASCII 行为需要新增测试后才能宣称完全等价。
- `isSpace`、`isDigit`、`isPunctuation` 是 ASCII 辅助函数且当前未接入解析主流程；不能据其存在推断标点或空格会在数字中被接受。

## 并发与资源生命周期

所有函数只操作栈上标量、借用的字符串切片或新建的短 `String`，没有锁、原子变量、线程、异步任务、通道、文件句柄、网络连接或事务。函数调用结束即释放局部资源，适合并发调用。标准错误对象的初始化生命周期由 `pkg/types/internal/field/lib.rs` 的 `LazyLock` 与平台初始化段管理，不由本文件管理；本文件只克隆/包装共享错误。

性能上，浮点函数为常数时间；`strToInt` 为 O(n) 时间、O(1) 附加空间并在首个非数字或溢出处停止；`TruncateFloatToString` 的唯一堆分配是返回字符串。若扩展解析规则，应保持单遍扫描和提前溢出检测，避免把热路径改为大整数或多次中间字符串分配，除非上层契约明确要求。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/types/helper.go`。Rust 保留了 Go 的函数名、参数含义、主要分支和返回形状：`math.RoundToEven` 对应 `f64::round_ties_even`，`math.Pow10` 对应 `10_f64.powi`，`math.Trunc` 对应 `f64::trunc`；Go 的 `(float64, error)`/`(int64, error)` 以数值加 `Option<errors::SharedError>` 表示。`GetMaxFloat`、正负限幅、NaN 溢出、整数边界和长度换算的分支逐项对应。

已确认的实现差异是：Go `TruncateFloatToString` 明确使用 `strconv.FormatFloat(f, 'f', -1, 64)`，Rust 使用 `format!("{}", f)`；现有 Go/Rust 测试覆盖的普通数值输出一致，但未对所有指数表示、负零、Inf/NaN 形式做穷举，扩展格式契约时应补跨语言用例。Rust 另增 `StrToIntForTest` 公开薄封装，Go 测试可直接访问同包私有函数而不需要它。

`pkg/types/helper_test.go` 与 `pkg/types/helper_test.rs` 都覆盖 i64 上下界、极端 `dec` 截断和截断后格式化。更广的 Rust 覆盖位于 `pkg/types/etc_test.rs`（`GetMaxFloat`、`RoundFloat`、`Round`、`TruncateFloat`）、`pkg/types/convert_test.rs`（字段转换中的限幅）以及 `pkg/types/field_type_5_aster_unit_test.rs` / `pkg/types/internal/field/migration_aster_unit_test.rs`（迁移契约与长度换算）。

## 扩展指南

- 修改实数 ROUND 语义时，应从 `RoundFloat`/`Round` 入手，同时检查 `pkg/expression/builtin_math.rs` 的实数分支；不要顺带改变 `MyDecimal::Round`。同步扩展独立的 `pkg/types/etc_test.rs` 及对应 Go 用例。
- 修改字段浮点限幅时，应成对审查 `GetMaxFloat` 与 `TruncateFloat`，并覆盖 NaN、Inf、正负边界、舍入后刚好越界等情形；同步 `pkg/types/convert_test.rs`，还要检查 `pkg/types/datum.rs` 的调用契约。
- 修改 `strToInt` 时，应保持“尽力返回数值 + 错误”的双通道语义，测试空白、只有符号、数字前缀、非 ASCII、`u64` 与 `i64` 边界；生产测试逻辑应继续放在独立 `pkg/types/helper_test.rs`，不要内嵌到本源文件。若公开 API 并非测试专用，应新增正式入口，而不是扩大 `StrToIntForTest` 的职责。
- 修改显示长度换算时，应同时维护两个方向，并在 `pkg/types/field_type_5_aster_unit_test.rs` 增加 signed/unsigned、scale 为零和异常元数据用例。
- 本文件由 `include!` 编译到 field crate；新增依赖或符号时必须确认名称在 `pkg/types/internal/field/lib.rs` 的 include 作用域可见，并更新 `pkg/types/internal/field/Cargo.toml` 而不是误改根 manifest。对外暴露路径变化还要复核根 `pkg/types/lib.rs` 的 `field` re-export。

兼容风险主要是 SQL ROUND/TRUNCATE 结果、字段转换错误类别及展示字符串变化；性能风险主要是整数解析热路径上的额外分配。浮点边界修改还可能影响 planner/executor 之外的 Datum 编解码，因此不能只靠单个 helper 测试判定安全。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件；`node --file pkg/types/helper.rs --offset 1 --limit 260` 返回完整 218 行源码，并报告 10 个使用文件；对 `RoundFloat`、`strToInt`、`TruncateFloat`、`DecimalLength2Precision` 的 `query --kind function --json` 分辨了 Rust、Go 与测试同名符号。精确 `callers/callees` 未产生边，已用精确文本检索补证，未将泛化 `explore` 的同名噪声当作结论。
- 源码与装配：`pkg/types/helper.rs`；`pkg/types/internal/field/lib.rs`（`include!("../../helper.rs")` 和 `pub use helper::*`）；`pkg/types/lib.rs`（field crate re-export）。
- Cargo 边界：`pkg/types/Cargo.toml`；`pkg/types/internal/field/Cargo.toml`。
- Go 对照：`pkg/types/helper.go`；Go 独立测试 `pkg/types/helper_test.go`。
- Rust 测试：`pkg/types/helper_test.rs`、`pkg/types/etc_test.rs`、`pkg/types/convert_test.rs`、`pkg/types/field_type_5_aster_unit_test.rs`、`pkg/types/internal/field/migration_aster_unit_test.rs`。
- 生产调用证据：`pkg/expression/builtin_math.rs`、`pkg/types/datum.rs`、`pkg/types/internal/datum/lib.rs`。
- 按任务约束未运行 Cargo；本文档通过任务规定的 11 章节结构命令验证，并人工复核了文件存在理由、执行路径和安全扩展位置。
