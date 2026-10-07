# `pkg/expression/builtin_threadunsafe_generated.rs`

## 文件定位

该文件属于 `astersql-expression` crate；crate 根由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 指向 [`lib.rs`](lib.rs)，后者用 `#[path = "builtin_threadunsafe_generated.rs"] mod builtin_threadunsafe_generated_kernel;` 将本文件作为私有子模块接入。模块没有被 crate 根公开再导出，因此这里的 `pub` 常量和函数主要是 crate 内部接口，而不是稳定的外部 API。

文件头标明它是 `expression/generator` 生成体系对应的产物，职责不是执行某个 SQL 内建函数，而是给 Rust 的统一动态 builtin 运行时提供“不可跨会话共享”的签名分类表。直接消费方是 [`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs) 中的 `GeneratedThreadSafetyPolicyForSignature`；测试接线位于 [`lib.rs`](lib.rs) 的 `#[cfg(test)]` 模块声明。

## 核心职责

本文件维护 88 个会话敏感或带可变上下文的 Go concrete signature 名称，并提供精确名称查询。命中清单的签名会在 [`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs) 中被映射为 `GeneratedThreadSafetyPolicy::Never`，继而由 `generatedSignatureSafeToShareAcrossSession` 恒定判为不可跨会话共享，避免实例级 Plan Cache 等复用路径把会话相关状态带到另一会话。

它只负责分类，不实现签名对应的 SQL 求值，也不保存会话对象。清单覆盖当前 Rust 正式运行时所采用的 88 个 unsafe 名称；它与当前同路径 Go 生成文件并非逐项完全一致，差异见“与 Go 版本的对应关系”。

## 主要符号

- `GENERATED_THREADUNSAFE_SIGNATURES: &[&str]`：静态字符串切片，按生成顺序保存 88 个不可共享签名。从 `builtinArithmeticMultiplyRealSig` 到 `builtinTiDBCurrentTsoSig`，涵盖依赖会话身份、序列、随机数、锁、用户变量、时间、正则、编码参数等状态的不同 builtin；本文件不再按原因细分各项。
- `GENERATED_SIGNATURE_COUNT: usize`：通过 `GENERATED_THREADUNSAFE_SIGNATURES.len()` 派生的数量常量。它避免另存一个可能漂移的手写数字；当前独立测试断言值为 88。
- `IsGeneratedThreadunsafeSignature(name: &str) -> bool`：唯一函数入口，使用切片的 `contains` 做区分大小写、全字符串相等查询。命中返回 `true`，未知名称、大小写不同或仅部分匹配均返回 `false`。

文件没有类型、trait、`impl`、宏定义、条件编译项或私有辅助函数；三项符号均声明为 `pub`，但受其私有父模块限制，实际可见边界仍是 crate 内部。

## 执行流程

1. builtin 构造路径把生成签名名保存在统一运行时对象中；具体共享判定入口由 [`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs) 的 `baseBuiltinFunc::GeneratedSafeToShareAcrossSession` 提供。
2. `generatedSignatureSafeToShareAcrossSession` 将可选签名交给 `GeneratedThreadSafetyPolicyForSignature`。该函数先查线程安全清单；未命中时调用本文件的 `IsGeneratedThreadunsafeSignature`。
3. 查询函数在线性静态切片上比较 `&str`。命中时上游返回 `Some(GeneratedThreadSafetyPolicy::Never)`，不递归检查参数，也不读写线程安全文件中的原子缓存。
4. `generatedSignatureSafeToShareAcrossSession` 对 `Never` 返回 `false`。未知签名同样走保守的 `false` 分支，但语义上仍与“已明确列入 unsafe 清单”不同：前者的策略查询结果是 `None`。

该文件自身没有初始化流程；静态切片由程序映像直接提供，查询时也不会分配内存。

## 数据与状态

唯一业务数据是编译期字符串切片。切片元素是 `'static` 字符串字面量，运行期间不可变；`GENERATED_SIGNATURE_COUNT` 始终从同一切片长度计算。名称是跨 Rust 统一运行时与 Go concrete signature 模型之间的协议键，必须保持精确拼写。

本文件不持有行数据、求值上下文、会话、事务、缓存或统计信息。清单顺序不影响 `contains` 的布尔结果，但 [`builtin_threadunsafe_generated_27_aster_unit_test.rs`](builtin_threadunsafe_generated_27_aster_unit_test.rs) 会校验完整顺序，因此生成顺序也是当前受测输出契约。该测试还用 `BTreeSet` 验证清单无重复项。

## 依赖与调用关系

上游直接调用边为：

- [`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs) 的 `GeneratedThreadSafetyPolicyForSignature` → `IsGeneratedThreadunsafeSignature` → `GENERATED_THREADUNSAFE_SIGNATURES.contains`。
- [`builtin_threadunsafe_generated_27_aster_unit_test.rs`](builtin_threadunsafe_generated_27_aster_unit_test.rs) 直接读取两项常量并调用查询函数，验证完整清单、分类结果、数量和唯一性。
- [`builtin_threadsafe_generated_26_aster_unit_test.rs`](builtin_threadsafe_generated_26_aster_unit_test.rs) 读取 unsafe 清单，与 safe 清单联合验证每个正式工厂签名都有策略、能通过 `formal_registry` 校验，并确认两张表不重叠。

下游仅使用 Rust 标准库切片的 `contains`，没有外部 crate 依赖。所属 crate 的边界由 [`Cargo.toml`](Cargo.toml) 定义；本文件不直接使用其中任何依赖或 feature。RustCodeGraph 对目标文件给出的直接使用者也仅为线程安全分发文件和本文件的独立单元测试。

## 错误处理与边界

查询 API 不返回 `Result`，不会产生业务错误：未知名称直接返回 `false`。上游策略函数对未知名称返回 `None`，最终共享判定同样保守返回 `false`，所以漏分类会降低共享机会或暴露策略漂移，但不会由本函数抛错。

匹配严格区分大小写且不做规范化。例如空字符串、`builtinUnknownSig`、带额外空白的名称以及合法名称的不同大小写都不会命中。线性查找当前最多比较 88 项；若清单显著增大或该路径成为热点，应先用基准证据评估，再考虑保持生成确定性的更快数据结构。

最重要的维护边界是两张生成表必须互斥且覆盖正式工厂接受的签名。只改本清单而不同步 safe 清单、正式注册表和独立测试，可能造成同一名称双重分类或落入未知策略。当前测试覆盖已列出的名称、重复项和 safe/unsafe 不重叠，但本文件自己的测试没有直接断言未知名称、大小写变化或 Go/Rust 全量集合相等。

## 并发与资源生命周期

静态字符串切片是只读共享数据，查询函数是纯读操作，没有锁、原子变量、线程局部状态、通道、任务或 `unsafe` 代码，可被多个线程并发调用。它也没有需要显式释放的资源。

与本文件相邻的线程安全实现使用 `AtomicU32` 缓存递归判定结果；unsafe 命中分支不会触碰该缓存，而是立即返回 `false`。因此本文件不承担原子状态生命周期，只提供决定是否进入 `Never` 分支的不可变分类依据。

## 与 Go 版本的对应关系

Go 的权威生成逻辑位于 [`generator/builtin_threadsafe.go`](generator/builtin_threadsafe.go)：它扫描非测试 `builtin_*.go`，收集名称为 `builtin*Sig` 的结构体；除 `specialSafeFuncs` 外，只有恰好包含一个 `baseBuiltinFunc` 或 `baseBuiltinCastFunc` 字段的结构体归入 safe，其余生成一个恒返回 `false` 的 `SafeToShareAcrossSession` 方法。Go 产物 [`builtin_threadunsafe_generated.go`](builtin_threadunsafe_generated.go) 因而为每个 unsafe concrete signature 生成独立方法。

Rust 没有为全部 Go concrete signature 重复生成类型方法，而是以本文件的名称清单加统一策略分发实现等价的 `Never` 分类。Rust 的生成器移植位于 [`generator/builtin_threadsafe.rs`](generator/builtin_threadsafe.rs)，其 `collect_thread_safe_builtin_funcs`、`gen_builtin_thread_safe_code` 和 `UNSAFE_FUNC_TEMPLATE` 保留了 Go 的分类与输出语义；不过该生成器当前输出的是 Go 源码，并不是自动重写本 Rust 清单的入口。

以当前工作树直接比较，同路径 Go unsafe 产物有 90 个唯一签名，Rust 本清单有 88 个；Go 独有 `builtinUncompressSig` 和 `builtinEmbedTextSig`。Rust 当前把 `builtinUncompressSig` 放在 [`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs) 的 safe 清单中，而 `builtinEmbedTextSig` 在 [`builtin_inference.rs`](builtin_inference.rs) 中走独立 concrete 实现。因此“Rust 清单与当前 Go unsafe 产物完全相同”尚未得到支持；扩展或同步时必须先判断这是有意的运行时建模差异还是待修复漂移，不能仅凭本文件头注释推断。

## 扩展指南

新增或修改 builtin signature 时，应先根据 Go 结构字段和实际会话依赖确定策略，再同步检查 [`generator/builtin_threadsafe.go`](generator/builtin_threadsafe.go)、Rust 生成器移植、safe/unsafe 两张 Rust 清单以及正式注册表。会话身份、用户变量、随机状态、锁、序列值、可变上下文或非线程安全内部字段通常要求 `Never`；只含不可变基类并递归依赖参数的签名才适合 safe 策略。

若需要增加 unsafe 名称，最直接接入点是 `GENERATED_THREADUNSAFE_SIGNATURES`，同时必须从 safe 清单排除同名项，并更新独立测试 [`builtin_threadunsafe_generated_27_aster_unit_test.rs`](builtin_threadunsafe_generated_27_aster_unit_test.rs) 的期望顺序和数量。还应保留 [`builtin_threadsafe_generated_26_aster_unit_test.rs`](builtin_threadsafe_generated_26_aster_unit_test.rs) 的正式注册、覆盖和互斥检查；生成器行为变更则同步更新 [`generator/builtin_threadsafe_test.rs`](generator/builtin_threadsafe_test.rs) 或其独立 Aster 测试文件，而不要把测试嵌入生产源文件。

兼容风险主要是错误地把会话敏感函数标为 safe，可能导致跨会话状态泄漏；反向误标为 unsafe 通常只损失共享和 Plan Cache 性能。性能修改需注意当前查询是 O(n) 且无分配；替换结构不能破坏确定性生成、精确名称协议或无锁只读特性。该文件标注为生成物，长期修订应优先修正生成源并重新生成，而不是只手工改产物。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标仓库；`node --file pkg/expression/builtin_threadunsafe_generated.rs` 读取了 124 行全貌，并报告直接使用者为 `builtin_threadsafe_generated.rs` 与 `builtin_threadunsafe_generated_27_aster_unit_test.rs`；`query IsGeneratedThreadunsafeSignature` 定位唯一函数定义；`explore` 确认策略分发和测试调用边。
- Rust 源与接线：[`builtin_threadunsafe_generated.rs`](builtin_threadunsafe_generated.rs)、[`builtin_threadsafe_generated.rs`](builtin_threadsafe_generated.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。目标包不存在 `pkg/expression/doc.go`，因此没有额外包级 Go 契约可读。
- Rust 测试：[`builtin_threadunsafe_generated_27_aster_unit_test.rs`](builtin_threadunsafe_generated_27_aster_unit_test.rs)、[`builtin_threadsafe_generated_26_aster_unit_test.rs`](builtin_threadsafe_generated_26_aster_unit_test.rs)、[`generator/builtin_threadsafe_test.rs`](generator/builtin_threadsafe_test.rs)、[`generator/builtin_threadsafe_1_aster_unit_test.rs`](generator/builtin_threadsafe_1_aster_unit_test.rs)。
- Go 对照：[`builtin_threadunsafe_generated.go`](builtin_threadunsafe_generated.go)、[`generator/builtin_threadsafe.go`](generator/builtin_threadsafe.go)、[`builtin.go`](builtin.go) 与 [`expression.go`](expression.go)。通过精确提取并排序 `builtin*Sig` 名称核对到 Go 90 项、Rust 88 项，以及两个 Go-only 名称。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前另运行固定 11 章结构命令，并人工复核文档只描述已由上述代码、调用边与测试支持的现状。
