# `pkg/expression/builtin_threadsafe_generated.rs`

## 文件定位

本文件是 `astersql-expression` crate 内的生成式线程安全策略内核。crate 根在 `pkg/expression/lib.rs` 通过 `#[path = "builtin_threadsafe_generated.rs"] mod builtin_threadsafe_generated_kernel;` 私有挂载它；因此它不是独立 crate，也不直接形成外部公共模块，而是供同一 crate 的内建函数运行时、正式工厂包装和测试使用。`pkg/expression/Cargo.toml` 指定该 crate 的库入口为 `lib.rs`，本文件自身只使用标准库原子类型以及 crate 内的 `expression_builtin`、unsafe 签名清单。

文件顶部明确标注其为生成代码。与 Go 为每个具体签名生成一个方法不同，Rust 用一套动态内建运行时承载所有正式签名，本文件保留 Rust 正式运行时的完整安全签名清单并集中实现 Go 同形的递归共享判定。这里的“线程安全”具体指表达式树能否跨会话共享，目的在于避免把会话敏感状态泄漏到另一个会话，并非对函数求值正确性的全面证明。清单并非与当前 Go 生成文件逐项相同：Rust 将 `builtinUncompressSig` 归为递归安全，而 Go 将其归为 unsafe，详见“与 Go 版本的对应关系”。

## 核心职责

1. `GENERATED_THREADSAFE_SIGNATURES` 保存 510 个 Rust 正式运行时可递归判定的内建签名名；`GENERATED_SIGNATURE_COUNT` 从切片长度派生，避免另存一份可能漂移的计数。
2. `GeneratedThreadSafetyPolicyForSignature` 联合本文件的安全清单和 `builtin_threadunsafe_generated_kernel` 的不安全清单，把正式签名映射为 `Recursive` 或 `Never`。不在两份清单中的名称返回 `None`，采取保守拒绝策略。
3. `generatedSignatureSafeToShareAcrossSession` 将签名策略应用到任意参数切片：只有 `Recursive` 才检查子表达式；`Never`、缺失签名和未知签名均直接返回 `false`。
4. `safeToShareAcrossSession` 递归检查所有参数，并把布尔结果以 `0/1/2` 三态原子缓存保存，供后续调用快速返回。
5. `baseBuiltinFunc::GeneratedSafeToShareAcrossSession` 把上述通用逻辑接到 Rust 动态内建函数基类的 `generatedSignature`、参数列表和缓存字段上。

## 主要符号

- `GeneratedThreadSafetyPolicy::{Recursive, Never}`：生成签名的封闭策略枚举。`Recursive` 表示结果取决于全部参数，`Never` 表示无条件禁止跨会话共享。
- `GeneratedThreadSafetyPolicyForSignature(&str) -> Option<GeneratedThreadSafetyPolicy>`：先查 `IsGeneratedThreadsafeSignature`，再查 `builtin_threadunsafe_generated_kernel::IsGeneratedThreadunsafeSignature`；安全清单优先，未知返回 `None`。
- `generatedSignatureSafeToShareAcrossSession<T, F>(Option<&str>, &AtomicU32, &[T], F) -> bool`：策略分发入口。泛型回调 `F: FnMut(&T) -> bool` 让算法不绑定某一种表达式 trait 对象，也便于独立测试探针复用。
- `safeToShareAcrossSession<T, F>(&AtomicU32, &[T], F) -> bool`：三态缓存与递归短路算法。`0` 表示未计算，`1` 表示安全，`2` 表示不安全；非零但不是 `1` 的值也按不安全处理。
- `baseBuiltinFunc::GeneratedSafeToShareAcrossSession(&self) -> bool`：crate 内可见的基类适配器，回调每个参数的 `Expression::SafeToShareAcrossSession`。
- `GENERATED_THREADSAFE_SIGNATURES: &[&str]`：从 `builtinASCIISig` 到 `builtinYearWeekWithoutModeSig` 的排序清单，当前长度由测试固定为 510。
- `GENERATED_SIGNATURE_COUNT` 与 `IsGeneratedThreadsafeSignature`：分别提供派生计数和清单成员查询。成员查询使用切片 `contains`，时间复杂度随清单长度线性增长。

本文件没有条件编译项、struct、trait 或错误类型；唯一 `impl` 是对 `baseBuiltinFunc` 的固有方法扩展。除该方法为 `pub(crate)` 外，策略枚举、查询函数、通用算法和清单常量均声明为 `pub`，但所在模块在 `lib.rs` 中仍是私有模块。

## 执行流程

以正式生成签名的共享检查为例，流程如下：

1. `pkg/expression/builtin.rs` 中 `GeneratedPolicyBuiltin::SafeToShareAcrossSession` 把静态签名名、自己的 `AtomicU32` 缓存和底层函数参数传给 `generatedSignatureSafeToShareAcrossSession`。另一条基类路径是 `baseBuiltinFunc::SafeToShareAcrossSession` 调用本文件的 `GeneratedSafeToShareAcrossSession`。
2. `generatedSignatureSafeToShareAcrossSession` 对 `Option<&str>` 使用 `and_then(GeneratedThreadSafetyPolicyForSignature)`。没有签名不会进入递归检查。
3. 策略查询先在 510 项安全清单中查找；未命中时再查独立的 unsafe 生成清单。命中安全清单得到 `Recursive`，命中 unsafe 清单得到 `Never`，两者都未命中得到 `None`。
4. 仅 `Recursive` 调用 `safeToShareAcrossSession`。后者先以 `Ordering::SeqCst` 读取缓存；值为 `1` 立即返回 `true`，其他非零值立即返回 `false`。
5. 缓存为 `0` 时，`args.iter().all(...)` 按顺序调用子表达式判定；首个 `false` 会短路，空参数切片按 `Iterator::all` 语义为 `true`。
6. 算法将结果以 `1` 或 `2` 用 `SeqCst` 写回缓存并返回。本次调用之后，在参数和策略不变的前提下，后续检查不再遍历参数。

`baseBuiltinFunc::SetGeneratedSignature` 是缓存生命周期的重要上游：它拒绝未知签名，成功设置签名后把缓存重置为 `0`。正式工厂包装 `GeneratedPolicyBuiltin` 则在构造时创建值为 `0` 的独立缓存，克隆时复制当前缓存值。

## 数据与状态

核心状态分为静态清单和实例缓存两类：

- 安全签名清单是只读的 `&'static [&'static str]`，运行期不会增删。`GENERATED_SIGNATURE_COUNT` 始终与切片长度一致；当前 510 项中有 509 项能在 Go 安全生成文件中找到同名方法，另有 Rust 特有安全分类 `builtinUncompressSig`。
- `baseBuiltinFunc` 在 `pkg/expression/builtin.rs` 中持有 `generatedSignature: Option<&'static str>`、`args: Vec<Box<dyn Expression>>` 和 `safeToShareAcrossSessionFlag: AtomicU32`。本文件只借用它们，不取得参数所有权。
- `GeneratedPolicyBuiltin` 持有自己的签名、底层函数和原子缓存，通过底层 `getArgs()` 提供参数切片。
- 缓存不记录中间态，也不会随某个参数的内部状态自动失效；正确性依赖“签名确定后参数树的跨会话安全属性保持不变”这一不变量。基类只在 `SetGeneratedSignature` 时显式清零缓存。

内存方面，检查过程不分配集合或复制表达式；首次检查为 O(n) 参数遍历，缓存命中为 O(1)。策略名称查询当前是对最多 510 个安全名称再对 unsafe 名称做线性查找，适用于构造/共享检查路径，但扩充清单时应关注查找成本。

## 依赖与调用关系

上游调用关系由 RustCodeGraph 和源码共同确认：

- `baseBuiltinFunc::SafeToShareAcrossSession` → `baseBuiltinFunc::GeneratedSafeToShareAcrossSession` → `generatedSignatureSafeToShareAcrossSession`。
- `GeneratedPolicyBuiltin::SafeToShareAcrossSession` → `generatedSignatureSafeToShareAcrossSession`，这是正式工厂生成签名的统一包装路径。
- `builtin.rs` 中另一套运行时策略也直接复用 `safeToShareAcrossSession`，说明该函数是可独立复用的递归缓存原语，而不仅服务于静态清单。
- 测试模块 `builtin_threadsafe_generated_26_aster_unit_test.rs` 在 `lib.rs` 的 `#[cfg(test)]` 下挂载，并直接导入本模块全部符号。

下游依赖为：

- `std::sync::atomic::{AtomicU32, Ordering}` 提供三态缓存与顺序一致内存序。
- `crate::expression_builtin::{Expression as _, baseBuiltinFunc}` 提供基类以及参数的共享安全方法解析。
- `crate::builtin_threadunsafe_generated_kernel::IsGeneratedThreadunsafeSignature` 提供互补的 `Never` 清单。
- 参数回调最终调用各表达式节点的 `SafeToShareAcrossSession`；该契约在 `pkg/expression/builtin.rs` 和正式表达式接口中定义。

本文件不直接依赖 `Cargo.toml` 中的第三方包，不执行 SQL 求值，也不访问会话、存储、网络或文件系统。它位于表达式复用/计划缓存安全边界，而非 `eval*` 数据计算主链。

## 错误处理与边界

本文件所有 API 返回布尔值或 `Option`，不产生 `Result`，也没有 panic 路径。关键边界采用保守策略：

- `signature == None`、未知签名及 unsafe 签名一律返回 `false`，并且不会探测参数，也不会写入递归缓存。
- 空参数在 `Recursive` 策略下判为安全并缓存 `1`。
- 子参数出现首个不安全结果时立即停止，未访问后续参数，并缓存 `2`。
- 任意非零缓存值只有精确等于 `1` 才代表安全；意外值不会被误认为安全。
- `SetGeneratedSignature` 在本文件之外负责把未知正式签名转成错误；直接调用通用函数时则只得到 `false`。

安全与 unsafe 清单必须互斥且覆盖所有正式工厂签名。相关测试会逐项调用 `formal_registry::ValidateGeneratedBuiltinSignature` 并验证两份集合不重叠；若新增签名遗漏两份清单，运行时会退化为安全的拒绝共享，而不是错误地放行。

## 并发与资源生命周期

`AtomicU32` 使多个线程可同时读取和发布缓存，全部操作使用最强的 `Ordering::SeqCst`。缓存一旦为 `1` 或 `2`，后续调用只做一次原子读取；文件不持有锁、线程、任务、通道或外部资源，也没有需要显式释放的对象。

未缓存时没有 compare-exchange 或“正在计算”状态，所以多个并发调用可能同时递归检查相同参数并分别写入结果。测试 `concurrent_callers_publish_a_stable_cached_result` 明确允许探针被调用多次，只要求所有调用返回安全且最终缓存为 `1`。这依赖参数判定是稳定、无破坏性的：如果同一表达式树的判定可能随并发调用改变，最后写入者会决定缓存值，现有算法不提供单次初始化保证。

缓存生命周期与持有它的内建函数实例一致。`baseBuiltinFunc::SetGeneratedSignature` 更换签名时清零；`GeneratedPolicyBuiltin::clone` 复制当时的缓存值到新的原子对象，克隆后两者不共享同一个原子地址。

## 与 Go 版本的对应关系

直接 Go 对照是 `pkg/expression/builtin_threadsafe_generated.go`，生成器是 `pkg/expression/generator/builtin_threadsafe.go`：

- Go 的 `safeToShareAcrossSession` 同样读取 `uint32`，以 `0/1/2` 表示未缓存/安全/不安全，顺序遍历 `args`、遇到不安全立即短路，并用原子写回结果。Rust 的 `safeToShareAcrossSession` 在控制流和三态语义上对应它。
- Go 生成器扫描 `builtin_*.go` 中以 `builtin` 开头、`Sig` 结尾的 struct；只有单字段且嵌入 `baseBuiltinFunc`/`baseBuiltinCastFunc` 的类型以及 `specialSafeFuncs` 才归入安全集合，其余进入 unsafe 集合。
- Go 输出为每个安全具体类型生成一个 `SafeToShareAcrossSession` 方法，当前对照文件包含 509 个这类方法、共 2585 行；Rust 不复制具体类型方法，而以 510 项签名字符串清单加动态策略入口表达分类，目标文件为 620 行。
- 509 个签名的安全分类及三态递归算法与 Go 对应。唯一直接清单差异是 `builtinUncompressSig`：`builtin_threadunsafe_generated.go` 对它返回 `false`，Rust 安全清单却包含它；`builtin.rs::allowedOptionalEvalPropsForSignature` 还为该签名单独允许（但不要求）`OptPropSessionVars`，`context_test.rs::uncompress_allows_session_vars_without_requiring_them` 验证这项接线。现有直接证据能确认差异存在及其可选属性规则，但不能仅凭这些文件证明改变分类的历史理由，因此该理由标记为“未验证”。
- Rust 还显式建模 `Recursive/Never/None`，并联合 Rust unsafe 清单验证未知输入；这是适配正式动态运行时的接线差异。不能把 Rust 测试中的 safe/unsafe 互斥结论外推为与 Go 两份生成清单逐项相同。

Go 生成文件是 Go 侧分类事实的来源，Rust 清单和正式注册表是 Rust 侧运行时事实的来源。Rust 独立测试固定安全签名数量为 510、验证首尾代表项和去重，并把 Rust 的安全、unsafe 清单逐项送入正式注册表验证。修改分类时应同时核对 Go 生成器规则、Go 生成结果、Rust 两份清单、`builtinUncompressSig` 的特殊可选属性接线及相关测试，不能只改某一侧以通过局部检查。

## 扩展指南

新增或调整生成签名时，最可能涉及以下位置：

1. 先确认 Go 具体签名的 struct 形态和 `pkg/expression/generator/builtin_threadsafe.go` 分类规则；若属于 `builtinUncompressSig` 一类的 Rust 特殊分类，应记录与 Go 的差异依据，并为共享行为和会话属性访问补充对应测试。
2. 将签名恰好加入 `GENERATED_THREADSAFE_SIGNATURES` 或 `builtin_threadunsafe_generated.rs` 的清单之一，保持两份集合互斥，并同步正式注册表接线。不要把未知签名默认按递归安全处理。
3. 若变更缓存算法，保持 `0/1/2` 协议、首次遍历短路、缓存命中不再探测和并发稳定发布；对应测试应放在独立文件 `pkg/expression/builtin_threadsafe_generated_26_aster_unit_test.rs`，不要内嵌到生产源文件。
4. 若允许签名或参数在实例存活期间改变，必须在所有变更入口清零缓存，或重新设计失效机制；否则旧结论会被永久复用。
5. 性能优化清单查询时，应保留静态、确定且可审计的生成结果，并重新验证 510 项覆盖、去重、safe/unsafe 不重叠及正式注册表覆盖。引入哈希结构会增加初始化和生成确定性方面的权衡。

兼容风险主要是错误分类导致会话状态泄漏或不必要地禁止计划共享；正确性风险集中在漏重置缓存和不稳定的子判定；性能风险集中在首次递归遍历、并发重复探测以及线性签名查找。此类修改需要同步 Go 对照语义和独立 Rust 测试，但本分析任务不修改运行时代码。

## 验证依据

- 源文件：`pkg/expression/builtin_threadsafe_generated.rs`，核对了 7 个 RustCodeGraph 符号、510 项清单、三态算法和 `baseBuiltinFunc` 适配器。
- crate/模块边界：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；确认库入口、私有 `#[path]` 模块挂载和独立测试挂载。
- Rust 调用入口：`pkg/expression/builtin.rs`；确认 `baseBuiltinFunc` 字段、`SetGeneratedSignature` 的校验/清零、`GeneratedPolicyBuiltin` 包装以及另一运行时策略对缓存原语的复用。
- 接口契约：`pkg/expression/builtin_core.rs`、`pkg/expression/expression.rs`；确认内建函数和表达式层的 `SafeToShareAcrossSession` 边界。
- Go 对照：`pkg/expression/builtin_threadsafe_generated.go`、`pkg/expression/builtin_threadunsafe_generated.go`、`pkg/expression/generator/builtin_threadsafe.go`；确认原子缓存算法、生成分类规则、509 个安全具体类型方法，以及 Go 把 `builtinUncompressSig` 归为 unsafe 的差异。
- 特殊签名接线：`pkg/expression/builtin.rs`、`pkg/expression/context_test.rs`；确认 Rust 为 `builtinUncompressSig` 允许但不要求 `OptPropSessionVars`，同时保留其分类理由“未验证”的边界。
- 独立 Rust 测试：`pkg/expression/builtin_threadsafe_generated_26_aster_unit_test.rs`；覆盖全安全、短路失败、缓存命中、基类路径、未知/unsafe 拒绝、并发发布、510 项去重及策略全集互斥。
- RustCodeGraph：`status` 显示索引包含目标文件且识别 7 个符号；`query/node/callers/callees` 确认 `generatedSignatureSafeToShareAcrossSession` 由 `builtin.rs` 的共享判定和本文件适配器调用，它下调 `safeToShareAcrossSession`，而 `IsGeneratedThreadsafeSignature` 由策略查询和覆盖测试调用。
- 本任务是纯文档分析，按计划未运行 Cargo 或代码测试；最终仅执行任务指定的 11 章节结构检查并人工复核上述事实链。
