# `pkg/types/compare.rs`

## 文件定位

该文件实现 AsterSQL Rust 类型系统中最基础的整数与字符串三路比较。它的直接归属不是根 `astersql-types` crate：`pkg/types/internal/scalar/lib.rs` 以 `#[path = "../../compare.rs"] mod compare` 挂载本文件，并用 `pub use compare::*` 导出；根 `pkg/types/lib.rs` 再把这个子 crate 以 `scalar` 模块别名重导出。因此直接依赖 `astersql-types-scalar` 的调用方可在 crate 根使用这些函数，经根 `astersql-types` 则应走 `scalar::...` 路径；根 crate 只显式平铺重导出了部分 Context/Flags 符号，没有平铺本文件函数。

本文件对应 Go 的 `pkg/types/compare.go`，只负责四种整数向量组合、带校对规则的字符串比较和带 signed/unsigned 标志的标量整数比较。Datum 跨类型比较、NULL/哨兵值排序等更高层逻辑位于其他模块；虽然 `pkg/types/compare_test.rs` 同时测试这些行为，但不能据此把它们归为本文件职责。

## 核心职责

- 把 Rust `std::cmp::Ordering` 统一映射为 Go/TiDB 约定的 `-1`、`0`、`1`，由私有函数 `ordering` 完成。
- 由 `VecCompareUU`、`VecCompareII`、`VecCompareUI`、`VecCompareIU` 对等长整数切片逐元素比较，并把三路结果写入调用方提供的 `&mut [i64]`。
- 在混合有符号/无符号比较时先处理负数和超出 `i64::MAX` 的无符号值，避免先强制转换导致环绕后得到错误次序。
- 由 `CompareString` 将具体字符串次序委托给 `astersql-util-collate`，使比较遵循调用方指定的 collation。
- 由 `CompareInt` 在一个标量入口中按两个布尔 unsigned 标志选择 UU、UI、IU 或 II 语义。

## 主要符号

- `fn ordering(value: std::cmp::Ordering) -> i32`：文件内私有适配器；`Less`、`Equal`、`Greater` 分别变为 `-1`、`0`、`1`。RustCodeGraph 显示本文件五个整数比较入口都调用它。
- `pub fn VecCompareUU(x: &[u64], y: &[u64], res: &mut [i64])`：无符号对无符号的逐元素比较。
- `pub fn VecCompareII(x: &[i64], y: &[i64], res: &mut [i64])`：有符号对有符号的逐元素比较。
- `pub fn VecCompareUI(x: &[u64], y: &[i64], res: &mut [i64])`：无符号左值对有符号右值；右值为负或左值大于 `i64::MAX` 时直接写 `1`。
- `pub fn VecCompareIU(x: &[i64], y: &[u64], res: &mut [i64])`：有符号左值对无符号右值；左值为负或右值大于 `i64::MAX` 时直接写 `-1`。
- `pub fn CompareString(x: &str, y: &str, collation: &str) -> i32`：取得 `collate::GetCollator(collation)` 后调用其 `Compare`。
- `pub fn CompareInt(arg0: i64, isUnsigned0: bool, arg1: i64, isUnsigned1: bool) -> i32`：根据 unsigned 标志的四种组合执行标量三路比较；标记为 unsigned 的 `i64` 参数按其二进制位重解释为 `u64`。

文件没有模块级常量、结构体、枚举、trait、`impl` 或条件编译项；条件编译只出现在挂载它的测试/模块入口中。

## 执行流程

向量函数的共同流程是：以 `x.len()` 为循环边界读取同一索引处的左右元素，确定三路次序，再写入 `res[index]`。UU/II 直接使用同型整数的 `cmp`；UI/IU 先检查无法安全落入共同有符号域的边界，只有双方都可表示为非负 `i64` 时才转换并调用 `cmp`。这保证 `u64::MAX` 永远大于任意 `i64`，任意负 `i64` 永远小于任意 `u64`。

`CompareInt` 对 `(isUnsigned0, isUnsigned1)` 做四分支匹配。UU 分支把两个参数都重解释为 `u64`；UI/IU 分支复用与向量版本相同的负数/上界判定；II 分支直接比较两个 `i64`。所有可比较分支最后经 `ordering` 归一化结果。

`CompareString` 不自行标准化大小写、重音或尾随空格，而是把三个入参直接交给校对模块。生产侧 `pkg/util/chunk/compare.rs` 的 `genCmpStringFunc` 捕获字段 collation，随后 `cmpStringWithCollationInfo` 在排除 NULL 后调用本函数；同文件的 Row-versus-Datum `Compare` 也使用 Datum 自带的 collation 调用它。

## 数据与状态

本文件没有全局可变状态，也不保存缓存。所有整数输入以借用切片或按值标量传入；向量结果由调用方预分配并通过可变切片传入。每个输出只依赖相同索引的两个输入值，结果域固定为 `{-1, 0, 1}`。

`CompareInt` 的 unsigned 标志是解释位模式的组成部分：例如 `arg0 == -1` 且 `isUnsigned0 == true` 表示 `u64::MAX`，不是负数。`pkg/types/binary_literal_1_aster_unit_test.rs` 用 `CompareInt(-1, true, -1, true) == 0` 和混合符号用例固定了这一语义。

向量 API 隐含长度不变量：`y` 与 `res` 至少要有 `x.len()` 个元素。实现不检查等长，也不调整 `res` 长度；多余的 `y`/`res` 元素不会被访问或写入。

## 依赖与调用关系

crate 接线链为 `pkg/types/internal/scalar/lib.rs` → `compare.rs` → `pub use compare::*`，随后 `pkg/types/lib.rs` 通过 `pub use types_group_1 as scalar` 把整个标量 crate 暴露为子模块；根文件另行平铺的列表不含本文件函数。直接承载本文件的 `pkg/types/internal/scalar/Cargo.toml` 声明 `collate = astersql-util-collate`；根 `pkg/types/Cargo.toml` 依赖该 scalar crate，也声明同一 collate 工作区路径依赖。两份 manifest 都没有为本文件设置专属 feature；比较 API 默认可编译，`types-integration` feature 只控制其他标量类型的额外再导出。

RustCodeGraph 的 callee 边确认 `VecCompareUU`、`VecCompareII`、`VecCompareUI`、`VecCompareIU`、`CompareInt` 调用私有 `ordering`。工具未解析出 `CompareString` 经动态 collator 的方法边，但源码直接显示 `GetCollator(...).Compare(...)`。

Rust 源码中的生产调用形状包括 `pkg/util/chunk/compare.rs` 中字符串列比较和 Row-versus-Datum 字符串分支。不过该 crate 的 manifest 当前把局部名 `types` 指向 `astersql-types-datum`，而后者的显式 `types_group_1` 再导出列表未包含 `CompareString`；在本任务禁止运行 Cargo 的前提下，不能把这个文本调用点进一步断言为已编译接通，应把它视为需要后续构建验证的现有接线疑点。`pkg/expression/generator/compare_vec.rs` 与 `pkg/expression/generator/other_vec.rs` 生成包含 `types.CompareString(...)` 的 Go 源码文本，不是运行时 Rust 调用。当前搜索未发现四个 `VecCompare*` 或 `CompareInt` 的非测试 Rust 直接调用；它们仍作为 Go 对齐的公开 API，被独立 Rust 测试覆盖。RustCodeGraph 的文件级关系报告 `compare.rs` 被 18 个文件使用，但逐符号 callers 对同名 Go/Rust 定义未给出可用边，因此上述直接调用结论由精确源码搜索补足。

## 错误处理与边界

这些函数都不返回 `Result`。整数比较对全部 `i64`/`u64` 值都有确定结果，关键边界是 `i64::MAX` 与 `i64::MAX + 1`：后者不能转换为非负 `i64` 后再比较，UI/IU 会提前返回确定次序。`pkg/types/compare_test.rs::test_vec_compare_int_and_uint` 明确覆盖了负有符号数和 `i64::MAX + 1`；`pkg/types/binary_literal_1_aster_unit_test.rs::vector_scalar_and_collation_comparisons_match_go` 还覆盖标量位模式解释与 collation 行为。

向量函数在 `y` 或 `res` 短于 `x` 时会因切片索引越界而 panic；这是当前实现与 Go 版本由调用方保证长度的契约，不是可恢复错误。若 `x` 为空则循环不执行。输入与输出切片因 Rust 借用规则不能安全地互相别名为同一存储；函数本身不分配内存。

`CompareString` 对未知 collation 的具体回退或选择规则由 `collate::GetCollator` 决定，本文件不验证名称也不包装下游行为。这里能确认的是结果原样返回，不能把 collate crate 的全部错误/兼容策略归因于本文件。

## 并发与资源生命周期

所有函数都是同步、无锁、无异步任务、无通道、无事务、无 I/O 的纯计算；唯一外部动作是 `CompareString` 获取 collator 并立即比较。函数不持有传入引用，返回后没有延续的资源生命周期。

不同线程可并行调用这些 API，因为本文件不读写共享状态。单次向量调用对 `res` 拥有独占可变借用，循环按索引顺序写入；实现没有内部并行化或 SIMD。时间复杂度为整数向量 `O(x.len())`、额外空间 `O(1)`；字符串比较复杂度和临时资源取决于所选 collator。

## 与 Go 版本的对应关系

`pkg/types/compare.go` 与本文件具有相同的六个公开函数和相同参数角色。Go 使用显式 `<`/`==` 分支或标准库 `cmp.Compare`，Rust 用整数的 `Ord::cmp` 加 `ordering`，结果语义等价。混合符号版本在两种语言中都先检查负数和 `math.MaxInt64`/`i64::MAX`，再做安全转换；`CompareInt` 也都把标记为 unsigned 的负 `int64`/`i64` 位模式解释为大 `uint64`/`u64`。

字符串路径均为 `collate.GetCollator(collation).Compare(x, y)`。Go 注释提到“specified collation and length”，但函数签名没有独立长度参数，Rust 也没有增加该概念。

`pkg/types/compare_test.go::TestVecCompareIntAndUint` 与 Rust 的 `pkg/types/compare_test.rs::test_vec_compare_int_and_uint` 使用相同类别的 UU/II/IU/UI 表格，包括 `MaxInt64 + 1` 边界。Rust 测试文件中的大部分 `test_compare`/`test_compare_datum` 对应 Go 的 Datum 比较测试，属于相邻类型系统的移植证据而非本文件实现覆盖；本文件自己的直接核心覆盖集中在向量测试，另由 `binary_literal_1_aster_unit_test.rs` 覆盖 `CompareInt` 和 `CompareString`。

## 扩展指南

- 若调整整数比较规则，应同时修改 `CompareInt` 与对应的 `VecCompare*` 分支，避免标量和向量语义分叉；尤其要保留负数、`i64::MAX`、`i64::MAX + 1`、以及 unsigned 标志下负 `i64` 位模式的用例。
- 若新增一种整数向量组合或改变长度契约，应在独立的 `pkg/types/compare_test.rs` 增补测试，不要把测试嵌入生产源文件；需明确决定长度不匹配是继续 panic、截断还是返回错误，因为这会改变公开 API 和性能特征。
- 若改变字符串比较入口，应同步检查 `pkg/util/chunk/compare.rs` 的两个直接调用点，以及表达式生成器中生成的 `types.CompareString` 调用形状；collation 兼容性测试应覆盖大小写、重音、尾随空格和 binary collation，而不是在本文件复制排序表。
- 与 Go 对齐的修改应同步核对 `pkg/types/compare.go` 和 `pkg/types/compare_test.go`。不得仅为 Rust 测试通过而删减 Go 已有分支；性能修改还应保持逐元素无分配，并评估 chunk/表达式比较热路径。
- 若移动本文件或拆分模块，必须更新 `pkg/types/internal/scalar/lib.rs` 的 `#[path]` 接线以及相应 Cargo 依赖；根 `pkg/types/lib.rs` 当前依赖的是 scalar crate 的再导出，不应误加第二份重复实现。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/types/compare.rs` 识别 8 个节点；`node --file pkg/types/compare.rs` 核对了 93 行完整实现和文件级使用关系；对 `ordering`、六个公开函数执行了 `query`、`node`、`callers`、`callees`，其中整数入口到 `ordering` 的调用边可见，逐符号 caller 边为空/受同名定义影响。
- 源码与接线：`pkg/types/compare.rs`、`pkg/types/internal/scalar/lib.rs`、`pkg/types/lib.rs`、`pkg/types/internal/scalar/Cargo.toml`、`pkg/types/Cargo.toml`。
- Rust 上游、接线核对与生成器：`pkg/util/chunk/compare.rs`、`pkg/util/chunk/Cargo.toml`、`pkg/types/internal/datum/lib.rs`、`pkg/expression/generator/compare_vec.rs`、`pkg/expression/generator/other_vec.rs`。
- Rust 测试：`pkg/types/compare_test.rs`、`pkg/types/binary_literal_1_aster_unit_test.rs`；测试与生产源文件分离。
- Go 对照：`pkg/types/compare.go`、`pkg/types/compare_test.go`，并由 RustCodeGraph 文件节点核对源码。
- 本任务是纯文档分析，按计划不运行 Cargo；交付验证以固定章节结构、路径/符号链接和人工事实复核为准。
