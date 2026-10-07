# `pkg/expression/builtin_like_vec.rs`

## 文件定位

[对应源码](builtin_like_vec.rs)是 `astersql-expression` crate 中 SQL `LIKE` 内建函数的遗留向量化求值内核。`pkg/expression/lib.rs` 通过 `#[path = "builtin_like_vec.rs"] mod builtin_like_vec_kernel;` 将它编入 crate；文件本身不定义新类型，而是为 `pkg/expression/builtin_like.rs` 定义的 `builtinLikeSig` 增加批量能力。`pkg/expression/Cargo.toml` 的 `[lib] path = "lib.rs"` 和 `[package.metadata.porting] go-package = "pkg/expression"` 分别确认 crate 入口及其 Go 对照包。

它位于“函数类构造 `builtinLikeSig` → 表达式选择向量化路径 → 对一个 `Chunk` 批量求值”的末端内核位置。当前文件只依赖 crate 内的签名类型与 `legacy_vectorized_runtime`；校对和通配符实现由签名持有的 `Collator` 间接提供。源码没有模块级常量、独立类型、trait 或条件编译项。

## 核心职责

- `builtinLikeSig::vectorized` 声明该签名支持向量化求值，固定返回 `true`。
- `builtinLikeSig::vecEvalInt` 将三个参数分别求值为“待匹配字符串列、模式字符串列、转义整数列”，把任一参数的 NULL 传播到结果列，再逐行执行 collation 感知的通配匹配。
- 匹配结果遵循表达式整型布尔约定：匹配写 `1`，不匹配写 `0`；NULL 行保留 NULL，底层整型槽位的零值没有 SQL 语义。
- 每次批量调用只创建一个局部 matcher，但每个非 NULL 行都重新 `Compile` 当前行的 pattern/escape。这保证行间模式可变化，同时不触碰 `builtinLikeSig.pattern_cache`。

该文件不负责参数个数校验、默认 collator 选择、标量缓存或签名元数据；这些职责在 `builtin_like.rs` 的 `likeFunctionClass::getFunction`、`builtinLikeSig::new/with_collator` 和 `evalInt` 中。

## 主要符号

- `pub fn vectorized(&self) -> bool`：`builtinLikeSig` 的公开固有方法，恒为 `true`，供调用方判断可否走批量路径。它不读取或改变签名状态。
- `pub fn vecEvalInt(&self, ctx: &EvalContext, input: &Chunk, result: &mut Column) -> Result<()>`：核心公开固有方法。`self.args[0]`、`[1]`、`[2]` 依次被当作 string、string、int 向量求值；输出写入调用者提供的 `result`。
- `builtinLikeSig`：定义于 `builtin_like.rs`，本文件使用其中的 `args: Vec<ExprRef>` 与 `collator: Arc<dyn Collator>`。构造器保证参数恰有三个，因此这里直接按索引访问。
- `Chunk`、`Column`、`EvalContext`、`Result`：来自 `legacy_vectorized_runtime.rs`。`Chunk::NumRows` 给出批大小；`Column` 提供定长整型缓冲、字符串缓冲、NULL 位图及 `MergeNulls`；`Result` 的错误类型为 `EvalError`。
- `Collator::Pattern` / `WildcardPattern::{Compile, DoMatch}`：定义于 `pkg/util/collate/collate.rs`。matcher 的具体语义取决于签名所持 collator；默认构造路径使用 `binCollator`。

## 执行流程

1. `vecEvalInt` 读取 `input.NumRows()`，将该数值作为所有中间列与输出列的共同长度。
2. 创建局部 `values`，调用 `self.args[0].VecEvalString(ctx, input, &mut values)`；失败时立即以 `?` 返回。
3. 同样把 `args[1]` 求值到 `patterns` 字符串列，把 `args[2]` 求值到 `escapes` 整型列。求值严格按 0、1、2 的顺序进行；任一步失败都不会继续后续步骤。
4. 通过 `self.collator.Pattern()` 创建本次调用私有的 matcher。它在整个批次中复用对象，但不共享已编译模式状态。
5. `result.ResizeInt64(rows, false)` 清空并重建长度为 `rows` 的整型结果缓冲与非 NULL 位图；随后 `MergeNulls(&[&values, &patterns, &escapes])` 对三列 NULL 位逐行做逻辑或。
6. 遍历 `0..rows`。若结果行已为 NULL，跳过 pattern 编译和匹配；否则用 `patterns.GetString(row)` 及 `escapes.Int64s()[row] as u8` 编译 matcher，再用 `values.GetString(row)` 匹配。
7. `i64::from(pattern.DoMatch(...))` 把布尔值写成 `0/1`。循环完成后返回 `Ok(())`。

时间复杂度除子表达式求值外，约为每行一次模式编译加一次匹配；模式编译成本不会因相邻行模式相同而省略。临时列及局部 matcher 的生命周期均限于一次调用。

## 数据与状态

持久状态位于 `builtinLikeSig`：三个 `ExprRef` 参数、`Arc<dyn Collator>`、标量路径使用的 `Mutex<Option<Box<dyn WildcardPattern>>>` 缓存，以及返回长度和 protobuf 签名码。本文件只读 `args` 与 `collator`，不读写 `pattern_cache`。

一次调用内创建三个拥有所有权的临时 `Column`。`values` 和 `patterns` 存字符串及各自 NULL 位，`escapes` 存 `i64` 转义值及 NULL 位。输出 `result` 被重置为 Int 列；在 NULL 行，整型槽仍可能是初始化的 `0`，调用者必须同时检查 `IsNull`。

转义值通过 Rust 的 `as u8` 转换，保留整数低 8 位；这对应 Go 的 `byte(escapes[i])`。模式和输入通过 `&str` 交给当前 collator 的 matcher，实际 `%`、`_`、escape 及字符比较语义由 `WildcardPattern` 实现决定。

## 依赖与调用关系

上游装配证据来自 `pkg/expression/lib.rs`：生产模块名是私有的 `builtin_like_vec_kernel`；测试配置下，`expression_group_15` 再导出该模块和 `builtin_like_kernel`，使独立测试可构造签名并调用两个方法。`builtin_like.rs` 的 `likeFunctionClass::getFunction` 是签名构造入口，并保证三个参数这一前置条件。

真实直接调用证据主要在独立测试：`builtin_like_vec_test.rs::test_vectorized_builtin_like_func` 和 `builtin_like_vec_15_aster_unit_test.rs::like_vector_propagates_nulls_and_recompiles_per_row_patterns` 都直接调用 `builtinLikeSig::vecEvalInt`。仓库中另有通用表达式批量分派，例如 `pkg/expression/builtin.rs` 的 `vecEvalInt` 转发到其持有的函数对象；但该区域使用另一组通用 `chunk`/表达式接口，当前索引没有给出一条可无歧义证明其动态分派最终落到本文件方法的 Rust 调用边，因此不把该转发宣称为已验证的直接调用者。

下游依赖为：三个 `ExprRef` 的 `LegacyExpression::{VecEvalString, VecEvalInt}`，`Column::{ResizeInt64, MergeNulls, IsNull, GetString, Int64s, Int64sMut}`，以及 `Collator::Pattern` 创建的 `WildcardPattern::{Compile, DoMatch}`。`Cargo.toml` 中与本路径最直接相关的 crate 依赖是 `collate-dependency = astersql-util-collate`；`Chunk`/`Column` 在当前迁移层由本 crate 的 `legacy_vectorized_runtime.rs` 提供。

## 错误处理与边界

三个子表达式求值均使用 `?` 原样传播 `EvalError`；文件不包装错误，也不追加 warning。由于执行顺序固定，较早参数报错会阻止较晚参数求值。matcher 的 `Compile` 与 `DoMatch` 接口不返回 `Result`，所以模式编译或匹配在此层没有可传播的普通错误分支。

参数数目不足不会在这里变成可恢复错误，而会因索引访问而 panic；正常调用必须经过 `builtinLikeSig::new/with_collator` 或 `likeFunctionClass::getFunction`，它们要求恰好三个参数并返回 `EvalError::Message`。`Column::MergeNulls` 还断言所有列长度等于结果长度，错误的子表达式实现可能触发 panic。

零行 `Chunk` 会生成四个空列并正常返回。任一输入为 NULL 的行不会读取该行的 pattern、escape 或 value，也不会调用 matcher。非 NULL escape 被截断为一个字节；这是与 Go 对照实现一致的显式兼容行为，而不是范围校验。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。`&self` 允许共享同一个签名实例，但向量路径刻意不使用签名里的 `pattern_cache: Mutex<_>`；每次 `vecEvalInt` 都从 collator 创建局部 matcher，因此并发调用之间不会共享可变的编译状态。这一点对应 Go 源码的 “Must not use b.pattern to avoid data race”。

三个临时列和 matcher 在函数返回时释放；`result` 由调用者拥有并在调用中原地重置。与 Go 的 allocator 借还模型不同，Rust 当前实现依赖局部 `Column::default()` 的分配与析构，没有缓冲池归还步骤。`EvalContext` 以共享引用传入，LIKE 内核自身不写 warning；子表达式仍可依据其实现访问上下文。

## 与 Go 版本的对应关系

`pkg/expression/builtin_like_vec.go` 与本文件具有一一对应的两个方法及相同步骤：声明 vectorized、获取行数、按 value/pattern/escape 顺序求三列、创建局部 matcher、重置 Int64 结果、合并三列 NULL、逐行编译并匹配、把 bool 转为整数。

已验证的语义一致点包括：三值 NULL 传播、每行 pattern 与 escape、局部 matcher 避免共享模式状态、escape 转为单字节，以及 `0/1` 结果。Go 测试 `builtin_like_vec_test.go::TestVectorizedBuiltinLikeFunc` 通过通用表驱动框架覆盖 `LIKE(string, string, int) -> int`；Rust 独立测试用具体数据覆盖 `%`、`_`、反斜杠转义、匹配/不匹配和两侧 NULL。

实现层差异是临时列管理：Go 从 `b.bufAllocator` 依次借出三个缓冲并用 `defer` 归还，借用失败也可返回错误；Rust 直接创建三个局部 `Column`，不存在 allocator 获取错误。Go 通过 `b.collator()` 方法取 matcher，Rust 直接读取 `self.collator` 字段。两者不改变匹配结果，但 Rust 当前缺少 Go 的缓冲复用性能特征。另需注意，Rust 专用测试注释中提及 pattern cache，但向量测试验证的实际设计是逐行重编译；缓存初始化与 Clone 隔离由同文件的标量测试验证，属于 `builtin_like.rs` 而非本文件行为。

## 扩展指南

- 修改向量 LIKE 的求值顺序、NULL 规则或输出编码时，入口应是 `builtinLikeSig::vecEvalInt`；必须同步核对 `builtin_like_vec.go`，避免破坏 Go 对齐。
- 扩展 collation 或 wildcard 语义应优先修改 `pkg/util/collate` 中相应 `Collator`/`WildcardPattern`，而非在本循环中硬编码 `%`、`_` 等规则；随后用 `builtinLikeSig::with_collator` 增加针对性测试。
- 若引入常量模式优化，不能直接复用 `pattern_cache` 而忽略并发设计。需要证明并发安全、非常量 pattern/escape 仍逐行编译，并评估与 Go “局部 matcher”策略的差异。
- 若优化临时分配，可在迁移运行时增加明确的缓冲复用机制，但需保持早期错误时的资源释放和结果列重置语义，并评估内存保留与跨线程共享风险。
- 测试必须保留在独立文件。直接行为测试放在 `pkg/expression/builtin_like_vec_test.rs`；更广的 Go 语义对齐可扩展 `pkg/expression/builtin_like_vec_15_aster_unit_test.rs`。建议新增：escape 超出 `u8` 范围的低八位行为、零行 Chunk、子表达式错误短路、非默认 collator、并发调用同一签名、相邻行相同/不同 pattern。
- 兼容风险集中在求值顺序、NULL 传播、escape 截断、collation 选择和布尔整型编码；性能风险集中在逐行 Compile 与三个临时列分配。

## 验证依据

- 目标源码：`pkg/expression/builtin_like_vec.rs`，确认全文件仅有 `builtinLikeSig::{vectorized, vecEvalInt}` 两个方法，且无条件编译项。
- 签名与标量对照：`pkg/expression/builtin_like.rs`，确认三参数校验、字段所有权、默认 binary collator、标量 cache 及 Clone 隔离。
- 运行时契约：`pkg/expression/legacy_vectorized_runtime.rs`，确认 `Chunk`、`Column`、`LegacyExpression`、`EvalContext`、`EvalError` 和 NULL 合并行为。
- 模块与 crate：`pkg/expression/lib.rs`、`pkg/expression/Cargo.toml`，确认 path 模块装配、测试再导出、crate 名称和 Go 包迁移元数据。
- matcher 接口：`pkg/util/collate/collate.rs` 与 `pkg/util/collate/bin.rs`，确认 `Collator::Pattern`、`WildcardPattern::{Compile, DoMatch}` 和默认 binary matcher。
- Go 对照：`pkg/expression/builtin_like_vec.go`、`pkg/expression/builtin_like_vec_test.go`；Rust 测试：`pkg/expression/builtin_like_vec_test.rs`、`pkg/expression/builtin_like_vec_15_aster_unit_test.rs`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/builtin_like_vec.rs` 返回完整 55 行源码；`query builtin_like_vec`、`query builtinLikeSig`、`query vecEvalInt` 找到目标方法、Go 对照和两份 Rust 测试。`callers/callees ... --file` 对常见方法名仍混入大量同名 Go/Rust 节点，故调用关系仅采用文件限定源码与 `rg` 的直接调用点复核，不把噪声边当作事实。
- 本任务是纯文档分析，按任务约束不运行 Cargo；交付结构以固定十一个二级标题检查，链接与路径以仓库文件存在性复核。
