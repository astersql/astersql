# `pkg/expression/core_impl.rs`

## 文件定位

[`core_impl.rs`](core_impl.rs) 是 `astersql-expression` crate 的核心 trait 适配层。`lib.rs` 通过 `#[path = "core_impl.rs"] mod core_impl;` 私有装入它；文件本身不导出新类型，而是为 `Column`、`CorrelatedColumn`、`Constant`、`ScalarFunction` 补齐 `VecExpr`、`CollationInfo`、`SafeToShareAcrossSession`、`StringerWithCtx`、`base::Hash64`、`base::Equals` 和 `Expression` 实现。因而调用者通常面向 `dyn Expression` 或这些父 trait，不直接引用 `core_impl` 模块。

crate 边界由 [`Cargo.toml`](Cargo.toml) 定义，包名为 `astersql-expression`，库入口为 `lib.rs`。本文件经 `use crate::*` 使用 crate 根重导出的 chunk、codec、types、exprctx、base 以及表达式具体类型；它没有条件编译项、模块级常量或自有状态类型。

## 核心职责

1. 用 `forward_vec_expr!` 将四种核心表达式的八类向量化求值入口及 `Vectorized` 查询转发给同名固有方法，统一满足 `VecExpr`。
2. 将字符集、排序规则、coercibility 与 repertoire 元数据接入 `CollationInfo`。`Column`、`Constant` 直接委托其 `collation_info` 字段，`CorrelatedColumn` 委托内层 `column`，`ScalarFunction` 委托自身固有方法。
3. 为表达式对象补齐跨会话共享判断、带上下文的 Explain 字符串以及结构哈希/相等接口。
4. 为四种具体类型实现对象安全的 `Expression` 总接口，使上层能以 `ExprBox = Box<dyn Expression>` 统一执行逐行求值、克隆、遍历、解关联、下标解析、列重映射、Explain、哈希和内存统计。
5. 只承担接口桥接和少量组合逻辑；具体取值、类型转换、列定位、函数执行及多数错误构造仍位于 `column.rs`、`constant.rs`、`scalar_function.rs` 等固有实现中。

## 主要符号

- `forward_vec_expr!($ty)`：生成 `VecExpr` 实现，覆盖 `Vectorized`、`VecEvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`。宏实例仅有四个核心表达式类型。
- `direct_collation!($ty, $field)`：为具有直接排序规则字段的 `Column`、`Constant` 生成 `CollationInfo` 实现。`SetCoercibility` 使用共享引用，具体内部可变性由被委托对象负责；其他可变 setter 接受 `&mut self`。
- `impl CollationInfo for CorrelatedColumn`：所有元数据操作都落到 `self.column`，确保关联列与它代表的普通列保持一致。
- `impl CollationInfo for ScalarFunction`：适配一个签名差异；trait 的 `SetCharsetAndCollation(charset, collation)` 被组装成二元组再传给固有方法。
- `safe_share!($ty)`：把 `SafeToShareAcrossSession` 转发给四种具体类型。是否真正安全由具体表达式及其子表达式决定，本文件不缓存结论。
- 四个 `StringerWithCtx` 实现：若调用方不给 `ParamValues`，统一使用 `exprctx::EmptyParamValues`；`CorrelatedColumn` 直接使用内层列的文本。
- `base_hash!($ty, $hash, $equals)`：注册 `base::Hash64` 和 `base::Equals`。`Equals` 先通过 `Any` 做运行时类型检查；类型不匹配时返回 `false`，其中 `ScalarFunction` 保留其固有 `Equals(&dyn Any)` 行为。
- `basic_explain`：用空参数和值不脱敏模式调用 `StringerWithCtx` 的私有辅助函数。当前仓库搜索未发现调用，是本文件内尚未接线的辅助符号，不能作为现有主流程入口。
- `impl Expression for Column`：列本身不相关、非常量；下标解析与重映射可能失败；`GetTypeMut`、哈希等通过克隆或固有实现适配 trait 签名。
- `impl Expression for Constant`：不依赖 schema 的解析/重映射均返回深克隆或成功；规范化 Explain 固定为 `"?"`；常量级别由固有实现决定。
- `impl Expression for CorrelatedColumn`：`IsCorrelated` 恒为 `true`；若 schema 已包含内层列，`Decorrelate` 返回普通 `Column`，否则保留关联列；下标解析是成功的恒等操作。
- `impl Expression for ScalarFunction`：对参数树递归执行解关联、下标解析和虚拟表达式解析；`all` 在首个失败参数处短路；求值、类型、重映射、Explain 和哈希由固有实现承担。

## 执行流程

典型逐行执行链为：上层持有 `ExprBox` 或 `&dyn Expression`，依据期望结果调用 `Eval*`；动态分派进入本文件对应具体类型的 trait 实现；桥接方法再调用该类型的同名固有实现，并原样返回 `(值, is_null)` 或 `Error`。批量执行同理，只是从 `VecExpr::VecEval*` 进入，输入为 `chunk::Chunk`、输出写入 `chunk::Column`。

表达式树重写遵循以下分支：

1. `Traverse` 先克隆当前节点，再交给 `TraverseAction::Transform`；`ScalarFunction` 的 `Clone()` 已包含函数节点的拥有权语义。
2. `Column`、`Constant` 的 `Decorrelate` 返回自身克隆；`CorrelatedColumn` 根据 `Schema::Contains` 决定降为普通列还是保留关联节点。
3. `ScalarFunction::Decorrelate` 先 `clone_scalar`，再逐个以子节点的动态 `Decorrelate` 结果替换参数，最终返回新树，不修改原对象。
4. `ResolveIndices` 的公开形式返回新对象；`resolveIndices` 是原地版本。标量函数递归处理全部参数，遇到第一个错误立即返回。
5. `ResolveIndicesByVirtualExpr` 返回新对象和布尔成功标记；标量函数使用 `Iterator::all`，任一参数失败即返回 `false`。
6. `RemapColumn` 对列及关联列调用固有映射逻辑；常量忽略映射并克隆；标量函数递归映射参数。

结构身份路径与求值路径分离：`Hash64/Equals` 服务于基于 `Any` 的结构集合，`Expression::HashCode/CanonicalHashCode` 服务于表达式标识和语义规范化。调用方不应混用两套契约，也不应把 Explain 文本当作身份键。

## 数据与状态

本文件不定义持久状态，只读取或转交以下数据：

- `Column` 的 `RetType`、`UniqueID`、`Index` 及 `collation_info`。`Expression::GetTypeMut` 对 `RetType` 执行 `unwrap`，隐含不变量是进入核心表达式接口的列必须已具有返回类型。
- `Constant` 的 `RetType`、值、可能的延迟表达式及 `collation_info`。规范化 Explain 隐去具体常量值，稳定计划摘要。
- `CorrelatedColumn` 的内层 `column` 与运行期关联值。其 `Expression::HashCode` 写入 `correlatedColumn` 标记和内层列 `UniqueID`，刻意不把运行期值纳入身份。
- `ScalarFunction` 的 `RetType`、函数实现和参数树。递归改写都作用于克隆或明确的可变接收者。
- `HashMap<i64, Column>` 以列 `UniqueID` 为重映射键；schema 下标与虚拟表达式选择的具体规则由 `Column` 固有方法实现。

`ExprBox` 的 `Clone` 在 `expression.rs` 中调用这里提供的 `CloneExpr`，所以 trait 对象克隆是深层表达式克隆，而非复制裸指针。独立测试 `core_impl_test.rs::expression_boxes_are_deeply_cloneable` 验证列的 `UniqueID` 与 `Index` 在克隆后保留。

## 依赖与调用关系

上游以 trait 动态分派为主。直接源码证据包括：`expression.rs` 中 `ExprBox::clone`、虚拟列构建的 `ResolveIndices`；`chunk_executor.rs`、`aggregation/descriptor.rs`、`core_support.rs`、`planner_bridge.rs` 等对 `CloneExpr` 或 `SafeToShareAcrossSession` 的调用。RustCodeGraph 将本文件标为被 15 个文件使用，但对 `ResolveIndices`、`Decorrelate`、`EvalInt` 的多态 callers/callees 查询没有返回具体边，因此这些直接用法由精确源码搜索补充验证。

下游关系如下：

- `VecExpr` 和所有 `Expression::Eval*` 调用 `column.rs`、`constant.rs`、`scalar_function.rs` 中的固有求值方法；`CorrelatedColumn` 的固有实现位于 `column.rs`。
- 排序规则接口依赖 `collation.rs` 的 `CollationInfo`、`Coercibility`、`Repertoire` 和具体元数据容器。
- Explain 缺省上下文依赖 `exprctx::EmptyParamValues`，脱敏常量依赖 `errors::RedactLogDisable`。
- 行列数据依赖 `astersql-util-chunk`，值与字段类型依赖 `astersql-types`，关联列哈希编码依赖 `astersql-util-codec`；这些均由 `Cargo.toml` 的路径依赖接入 crate 根。
- 结构哈希和相等依赖 crate 根的 `base::Hasher`、`base::Hash64`、`base::Equals`，运行时降型依赖 `std::any::Any`。

## 错误处理与边界

所有求值、下标解析与重映射错误均使用 `Result<_, Error>` 原样传播；本文件没有吞错、重试或错误降级。`Column::ResolveIndices`、列重映射、关联列重映射及 `ScalarFunction::RemapColumn` 的错误由固有实现构造，标量函数递归路径通过 `?` 在首错处停止。

需要调用方维护的边界包括：

- `Column`、`Constant`、`CorrelatedColumn.column` 与 `ScalarFunction` 的 `RetType` 在 `GetTypeMut` 或部分 `GetType` 路径必须为 `Some`，否则 `unwrap` 会 panic；本层不补默认类型。
- `base::Equals` 面对错误具体类型安全地返回 `false`，而不会 panic。
- 缺失 `ParamValues` 被转换为 `EmptyParamValues`，避免 Explain 路径因空上下文失败；这不等于准备语句参数有值。
- `Constant` 的 schema 解析、虚拟表达式解析和重映射是刻意的恒等成功，不代表其延迟表达式一定已求值。
- `CorrelatedColumn::Decorrelate` 仅以 `Schema::Contains` 为依据；schema 不包含目标列时必须保留相关性。
- `ScalarFunction` 的虚拟表达式解析布尔值只说明所有参数均成功，不携带哪个参数失败的诊断。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件句柄或对象池，也没有 `unsafe` 代码。每次求值只借用 `EvalContext`、输入行/块和输出列，生命周期限定在调用期间。

跨会话复用由 `SafeToShareAcrossSession` 显式守卫：桥接层只转发具体类型结论，尤其不能因为对象实现了 `Send/Sync` 或可克隆就推断它可跨会话共享。上层实例计划缓存应继续调用该 trait；`planner_bridge.rs` 和生成的 builtin 线程安全逻辑提供了实际调用证据。

重写接口主要返回拥有的 `Box<dyn Expression>`：列、常量和标量函数通过克隆隔离修改；可变的 `resolveIndices*` 则明确要求独占 `&mut self`。排序规则的 `SetCoercibility(&self, ...)` 是一个例外，其内部同步/可变性由 `collation_info` 或具体类型实现负责，本文件不增加额外同步保证。

## 与 Go 版本的对应关系

Rust 的 `Expression`、`VecExpr`、`SafeToShareAcrossSession` 与 `CollationInfo` 分别对应 Go `pkg/expression/expression.go` 及排序规则接口；四种具体类型对应 `column.go`、`constant.go`、`scalar_function.go`。逐行/向量求值类型集合、`ConstLevel`、解关联、下标解析、重映射、Explain、哈希和内存统计入口均保持 Go 接口意图。

已核对的关键一致性包括：关联列仅在 schema 含内层列时解关联；列下标解析找不到列时返回错误；常量的 schema 操作为恒等成功；标量函数递归处理参数并传播首个错误；规范化常量 Explain 使用 `?`；跨会话共享判断委托具体表达式。

Rust 为所有返回表达式的路径使用拥有的 `ExprBox`，因此比 Go 接口指针语义更明确。特别是本文件的 `ScalarFunction::Decorrelate` 先克隆后改写，而 Go `scalar_function.go::Decorrelate` 会原地替换接收者参数；Rust 调用方不应依赖 Go 的原地副作用。Rust 还用 `as_any/as_any_mut` 和 `Any::downcast_ref` 代替 Go 类型断言。数值返回也适应 Rust 所有权：例如 decimal/JSON/vector 返回拥有值而非 Go 的可空指针形态，可空性仍由布尔值表达。

## 扩展指南

新增第五种核心表达式类型时，至少应逐项评估并实现本文件覆盖的七组 trait；仅实现 `Expression` 的部分方法不够，因为其父 trait 是编译期约束。若类型只是现有表达式包装器，应明确 Collation、共享安全、哈希、Explain 和解关联究竟委托包装层还是内层，避免身份与显示语义分裂。

增加新求值类型时，需要同步修改 `VecExpr`、`Expression` trait、`forward_vec_expr!` 以及四个 `Expression` 实现，并在各具体类型固有实现中提供行为；Go `Expression`/`VecExpr` 接口及相应测试也应同步核对。增加树重写操作时，应同时设计拥有返回与原地版本，保持 `ResolveIndices`/`resolveIndices` 的现有约定。

测试必须放在独立 Rust 测试文件，不应内嵌到 `core_impl.rs`。最直接位置是 [`core_impl_test.rs`](core_impl_test.rs)；具体列、常量、标量函数行为还应分别扩展 `column_test.rs`、`constant_test.rs`、`scalar_function_test.rs`。应覆盖：四种动态分派求值、错误传播、错误具体类型相等、缺省参数 Explain、相关列解关联两分支、递归解析首错、虚拟表达式短路、缺失列映射和跨会话安全。

兼容性风险集中在 trait 对象 API 与 Go 行为偏差；正确性风险集中在递归时遗漏子节点、错误地共享会话状态及哈希/相等不一致；性能风险集中在无意增加深克隆、重复类型计算或破坏向量化转发。修改宏会同时影响四种类型，必须逐类型验证。

## 验证依据

- RustCodeGraph：`status` 显示项目索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`query core_impl` 定位目标文件及其中的 `Vectorized`、各 `VecEval*`、`Eval*`、Collation 方法和测试文件；`node --file pkg/expression/core_impl.rs` 读取了完整 767 行，并报告该文件被 15 个文件使用。
- RustCodeGraph 限制：对 `basic_explain` 的精确查询定位到第 290 行；对多态名称 `ResolveIndices`、`Decorrelate`、`EvalInt` 执行 callers/callees 未返回边。因此调用关系又由 `rg` 精确检查 `pkg/expression` 内的 trait 调用点，未把缺失图边写成“无调用”。
- 读取的 Rust 边界与入口：`pkg/expression/lib.rs`、`Cargo.toml`、`expression.rs`、`collation.rs`、`column.rs`、`constant.rs`、`scalar_function.rs`。
- 读取的 Rust 测试：`core_impl_test.rs`；并通过源码搜索核对 `column_test.rs`、`constant_test.rs`、`scalar_function_test.rs` 中与共享安全、解关联、重映射相关的用例入口。
- 读取的 Go 对照：`expression.go` 的接口定义，`column.go` 的关联列/普通列解关联、下标解析和重映射，`constant.go` 的恒等解析，`scalar_function.go` 的递归解关联、解析和重映射。
- 人工复核结论：文件存在的原因是集中完成核心具体类型到对象安全 trait 的适配；运行时以动态分派进入、以固有方法完成真实算法；安全扩展必须同时维护父 trait、四类适配、独立测试及 Go 语义对照。
