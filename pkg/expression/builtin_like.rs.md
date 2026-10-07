# `pkg/expression/builtin_like.rs`

## 文件定位

对应源码：[builtin_like.rs](builtin_like.rs)。

本文件位于 `astersql-expression` crate（`pkg/expression/Cargo.toml`）中，由 `pkg/expression/lib.rs` 以私有模块 `builtin_like_kernel` 编译进 crate。它是从 Go `pkg/expression/builtin_like.go` 独立迁移出的 LIKE 标量求值内核，负责表达式形态 `value LIKE pattern ESCAPE escape` 的构造元数据、逐行求值和常量模式缓存；同一 Rust 类型的批量求值方法在相邻的 `pkg/expression/builtin_like_vec.rs` 中实现。

需要区分“模块已编译”和“通用 SQL 主路径已接入”：当前 `pkg/expression/builtin.rs` 会把函数名 `like` 绑定为 `CoreBuiltinKind::Like`，并在其自身的 `EvalInt` 分支内直接创建 collator pattern；仓库内除本文件、向量伴随文件和测试外，没有生产 Rust 代码直接构造这里的 `likeFunctionClass` 或 `builtinLikeSig`。因此本文件目前更准确的角色是可复用的独立迁移内核及语义对照面，而不能据此声称所有 Rust SQL LIKE 求值都经过 `builtinLikeSig::evalInt`。

## 核心职责

- `likeFunctionClass` 保存函数名并执行三参数数量校验，然后构造 `builtinLikeSig`。
- `builtinLikeSig` 保存三个表达式参数、字符串校对器、运行期通配符缓存、返回显示宽度和 LIKE 签名码。
- `builtinLikeSig::evalInt` 严格按“被匹配值、模式、escape”的顺序求值，传播 NULL 和表达式求值错误。
- 求值通过 `Collator::Pattern` 取得与 collation 对应的 `WildcardPattern`，依次调用 `Compile` 和 `DoMatch`；匹配结果转成 `Some(0)` 或 `Some(1)`。
- 当模式和 escape 都至少是 `ConstOnlyInContext` 时，编译结果缓存在签名对象内；否则每次逐行重新编译，避免错误复用按行变化的模式。

## 主要符号

- `ScalarFuncSig::LikeSig`：本文件的最小签名枚举。`builtinLikeSig::pb_code` 初始化为该值，供测试和下推元数据识别；通用主路径实际使用的是 `tipb::ScalarFuncSig::LikeSig`（见 `pkg/expression/builtin.rs`）。
- `likeFunctionClass { func_name }`：轻量函数类。`new` 接收可转成 `String` 的名字；`getFunction` 要求 `args.len() == 3`，错误信息使用保存的函数名。
- `builtinLikeSig { args, collator, pattern_cache, return_flen, pb_code }`：LIKE 标量签名。`args` 与 `collator` 对 crate 内伴随向量模块可见；缓存和元数据字段保持私有。
- `builtinLikeSig::new`：使用 `collate::binCollator` 的便捷构造入口。
- `builtinLikeSig::with_collator`：显式注入 `Arc<dyn Collator>`，再次校验三参数，并设置空缓存、`return_flen = 1` 和 `LikeSig`。
- `builtinLikeSig::Clone`：遵循 Go 风格命名的显式克隆方法；克隆参数引用和独立 collator，但故意创建空缓存，而不是实现/派生 Rust `Clone` 后复制运行期状态。
- `return_flen`、`pb_code`：暴露构造后的元数据；`cache_initialized` 是观测缓存是否建立的测试辅助方法。
- `evalInt(&self, ctx, row)`：标量核心入口，返回 `Result<Option<i64>>`；`None` 表示 SQL NULL，`Some(0/1)` 表示布尔匹配结果。

## 执行流程

1. 构造阶段由 `likeFunctionClass::getFunction` 或 `builtinLikeSig::{new,with_collator}` 接收三个 `ExprRef`。两层入口都防御性检查参数数量；默认入口选择 binary collation，显式入口允许调用者指定 CI 等校对规则。
2. `evalInt` 首先调用 `args[0].EvalString(ctx, row)`。结果为 NULL 时立即返回 `Ok(None)`，错误通过 `?` 原样上抛，因此后续参数不会求值。
3. 依次用 `args[1].EvalString` 取得模式、用 `args[2].EvalInt` 取得 escape；任一为 NULL 都立即返回 SQL NULL，保持 Go 的求值顺序和短路边界。
4. 若模式与 escape 的 `ConstLevel` 均不低于 `ConstOnlyInContext`，锁住 `pattern_cache`。首次调用从 `collator.Pattern()` 创建匹配器，以 `escape as u8` 编译模式并存入缓存；后续调用直接在缓存匹配器上执行 `DoMatch`。
5. 若任一参数不是上下文常量，则创建局部匹配器并在当前行编译，不写入共享缓存。
6. `bool` 匹配结果通过 `i64::from` 转为 0/1，并包装成 `Ok(Some(...))`。

批量路径不在本文件：`pkg/expression/builtin_like_vec.rs::vecEvalInt` 先批量求三个参数列、合并 NULL 位图，再用局部 matcher 为每行重新 `Compile`。它刻意不访问本文件的共享缓存，以支持逐行变化的 pattern/escape 并避免共享状态竞争。

## 数据与状态

`args: Vec<ExprRef>` 中元素是 `Arc<dyn LegacyExpression>`，约定位置分别为 value、pattern、escape。文件只验证数量，不在构造器内插入类型转换；相较之下，通用 `pkg/expression/builtin.rs` 的函数绑定会显式调用 `WrapWithCastAsString`、`WrapWithCastAsString` 和 `WrapWithCastAsInt`。因此独立调用者必须传入能按对应类型求值的表达式。

`collator: Arc<dyn Collator>` 决定字符等价关系以及 `%`、`_`、escape 的匹配实现。默认 `binCollator` 按字节严格匹配；测试通过 `collate::GetCollator` 注入 `utf8mb4_general_ci`、`utf8mb4_unicode_ci` 和 `utf8mb4_0900_ai_ci`，证明相同输入在不同 collation 下可以得到不同结果。

`pattern_cache: Mutex<Option<Box<dyn WildcardPattern>>>` 是惰性运行期状态。其键没有单独保存，因为只有 pattern 与 escape 都是上下文常量时才允许复用；这个常量级别不变量一旦被破坏，缓存就会把首次编译结果错误用于后续行。`Clone` 清空缓存，确保克隆表达式不会继承另一执行上下文已经编译的 matcher。

`return_flen = 1` 表示 0/1 结果的显示宽度，`pb_code = LikeSig` 表示 LIKE 签名。两者在构造后不再变化。

## 依赖与调用关系

- crate 边界：`pkg/expression/Cargo.toml` 定义 `astersql-expression`，并以路径依赖 `collate-dependency = astersql-util-collate`；本文件还使用同 crate 的 `collate` 门面和 `legacy_vectorized_runtime`。
- 模块装配：`pkg/expression/lib.rs` 将本文件声明为 `builtin_like_kernel`，随后声明 `builtin_like_vec_kernel`；测试配置下的 `expression_group_15` 会同时再导出二者。
- 下游依赖：`Collator::Pattern` 创建 `Box<dyn WildcardPattern>`，`WildcardPattern::{Compile,DoMatch}` 完成编译与匹配；`LegacyExpression::{EvalString,EvalInt,ConstLevel}` 提供参数值和缓存判定。
- 直接扩展：`pkg/expression/builtin_like_vec.rs` 对 `builtinLikeSig` 增加 `vectorized` 与 `vecEvalInt` 方法。
- 上游现状：RustCodeGraph 将该文件识别为被测试、向量伴随模块及相邻表达式文件引用；文本检索确认生产 Rust 中只有模块声明和向量伴随实现直接引用该 kernel。`pkg/expression/builtin.rs` 的通用 SQL 构建/求值链使用同名签名字符串和 `CoreBuiltinKind::Like`，但没有调用本文件的构造器或 `evalInt`。
- Go 生产链：`pkg/expression/builtin_like.go::likeFunctionClass.getFunction` 构建 Go `builtinLikeSig`；`pkg/expression/distsql_builtin.go` 能按 `tipb.ScalarFuncSig_LikeSig` 恢复该签名。这些是移植语义的依据，不应误当成 Rust 的实际调用边。

## 错误处理与边界

- 参数数量不是 3 时返回 `EvalError::Message`；函数类路径包含配置的函数名，`with_collator` 路径固定使用 `LIKE`。
- 任一参数求值错误由 `?` 传播；任一已求值参数为 NULL 时返回 `Ok(None)`。由于严格短路，较后参数在较前参数为 NULL 时不会被求值。
- escape 从 `i64` 以 `as u8` 转换，仅保留低 8 位；这对应 Go 的 `byte(escape)`，但调用者不能把它理解为完整 Unicode 字符值。
- `WildcardPattern::Compile` 的 trait 返回 `()`，本层没有“非法 LIKE 模式”的错误分支；模式语法和 escape 行为完全由具体 collator matcher 决定。
- 锁中毒使用 `expect("LIKE pattern cache poisoned")`，会 panic，而不是转换为 `EvalError`。首次缓存编译和随后 `DoMatch` 都在互斥锁持有期间执行。
- 本文件不自行做字符集转换、参数 cast、warning 收集或 protobuf 序列化；这些能力属于外层表达式构建/执行设施。不能仅凭本地 `ScalarFuncSig` 推断已完成完整下推序列化。

## 并发与资源生命周期

`Collator`、`WildcardPattern` 和 `LegacyExpression` 均要求 `Send + Sync`，参数与 collator 用 `Arc` 共享。缓存用 `Mutex` 串行化首次初始化和共享 matcher 的每次匹配，因此同一个签名被并发求值时不会并发修改 matcher；代价是常量模式标量求值会在每次匹配时获取锁，并在 `DoMatch` 完成后才释放。

缓存随 `builtinLikeSig` 生命周期存在，没有后台任务、通道、文件句柄或显式清理动作；对象析构时 matcher 自动释放。显式 `Clone` 创建独立 collator 和空 `Mutex<Option<_>>`，避免跨会话继承缓存。非常量标量路径的 matcher 只活到单次 `evalInt` 结束；向量伴随实现的局部 matcher 活到一批 Chunk 求值结束，并在行间重复编译。

## 与 Go 版本的对应关系

Rust `likeFunctionClass::getFunction`、`builtinLikeSig::Clone` 和 `evalInt` 分别对应 Go `pkg/expression/builtin_like.go` 的同名方法。两边都要求 value/string、pattern/string、escape/int 三个参数，将返回长度设为 1，使用 LIKE pb 签名，按相同顺序传播 NULL/错误，并只在 pattern 与 escape 都达到 `ConstOnlyInContext` 时复用已编译 matcher。Rust 的 `Mutex<Option<Box<dyn WildcardPattern>>>` 对应 Go 的 `builtinFuncCache[collate.WildcardPattern]`，而显式 `Clone` 都不复制缓存。

目前并非完全等形移植：Go 函数类通过 `newBaseBuiltinFuncWithTp` 完成参数类型约束、collator/base 元数据和 pb code 接线；本 Rust 文件用简化的 `legacy_vectorized_runtime`、本地 `ScalarFuncSig` 与显式 collator 注入表达同一核心算法。Go 缓存初始化可返回 error，并在理论错误路径返回 NULL/error；Rust matcher 的 `Compile` 无返回值，缓存锁失败则 panic。Go 的完整函数注册与 distsql 反序列化已连接 `builtinLikeSig`，而 Rust 通用 `builtin.rs` 当前另行实现 LIKE 主路径。

语义用例保持对齐：`pkg/expression/builtin_like_test.rs` 和 Go `pkg/expression/builtin_like_test.go` 覆盖 binary 通配符、转义、中文字符及三种 utf8mb4 CI collation；其中 `ß`、`ss` 和 Glagolitic 字母用例明确验证 matcher 是按 collation/字符规则工作，而非简单 lowercase 或字节查找。

## 扩展指南

- 若修改标量 NULL/错误顺序、常量缓存条件或 escape 转换，应首先改 `builtinLikeSig::evalInt`，并同步独立测试 `pkg/expression/builtin_like_test.rs` 与 `pkg/expression/builtin_like_vec_test.rs`；需要继续对照 `pkg/expression/builtin_like.go`，防止 Rust/Go 语义漂移。
- 若增加或改变 collation 匹配语义，应在 `pkg/util/collate` 的具体 `Collator`/`WildcardPattern` 实现处理，本文件只负责选择和调用 matcher；同时扩展 CI collation 表驱动用例，特别关注 `%`、`_`、escape、多字节字符及权重等价边界。
- 若改变缓存结构，必须维持“只缓存上下文常量 pattern+escape”“Clone 不复制运行期缓存”“并发共享不产生数据竞争”三个不变量，并评估每次持锁匹配的性能。新增测试仍应放在独立 `*_test.rs` 文件，不要嵌入生产源文件。
- 若目标是把此迁移内核接入完整 SQL 主链，需要同时审查 `pkg/expression/builtin.rs` 的函数注册、类型 cast、`CoreBuiltinKind::Like` 求值和 `tipb::ScalarFuncSig` 映射；不能只替换同名字符串。还需核对 `pkg/expression/distsql_builtin.rs` 当前是 Go 构造配方数据，而不是本 Rust 类型的直接构造代码。
- 若改向量行为，应修改 `pkg/expression/builtin_like_vec.rs` 并同步 `pkg/expression/builtin_like_vec_test.rs`、`pkg/expression/builtin_like_vec_15_aster_unit_test.rs` 及 Go `pkg/expression/builtin_like_vec_test.go`，验证按行 pattern、NULL 位图与标量结果一致。

## 验证依据

- 源码全貌：`pkg/expression/builtin_like.rs`（159 行），逐项核对 `ScalarFuncSig`、`likeFunctionClass`、`builtinLikeSig` 及其全部方法。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`query builtinLikeSig --kind struct` 同时定位 Go/Rust 定义；`node --file pkg/expression/builtin_like.rs --offset 1 --limit 220` 返回完整文件并报告其被 19 个文件使用。精确 callers/callees 查询未给出可归属到重名 Rust 符号的边，因此调用接线结论另用限定路径的 `rg` 核验，没有据重名结果推断调用关系。
- crate/装配：读取 `pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`，确认 crate 名、`collate-dependency`、私有 kernel 声明、向量伴随模块及测试再导出。
- 直接依赖：读取 `pkg/expression/legacy_vectorized_runtime.rs` 的 `ConstLevel`、`EvalContext`、`LegacyExpression` 和 `ExprRef`；读取 `pkg/util/collate/collate.rs` 的 `Collator`/`WildcardPattern` trait 与 `pkg/util/collate/bin.rs` 的 `binCollator` 实现。
- Rust 接线与伴随实现：读取 `pkg/expression/builtin_like_vec.rs`；检索 `builtin_like_kernel|builtinLikeSig|likeFunctionClass` 的非测试 Rust 引用；读取 `pkg/expression/builtin.rs` 中 LIKE 构建、标量求值和 tipb 签名映射，以及 `pkg/expression/distsql_builtin.rs` 的配方条目。
- Go 对照：读取 `pkg/expression/builtin_like.go`、`pkg/expression/builtin_like_vec.go`、`pkg/expression/builtin_like_test.go` 和 `pkg/expression/builtin_like_vec_test.go`。
- Rust 测试：读取 `pkg/expression/builtin_like_test.rs`、`pkg/expression/builtin_like_vec_test.rs` 和 LIKE 相关的 `pkg/expression/builtin_like_vec_15_aster_unit_test.rs`；证据覆盖参数数量、0/1 结果、通配符/escape、NULL、collation 差异、缓存初始化和 Clone 隔离。
- 本任务是纯文档分析，依计划未运行 Cargo；交付前仅执行任务指定的 11 章节结构验证并人工检查唯一产物、事实限定和源码链接。
