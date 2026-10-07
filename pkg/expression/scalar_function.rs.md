# `pkg/expression/scalar_function.rs`

源文件：[`scalar_function.rs`](./scalar_function.rs)  
Go 对照：[`scalar_function.go`](./scalar_function.go)  
独立测试：[`scalar_function_test.rs`](./scalar_function_test.rs)、[`scalar_function_37_aster_unit_test.rs`](./scalar_function_37_aster_unit_test.rs)

## 文件定位

本文件实现 Rust 表达式树中的标量函数节点 `ScalarFunction`，位于 `astersql-expression` crate。`pkg/expression/lib.rs` 以 `expression_scalar_function` 模块装载它并公开再导出，因此 planner、ranger、DDL、PB 反序列化等上层代码都通过 `astersql_expression::NewFunction*`、`ScalarFunction` 或动态 `Expression` 接口使用这里的能力。

它处于“函数注册表/具体 builtin”和“通用表达式树”之间：构造阶段从 `funcs` 或 `extensionFuncs` 取得 `functionClass` 并生成 `Box<dyn builtinFunc>`；运行阶段按返回类型选择标量或向量入口，再把实际计算委托给该 builtin。`pkg/expression/core_impl.rs` 中的 `impl Expression for ScalarFunction` 是动态表达式接口到本文件固有方法的桥梁；`pkg/expression/pb_to_expr_runtime.rs::PBToExpr` 则是一个明确上游入口，它用 `NewFunctionBase` 恢复下推表达式而不立即折叠。

## 核心职责

- 保存函数身份、返回类型与实现：`FuncName: ast::CIStr`、`RetType: Option<FieldType>`、`Function: Box<dyn builtinFunc>`。
- 提供八类标量求值和八类向量求值入口，并在测试断言开启时用 `wrapEvalAssert` 包装上下文。
- 统一构造函数：处理特殊函数短路、注册表查找、 noop 策略、NULL 参数类型推导、初始化回调和常量折叠策略。
- 维护普通哈希与规范哈希，分别服务于结构身份、缓存/集合比较和可交换或等价比较式的语义判等。
- 支持表达式树改写：去关联、索引解析、虚拟表达式索引解析、列重映射、遍历和克隆。
- 转发字符集、排序规则、 coercibility、repertoire、显式字符集标记与内存估算等元数据。
- 为排序键分析识别 `列 ± 常量`、`常量 ± 列` 和一元负号等可归约为单列的形态。

本文件不实现具体 SQL 函数算法；算法位于各 `builtin_*` 实现。它也不负责函数名到 PB signature 的编码，那部分在 `expr_to_pb.rs` 等文件中。

## 主要符号

- `ScalarFunction`：核心节点。`hashcode` 和 `canonicalhashcode` 是惰性缓存；参数实际由 `Function.getArgs()/getArgsMut()` 持有。
- `VecEvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`：批量入口，检查 `EvalContext` 后委托 `builtinFunc::vecEval*`。
- `Eval`：依据 `RetType.EvalType()` 分派到 `Eval*`，把 `(值, is_null)` 重新装入 `Datum`；整型保留 unsigned 标志，字符串返回类型为 ENUM 时执行 `ParseEnum` 和截断处理。
- `EvalInt/Real/String/Decimal/Time/Duration/JSON/VectorFloat32`：逐行入口，只负责断言包装与委托 `builtinFunc::eval*`。
- `GetArgs/GetArgsMut`、`Vectorized`：访问子表达式；向量化要求 builtin 自身和全部子节点均支持。
- `Clone/clone_scalar`：克隆 builtin 与类型。`Clone` 还复制字符集、排序规则、 coercibility 和 repertoire；普通哈希清空，规范哈希保留。`clone_scalar` 是较轻的内部克隆，调用方若依赖元数据必须自行确认或改用 `Clone`。
- `Equal`、`Hash64/Equals`：前者实现表达式递归相等，已有普通哈希时走快速比较；后两者实现通用 `HashEquals` 语义，并编码函数名、可空返回类型、参数数量和参数。
- `HashCode/CanonicalHashCode/CleanHashCode`、`ReHashCode`、`simpleCanonicalizedHashCode`：惰性生成、清理和重建两类哈希。参数原地改写后必须清缓存或重哈希。
- `ExpressionsSemanticEqual`：比较两个表达式的规范哈希。
- `typeInferForNull`：当参数同时含 SQL NULL 常量与非 NULL 表达式时，用最后一个非 NULL 参数的类型克隆替换 NULL 常量类型，并移除 `NotNullFlag`。
- `newFunctionImpl`：所有公开构造器的共同实现；`fold` 的 `1/0/-1` 分别表示强制折叠、不折叠、无新增告警时才折叠。
- `NewFunctionWithInit/NewFunction/NewFunctionBase/NewFunctionTryFold/NewFunctionInternal`：公开构造策略。`NewFunctionInternal` 记录错误后返回 `None`，只供旧内部调用。
- `defaultScalarFunctionCheck`：拒绝元数据尚未初始化的 `GROUPING` 节点。
- `ResolveIndices/ResolveIndicesByVirtualExpr/RemapColumn/Decorrelate`：递归改写子节点；公开解析与重映射路径先克隆，避免改写共享树。
- `GetSingleColumn`、`single_column_from_unary/binary`：分析排序键等价列及方向翻转。
- `ScalarFuncs2Exprs`、`emptyScalarFunctionSize`、`MemoryUsage`：动态表达式转换和内存估算辅助。

## 执行流程

构造主流程以 `newFunctionImpl` 为中心：

1. 要求 `ret_type` 非空；`CAST`、`GET_VAR`、内部二进制转换函数直接进入专用 builder，`SYSDATE` 可按会话配置改写为 `NOW`。
2. 先查静态 `funcs`，再查 `extensionFuncs`；均未命中时根据当前数据库是否为空返回“未选择数据库”或“函数不存在”。
3. 根据 `GetNoopFuncsMode()` 处理仅有 noop 实现的函数：Off 返回错误，Warn 追加 warning 并继续，On 正常继续。
4. 除 `IF`、`IFNULL`、`NULLIF`、`ROW` 外，调用 `typeInferForNull`。这些例外分别有控制函数专用推导，或需要在 ROW 展开比较时再推导。
5. 调用 `functionClass::getFunction` 生成 builtin；若 builtin 返回类型已确定，或传入类型仍未指定，以 builtin 类型为准。
6. 组装 `ScalarFunction`，执行可选回调；默认回调验证 `GROUPING` 元数据。
7. 按 `fold` 决定是否调用 `FoldConstant`。尝试折叠模式先记录 warning 数；若折叠新增 warning，则截断新增 warning 并返回未折叠克隆。

求值主流程从动态 `Expression::Eval` 经 `core_impl.rs` 转入 `ScalarFunction::Eval`：读取静态返回类型，调用对应 `Eval*`，错误用 `?` 原样传播；NULL 统一写成空 `Datum`。ENUM 字符串会解析为枚举值，解析失败交给 `TypeCtx::HandleTruncate` 按 SQL 模式决定报错或降级。

改写流程遵守“先克隆、后递归”的共享树保护原则。`ResolveIndices` 和 `RemapColumn` 返回新节点；`Decorrelate` 的固有可变版本会原地替换参数并清哈希，而动态 `Expression` 实现在 `core_impl.rs` 中克隆后递归。任何绕过这些入口直接通过 `GetArgsMut` 修改参数的代码都必须调用 `CleanHashCode` 或 `ReHashCode`。

## 数据与状态

节点的长期状态有三组：函数身份/类型、动态 builtin、哈希缓存。参数和字符集相关元数据均在 builtin 内；因此克隆、重映射和构造回调是否保留 builtin 状态会直接影响表达式语义。

普通哈希编码节点标记、函数名和参数哈希；`CAST` 额外编码返回 `EvalType`，`GROUPING` 额外编码 grouping mode、mark 数量及排序后的键，保证 map 键迭代顺序不会造成非确定哈希。规范哈希进一步把 `+`、`*`、`=`、`IN`、`OR`、`AND` 的参数排序，把 `>=/<=` 和 `>/<` 统一方向，并规范化 `NOT` 包裹的比较运算。`CAST` 同样编码返回类型。缓存使用空 `Vec` 表示尚未计算。

`RetType` 在类型上允许 `None`，以对齐 Go 可空指针及 `Hash64/Equals`；但正常构造与绝大多数求值/显示路径要求它存在，并通过 `unwrap()` 取值。`newFunctionImpl` 明确拒绝空返回类型，因此手工构造节点必须维持这一不变量。

`typeInferForNull` 只替换确为 `Constant`、类型为 `TypeNull` 且值为 NULL 的参数；它不会改变非 NULL 参数，也不会在全 NULL、全非 NULL或少于两个参数时工作。

## 依赖与调用关系

上游直接证据：

- `pkg/expression/lib.rs` 装载并再导出本模块，同时在 `cfg(test)` 下挂载两个独立测试模块。
- `pkg/expression/core_impl.rs::impl Expression for ScalarFunction` 把动态 trait 的求值、类型、相等、改写、哈希与元数据操作转到这里。
- `pkg/expression/pb_to_expr_runtime.rs::PBToExpr` 用 `NewFunctionBase` 重建 PB 标量函数，以保留下推形状。
- `pkg/expression/expression.rs` 的表达式重写与折叠路径调用 `NewFunction`/`NewFunctionInternal`；`pkg/expression/planner_bridge.rs`、`pkg/util/ranger/*`、`pkg/planner/*` 和 `pkg/ddl/storage_class.rs` 也构造或求值标量函数。

下游依赖：

- `builtinFunc`/`functionClass` 及 `funcs`、`extensionFuncs`、`noopFuncs` 提供实际实现和注册信息。
- `BuildCastFunction`、`BuildGetVarFunction`、`BuildFromBinaryFunction`、`BuildToBinaryFunction` 处理特殊构造；`FoldConstant` 处理折叠。
- `Expression`、`Schema`、`Column`、`Constant` 支撑表达式树递归；`types::Datum/FieldType`、`chunk::Row/Chunk/Column` 承载输入输出。
- `EvalContext/BuildContext` 提供 SQL mode、数据库、warning、sysdate/noop 配置和类型错误策略。
- `codec` 与 `base::Hasher` 提供稳定编码；`ast`、`mysql` 提供函数名、求值类型和类型标志。

`Cargo.toml` 确认 crate 名为 `astersql-expression`、入口为 `lib.rs` 且关闭自动测试发现；本文件使用的 parser AST/MySQL、types、chunk、codec、context、session variable、intest 等均由该 crate 的路径依赖或外部依赖提供。测试必须通过 `lib.rs` 的显式 `#[cfg(test)] mod ...` 挂载，不能依赖 Cargo 自动发现。

## 错误处理与边界

- 构造错误包括空返回类型、函数未注册、未选择数据库、noop Off、builtin 参数/类型校验失败、回调失败以及未初始化 `GROUPING`。除 `NewFunctionInternal` 只记录并转成 `None` 外，其余入口保留 `Result`。
- `NewFunctionInternal` 会丢失结构化错误，新增代码不应优先选择它；需要可诊断行为时使用返回 `Result` 的构造器。
- 特殊构造分支直接访问 `args[0]`；调用者必须满足这些函数的最小参数数，否则会越界 panic。通常参数数量由更上游 parser/functionClass 保证。
- `Eval` 对未支持的 `EvalType` 返回显式错误；各具体 `Eval*` 原样传播 builtin 错误。ENUM 转换服从 `TypeCtx` 的截断策略。
- `StringWithCtx` 的 CAST 分支和多处类型/哈希路径假定 `RetType` 存在；手工构造无返回类型节点会 panic。
- `GetSingleColumn` 辅助函数按合法运算符元数直接索引参数；只应传入已完成函数签名校验的 `+`、`-`、一元负号节点。
- 规范哈希是语义等价的工程化归一规则，不是任意 SQL 等价证明；扩展规则前必须确认 NULL、溢出、排序规则和副作用语义允许重排。
- `Equal` 在双方普通哈希均已生成时直接比较哈希，是快速路径；调用者必须在参数原地变化后清除旧缓存，否则可能得到陈旧结果。测试模式的 `assertCheckHashCode` 用重算来捕捉这一错误。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务或外部 I/O。求值期间借用调用方的 `EvalContext`、输入 row/chunk 和输出 column，生命周期不越过方法调用；builtin 由 `Box<dyn builtinFunc>` 独占持有，节点销毁时随之释放。

哈希缓存和参数改写需要 `&mut self`，因此 Rust 类型系统阻止同一节点在没有外部同步的情况下并发写入。`Coercibility()` 虽接收 `&self`，但可能通过 builtin 的内部可变性惰性设置 coercibility；跨线程或跨会话共享能力不能由 `ScalarFunction` 本身推断，必须遵从 `SafeToShareAcrossSession()` 的 builtin 判定。

`intest::EnableAssert` 和 `intest::InTest` 是原子全局测试开关，读取分别使用 Relaxed/SeqCst；它们只影响断言包装和哈希自检，不改变生产 SQL 算法。warning 属于外部 `EvalContext` 状态；尝试折叠会暂时追加 warning，失败回退时将其截断到原计数。

内存估算包含结构体静态大小、函数名、普通哈希容量、可选返回类型和 builtin 自报内存。它没有单独加入规范哈希容量，与 Go 对照实现一致；因此这是兼容口径而非完整分配器统计。

## 与 Go 版本的对应关系

Rust 文件逐段对应 `pkg/expression/scalar_function.go`：字段、八类标量/向量入口、NULL 推导、`NewFunction*` 策略、折叠 warning 回退、哈希规范化、索引解析、列重映射、排序键识别及字符集元数据接口均保持同一控制流。

语言层差异主要是所有权和错误表示：Go 的接口/指针切片映射为 `Box<dyn ...>` 与 `Vec`，可空 `*FieldType` 映射为 `Option<FieldType>`，三返回值求值映射为 `Result<(T, bool), Error>`。Go 回调可返回替换后的 `*ScalarFunction`，Rust `ScalarFunctionCallBack` 原地修改 `&mut ScalarFunction`；当前默认检查和初始化用途仍能表达，但若未来 Go 回调需要用完全不同节点替换对象，Rust 签名需要同步扩展。

Go `typeInferForNull` 仅在原类型不已经满足特定条件时克隆；Rust 对所有匹配的 NULL 常量都克隆并设置推导类型。结果不变量相同：NULL 采用非 NULL 操作数类型且清除 `NotNullFlag`，但分配细节可能不同。

Rust `Eval` 对未知 `EvalType` 返回错误，而当前 Go `switch` 没有显式 default；这是更明确的边界失败。Rust `NewFunctionInternal` 用 `Option` 表示 Go 的可能 nil 表达式。Rust `RemapColumn` 使用 `clone_scalar`，现有回归测试确认字符集、coercibility 和 repertoire 能通过 builtin 克隆保留；修改克隆策略时应继续以该测试为约束。

## 扩展指南

- 新增普通 SQL builtin 时，优先在相应 `builtin_*` 文件和注册表接入；只有需要特殊构造短路、NULL 推导例外、默认初始化或全局折叠策略时才修改 `newFunctionImpl`。
- 新增返回求值类别必须同步：`ScalarFunction::Eval`、逐行 `Eval*`、向量 `VecEval*`、`Expression` trait 及 `core_impl.rs` 转发、具体 builtin trait 方法和独立测试。
- 新增可交换或方向等价函数的规范哈希规则时，同步修改 `simpleCanonicalizedHashCode`，验证 NULL/错误/排序规则/溢出不会破坏等价性，并在 `scalar_function_test.rs` 增加正反例和重复缓存测试。
- 若新函数的返回类型会改变身份，评估 `ReHashCode`、`simpleCanonicalizedHashCode`、`Hash64` 与 `Equals` 是否必须编码更多类型信息；当前只有 CAST 在普通/规范哈希中额外编码 `EvalType`。
- 原地改写 `GetArgsMut()` 后立即调用 `CleanHashCode` 或 `ReHashCode`。若改写还影响 builtin 元数据，应优先用 `Clone()` 并同步验证字符集、coercibility、repertoire 和显式字符集标记。
- 扩展排序键归约时修改 `GetSingleColumn` 及两个私有辅助函数，并在 `scalar_function_37_aster_unit_test.rs` 添加方向、常量位置、嵌套与拒绝案例。
- 构造/折叠行为测试应放在独立测试文件，不要嵌入生产源文件。核心哈希和转换可扩展 `scalar_function_test.rs`；构造上下文、NULL 推导和排序键行为可扩展 `scalar_function_37_aster_unit_test.rs`；PB 无折叠恢复应扩展 `pb_to_expr_runtime_test.rs`。
- 保持 Go 对照：先核对 `scalar_function.go` 的同名逻辑与测试意图，再做最小 Rust 变更；不得为测试通过而删减 warning、NULL、哈希或元数据分支。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`files --filter pkg/expression/scalar_function.rs` 确认目标文件已索引且含 72 个符号；`node --file ... --offset/--limit` 分段核对了 981 行完整源码；`query` 精确定位了 Rust/Go 两侧的 `newFunctionImpl`、`ReHashCode` 和 `ExpressionsSemanticEqual`。
- RustCodeGraph 调用图：执行了目标符号的 `callers`/`callees` 查询，但当前索引未返回可展示的 Rust 边；因此调用关系结论改由实际调用点交叉核验，不据此声称“无调用者”。
- crate/入口证据：`pkg/expression/Cargo.toml`、`pkg/expression/lib.rs`；动态 trait 桥接：`pkg/expression/core_impl.rs`；PB 入口：`pkg/expression/pb_to_expr_runtime.rs`。
- Go 对照：`pkg/expression/scalar_function.go`，逐段核对了构造、求值、哈希、改写、排序键和元数据路径。
- Rust 独立测试：`pkg/expression/scalar_function_test.rs` 覆盖规范语义相等、常量值/类型区分、空转换、重映射元数据和 NOT 非比较 fallback；`pkg/expression/scalar_function_37_aster_unit_test.rs` 覆盖单列减法方向和 NULL 类型推导。本任务只做文档分析，按计划不运行 Cargo。
- 人工复核重点：文档区分本文件的编排职责和 builtin 算法职责，说明了构造/求值/改写/哈希流程，明确了可空类型、参数元数、缓存失效、warning 回退和规范哈希适用边界，并给出安全扩展位置及对应独立测试文件。
