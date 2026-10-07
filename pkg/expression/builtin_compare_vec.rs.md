# `pkg/expression/builtin_compare_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate。`pkg/expression/Cargo.toml` 以 `lib.rs` 为 crate 入口，`lib.rs:181-182` 再用 `#[path = "builtin_compare_vec.rs"] mod builtin_compare_vec_kernel;` 将它编译为私有模块。它不是 SQL 表达式注册入口，也没有直接操作正式的 `Expression`、`Chunk` 或求值上下文；它把 Go 文件 `pkg/expression/builtin_compare_vec.go` 中最容易独立验证的列式比较算法抽成 Rust 数据结构和纯函数。

当前接线状态必须与“完整应用中的设计位置”区分：概念上它对应表达式执行链中的比较、`GREATEST`/`LEAST` 和 `INTERVAL` 向量内核；但仓库搜索只发现这些 API 被 `builtin_compare_vec_6_aster_unit_test.rs` 和 `builtin_compare_vec_test.rs` 调用，`builtin_compare_vec_kernel` 也只在 `lib.rs` 声明而未被生产模块再导出。因此当前事实是“随 crate 编译、用于迁移语义和独立测试的内核”，尚不能据此声称正式 Rust 表达式求值器会调用它。另一个私有模块 `builtin_compare_vec_generated_kernel` 承担生成式通用比较逻辑，二者不是互相调用关系。

## 核心职责

文件包含五组职责，均可从源码中的公开函数直接核验：

1. 用 `NullableColumn<T>` 和 `IntColumn` 表示固定宽度值与 SQL NULL 位图，并在构造时检查 NULL 下标。
2. 通过 `extremum_by` 为 decimal、`i64`、`f64`、字符串、时间和 duration 实现逐行 `GREATEST`/`LEAST`，保持任一参数为 NULL 时结果为 NULL。
3. 通过 `compare_int_columns` 保留 MySQL 有符号/无符号整数的四种组合顺序，并把原始三态比较映射成六种布尔比较或 NULL-safe equality（`<=>`）。
4. 通过 `interval_int`、`interval_real` 实现 `INTERVAL` 的逐行边界定位：边界可能为 NULL 时线性扫描，否则使用二分查找；目标为 NULL 时返回数值 `-1`。
5. 用 `Vectorized` 与 `vectorized_signatures!` 声明 Go 文件覆盖的 23 个签名具备向量化能力。这里的签名是无字段标记结构体，并非 `builtin_core::BuiltinFunc` 的具体运行时实现。

## 主要符号

- `CompareVecError`：错误枚举。`NoArguments` 表示极值聚合没有输入列，`LengthMismatch` 表示列行数不同，`InvalidNullIndex` 表示构造位图时下标越界，`Conversion` 承载调用方时间转换错误。它实现 `Display` 和 `Error`。
- `NullableColumn<T>`：公开 `values`、私有 `nulls` 的通用列。`new` 创建全非空列，`with_nulls` 校验并设置 NULL，`len`/`is_empty`/`is_null`/`null_indices` 提供只读观察。
- `validate_columns`、`extremum_by`：所有通用极值函数的共享校验和折叠内核；前者要求至少一列且全部等长，后者从第一列克隆结果并依次合并后续列。
- `greatest_decimal`/`least_decimal`、`greatest_i64`/`least_i64`、`greatest_f64`/`least_f64`：数值极值薄封装。浮点使用 Rust 直接 `<`/`>`，相等的有符号零和不可比较的 NaN 都保留先出现的值。
- `greatest_string_by`、`least_string_by`：由调用方注入 collation 比较器。候选与当前值 collation 相等时选择较后的候选，以对齐 Go `src` 只有严格胜出时才被保留的双缓冲分支。
- `CompareOp`、`map_compare_results`：表示 `Lt/Le/Eq/Ne/Gt/Ge/NullEq`，并将负数、零、正数映射成 SQL 整型布尔值 `0/1`。
- `IntValues`、`IntColumn`：分别保存 `Vec<i64>` 或 `Vec<u64>`，避免把 MySQL unsigned 值收窄。`signed`/`unsigned` 经私有 `build` 构造；`is_unsigned` 暴露符号属性。
- `compare_int_at`、`compare_int_columns`：前者按行完成 II/UU/UI/IU 比较，负有符号值与任意无符号值比较时直接决定次序；后者处理等长校验、NULL 传播及 `<=>` 特例。
- `interval_int`、`interval_real`：返回每行第一个严格大于目标的有效边界下标，若没有则返回边界数；目标 NULL 返回 `-1`。
- `string_extremum_as_time`、`greatest_string_as_time`、`least_string_as_time`：逐参数、逐行调用注入的字符串时间转换器，再比较规范化字符串。
- `time_extremum_by`、`greatest_time_by`、`least_time_by`：先做极值，再对结果向量的每个物理槽调用转换器，包括 NULL 位已置位的槽。
- `greatest_duration`、`least_duration`：对任意 `Clone + Ord` 的 duration 表示复用极值内核。
- `Vectorized`、`vectorized_signatures!`：trait 默认返回 `true`；宏生成 23 个零大小 `Builtin*Sig` 标记类型及其实现。

## 执行流程

极值流程从 `validate_columns` 开始：取第一列长度；没有第一列则返回 `NoArguments`；逐列核对长度后，`extremum_by` 克隆第一列作为结果。后续每个参数按行处理，如果结果此前已为 NULL 或当前参数为 NULL，就只把结果 NULL 位设为真；否则由注入谓词决定是否复制候选值。decimal、整数、浮点、duration 只是选择 `>` 或 `<`；字符串版本把判断替换为 collation 比较。字符串时间版本略有不同：它从空结果开始，对尚未成为 NULL 的每个值先执行转换，然后参与极值比较。

整数比较流程先由 `validate_int_columns` 保证左右等长，再对每行调用 `compare_int_at` 生成 `-1/0/1`。普通算子通过 `map_compare_results` 得到 `0/1`，结果 NULL 位是左右 NULL 位的或。`NullEq` 不传播 NULL：两边 NULL 得 `1`，仅一边 NULL 得 `0`，两边非 NULL 才检查原始比较值是否为零，返回列本身全非空。

`INTERVAL` 先验证目标列和每个边界列等长。每行目标 NULL 立即写 `-1`。`has_nullable == true` 时从头跳过 NULL 边界，遇到首个“目标小于边界”即停止；否则在假定边界有序的前提下做 upper-bound 风格二分，目标等于边界时继续向右。最终下标范围是 `0..=boundaries.len()`。

时间列流程由 `time_extremum_by` 先复用普通 NULL 传播和极值逻辑，再遍历结果的所有物理值运行 `convert_result`。这刻意复现 Go 在聚合后转换全部结果槽的顺序，所以即使某槽逻辑上是 NULL，其底层占位值转换失败仍会使整批求值返回错误。

## 数据与状态

全部状态都由调用栈和拥有所有权的 `Vec` 保存，没有全局变量、缓存或可变静态数据。`NullableColumn<T>` 的不变量是 `values.len() == nulls.len()`；其公开构造器维持该约束，但 `is_null(row)` 直接索引，调用方必须保证 `row < len()`。NULL 只影响语义有效性，不清除对应的底层值，因此转换钩子可能观察到 NULL 槽的占位值。

`IntColumn` 用私有 `IntValues` 把符号性固定在整列级别，`nulls.len()` 同时作为行数。`CompareOp` 是无状态值；比较输出沿用 SQL 的 `i64` 布尔表示。`INTERVAL` 输出没有 NULL 位图：`-1` 是目标 NULL 的业务结果，而不是 SQL NULL。

极值函数会克隆第一列以及胜出的候选值；字符串时间函数还会为转换后的字符串重新分配结果。宏生成的 23 个签名结构体是零状态标记，它们不保存参数、allocator、collation 或上下文。

## 依赖与调用关系

直接外部依赖只有 `rust_decimal::Decimal`，由 `pkg/expression/Cargo.toml` 的 `rust_decimal = "1.37"` 提供；其余依赖是标准库的 `Ordering`、`Error` 和格式化接口。该文件没有 feature 或条件编译项。

装配边为 `lib.rs -> builtin_compare_vec_kernel`。测试边为 `builtin_compare_vec_test.rs -> builtin_compare_vec_6_aster_unit_test.rs`（通过 `#[path] mod go_parity`）以及两者对私有内核的调用。仓库级 `rg` 未发现排除测试后的生产调用边，也未发现 `pub use builtin_compare_vec_kernel::*`；所以它当前不连接 `expression.rs`/`evaluator.rs` 的正式 `Vectorized()` 分派。`builtin_compare_vec_generated.rs` 虽处理其他类型的通用向量比较，但源码中也没有与本文件互调。

Go 对照的真实主链是 builtin 签名的 `vecEval*`：它先让参数表达式写入 `chunk.Column`，从 `bufAllocator` 借临时列，合并 NULL，再做比较并把结果列交回表达式执行器。本 Rust 文件只保留其中的算法层，未移植参数表达式求值、chunk allocator、warning/context、collation 获取或结果列复用，因此扩展者不能把这里的纯函数调用边等同于完整 SQL 执行边。

## 错误处理与边界

所有可能失败的构造、校验和转换函数返回 `Result<_, CompareVecError>`；错误不被吞掉或转成默认值。`Display` 给出可定位的期望/实际长度、非法下标或转换消息。普通比较和 `INTERVAL` 的逐值比较本身不返回错误。

重要边界包括：空极值参数返回 `NoArguments`；空但存在的第一列是合法零行输入；列长不一致在计算前失败；重复 NULL 下标合法且幂等；公开 `is_null` 越界会 panic。`map_compare_results` 接受任意负/零/正数，不要求恰好为 `-1/0/1`。`interval_*` 不验证边界排序，也不核对 `has_nullable` 是否真实反映位图；传错该标志可能破坏二分前提，责任在调用方。

浮点极值沿用 `<`/`>`，因此 NaN 不替换当前值，且 `-0.0`/`0.0` 相等时保留先出现的位模式。`interval_real` 对 NaN 同样按直接 `<` 判断，源码未提供额外总序。字符串 collation 函数可变且由调用方提供，其 panic 或不满足全序不被本层捕获。转换器一旦返回 `Conversion`，整批立即失败，不提供部分结果。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、channel、事务或外部句柄。输入以共享借用传入，输出拥有自己的向量；闭包以 `FnMut` 传入且仅在当前同步调用内使用，因此函数本身没有跨调用共享状态。类型是否可在线程间传递由泛型参数、闭包及 `Vec` 的自动 trait 决定，本文件没有额外声明 `Send`/`Sync`。

资源生命周期主要是内存所有权：`extremum_by` 克隆第一列作为可写结果；后续候选按需克隆；临时 `raw` 和输出向量在返回时转移给调用方。与 Go 版本的 `bufAllocator.get()/put()` 和 chunk 双缓冲不同，Rust 内核没有池化或显式归还动作，所以其分配成本与生产 Go 路径并不等价。

## 与 Go 版本的对应关系

`greatest_*`/`least_*` 对应 `builtin_compare_vec.go` 中各 `builtinGreatest*Sig.vecEval*`、`builtinLeast*Sig.vecEval*`：共同点是先取首参数、合并任意参数的 NULL、只在严格大于或小于时替换。字符串版本对相等值选后一个参数，来自 Go 中“不严格胜出就 append arg”的分支；独立测试 `collation_equal_string_extrema_choose_later_argument_like_go` 专门固定该语义。

`compare_int_at` 对应 Go `vecCompareInt` 调用的 `types.VecCompareUU/UI/IU/II`，`map_compare_results` 对应 `vecResOfLT/LE/EQ/NE/GT/GE`。`compare_int_columns` 的 `NullEq` 分支逐项复刻 `builtinNullEQIntSig.vecEvalInt` 的两 NULL、一 NULL和非 NULL判断。Rust 比 Go 多出显式等长错误，因为纯向量 API 没有 `Chunk.NumRows()` 提供统一长度保证。

`interval_int`/`interval_real` 对应两个 `builtinInterval*Sig.vecEvalInt`：目标 NULL 清除 NULL 标志并写 `-1`，`hasNullable` 决定 `linearSearch` 或 `binSearch`。Rust 为便于独立测试把所有边界预先表示成列；Go 则对当前 row 调用参数表达式的搜索方法，后者可能返回求值错误。

`greatest_string_as_time`/`least_string_as_time` 对应 `doTimeConversionForGL` 后的字符串字典序比较；`greatest_time_by`/`least_time_by` 对应先比较 `Time`、再对所有结果槽调用 `Convert`。duration 对应 Go duration 的直接大小比较。23 个标记签名逐一对应 Go 文件内返回 `true` 的 `vectorized()` 方法，但 Rust 标记没有 Go 签名对象的字段或 `vecEval*` 方法。

Go 测试 `builtin_compare_vec_test.go` 通过 `testVectorizedEvalOneVec` 和 `testVectorizedBuiltinFunc` 覆盖比较、极值、`INTERVAL` 及混合 unsigned 类型的完整表达式框架；Rust 的 `builtin_compare_vec_6_aster_unit_test.rs` 覆盖纯内核等价边界，`builtin_compare_vec_test.rs` 补充 collation 相等时选择后参数。两类测试的层级不同，Rust 测试不能证明 Go 式 chunk/allocator 接线已经移植。

## 扩展指南

新增比较算子时，应同时更新 `CompareOp`、`map_compare_results` 以及 `compare_int_columns` 对 NULL 的分类，并在独立文件 `builtin_compare_vec_6_aster_unit_test.rs` 增加负/零/正、双方 NULL 和单方 NULL 用例；不要把测试嵌入生产源文件。新增数值族极值优先复用 `extremum_by`，但先明确相等、NaN、符号零和 NULL 时是保留先参数还是后参数。

新增运行时接线不能只增加 `Builtin*Sig` 标记：需要在正式 builtin 类型上实现参数求值、类型/unsigned 标志、collation、错误上下文和结果列管理，并让 `expression.rs`/`evaluator.rs` 的正式接口能够到达它；同时核对 `builtin_compare_vec_generated.rs`，避免重复两套比较 API。若只扩展本纯内核，应保持 `NullableColumn`/`IntColumn` 长度不变量和“错误发生前完成校验”的顺序。

修改 `INTERVAL` 必须维护“首个严格大于目标的有效边界”以及等值向右的 upper-bound 语义，并分别测试 nullable 线性路径、非 nullable 二分路径、空边界、目标 NULL、混合 signed/unsigned 和未排序输入的契约处理。若引入自动检测 nullable，应消除目前由调用方保证 `has_nullable` 正确的风险。

时间和字符串扩展需同步 `builtin_compare_vec.go` 的转换顺序、collation 规则与错误传播，尤其不要为了跳过无效值而擅自停止转换 NULL 槽，否则会改变 Go 可观察错误。涉及性能时应测量克隆和分配，并考虑正式 chunk allocator 的生命周期；不能仅凭纯函数正确就宣称生产向量化路径可用。

## 验证依据

- 源码全读：`pkg/expression/builtin_compare_vec.rs`，核对了错误、列模型、极值、整数比较、`INTERVAL`、时间转换、23 个签名及无条件编译事实。
- crate/装配：`pkg/expression/Cargo.toml`（`astersql-expression`、`lib.rs`、`rust_decimal = "1.37"`）和 `pkg/expression/lib.rs:179-182,453-463`（生成式模块、本模块及独立测试模块声明）。
- Rust 测试：`pkg/expression/builtin_compare_vec_6_aster_unit_test.rs` 与 `pkg/expression/builtin_compare_vec_test.rs`，覆盖数值/字符串极值、signed/unsigned、六种结果映射、`<=>`、两条 `INTERVAL` 路径、转换错误、时间/duration、非法输入及 vectorized 标记。
- Go 对照：`pkg/expression/builtin_compare_vec.go` 与 `pkg/expression/builtin_compare_vec_test.go`；前者提供 `vecEval*`、`vecCompareInt`、`vecResOf*`、allocator 和转换顺序，后者列出完整向量表达式用例并调用统一向量测试框架。
- 调用边搜索：`rg` 结果表明 `compare_int_columns`、`interval_*`、各极值函数只在上述 Rust 独立测试出现；排除目标源和测试后，`builtin_compare_vec_kernel` 仅在 `lib.rs` 声明，未发现生产调用或再导出。因此文档将“尚未接入正式 Rust 求值主链”列为当前限制。
- RustCodeGraph：执行了 `rustcodegraph status`，索引可用（11,467 files、307,296 nodes、1,848,419 edges）；但 `files --filter pkg/expression/builtin_compare_vec`、`explore "pkg/expression/builtin_compare_vec.rs vectorized comparison builtins"` 和 `query builtin_compare_vec` 均未返回目标文件/符号，故按技能规则对该未覆盖文件使用直接源码与 `rg` 取证，不能声称获得图调用边。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另以任务指定命令检查目标存在且恰有 11 个固定二级标题，并人工复核上述定位、运行方式和安全扩展建议都能回指真实符号或文件。
