# `pkg/expression/explicit_collation.rs`

## 文件定位

新排序规则模式下查找用户给出的排序规则，并校验它与参数字符集匹配；JSON 按 `utf8mb4` 而非其字段元数据中的 `binary` 字符集校验。

## 核心职责

`SetCollationToExpression`（`pkg/expression/explicit_collation.rs:32-92`）负责四件事：

1. 新排序规则模式下查找用户给出的排序规则，并校验它与参数字符集匹配；JSON 按 `utf8mb4` 而非其字段元数据中的 `binary` 字符集校验。
2. 对列和 JSON 构造新的 CAST 表达式，避免直接污染共享的列 `FieldType`；JSON 的 CAST 目标固定为 `LONGTEXT`（Rust 类型码为 `TypeLongBlob`）与 `utf8mb4`。
3. 对其他表达式直接修改其返回类型的 collation。
4. 把结果 coercibility 设为 `CoercibilityExplicit`，并用最终类型的 charset/collation 同步表达式自身的排序规则元数据，使后续推导尊重显式 `COLLATE`。

该文件只处理表达式元数据与必要的 CAST 包装，不解析 SQL、不选择 collation 比较算法，也不执行字符串比较。

## 主要符号

- `pub fn SetCollationToExpression(ctx: &dyn BuildContext, mut argument: ExprBox, collation: &str, use_new_collation: bool) -> Result<ExprBox, Error>`：唯一模块级符号和公开 API。它取得构建上下文、消费一个装箱表达式、接收目标排序规则名及新旧排序规则模式开关，成功时返回原表达式或包装后的表达式，失败时返回 crate 的统一 `Error`。
- `BuildContext` / `GetEvalCtx`：提供读取表达式类型所需的求值上下文，也是 `BuildCastFunction` 构造 CAST 时使用的上下文边界。
- `ExprBox` / `Expression`：输入输出的动态表达式所有权容器。函数通过 `as_any().is::<Column>()` 识别列，通过 `GetType`、`GetTypeMut`、`SetCoercibility` 和 `SetCharsetAndCollation` 操作类型及推导元数据。
- `CoercibilityExplicit`：表示用户显式指定排序规则的最高优先级标记；无论走 CAST 还是就地更新，成功返回前都会写入。

文件没有常量、结构体、枚举、trait、`impl` 或条件编译项。

## 执行流程

1. 通过 `ctx.GetEvalCtx()` 和 `argument.GetType(eval_ctx)` 取得输入类型（`explicit_collation.rs:38-39`）。
2. 当 `use_new_collation` 为真时，调用 `collate::GetCollationByName` 查表。未知名称立即通过 `?` 返回错误。随后确定待校验字符集：JSON 强制使用 `charset::CharsetUTF8MB4`，其他类型使用输入 `FieldType` 的 charset；非空字符集若与查到的 `Collation.CharsetName` 不同，则返回 `ErrCollationCharsetMismatch`（`explicit_collation.rs:41-57`）。
3. 判断输入是否为 `Column`，并缓存 MySQL 类型码。列或 JSON 进入 CAST 分支；其中 ENUM/SET 列先被拒绝（`explicit_collation.rs:59-66`）。
4. JSON 新建 `TypeLongBlob` 目标类型并设置 `utf8mb4`；普通列克隆原 `FieldType`。两者都把目标 collation 写入克隆/新类型，然后调用 `BuildCastFunction` 生成新的表达式（`explicit_collation.rs:68-77`）。
5. 非列且非 JSON 的表达式直接经 `GetTypeMut().SetCollate` 修改返回类型（`explicit_collation.rs:78-80`）。
6. 成功路径统一设置显式 coercibility，再从最终结果类型重新读取 charset/collation，并调用 `SetCharsetAndCollation` 同步表达式元数据，最后返回表达式（`explicit_collation.rs:82-91`）。

## 数据与状态

函数没有模块级或全局可变状态。`argument` 的所有权传入函数：列/JSON 路径把它作为 CAST 子表达式克隆进新节点并用新节点替换局部变量；普通表达式路径则原位修改其 `FieldType`。这一区别是核心不变量：列类型可能被 schema 或其他表达式共享，不能为了一个局部 `COLLATE` 就地更改。

类型状态分为两层，成功路径必须保持一致：`FieldType` 保存 charset/collation，表达式的 collation metadata 保存 coercibility 以及 charset/collation。最后一次 `SetCharsetAndCollation` 使用最终 `GetType` 的值，避免 CAST 后两层元数据脱节。JSON 路径还发生求值类型转换：`TypeJSON` 变为 `TypeLongBlob`，charset 变为 `utf8mb4`。

`use_new_collation` 是调用者提供的快照参数，不读取或修改全局开关。为假时函数不查 collation 表，也不校验字符集，所以任意字符串会按旧模式写入类型；`pkg/expression/explicit_collation_test.rs:257-276` 明确锁定了这一兼容行为。

## 依赖与调用关系

上游方面，`pkg/expression/lib.rs` 挂载并公开再导出本函数。RustCodeGraph 对 `SetCollationToExpression` 的精确查询只找到五个调用者，均在 `pkg/expression/explicit_collation_test.rs:187-277`；没有发现生产调用者。Go 主链中，对应工作由规划器表达式改写器在访问 `ast.SetCollationExpr` 后直接完成，而不是调用独立的 Go `pkg/expression` 函数。

下游方面，源码中的直接依赖是：

- `collate::GetCollationByName`：来自 `astersql-util-collate`，解析名称并给出规范 collation/charset 信息。
- `charset::ErrCollationCharsetMismatch` 与 `charset::CharsetUTF8MB4`：来自解析器字符集依赖，分别构造兼容错误及规定 JSON 字符集。
- `mysql::{TypeJSON, TypeEnum, TypeSet, TypeLongBlob}`：决定特殊类型分支。
- `types::NewFieldType`：构造 JSON 的 LONGTEXT 目标类型。
- `BuildCastFunction`：表达式 crate 内的 CAST 构造器；`pkg/expression/builtin.rs:7291-7333` 表明它会克隆目标类型、保留必要的标志和 collation 推导信息，并创建/折叠 CAST 表达式。
- `Expression` 的类型与 collation 接口：读取/修改 `FieldType`，以及设置 coercibility 和 charset/collation 元数据。

RustCodeGraph 的 `callees SetCollationToExpression` 没有生成边；这是索引对通配符再导出/动态 trait 调用覆盖不足的结果，不能覆盖源码中上述可直接核验的调用。

## 错误处理与边界

- 新模式下未知 collation 名由 `GetCollationByName` 的错误原样向上传播；测试只约束错误文本包含请求名称（`explicit_collation_test.rs:194-201`）。
- 新模式下，非空输入 charset 与目标 collation charset 不一致时返回带目标 collation 名和输入 charset 的 `ErrCollationCharsetMismatch`（`explicit_collation.rs:49-55`；测试见 `explicit_collation_test.rs:187-193`）。空 charset 不触发此检查。
- JSON 无论原字段 charset 写成什么，都按 `utf8mb4` 检查，并在成功时转为 `LONGTEXT utf8mb4`（测试见 `explicit_collation_test.rs:226-239`）。
- ENUM/SET 的拒绝位于“列或 JSON”分支内，因此源码严格保证的是 ENUM/SET **列**报错；不能把它扩大表述为所有可能承载 ENUM/SET 类型的表达式都会报错。Rust 使用 `errors::New` 构造提示文本，Go 对照使用 `plannererrors.ErrNotSupportedYet`；行为意图和消息主体一致，但结构化错误类别并不相同。
- 旧模式有意跳过名称存在性及字符集匹配校验。调用者必须确保这是所需兼容模式，不能把该路径当作已验证的 collation 注册入口。
- 函数不验证目标 collation 是否适用于非字符串求值类型；Go 注释也明确显式 collation 可设置到非字符串表达式。

## 并发与资源生命周期

本文件不创建锁、线程、异步任务、通道、事务、文件或网络资源。函数只在调用栈内消费并返回 `ExprBox`，资源生命周期由 Rust 所有权及表达式树持有关系管理。

并发安全性取决于传入 `BuildContext`、表达式实现以及下游 collation 注册表的契约；本函数本身没有同步原语。它没有修改全局“新 collation”状态，而是使用显式布尔参数，因此同一进程中不同调用可选择不同模式。普通表达式会被原位修改，调用方若通过内部共享机制复用同一表达式，必须遵守具体表达式类型的可变性约束；本文件没有提供跨线程共享保证。

## 与 Go 版本的对应关系

可见差异是 Rust 将会话状态抽成 `use_new_collation` 参数并把操作做成可复用函数；Go 直接读取 `er.sctx.NewCollationEnabled()` 并更新规划器的 `ctxStack`。Go 使用 `plannererrors.ErrNotSupportedYet` 表示 ENUM/SET 不支持，Rust 当前只创建通用错误文本。此外，Rust 函数虽已公开再导出，但索引未显示它已替换 Rust 规划器中的对应分支。

## 扩展指南

- 若要把该能力接入 Rust SQL 规划主链，应在处理 `ast.SetCollationExpr` 的表达式改写位置调用本函数，并明确从会话/构建上下文取得 `use_new_collation`；不要在本文件内引入全局模式读取。接线后应新增独立规划器测试，证明 SQL AST 到本函数再到结果表达式的完整路径。
- 新增特殊数据类型时，优先审查 `SetCollationToExpression` 的分支顺序及 CAST 目标类型。必须同时保持“共享列类型不被修改”和“最终类型元数据与 expression collation metadata 一致”两个不变量。
- 若改变错误类型或校验规则，应与 `pkg/planner/core/expression_rewriter.go` 的 Go 分支同步，并在 `pkg/expression/explicit_collation_test.rs` 增加错误类别、参数和边界测试，而不是把测试嵌入生产文件。
- 若修改 JSON、ENUM 或 SET 行为，应补充新旧 collation 模式、列与非列表达式的矩阵测试；当前测试只证明 ENUM/SET 列被拒绝。
- 若改变 `BuildCastFunction` 的 collation 继承或常量折叠行为，需要重新验证本函数在 CAST 后读取到的 charset/collation，以及原列类型不污染属性。
- 性能风险主要来自新增 CAST 节点及其可能触发的常量折叠；兼容风险集中在旧模式允许未注册名称、JSON 的强制 `utf8mb4` 语义和结构化错误类别。
