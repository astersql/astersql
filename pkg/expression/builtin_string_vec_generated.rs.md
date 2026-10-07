# `pkg/expression/builtin_string_vec_generated.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的 FIELD 函数向量内核，源文件通过 `pkg/expression/lib.rs:263-264` 以私有模块 `builtin_string_vec_generated_kernel` 装配。文件头声明其对应 `expression/generator` 的生成产物；当前 Rust 内容聚焦 FIELD 的整型、实数和字符串三条逐列求值路径，而不是 Go 版本完整的 `Expression`、`chunk.Column` 与缓冲区分配器适配层。

当前生产接线边界必须特别区分：仓库搜索只发现 `pkg/expression/builtin_string_vec_generated_test.rs` 和 `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs` 直接导入本模块；除 `lib.rs` 的模块声明外，没有 Rust 生产调用点。因此它是已实现、可由独立测试验证的内核，但尚不能据此断言已经进入 Rust SQL 表达式的生产求值主链。

## 核心职责

- `field_int`、`field_real`、`field_string` 接收一列待查值和若干候选列，逐行返回首次匹配候选的 1-based 位置；无匹配返回 `0`。
- `field_by` 统一实现三种类型共同的“按候选列优先、按行扫描、首匹配后跳过”算法。
- `validate_lengths` 在任何下标访问前确保所有候选列与 `search` 行数一致。
- `FieldVectorError` 提供当前唯一的结构错误：候选列行数不一致。
- 三个 `field_*_vectorized` 常量函数声明对应类型路径具备向量实现；它们与 Go 生成文件中三个签名的 `vectorized() bool { return true }` 对应。

该内核保持的关键 SQL 语义是：NULL 不与任何值相等、首次匹配胜出、位置从 1 开始、无匹配为 0。实数比较直接使用 IEEE `==`，因此 `NaN != NaN`，而 `-0.0 == 0.0`。

## 主要符号

- `pub enum FieldVectorError { RowCountMismatch }`：公开给 crate 内调用者/测试的可比较错误类型，派生 `thiserror::Error`、`Clone`、`Debug`、`PartialEq`、`Eq`；显示文本为 `FIELD vector columns have different row counts`。
- `fn validate_lengths<T>(search, candidates) -> Result<(), FieldVectorError>`：私有泛型前置校验。只检查行数，不检查候选列数量；零候选是合法输入。
- `fn field_by<T, F>(..., equal: F) -> Result<Vec<i64>, FieldVectorError>`：私有通用内核。比较器是 `FnMut(&T, &T) -> bool`，允许字符串路径携带可变比较状态，但本函数不保留比较器。
- `pub fn field_int(...)`：以 `i64::eq` 语义调用 `field_by`。
- `pub fn field_real(...)`：以 `f64` 直接相等语义调用 `field_by`。
- `pub fn field_string<'a, F>(...)`：接收借用字符串切片，并把调用方提供的 `FnMut(&str, &str)` 比较器转交给通用内核；collation 策略不在本文件硬编码。
- `pub const fn field_int_vectorized()`、`field_real_vectorized()`、`field_string_vectorized()`：均返回 `true`，是能力标记而非调度器。

文件没有模块级可变状态、trait、struct、宏或条件编译项。公开符号仍受 `lib.rs` 中私有模块边界限制，并未从 crate 根重新导出。

## 执行流程

1. 类型专用入口选择比较策略：整数和实数使用 `left == right`；字符串调用注入的比较器。
2. `field_by` 先调用 `validate_lengths`。任一候选列长度不同即立即返回 `RowCountMismatch`，不会产生部分结果或调用比较器。
3. 创建长度等于 `search.len()`、初始值全为 `0` 的 `Vec<i64>`。
4. 外层按候选列顺序遍历，并用 `candidate_index + 1` 形成 SQL 所需的 1-based 位置；内层遍历所有行。
5. 某行结果已经大于 0 时直接跳过，保证后续重复匹配不能覆盖首次匹配。
6. `search[row]` 或当前候选值为 `None` 时跳过比较，因此 NULL 始终保持未匹配。
7. 比较器返回 `true` 时记录当前位置；所有候选处理完毕后返回结果。

时间复杂度最坏为 `O(R × C)`（`R` 为行数、`C` 为候选列数），结果分配为 `O(R)`；已匹配行仍会在后续候选列经历一次常数时间的跳过判断。

## 数据与状态

输入采用列式只读切片：`search: &[Option<T>]` 表示第一参数列，`candidates: &[Vec<Option<T>>]` 表示其余参数列。`Option` 直接承载 SQL NULL；输出 `Vec<i64>` 不使用 NULL，因为 FIELD 对 NULL 搜索项也返回 0。

算法唯一的可变数据是函数栈上的 `result`、循环索引以及按值传入的 `FnMut` 比较器。输入不会被修改，输出拥有自己的存储。字符串入口用同一生命周期 `'a` 约束搜索列和候选列中的 `&str`，输出不借用输入，因此返回后没有资源绑定。

空输入得到空结果；非空搜索列配零候选列得到等长的全零结果。这两项由 `builtin_string_vec_generated_test.rs:174-175` 明确断言。

## 依赖与调用关系

crate 边界由 `pkg/expression/Cargo.toml` 确认：包名为 `astersql-expression`，库入口是 `lib.rs`，`autotests = false`；本文件直接使用的唯一外部 crate 是依赖表中的 `thiserror = "2"`。

下游调用边为：`field_int`、`field_real`、`field_string` → `field_by` → `validate_lengths`；`field_by` 还调用传入的比较器。没有 IO、存储、session、planner 或 executor 依赖。

上游直接证据为：

- `pkg/expression/lib.rs:263-264` 无条件编译该私有模块。
- `pkg/expression/builtin_string_vec_generated_test.rs:37-39` 导入全部公开符号，并在 parity suite 中调用三种 FIELD 路径和能力标记。
- `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs:27,173-199` 复用三个 FIELD 入口验证 NULL、首匹配、NaN 和自定义字符串比较。
- RustCodeGraph 对目标文件报告 “used by 2 files”，与上述两个测试文件一致；精确 callers/callees 查询未产生额外生产调用边。全仓 `rg` 也未发现其他 Rust 调用点。

## 错误处理与边界

唯一显式错误是 `FieldVectorError::RowCountMismatch`。它在分配结果和执行比较之前返回，因此错误路径没有部分写入。`builtin_string_vec_generated_test.rs:176-179` 覆盖搜索列长度为 1、候选列长度为 2 的失败场景。

以下情况不是错误：零行、零候选列、NULL、重复匹配、NaN 或未匹配。重复匹配保留最小候选位置；NULL 与任何值（包括另一 NULL）均不匹配；NaN 依照直接 IEEE 比较保持不匹配。字符串相等性的正确性完全取决于调用方比较器，错误的 collation 比较器会产生语义错误，但本 API 没有可验证比较器策略的类型约束。

候选位置转换为 `i64` 使用 `(candidate_index + 1) as i64`。现实内存限制下候选数量远小于 `i64::MAX`；代码没有单独处理理论上的索引转换溢出。该文件也不负责表达式求值错误、缓冲区获取错误或 collation 构造错误，因为这些适配层尚未接入此内核。

## 并发与资源生命周期

本文件没有全局可变状态、锁、线程、异步任务、通道、事务或外部句柄。每次调用只借用输入并拥有输出，可由不同线程独立调用；实际能否跨线程共享输入/比较器仍由 Rust 的调用上下文和类型约束决定，函数签名本身未额外要求 `Send` 或 `Sync`。

`field_by` 在调用期间拥有比较器并以可变方式调用它；比较器在函数返回或报错时正常释放。与 Go 实现不同，Rust 内核不从 `bufAllocator` 获取临时列，也没有 `defer put` 式资源归还，因此当前不存在缓冲池生命周期。若未来接入 chunk 适配层，必须在所有提前返回路径保证临时缓冲归还。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/builtin_string_vec_generated.go`。Go 为 `builtinFieldIntSig`、`builtinFieldRealSig`、`builtinFieldStringSig` 分别生成 `vecEvalInt` 和 `vectorized` 方法；三条路径都先求值第一个参数列，再逐个求值候选列，并以“已有结果、任一 NULL、否则比较”的顺序处理每一行。Rust 的 `field_by` 保留了首匹配、NULL 跳过、1-based 下标及零默认值这些核心行为。

差异在适配层而非上述核心结果语义：

- Go 从 `input *chunk.Chunk` 调用每个子表达式的 `VecEval*`，写入调用方提供的 `result *chunk.Column`；Rust 接收已求值的 `Option<T>` 列并返回新 `Vec<i64>`。
- Go 使用 `bufAllocator.get/put` 并传播分配及子表达式错误；Rust 只有行数校验错误。
- Go 字符串路径固定调用 `b.ctor.Compare(...) == 0`；Rust 由调用方注入比较器，从而保留签名相关 collation 的扩展边界，但生产调用方尚未出现。
- Go 各候选参数可以复用同一临时列并按参数逐次求值；Rust 要求所有候选列预先物化，因此峰值输入内存形态不同。

Go 独立测试 `pkg/expression/builtin_string_vec_generated_test.go` 用三类 `vecExprBenchCase` 经过通用 `testVectorizedEvalOneVec` / `testVectorizedBuiltinFunc` 验证完整表达式框架，并保留 benchmark。Rust 对照测试 `pkg/expression/builtin_string_vec_generated_test.rs` 保存同样的三类 case 元数据，但实际断言聚焦本文件内核；其 benchmark 函数仅保留入口形状，没有执行性能测量。因此当前迁移覆盖不能等同于 Go 完整集成与 benchmark 覆盖。

## 扩展指南

- 修改 FIELD 公共扫描规则时，优先改 `field_by`，并同步检查三种类型是否仍与 `pkg/expression/builtin_string_vec_generated.go` 一致；不要在单一入口复制算法。
- 新增类型签名时，应增加类型专用入口和能力标记，选择符合 TiDB/MySQL 语义的比较器，并在独立的 `*_test.rs` 中覆盖 NULL、首匹配、无匹配、空列和长度不一致；测试逻辑不得内嵌到生产源文件。
- 字符串路径接入生产表达式时，应从签名构造器传入真实 collation 比较，而不是默认 Rust 字节或 Unicode 相等；同步验证大小写、重音、二进制 collation 等边界。
- 若要对齐 Go 完整执行链，接线位置应是表达式/chunk 适配层：负责逐个求值参数、控制临时缓冲生命周期、把内核结果写回结果列并传播子表达式错误。不要把这些职责隐藏进纯比较内核。
- 变更生成规则时还需检查 `pkg/expression/generator/string_vec.rs` 与 Go 生成器 `pkg/expression/generator/string_vec.go`。当前 Rust 生成器的 `generate_one_file` 生成的是 `.go` / `_test.go` 文件，并不证明本 Rust 内核可被自动重建；编辑带有 “DO NOT EDIT” 标记的文件前应先确认实际生成源与流程。
- 性能变化应关注 `O(R × C)` 扫描、候选列全部预物化和已匹配行的后续遍历；需要性能结论时补 Rust benchmark，不能引用当前仅为空壳的 benchmark 入口。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/expression/builtin_string_vec_generated.rs` 读取了完整 118 行源码，并报告两个使用文件；`query` 定位了 `FieldVectorError`、`validate_lengths`、`field_by`、三个类型入口和三个能力标记。精确 callers/callees 查询未返回额外边，因此生产接线结论又用全仓引用搜索复核。
- 源码与装配：`pkg/expression/builtin_string_vec_generated.rs`、`pkg/expression/lib.rs:263-264,594-595`、`pkg/expression/Cargo.toml`。
- Rust 测试：`pkg/expression/builtin_string_vec_generated_test.rs`；补充交叉回归证据为 `pkg/expression/builtin_regexp_util_23_aster_unit_test.rs:157-199`。
- Go 对照：`pkg/expression/builtin_string_vec_generated.go`、`pkg/expression/builtin_string_vec_generated_test.go`；生成入口参考 `pkg/expression/generator/string_vec.go:196-209` 和 `pkg/expression/generator/string_vec.rs:308-330`。
- 人工复核结论：该文件存在是为了隔离三类 FIELD 的共同向量匹配语义；运行方式是类型入口选择比较器后委托 `field_by`；安全扩展需要维持列长前置校验、NULL 不匹配、首匹配和 collation 边界，并同步独立测试与 Go 对照。
