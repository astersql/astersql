# `pkg/expression/constant.rs`

## 文件定位

本文件实现 SQL 表达式树中的常量节点 `Constant`，同时容纳三种运行形态：编译期即可确定的普通字面量、在执行 prepared statement 时按序号读取的 `ParamMarker`，以及计划缓存命中后才重新求值的 `DeferredExpr`。它属于 `astersql-expression` crate；`pkg/expression/lib.rs:318-319` 以 `expression_constant` 模块挂载本文件，并在 `pkg/expression/lib.rs:386` 将其公开符号重新导出。crate 的边界、`lib.rs` 入口和 `autotests = false` 由 `pkg/expression/Cargo.toml` 声明。

`Constant` 是通用 `Expression` trait 的一种具体节点。固有求值方法位于本文件，而 trait 对象接线位于 `pkg/expression/core_impl.rs:410-520`；常量展示逻辑另由 `pkg/expression/explain.rs:151-184` 扩展。上层常量折叠和传播分别通过 `pkg/expression/constant_fold.rs`、`pkg/expression/constant_propagation.rs` 识别 `as_constant()`，执行器则通过 `Expression::Eval*` 统一消费常量、列和标量函数。

## 核心职责

1. `tiny_integer`、`NewOne`、`NewSignedOne`、`NewZero`、`NewSignedZero`、`NewUInt64Const*`、`NewInt64Const`、`NewStrConst`、`NewNull*` 构造带 MySQL `FieldType` 元数据的常量；整数宽度、符号标志、`flen` 和 `decimal` 会影响后续类型推导与整数提升。
2. `ParamMarker::GetUserVar` 在每次调用时通过 `ParamValues::GetParamValue(order)` 读取当前绑定值；`Constant::getLazyDatum` 对 prepared 参数和延迟表达式统一执行惰性取值。
3. `Eval` 及八组类型化 `Eval*` 方法把 `Datum` 转成表达式接口要求的 Rust 值，并以返回元组中的布尔量表示 SQL `NULL`。`VecEval*` 则把普通常量广播到整个 `Chunk`，或把延迟表达式转交给其自身的向量化实现。
4. `Clone`、`Equal`/`Equals`、`HashCode`/`CanonicalHashCode`、`Hash64` 和 `MemoryUsage` 为计划复制、缓存键、表达式去重与内存核算提供结构语义。
5. `ConstLevel`、`SafeToShareAcrossSession`、`Vectorized`、`Coercibility`、`Repertoire` 以及索引解析/列重映射方法向优化器报告常量属性。

## 主要符号

- `Constant`（`constant.rs:94-102`）：核心状态包含 `Value: Datum`、可选 `RetType`、可选 `DeferredExpr`、可选 `ParamMarker`、惰性哈希缓存 `hashcode`、展示用 `SubqueryRefID` 和内部可变的 `collation_info`。普通构造器保证 `RetType = Some(...)`；手工构造或迁移接线仍可能产生 `None`，部分方法对此容忍、部分方法会 `unwrap`。
- `ParamMarker`（`constant.rs:104-126`）：只保存零起始 `order`。`GetUserVar` 将 `exprctx::ParamError` 映射为 crate 的通用 `Error`；具体越界语义定义在 `pkg/expression/exprctx/param.rs:25-49`。
- 构造器族（`constant.rs:23-90`）：`NewOne`/`NewZero` 是无符号 `TINY`，signed 版本不带 `UnsignedFlag`；64 位构造器使用 `TypeLonglong`；字符串 `flen` 为 UTF-8 字节长度；`NewNull` 使用 `TINY` 元数据，`NewNullWithFieldType` 保留调用方类型。
- `with_type`、`with_deferred`、`with_subquery`、`with_collation`、`clone_with_value`（`constant.rs:142-176`）：为迁移代码提供显式组装入口。`clone_with_value` 会先深复制表达式和缓存，再替换值，因此调用者若改变值后需要哈希，应确保沿用的缓存仍符合其使用约束。
- `GetType`（`constant.rs:241-253`）：prepared 参数会按当前 `Datum` 动态推导一个新 `FieldType`，每次创建新对象以避免 IndexJoin 等多线程构建路径共享可变类型；普通常量返回 `RetType` 克隆。注意，trait 接线 `pkg/expression/core_impl.rs:458-463` 的 `Expression::GetType` 直接返回存储的 `RetType` 引用，动态参数类型应调用本文件的固有方法或通过既有求值路径取得。
- `Eval`、`EvalInt`、`EvalReal`、`EvalString`、`EvalDecimal`、`EvalTime`、`EvalDuration`、`EvalJSON`、`EvalVectorFloat32`（`constant.rs:363-537`）：覆盖通用 `Datum` 与全部表达式求值类型。
- `VecEval*`（`constant.rs:255-343`）：普通常量调用 `genVecFromConstExpr`；该函数在 `pkg/expression/vectorized.rs:26-148` 中一次标量求值后按输入行数广播，空输入会重置结果，延迟表达式则直接走自身向量路径。
- `Hash64`/`Equals` 与 `getHashCode`（`constant.rs:576-623`）：前一组包含返回类型和 collation，并区分 deferred/parameter/plain 三种结构；字节哈希对 deferred 表达式委托其哈希，对参数编码 `parameterFlag + order`，对普通常量编码 `constantFlag + Datum`。
- `Coercibility`/`Repertoire`（`constant.rs:652-675`）：排序规则强制性按需推导并缓存；NULL、非字符串、字符串分别由 `deriveCoercibilityForConstant` 映射为 ignorable、numeric、coercible（`pkg/expression/collation.rs:386-395`）。字符 repertoire 对非字符串和 ASCII 字符集返回 `ASCII`，其余字符串返回 `UNICODE`。

## 执行流程

普通常量的标量求值路径是：构造器写入 `Value` 与 `RetType`；`Expression` trait 调用由 `core_impl.rs` 转发到本文件；`effectiveDatum` 发现没有 lazy 来源后复制 `Value`；目标 `Eval*` 先检查 `TypeNull` 或 `Datum::IsNull`，再根据 `Datum::Kind` 直接读取或调用 `ToInt64`、`ToFloat64`、`ToString`、`ToDecimal` 等转换。二进制字面量/MySQL BIT 和 hybrid 类型在整数、浮点求值中有专门转换分支。

prepared 参数路径是：解析/计划构建阶段把参数序号放入 `ParamMarker`；执行阶段 `getLazyDatum` 调用 `ParamMarker::GetUserVar`；上下文从本次 EXECUTE/COM_EXECUTE 的参数数组返回 `Datum`；`GetType` 可从该值推导运行时类型；随后类型化求值器做 NULL 检查和必要转换。因读取发生在每次求值时，同一缓存计划可接受不同批次绑定值。

延迟表达式路径是：`with_deferred` 保存原 `ScalarFunction`；`getLazyDatum` 在当前行调用其 `Eval`。通用 `Eval` 对非 NULL 结果按 `RetType` 转换；十进制结果由 `adjustDecimal` 在实际小数位少于目标位数时按 `ModeHalfUp` 补齐。类型化求值器同样通过 `effectiveDatum` 每次重算。向量路径不先求一次再盲目广播，而是直接委托 `DeferredExpr::VecEval*`，保留逐行或上下文相关语义。

优化与展示路径中，`ConstLevel` 把普通常量标为 `ConstStrict`，把参数和 deferred 常量标为 `ConstOnlyInContext`；索引解析、虚拟表达式解析和列重映射对无列引用的常量只返回克隆。`StringWithCtx` 按 redact 策略输出真实值、标记包裹值或 `?`，子查询来源附加 `ScalarQueryCol#ID(...)`；完整 EXPLAIN 入口在 `explain.rs` 中先求值并处理无法识别的常量。

## 数据与状态

`Value` 是普通常量的真实值，也是结构相等和哈希的一部分。对于 `ParamMarker`/`DeferredExpr`，当前有效值来自上下文，但存储的 `Value` 仍参与 `Equal` 和 `Equals` 的最终比较；因此不能把“成功调用了一次 `Eval`”理解为它会回写 `Value`。`RetType` 描述 SQL 类型、无符号标志、长度、小数位、字符集等元数据，求值与 collation 推导都依赖它。

`hashcode` 是按需填充的字节缓存。`Clone` 会复制缓存，`clone_with_value` 也继承该缓存；修改 `Value`、`RetType`、参数序号或 deferred 表达式后重用旧缓存会破坏哈希与内容一致性，所以扩展代码应优先构造新 `Constant`，或在可变修改时显式失效缓存。`Hash64` 不使用这段缓存，而是直接遍历当前字段。

`collation_info` 使用内部可变状态，使 `Coercibility(&self)` 可以首次推导后缓存结果；`Repertoire` 也优先读取已缓存值。`SubqueryRefID` 只参与显示，不进入当前 `Hash64`、`Equals` 或字节哈希。`EMPTY_CONSTANT_SIZE` 是结构体静态大小，`MemoryUsage` 再加 `Datum`、哈希容量和 `FieldType` 的动态占用；它不递归计算 boxed deferred 表达式或 collation 字符串的全部内存。

## 依赖与调用关系

上游方面，RustCodeGraph 的文件节点显示 `pkg/expression/constant.rs` 被 98 个文件使用。直接语义入口包括：`constant_fold.rs` 在折叠 CASE/IF/控制函数时读取常量，`constant_propagation.rs` 从等值条件抽取 `Column + Constant`，`core_support.rs` 提供 `as_constant` 等桥接，`chunk_executor.rs` 通过 `Eval*` 执行行表达式，DDL 的 `pkg/ddl/storage_class.rs` 也直接求值整型常量。`pkg/expression/core_impl.rs:411-520` 是 `Box<dyn Expression>` 到本文件固有方法的关键分派层。

下游方面，`Constant` 依赖 `astersql-types` 的 `Datum`、`FieldType` 和具体 SQL 值类型，依赖 parser/mysql 的类型码与标志，依赖 `astersql-util-chunk` 的行/列批结构，依赖 codec 生成稳定字节哈希，依赖 collate 做二进制比较，并通过 planner cascades base 的 `Hasher` 实现结构哈希。这些依赖由 `pkg/expression/Cargo.toml` 中的 `types-dependency`、`parser-mysql-dependency`、`chunk-dependency`、`codec-dependency`、`collate-dependency` 和 `planner-base-dependency` 明确声明。

RustCodeGraph 能列出文件级使用关系并解析 `NewOne` 等符号节点，但本次索引对目标 Rust `impl Constant` 的部分方法没有产生独立 callers/callees 节点，且 `NewOne` 的图边结果为空；因此本文对具体调用点的结论同时采用索引化文件源码与精确文本搜索核验，不把空图边解释为“无人调用”。

## 错误处理与边界

- 参数序号越界由 `ParamValues::GetParamValue` 返回 `ParamError::IndexExceedsParamCount`，`GetUserVar` 转成通用 `Error`；标量求值继续向上传播。`StringWithCtx` 是展示路径，读取失败时降级为 `?`。
- `GetType` 在参数读取失败时返回 `None`；普通路径返回可选 `RetType`。`adjustDecimal`、`deriveCoercibilityForConstant` 和 trait `GetType` 等位置会 `unwrap`，所以可执行常量必须满足 `RetType` 已设置这一不变量。
- SQL NULL 同时由 `RetType.TypeNull` 或 `Datum::IsNull` 识别。各具体 `Eval*` 返回该类型的零值加 `is_null = true`，调用者不得把零值当作 SQL 实值；`constant_test.rs:93-129` 专门验证 TypeNull 能在读取不匹配的占位 Datum 前短路。
- `EvalInt` 对 binary literal/MySQL BIT 使用无符号转换后转成 `i64`，字符串和 hybrid 类型走上下文感知转换；转换、截断、十进制舍入错误都原样上抛。
- `Equal` 只接受另一 `Constant`，任一惰性求值报错即返回 `false`，之后却比较两边存储的 `Value`；它用于表达式语义比较，不等价于比较当前上下文里两个 lazy 结果。结构级 `Equals` 还比较类型、collation、deferred 表达式、参数序号和值。
- `getHashCode` 只要缓存非空就直接返回。任何新增可变字段若影响相等性，都必须同步纳入哈希并处理缓存失效，否则会违反相等对象同哈希的不变量。

## 并发与资源生命周期

`Constant` 没有锁、线程或异步任务。普通值和类型由对象拥有；`DeferredExpr` 由 `Box<dyn Expression>` 独占，克隆时通过 `CloneExpr` 深复制；`ParamMarker`、`Datum`、`FieldType` 和哈希缓冲也会复制，避免计划缓存副本相互改写。`SafeToShareAcrossSession` 只有在 deferred 表达式也允许共享时才返回真；参数标记本身不保存会话值，实际值始终从调用时的 `EvalContext` 读取。

并发敏感点是参数类型推导：固有 `GetType` 每次创建新的 `FieldType`，其注释明确覆盖 IndexJoin 内部执行器并行构建时的数据竞争风险。另一方面，`hashcode` 需要 `&mut self` 才写入，而 `collation_info` 的缓存通过内部可变性更新；共享同一个对象前必须遵守 trait 的 `Send + Sync` 约束以及现有封装，不应绕过 API 并发修改内部缓存。向量求值借用调用方提供的 `Column` 并在调用内完成填充，不持有输入 `Chunk` 或输出列。

## 与 Go 版本的对应关系

主要语义逐段对应 `pkg/expression/constant.go`：构造器（Go 36-141 / Rust 23-90）、`Constant` 和 `ParamMarker`（Go 143-165 / Rust 93-108）、参数与展示（Go 167-210 / Rust 179-226）、类型和向量求值（Go 228-308 / Rust 241-343）、惰性/类型化求值（Go 310-505 / Rust 345-537）、相等/常量级别/哈希/解析/内存（Go 507-673 / Rust 539-698）。Rust 以拥有值和 `Option` 替代 Go 指针/nil，并把 Go 的三返回值 `(value, isNull, error)` 收拢为 `Result<(value, is_null), Error>`。

两版都保留以下关键行为：prepared 参数每次从上下文读取；动态 `GetType` 每次返回独立类型对象；普通常量向量广播而 deferred 表达式委托自身；TypeNull 与 NULL Datum 均短路；十进制按目标 frac half-up 调整；常量无列索引可解析；哈希区分 deferred、parameter 和 plain constant。

已核实的差异包括：Rust `StringWithCtx` 对 MySQL time 补齐声明的小数位，而当前 Go `StringWithCtx` 直接使用 `TruncatedStringify`；Rust `Clone` 对 deferred 表达式调用 `CloneExpr`，Go 测试 `TestDeferredExprNotNull` 的旧断言关注其克隆结果；Rust 的拥有权模型使 `Decorrelate`、`ResolveIndices`、`RemapColumn` 返回克隆，而 Go 可直接返回原指针；Rust 在本文件实现 `Repertoire`，Go 对应能力由包内其他接口/文件组织。Rust 固有 `GetType` 返回 `Option<FieldType>`，但 trait 接线返回存储类型引用，这一点与 Go 单一方法模型不同，扩展参数类型逻辑时必须同时审查接线层。

Rust 独立测试 `pkg/expression/constant_test.rs` 当前覆盖构造器元数据、typed NULL、克隆/哈希隔离、严格常量属性、参数序号和 TypeNull 的具体求值短路。Go `pkg/expression/constant_test.go:336-587` 还覆盖多种 prepared 参数动态类型、deferred 错误/NULL/具体值、向量广播与 selection、`GetType` 返回对象不共享以及 `Hash64`/`Equals`。这些 Go 用例是移植语义证据，但不能当作 Rust 侧已经运行或已完整覆盖的测试证据。

## 扩展指南

新增常量类型或构造器时，应同时设置正确 `Datum` 和完整 `FieldType`，检查 `Eval*` 的 Kind 分支、`vectorized.rs::genVecFromConstExpr` 的目标类型广播、collation/repertoire、哈希/相等和 `MemoryUsage`。回归测试应放在独立的 `pkg/expression/constant_test.rs`，不要嵌入生产文件；至少覆盖正常值、SQL NULL、类型转换错误和元数据。

修改 prepared 参数或 deferred 语义时，优先从 `getLazyDatum`、`effectiveDatum`、固有 `GetType` 和八组标量/向量入口切入，并同步审查 `core_impl.rs` 的 `Expression for Constant` 分派。尤其要分别验证：每次执行读取新绑定值、越界错误传播、动态类型不共享、deferred 的行/批求值、`SafeToShareAcrossSession` 与 `ConstLevel`。Go 的 `TestDeferredParamNotNull`、`TestDeferredExprNotNull`、`TestVectorizedConstant`、`TestGetTypeThreadSafe` 可作为待移植用例清单。

修改任何参与结构身份的字段时，必须成对维护 `Hash64`/`Equals` 和字节 `getHashCode`，并定义缓存失效策略；还应测试普通、参数、deferred 三种节点。修改显示或脱敏规则时同时审查 `StringWithCtx` 和 `pkg/expression/explain.rs`，覆盖 redact OFF/MARKER/ON、参数错误、时间小数位和 `SubqueryRefID`。

兼容性风险主要是 MySQL 类型/NULL/舍入与 Go 行为漂移；性能风险主要是把普通常量从“一次求值后广播”退化为逐行转换、频繁克隆 deferred 树，或错误扩大哈希/内存遍历。并发风险集中在动态 `FieldType` 与内部缓存共享，新增缓存前应明确其可变性和跨会话共享条件。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file` 完整读取 `pkg/expression/constant.rs`，文件节点报告 98 个使用文件；并查询 `NewOne`、`NewInt64Const` 等符号及 callers/callees，记录到上述 impl 方法/部分函数缺边的索引限制。
- Rust 源码：`pkg/expression/constant.rs:1-698`；模块与公开边界 `pkg/expression/lib.rs:300-339, 380-388, 635-652`；trait 定义 `pkg/expression/expression.rs:267-335`；trait 实现 `pkg/expression/core_impl.rs:410-520`；向量广播 `pkg/expression/vectorized.rs:23-148`；collation 推导 `pkg/expression/collation.rs:381-395`；展示 `pkg/expression/explain.rs:151-184`；参数错误契约 `pkg/expression/exprctx/param.rs:25-57`。
- crate 配置：`pkg/expression/Cargo.toml`，核对 crate 名、`lib.rs` 入口、关闭自动测试、依赖边界和 `go-package = "pkg/expression"` 移植元数据。
- 对照实现与测试：完整读取 `pkg/expression/constant.go:1-673`、`pkg/expression/constant_test.rs:1-129`，并复核 `pkg/expression/constant_test.go:336-587` 的参数、deferred、向量、并发类型和哈希测试。目标包没有 `doc.go`，因此以 crate 根 `pkg/expression/lib.rs` 的包级说明作为最近契约入口。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令确认本文存在且恰好包含 11 个固定二级章节，并人工复核没有把 Go 测试或图索引缺边误报为 Rust 已验证行为。
