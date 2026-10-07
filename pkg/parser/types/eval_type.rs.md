# `pkg/parser/types/eval_type.rs`

## 文件定位

`eval_type.rs` 位于 `astersql-parser-types` crate 的类型子系统中。crate 根 `pkg/parser/types/lib.rs` 通过 `types::eval_type` 模块 `include!("eval_type.rs")`，随后先在 `types` 内、再在 crate 根公开再导出该文件的全部公开项。因此，下游既可经 `parser_types::types::EvalType`，也可经 crate 根再导出使用这些定义。

这个文件定义的不是 MySQL 列的物理类型码，而是表达式系统使用的较粗粒度求值类别。典型入口是相邻文件 `pkg/parser/types/field_type.rs` 的 `FieldType::EvalType`：它把 `TypeTiny`、`TypeDouble`、`TypeDatetime`、`TypeJSON` 等存储类型折叠成这里的九类结果。之后，规划、类型聚合和表达式执行代码根据该结果选择强制转换或具体的 `Eval*`/`VecEval*` 路径，例如 `pkg/planner/core/expression_rewriter.rs` 的转换分支与 `pkg/expression/chunk_executor.rs::evalOneVec` 的向量化分派。

`pkg/parser/types/Cargo.toml` 将本目录声明为包 `astersql-parser-types`，库入口为 `lib.rs`，且没有为本文件设置条件 feature；本文件自身唯一的直接依赖是标准库 `std::fmt`。

## 核心职责

1. 用 `EvalType(pub u8)` 表示可复制、可比较、可哈希的求值类别，同时保留 Go `type EvalType byte` 能承载未知字节值的性质。
2. 用 `ETInt` 到 `ETVectorFloat32` 九个公开常量固定类别编号；编号顺序与 Go 的 `iota` 顺序一致，取值为 `0..=8`。
3. 用 `EvalType::IsStringKind` 提供“走字符串形态规则”的跨类型分类。该集合不仅有 `ETString`，还包含日期时间、时长、JSON 和向量。
4. 用 `EvalType::IsVectorKind` 隔离向量类别；当前只有 `ETVectorFloat32` 命中。
5. 用 `fmt::Display` 和兼容 Go 命名的 `EvalType::String` 提供稳定英文名称，并对非法类别立即 panic，避免把未知值静默显示成合法类型。

本文件不负责从 `FieldType` 推导类别、不保存列元数据，也不执行表达式；这些职责分别位于 `field_type.rs` 和表达式相关 crate 中。

## 主要符号

- `pub struct EvalType(pub u8)`：公开 tuple newtype。派生 `Clone`、`Copy`、`Debug`、`Eq`、`PartialEq`、`Hash`，可作为轻量值传递、模式匹配和哈希键。内部字节公开，因此调用者可以构造 `EvalType(9)` 之类的未知值；这正是格式化路径必须保留非法值检查的原因。
- `ETInt = EvalType(0)`：整数求值，包括 `FieldType::EvalType` 映射的整数、BIT、YEAR，以及带 `EnumSetAsIntFlag` 的 ENUM/SET。
- `ETReal = EvalType(1)` 与 `ETDecimal = EvalType(2)`：分别代表双精度浮点形态和定点十进制形态。
- `ETString = EvalType(3)`：字符串形态，也是 `FieldType::EvalType` 对未被更具体分支识别的类型的默认结果。
- `ETDatetime = EvalType(4)`、`ETTimestamp = EvalType(5)`、`ETDuration = EvalType(6)`：区分无时区日期时间、时间戳和 MySQL TIME/时长求值；显示名中 `ETDuration` 对应 `"Time"`。
- `ETJson = EvalType(7)` 与 `ETVectorFloat32 = EvalType(8)`：JSON 和 Float32 向量的专用类别。
- `EvalType::IsStringKind(self) -> bool`：对 `ETString | ETDatetime | ETTimestamp | ETDuration | ETJson | ETVectorFloat32` 返回 `true`，对整数、实数、小数以及任何未知值返回 `false`。
- `EvalType::IsVectorKind(self) -> bool`：仅比较 `self == ETVectorFloat32`；未知值也返回 `false`。
- `EvalType::String(self) -> String`：调用标准 `ToString::to_string`，实际复用本文件的 `Display` 映射，并返回拥有所有权的字符串。
- `impl fmt::Display for EvalType`：将九个合法常量映射到 `Int`、`Real`、`Decimal`、`String`、`Datetime`、`Timestamp`、`Time`、`Json`、`VectorFloat32`；其余 `EvalType(value)` 执行 `panic!("invalid EvalType {value}")`。

本文件没有模块级可变变量、trait 定义、泛型函数或条件编译项。

## 执行流程

求值类别在主链上的典型流动如下：

1. 上游持有 `FieldType`。`pkg/parser/types/field_type.rs::FieldType::EvalType` 根据 MySQL 类型码和 `EnumSetAsIntFlag` 返回本文件的一个常量；未匹配的类型落到 `ETString`。
2. 类型推断或规划代码消费该分类。`pkg/types/field_type.rs::mergeEvalType` 用 `IsStringKind` 判断任一输入是否需要合并为字符串形态；`pkg/planner/core/expression_rewriter.rs` 在处理字符串分支时，用同一方法避免给日期时间、JSON 或向量重复包裹字符串 CAST。
3. 表达式执行阶段按精确类别分派。`pkg/expression/chunk_executor.rs::evalOneVec` 将整数、实数、小数、时间、时长、JSON、向量和字符串分别送往对应的 `VecEval*` 方法，其中 `ETDatetime | ETTimestamp` 共享 `VecEvalTime`，但仍保留两个类别供前序语义判断使用。
4. 少数边界逻辑使用专门谓词。`pkg/table/column.rs::Column::CheckNotNull` 用 `IsVectorKind` 为非空向量列生成专用错误信息。
5. 诊断或测试需要名称时，调用 `String` 或标准格式化。合法值获得固定文本；未知值进入 `Display` 的兜底分支并 panic。

`IsStringKind` 是语义分组，不表示这些类别会丢失自身身份：执行器仍按 `ETDatetime`、`ETJson`、`ETVectorFloat32` 等精确常量选择不同求值接口。

## 数据与状态

`EvalType` 的全部实例状态只有一个 `u8`。九个类别以编译期常量存在，不分配内存、不引用外部对象，也没有初始化顺序要求。`Copy` 语义使参数按值传递不会转移所有权；`Eq`/`Hash` 由底层字节决定。

关键不变量是数值与顺序稳定：`0..=8` 分别对应 Go 的九个 `iota` 常量。这个编号可能越过 crate 边界被比较或保存在结构中，因此不能为了“整理”而重排。另一方面，`EvalType` 并未限制值域；`EvalType(255)` 可以构造、比较并执行两个分类谓词，只在名称格式化时被拒绝。

`String` 为每次成功调用创建一个新的 `String`；`Display::fmt` 本身只选择静态字符串并写入调用者提供的 formatter。分类方法只有固定数量的相等比较/模式匹配，时间和额外空间开销均为常数级。

## 依赖与调用关系

直接下游依赖只有 `std::fmt`：`Display` 使用 `fmt::Formatter`、`fmt::Result` 和 `write_str`。crate 级依赖虽然由 `pkg/parser/types/Cargo.toml` 声明了 errors、charset、format、mysql、terror、util、serde 等，但 `eval_type.rs` 不直接引用它们。

主要上游与消费者包括：

- `pkg/parser/types/lib.rs`：装配并公开再导出模块，是 API 暴露边界。
- `pkg/parser/types/field_type.rs::FieldType::EvalType`：把物理/声明类型映射到这些求值常量，是最靠近定义的生产者；其 `Equal`、`PartialEqual` 也用结果调整类型比较规则。
- `pkg/types/field_type.rs::mergeEvalType`：通过 `IsStringKind` 聚合多个字段的计算类别，之后按 Real、Decimal、Int 的优先级继续合并。
- `pkg/planner/core/expression_rewriter.rs`：在表达式重写和 CAST 选择中使用精确常量与 `IsStringKind`。
- `pkg/expression/chunk_executor.rs::evalOneVec`：把类别映射到具体向量化求值接口，是从分类进入运行时执行路径的直接证据。
- `pkg/table/column.rs::Column::CheckNotNull`：通过 `IsVectorKind` 区分向量列错误语义。

RustCodeGraph 对文件节点报告的直接使用文件包括 `pkg/parser/types/field_type_test.rs`、`pkg/parser/types/migration_aster_unit_test.rs` 和 `pkg/planner/core/expression_rewriter.rs`；由于该文件由 `include!` 装配，且索引未解析大写方法名的 callers/callees，跨 crate 消费关系还使用上述精确源码引用进行了补充核验。

## 错误处理与边界

两个分类谓词均为总函数：所有 `u8` 值都有确定布尔结果，未知值不会报错。`String`/`Display` 则只接受九个已知值，未知值会 panic，且消息包含十进制底层数值。这与 Go `String` 的 default 分支一致，而不是可恢复的 `Result`；因此不得对不可信字节直接调用格式化，除非上游已验证范围或允许进程因内部不变量破坏而失败。

`fmt::Result` 仍会传播 formatter 写入错误：合法类别最终调用 `f.write_str(name)`，不会吞掉底层格式化错误。非法值在写入前 panic。

边界上需特别注意：

- `ETDuration` 的稳定显示文本是 `Time`，不是常量名中的 `Duration`。
- `IsStringKind` 包含 `ETVectorFloat32`，与 Go 当前实现一致；不能把它误解为只测试 `ETString`。
- `ETDatetime` 与 `ETTimestamp` 是不同值，即使部分执行路径共享时间求值函数。
- `EvalType` 公开构造器是有意保留的 Go 兼容能力，不能假设类型系统已排除未知值。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或 I/O 资源。`EvalType` 是不可变的 `Copy` 值；所有常量都是编译期值，分类和格式化过程中也不修改共享状态。因此并发安全性来自无共享可变状态，而不是显式同步。

生命周期方面，分类调用仅覆盖函数栈帧；`Display` 借用 formatter 到单次调用结束，`String` 返回的新字符串由调用者拥有。唯一可能的资源行为是 `String` 的小额堆分配；直接使用 `{value}`/`format_args!` 走 `Display` 时由外层格式化目标决定存储。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/parser/types/eval_type.go`。Rust 实现保持了以下语义：

- Go `type EvalType byte` 对应 Rust `EvalType(pub u8)`，两者都允许表示九个已知常量之外的字节。
- Go `iota` 常量顺序与 Rust 显式 `0..=8` 数值完全一致。
- `IsStringKind` 和 `IsVectorKind` 的成员集合一致，包括 VectorFloat32 同时属于 string-kind 与 vector-kind。
- Go `fmt.Stringer` 的 `String() string` 由 Rust 的 `Display` 承担标准格式化职责，同时额外保留同名 `String() -> String` 供机械移植代码调用。
- 九个显示名称逐项一致，非法值也都 panic 并包含数值。

Rust 的主要实现差异是使用 newtype 而非类型别名，以获得独立的 trait 实现；`String` 返回拥有所有权的 `String`，Go 返回语言内建字符串。Rust 还派生了 `Debug`、相等比较和哈希能力。当前实现不是桩或未接线门面：`FieldType::EvalType`、类型聚合、规划重写和执行分派均在生产 Rust 路径中消费这些定义。

Go 同目录没有针对 `eval_type.go` 的独立测试文件；Rust 的直接移植对照覆盖位于独立文件 `pkg/parser/types/migration_aster_unit_test.rs::type_names_and_eval_types_match_go`，它逐项验证九个名称，并检查 VectorFloat32 的两个分类谓词和 `ETInt` 的非字符串分类。

## 扩展指南

新增求值类别时，应把它视为跨层协议修改，而不只是追加一个常量：

1. 在 `eval_type.rs` 末尾追加新数值，避免重排已有编号；同步 `Display` 名称，并明确它是否属于 `IsStringKind` 或 `IsVectorKind`。
2. 同步 Go `pkg/parser/types/eval_type.go` 的常量、分类方法和 `String`，保持编号与文本一致。
3. 更新 `pkg/parser/types/field_type.rs::FieldType::EvalType` 及 Go 对照，使相应 MySQL 类型能产生新类别；检查 ENUM/SET 标志和默认 `ETString` 分支是否会掩盖新类型。
4. 检查所有对现有九类做穷举分派的位置，尤其是 `pkg/expression/chunk_executor.rs::evalOneVec`、标量函数/内建函数选择、CAST 构建和类型聚合。新增值若落入通配分支，可能编译成功却在运行时采用错误语义。
5. 在独立测试文件中同步测试。最低限度应扩展 `pkg/parser/types/migration_aster_unit_test.rs` 的名称与分类矩阵；若边界增多，宜新建并由 `lib.rs` 注册独立的 `eval_type_test.rs`，不要把测试嵌入生产源文件。还应为实际消费者补充针对分派或类型合并的回归测试。

兼容风险主要是编号漂移、显示文本变化以及 string-kind 集合变化；它们会影响 Go/Rust 对齐、诊断输出、CAST 决策和类型合并。性能风险较低，但新增类别若在执行层退回字符串通路，可能引入错误转换或额外分配。设计新类别时还要决定未知值继续 panic 的契约是否保持；不要只在 `Display` 中提供名称而遗漏生产者和执行器。

## 验证依据

本说明依据以下代码与查询结果编写：

- RustCodeGraph `status`：索引覆盖当前仓库，包含 `pkg/parser/types/eval_type.rs`；文件节点完整显示该文件 91 行源码。
- RustCodeGraph `node --file pkg/parser/types/eval_type.rs --offset 1 --limit 260`：核对 `EvalType`、九个常量、三个方法/实现及非法值 panic。
- RustCodeGraph `query EvalType --limit 20`：定位本文件结构体、`field_type.rs::FieldType::EvalType` 和表达式侧消费点。
- RustCodeGraph 文件节点：读取 `pkg/parser/types/field_type.rs` 第 250 行附近的映射与比较逻辑、`pkg/types/field_type.rs` 第 130 行附近的类型合并、`pkg/expression/chunk_executor.rs` 第 90 行附近的求值分派、`pkg/planner/core/expression_rewriter.rs` 第 4500 行附近的 CAST 分支，以及 `pkg/table/column.rs` 第 985 行附近的向量非空检查。
- `pkg/parser/types/Cargo.toml` 与 `pkg/parser/types/lib.rs`：核对 crate 名称、库入口、无条件模块装配、公开再导出和独立测试注册。
- `pkg/parser/types/eval_type.go` 与 `pkg/parser/types/field_type.go`：核对 Go 常量顺序、分类集合、显示/失败行为和字段类型映射。
- `pkg/parser/types/migration_aster_unit_test.rs::type_names_and_eval_types_match_go`：核对现有 Rust 测试对九个名称及关键分类边界的覆盖。`pkg/parser/types/field_type_test.rs` 未直接测试本文件方法，主要覆盖消费 `EvalType` 的字段类型行为。
- 在 RustCodeGraph 无法返回大写 impl 方法调用边后，使用 `rg` 精确检索 `.IsStringKind()`、`.IsVectorKind()` 和 `types::ET*`，确认前述生产消费者；未用这些文本结果推断未读取的行为。

本任务是纯文档分析，按计划未运行 Cargo。结构校验应确认文件存在且恰有本文这十一个固定二级标题；人工复核重点是每项行为都能回指上述定义、生产调用点、Go 对照或独立测试，而没有把预期设计写成当前事实。
