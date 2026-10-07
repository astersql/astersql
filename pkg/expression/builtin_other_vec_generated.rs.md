# `pkg/expression/builtin_other_vec_generated.rs`

## 文件定位

本文件属于 `astersql-expression` crate，是 SQL `IN` 谓词的 Rust 向量化实现之一。crate 根在 `pkg/expression/lib.rs` 中通过 `#[path = "builtin_other_vec_generated.rs"] mod builtin_other_vec_generated_kernel;` 私有挂载它；依赖的列式求值接口和结果写入工具来自相邻的 `builtin_other_vec.rs`（模块名 `builtin_other_vec_kernel`）。`pkg/expression/Cargo.toml` 将该目录定义为 `astersql-expression` 库，并通过 `chunk-dependency`、`collate-dependency`、`types-dependency` 分别提供列批、排序规则和 SQL 类型能力。

文件头明确标记为生成代码，对应 Go 文件 `pkg/expression/builtin_other_vec_generated.go`。不过当前 Rust 文件不是空门面：它包含共享求值内核和 Int、String、Decimal、Real、Time、Duration、JSON 七类签名的完整实现。需要同时注意当前接线边界：这些类型虽然声明为 `pub`，所在模块在生产 crate 中仍是私有模块；`lib.rs` 只在 `#[cfg(test)]` 的 `expression_other_vec` 测试门面中再导出它们。RustCodeGraph 显示目标文件直接被 `pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs` 使用，仓库搜索也未发现排除本文件及其两份测试后还有 Rust 构造点，因此“生产表达式注册/构造已接通”没有证据，不能由 Go 现状推断。

## 核心职责

- `evaluate_in` 统一实现批量 `IN` 的三值逻辑：按列求左操作数，先探测常量集合，再逐个求值动态右操作数，最终为每行输出 `1`、`0` 或 SQL `NULL`。
- 常量右参数可预处理为类型专用集合，避免每批重复计算；动态右参数仍按整列求值。是否采用常量快路径由集合是否非空决定（`hash_present`）。
- 各签名只注入四类差异：子表达式的向量求值方法、列值读取方法、常量集合探测方法、动态值相等判定。这样把 NULL 传播、结果短路和行数检查集中在一个内核中。
- 保留 Go 的关键比较语义：整数有符号/无符号兼容规则、字符串 collation、Decimal 哈希键、浮点 NaN 与正负零、Time 的 `CoreTime`、Duration 的整数时长、JSON 的二进制 JSON 比较。

## 主要符号

- `type Expressions = Vec<Box<dyn VectorExpression>>`：异构表达式列表。`VectorExpression` 定义在 `builtin_other_vec.rs:148`，要求实现者为 `Send + Sync`，并按结果类型提供 `vec_eval_*` 方法。
- `evaluate_in<T, Evaluate, Read, HashMatch, Equal>(...) -> EvalResult<()>`：私有共享内核。四个闭包把类型专用行为参数化；`T: Clone`，但函数本身不保存跨调用状态。
- `int_equal(left, left_unsigned, right, right_unsigned)`：整数相等判定。相同符号属性直接比较；跨符号比较要求有符号一侧非负。
- `BuiltinInIntSig`：常量表为 `HashMap<i64, bool>`，值记录该常量是否无符号；动态比较还会读取每个右表达式的 `is_unsigned()`。
- `BuiltinInStringSig`：常量按 `collate::GetCollator(...).Key(...)` 转为 `HashSet<Vec<u8>>`；动态比较调用同一 collator 的 `Compare`。
- `BuiltinInDecimalSig`：常量用 `MyDecimal::ToHashKey` 建集合，并用 `hash_error: Option<String>` 保存构造期哈希失败，延迟到求值时返回；动态比较使用 `Compare`。
- `BuiltinInRealSig`：常量使用 `Vec<f64>` 而不是哈希集合，以模拟 Go `map[float64]` 的查找语义；常量探测用普通 `==`，所以 NaN 不命中，正负零相等。动态比较额外把两个 NaN 判为相等，对应 Go `cmp.Compare` 的 NaN 规则。
- `BuiltinInTimeSig`：常量集合键是 `Time::CoreTime().0`，动态比较调用 `Time::Compare`。
- `BuiltinInDurationSig`：常量集合键为 `i64`，列值也从 `Column::GetInt64` 读取。
- `BuiltinInJsonSig`：不建常量哈希，全部右参数动态扫描并调用 `types::CompareBinaryJSON`。`BuiltinInJSONSig` 是与 Go 大写命名对应的公开类型别名。
- 每个签名的 `new` 保存表达式和预处理元数据；`vectorized()` 固定返回 `true`；`vec_eval_int()` 选择类型回调后进入 `evaluate_in`。

## 执行流程

1. 调用某个 `BuiltinIn*Sig::vec_eval_int`。签名根据自身类型准备求值、读取、常量探测和动态相等闭包；整数签名还先从第一个表达式读取左值的无符号属性，字符串签名取得 collator，Decimal 签名先检查延迟错误。
2. `evaluate_in` 要求 `args.first()` 存在，否则返回 `EvalError::Message("IN requires at least one argument")`。
3. 内核以 `input.NumRows()` 为期望行数，把第一个表达式整列求值到临时 `left` 列；若实际长度不等于输入行数，立即报错。
4. 初始化 `output = vec![None; row_count]`。`None` 暂时代表“结果尚未确定”；`has_null` 则用 `constant_has_null` 为每行建立初值，记录比较链中是否见过 NULL。
5. 若常量集合非空，逐行探测左值。左值为 NULL 时只设置 `has_null`；常量命中时把该行置为 `Some(1)`。
6. 决定动态参数范围：存在常量快路径时仅使用 `non_const_args_idx`；集合为空时忽略该索引表并扫描 `args[1..]`。这与 Go 代码在有 `hashSet` 时重建非常量参数列表、否则扫描全部右参数的分支一致。
7. 每个动态参数整列求值到复用的 `right` 临时列，并检查行数。对已命中的行短路；左右任一为 NULL 时记录 `has_null`；否则调用类型专用 `equal`，命中则写 `Some(1)`。
8. 扫描完成后，仍未命中且没有见过 NULL 的行改成 `Some(0)`；其余未命中行保持 `None`。最后由 `write_int_options` 重置结果列并依次追加整数或 NULL。

这套流程实现 SQL 三值逻辑：任何匹配优先得到 true；没有匹配且候选中出现 NULL 得到 NULL；只有能够证明所有参与比较的值均非 NULL 且均不相等时才得到 false。

## 数据与状态

每个签名持有不可变的构造后状态：表达式所有权 `args`、常量预处理数据、常量中是否含 NULL 的 `has_null`（JSON 除外）以及非常量参数下标。求值方法只借用 `&self`，不会更新签名。每次调用临时创建左/右 `chunk::Column`、逐行 `output` 和 `has_null` 向量；这些对象在调用结束时释放，不跨批次缓存。

常量集合的结构决定语义和复杂度：Int/String/Decimal/Time/Duration 通常为均摊 O(1) 探测；Real 因使用 `Vec<f64>` 为 O(C) 线性探测，其中 C 是常量数；JSON 没有常量快路径。动态阶段最坏为 O(R×D)，R 为行数、D 为动态右参数数，并会为每个动态参数调用一次整列求值。命中行在后续动态参数中会跳过比较，但当前实现仍会求完整个右列。

`non_const_args_idx` 存储的是 `args` 的绝对下标而非右参数切片下标。它只在常量集合非空时生效；空集合会使内核回退到扫描所有右参数。`constant_has_null` 是每行 NULL 状态的初值，而不是结果本身：后续匹配仍可把结果覆盖为 true。

## 依赖与调用关系

上游方面，`pkg/expression/lib.rs:251-252` 将文件编译进 `astersql-expression`。RustCodeGraph 对目标文件报告的直接使用文件是 `pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs`；另有 `pkg/expression/builtin_other_vec_generated_test.rs` 通过同一私有模块测试整数签名。`lib.rs:564-568` 仅在 `#[cfg(test)]` 下挂载这两份测试，`lib.rs:814` 也只在测试辅助模块中再导出生成签名。排除目标文件和两份测试后的 Rust 搜索没有 `BuiltinIn*Sig` 使用点，因此当前没有可证实的 Rust 生产构造者或注册器。

下游方面：

- `evaluate_in` 调用 `VectorExpression::vec_eval_int/string/decimal/real/time/duration/json` 取得子表达式列；该 trait 默认对不支持的类型返回 `EvalError::Unsupported`。
- 它使用 `chunk::Chunk` 和 `chunk::Column` 获取行数、NULL 位与类型值，并调用 `builtin_other_vec.rs:134` 的 `write_int_options` 写回结果。
- String 依赖 `collate::GetCollator` 的 `Key` 与 `Compare`；Decimal 依赖 `MyDecimal::ToHashKey` 和 `Compare`；Time 依赖 `CoreTime` 与 `Compare`；JSON 依赖 `types::CompareBinaryJSON`。
- `std::collections::HashMap/HashSet` 保存大多数常量快路径；Real 刻意不用哈希集合。

RustCodeGraph 的调用边确认 `evaluate_in -> write_int_options`，并把各 `vec_eval_int -> evaluate_in` 识别为内部调用。由于多个同名 `vec_eval_int`，图查询会折叠同名结果，源码中的七处调用是更精确的逐类型证据。

## 错误处理与边界

- 参数为空：`evaluate_in` 返回带明确信息的 `EvalError::Message`，不会索引越界。
- 子表达式错误：所有 `vec_eval_*` 错误通过 `?` 原样传播；例如类型不支持会从 `VectorExpression` 返回 `EvalError::Unsupported`。
- 行数不一致：左表达式和每个动态右表达式都必须产生恰好 `input.NumRows()` 行，否则返回包含实际值、期望值和参数下标的错误。
- 非常量下标非法：通过 `args.get(arg_index)` 检查并返回错误；只有通过检查后，整数动态比较闭包才安全地使用相同下标访问 `self.args[arg_index]`。
- Decimal 常量哈希错误：构造器不能返回 `Result`，因此保存首个错误字符串；`vec_eval_int` 在任何列求值前返回它。运行期对左值计算哈希键的错误同样转换为 `EvalError::Message`。
- NULL：左值 NULL、动态右值 NULL、或常量集合预处理时记录的 NULL 都会影响未命中结果；任何实际匹配仍优先返回 1。
- JSON：构造器没有 `has_null` 元数据和常量集合，所有传入右表达式都按动态参数处理；调用方不能通过此类型单独传入“已折叠的 NULL 常量”状态。
- Real：常量 NaN 不命中自身，动态 NaN 与 NaN 相等；这是刻意复现 Go 常量 map 查找与 `cmp.Compare` 动态比较之间的差别，不应统一成一种比较器。
- 空常量集合：`hash_present` 为 false，内核会扫描所有 `args[1..]`。因此若调用方同时提供了 `non_const_args_idx`，该列表在此分支不会限制扫描范围。

## 并发与资源生命周期

文件不创建线程、异步任务、锁、通道、事务或外部 I/O。签名的求值接口使用 `&self`，其字段在构造后不变；表达式 trait 本身要求 `Send + Sync`，所以类型层面允许表达式对象跨线程共享，但这里没有负责调度并发，也没有声明同一个可变结果列可被并发写入。调用者必须独占传入的 `&mut chunk::Column`。

与 Go 版本不同，Go 用 `bufAllocator.get/put` 借还两个临时列并以 `defer` 保证归还；Rust 当前每次调用创建默认 `Column`，依靠所有权在作用域结束时释放。`output`、`has_null` 和动态索引副本同样是调用级分配。字符串构造时生成常量排序键，求值时为探测值生成键；其生命周期均由签名或单次调用管理。

## 与 Go 版本的对应关系

`pkg/expression/builtin_other_vec_generated.go` 为直接语义对照。两端均为七类 IN 签名，均先求左列、常量命中优先、只在有常量集合时按 `nonConstArgsIdx` 缩减动态参数、按列扫描动态参数，并在末尾落实 true/false/NULL。

主要对应关系如下：

- Go 的 `builtinInIntSig`/`builtinInStringSig` 等对应 Rust 的 `BuiltinInIntSig`/`BuiltinInStringSig` 等；Go 的 `vecEvalInt` 对应 Rust 的 `vec_eval_int`，`vectorized` 均返回 true。
- Go 各函数重复展开模板逻辑；Rust 用泛型 `evaluate_in` 合并共同流程，再由闭包保留类型差异。
- Go Int 使用 `map[int64]bool` 同时编码值和 unsigned 标志；Rust 使用相同概念的 `HashMap<i64, bool>` 与 `int_equal`。
- Go String 的常量和动态比较均受 collation 控制；Rust 分别使用 collator key 与 `Compare`。
- Go Decimal、Time、Duration、JSON 的键或比较方法在 Rust 中分别对应 `ToHashKey`、`CoreTime`、`i64` 时长、`CompareBinaryJSON`。
- Go Real 常量集合是 `map[float64]struct{}`，NaN 查找不命中；动态路径用 `cmp.Compare`，两个 NaN 比较相等。Rust 的 `Vec` 常量探测和显式 NaN 动态分支保留了这一非直观差别。
- Go 复用 allocator 缓冲列；Rust 当前按调用分配临时列。Rust 还增加了空参数、错误下标和行数不一致的显式防御检查，这是其本地接口所需的边界保护。

Go 测试 `pkg/expression/builtin_other_vec_generated_test.go` 用通用随机/基准框架覆盖七种类型的动态与常量场景，并比较标量、向量结果。Rust 的 `builtin_other_vec_generated_21_aster_unit_test.rs` 对七种类型、NULL、collation、有符号性、NaN 等做确定性断言；较小的 `builtin_other_vec_generated_test.rs` 专门覆盖整数常量哈希、动态命中和符号边界。两者测试风格不同，但共同验证核心语义。

## 扩展指南

新增一种 `IN` 求值类型时，优先复用 `evaluate_in`，并明确四项策略：调用哪个 `VectorExpression::vec_eval_*`、如何从 `Column` 读取值、常量能否建立与相等语义一致的键、动态相等是否存在特殊规则。若常量哈希与动态比较语义不同（如 Real/NaN），必须分别实现并增加针对差异的测试，不能为了复用而合并。

修改现有类型时应重点同步：

- Int：`int_equal`、哈希表中 unsigned 元数据、左右表达式的 `is_unsigned()`。
- String：构造期与求值期必须使用同一 collation；否则哈希命中与动态比较会分歧。
- Decimal/Time：哈希键必须与 `Compare == 0` 的等价类一致，并保留键生成错误传播。
- Real：保持 NaN 和 ±0 的 Go 兼容性，并评估线性常量探测的性能。
- JSON：若未来增加常量快路径，先证明序列化键与 `CompareBinaryJSON == 0` 完全等价，并设计常量 NULL 元数据。
- 通用内核：不能把“尚未决定”的 `None` 直接当 NULL；必须结合 `has_null` 在末尾区分 false 和 NULL。任何优化都要保留命中短路和行数/索引检查。

测试应放在独立文件而非生产源文件中。最低限度同步 `pkg/expression/builtin_other_vec_generated_test.rs` 或覆盖更全的 `pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs`；若改变 Go 对齐语义，还应核对 `pkg/expression/builtin_other_vec_generated_test.go` 的标量/向量对照用例。若要让这些签名进入 Rust 生产链，还需在表达式构造或注册层新增明确接线，并添加通过公开执行入口到本签名的集成测试，而不能仅依靠直接构造单测。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标文件被索引为 31 个符号。
- RustCodeGraph `node --file pkg/expression/builtin_other_vec_generated.rs`：逐行读取完整 539 行，确认 `evaluate_in`、`int_equal`、七个签名和 JSON 别名的实现。
- RustCodeGraph `node write_int_options` 与 `node VectorExpression`：确认结果列写入方式、`Send + Sync` 约束、各类型求值方法及默认错误行为；调用轨迹包含 `evaluate_in -> write_int_options`。
- RustCodeGraph `query builtin_other_vec_generated`、`query BuiltinInIntSig/BuiltinInJsonSig` 和 `explore`：确认目标符号、同路径 Go 文件、Go/Rust 测试以及内部调用关系；文件级关系报告直接使用者为 `pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs`。
- 已读取 `pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`，确认 crate 边界、依赖、生产私有挂载和测试期再导出；该包没有 `doc.go`。
- 已读取 Go 对照 `pkg/expression/builtin_other_vec_generated.go` 及 Go 测试 `pkg/expression/builtin_other_vec_generated_test.go`，确认七种类型、常量快路径、动态比较、NULL 和基准覆盖。
- 已读取独立 Rust 测试 `pkg/expression/builtin_other_vec_generated_test.rs` 与 `pkg/expression/builtin_other_vec_generated_21_aster_unit_test.rs`，确认整数专项边界及七类型确定性行为。
- 使用 `rg` 排除目标文件及两份测试后搜索所有 `BuiltinIn*Sig`，没有发现其他 Rust 使用点；因此生产构造接线记为“未发现”，而不是推断为已支持。
- 按任务约束未运行 Cargo；本任务只新增文档，验证以源码/调用图事实核对和固定章节结构检查为准。
