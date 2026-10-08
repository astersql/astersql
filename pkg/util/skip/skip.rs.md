# `pkg/util/skip/skip.rs`

## 文件定位

本文件是 Cargo 包 `astersql-util-skip` 的行为实现，包入口 [`lib.rs`](./lib.rs) 通过 `pub mod skip` 声明模块并用 `pub use skip::*` 在 crate 根重新导出这里的公开 API。根工作区把该包列为 member，并以 `facade_util_skip` 暴露同一个 path 依赖；包自身只依赖 `astersql-testkit-testflag`，没有 feature 或可选依赖（依据：[`Cargo.toml`](./Cargo.toml) 与仓库根 `Cargo.toml`）。

它不参与 SQL 请求、规划或存储运行链，而是把 Go `pkg/util/skip` 的测试跳过约定移植给 Rust 测试：根据 `-short` 或 `-long` 状态决定是否终止当前用例。当前 Rust 仓库中，目标文件的公开能力只由同 crate 的独立测试 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 引用；RustCodeGraph 未找到其他 Rust 调用者。因此它目前是已实现、已做迁移回归，但尚未接入其他 Rust 测试套件的测试基础设施。

## 核心职责

- 用 `TestContext` 抽象本文件真正需要的测试框架能力：标记 helper，以及以永不返回的方式跳过用例。
- 用 `short_from_args` 从可注入参数序列中识别 short 模式，使解析行为可以脱离进程全局参数独立测试。
- 用 `under_short_with`、`not_under_long_with` 接收显式布尔值，集中实现条件判断、固定原因前缀和附加参数拼接。
- 用 `UnderShort`、`NotUnderLong` 保留 Go 风格公开名称，并分别从当前进程参数和 `testflag::Long()` 获取真实模式。

该文件只决定“是否调用测试上下文的 skip”；它不负责测试注册、命令行入口、输出格式化或 panic 捕获。`TestContext` 的具体实现必须由接入它的 Rust 测试框架或适配层提供。

## 主要符号

- `pub trait TestContext`：公开适配契约。`helper(&mut self)` 对应 Go 的 `t.Helper()`；`skip(&mut self, args: &[&dyn Any]) -> !` 对应 `t.Skip(...)`，返回类型 `!` 把“跳过后控制流不得继续”编码进类型系统。
- `fn parse_go_bool(value: &str) -> Option<bool>`：私有解析器，仅接受 Go `strconv.ParseBool` 所支持的十二种大小写固定拼写：真值 `1/t/T/true/TRUE/True`，假值 `0/f/F/false/FALSE/False`；其余返回 `None`。
- `pub fn short_from_args<I, S>(args: I) -> bool`：公开、泛型、可测试的 short 标志解析入口。输入项只需实现 `AsRef<OsStr>`；约定首项是可执行文件名。
- `pub fn under_short_with<T>(t, short, args)`：公开的可注入实现。无论 `short` 值为何都先调用 `helper`；仅在 `short == true` 时调用 `skip`。
- `pub fn UnderShort<T>(t, args)`：公开兼容入口；读取 `std::env::args_os()`，经 `short_from_args` 计算模式，再委托 `under_short_with`。`#[allow(non_snake_case)]` 只为保留 Go API 名称。
- `pub fn not_under_long_with<T>(t, long, args)`：公开的可注入实现。始终先调用 `helper`，仅在 `long == false` 时调用 `skip`。
- `pub fn NotUnderLong<T>(t, args)`：公开兼容入口；调用 `testflag::Long()` 读取进程的 `-long` 状态，再委托 `not_under_long_with`，同样局部允许非 snake case 名称。

文件中没有模块级常量、struct、enum、impl、异步函数或条件编译项；条件编译只出现在相邻 `lib.rs`，用于把独立测试文件装入 `#[cfg(test)]` 模块。

## 执行流程

`UnderShort` 的主流程如下：

1. 获取 `std::env::args_os()`；`short_from_args` 跳过 `argv[0]`，初始状态为 `false`。
2. 依次扫描剩余参数。裸标志 `-test.short`、`--test.short`、`-short`、`--short` 将状态设为 `true`；四种 `key=value` 写法在值合法时覆盖当前状态。因此多个合法同名标志以最后一个为准。
3. 非目标参数、非法布尔值和不能精确匹配的值不会修改状态，扫描仍继续。参数先经 `OsStr::to_string_lossy()` 转换；含不可解码字节的参数通常因替换字符而不能匹配。
4. `under_short_with` 先执行 `t.helper()`。非 short 模式直接返回；short 模式分配一个容量为 `args.len() + 1` 的局部向量，把 `"disabled under -short"` 放在首位，再保持顺序附加调用者参数，最后调用永不返回的 `t.skip(...)`。

`NotUnderLong` 不在本文件重复解析参数：它先调用相邻 crate `astersql-testkit-testflag` 的 `testflag::Long()`，后者以 `std::env::args_os()` 调用 `long_from_args`；然后 `not_under_long_with` 先标记 helper，在 `long == false` 时用固定前缀 `"disabled not under -short"` 加调用者参数并调用 `skip`。该前缀看似提到 short，但与 Go 源码完全一致，是兼容文本而非本文件新定义的含义。

## 数据与状态

本文件没有全局可变状态，也不缓存标志值。`short_from_args` 只有一个局部 `bool`；两个 `*_with` 函数只在确实跳过时创建局部 `Vec<&dyn Any>`。向量借用固定原因字符串和调用者传入的参数，不取得参数所有权，并在 `skip` 终止控制流前保持有效。

`&[&dyn Any]` 保留 Go `...any` 的异构参数能力，但本文件不读取、克隆或格式化这些值。实际如何显示由 `TestContext::skip` 的实现决定。`&mut T` 保证一次调用期间对测试上下文的独占可变访问；`T: ?Sized` 允许具体类型或 trait object 适配器。

## 依赖与调用关系

上游关系：

- `lib.rs` 在 crate 根重导出 `TestContext`、两组公开入口和可注入辅助函数。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 是 RustCodeGraph 找到的唯一 Rust 使用方：它实现 `RecordingTest: TestContext`，直接调用 `short_from_args`、`under_short_with`、`not_under_long_with`。
- Go 版本有真实测试调用者：`pkg/executor/test/unstabletest/memory_test.go` 用 `skip.UnderShort(t)` 避免 short 模式执行耗时内存测试；`pkg/ttl/ttlworker/job_manager_integration_test.go` 用 `skip.NotUnderLong(t)` 把长时间故障测试限制到 long 模式。这些 Go 调用说明包的应用位置，但不是 Rust 已接线的证据。

下游关系：

- `UnderShort -> short_from_args(std::env::args_os()) -> under_short_with -> TestContext::{helper, skip}`。
- `NotUnderLong -> testflag::Long() -> not_under_long_with -> TestContext::{helper, skip}`。其中 `testflag::Long()` 定义在 `pkg/testkit/testflag/flag.rs`。
- `short_from_args -> parse_go_bool` 只发生在 `key=value` 分支。

RustCodeGraph 正确识别了两个 `*_with` 到 `helper`/`skip` 以及 Rust `NotUnderLong` 到 `not_under_long_with` 的调用边；它还把迭代器的 `.skip(1)` 误配成了 `TestContext::skip`，该歧义边未用于本文结论。

## 错误处理与边界

这里没有 `Result` 或显式错误类型。`short_from_args` 对未知参数、非法布尔值采取忽略并继续扫描的策略，默认值保持 `false`；这使它宽容，但也意味着拼写错误不会被报告。裸标志只能表达 `true`，显式关闭必须使用合法的 `=false` 类写法。

`args` 为空是合法输入：跳过时仍会传递仅含固定原因的一项切片。额外参数保持原顺序且不被解释。`helper` 在条件判断前无条件调用，所以即使不跳过也要求上下文能接受 helper 标记；测试对此有明确断言。

`TestContext::skip -> !` 是关键边界：实现若通过 panic、测试框架的控制流机制或其他方式终止均可，但绝不能返回。独立测试以 `panic_any` 模拟终止，并用 `catch_unwind` 断言调用方没有继续执行；这只是测试适配方式，不表示所有生产适配器必须 panic。

## 并发与资源生命周期

所有逻辑同步执行，不创建线程、异步任务、锁、通道、文件句柄或网络资源。函数每次调用重新读取或接收标志状态，因此没有跨测试共享的内部缓存，也没有初始化/关闭协议。

并发测试进程通常共享不可变的启动参数，读取 `std::env::args_os()` 本身不修改状态；但具体 `TestContext` 是否可跨线程使用不由本 trait 保证，因为它没有 `Send`/`Sync` 约束，且 API 要求 `&mut T`。跳过分支的 `Vec` 只活到 `skip` 接管控制流，不会被存储；若未来适配器需要异步保存参数，就不能直接保留这些借用，必须先转成自有数据。

## 与 Go 版本的对应关系

[`skip.go`](./skip.go) 定义相同的两个公开概念：`UnderShort` 先 `t.Helper()`，在 `testing.Short()` 为真时以 `"disabled under -short"` 为首参数调用 `t.Skip`；`NotUnderLong` 在 `testflag.Long()` 为假时以 `"disabled not under -short"` 为首参数跳过。Rust 的 `UnderShort`/`NotUnderLong`、无条件 helper 调用、判断极性、固定文案和附加参数顺序均与之对应。

Rust 为可测试性增加了 Go 文件中没有的边界：`TestContext` trait、显式布尔值的两个 `*_with` 函数，以及 `short_from_args`。Rust 不能直接使用 Go `testing.T`，因此由调用方实现适配器；当前仓库只在迁移测试中提供 `RecordingTest`，尚未提供通用测试框架适配器。

解析层不是逐字等价实现。Go `UnderShort` 读取 `testing.Short()` 已注册的测试标志；Rust `short_from_args` 自行扫描参数，并额外接受用户侧 `short` 拼写、双横线写法和后值覆盖，同时忽略非目标/非法参数继续扫描。因而公开意图和已覆盖形式对齐，但遇到未知参数、位置参数或解析错误时不能据此宣称与 Go `flag`/测试驱动器完全等价。`NotUnderLong` 委托的 Rust `testflag::long_from_args` 则会在位置参数、`--`、未知或非法标志处停止，两个解析器的边界策略也不相同。

## 扩展指南

- 接入新的 Rust 测试框架时，优先新增独立适配/测试文件并实现 `TestContext`；实现必须先记录 helper 语义，并保证 `skip` 永不返回。不要把单元测试嵌回 `skip.rs`。
- 若增加新的 short 参数形式或改变冲突规则，应修改 `parse_go_bool`/`short_from_args`，并在 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 增加缺省、真假值、重复值、非法值、未知参数、位置参数和非 Unicode 参数等独立回归。需特别评估与 Go `testing.Short()` 的差异，而不是只让现有断言通过。
- 若调整跳过原因或附加参数布局，应同时修改 `under_short_with`/`not_under_long_with` 及对应测试，并核对 [`skip.go`](./skip.go)；这些字符串可能出现在测试日志或外部筛选脚本中，属于兼容风险。
- 若增加新的测试模式，应复用 `*_with` 的“显式状态 + 真实进程入口”分层，并在独立测试文件注入状态；涉及 `-long` 的解析规则应在 `astersql-testkit-testflag` 内维护，避免两个 crate 各自演化。
- 当前 API 在跳过分支会分配一个小向量。只有分析证明测试基础设施中该成本重要时才考虑改动；任何无分配方案仍须保持异构 `Any` 参数的顺序和借用生命周期安全。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `pkg/util/skip/{lib.rs,skip.rs,migration_aster_unit_test.rs,skip.go}`；`node --file pkg/util/skip/skip.rs` 覆盖源文件 125 行及全部 10 个索引符号。
- RustCodeGraph 符号/边查询：`short_from_args`、`under_short_with`、`not_under_long_with` 各只有一个 Rust 定义；两个 `*_with` 的下游包含 `TestContext::helper` 与 `TestContext::skip`；Rust `NotUnderLong` 的下游包含 `not_under_long_with`。调用者查询与文本引用搜索均只找到同目录迁移测试。
- 直接阅读：[`skip.rs`](./skip.rs)、[`Cargo.toml`](./Cargo.toml)、[`lib.rs`](./lib.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)、[`skip.go`](./skip.go)、`pkg/testkit/testflag/flag.rs`、根 `Cargo.toml`、`pkg/util/skip/BUILD.bazel`。
- Go 使用面核验：`pkg/executor/test/unstabletest/memory_test.go` 的两个内存测试调用 `UnderShort`；`pkg/ttl/ttlworker/job_manager_integration_test.go::TestJobManagerWithFault` 调用 `NotUnderLong`。
- 独立 Rust 测试覆盖的事实：非 short/short 分支、helper 次数、固定原因与附加参数顺序、long/非 long 分支、Go 布尔拼写与最后值优先，以及 `skip` 不返回。按任务约束本次不运行 Cargo，故这里只记录源码级测试证据，不声称执行了测试。
