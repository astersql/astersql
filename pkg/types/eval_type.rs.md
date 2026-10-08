# `pkg/types/eval_type.rs`

## 文件定位

本文件是 `pkg/types` 元数据分组中的 **EvalType 兼容门面**，不是求值类型的真实实现。`pkg/types/internal/metadata/lib.rs` 在 `eval_type_defs` 私有模块中通过 `include!("../../eval_type.rs")` 编译本文件，并以 `pub use eval_type_defs::*` 将其 API 暴露为 `astersql-types-metadata` 的公开符号；根 crate `pkg/types/lib.rs` 又以 `pub use types_group_4 as metadata` 暴露该分组。该装配方式使 Rust 调用方能够沿用 Go `pkg/types` 的类型名称，同时避免在 types 层复制 parser 层实现。

目标文件仅含一个公开类型别名和九个公开常量，没有函数、trait、`impl`、条件编译项或私有运行时逻辑。真实定义位于 `pkg/parser/types/eval_type.rs`，并由 metadata crate 的 `ast_types` 模块（`pub use parser_types::types::*`）以局部名 `ast` 提供给本文件。

## 核心职责

1. `pub type EvalType = ast::EvalType` 保持 `pkg/types` API 与 parser 类型系统使用同一个 Rust 类型；它不创建新类型，因此 parser 上的 `IsStringKind`、`IsVectorKind`、`String`、`Display`、`Copy`、`Eq`、`Hash` 等能力原样可用。
2. `ETInt` 至 `ETVectorFloat32` 将 parser 层九个规范常量逐一转发到 types 元数据门面，保证比较、模式匹配和类型推断使用同一组值。
3. 与 `pkg/types/eval_type.go` 保持同路径接口形状：Go 文件同样把 `EvalType` 和全部 `ET*` 常量转发到 `pkg/parser/types`，因此本文件的职责是兼容导出，而非重新定义分类规则。

## 主要符号

- `EvalType`：`ast::EvalType` 的公开类型别名。底层真实类型是 `pkg/parser/types/eval_type.rs` 中的 `pub struct EvalType(pub u8)`；使用别名意味着不存在 types/parser 两套值之间的转换或所有权边界。
- `ETInt`、`ETReal`、`ETDecimal`：数值求值类别，分别转发 `ast::ETInt`、`ast::ETReal`、`ast::ETDecimal`。
- `ETString`：字符串求值类别。
- `ETDatetime`、`ETTimestamp`、`ETDuration`：日期时间相关求值类别；parser 实现把这些类别纳入 `IsStringKind`。
- `ETJson`：JSON 求值类别；parser 实现也把它纳入 `IsStringKind`。
- `ETVectorFloat32`：float32 向量求值类别；parser 实现同时令其满足 `IsStringKind` 和 `IsVectorKind`。

九个常量的规范数值由 parser 文件维护，按 Go `iota` 顺序为 0 到 8。本文件没有数值字面量，只引用规范常量，因此不会自行引入编号漂移。

## 执行流程

该文件没有可调用的运行时流程；其作用发生在编译和名称解析阶段：

1. `pkg/types/internal/metadata/lib.rs` 先把 `parser_types::types::*` 暴露为 `ast_types`。
2. `eval_type_defs` 使用 `crate::ast_types as ast` 建立本文件所需的局部路径，然后通过 `include!` 展开本文件。
3. 类型别名把 `EvalType` 解析到 parser 的真实类型；每个 `ET*` 常量初始化为相应的 parser 常量。
4. `pub use eval_type_defs::*` 向 metadata crate 使用者公开这些名称；上层可经 `types_group_4` 或 `astersql_types::metadata` 使用。
5. 后续方法调用和比较直接在 parser `EvalType` 上执行。例如 `pkg/types/field_type.rs::AggregateEvalType` 使用 `ETString` 初始化结果、比较 `ETReal`/`ETDecimal`/`ETInt`，并调用 `IsStringKind`；这里没有门面层分派。

## 数据与状态

本文件不持有全局可变状态、缓存、集合或堆资源。`EvalType` 的真实表示是 `u8` 新类型包装；转发常量均为可在编译期求值的 `const` 值。别名不会产生额外布局，常量也不会产生独立的可变实例。

应保持的关键不变量是：每个 types 常量必须与同名 `ast` 常量完全相等，且公开集合和顺序须与 Go 门面一致。`pkg/types/enum_4_aster_unit_test.rs::eval_types_and_explain_formats_preserve_aliases_values_and_order` 已直接断言 `ETInt`、`ETVectorFloat32` 与 parser 常量相等，并验证分类方法可通过别名使用；更完整的 0..8 名称与分类行为由 `pkg/parser/types/migration_aster_unit_test.rs` 验证。

## 依赖与调用关系

- 直接定义依赖：本文件只依赖 include 上下文提供的 `ast` 名称；该名称在 `pkg/types/internal/metadata/lib.rs` 中指向 `crate::ast_types`，最终来自 `parser_types::types`。
- Cargo 边界：`pkg/types/Cargo.toml` 将内部 metadata crate 作为 `types-group-4` 依赖；`pkg/types/internal/metadata/Cargo.toml` 则以路径依赖 `astersql-parser-types`，这条依赖提供真实 `EvalType`。
- 装配上游：`pkg/types/internal/metadata/lib.rs::eval_type_defs` 是本文件的直接编译入口，随后由该文件的 glob 再导出公开。
- 典型使用者：`pkg/types/field_type.rs::AggregateEvalType` 和 `mergeEvalType` 消费这些类别完成字段求值类型聚合；`pkg/types/internal/field/lib.rs` 也直接从 `parser_types::types` 再导出同一组符号，说明字段子 crate 与 metadata 门面共享同一规范类型，而非相互转换。
- RustCodeGraph 对 `pkg/types/eval_type.rs` 识别为 41 行、一个主要符号的文件，但由于 `include!` 展开和类型别名，未给出可靠的目标文件 callers/callees 边；上述调用关系因此由模块装配、Cargo 路径与精确引用共同核验，而不是臆测图边。

## 错误处理与边界

本文件自身没有 `Result`、错误构造、panic 分支或输入验证。它的边界是“只转发已存在的类型和值”：非法 `EvalType` 数值如何处理由 parser 实现决定。当前 `pkg/parser/types/eval_type.rs` 允许构造任意 `u8` 包装值，以保留 Go byte 别名语义；将未知值格式化时，`Display`/`String` 会 panic。该 panic 不在本文件内，也不应在此门面重复实现。

分类边界同样由 parser 维护：`IsStringKind` 包含字符串、三类时间、JSON 和 VectorFloat32，`IsVectorKind` 当前只匹配 VectorFloat32。若新增类别却只在本文件增加常量，不同步 parser 的方法和 Go 定义，会造成行为不完整。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道、事务、文件句柄或网络资源。所有符号都是类型级/常量级转发，生命周期等同于编译产物；`EvalType` 是 `Copy` 值，传递时没有共享所有权协调需求。并发安全性完全来自底层不可变小值的性质，本文件没有额外同步协议。

## 与 Go 版本的对应关系

`pkg/types/eval_type.go` 是本文件的直接 Go 对照：它将 `type EvalType = ast.EvalType`，并逐项把 `ETInt`、`ETReal`、`ETDecimal`、`ETString`、`ETDatetime`、`ETTimestamp`、`ETDuration`、`ETJson`、`ETVectorFloat32` 绑定到 `ast`。Rust 门面的别名和九个常量与之逐项一致。

实际方法位于两种语言各自的 parser 文件：Go 的 `pkg/parser/types/eval_type.go` 使用 `type EvalType byte`、`iota` 常量和三个方法；Rust 的 `pkg/parser/types/eval_type.rs` 使用 `EvalType(pub u8)` 新类型、0..8 常量、两个分类方法以及 `String`/`Display`。Rust 新类型用于表达类型约束，但保留未知 `u8` 可构造以及格式化非法值 panic 的 Go 行为。本门面不复刻方法，依靠类型别名把底层方法带到 types API。

## 扩展指南

新增求值类别时，不能只修改本文件。安全顺序是：

1. 在 Go `pkg/parser/types/eval_type.go` 与 Rust `pkg/parser/types/eval_type.rs` 增加规范常量，并同步字符串/类别判断；保持既有数值稳定，避免协议或持久化兼容风险。
2. 在 Go `pkg/types/eval_type.go` 和本文件添加同名转发，确保两侧门面集合一致。
3. 更新独立测试：parser 行为应扩展 `pkg/parser/types/migration_aster_unit_test.rs`（以及相邻 parser 测试）；门面别名应扩展 `pkg/types/enum_4_aster_unit_test.rs`。不要把测试嵌入本生产文件。
4. 检查 `pkg/types/field_type.rs::AggregateEvalType`、`mergeEvalType` 及字段类型到 EvalType 的映射是否需要新分支。兼容性风险主要是常量编号变化和遗漏分类分支；性能风险很低，因为门面没有运行时开销，但下游新增分支可能影响热路径模式匹配。

若只是修改说明或注释，不应改变 `include!` 装配、别名关系或常量值。若计划把门面改为独立 enum/struct，则会引入转换、布局和 API 兼容问题，已超出本文件当前设计。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/types/eval_type.rs` 确认目标已索引；`node --file pkg/types/eval_type.rs --offset 1 --limit 400` 确认文件全貌为一个别名和九个常量；`node --file pkg/parser/types/eval_type.rs --offset 1 --limit 140` 核对真实表示、编号、分类和格式化行为。图查询对 include 门面没有产出可靠调用边，此限制已在“依赖与调用关系”中说明。
- Rust 源与装配：`pkg/types/eval_type.rs`、`pkg/types/internal/metadata/lib.rs`、`pkg/types/lib.rs`、`pkg/parser/types/eval_type.rs`、`pkg/types/field_type.rs`、`pkg/types/internal/field/lib.rs`。
- Cargo：`pkg/types/Cargo.toml`、`pkg/types/internal/metadata/Cargo.toml`，用于确认 `types-group-4` 与 `parser-types` 的 crate 边界和路径依赖。
- Go 对照：`pkg/types/eval_type.go`（门面）和 `pkg/parser/types/eval_type.go`（真实定义与方法）。
- 独立测试：`pkg/types/enum_4_aster_unit_test.rs::eval_types_and_explain_formats_preserve_aliases_values_and_order`；`pkg/parser/types/migration_aster_unit_test.rs` 中的 EvalType 名称、`IsStringKind`、`IsVectorKind` 断言；聚合使用边界另见 `pkg/types/field_type_test.rs::TestAggregateEvalType` 与 `pkg/types/field_type_5_aster_unit_test.rs::aggregate_eval_type_matches_string_numeric_and_mixed_sign_rules`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求文档存在且恰有上述 11 个固定二级标题。
