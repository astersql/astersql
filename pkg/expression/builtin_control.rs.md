# `pkg/expression/builtin_control.rs`

## 文件定位

该文件属于 `astersql-expression` crate（见 `pkg/expression/Cargo.toml`），由 `pkg/expression/lib.rs` 通过 `#[path = "builtin_control.rs"] mod builtin_control_kernel;` 以私有模块装配。它是一个自包含的 Rust 控制流内核：一部分用 `SqlValue` 执行标量及按行的 `CASE WHEN`、`IF`、`IFNULL`，另一部分用简化的 `FieldType` 模型推导这些控制函数的结果类型。

当前接线范围需要明确：该模块没有导出为 crate 公共 API；仓库内可见的直接引用主要来自同 crate 的 `builtin_control_test.rs` 与 `builtin_control_9_aster_unit_test.rs`。生产表达式框架中另有 `planner_bridge.rs` 的类型推导实现，生成式向量控制函数也位于 `builtin_control_vec_generated.rs`。因此，本文件提供的是可测试的移植内核，不应被描述成已经替代 Go `builtin_control.go` 的完整函数类、表达式构造与下推接线。

## 核心职责

1. 用 `SqlValue` 统一承载 SQL NULL、常见标量类型、向量值和显式错误占位，并用 `is_true` 实现控制条件的真值转换。
2. 在 `eval_case_when`、`eval_if`、`eval_if_null` 中保持 Go 控制函数的从左到右选择、NULL 条件为假以及未选中分支不观察错误的短路语义。
3. 通过三个 `*_rows` 函数把相同标量规则逐行应用到批量输入，并检查多列行数一致性。
4. 用 `infer_type_for_control` 归并候选结果的求值类型、字段种类、显示宽度、小数位、标志以及字符集/校对信息，并处理 NULL、ENUM/SET 和 DATETIME/TIMESTAMP 的收尾规则。

这些职责直接对应 `pkg/expression/builtin_control.go` 中 `InferType4ControlFuncs`、各类型的 CASE/IF/IFNULL `eval*` 方法及其辅助函数，但 Rust 以枚举而非 Go 的 `Expression`、`types.FieldType` 和多组 typed signature 表达。

## 主要符号

- 常量 `UNSPECIFIED_LENGTH`、`MAX_REAL_WIDTH`、`NOT_NULL_FLAG`、`UNSIGNED_FLAG`、`BINARY_FLAG`：提供类型推导需要的哨兵、宽度和位标志。它们是 Go `types.UnspecifiedLength`、`mysql.MaxRealWidth` 及对应 MySQL flags 的局部表示。
- `EvalType`：结果归约使用的类型族；`is_string_kind` 仅识别 `String`。时间戳与日期时间、JSON、Duration、VectorFloat32 保持独立类型族。
- `FieldKind` 与 `FieldType`：简化的 MySQL 字段类型元数据。`FieldType::new` 提供各 kind 的默认 flen、decimal、charset/collation；`with_*` 是构造测试输入的链式修改器；`eval_type`、`is_binary_string`、`is_non_binary_string` 是推导辅助查询。
- `SqlValue`：控制流运行值。`Null` 表示 SQL NULL；`Error(String)` 是惰性求值边界的测试性错误占位，不是一般表达式执行器。
- `EvalError`：字符串错误包装，实现 `Display` 和标准错误 trait。内部 `evaluated` 仅在实际选择某个结果值时把 `SqlValue::Error` 转成 `Err`。
- `numeric_prefix` 与 `is_true`：前者扫描字符串的符号、数字、小数点和指数前缀，后者按 `SqlValue` variant 判定真假或传播错误。
- `eval_case_when`、`eval_if`、`eval_if_null`：三个标量入口。它们返回克隆的 `SqlValue`，不会取得输入所有权或修改输入。
- `eval_case_when_rows`、`eval_if_rows`、`eval_if_null_rows`：按行入口。前者每行自带一组 CASE 参数；后两者接受平行列。
- `max_len`、`set_flen_from_args`、`set_decimal_from_args`：计算结果显示宽度与小数位。`integer_display_width` 只为字符串归约时的整数 kind 提供固定宽度。
- `aggregate_eval_type`、`aggregate_kind`、`aggregate_flags`：分别归约求值族、具体 kind 和标志。它们是 `infer_type_for_control` 的内部阶段。
- `derive_charset` 与 `set_binary_charset`：为支持的控制函数名选择 charset/collation，并维护 `BINARY_FLAG`。
- `infer_type_for_control`：类型推导总入口，要求至少一个字段参数，返回新的 `FieldType` 或 `EvalError`。

## 执行流程

标量 CASE 流程由 `eval_case_when` 定义。函数先拒绝少于两个参数的输入，再把偶数长度部分解释为 `(WHEN, THEN)` 对；奇数长度时最后一个参数是 ELSE。它从索引 0 开始每次前进 2，通过 `is_true` 求条件；首个真条件立即对对应 THEN 调用 `evaluated` 并返回。若没有命中，则奇数参数返回 ELSE，偶数参数返回 `SqlValue::Null`。因此首个命中后的条件和结果不会被观察。

`eval_if` 只先对 condition 调用 `is_true`，随后仅对被选中的 true/false 值调用 `evaluated`。`eval_if_null` 则先区分 first：`Error` 立即失败，`Null` 才观察 fallback，其他值直接克隆返回。三个入口都把“被选择后才传播结果错误”作为短路边界。

按行入口不引入新的求值规则：`eval_case_when_rows` 直接 map `eval_case_when`；`eval_if_rows` 和 `eval_if_null_rows` 先验证平行切片长度，再 zip 并调用标量函数。迭代器 `collect::<Result<...>>()` 使首个错误终止整个批次，先前已生成的结果不会作为部分成功返回。

类型推导由 `infer_type_for_control` 驱动：先把参数克隆并按 `FieldKind::Null` 分组；全 NULL 时构造 NULL/binary 结果并清除 NOT NULL；只有一个非 NULL 候选时直接以它为基础；多个非 NULL 候选时依次调用 `aggregate_eval_type`、`aggregate_kind`、`aggregate_flags`、`set_decimal_from_args`、`derive_charset`、`set_flen_from_args`。最后根据 NULL 的存在清除 NOT NULL，并修正 Int/String decimal、ENUM/SET kind 以及 DATETIME/TIMESTAMP 的带小数秒显示宽度。

## 数据与状态

文件没有全局可变状态。五个常量是编译期值；其余状态完全来自参数或函数内局部变量。

`SqlValue` 和 `FieldType` 都拥有其字符串/字节/向量数据。求值函数接收共享引用并通过 `Clone` 生成结果，因此不会改变调用者输入；代价是 String、Binary、Json 和 VectorFloat32 等结果会复制其堆数据。类型推导也先克隆字段描述，再修改独立结果。

主要不变量如下：

- `SqlValue::Null` 与 `SqlValue::Error` 含义互斥；前者参与 SQL NULL 流程，后者在被观察时成为 `EvalError`。
- CASE 参数按成对前缀加可选末尾 ELSE 解释；最低合法长度是 2。
- 多列按行 IF/IFNULL 必须具有相同行数。
- `aggregate_eval_type` 只在非空字段集合上调用；`aggregate_kind` 同样依赖至少一个字段。该前置条件由 `infer_type_for_control` 的分组分支保证。
- NOT NULL 和 UNSIGNED 只有所有非 NULL 候选都具有相应标志时才聚合；BINARY 只要任一候选具有即聚合。随后任一显式 NULL 候选都会清除结果 NOT NULL。
- Decimal/Int flen 上限在当前实现中硬截为 65，非 Int decimal 上限截为 30；未知宽度/小数位使用 `-1` 哨兵传播或回退。

## 依赖与调用关系

文件只直接依赖标准库 `std::fmt`，没有使用 `pkg/expression/Cargo.toml` 中的外部 crate。因此它的运行值和字段模型都是本地定义，不持有会话、chunk、存储或网络对象。

模块装配边为 `pkg/expression/lib.rs` → `builtin_control_kernel`。RustCodeGraph 对目标文件报告 11 个使用文件，其中明确包含 `pkg/expression/builtin_control_test.rs`、`pkg/expression/builtin_control_9_aster_unit_test.rs` 以及若干相邻表达式/测试文件；文本引用核验显示本文件公开函数的直接调用集中在上述两个独立测试模块。`builtin_control_test.rs` 通过 `use crate::builtin_control_kernel::*` 使用由 crate 根装配的模块；`builtin_control_9_aster_unit_test.rs` 则用 `#[path = "builtin_control.rs"]` 建立测试局部模块。

文件内部的关键调用边为：

- `eval_case_when` / `eval_if` → `is_true`，命中后 → `evaluated`；`eval_if_null` 在 NULL fallback 路径 → `evaluated`。
- `eval_case_when_rows` → `eval_case_when`，`eval_if_rows` → `eval_if`，`eval_if_null_rows` → `eval_if_null`。
- `infer_type_for_control` → `aggregate_eval_type` → `FieldType::eval_type`；并继续调用 `aggregate_kind`、`aggregate_flags`、`set_decimal_from_args`、`derive_charset`、`set_flen_from_args`。
- `derive_charset` → `FieldType::{is_binary_string,is_non_binary_string}`，需要 binary 结果时 → `set_binary_charset`。

与 Go 主链相比，本文件没有 `functionClass.getFunction`、`baseBuiltinFunc`、`BuildContext`、`EvalContext`、`chunk.Row`、`tipb.ScalarFuncSig` 或 `CheckAndDeriveCollationFromExprs` 的调用边；这些仍位于 `pkg/expression/builtin_control.go` 的完整执行路径。Rust 的 `planner_bridge.rs` 也另有同名宽度/小数位辅助逻辑，不能假定它会调用这里的实现。

## 错误处理与边界

显式错误路径包括：CASE 参数少于 2 个；IF/IFNULL 按行输入长度不同；类型推导参数为空；VectorFloat32 与其他求值类型混合；任何实际求值到的 `SqlValue::Error`。错误均以 `EvalError(String)` 返回，没有 panic 路径或错误上下文链。

短路是最重要的错误边界：`eval_case_when` 不观察首个真分支之后的值；`eval_if` 不观察未选分支；`eval_if_null` 在首值非 NULL 时不观察 fallback。相反，条件错误、IFNULL 首值错误、选中结果错误必须立即返回。独立 Rust 测试以 `SqlValue::Error("unreached")` 验证未达分支，以 condition/first 错误验证传播。

`numeric_prefix` 是宽松转换，不返回解析错误：无数字前缀和最终 `parse` 失败都得到 0。它扫描指数标记但把 `valid_end` 保持在指数之前，因此测试中的 `"1e"` 和 `"1e+"` 仍按前缀 1 为真。时间值通过移除 `-`、`:`、空格后再取前缀；JSON 只特判文本 `null`、`false`、`true`，其他 JSON 文本走数值前缀；这些是当前局部模型，不等同于完整 TiDB 类型转换/告警语义。

类型边界还包括：空字段列表返回错误而 Go `InferType4ControlFuncs` 对该内部不变量使用 panic；未知控制函数名在 `derive_charset` 中不做处理而不是报错；简化的 charset 推导只是选择首个非 binary 字符串的 charset/collation，未执行 Go 的 collation 合法性检查。扩展时不能把这些差异误当成已完全兼容。

## 并发与资源生命周期

所有入口仅使用不可变共享引用和局部拥有值，没有锁、原子、通道、异步任务、事务或外部句柄。函数本身可重入；只要输入切片在调用期间有效，就没有额外生命周期约束。返回值不借用输入，因此可独立于输入存活。

批量函数是同步串行迭代，不会并行处理行，也没有取消或背压机制。遇到首个错误时由 `Result` 收集提前结束，已分配的临时结果随栈展开正常释放。可能的资源成本主要来自每个选中值和类型字段的克隆，以及 `infer_type_for_control` 为 NULL/非 NULL 分组建立的两个 `Vec<FieldType>`。

Go 实现的 signature 注释强调跨 session 共享时新增字段必须线程安全或不可变；Rust 内核没有 signature 对象或跨会话缓存状态，所以当前不存在同类共享可变状态，但将来若加入缓存或上下文句柄，必须重新审视 `Send`/`Sync`、克隆成本和会话隔离。

## 与 Go 版本的对应关系

运行语义对应关系：`eval_case_when` 对应 Go 各 `builtinCaseWhen*Sig.eval*` 的成对扫描、NULL/0 条件跳过、首个命中返回和无 ELSE 返回 NULL；`eval_if` 对应各 `builtinIf*Sig.eval*` 的条件求值后只执行一个结果分支；`eval_if_null` 对应各 `builtinIfNull*Sig.eval*` 的首参数非 NULL/错误即返回、只有 NULL 才执行第二参数。Rust `SqlValue` 把 Go 按 EvalType 分开的签名合并成一个枚举入口。

类型语义对应关系：`max_len`、`set_flen_from_args`、`set_decimal_from_args` 分别对应 Go `maxlen`、`setFlenFromArgs`、`setDecimalFromArgs`；`infer_type_for_control` 对应 `InferType4ControlFuncs` 的 NULL 分组、聚合、标志修正、ENUM/SET 提升和 datetime flen 修正。`FieldKind`/`FieldType` 是 `mysql.Type*`/`types.FieldType` 的简化映射。

已确认的差异与迁移限制：

- Go 的 `caseWhenFunctionClass`、`ifFunctionClass`、`ifNullFunctionClass` 会验证参数、包装 `IsTrue`、构建 typed builtin、设置 PB code，并接入真实表达式注册表；Rust 文件没有这些构造与注册步骤。
- Go CASE 类型推导只传 THEN/ELSE 表达式；本文件的推导入口只接受已经筛选好的字段列表，调用者需要承担筛选责任。
- Go collation 路径通过 `CheckAndDeriveCollationFromExprs` 使用构建上下文，并对 IF/IFNULL、CASE、COALESCE 分别处理；Rust `derive_charset` 是无上下文近似模型，且把多个函数名合并处理。
- Go 对空推导参数采用内部 panic；Rust 返回 `EvalError`。Go 的参数数量错误通常在 function class 的 `verifyArgs` 阶段报告，而 Rust 只有 CASE 最小长度和向量行数检查。
- Rust 测试覆盖了 Go `builtin_control_test.go` 的主要 CASE/IF/IFNULL 表格与错误分支，并额外用 Error 占位验证短路；但不能据此宣称真实 `Expression`、告警、PB 下推或全部 collation 行为已迁移。

## 扩展指南

新增 `SqlValue` 或 `FieldKind` variant 时，必须同步检查 `FieldType::new`、`FieldType::eval_type`、`is_true`、`aggregate_eval_type` 和 `aggregate_kind` 的穷尽匹配；若新类型能参与字符串/数值归约，还要补 `set_flen_from_args`、decimal 和 charset 规则。测试应放在独立的 `pkg/expression/builtin_control_test.rs`，不要把测试内嵌到生产文件；与 Go 迁移对齐的更宽覆盖可同步更新 `builtin_control_9_aster_unit_test.rs`。

新增控制函数名或改变类型推导时，最可能的入口是 `infer_type_for_control` 与 `derive_charset`。调用者必须传入真正的结果候选，而非条件参数。应增加全 NULL、单个非 NULL、混合类型、未知 flen/decimal、flags、binary/non-binary charset、ENUM/SET、datetime fractional seconds 和 VectorFloat32 混合拒绝等独立测试，并逐项核对 Go `InferType4ControlFuncs` 与 `addCollateAndCharsetAndFlagFromArgs`。

改变求值规则时，优先修改 `is_true` 或三个标量入口，让按行入口自然复用；同时覆盖条件错误、选中结果错误、未选错误、NULL 条件、缺失 ELSE 和批量首错终止。若目标是接入生产表达式主链，还需要在本文件之外设计 `Expression`/上下文/类型转换/PB 签名桥接；那不是对当前内核增加一个调用点即可完成的局部改动。

兼容性风险集中在 MySQL 真值转换、字符串数值前缀、collation 合并、NOT NULL/UNSIGNED/BINARY 标志和 flen/decimal 上限。性能风险集中在大 String/Binary/Vector 的克隆和批量输入的串行处理；在改变所有权或引入并行前，应先保留短路顺序和首错语义。

## 验证依据

- RustCodeGraph：`status` 显示项目索引包含目标文件；`node --file pkg/expression/builtin_control.rs --offset 1 --limit 500` 与后续 `--offset 499 --limit 220` 覆盖全部 638 行、符号和内部调用；索引报告该文件被 11 个文件使用。泛化 `explore` 结果噪声较大，精确 `callers/callees` 在本次时限内未返回，因此外部直接引用另用局部文本搜索核验，未据此虚构生产调用链。
- Rust 源与装配：`pkg/expression/builtin_control.rs`；`pkg/expression/lib.rs` 第 183–184 行的 `builtin_control_kernel` 私有模块声明。
- crate 边界：`pkg/expression/Cargo.toml` 的 package 名为 `astersql-expression`、lib 入口为 `lib.rs`、`autotests = false`；本文件实际只使用 `std::fmt`。
- Rust 独立测试：`pkg/expression/builtin_control_test.rs` 覆盖 CASE 首匹配/NULL ELSE/错误、IF/IFNULL 真值与短路、类型宽度/标志/ENUM 提升、空推导参数；`pkg/expression/builtin_control_9_aster_unit_test.rs` 扩展覆盖 Go 真值表、按行 CASE、binary flag 及更多值类型。
- Go 对照：`pkg/expression/builtin_control.go` 的 `maxlen`、`setFlenFromArgs`、`setDecimalFromArgs`、`addCollateAndCharsetAndFlagFromArgs`、`InferType4ControlFuncs`、三个 function class 和各 typed `eval*`；`pkg/expression/builtin_control_test.go` 的 `TestCaseWhen`、`TestIf`、`TestIfNull`。
- 人工复核结论：该文件为何存在（隔离可测试的 Rust 控制流与类型推导内核）、如何运行（私有模块、标量入口、按行包装、推导阶段）、如何安全扩展（同步穷尽匹配、独立测试、保持短路及 Go 对照）均可由上述符号和文件反查；未将缺失的真实表达式框架接线描述为已支持。
