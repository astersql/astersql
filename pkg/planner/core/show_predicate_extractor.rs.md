# `pkg/planner/core/show_predicate_extractor.rs`

## 文件定位

本文件位于 `astersql-planner-core` crate，负责把 `SHOW ... LIKE ...` 或 `DESCRIBE ... <column>` 中可在 SHOW 结果生成阶段使用的字段条件提取为精确值或通配模式。crate 根在 `pkg/planner/core/lib.rs` 中以私有模块 `mod show_predicate_extractor` 声明它，再用 `pub use show_predicate_extractor::*` 对外重导出其公开符号。`pkg/planner/core/Cargo.toml` 表明它直接依赖 parser AST（通过 crate 根再导出）、`astersql-expression` 的 collation API 和 `astersql-util-stringutil` 的 LIKE 模式编译 API；本功能不受 `nextgen` feature 条件编译控制。

当前 Rust 接线状态必须与 Go 主链区分：RustCodeGraph 在目标源文件上只找到 `pkg/planner/core/show_predicate_extractor_test.rs` 的使用；Rust `pkg/planner/core/logical_plan_builder_runtime.rs::build_show_runtime` 当前也没有调用 `newShowBaseExtractor`。因此这是“已移植、已单测，但尚未接入 Rust SHOW 生产规划链”的实现，不应把 Go `planbuilder.go` 的已接线状态当成 Rust 现状。

## 核心职责

- `ShowBaseExtractor::Extract` 识别 AST 中的 LIKE 模式或 DESCRIBE 列名，将它归一化为小写后保存。
- 对不含通配语义的 LIKE 值，保存编译后的精确文本 `field`；对含通配语义的值，保存原始模式文本的小写形式 `fieldPattern`。
- `ExplainInfo` 把提取结果格式化为计划说明；`Field` 和 `FieldPatternLike` 分别为潜在上层消费者提供精确值与已编译通配器。
- 保留 Go `pkg/planner/core/show_predicate_extractor.go` 的数据流和 EXPLAIN key 映射，同时以 Rust 的 `Option<Box<dyn WildcardPattern>>` 表达 Go 中可空的 `collate.WildcardPattern`。

该文件只提取和暴露条件，不读取元数据、不执行 SHOW、不构造逻辑或物理计划，也不负责报告 parser 语法错误。

## 主要符号

- `fieldKey` / `tableKey` / `databaseKey` / `collationKey` / `databaseNameKey`：EXPLAIN 文本的键名，分别是 `field`、`table`、`database`、`collation` 和 `db_name`；它们都是模块私有常量。
- `pub struct ShowBaseExtractor`：提取器的持久状态。`ShowStmt: ShowStmt` 是公开的 AST 快照；`field` 和 `fieldPattern` 是私有、可变的提取结果。类型派生 `Clone`/`Debug`/`Eq`/`PartialEq`，本文件不提供 `Default`。
- `pub fn newShowBaseExtractor(showStatement: ShowStmt) -> ShowBaseExtractor`：按值接收 SHOW AST，保留它并将两个结果字符串初始化为空。返回具体类型，不像 Go 构造器那样直接返回 `base.ShowPredicateExtractor` 接口。
- `pub fn Extract(&mut self) -> bool`：主入口。`true` 表示已识别并写入一个可用条件，`false` 表示没有条件或 AST 形状不支持；它不返回错误。
- `pub fn ExplainInfo(&self) -> String`：根据 `ShowStmt.Tp` 选 key，按“精确值在前、通配模式在后”的顺序生成零到两个片段，用 `, ` 连接。
- `pub fn Field(&self) -> String`：克隆并返回 `field`，不暴露内部可变引用。
- `pub fn FieldPatternLike(&self) -> Option<Box<dyn WildcardPattern>>`：没有通配模式时返回 `None`；否则使用 utf8mb4 默认 collation 创建模式对象，以反斜杠为 escape 编译后返回 trait object。

源文件没有定义 `trait`、枚举、宏、异步函数或条件编译项。它也没有为 `ShowBaseExtractor` 实现 `pkg/planner/core/base/misc_base.rs::ShowPredicateExtractor` 或 `pkg/planner/core/operator/logicalop/logical_show.rs::ShowPredicateExtractor`。

## 执行流程

1. 调用者先用 `newShowBaseExtractor` 移入一份 `ShowStmt`，两个结果字段为空。
2. `Extract` 优先检查 `ShowStmt.Pattern`。只有当其 `ExprKind` 是 `Like` 时才继续，并从 LIKE 节点中取出 `Pattern` 和 `Escape`。
3. 若 LIKE 右侧是 `ExprKind::Value`，则取 `value.text()`，使用 `Escape` 的第一个字节；当 escape 字符串为空时回退到 `b'\\'`。
4. `stringutil_dependency::string_util::CompilePattern` 返回编译后字符和模式类型。`IsExactMatch` 逐项要求类型为 `PatMatch`：若成立，则把编译字符收集成字符串、转小写后写入 `field`；否则把未编译的模式文本转小写后写入 `fieldPattern`。两条路径都立即返回 `true`。
5. 若 LIKE 右侧是 `ExprKind::Column`，则为了对齐 MySQL 对 `SHOW COLUMNS FROM t LIKE abc` 的拒绝行为返回 `false`；其他右侧类型不写入状态，最终返回 `false`。
6. 只在 `Pattern` 整体不存在时，才尝试 DESCRIBE 分支：若 `ShowStmt.Column` 存在且 `column.Name.L` 非空，将已由 `CIStr` 保存的小写名称复制到 `field` 并返回 `true`。存在一个不可提取的 `Pattern` 时不会再回退到 `Column`。
7. 提取后，调用者可用 `ExplainInfo` 查看文本摘要，或使用 `Field` / `FieldPatternLike` 获取两种结果。`FieldPatternLike` 每次调用都新建 collator 模式对象并重新编译。

`ExplainInfo` 的 key 映射为：`Variables`/`Columns` → `field`，`Tables`/`TableStatus` → `table`，`Databases` → `database`，`Collation` → `collation`，`StatsHealthy` → `db_name`，其他类型 → 空 key。因此未列举类型若人为构造出已提取状态，文本会是 `:[value]` 或 `_pattern:[value]`；当前代码没有单独拒绝这种组合。

## 数据与状态

`ShowBaseExtractor` 拥有 `ShowStmt`，因此其生命周期不依赖 parser 临时引用。`field` 与 `fieldPattern` 使用空字符串同时表示“尚未提取”和“该类结果不存在”，成功与否要以 `Extract` 的 bool 为准。精确 LIKE 成功时只写 `field`；通配 LIKE 成功时只写 `fieldPattern`；DESCRIBE 列成功时只写 `field`。

一个重要的状态边界是：`Extract` 不会在入口清空旧结果。正常构造后单次调用与 Go 主链一致；但若外部修改公开的 `ShowStmt` 并在同一实例上重复调用，旧 `field` 和 `fieldPattern` 可能同时存在，`ExplainInfo` 会把两者都输出。这是当前 Rust/Go 共有的可变状态语义，不应默认它是幂等重算 API。

`Field` 返回字符串克隆；`FieldPatternLike` 返回新分配的 boxed trait object。除这些所有权与堆分配外，没有全局状态、缓存或外部副作用。

## 依赖与调用关系

上游与装配：

- `pkg/planner/core/lib.rs` 声明模块并重导出其公开项；同一文件用 `#[cfg(test)] mod show_predicate_extractor_test` 挂载独立测试。
- RustCodeGraph 对 `newShowBaseExtractor` 的调用边只列出 `extracts_exact_and_wildcard_patterns_like_go`、`rejects_column_pattern_and_extracts_describe_column` 和 `explain_keys_and_empty_state_match_go`，三者都在 `pkg/planner/core/show_predicate_extractor_test.rs`。
- Go 生产对照链是 `pkg/planner/core/planbuilder.go` 中的 SHOW 计划构建分支 → `newShowBaseExtractor` → `Extract` → 成功时赋给 `LogicalShow.Extractor` 并禁用后续通用 pattern 构建。此边目前不存在于 Rust `build_show_runtime`。

下游：

- `crate::ast::{ShowStmt, ShowStmtType, ExprKind}` 提供 SHOW 类型、Pattern/Column 载荷和 LIKE 表达式形状。RustCodeGraph 确认 Rust `ShowStmt` 定义在 `pkg/parser/ast/lib.rs`，其 `Pattern`/`Column` 都是 `Option`。
- `stringutil_dependency::string_util::{CompilePattern, IsExactMatch}` 负责 escape/通配符解析和“是否全为普通字符”判定；RustCodeGraph 定位其 Rust 实现于 `pkg/util/stringutil/string_util.rs`。
- `expression_dependency::collate::{CollationName2ID, GetCollatorByID, mysql::UTF8MB4DefaultCollation}` 选择默认 utf8mb4 collator，`Collator::Pattern` 创建通配器，`WildcardPattern::Compile` 编译已存模式。`GetCollatorByID` 会根据新 collation 开关选择具体实现，找不到 collation 时回退到 binary-padding collator。

存在两个相关但当前未打通的 Rust 接口：`pkg/planner/core/base/misc_base.rs::ShowPredicateExtractor` 使用 snake_case 方法且通配器返回非可选 trait object；`pkg/planner/core/operator/logicalop/logical_show.rs::ShowPredicateExtractor` 要求 `CloneBox`、`Extract(&self)` 并以 `Option<String>` 表示模式。`ShowBaseExtractor` 没有实现任一接口，所以它现在不能直接存入 `LogicalShow.Extractor` 或 `PhysicalShow.Extractor`。

## 错误处理与边界

- API 不返回 `Result`。“无 Pattern”、“空 DESCRIBE 列名”、“Pattern 不是 LIKE”、“LIKE 右侧不是 Value”等都收敛为 `Extract() == false`。
- LIKE 右侧为 `ExprKind::Column` 时显式返回 `false`，注释表明真正的 MySQL 语法错误应由上游解析/规划边界处理，提取器本身不生成错误对象。
- `Escape` 按 UTF-8 字节串的首字节取值，多字节 escape 不会作为完整 Unicode 字符传给 `CompilePattern`；空值回退到反斜杠。
- `FieldPatternLike` 固定以 `b'\\'` 重新编译，不保存也不重用 AST LIKE 节点原始 `Escape`。这与 Go 对照实现一致，但扩展自定义 escape 语义时必须同时审查提取和二次编译两处。
- `ExplainInfo` 对未映射的 `ShowStmtType` 不报错，只使用空 key；调用方应限制可构造提取器的 SHOW 类型，或扩展此映射。
- 空模式的精确匹配可能使 `Extract` 返回 `true` 但 `field` 仍为空；后续 `ExplainInfo` 和 `FieldPatternLike` 都无法单独区分它与未提取状态，所以不能丢弃 `Extract` 的 bool。

## 并发与资源生命周期

该实现是同步的，不创建线程、异步任务、锁、通道、事务或 I/O 资源。`Extract(&mut self)` 的独占借用保证单次状态更新期间不会有其他 Rust 安全引用同时访问该实例；类型本身未提供内部同步，跨线程共享如有需要应由调用者包装。

`ShowStmt`、`field` 和 `fieldPattern` 与提取器共同存活。`FieldPatternLike` 中的 collator 和 pattern 是局部所有权对象：pattern 移入 `Box<dyn WildcardPattern>` 后交给调用者，调用者丢弃 box 时自动释放。方法每次都重新分配和编译，若未来在热路径中频繁调用，需要以性能测量决定是否缓存，不能在没有生命周期设计时盲目引入共享可变状态。

## 与 Go 版本的对应关系

Rust `ShowBaseExtractor`、五个 key 常量及四个操作方法直接对应 `pkg/planner/core/show_predicate_extractor.go`。核心分支保持一致：优先 LIKE，Value 根据 `IsExactMatch` 分成精确/通配状态，ColumnName 模式拒绝，没有 Pattern 时处理 DESCRIBE 列；EXPLAIN key 表与输出顺序也一致。`pkg/planner/core/show_predicate_extractor_test.rs` 的三个测试明确覆盖这些移植契约。

需要注意的 Rust 形态差异：

- Go 通过嵌入 `ast.ShowStmt` 暴露其字段，Rust 使用显式公开字段 `ShowStmt`。
- Go 构造器返回 `base.ShowPredicateExtractor` interface；Rust 返回具体类型，且未实现现有 Rust 同名 trait。
- Go `Field()` 返回字符串值，Rust 也返回拥有的 `String`，但通过克隆生成；Go 的 nil wildcard 在 Rust 中是 `None`。
- Rust 对空 `Escape` 显式回退到反斜杠；Go AST 字段是单字节，直接传给 `CompilePattern`。
- Go 的生产 `planbuilder.go` 已为多种 SHOW 和部分统计 SHOW 构造提取器；Rust `build_show_runtime` 当前只构造 `LogicalShow`、处理 `WHERE` 和添加投影，未调用本文件。

以 `rg` 搜索 `pkg/planner/core/*_test.go` 未找到直接针对 `ShowBaseExtractor` 的 Go 单元测试；本文档对 Go 行为的依据是 Go 实现本身及 `planbuilder.go` 的生产调用点，不宣称有未找到的 Go 专项测试覆盖。

## 扩展指南

- 新增可提取的 `ShowStmtType` 时，首先更新 `ExplainInfo` 的 key 映射，并在 `pkg/planner/core/show_predicate_extractor_test.rs` 的 `explain_keys_and_empty_state_match_go` 表驱动用例中添加该类型；同时与 Go 对照实现和构造点核对兼容性。
- 扩展支持新的 Pattern AST 形状时，修改 `Extract` 的 `ExprKind` 匹配，为成功状态、拒绝状态和不可识别状态分别添加独立测试；不要把 parser 应该产生的语法错误静默转成一个可下推条件。
- 若要支持自定义 escape，需在状态中保留 escape，并同时修改 `Extract` 和 `FieldPatternLike`；测试应包含空 escape、反斜杠、自定义 ASCII escape 及多字节输入的边界。
- 若要把本实现接入 Rust SHOW 主链，不应只在 `build_show_runtime` 加一个构造调用；必须先统一 `base::ShowPredicateExtractor`、`logicalop::ShowPredicateExtractor` 与本具体类型的方法可变性、克隆契约和 wildcard 返回类型，再验证 `LogicalShow` 到 `PhysicalShow` 的传递、EXPLAIN 输出及执行端消费。这是兼容性与正确性风险最高的扩展点。
- 若要让提取器可重用，应先明确“每次 `Extract` 清空旧状态”的新契约，同步修改 Go 语义或记录差异，并增加精确→通配、通配→精确、成功→失败的重复调用测试。
- 性能调整应关注 `Field()` 的克隆和 `FieldPatternLike()` 的重复分配/编译，但必须保留 collator 选择、模式所有权和线程安全契约；没有基准数据时不建议引入缓存。

Rust 测试必须继续放在独立的 `pkg/planner/core/show_predicate_extractor_test.rs`，不应嵌入生产源文件。

## 验证依据

- 目标实现：`pkg/planner/core/show_predicate_extractor.rs`，通读了 119 行全文，确认常量、结构体、构造函数、四个 inherent method 以及无条件编译项。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/planner/core/show_predicate_extractor.rs --offset 1 --limit 500` 返回完整源文件；`query ShowPredicateExtractor`、`query newShowBaseExtractor`、`node newShowBaseExtractor`、`node ShowStmt`、`node CompilePattern`、`node IsExactMatch` 和 `node GetCollatorByID` 用于核对符号、定义与下游语义。
- 调用边：RustCodeGraph `node newShowBaseExtractor` 列出三个 Rust 测试调用者；搜索 Rust 生产文件没有发现其他 `newShowBaseExtractor`/`ShowBaseExtractor` 用法。Go 生产边则由 `pkg/planner/core/planbuilder.go` 中的三处 `newShowBaseExtractor` 构造点验证。
- crate 与装配：`pkg/planner/core/Cargo.toml` 验证 crate 名、`autotests = false`、依赖与 feature；`pkg/planner/core/lib.rs` 验证模块声明、独立测试挂载和公开重导出。
- Go 对照：`pkg/planner/core/show_predicate_extractor.go` 验证字段、分支、EXPLAIN 格式和 wildcard 编译；`pkg/planner/core/planbuilder.go` 验证 Go 主链的构造与挂载。
- Rust 上下游：`pkg/parser/ast/lib.rs::ShowStmt`、`pkg/util/stringutil/string_util.rs::{CompilePattern, IsExactMatch}`、`pkg/util/collate/collate.rs::GetCollatorByID`、`pkg/planner/core/logical_plan_builder_runtime.rs::build_show_runtime`、`pkg/planner/core/operator/logicalop/logical_show.rs` 和 `pkg/planner/core/operator/physicalop/physical_show.rs` 用于验证 AST、模式编译、collation 选择、当前未接线状态及接口差异。
- 独立 Rust 测试：`pkg/planner/core/show_predicate_extractor_test.rs` 验证精确值小写化、通配模式与匹配结果、Column 模式拒绝、DESCRIBE 列提取、七种 EXPLAIN key 及空状态。本任务按计划不运行 Cargo，因此只把测试源码作为行为证据，不声称本轮实际执行了测试。
