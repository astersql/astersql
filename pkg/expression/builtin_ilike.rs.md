# `pkg/expression/builtin_ilike.rs`

## 文件定位

本文件是 `astersql-expression` crate 中 ILIKE 的标量匹配内核，源文件为 [`pkg/expression/builtin_ilike.rs`](builtin_ilike.rs)。`pkg/expression/lib.rs` 通过 `#[path = "builtin_ilike.rs"] mod builtin_ilike_kernel` 将它挂载为 crate 内部模块；SQL 函数的参数构建、注册、TiPB 签名和通用 `builtinFunc` 接口仍在 `pkg/expression/builtin.rs` 中完成。

这意味着本文件不是独立的 SQL 函数工厂，而是被 `CoreBuiltinKind::Ilike` 持有的可执行状态。`pkg/expression/builtin.rs` 将 `ilike` 注册为三参函数，把前两个参数转成字符串、第三个参数转成整数，推导排序规则后构造 `IlikeSig`，并将下推签名设为 `tipb::ScalarFuncSig::IlikeSig`。

## 核心职责

- 实现 ILIKE 的三值 SQL 语义：`value`、`pattern` 或 `escape` 任一为 NULL 时返回 NULL，否则返回 `0` 或 `1`（`IlikeSig::eval_int`）。
- 在通配匹配前对值和模式做 ASCII 小写折叠，但不对非 ASCII Unicode 字符执行完整 case folding（`normalize_ilike`）。
- 保护字母型 escape 的语义：折叠 pattern 时不把 escape 引用的字节当成普通字母改写，并返回折叠后的 escape 字节。
- 使用所选 collation 的二进制对应排序器编译和执行 `%`/`_` 通配模式（`compile_pattern`）。
- 仅在 pattern 和 escape 都达到 `ConstOnlyInContext` 级别时复用已编译 pattern，并保证克隆表达式不继承运行时缓存。

## 主要符号

- `ExpressionError`：通过 `thiserror::Error` 定义的表达式错误集。对本文件的标量 ILIKE 路径而言，`eval_int` 的签名使用它，但当前匹配内核没有主动产生错误；`EscapeMustBeConstant` 由相邻的向量实现用于拒绝非常量 escape。其余会话、权限、参数、取消和外部错误变体是该文件组的共用边界，不应误解为 ILIKE 匹配自身已覆盖的失败分支。
- `CachedPattern`：私有缓存项，保留规范化后的 `source: String`、`escape: u8` 和 `Arc<dyn WildcardPattern>`。命中条件是 source 和 escape 同时相等。
- `IlikeSig`：可执行签名。`collation` 和 `use_new_collation` 是构建时捕获的匹配策略；`pattern_is_constant` 与 `escape_is_constant` 决定是否允许缓存；`pattern_cache` 是受 `RwLock` 保护的可选缓存。
- `IlikeSig::new`：从全局 `collate::NewCollationEnabled()` 捕获排序规则模式的便捷构造器，主要用于直接构造和测试。
- `IlikeSig::new_with_collation_mode`：显式接收 `use_new_collation` 的构造器。`core_builtin_factory` 使用它将 `BuildContext` 当时的模式固定在签名中，避免求值时受后续全局状态变化影响。
- `Clone for IlikeSig`：复制配置和常量标志，但通过构造器创建空缓存。
- `cache_initialized`：只读诊断入口，用于检查缓存是否已填充；当前主要由独立测试验证缓存生命周期。
- `eval_int`：标量公开求值入口，输入三个 `Option`，输出 `Result<Option<i64>, ExpressionError>`。
- `matches`：crate 内部匹配入口，先规范化，再选择缓存或当次编译，最后调用 `WildcardPattern::DoMatch`。
- `cached_pattern`：实现读锁快路径和写锁更新路径。
- `compile_pattern`：将 collation 转成 binary counterpart，获取 collator 的 pattern 对象，执行 `Compile(source, escape)` 并转成共享 `Arc`。
- `normalize_ilike`：对 value 和 pattern 做字节级 ASCII 折叠，将 `i64` escape 按 Go 行为截取为 `u8`，返回拥有所有权的两个字符串和最终 escape。

## 执行流程

1. `pkg/expression/builtin.rs` 的注册表把名称 `ilike` 映射到 `ilike_factory`，参数元数表约束其必须恰好有三个参数。
2. `core_builtin_factory` 对 value/pattern/escape 分别包装字符串、字符串、整数 CAST，返回布尔字段类型，并从前两个字符串参数推导 charset/collation。
3. 工厂根据 `BuildContext::NewCollationEnabled()`、推导出的 collation，以及 pattern/escape 的 `ConstLevel()` 构造 `IlikeSig`；同时设置 TiPB `IlikeSig` 签名。`SetCharsetAndCollation` 后续被调用时也会按新 collation 重建该内核。
4. `CoreBuiltin::evalInt` 按顺序求值 value、pattern、escape；任一参数 NULL 都立即返回 SQL NULL。三者都非 NULL 时调用 `IlikeSig::eval_int`，并把本文件的错误转成通用 expression error。
5. `IlikeSig::eval_int` 再保留可独立调用的 NULL 传播契约，然后以“pattern 和 escape 均为常量”作为 `cacheable` 传入 `matches`。
6. `normalize_ilike` 先小写折叠 value。对 pattern，若 escape 是 ASCII 大/小写字母，调用 `LowerOneStringExcludeEscapeChar`；否则直接调用 `LowerOneString`。
7. 可缓存路径调用 `cached_pattern`：先在读锁下比对 source/escape，命中则克隆 `Arc`；未命中则编译，再取写锁替换单项缓存。不可缓存路径每次直接调用 `compile_pattern`。
8. 最终由编译后的 `WildcardPattern::DoMatch` 返回布尔值，`eval_int` 将它转为 `i64` 的 `0` 或 `1`。

## 数据与状态

`IlikeSig` 的持久状态可分为两类。第一类是构建时配置：`collation`、`use_new_collation`、`pattern_is_constant` 和 `escape_is_constant`，在求值期间不变。第二类是可变的单项 `pattern_cache`，它仅保存最近一个已规范化的 pattern/escape 组合及其编译器，不是无界 map。

value 和 pattern 在每次匹配时都被拷贝为字节向量再就地做 ASCII 折叠，因此不会修改调用者的输入。折叠只改写 ASCII 字节，不会破坏原 UTF-8 编码；代码使用 `String::from_utf8(...).expect(...)` 表达这个内部不变量。escape 由 `i64 as u8` 转换，即与 Go 的 `byte(escape)` 一样只保留低 8 位。

## 依赖与调用关系

上游主链是 `NewFunctionBase`/正式 builtin 注册表 → `ilike_factory` → `core_builtin_factory` → `CoreBuiltin { kind: Ilike, ilike: Some(IlikeSig) }` → `CoreBuiltin::evalInt` → `IlikeSig::eval_int`。`pkg/expression/builtin_ilike_test.rs` 中的 `canonical_ilike_factory_preserves_escape_null_and_clone` 通过 `NewFunctionBase("ilike", ...)` 验证了这条真实工厂路径及 TiPB 签名，不只是直接调内核。

本文件的直接下游依赖为：

- `crate::collate` / Cargo 中的 `collate-dependency` (`astersql-util-collate`)：转换 binary collation、选择 collator、创建与编译 `WildcardPattern`。
- Cargo 中的 `stringutil-dependency` (`astersql-util-stringutil`)：`LowerOneString`、`IsUpperASCII`、`IsLowerASCII` 和 `LowerOneStringExcludeEscapeChar`。
- Cargo 中的 `thiserror`：为 `ExpressionError` 生成 `Display` 和标准错误实现。
- 标准库 `Arc` 与 `RwLock`：分享已编译 pattern 并保护延迟缓存。

`pkg/expression/builtin_ilike_vec.rs` 在同一 `IlikeSig` 上增加批量求值方法，但不属于本文件的实现范围。它复用本文件的匹配和缓存状态，因此修改标量内核时必须同时考虑向量路径的一致性。

## 错误处理与边界

- NULL 不是错误：`eval_int` 返回 `Ok(None)`；集成层将其翻译为 `(0, true)`。Rust 工厂测试覆盖三个位置的 NULL。
- 大小写不敏感限于 ASCII 折叠。`ü`/`Ü` 和 `ß`/`ss` 不会因 ILIKE 而变成相等；`_` 仍按 collator pattern 的字符匹配规则工作。
- 字母 escape 是特别边界。pattern 的折叠必须排除被解释为 escape 的字节；Rust/Go 用例明确区分 `'A'` 和 `'a'` 下的结果。
- 空值与空 pattern 是有效输入：`"" ILIKE ""` 匹配，非空值对空 pattern 不匹配。
- pattern 编译 API 当前不返回 `Result`，因此本标量路径没有“非法 pattern”错误分支。如果下游 API 将来变为可失败，需要扩展 `ExpressionError` 并保留 `CoreBuiltin` 的错误转换。
- 中毒的 `RwLock` 不会导致求值返错；实现使用 `poisoned.into_inner()` 继续使用其内部状态。这是显式的容错选择，不代表能修复由 panic 留下的逻辑不一致。
- `from_utf8(...).expect(...)` 可理论上 panic，但输入原本是 `&str` 且只改写 ASCII 字节，因此在当前实现不变量下不可达。

## 并发与资源生命周期

`IlikeSig` 可被共享读取：配置字段不变，唯一可变状态由 `RwLock` 保护，编译后的 pattern 放在 `Arc<dyn WildcardPattern>` 中。命中时只需读锁和 `Arc::clone`；未命中时编译在锁外完成，之后用写锁替换缓存，从而避免在较慢的编译期间持有锁。

该设计允许两个并发未命中各自编译同一或不同 pattern，后获得写锁的调用可以覆盖前一个缓存项。这不影响已返回 `Arc` 的正确性，但表明缓存只是性能优化，不是全局唯一编译保证。没有后台任务、通道、事务、文件句柄或需要显式关闭的资源；缓存与 `IlikeSig` 同寿命，最后一个 `Arc` 释放后自动销毁。

克隆是重要的会话边界：`Clone` 不复制 `pattern_cache`，避免把某一构建/求值上下文中的运行时缓存无意共享给克隆表达式。`ilike_scalar_matches_go_ascii_escape_and_null_semantics` 和正式工厂测试都覆盖了这一点。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/expression/builtin_ilike.go`：`ilikeFunctionClass.getFunction` 对应 Rust 正式注册/工厂逻辑，`builtinIlikeSig` 对应 `IlikeSig`，`builtinIlikeSig.evalInt` 对应 `IlikeSig::eval_int` 及其下游辅助函数。两者的共同语义是：

- 三参中任意 NULL 传播为 SQL NULL。
- value 使用 `LowerOneString`；pattern 在字母 escape 时使用 `LowerOneStringExcludeEscapeChar`。
- collation 先经 `ConvertAndGetBinCollation` 转成二进制对应项，再用其 `WildcardPattern`。
- pattern 与 escape 均为 context 常量时缓存编译结果，否则每次编译。
- escape 最终按单字节传入 pattern 编译器。

结构上有两个可见差异。第一，Go 将参数表、返回类型、collator 和 protobuf 签名嵌入 `baseBuiltinFunc`，Rust 将这部分留在 `CoreBuiltin` 与工厂，本文件只保留匹配内核。第二，Go 用 `builtinFuncCache` 按求值上下文延迟初始化；Rust 明确记录 source/escape，以 `RwLock<Option<CachedPattern>>` 支持共享与替换，并在 `Clone` 时丢弃缓存。

`pkg/expression/builtin_ilike_test.rs` 的主用例表直接对齐 `pkg/expression/builtin_ilike_test.go::TestIlike`，包括 general_ci、unicode_ci、bin、中文、`ü`、`ß`、`_`、`%` 和字母 escape。Rust 测试还通过 `canonical_ilike_factory_preserves_escape_null_and_clone` 补充检查正式工厂、NULL 与克隆路径。

## 扩展指南

- 改变大小写规则时，优先修改 `normalize_ilike`，同时检查 `pkg/expression/builtin_ilike_vec.rs` 的批量规范化是否仍与标量一致。必须保留字母 escape 的排除规则，并在独立测试文件增加大/小写 escape 对照。
- 改变 collation 选择时，修改 `compile_pattern` 以及 `pkg/expression/builtin.rs` 的构建/重设 collation 接线，同步对照 Go `builtinIlikeSig.SetCharsetAndCollation`。这会影响兼容性和查询下推结果，不能只修本文件中的一个字符串。
- 扩展缓存策略时，集中修改 `CachedPattern`、`cached_pattern` 和 `Clone`。需要继续保证非常量 pattern/escape 不复用错误编译结果，考虑并发覆盖的性能代价，并保持克隆的会话隔离。
- 新增可失败分支时，在 `ExpressionError` 中添加有语义的变体，让 `eval_int`/`matches`/`compile_pattern` 逐层传播，并确认 `CoreBuiltin::evalInt` 的通用错误映射保留足够上下文。
- 测试逻辑不应内嵌进本文件。标量与工厂回归放在 `pkg/expression/builtin_ilike_test.rs`；向量 NULL/参数形状放在 `pkg/expression/builtin_ilike_vec_test.rs`；缓存、克隆与标量/向量综合契约现也在 `pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs` 中有直接覆盖。Go 语义变更时还应对照 `pkg/expression/builtin_ilike_test.go`。
- 性能风险主要来自每行字符串分配/折叠、pattern 重复编译和锁竞争；正确性风险主要来自 Unicode/collation、escape 与 NULL 语义；兼容性风险还包括 TiPB `IlikeSig` 下推结果必须与本地求值一致。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/expression/builtin_ilike.rs` 核对了全部 234 行源码与 `ExpressionError`、`CachedPattern`、`IlikeSig`、`eval_int`、`matches`、`cached_pattern`、`compile_pattern`、`normalize_ilike` 的定义。
- RustCodeGraph `explore`/`node` 确认了关键边：`eval_int → matches`，`matches → normalize_ilike/cached_pattern/compile_pattern`，以及 `pkg/expression/builtin.rs::CoreBuiltin::evalInt → IlikeSig::eval_int`。`callers/callees` 的精确名称查询未返回额外文本，因此本文档没有据此声称更广的静态调用者。
- `pkg/expression/lib.rs` 证明了模块挂载和独立测试挂载；`pkg/expression/Cargo.toml` 证明 crate 名称、`lib.rs` 入口、`collate-dependency`、`stringutil-dependency` 和 `thiserror` 依赖。目录中没有 `pkg/expression/doc.go`，因此未能从包级 `doc.go` 获得额外契约。
- `pkg/expression/builtin.rs` 的工厂、注册表、元数表、collation 推导、TiPB 签名和 `CoreBuiltin::evalInt` 证明了本文件在完整应用中的接线。
- Go 对照：`pkg/expression/builtin_ilike.go` 与 `pkg/expression/builtin_ilike_test.go`。Rust 相关独立测试：`pkg/expression/builtin_ilike_test.rs`、`pkg/expression/builtin_ilike_vec_test.rs` 和 `pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs`。这些证据覆盖了排序规则、ASCII/非 ASCII、escape、NULL、常量/列向量路径、缓存和克隆。
- 本任务仅产生文档，按计划不运行 Cargo；结构验证要求目标文件存在且恰好包含十一个固定二级标题。
