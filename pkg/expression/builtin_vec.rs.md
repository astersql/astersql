# `pkg/expression/builtin_vec.rs`

## 文件定位

本文件属于 `astersql-expression` crate（`pkg/expression/Cargo.toml` 的 `[lib] path = "lib.rs"`），由 `pkg/expression/lib.rs` 以 `builtin_vec_kernel` 私有模块装入。它承载 TiDB `VECTOR FLOAT32` 的八个标量内置函数签名：`VEC_DIMS`、四种双向量度量、`VEC_L2_NORM`、`VEC_FROM_TEXT` 和 `VEC_AS_TEXT`。这里的“向量”是数据库的浮点向量值，不是按列执行机制；对应的按列方法定义在 `pkg/expression/builtin_vec_vec.rs`。

本文件已经实现函数类、签名对象和逐行求值，但普通 SQL 名称到这些专用函数类的 Rust 接线尚不完整：`pkg/expression/builtin.rs` 的 `BUILTIN_SPECS` 虽列出八个 `vec_*` 名称，`CORE_BUILTIN_FACTORIES` 却没有相应工厂，仓库也没有向量函数调用 `registerBuiltinFactory` 的证据。因此不能仅凭名称表断言普通 Rust SQL 构造链已经可用。另一方面，tipb 方向已有名称映射和签名配方（`pkg/expression/pb_to_expr_runtime.rs`、`pkg/expression/distsql_builtin.rs`），但配方表本身也不等同于本文件函数类已接入全局工厂。

## 核心职责

- `vectorFieldType` 将声明的 MySQL 类型码转成 `FieldType`：字符串返回值/参数使用 `utf8mb4`，其他类型使用 binary 字符集与排序规则。
- `buildVectorBase` 校验参数数量，按目标 `EvalType` 为不匹配的参数包一层 `BuildCastFunction`，构造递归拥有参数的 `RegistryBuiltinBase`，并保存 tipb `pb_code`。
- `vector_function_class!` 为每个 SQL 函数生成函数类，统一完成参数个数校验和具体签名对象构造。
- `vector_builtin_common!` 把具体签名接到 `builtinFunc`/`CollationInfo`，转发元数据、克隆、相等比较、内存统计、标量与按列求值，并声明 `vectorized() == true`。
- 八个签名实现实际逐行语义：维度、L1/L2 距离、负内积、余弦距离、L2 范数，以及文本和向量之间的转换。

## 主要符号

| 符号 | 可见性与语义 |
| --- | --- |
| `VectorBuiltinBase` | 文件私有别名，指向 `formal_registry::RegistryBuiltinBase`，保存参数、返回类型、collator 与 `pb_code`。 |
| `vectorFieldType(tp)` | 文件私有类型构造器；`TypeVarString` 选择 UTF-8，其余选择 binary。 |
| `buildVectorBase(ctx, args, argument_types, return_type, pb_code)` | 文件私有公共构建路径；参数数不符时报错，求值类型不符时插入 CAST。 |
| `vector_function_class!` | 生成八个公开函数类及其 `functionClass` 实现。生成类内部的 `baseFunctionClass` 负责 min/max 参数检查。 |
| `vector_builtin_common!`、`real_vector_builtin!` | 为签名补齐公共 trait 实现；实际 `vecEval*` 方法来自相邻的 `builtin_vec_vec.rs`。 |
| `builtinVecDimsSig` | 单参数向量签名；`evalInt` 返回元素数，NULL 原样传播。 |
| `builtinVecL1DistanceSig`、`builtinVecL2DistanceSig` | 双参数向量签名；分别调用 `VectorFloat32::L1Distance`、`L2Distance`。 |
| `builtinVecNegativeInnerProductSig`、`builtinVecCosineDistanceSig` | 双参数向量签名；分别计算负内积与余弦距离。 |
| `builtinVecL2NormSig` | 单参数向量签名；调用 `VectorFloat32::L2Norm`。 |
| `builtinVecFromTextSig` | 单字符串参数签名；解析文本并按返回类型 `flen` 校验维度。 |
| `builtinVecAsTextSig` | 单向量参数签名；调用 `VectorFloat32::String` 生成文本。 |
| `vectorDistance` | 四种双向量度量的私有标量模板；集中处理参数求值、NULL、底层错误和 NaN。 |

八个 `vector_function_class!` 展开实例分别声明参数/返回类型。距离函数返回 `TypeDouble`，维度返回 `TypeLonglong`，文本转换在 `TypeVarString` 与 `TypeTiDBVectorFloat32` 间转换。除 `VEC_FROM_TEXT` 使用 `pb_code = 0` 外，其余函数设置对应的 `tipb::ScalarFuncSig`；这与 Go 文件中注释掉 `VecFromTextSig` 下推码的现状一致。

## 执行流程

1. 理想的函数构造入口取得 SQL 名称对应的函数类，先由 `baseFunctionClass::verifyArgs` 检查实参数量；当前本文件的专用函数类可被直接构造和测试，但尚未接入 `builtin.rs` 的全局工厂链。
2. 生成的 `getFunction` 调用 `buildVectorBase`。每个实参的当前 `EvalType` 与声明类型比较，不同则插入 CAST；随后保存返回 `FieldType` 和下推码，创建相应 `builtin*Sig`。
3. 逐行执行通过 `builtinFunc::evalInt`、`evalReal`、`evalVectorFloat32` 或 `evalString` 分派到本文件的固有方法；按列执行通过同一 trait 分派到 `pkg/expression/builtin_vec_vec.rs` 的 `vecEval*`。
4. `VEC_DIMS` 求值一个向量并返回 `Len()`。任一输入为 NULL 时返回类型的零值和 `is_null = true`。
5. 四个双向量度量进入 `vectorDistance`：先后求值左右参数，任何一侧 NULL 都短路为 NULL；否则调用具体 `VectorFloat32` 算法。维度不一致等算法错误用 `?` 上抛；结果若为 NaN（典型例子是零向量余弦距离），转换成 SQL NULL。
6. `VEC_L2_NORM` 求值单个向量并计算范数，同样把 NULL 和 NaN 转成 SQL NULL。
7. `VEC_FROM_TEXT` 先求字符串，再调用 `ParseVectorFloat32`，最后用返回列的 `GetFlen()` 调用 `CheckDimsFitColumn`；`VEC_AS_TEXT` 则求向量并调用其稳定的字符串格式化方法。

## 数据与状态

每个签名只拥有一个 `VectorBuiltinBase`，其中 `args: Vec<ExprBox>` 递归拥有子表达式，`return_type` 描述 SQL 返回类型，`pb_code` 供下推序列化识别，collator 和字符集信息由基座统一保存。签名本身没有可变业务状态；`Clone` 会克隆基座，`equal` 比较同型签名的基座内容，`MemoryUsage` 委托给基座估算。

值层使用 `types::VectorFloat32`，SQL NULL 不编码在该值内，而通过返回元组的布尔位表示。因此 NULL 分支中的 `0`、`0.0`、空字符串或 `ZeroVectorFloat32()` 都只是占位值，调用者必须以 `is_null` 为准。返回向量的 `flen` 是 `VEC_FROM_TEXT` 的列维度约束；未指定/特殊 `flen` 的含义由 `VectorFloat32::CheckDimsFitColumn` 决定，本文件不自行解释。

## 依赖与调用关系

- 上游模块装配：`pkg/expression/lib.rs` 将本文件声明为 `builtin_vec_kernel`；`builtin_vec_vec.rs` 导入其中的签名并为其补充按列方法。
- 构建依赖：`BuildContext`、`baseFunctionClass`、`formal_registry::RegistryBuiltinBase`、`BuildCastFunction` 和 `functionClass` 来自 expression crate 的公共/正式注册基础设施。
- 求值依赖：子表达式的 `EvalVectorFloat32`/`EvalString`，以及 `types::VectorFloat32` 的 `Len`、四种距离、`L2Norm`、`String`、`ParseVectorFloat32`、`CheckDimsFitColumn`。
- 下推相关：函数类记录 `tipb::ScalarFuncSig`；`pkg/expression/pb_to_expr_runtime.rs` 将相应签名名映射回 AST 函数名，`pkg/expression/distsql_builtin.rs` 列出对应 Go 构造器配方。`VEC_FROM_TEXT` 没有非零下推码，而配方/名称映射的存在不改变这一点。
- 优化器旁路使用：`pkg/expression/vs_helper.rs` 识别四种距离函数名及其 protobuf 码，用于判断“一个向量列 + 一个向量常量”的向量索引表达式；它依赖函数名和表达式形态，并不直接调用本文件的 `vectorDistance`。
- 测试调用者：`pkg/expression/builtin_vec_vec_31_aster_unit_test.rs` 直接构造全部签名和函数类；`pkg/expression/builtin_vec_vec_test.rs` 固定八个函数清单并复用该回归集合；`pkg/expression/scalar_function_37_aster_unit_test.rs` 直接使用 `vecL2DistanceFunctionClass` 验证向量搜索元数据路径。

RustCodeGraph 的文件节点报告本文件被 11 个文件使用，但本次 `callers/callees` 子命令未在 60 秒内返回；上述边均由实际模块声明、符号引用和测试源码复核，而非依据超时查询推断。

## 错误处理与边界

- `buildVectorBase` 对声明数组与实参长度不一致返回 `unexpected length of vector builtin arguments`；正常入口还会先产生带函数名的参数个数错误。
- CAST 构建沿用 `BuildCastFunction` 的行为。本文件只比较 `EvalType`，同一求值类型内更细的 `FieldType` 差异不会触发 CAST。
- 子表达式求值、文本解析、维度校验和向量算法错误全部用 `?` 原样传播；本文件不把这些错误降级成 warning 或 NULL。
- SQL NULL 与 NaN 有意区分于错误：NULL 输入传播 NULL；距离/范数结果 NaN 转为 NULL；无穷值没有在本文件中额外拦截。
- 双向量函数只在左值非 NULL 后才求右值，保留短路行为。底层距离函数负责维度一致性，本文件不预先复制该校验。
- `VEC_FROM_TEXT` 的非法文本和不适配列维度均为错误；NULL 文本返回 NULL。`VEC_AS_TEXT` 的 NULL 向量返回 NULL，而不是可见空字符串。
- 所有 trait 分派都假设签名至少拥有声明数量的参数；应经函数类构造，绕过构造器并制造空 `args` 会在索引处失败，属于内部不变量违例。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部句柄。逐行求值仅在调用栈上借用 `EvalContext` 和 `Row`；`row.clone()` 只用于允许先后求值两个参数。向量与字符串结果按值返回，其所有权交给调用者。

`builtinFunc::SafeToShareAcrossSession` 委托给基座；仓库的 `builtin_threadsafe_generated.rs` 将八个签名列入线程安全清单，但本文件不自行声明额外同步机制。collator 的修改发生在函数对象构建/配置阶段；并发共享是否安全仍受基座契约约束。按列路径的临时 `Column` 在 `builtin_vec_vec.rs` 的函数调用内创建并在返回时释放，不由本文件持有。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/expression/builtin_vec.go` 和生成的 `pkg/expression/builtin_vec_vec.go`。Rust 保留了 Go 的八个函数类/签名、参数与返回求值类型、NULL 传播、NaN→NULL、维度错误、文本解析/格式化和 tipb 码；`VEC_FROM_TEXT` 在两端都没有启用下推码。Rust 用两个宏消除 Go 中重复的 `Clone`、trait 方法和函数类构造代码，并以 `vectorDistance` 合并四份标量距离流程。

实现层面的差异是：Go 的 `newBaseBuiltinFuncWithTp` 同时处理完整类型推导和缓冲区分配器，Rust 的 `buildVectorBase` 只按 `EvalType` 插 CAST，并使用 `RegistryBuiltinBase`；Go 的向量化代码从 `bufAllocator` 借还临时列，Rust 相邻文件当前直接创建局部 `Column`。两者的可观察 NULL/NaN/错误语义由 Rust 测试对齐，但资源复用与性能结构并非逐行等价。

Go `builtin.go` 把八个具体类直接放入全局 `funcs` map；Rust `builtin.rs` 当前只为名称创建通用 `registeredFunctionClass`，且核心工厂表没有对应 `vec_*` 工厂。这是接线状态差异，不应被文档描述为已完成迁移。Go 的表驱动测试 `pkg/expression/builtin_vec_vec_test.go` 覆盖八个函数、NULL 和余弦 NaN；Rust 的独立测试复现这些行为并额外覆盖明确数值、维度不匹配、非法文本、函数类参数数和 `pb_code`。

## 扩展指南

新增向量内置函数时，应同时完成以下最小闭环：

1. 在本文件增加签名结构，选择或新增标量公共模板，并用 `vector_function_class!` 明确参数类型、返回类型和下推码；如果不是简单距离，不要强套 `vectorDistance`。
2. 在 `pkg/expression/builtin_vec_vec.rs` 增加对应 `vecEval*`，确保 NULL、NaN、错误与标量路径一致；不要把 Rust 测试写回生产源文件。
3. 将名称接入 `pkg/expression/builtin.rs` 的规格表和真实 `BuiltinFactory`。当前八个函数自身也需要补齐这一工厂接线后，才能声称普通 Rust SQL 构造路径完整可用。
4. 若支持下推，同步 tipb 枚举、`pb_to_expr_runtime.rs` 映射和 `distsql_builtin.rs` 构造逻辑；不能只设置一个整数码。若不支持，应像 `VEC_FROM_TEXT` 一样显式保持 `0` 并记录边界。
5. 同步独立测试 `builtin_vec_vec_31_aster_unit_test.rs` 与入口清单 `builtin_vec_vec_test.rs`；Go 语义变化时还应对照 `builtin_vec.go`、`builtin_vec_vec.go`、`builtin_vec_vec_test.go`。
6. 兼容性风险集中在隐式 CAST、字符集/排序规则、NULL/NaN、向量维度错误和下推码一致性；性能风险集中在每行分配、重复解析文本、缺少临时列复用及距离计算热路径。

## 验证依据

- 源码全量阅读：`pkg/expression/builtin_vec.rs`（416 行）；相邻按列实现：`pkg/expression/builtin_vec_vec.rs`。
- crate/模块证据：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；该包没有 `doc.go`，因此无可读取的包级 Go 契约文件。
- 注册与调用证据：`pkg/expression/builtin.rs` 的 `FunctionClassRegistry`、`CORE_BUILTIN_FACTORIES`、`BUILTIN_SPECS`；`pkg/expression/pb_to_expr_runtime.rs`；`pkg/expression/distsql_builtin.rs`；`pkg/expression/vs_helper.rs`；`pkg/expression/builtin_threadsafe_generated.rs`。
- Go 对照：`pkg/expression/builtin_vec.go`、`pkg/expression/builtin_vec_vec.go`、`pkg/expression/builtin_vec_vec_test.go`。
- Rust 独立测试：`pkg/expression/builtin_vec_vec_31_aster_unit_test.rs`、`pkg/expression/builtin_vec_vec_test.rs`、`pkg/expression/scalar_function_37_aster_unit_test.rs`。测试覆盖明确数值、NULL、余弦 NaN、维度不匹配、非法文本、文本往返、参数数量与下推码；本任务按计划不运行 Cargo。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件且目标文件有 46 个符号；`files --filter` 命中目标；`node --file ... --offset 1 --limit 500` 读取完整文件并报告 11 个引用文件；`query` 精确命中 `vectorDistance`、`buildVectorBase`、`builtinVecDimsSig`。`callers vectorDistance` 连续等待 60 秒无输出后终止，调用边改由上述源码引用复核。
- 结构检查使用任务规定命令，要求目标存在且固定二级标题恰好为 11 个；人工复核重点是区分“求值实现存在”“tipb 配方存在”和“普通全局工厂已接线”三种不同事实。
