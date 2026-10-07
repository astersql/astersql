# `pkg/expression/builtin.rs`

## 文件定位

`builtin.rs` 是 `astersql-expression` crate 的内置标量函数公共框架与正式注册边界。crate 根文件 `pkg/expression/lib.rs` 以 `#[path = "builtin.rs"] mod expression_builtin` 装入它，并公开再导出 `formal_registry` 及其中的函数类、注册表、工厂和动态构造 API。它处在表达式构建与执行之间：上游按 SQL 函数名取得 `functionClass`、校验实参数量并构造 `builtinFunc`，下游由标量或向量化求值入口执行具体签名。

文件同时保留两层实现。文件前半的 `Error`、`types`、`chunk`、`Expression`、`baseBuiltinFunc` 等是与 Go `builtin.go` 对照的本地精简框架，供移植测试和分段实现复用；`formal_registry` 则使用 crate 根导入的正式 `Expression`、`BuildContext`、`types`、`chunk` 和 `builtinFunc`，是 `lib.rs` 对外再导出的生产接线。阅读或扩展时不能把前半的精简 `ScalarFunction` 当成 crate 对外的正式标量函数节点。

crate 边界由 `pkg/expression/Cargo.toml` 定义：包名为 `astersql-expression`，库入口是 `lib.rs`，关闭自动测试发现和 doctest；该 manifest 的 `[package.metadata.porting]` 明确把 Go 对照包标为 `pkg/expression`。本文件本身主要依赖标准库同步原语，正式注册实现再通过 crate 根类型以及 `types_dependency` 的 JSON 类型接入表达式子系统。

## 核心职责

1. 定义精简的字段类型、行列容器、求值上下文和表达式 trait，使基础构造、类型推导、逐行/批量求值及缓存语义能够独立验证（`types`、`chunk`、`EvalContext`、`Expression`）。
2. 为具体内置签名提供共享基类：保存参数、返回类型、 protobuf 签名码、collation、线程共享判定缓存和子表达式向量化判定（`baseBuiltinFunc`）。
3. 在构建期完成返回类型、字符集/排序规则、布尔标志、NULL 可达性和参数 CAST 推导（`newBaseBuiltinFunc*`、`newReturnFieldTypeForBaseBuiltinFunc`、`adjustNullFlagForReturnType`）。
4. 维护函数名、展示名和参数个数契约；正式层用完整 `BUILTIN_SPECS` 建立可并发读写的 `FunctionClassRegistry`，并保留独立扩展函数表 `extensionFuncs`。
5. 将函数类映射到具体工厂。`invoke_factory` 优先把 `grouping`/全文检索交给 planner bridge，其余从内建或运行时注册的工厂中构造签名，并用 `GeneratedPolicyBuiltin` 套用生成的线程安全策略。
6. 提供正式动态表达式构造器：`BuildCastFunction`、`BuildGetVarFunction`、`BuildToBinaryFunction`、`BuildFromBinaryFunction`，负责类型标志、collation、常量折叠及会话变量读取边界。
7. 提供基础过滤与按上下文缓存工具（`EvalBool`、`VecEvalBool`、`builtinFuncCache<T>`），并承载真值、`VALUES()`、比较/算术/JSON 等核心签名的正式实现和工厂选择。

## 主要符号

- `EvalContext` / `SimpleEvalContext`：精简层只暴露稳定的 `CtxID()`；缓存以该 ID 区分语句/求值上下文。
- `Expression`：对象安全、可克隆且要求 `Send + Sync`。类型化 `Eval*` 默认返回“未实现”错误；`VecEval*` 默认逐行循环、遇错立即返回，并按 NULL 标志向结果列追加值或 NULL。
- `ScalarFunction` 与内部 `CastExpression`：前者保存精简函数名、参数和返回类型，后者只对整数、实数、字符串做实际互转，其他类型委托或报告不支持。它们属于前半的对照框架。
- `baseBuiltinFunc`：精简签名基类。`SetGeneratedSignature` 验证生成签名，`GeneratedSafeToShareAcrossSession`（由其调用链实现）使用原子状态缓存共享判定；`isChildrenVectorized` 用 `OnceLock<bool>` 缓存子节点判定；直接调用基类 `Eval*` 会产生“should never be called”错误。
- `newBaseBuiltinFunc`、`newBaseBuiltinFuncWithTp`、`newBaseBuiltinFuncWithFieldTypes`：依次覆盖已有返回字段类型、按 `EvalType` CAST、按显式 `FieldType` CAST 三种构造方式；缺少构建上下文或参数/类型数量不等时返回错误。
- `newReturnFieldTypeForBaseBuiltinFunc`：把 `EvalType` 映射到 MySQL 类型码、长度、小数位、binary charset/collation，并给比较、逻辑、`IN`、`LIKE` 等函数加布尔标志。
- `baseFunctionClass`：保存函数名和参数下界/上界；前半精简版本使用闭区间，正式版本以负的 `max_args` 表示无上限。
- `builtinFuncCache<T>`：单槽缓存 `(CtxID, value)`。读路径用 `RwLock`，初始化路径用 `Mutex` 做双重检查，只在构造成功后写入。
- `formal_registry::GeneratedBuiltinFactoryOutput` / `BuiltinFactory`：工厂结果必须携带已知生成签名；工厂签名接收正式 `BuildContext`、参数和 `FunctionClassMetadata`。
- `formal_registry::RegistryBuiltinBase`：正式签名共享状态，包含参数、返回类型、PB code、collator/collation 信息和 `RuntimeThreadSafetyPolicy`。`Recursive` 递归检查参数，`Never` 明确禁止跨会话共享。
- `formal_registry::functionClass`：正式函数类接口，负责 `getFunction`、按数量校验、展示名和可选元数据。`registeredFunctionClass`、`valuesFunctionClass`、`isTrueOrFalseFunctionClass` 分别覆盖普通注册函数、`VALUES()` 和真值判断。
- `formal_registry::FunctionClassRegistry`、`funcs`、`extensionFuncs`：以 `RwLock<HashMap<_, Arc<dyn functionClass>>>` 提供 `LoadOrStore`、`Store`、`Delete`、排序名称列表等并发注册能力；`funcs` 从静态规格初始化，`extensionFuncs` 初始为空。
- `formal_registry::registerBuiltinFactory` / `removeBuiltinFactory`：管理运行时工厂覆盖表；重复注册返回错误，删除在锁中毒时静默不操作。
- `formal_registry::BuildCastFunction` 等动态构造器：创建正式 `ScalarFunction`，设置 PB code、线程安全策略与字符集信息，并在允许时调用 `FoldConstant`。

## 执行流程

普通内置函数的构建主链如下：

1. 上游通过 `formal_registry::funcs.get(name)` 或 `Load(name)` 获取 `Arc<dyn functionClass>`；`lib.rs` 把该表和 trait 直接再导出给 crate 其余模块。
2. `functionClass::getFunction` 先调用 `baseFunctionClass::verifyArgs`。固定参数函数检查精确范围，可变参数函数使用负上界表示只校验最小值。
3. `registeredFunctionClass` 调用 `invoke_factory`。内置静态工厂在 `FunctionClassRegistry::from_specs` 初始化时加入工厂表；运行时工厂可由 `registerBuiltinFactory` 补充。
4. `invoke_factory` 对 planner bridge 专属函数走 `planner_bridge_kernel::build_builtin`；其余名称若存在函数规格但没有连接完整工厂，会返回“registered but ... not linked”，不会制造一个成功但无行为的桩。
5. 工厂根据参数求值类型、函数名和 `FunctionClassMetadata` 选择具体 `CoreBuiltinKind`/专用签名，必要时用 `BuildCastFunction` 调整参数类型，设置返回 `FieldType` 和 PB code，再产出 `GeneratedBuiltinFactoryOutput`。
6. `GeneratedPolicyBuiltin` 包裹具体 `builtinFunc`：所有标量/向量化求值、collation、相等比较和内存统计继续委托给内部函数，但跨会话共享判定按生成签名的策略执行。
7. 上层正式 `ScalarFunction` 持有返回类型和 `builtinFunc`；执行时由对应 `evalInt`/`evalString`/`vecEval*` 等入口得到 `(值, is_null)` 或错误。

动态 CAST 的流程略有不同：`BuildCastFunction` 复制目标类型，继承源表达式的 NOT NULL 约束（源可空时删除目标 NOT NULL），修正 BIT 到字符串的长度、字符串到 JSON 的 `ParseToJSONFlag`，继承 coercibility/repertoire，设置连接或源字符集，再选择 PB code。JSON CAST 原样返回，其余 CAST 经过常量折叠。`BuildGetVarFunction` 对只读且常量命名的用户变量可在构建期求值，否则保留动态节点；to/from-binary 构造器仅处理字符串，非字符串直接克隆原表达式。

精简过滤流程中，`EvalBool` 逐个求值过滤器，错误立即返回；NULL 或零会短路为 false。`VecEvalBool` 先按输入行数初始化 selection/null 位图，再逐行逐过滤器更新；它当前忽略 `_vec_enabled`，实际仍调用逐行 `EvalInt`。

## 数据与状态

- `types::FieldType` 保存类型码、flag、显示长度、小数位、字符集、collation 和枚举元素；`EvalType()` 将 MySQL 类型码归并到 Int/Real/Decimal/String/Datetime/Timestamp/Duration/Json/VectorFloat32。
- `chunk::Value` 是精简列容器的值联合，`Column` 保存值向量，`Chunk` 保存列、行数和可选 selection；`Iterator4Chunk` 迭代时尊重 selection 下标。
- NULL 不作为普通值隐式编码：精简 `Eval*` 返回三元组中的独立布尔位，正式 `builtinFunc` 返回 `Result<(T, bool), Error>`；列式结果则显式追加 `Value::Null`。
- `baseBuiltinFunc` 的 `generatedSignature`、`pbCode`、返回类型和 collation 是签名级状态；clone 会复制逻辑状态，但重新创建 `OnceLock`，避免把子节点向量化缓存错误地共享到已克隆参数树。
- `builtinFuncCache<T>` 只有一个上下文槽。切换到新 `CtxID` 会覆盖旧值；它不是保留所有会话结果的 map。构造错误不会写入缓存。
- 正式 `FunctionClassRegistry` 的条目用 `Arc` 共享，表本身用 `RwLock` 保护；`names()` 返回排序副本，因此调用者不能借此修改注册表。
- `CURRENT_INSERT_VALUES_READER` 是 `LazyLock<RwLock<Option<fn>>>`，为 `VALUES()` 提供可替换读取钩子；未注册时求值会报告边界错误，而不是猜测当前插入行。
- `RegistryBuiltinBase` 和 `GeneratedPolicyBuiltin` 都用 `AtomicU32` 缓存跨会话共享判定。正式层还保存真实 collator 对象及 coercibility/repertoire 等 collation 状态。

## 依赖与调用关系

- 装配入口：`pkg/expression/lib.rs` 装入 `builtin.rs`，再导出 `formal_registry::{funcs, extensionFuncs, functionClass, registerBuiltinFactory, BuildCastFunction, ...}`；这是正式应用链的直接证据。
- 同包下游：各 `builtin_*.rs`、`core_support.rs`、`distsql_builtin.rs` 和表达式测试使用这些基类或正式注册 API。RustCodeGraph 将 `pkg/expression/builtin.rs` 标记为被 54 个文件使用，并能定位 `newBaseBuiltinFunc`、`builtinFuncCache`、`EvalBool`、`registerBuiltinFactory`、`FunctionClassRegistry`、`BuildCastFunction` 等定义。
- 工厂下游：生成线程安全策略来自 `builtin_threadsafe_generated_kernel`；特殊 planner 内置函数通过 `planner_bridge_kernel`；JSON 路径解析使用 `types_dependency::json_path` 与 `json_binary`。
- 类型/上下文下游：正式注册层使用 crate 的 `BuildContext`、`EvalContext`、`Expression`、`builtinFunc`、collator 和可选求值属性集合。动态变量函数读取 `EvalContext::GetUserVarsReader()`。
- Go 对照：`pkg/expression/builtin.go` 给出 `baseBuiltinFunc`、构造器、`functionClass`、`funcs`、展示名和缓存契约；`builtin.rs` 的 `formal_registry::BUILTIN_SPECS` 注释声明其参数规格逐项来自 Go `funcs` 表。
- Cargo 边界：`pkg/expression/Cargo.toml` 声明 expression crate 的内部 path 依赖及 parser/types/session variable 等依赖；本文件没有独立 feature gate，测试由 `lib.rs` 的显式模块和文件末尾 `#[cfg(test)]` 模块装入。

RustCodeGraph 对重名符号的调用边分辨有限：例如 `BuildCastFunction` 同时存在于 `builtin.rs` 和 `builtin_cast.rs`，其 `callers` 结果会合并候选。因此具体上游关系以 `lib.rs` 再导出和同包精确文本引用复核，不把空的 Rust callers 列表解释为“无人调用”。

## 错误处理与边界

- 精简层使用 `Error(String)`；正式层通过 crate 的 `errors::New`/`Error` 传播。构建上下文缺失、参数数量不一致、CAST 文本解析失败和未知类型都会返回显式错误。
- `Expression` 与 `baseBuiltinFunc` 的默认类型化求值是保护性失败路径。具体签名必须覆盖相应 `Eval*`/`eval*`；基类成功返回默认值不被允许。
- 函数规格存在但具体工厂未接线时，`invoke_factory` 返回错误。这一边界明确区分“函数名受支持”和“完整实现已链接”。
- `ValidateGeneratedBuiltinSignature` 与 `SetGeneratedSignature` 拒绝生成表中不存在的签名，避免未定义的线程安全策略进入运行时。
- 参数校验使用函数自己的 min/max 契约；正式 `VerifyArgsWrapper` 在名称未找到时按 Go 约定返回 `Ok(())`，因为其调用者应先保证函数受支持。这个行为与前半精简版本对未知函数报错不同，生产调用应使用正式再导出版本。
- `BuildGetVarFunction` 只有在变量名是非 deferred 常量且上下文声明其只读时才折叠；其他情况保留运行时读取，避免冻结可变会话状态。
- 锁中毒处理并不完全一致：关键工厂注册和 `LoadOrStore` 使用错误或 `expect` 暴露问题；部分查询/删除返回空值或跳过。扩展代码应延续所在 API 的既有策略，不能假设所有锁失败都会变成 `Result`。
- 本文件前半的 `CastExpression` 只覆盖少数精简类型转换；完整 SQL CAST 语义位于正式 `BuildCastFunction` 及相关 builtin 文件。不能用精简包装器证明所有 MySQL CAST 行为。

## 并发与资源生命周期

所有公开表达式对象要求 `Send + Sync`。`builtinFuncCache` 的快速路径持读锁，未命中后持初始化互斥锁并再次检查，保证相同上下文的并发初始化只执行一次；构造闭包完成后才取得写锁写入。`pkg/expression/builtin_test.rs::test_builtin_func_cache_concurrency` 和 `builtin_32_aster_unit_test.rs::cache_is_once_per_context_and_does_not_cache_errors` 都用 8 个线程验证该不变量，并验证失败不缓存。

正式注册表和工厂表以 `LazyLock` 延迟初始化、`RwLock` 保护读写。函数类通过 `Arc` 跨调用者共享；注册、删除或扩展函数表变更的生命周期是进程级，而不是单次查询级。`LoadOrStore` 在一个写锁临界区内完成检查和插入，返回值的布尔语义与 Go `sync.Map.LoadOrStore` 相同。

线程安全不是由“实现了 `Send + Sync`”单独决定。`RegistryBuiltinBase` 的 `RuntimeThreadSafetyPolicy::Recursive` 会递归检查参数，`Never` 永远拒绝跨会话共享；生成签名再由 `GeneratedPolicyBuiltin` 调用生成策略核验并缓存结果。用户变量和 `VALUES()` 这类依赖上下文的节点必须选用禁止共享或显式上下文读取的路径。

表达式 clone 会复制参数树和类型状态；原子缓存值按当前值重建，`OnceLock` 则重置。列式求值由调用者拥有输入 `Chunk` 和可变输出 `Column`，函数先清空输出再写入，错误时可能已经写入部分行，因此调用者只能在 `Ok(())` 后消费完整结果。

## 与 Go 版本的对应关系

- Rust `baseBuiltinFunc` 对应 Go `builtin.go` 的同名结构：参数、返回类型、PB code、collator、子节点向量化、clone、相等比较、内存估算和基础 `eval*` 保护路径均保留相同职责。
- `newBaseBuiltinFunc*` 对应 Go 第 127、186、247、305 行附近的构造器；共同顺序是推导 collation、为参数构造 CAST、建立返回类型、再调整 NULL 标志。Rust 精简实现只实现了足够验证框架的转换子集，正式实现通过 crate 类型补足生产接线。
- `adjustNullFlagForReturnType` 对应 Go 的三个函数集合：恒非空、恒可空、参数全非空时非空。Rust 当前集合规模小于 Go 完整集合，因此它是迁移框架证据，不应被描述为完整 Go 集合的逐项等价。
- 正式 `functionClass`、`baseFunctionClass`、`funcs` 和 `extensionFuncs` 对应 Go 的函数类接口、静态 `funcs` map 与扩展函数边界。Rust 用 `RwLock<HashMap>`/`Arc` 模拟 Go map 或并发映射的共享语义。
- `GetDisplayName` 保留运算符到 SQL 展示文本的映射；`pkg/expression/builtin_test.rs::test_display_name` 与 Go `TestDisplayName` 对照验证 `eq`、`nulleq`、`istrue` 和普通/未知名称。
- `builtinFuncCache` 对应 Go 文件末尾的泛型缓存：按 statement context ID 命中、上下文变化后重建、并发仅构造一次、错误不缓存。Rust 测试实际执行这些断言，不只是保留 Go 注释。
- Go `builtin_test.go` 的 `TestIsNullFunc`、`TestLock` 在 Rust 对照测试中转调注册表 parity suite；其余大量 Go 注释用于保留迁移意图。真正的基础框架回归还在独立的 `builtin_32_aster_unit_test.rs`，符合测试与源文件分离要求。

## 扩展指南

新增普通内置函数时，应先确定它属于正式工厂链而不是前半精简框架：

1. 在 `formal_registry::BUILTIN_SPECS` 添加规范化名称与参数范围，并确保名称/别名和 Go `pkg/expression/builtin.go` 对齐。
2. 在适当的 `builtin_*.rs` 中实现独立 `builtinFunc`，不要把领域实现继续堆入公共基类；为签名设置准确返回 `FieldType`、PB code、collation 和 optional eval props。
3. 将名称接到静态工厂或用 `registerBuiltinFactory` 注册；工厂必须返回通过 `ValidateGeneratedBuiltinSignature` 的签名。若新增生成签名，同步更新线程安全生成表及其独立测试。
4. 明确线程策略：纯函数通常递归依赖子参数；读取会话变量、当前插入行或有副作用的函数应选 `Never` 或专用策略。不要仅为了通过 `Send + Sync` 而声明可跨会话共享。
5. 若参数需要隐式转换，复用正式 `BuildCastFunction`，并检查 NULL flag、BIT 长度、JSON parse flag、coercibility/repertoire 和常量折叠是否符合 Go。
6. 在独立测试文件中扩展：基础框架规则放 `pkg/expression/builtin_32_aster_unit_test.rs`，Go 顶层回归对应关系放 `pkg/expression/builtin_test.rs`，具体函数行为放相应 `builtin_*_test.rs`；不要把测试内嵌进 `builtin.rs`。
7. 至少覆盖正常值、NULL、错误、参数边界、标量/向量化一致性和跨上下文状态；涉及注册表时还要覆盖重复注册、删除恢复和并发访问。涉及 Go 对齐时保持原有分支与测试意图，不用缩减版实现代替完整行为。

修改已有函数时要特别检查两套同名 API。crate 外部通常通过 `lib.rs` 获得 `formal_registry` 的正式符号；仅修前半精简 `GetDisplayName`、`VerifyArgsWrapper` 或 `BuildCastFunction` 的同名概念，不一定会改变生产路径。

## 验证依据

- RustCodeGraph：`status` 显示索引含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/expression/builtin.rs` 确认目标文件已索引、含 1,269 个符号并被 54 个文件使用。
- RustCodeGraph：对 `newBaseBuiltinFunc`、`adjustNullFlagForReturnType`、`builtinFuncCache`、`EvalBool`、`registerBuiltinFactory`、`FunctionClassRegistry`、`BuildCastFunction`、`IsFunctionSupported` 的 `query` 定位了 Rust/Go 对照定义；`node --file` 分段核对了表达式 trait、构造器、缓存、正式注册表和动态构造器源码。
- 装配与 crate：读取 `pkg/expression/lib.rs` 中 `expression_builtin` 模块及 `formal_registry` 再导出，读取 `pkg/expression/Cargo.toml` 的库入口、依赖和 porting metadata。目标包没有 `pkg/expression/doc.go`。
- Go 对照：读取 `pkg/expression/builtin.go`，核对 `baseBuiltinFunc`、构造器、函数类、静态函数表、展示名和缓存；读取 `pkg/expression/builtin_test.go` 的显示名、锁函数和缓存测试意图。
- Rust 测试：读取 `pkg/expression/builtin_test.rs` 的 `test_display_name`、`test_builtin_func_cache_concurrency`、`test_builtin_func_cache`，以及 `pkg/expression/builtin_32_aster_unit_test.rs` 对注册表、构造/Cast、NULL 规则、缓存、向量化过滤的独立回归。另由引用搜索确认生成签名校验覆盖在 `builtin_threadsafe_generated_26_aster_unit_test.rs`，正式注册 parity suite 位于 `builtin_registry_aster_unit_test.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo，也不以编译结果替代结构与源码证据。交付结构检查要求本文恰有“文件定位”至“验证依据”十一节；文档内容由上述源码、图查询、Cargo、Go 对照和测试路径人工复核。
