# `pkg/expression/builtin_ilike_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate。模块入口在 `pkg/expression/lib.rs`，它通过 `#[path = "builtin_ilike_vec.rs"] mod builtin_ilike_vec_kernel;` 将本文件作为私有模块编入 crate；同一入口还把 `builtin_ilike_vec_test.rs` 和 `builtin_ilike_vec_12_aster_unit_test.rs` 作为独立测试模块编入。`pkg/expression/Cargo.toml` 的 `[lib] path = "lib.rs"` 确认了这一 crate 边界，并声明本文件直接使用的 `stringutil-dependency`（工作区包 `astersql-util-stringutil`）。

它是 Rust ILIKE 的批量求值辅助层：定义批量字符串参数、escape 参数、ASCII 折叠辅助函数，并为 `pkg/expression/builtin_ilike.rs` 中的 `IlikeSig` 增加 `vectorized` 与 `vec_eval_int` 方法。需要注意当前接线状态：生产表达式注册器位于 `pkg/expression/builtin.rs`，其中 `CoreBuiltinKind::Ilike` 的逐行执行会调用 `IlikeSig::eval_int`，但 `CoreBuiltin` 的 `builtinFunc::vectorized()` 当前固定返回 `false`，且没有调用本文件的 `vec_eval_int`。因此本文件的批量入口目前由 Rust 独立测试直接覆盖，尚未接入统一表达式向量调度；不能仅凭本文件的 `IlikeSig::vectorized() == true` 推断生产调度已经启用。

## 核心职责

- 用 `StringParam` 表示“单个常量值”或“每行一个值的列”，并以 `Option` 表达 SQL `NULL`。
- 用 `EscapeParam` 明确区分严格常量 escape 与列 escape；后者是为保持 Go 向量接口的拒绝语义而存在的错误输入。
- 在 `IlikeSig::vec_eval_int` 中保持批大小、NULL 短路、逐行 NULL 合并、常量 pattern 缓存选择以及布尔结果到 `i64` 的转换。
- 提供 `LowerAlphaASCII` 与 `LowerAlphaASCIIExcludeEscapeChar` 两个公开辅助函数，对应 Go 文件的同名函数。当前 `vec_eval_int` 并不直接调用它们，而是逐行委托 `IlikeSig::matches`；实际规范化逻辑位于 `pkg/expression/builtin_ilike.rs::normalize_ilike`。

本文件不负责 SQL 函数注册、参数类型转换、排序规则选择器实现或通配模式编译细节；这些分别位于 `pkg/expression/builtin.rs`、`pkg/expression/builtin_ilike.rs` 及 `collate-dependency`。

## 主要符号

- `pub enum StringParam { Constant(Option<String>), Column(Vec<Option<String>>) }`：批量字符串实参。`Constant` 对所有行复用同一值；`Column` 必须与 `row_count` 等长。
- `StringParam::is_constant`：仅判断表示形式，供常量 pattern 的缓存决策使用。
- `StringParam::is_constant_null`：识别严格常量 NULL。它是私有方法，因为这种提前短路只属于本文件的求值次序。
- `StringParam::value`：按行借用字符串；常量忽略行号，列访问通过 `get` 完成，越界会表现为 `None`。正常入口会先由 `validate_len` 排除列长度错误，因此越界 NULL 只是防御性行为。
- `StringParam::validate_len`：列长度与批大小不相等时返回 `ExpressionError::InvalidArgument`，常量无需校验长度。
- `pub enum EscapeParam { Constant(Option<i64>), Column }`：第三参数模型。`Constant(None)` 表示 NULL escape，`Column` 表示违反 Go `ConstStrict` 约束。
- `pub fn LowerAlphaASCII`：就地处理每个非 NULL 字符串，调用 `stringutil::string_util::LowerOneString` 只折叠 ASCII 字母；通过重新分配字节向量并写回字符串，保留 NULL。
- `pub fn LowerAlphaASCIIExcludeEscapeChar`：折叠 pattern 时调用 `LowerOneStringExcludeEscapeChar` 保护 escape 字节，并返回折叠后实际 escape。空列或全 NULL 列返回传入 escape 截断为 `u8` 后的值；多行时返回最后一次辅助调用给出的值。
- `IlikeSig::vectorized`：能力声明方法，固定返回 `true`；当前没有被统一 `builtinFunc` 调度采用。
- `IlikeSig::vec_eval_int`：本文件核心入口，返回与 `row_count` 等长的 `Vec<Option<i64>>`，匹配为 `1`、不匹配为 `0`、SQL NULL 为 `None`。

文件中没有模块级常量、局部 struct、trait 或条件编译项；公开 API 是两个参数 enum、两个 ASCII 辅助函数，以及 `IlikeSig` 上的两个方法，其余方法均为内部实现。

## 执行流程

`IlikeSig::vec_eval_int(expression, pattern, escape, row_count)` 的次序是可观察语义的一部分：

1. 先检查 `expression` 或 `pattern` 是否为 `StringParam::Constant(None)`。若是，立即返回 `vec![None; row_count]`，甚至不会验证 escape；`builtin_ilike_vec_test.rs::constant_null_string_argument_short_circuits_escape_validation` 专门锁定了“常量字符串 NULL 优先于非常量 escape 报错”的次序。
2. 分别调用 `validate_len(row_count)`。任一列长度不符即返回 `ExpressionError::InvalidArgument`，不会产生部分结果。
3. 解析 escape：`EscapeParam::Column` 返回 `EscapeMustBeConstant`；`Constant(None)` 令整批为 NULL；`Constant(Some(value))` 才进入匹配。
4. 以 `pattern.is_constant()` 计算 `cacheable`。这意味着仅 pattern 的表示形式决定本次批量调用是否请求复用缓存；escape 已经由类型保证为常量。`IlikeSig::matches` 还会按规范化后的 pattern 与 escape 核验实际缓存键。
5. 预分配 `row_count` 容量并逐行取 expression/pattern。任一行任一侧为 NULL，就压入 `None`；否则调用 `self.matches(value, pattern, escape, cacheable)`，将 `bool` 转成 `i64` 后压入 `Some`。
6. `IlikeSig::matches`（定义于 `builtin_ilike.rs`）调用 `normalize_ilike`：value 做 ASCII 小写折叠；pattern 在 escape 是 ASCII 字母时保护 escape 字节，否则普通折叠。随后按二进制化后的排序规则编译或复用 `WildcardPattern`，最终调用 `DoMatch`。

四种常量/列组合都汇入同一个循环：列×列、常量 expression×列 pattern、列 expression×常量 pattern、常量×常量。与 Go 版为这几类组合分设 `vecVec`、`constVec`、`ilikeWithMemorization` 和 `ilikeWithoutMemorization` 不同，Rust 版通过 `StringParam::value` 统一取值。

## 数据与状态

本文件自身不保存全局状态。每次调用的输入由拥有所有权的 `String`/`Vec` 构成，输出新建 `Vec<Option<i64>>`；`vec_eval_int` 只借用输入，不修改它们。两个 `LowerAlphaASCII*` 辅助函数则显式接受可变切片并就地替换非 NULL 字符串，因此调用方若要保持上游列不变，必须像 Go 的 `CopyConstruct` 路径一样先复制数据。

持久的运行时状态属于 `IlikeSig`：`pkg/expression/builtin_ilike.rs` 中的 `pattern_cache: RwLock<Option<CachedPattern>>` 保存规范化 pattern、escape 字节和 `Arc<dyn WildcardPattern>`。本文件只通过 `matches(..., cacheable)` 间接触发它。常量 pattern 可以复用缓存，列 pattern 每行重新编译；`builtin_ilike_vec_12_aster_unit_test.rs` 用 `cache_initialized()` 验证列×常量路径确实填充缓存。

SQL NULL 的表示保持一致：字符串常量或行值使用 `Option<String>`，escape 使用 `Option<i64>`，结果使用 `Option<i64>`。结果向量长度不随 NULL 数量变化，始终等于 `row_count`。

## 依赖与调用关系

上游与装配关系：

- `pkg/expression/lib.rs` 将本文件装配为 `builtin_ilike_vec_kernel` 私有模块。
- RustCodeGraph 对该文件报告 16 个符号，并显示直接使用文件为 `pkg/expression/builtin_ilike_test.rs` 与 `pkg/expression/builtin_ilike_vec_test.rs`；仓库搜索还确认 `pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs` 直接调用 `vec_eval_int`。
- `pkg/expression/builtin.rs` 负责 `ilike` 工厂、参数 cast、排序规则和 protobuf 签名，并构造 `IlikeSig`；当前生产求值调用的是 `IlikeSig::eval_int`，没有从该注册层进入本文件的批量入口。

下游关系：

- `StringParam::validate_len` 依赖 `pkg/expression/builtin_ilike.rs::ExpressionError::InvalidArgument`。
- escape 约束依赖同一错误枚举的 `EscapeMustBeConstant`。
- 两个独立折叠函数依赖 `stringutil-dependency` 的 `LowerOneString` / `LowerOneStringExcludeEscapeChar`。
- `vec_eval_int` 的匹配委托给 `pkg/expression/builtin_ilike.rs::IlikeSig::matches`，后者继续依赖排序规则模块、`WildcardPattern::Compile` / `DoMatch` 和受 `RwLock` 保护的缓存。

应用主链应区分“现状”和“目标接线”：现状是 SQL `ILIKE` → `pkg/expression/builtin.rs` 的工厂与 `CoreBuiltin::evalInt` → `IlikeSig::eval_int`；本文件目前是测试可直接调用的向量内核。若未来接入统一向量调度，才会形成 SQL 批量输入 → `StringParam`/`EscapeParam` 适配 → `vec_eval_int` → `matches` 的生产链。

## 错误处理与边界

- 严格常量 expression/pattern 为 NULL 时优先整批返回 NULL，优先级高于列长度和 escape 合法性检查。这与 Go `buildStringParam` 提前返回的行为对齐。
- 非 NULL 字符串列长度必须精确等于 `row_count`；错误包含实际行数与期望行数。常量不受此约束，`row_count == 0` 时返回空结果。
- escape 列直接返回 `ExpressionError::EscapeMustBeConstant`，其展示文本是 `escape should be const`；NULL escape 不报错，而是整批 NULL。
- 行级 expression 或 pattern 为 NULL 时只影响对应结果行，等价于逐行合并两侧 NULL 位图。
- escape 从 `i64` 转为 `u8`，行为与 Go 的 `byte(escape)` 一致：只保留低 8 位。本文件不单独做范围校验。
- ASCII 折叠只修改 ASCII 字母，不把 `Ü`、`ß` 等非 ASCII 字符做 Unicode 大小写展开；相关语义由 `builtin_ilike_test.rs::test_ilike` 的多排序规则用例验证。
- 两个折叠函数用 `String::from_utf8(...).expect(...)` 写回。依据是只修改 ASCII 字节不会破坏原 UTF-8；若这个不变量被未来的 stringutil 实现打破，进程会 panic，而不是返回 `ExpressionError`。
- `LowerAlphaASCIIExcludeEscapeChar` 对多行反复覆盖 `actual`。当前 stringutil 对固定 excluded byte 应给出一致实际 escape；若未来改成依赖行内容的返回值，该“最后一行决定返回值”的行为必须重新评估。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务、文件句柄或网络资源。`vec_eval_int` 的临时结果和逐行借用都局限于一次同步调用；输入借用结束后即可释放，输出所有权交给调用方。两个折叠函数对传入切片独占可变借用，Rust 类型系统阻止同一调用期间的并发读写。

共享并发状态只来自 `IlikeSig::matches` 下游的 pattern 缓存。`builtin_ilike.rs` 用 `RwLock` 保护 `Option<CachedPattern>`，用 `Arc` 分享已编译模式，并在锁中毒时通过 `into_inner` 继续使用数据。`IlikeSig::clone` 不复制缓存，从而避免把运行时缓存跨表达式/会话传播。列 pattern 的 `cacheable == false` 路径不写共享缓存；常量 pattern 可能读写缓存。当前逐行循环是串行的，没有行级并行。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/expression/builtin_ilike_vec.go`，测试对照是 `pkg/expression/builtin_ilike_test.go`。

- 同名 `LowerAlphaASCII` 与 `LowerAlphaASCIIExcludeEscapeChar` 保留了“只折叠 ASCII”和“保护 escape”的算法意图；Go 操作 `chunk.Column` 的字符串内存，Rust 操作 `&mut [Option<String>]` 并重建字符串。
- Go `funcParam` 通过是否持有列区分常量/列；Rust 将状态显式化为 `StringParam`。Go 的 `buildStringParam`、buffer pool 和 `releaseBuffers` 没有逐项移植，因为 Rust 输入由调用方持有且通过所有权自动回收。
- Go `getEscape` 检查第三参数必须为 `ConstStrict`，NULL 时填充整列 NULL；Rust 用 `EscapeParam` 在类型层建模相同分支。
- Go 会先复制列再小写化，并针对四种常量/列组合选择专门函数。Rust `vec_eval_int` 不预先改写输入，而让每行 `matches` 完成规范化，并用统一循环覆盖四种组合。
- Go 仅在 pattern 是常量时通过 `patternCache` 记忆已编译模式。Rust 将 `pattern.is_constant()` 作为 `matches` 的 `cacheable` 参数，并由 `CachedPattern` 的 source/escape 键保证复用安全。
- 两版都逐行传播 expression/pattern NULL，返回 0/1 整数，并保持 ASCII-only 的大小写不敏感语义。Go 测试 `TestVectorizedBuiltinIlikeFunc` 和 `TestVectorizedBuiltinIlikeForConstants` 对应 Rust 的 `test_vectorized_builtin_ilike_func`、`test_vectorized_builtin_ilike_for_constants` 及综合向量测试。
- 接线存在差异：Go 的 `builtinIlikeSig.vectorized()` 与 `vecEvalInt` 已实现标准 `builtinFunc` 调度；Rust 本文件虽提供同等内核，`pkg/expression/builtin.rs::CoreBuiltin::vectorized()` 目前仍返回 `false`，所以尚不能宣称 Rust 生产调度已与 Go 完全对齐。

## 扩展指南

- 新增参数表示或批量广播规则时，优先修改 `StringParam`、`value` 和 `validate_len`，并在独立的 `pkg/expression/builtin_ilike_vec_test.rs` 或 `builtin_ilike_vec_12_aster_unit_test.rs` 增加常量/列/NULL/零行/长度错误组合；不要把测试嵌入生产文件。
- 调整 NULL 或错误优先级时，必须保留并扩展 `constant_null_string_argument_short_circuits_escape_validation`，同时对照 Go `vecEvalInt` 中 `buildStringParam` 在 `getEscape` 之前执行的顺序。
- 调整 ASCII/escape 规范化时，应同步检查 `LowerAlphaASCII*`、`builtin_ilike.rs::normalize_ilike` 和 Go 同名函数，覆盖大写 escape、小写 escape、反斜杠、非 ASCII 字符与多字节 UTF-8；最大的兼容风险是把 ASCII ILIKE 错改成完整 Unicode case folding。
- 改变缓存策略时，应修改 `vec_eval_int` 的 `cacheable` 决策及 `builtin_ilike.rs::cached_pattern` 的键/锁语义，并测试常量 pattern 命中、列 pattern 不缓存、不同 escape 不误复用以及 clone 清空缓存。错误缓存会带来跨行错误结果；过度编译则是性能风险。
- 真正接入生产向量调度不能只改本文件：还需在 `pkg/expression/builtin.rs` 的 `builtinFunc` 向量接口中把 chunk/表达式参数安全适配为本文件类型，处理排序规则和错误映射，并将 `CoreBuiltin::vectorized()` 按 kind 返回能力；随后应增加通过公共表达式调度入口而非直接调用 `IlikeSig` 的回归测试。
- 若为了性能改成原地折叠列，必须复制上游列或证明独占所有权，避免污染后续表达式；还应基准比较当前逐行分配/编译路径与 Go 的列预处理路径。

## 验证依据

- 源文件：`pkg/expression/builtin_ilike_vec.rs`，共 152 行；RustCodeGraph `files --filter` 报告 16 个符号，`node --file ... --offset 1 --limit 260` 展示了完整源码和直接使用文件。
- 核心实现：`pkg/expression/builtin_ilike.rs` 的 `ExpressionError`、`IlikeSig`、`matches`、`cached_pattern` 与 `normalize_ilike`。
- crate 与装配：`pkg/expression/Cargo.toml` 的包名、lib 入口和 `stringutil-dependency`；`pkg/expression/lib.rs` 的生产/测试模块声明。
- 生产调用边：`pkg/expression/builtin.rs` 的 `CoreBuiltinKind::Ilike`、`core_builtin_factory`、`CoreBuiltin::evalInt` 和 `builtinFunc::vectorized`。仓库搜索确认生产路径调用 `eval_int`，未发现对本文件 `vec_eval_int` 或两个折叠辅助函数的生产调用。
- Rust 测试：`pkg/expression/builtin_ilike_vec_test.rs` 验证常量 NULL 的短路顺序；`pkg/expression/builtin_ilike_test.rs` 验证排序规则、ASCII/escape、标量与向量一致性以及常量/列组合；`pkg/expression/builtin_ilike_vec_12_aster_unit_test.rs` 验证缓存、行级 NULL、NULL escape 和非常量 escape 错误。
- Go 对照：`pkg/expression/builtin_ilike_vec.go` 的完整向量流程及 `pkg/expression/builtin_ilike_test.go` 的 `TestIlike`、`TestVectorizedBuiltinIlikeFunc`、`TestVectorizedBuiltinIlikeForConstants`。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证本文恰好包含 11 个固定二级章节，并人工复核“文件为何存在、如何运行、如何安全扩展”以及“尚未接入统一向量调度”的限制均有源码依据。
