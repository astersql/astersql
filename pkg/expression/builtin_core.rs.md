# `pkg/expression/builtin_core.rs`

[源文件：`pkg/expression/builtin_core.rs`](./builtin_core.rs)

## 文件定位

本文件是 `astersql-expression` crate 的内置标量函数核心契约层。`pkg/expression/lib.rs:192-193` 以私有模块 `builtin_core` 装配它，并在 `pkg/expression/lib.rs:364-367` 将其中条目从 crate 根再导出；因此外部模块通常通过 `crate::builtinFunc` 等名称使用它，而不是直接访问模块路径。crate 边界由 `pkg/expression/Cargo.toml` 的 `[package] name = "astersql-expression"` 与 `[lib] path = "lib.rs"` 确认。

文件只有三个顶层生产符号：公开 trait `builtinFunc`，以及 crate 内可见的 `is_core_cache_snapshot_builtin`、`rebuild_core_cache_snapshot_builtin`。前者规定一个具体内置函数签名如何求值、克隆、比较、携带类型/校对/下推元数据；后两者把实例级计划缓存的表达式快照限制在核心函数注册表内。具体函数实现、注册表和 `ScalarFunction` 包装分别位于 `builtin_*.rs`、`pkg/expression/builtin.rs` 与 `pkg/expression/scalar_function.rs`，不在本文件实现。

## 核心职责

1. `builtinFunc` 统一八种返回求值类型的行式接口：Int、Real、String、Decimal、Time、Duration、JSON、VectorFloat32（`builtin_core.rs:73-128`）。返回二元组中的 `bool` 是 SQL NULL 标志，`Error` 单独承载失败，避免把 NULL 与错误混为一类。
2. 为同样八种类型提供默认列式适配器（`builtin_core.rs:130-274`）：遍历 `chunk::Chunk` 的每一行，调用对应 `eval*`，并将值或 NULL 追加到结果 `chunk::Column`。具体签名可以覆盖这些方法实现真正的批量算法。
3. 暴露构造与运行所需元数据：参数、返回类型、 protobuf 函数码、校对器、可选求值属性、扩展函数标志、GROUPING 元数据、额外编码元数据、跨会话共享能力、克隆、相等性、内存估算与向量化声明（`builtin_core.rs:33-71,276-302`）。
4. 维护计划缓存快照的封闭核心函数边界（`builtin_core.rs:305-333`）。快照只接受正式注册表 `crate::funcs` 中的名字，外加由构造器特殊处理的 `Cast`、`GetVar`、`InternalFuncFromBinary`、`InternalFuncToBinary`；重建仍委托规范入口 `NewFunctionBase`，不另造工厂路径。

## 主要符号

- `pub trait builtinFunc: CollationInfo`（`builtin_core.rs:32`）：对象安全的动态分派接口；继承 `CollationInfo`，所以每个签名同时承担字符集、排序规则、 coercibility 与 repertoire 语义。
- `as_any(&self) -> &dyn Any`：供快照、测试和其他运行时代码安全向下转型。`cache_snapshot.rs:210` 据此识别 `ScalarFunction` 外层表达式；具体 builtin 也可按实现类型识别。
- `RequiredOptionalEvalProps` / `AllowedOptionalEvalProps`：默认均为空集合。前者表示求值必需的会话属性，后者表示若存在可以消费、但调用方不必提供的属性。`context_test.rs:211-260` 验证嵌套检查只约束当前 builtin、allowed 属性可读取、未声明属性会触发断言。
- `isExtensionFunction`、`groupingMetaInitialized`、`groupingModeAndMarks`、`restoreGroupingModeAndMarks`、`metadata`：默认分别为 `false`、`None`、`None`、拒绝恢复、`None`。专用签名必须显式覆盖；`cache_snapshot.rs:210-239,298-302` 使用这些钩子拒绝扩展函数/未初始化 GROUPING，并在恢复后写回 GROUPING 状态。
- `SafeToShareAcrossSession`：无默认实现，强制每个实现声明能否跨会话共享。它是生命周期安全承诺，不等同于 `Send + Sync`。
- `evalInt` 至 `evalVectorFloat32`：默认返回带有具体方法名的“未实现”错误。具体签名应只覆盖与自身返回 `EvalType` 对应的方法；调用错误类型入口会显式失败。
- `vecEvalInt` 至 `vecEvalVectorFloat32`：通用逐行回退。固定宽度列先调用相应 `Resize*` 清空，String 使用 `ReserveString` 预留，JSON/Vector 使用带 `EvalType` 的 `Reset`；之后严格按输入行序追加结果。
- `getArgs` / `getArgsMut`、`getRetTp`、`setPbCode` / `PbCode`、`setCollator` / `collator`：提供表达式树、类型、下推协议码和字符串比较器的只读/可变访问边界。
- `equal`、`Clone`、`MemoryUsage`：分别定义语义相等、深拷贝 trait object 和内存估算契约。实现者必须让克隆后的参数及可变状态彼此独立。
- `vectorized` 与 `isChildrenVectorized`：前者由实现声明自身是否有向量化能力；后者默认逐个检查所有参数的 `Expression::Vectorized()`，空参数按 `Iterator::all` 语义为真。两者需共同满足才表示整棵调用可走向量化路径。
- `is_core_cache_snapshot_builtin(name)`：大小写敏感地查询核心注册表或四个特殊构造名。名称规范化发生在 `CachedBuiltinId::from_name`（`cache_snapshot.rs:120-128`），而不是本函数内部。
- `rebuild_core_cache_snapshot_builtin(ctx, name, ret_type, arguments)`：先重复执行白名单检查，失败时返回 `builtin {name} is not registered for plan-cache snapshots`，成功时调用 `crate::NewFunctionBase`。

## 执行流程

普通行式求值的主流程是：上层持有 `ScalarFunction`/`Box<dyn builtinFunc>`，根据表达式返回类型选择对应 `eval*`；具体签名读取 `getArgs()` 中的子表达式并使用传入 `EvalContext` 求值；最后返回 `(value, is_null)` 或 `Error`。本文件不选择求值类型，也不吞掉错误。

走默认向量化回退时，以 `vecEvalInt` 为例（其余七类同构）：先把结果列调整为空；对 `0..input.NumRows()` 逐行调用 `input.GetRow(index)`；`evalInt` 的错误通过 `?` 立即结束；成功后按 `is_null` 调用 `AppendNull` 或 `AppendInt64`；全部行完成后返回 `Ok(())`。因此输出顺序与输入一致，错误之前已写入的前缀不会在本方法内回滚。真正的向量实现可覆盖该方法，但必须保持相同的值、NULL、错误与行序语义。

计划缓存快照流程由 `pkg/expression/cache_snapshot.rs` 驱动：捕获时先拒绝扩展函数、检查核心白名单和 GROUPING 初始化状态，再用小写函数名生成 `builtin:v1:<name>` 稳定 ID（`cache_snapshot.rs:210-239`）；恢复时递归恢复参数，通过 `registered_name()` 再验 ID 和白名单，调用 `rebuild_core_cache_snapshot_builtin`，确认工厂结果确为 `ScalarFunction`，最后恢复 GROUPING 与校对信息（`cache_snapshot.rs:274-305`）。本文件的双重白名单检查是防御性边界，防止伪造或过期快照绕过注册表。

## 数据与状态

本文件不定义结构体、全局可变状态、锁或缓存。trait 方法操作的状态由具体实现持有：参数表达式切片、返回 `FieldType`、protobuf code、`Collator`、函数专用 metadata，以及可能的 GROUPING mode/marks。

默认列式适配器只临时持有当前行求值结果，并原地修改调用方提供的 `chunk::Column`。Int/Real/Decimal/Time/Duration 路径通过 `Resize*` 将逻辑长度置零；JSON 与 VectorFloat32 通过 `Reset(EvalType)` 重置列类型；String 路径调用 `ReserveString(input.NumRows())`。追加到结果列的所有权规则由 `chunk::Column` API 负责，局部 `String`、Decimal 等值不会保存在 trait 对象中。

`metadata()` 返回已经编码的 `Vec<u8>` 副本，默认无数据；`groupingModeAndMarks()` 同样按值返回 mode 与位图。`Clone()` 返回新的 boxed trait object，`getArgsMut()` 则显式允许构造/重写阶段原地替换参数，因此调用者必须避免与并发求值重叠。

## 依赖与调用关系

直接依赖均从 crate 根导入（`builtin_core.rs:22-27`）：标准库 `Any`；构建/求值上下文 `BuildContext`、`EvalContext`；表达式接口 `Expression`；校对接口 `CollationInfo`、`collate::Collator`；错误构造 `Error`、`errors::New`；数据容器 `chunk`；SQL 类型 `types`；可选属性集合 `OptionalEvalPropKeySet`。`pkg/expression/Cargo.toml` 对应声明了 chunk、collate、context、exprctx、parser AST、types、tipb 等工作区依赖，且没有为本文件设置条件 feature。

RustCodeGraph 将目标文件索引为 333 行、43 个符号。精确图查询确认 `NewFunctionBase` 位于 `pkg/expression/scalar_function.rs:746-760`，向下调用 `newFunctionImpl` 与 `defaultScalarFunctionCheck`；注册表 `funcs` 位于 `pkg/expression/builtin.rs:6820`。对本文件两个快照函数执行 `callers/callees` 未返回边，因此以直接引用搜索核验：`cache_snapshot.rs:122,137,217` 调用白名单函数，`cache_snapshot.rs:281` 调用重建函数；除此之外没有生产调用点。

crate 装配关系见 `lib.rs:192-207,364-367`。相关独立 Rust 测试是 `pkg/expression/context_test.rs`（trait 的可选属性契约）和 `pkg/expression/cache_snapshot_test.rs`（核心函数稳定 ID 与工厂恢复边界）；具体内置签名的标量/向量行为由同目录各 `builtin_*_test.rs` 独立覆盖，测试逻辑没有内嵌在生产文件。

## 错误处理与边界

- 调用未覆盖的 `eval*` 会得到 `builtin does not implement evalX`，而不是默认零值。默认 `restoreGroupingModeAndMarks` 同样拒绝不支持 GROUPING 的签名。
- 默认 `vecEval*` 使用 `?` 原样传播首个行式错误；方法不会继续处理后续行，也不会清除错误前已追加的结果前缀。调用方不能把失败后的结果列视为完整输出。
- SQL NULL 由独立 `bool` 表示：当其为真时，适配器忽略返回的占位值并追加 NULL。错误优先由 `Result` 决定。
- 快照白名单是封闭边界。扩展函数在捕获阶段由 `isExtensionFunction()` 拒绝；未知或被移除的稳定 ID 在 `registered_name()` 和重建入口两处被拒绝；返回类型和参数则交由规范工厂继续校验。
- `is_core_cache_snapshot_builtin` 不负责大小写折叠。合法捕获路径先在 `CachedBuiltinId::from_name` 转为 ASCII 小写；直接调用者必须传入注册表的规范名称。
- `vectorized()` 只声明当前签名能力，`isChildrenVectorized()` 只检查子节点能力；任何调度点都不能只看其中一个。默认列式方法存在并不自动令 `vectorized()` 为真。
- `SafeToShareAcrossSession()` 必须保守实现：持有会话可变状态、非隔离缓存或依赖会话身份的签名应返回 false。错误声明可能造成跨会话状态泄漏。

## 并发与资源生命周期

文件本身不启动线程、任务或通道，也不获取锁、网络连接和事务资源。默认求值均为同步借用：`EvalContext` 与输入 `Chunk` 只读借用，结果 `Column` 独占可变借用；Rust 借用规则阻止同一结果列在一次调用期间被其他代码并发修改。

trait 没有 `Send`/`Sync` 超约束；跨线程能力取决于具体实现及其所在包装类型。`SafeToShareAcrossSession` 是语义级共享判定，不能替代 Rust 的线程安全标记。相对地，快照类型的线程安全由 `cache_snapshot_test.rs:20-26` 用 `assert_send_sync` 独立验证，不能反推任意运行时 builtin 都可并发共享。

默认 `isChildrenVectorized()` 每次调用都遍历当前参数，不缓存结果；这与 `getArgsMut()` 可修改子树相容。默认向量回退不保留跨调用缓冲区，资源生命周期止于调用结束。实现高性能覆盖时若引入列池、惰性初始化或锁，必须在具体实现及其独立测试中说明并验证归还、错误清理和并发语义。

## 与 Go 版本的对应关系

Go 的直接对照位于 `pkg/expression/builtin.go:508-585`：`vecBuiltinFunc` 加 `builtinFunc` 覆盖相同八类行式/列式求值、参数、返回类型、pb code、collator、metadata、Clone、MemoryUsage、CollationInfo、可选属性与跨会话共享语义。Rust 将这些接口合并为一个 trait，并增加 `as_any`、`getArgsMut`、扩展函数与 GROUPING 快照钩子，以及计划缓存快照的两个 crate 内函数。

两版的重要实现差异必须保留：Go `baseBuiltinFunc` 的 `vecEval*` 和错误类型 `eval*` 均报告“should never be called”（`builtin.go:321-385`），真正的逐行向量回退在 Go 的其他辅助逻辑中；Rust trait 自身的 `vecEval*` 默认逐行适配。Go `isChildrenVectorized` 在 `builtin.go:389-403` 用 `sync.Once` 缓存并在全为真时初始化列池；Rust 每次遍历子表达式且不分配池。Rust 的 Decimal 行式返回拥有的 `MyDecimal`，Go 返回指针；Rust `metadata` 是已编码 `Vec<u8>`，Go 是 `proto.Message`；Rust pb code 暂以 `i32` 表示，Go 使用 `tipb.ScalarFuncSig`。

`SafeToShareAcrossSession`、可选属性和校对契约仍应尽量与 Go 语义一致。计划缓存稳定 ID/核心白名单是当前 Rust 快照层的接线，Go 同路径文件中未发现 `builtin:v1:` 或 `CachedBuiltinId` 直接对应物，不能宣称这是逐函数同构移植。

## 扩展指南

新增普通 builtin 时，优先在对应 `builtin_<category>.rs` 实现具体签名，而不是修改本 trait：实现正确返回类型的 `eval*`，按实际能力覆盖 `vecEval*`/`vectorized`，并完整实现参数、类型、pb code、collator、Clone、equal、MemoryUsage 与 `SafeToShareAcrossSession`。需要会话属性时准确区分 Required 与 Allowed；需要 GROUPING 或额外 metadata 时覆盖相应钩子。测试应放在同目录独立 `*_test.rs` 文件，并与 Go 对照测试保持边界、NULL 和错误语义一致。

若增加新的求值类型，才需要同步扩展本 trait 的行式/列式接口、`chunk::Column` 写入协议、`ScalarFunction`/evaluator 调度、具体实现与独立测试；这是跨模块兼容变更，不能只在这里加方法。性能风险主要是默认逐行回退的虚调用和逐值追加，应先以行为一致为基线，再用专用批量实现优化。

若新增的核心函数进入 `crate::funcs`，它会自动进入快照白名单；若属于像 Cast/GetVar 一样由构造器特殊处理且不在注册表中，必须审查是否将其加入 `is_core_cache_snapshot_builtin` 的显式列表，并确认 `NewFunctionBase` 能规范重建。同步扩展 `cache_snapshot_test.rs::every_core_snapshot_builtin_has_a_stable_id` 或增加拒绝/往返用例。扩展函数不要加入核心快照白名单，除非同时设计稳定身份、无运行时捕获状态的重建协议。

修改默认列式适配器时，应为所有受影响类型补充独立回归测试，至少覆盖空 Chunk、普通值、NULL、首行/中途错误、结果列预存内容和输出顺序。修改 `isChildrenVectorized` 或共享判定时，还要验证参数重写后的结果与跨会话隔离，避免引入 Go `sync.Once` 式缓存却未处理 `getArgsMut()` 带来的失效问题。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/expression/builtin_core.rs` 确认目标文件与 43 个符号；`node --file ... --offset 1 --limit 520` 读取完整 333 行；`query builtinFunc --kind trait`、`query is_core_cache_snapshot_builtin`、`query rebuild_core_cache_snapshot_builtin`、`node NewFunctionBase`、`node funcs` 核对符号与工厂/注册表关系。精确 `callers/callees` 查询无输出，调用边另以直接引用搜索补齐，未将无输出解释为“无调用者”。
- Rust 源与装配：`pkg/expression/builtin_core.rs`、`pkg/expression/lib.rs:192-207,364-379`、`pkg/expression/scalar_function.rs:746-760`、`pkg/expression/builtin.rs:6820`、`pkg/expression/cache_snapshot.rs:113-163,165-305`。
- crate 声明：`pkg/expression/Cargo.toml`，确认 crate 名称、`lib.rs` 入口、工作区依赖与 `autotests = false`；测试由 `lib.rs` 的 `#[cfg(test)]` 模块或显式 `[[test]]` 装配。
- Rust 独立测试：`pkg/expression/context_test.rs:106-260` 验证 trait 实现及 Required/Allowed 属性；`pkg/expression/cache_snapshot_test.rs:20-26,123-190` 验证快照线程安全、稳定 ID、未知 ID 拒绝和全部核心函数覆盖。未发现与 `builtin_core.rs` 同名的独立测试。
- Go 对照：`pkg/expression/builtin.go:321-403,508-585`；直接引用搜索确认 Go 侧无 `builtin:v1:`/`CachedBuiltinId` 同名实现。仓库中不存在 `pkg/expression/doc.go`，因此无可读取的包级 `doc.go` 合同。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级标题，并人工复核没有把默认回退、快照白名单或 Go 差异写成未经验证的推断。
