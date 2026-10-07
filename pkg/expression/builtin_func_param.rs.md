# `pkg/expression/builtin_func_param.rs`

## 文件定位

`builtin_func_param.rs` 是 `astersql-expression` crate 内一个小型、类型化的内置函数参数规约器。它把“参数未提供、常量、已经批量求值的列、求值失败”四类输入统一成可按行读取的 `FuncParam<T>`，对应 Go `pkg/expression/builtin_func_param.go` 的 `funcParam`、`buildStringParam` 和 `buildIntParam` 概念。

crate 根 `pkg/expression/lib.rs` 通过 `#[path = "builtin_func_param.rs"] mod builtin_func_param_kernel;` 装入该文件，但没有把它作为正式公共 API 再导出。当前唯一的再导出位于 `#[cfg(test)] mod expression_encryption`，供独立测试 `pkg/expression/builtin_encryption_11_aster_unit_test.rs` 使用。仓库精确引用搜索没有发现生产 Rust 调用者，因此它目前是已装配、已测试但尚未接入 Rust 正则/ILIKE 执行链的迁移实现，不能据此声称 Go 的生产调用链已经移植完成。

`pkg/expression/Cargo.toml` 声明包名为 `astersql-expression`、库入口为 `lib.rs`、`autotests = false`，并用 porting metadata 指向 Go 包 `pkg/expression`。本文件的非标准库直接依赖只有 manifest 中的 `thiserror = "2"`。

## 核心职责

1. 用 `ParamSource<T>` 显式表示调用方完成表达式分类/求值后的输入状态，避免本文件依赖完整 `EvalContext`、表达式树或 chunk 分配器。
2. 将缺省参数和非 NULL 常量压缩成单值 `ParamStorage::Default`，将非常量批量结果保存为 `ParamStorage::Column`。
3. 将常量 SQL NULL 提升为独立的 `BuildParam::ConstNull`，使调用方能在进入逐行计算前短路。
4. 将参数求值失败转换成 `ParamError::Evaluation`，并在列下标越界时返回包含下标和列长的 `ParamError::RowOutOfBounds`。
5. 为字符串和整数提供语义明确的构建入口：字符串缺省为空串，整数缺省值由具体内置函数传入。

它不负责判定表达式是否为常量、不执行标量或向量表达式、不分配/回收列缓冲，也不实现具体 SQL 内置函数；这些决策在当前 API 中由构造 `ParamSource` 的上游承担。

## 主要符号

- `ParamSource<T>`：公开输入枚举。`NotProvided` 表示可选实参缺省；`Constant(Option<T>)` 用 `None` 表示常量 SQL NULL；`Column(Vec<T>)` 保存已求值的非空元素列；`EvalError(String)` 保存上游求值失败文本。
- `ParamError`：公开错误枚举，派生 `thiserror::Error`。`Evaluation(String)` 保留上游错误文本；`RowOutOfBounds { index, length }` 精确报告非法行号及列长度。
- `ParamStorage<T>`：文件私有存储枚举。`Default(T)` 同时承载缺省值与非 NULL 常量，`Column(Vec<T>)` 承载逐行值；私有性保证调用方只能通过安全访问器观察它。
- `FuncParam<T>`：公开参数句柄，内部只含一个 `ParamStorage<T>`。`get(row)` 对单值存储忽略行号，对列存储执行边界检查；`is_column()` 判别存储形态；`column()` 仅为列形态返回切片。
- `BuildParam<T>`：公开构建结果。`Value(FuncParam<T>)` 表示可继续求值，`ConstNull` 表示整项是常量 NULL；`value()` 将两种形态映射为 `Some(&FuncParam<T>)` 或 `None`。
- `build_param<T>`：文件私有的统一五分支规约函数，接收来源与缺省值，是两个公开构建器的共同实现。
- `build_string_param`：公开字符串入口，缺省值固定为 `String::new()`。
- `build_int_param`：公开整数入口，要求调用方传入 `default_int_value`，以覆盖 position、occurrence、return option 等不同 Go 缺省约定。

文件没有模块级常量、trait、条件编译项、异步函数或 `unsafe` 代码。各公开数据类型都派生 `Debug`、`Clone`、`PartialEq`、`Eq`，便于测试和按值传递。

## 执行流程

调用流程从上游先完成表达式分类开始：缺省实参生成 `NotProvided`，常量求值得到 `Constant(Some(value))` 或 `Constant(None)`，非常量表达式批量求值后生成 `Column(values)`，失败则生成 `EvalError(message)`。随后调用字符串或整数构建器：

1. `build_string_param` 提供空串缺省值，`build_int_param` 接受具体函数给出的整数缺省值；二者都转入 `build_param`。
2. `NotProvided` 被包装为 `BuildParam::Value(FuncParam { Default(omitted_default) })`。
3. `Constant(Some(value))` 使用常量本身构造相同的 `Default` 存储；因此后续任意行号都返回同一个值。
4. `Constant(None)` 不创建 `FuncParam`，而直接返回 `BuildParam::ConstNull`，调用方应在批量循环前应用 SQL NULL 短路语义。
5. `Column(values)` 转移向量所有权到 `ParamStorage::Column`；后续 `get(row)` 通过 `Vec::get` 借用该行。
6. `EvalError(message)` 直接返回 `Err(ParamError::Evaluation(message))`，不会产生部分构建结果。

构建成功后，调用方可先用 `BuildParam::value()` 区分常量 NULL，再以 `FuncParam::get(row)` 统一读取常量/缺省与列值。测试证明常量在行号 99 仍返回同一字符串、缺省整数在行号 3 仍返回默认值、三元素列的下标 2 成功而下标 3 报错。

## 数据与状态

所有状态都归单个 `FuncParam<T>` 所有，没有全局变量或隐藏缓存。`ParamSource` 按值进入构建器；常量值、缺省值或 `Vec<T>` 随后被移动进结果，不发生克隆。`get`、`column` 和 `value` 只返回借用，调用者不能通过这些 API 修改内部数据。

`Default` 不是“第零行”的特殊列，而是对任意 `usize` 行号都有效的单值广播。因此它没有行数概念，也不会校验调用者传入的行号是否属于当前输入批次。相反，`Column` 的长度就是本类型唯一可验证的行范围；空列允许构建，但任何 `get` 都会得到 `RowOutOfBounds`。

NULL 的表达能力有意受限：`Constant(None)` 能表示常量 SQL NULL并触发整体短路，但 `Column(Vec<T>)` 的元素不是 `Option<T>`，不能表示逐行 NULL。逐行 NULL 位图/可空列必须由上游或最终生产接线另行携带。`EvalError` 同样只保存字符串，不保留结构化错误源或错误链。

## 依赖与调用关系

- 装配上游：`pkg/expression/lib.rs` 无条件装入 `builtin_func_param_kernel`，但该模块是私有的。只有测试配置下的 `expression_encryption::builtin_func_param` 会 `pub use` 其符号。
- 当前 Rust 调用者：`pkg/expression/builtin_encryption_11_aster_unit_test.rs::grouping_fts_and_function_parameters_preserve_go_control_flow` 调用两个公开构建器，并覆盖常量、常量 NULL、缺省整数、列读取/越界和求值错误。精确 `rg` 搜索没有发现其他 Rust 使用点。
- 文件内部调用边：RustCodeGraph 的 callees 结果确认 `build_string_param -> build_param` 和 `build_int_param -> build_param`；其余访问器不调用仓库内其他符号。
- Cargo 依赖：`ParamError` 的展示文本来自 `thiserror::Error` derive；泛型容器与 `Vec`、`String`、`Option`、`Result` 均来自 Rust 标准库。本文件没有 feature gate。
- Go 生产上游：`pkg/expression/builtin_regexp.go` 的 REGEXP_LIKE/INSTR/SUBSTR/REPLACE 向量路径反复调用 `buildStringParam`、`buildIntParam` 并逐行调用 `getStringVal`/`getIntVal`；`pkg/expression/builtin_ilike_vec.go` 也使用同一 Go `funcParam`。
- Go 资源下游：`pkg/expression/builtin_regexp_util.go` 的 `getBuffers`/`releaseBuffers` 从 Go `funcParam.col` 收集并归还 `chunk.Column`。Rust 当前目标类型并不与 `pkg/expression/builtin_regexp_util.rs::FuncParam` 共用类型；后者是另一套面向任务本地正则辅助测试的列/分配器模型。

因此当前完整应用中的真实位置应描述为“expression crate 内的参数规约迁移单元和测试证据”，而非已经进入 SQL 请求执行主链的生产组件。

## 错误处理与边界

- `EvalError(String)` 在构建时一对一变成 `ParamError::Evaluation`；错误文本的展示前缀是 `parameter evaluation failed:`。当前设计不保留原始错误类型，也没有 `source()` 链。
- 列访问使用 `Vec::get`，不会因越界 panic；错误同时记录请求的 `index` 和实际 `length`。`usize` 排除了负下标，但不验证批次外的 `Default` 行号。
- 常量 NULL 是成功结果 `Ok(BuildParam::ConstNull)`，不是错误；调用者若直接调用 `value()` 会得到 `None`，必须将其解释为 NULL 短路，而不是“构建缺失”。
- `NotProvided` 与显式空串/显式整数缺省值最终具有同一 `Default` 存储，构建后无法追溯来源。若具体函数需要区分“省略”和“显式给值”，必须在调用构建器前处理。
- `Column(Vec<T>)` 不检查列长是否等于输入 chunk 行数，也不表示逐行 NULL；这是最重要的当前接线边界。不要通过填充默认值悄悄代替 SQL NULL。
- `build_string_param` 的省略缺省固定为空串；`build_int_param` 不验证传入缺省值的 SQL 合法性。position/occurrence 等范围约束属于具体内置函数，而不是本文件。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道或事务，也没有共享静态状态。每次构建都取得 `ParamSource` 的所有权并生成独立 `BuildParam`；是否跨线程传递取决于泛型 `T` 自身是否满足相应 auto trait，API 没有额外声明 `Send`/`Sync` 边界。

列向量的生命周期与拥有它的 `FuncParam` 相同，`column()`/`get()` 的借用不能超过该句柄。对象析构时由 Rust 正常释放 `Vec<T>`；当前实现没有 Go `baseBuiltinFunc.bufAllocator.get/put` 那样的缓冲池借用和归还协议。因此它既不会泄漏一个显式资源句柄，也不会复用批量列容量；未来接入生产向量路径时，需要同时设计列 NULL 信息、分配失败和缓冲归还，不能只把 `Vec<T>` 当成 Go `*chunk.Column` 的完全等价物。

派生的 `Clone` 会深拷贝 `String`/`Vec` 等拥有数据（取决于 `T: Clone`）。大列的无意 clone 可能带来线性时间与内存开销；当前读取 API 本身全部是共享借用且无锁。

## 与 Go 版本的对应关系

- Rust `FuncParam<T> { storage }` 对应 Go `funcParam` 的 `defaultStrVal`/`defaultIntVal` 与可选 `col` 两种形态。Rust 通过泛型把字符串和整数统一为一个结构，通过枚举保证 Default/Column 互斥；Go 结构可同时保留多个字段。
- Rust `FuncParam::get` 合并了 Go `getStringVal` 和 `getIntVal`。常量/省略参数忽略行号，列参数按行读取，这一核心广播语义一致；Rust 额外把越界变成 `Result`，Go `chunk.Column` 访问遵循其自身边界行为。
- Rust `BuildParam::ConstNull` 对应 Go 构建器的第二个布尔返回值 `isConstNull = true`。Rust 用枚举避免“指针、布尔、错误”三返回值出现不一致组合。
- Rust `ParamSource::EvalError` 对应 Go `EvalString`/`EvalInt`/`VecEval*` 返回的错误，但 Rust API 要求上游先求值；它本身不持有 `EvalContext`、`baseBuiltinFunc`、参数下标或输入 `Chunk`。
- Rust `NotProvided` 的字符串空值及调用方传入的整数缺省值与 Go 两个构建器的缺省分支一致。Go 正则调用点可见 position/occurrence 常用 1、return option 常用 0。
- Go 用 `ConstLevel() >= ConstOnlyInContext` 在每次求值上下文内判断常量，并把非常量表达式求值到分配器提供的 `chunk.Column`；Rust 当前把分类、求值、NULL 列和缓冲生命周期全部移到 `ParamSource` 生产者之外。
- Go `setStrVal`、`setCol` 支持 ILIKE 对常量或列进行小写转换并替换存储；Rust 目标 `FuncParam` 没有可变 setter。Go 的 `getCol` 还服务于 NULL 合并、记忆化判断和缓冲归还；Rust 的 `column()` 只暴露只读 `&[T]`。

上述差异说明 Rust 文件保留了参数规约的核心控制流，但尚不是 Go 实现的完整生产替代品。尤其不能用现有测试推断向量 NULL、分配器复用、ILIKE 变换或正则执行已经对齐。

## 扩展指南

若只新增一种标量参数类型，优先复用私有 `build_param`，新增一个公开、类型明确的薄构建器，并由调用方明确提供省略值；不要复制五分支状态机。对应回归应放在独立测试文件，当前最近入口是 `pkg/expression/builtin_encryption_11_aster_unit_test.rs`，不要把测试内嵌到生产 `.rs`。

若要把该类型接入真实 Rust REGEXP/ILIKE 向量执行链，需要先解决而不是绕开以下事项：

1. 定义从正式表达式、`EvalContext` 和输入 chunk 产生 `ParamSource` 的位置，并保持 Go 的 `ConstOnlyInContext` 判定时机。
2. 为列参数保留逐行 NULL 状态；可改用可空列抽象或并行 NULL 位图，但必须覆盖常量 NULL与行级 NULL的不同短路范围。
3. 明确缓冲分配、分配失败和归还所有权，并与 `builtin_regexp_util.rs` 的现有辅助类型整合或清楚分层，避免仓库长期存在两个不兼容的 `FuncParam`。
4. 若 ILIKE 需要原地/替换式小写转换，为可变访问设计受控 API，同时验证常量与列两条路径、NULL 合并和内存复用。
5. 保留 `ParamError::RowOutOfBounds` 的非 panic 契约，并增加空列、首尾下标、列长与批次不匹配、每行 NULL、上游结构化错误等测试。
6. 对照 `pkg/expression/builtin_regexp_test.go` 的 REGEXP_LIKE/INSTR/SUBSTR/REPLACE 向量用例，以及 `builtin_ilike_vec.go` 的常量/列组合，验证真实函数行为，而不能只保留当前构建器单元测试。

任何扩展都应继续保持源文件与 Rust 测试分离，并同步核对 Go 的具体调用点和缺省值。若只是增加公开性，还需先证明有生产调用者；不应为了“看起来已接线”而无条件从 crate 根再导出未使用 API。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/expression/builtin_func_param.rs --offset 1 --limit 400` 完整读取了目标文件 138 行及其装配使用概览。
- RustCodeGraph：`query` 精确定位 `FuncParam`、`build_param`、`build_string_param`、`build_int_param`；`callees` 确认两个公开构建器均只调用私有 `build_param`。图工具对路径限定符存在候选合并，因此生产引用结论又用精确文本搜索复核。
- Rust 源与装配：读取 `pkg/expression/lib.rs` 中 `builtin_func_param_kernel` 装入、`expression_encryption` 测试门面和 `builtin_encryption_11_aster_unit_test.rs` 测试模块声明；目标包没有 `pkg/expression/doc.go`。
- Cargo：读取 `pkg/expression/Cargo.toml`，核对 crate 名、`lib.rs` 入口、关闭自动测试发现、`thiserror` 依赖和 `go-package = "pkg/expression"` 的移植边界。
- Rust 测试：读取 `pkg/expression/builtin_encryption_11_aster_unit_test.rs::grouping_fts_and_function_parameters_preserve_go_control_flow`，核对常量广播、常量 NULL、缺省整数、列成功/越界和求值错误；精确引用搜索确认这是当前唯一 Rust 消费点。
- Go 对照：完整读取 `pkg/expression/builtin_func_param.go`；检索并读取其直接调用/资源关系，证据来自 `pkg/expression/builtin_regexp.go`、`builtin_ilike_vec.go`、`builtin_regexp_util.go` 及相关 `builtin_regexp_test.go` 用例位置。
- 迁移边界：读取 `pkg/expression/builtin_regexp_util.rs`，确认其中存在另一套 `FuncParam<T>`、`Column<T>` 与 `BufferAllocator<T>`，且目标文件没有与其建立类型或调用连接。
- 本任务为纯文档分析，依计划未运行 Cargo。本文内容已人工复核，不把 Rust 测试门面、Go 生产调用链或相邻正则辅助模型误写为目标文件的生产接线。
