# `pkg/planner/core/schema_table_key.rs`

## 文件定位

本文件属于 `astersql-planner-core` crate，crate 根由 `pkg/planner/core/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定。`pkg/planner/core/lib.rs` 以私有模块 `mod schema_table_key;` 装配本文件，再通过 `pub use schema_table_key::*;` 将其中的公开类型和构造函数重导出到 crate 根；同目录的 `schema_table_key_test.rs` 只在 `#[cfg(test)]` 下装配。

它提供 SQL 标识符的规范化字符串以及两类可哈希查找键。当前 Rust 生产代码中，`CIString` 被 `util.rs` 和 `common_plans.rs` 等模块使用；RustCodeGraph 与精确引用搜索没有发现三个键构造函数的生产调用者，它们目前只由 `schema_table_key_test.rs` 直接验证。因此，本文件是 Go 规划器同名键语义的已实现移植基础，但不能据此声称 Rust 规划器已经接通 Go 端的别名检查、锁目标绑定或视图递归检测流程。

## 核心职责

- `CIString` 同时保存标识符原文 `O` 与规范化小写值 `L`，让调用者既能保留展示文本，又能用统一文本构造查找键。
- `SchemaTableKey` 用规范化后的 schema 名和 table 名表达二元表身份。
- `TableAliasKey` 用规范化后的 schema、名称和独立的 `qualified` 位表达表别名身份，确保“不带 schema”与“显式带空 schema”仍是不同的键。
- 三个构造函数集中执行从 `CIString` 到键的转换，避免调用者误把保留大小写的 `O` 字段用于相等比较或哈希。

以上职责来自 `CIString::New`、`newSchemaTableKey`、`newTableAliasKey`、`newQualifiedTableAliasKey` 的字段赋值，以及三种结构体派生的 `Eq`、`PartialEq`、`Hash`。

## 主要符号

- `pub struct CIString { pub O: String, pub L: String }`：大小写不敏感字符串载体。`O` 保存输入原文；`L` 保存 `String::to_lowercase()` 的结果。它派生 `Clone`、`Debug`、`Default`、`Eq`、`Hash`、`PartialEq`。需要注意：派生相等和哈希会同时考虑 `O`、`L`，所以 `CIString` 自身并不是仅按 `L` 比较的键；真正的查找键构造器只提取 `L`。
- `pub fn CIString::New(value: impl Into<String>) -> CIString`：消费任意可转成 `String` 的输入，先取得原文，再生成小写副本。
- `pub struct SchemaTableKey { pub schema: String, pub table: String }`：由两个规范化字符串组成的表键，派生 `Clone`、`Debug`、`Eq`、`Hash`、`PartialEq`。
- `pub fn newSchemaTableKey(schema: CIString, table: CIString) -> SchemaTableKey`：消费两个 `CIString`，只移动各自的 `L` 字段进入结果。
- `pub struct TableAliasKey { pub schema: String, pub name: String, pub qualified: bool }`：别名键。`qualified` 是身份的一部分，不由 `schema` 是否为空推导。
- `pub fn newTableAliasKey(name: CIString) -> TableAliasKey`：产生 `schema == ""`、`name == name.L`、`qualified == false` 的未限定键。
- `pub fn newQualifiedTableAliasKey(schema: CIString, name: CIString) -> TableAliasKey`：产生 `schema == schema.L`、`name == name.L`、`qualified == true` 的限定键，即使 schema 文本为空也保持 `qualified == true`。

文件中没有 trait、自定义 `impl`（除 `CIString` 的固有实现外）、常量、静态变量或条件编译项。

## 执行流程

1. 调用者先通过 `CIString::New` 输入标识符。该函数把输入转换为拥有所有权的 `String`，以 `to_lowercase()` 计算 `L`，同时把原字符串保存在 `O`。
2. 构造表键时，`newSchemaTableKey` 丢弃原始大小写身份，只取 schema 和 table 的 `L`，形成可用于 `HashMap`/`HashSet` 的二字段键。
3. 构造别名键时，调用者必须选择未限定或限定构造器。未限定构造器清空 schema 并将 `qualified` 设为 `false`；限定构造器保留规范化 schema 并将其设为 `true`。
4. 集合比较由派生的 `Eq`/`Hash` 完成。`schema_table_key_test.rs` 证明大小写不同的 `SchemaTableKey` 在 `HashSet` 中折叠为一个元素，也证明未限定别名与“限定但 schema 为空”的别名不相等。

当前 Rust 运行时没有从预处理或计划构建入口调用这些键构造器的证据；上述流程止于数据构造和集合语义。Go 端完整调用流程见 `preprocess.go`、`planbuilder.go` 与 `logical_plan_builder.go`，不能直接当作 Rust 调用链。

## 数据与状态

所有结构体都是拥有数据的值类型：字段为 `String` 或 `bool`，没有借用、内部可变性、全局状态或缓存。构造函数消费 `CIString`，把所需的 `L` 字段移动到结果中；未保存 `O`，因此键不会携带诊断所需的原始拼写。

关键不变量如下：

- 通过 `CIString::New` 创建的值满足 `L == O.to_lowercase()`；字段为公开字段，直接结构体字面量可绕过这一不变量。
- 通过 `newSchemaTableKey` 创建的两个字段都来自输入的 `L`。
- 通过 `newTableAliasKey` 创建的键总是未限定，且 schema 为空。
- 通过 `newQualifiedTableAliasKey` 创建的键总是限定；`qualified` 与 schema 文本是否为空彼此独立。
- 键的全部字段都参与派生相等与哈希，因此相等值必然产生一致哈希；扩充字段会自动改变键身份。

## 依赖与调用关系

本文件只依赖 Rust 标准库：`String`、`Into<String>`、`str::to_lowercase`，以及编译器提供的派生实现；它不直接依赖 `Cargo.toml` 中列出的其他 workspace crate，也不受 `nextgen` feature 条件控制。

RustCodeGraph 对文件给出的直接使用文件为：

- `pkg/planner/core/schema_table_key_test.rs`：调用全部三个键构造函数和 `CIString::New`，验证规范化与限定状态。
- `pkg/planner/core/util.rs`：`getLowerDB` 接收 `CIString` 并返回 `L`；不使用两类键。
- `pkg/planner/core/util_test.rs`：构造 `CIString` 验证 `getLowerDB`；不使用两类键。

额外精确搜索显示 `pkg/planner/core/common_plans.rs` 也将 `CIString` 用作若干计划字段类型。Rust 端没有发现 `newSchemaTableKey`、`newTableAliasKey` 或 `newQualifiedTableAliasKey` 的非测试调用。Go 端的直接调用关系则是：`preprocess.go::isTableAliasDuplicate` 使用两种别名键；`lockSelectCtx` 使用 `schemaTableKey` 绑定限定锁目标；`logical_plan_builder.go::PlanBuilder.checkRecursiveView` 用它维护视图构建栈；`planbuilder.go` 用它记录正在重命名的视图。

## 错误处理与边界

本文件没有 `Result`、`Option`、显式错误分支或 panic。分配 `String` 和 Unicode 小写转换的资源失败遵循 Rust 标准库的进程级行为，不在此层恢复。

需要特别关注的边界是：

- 规范化采用 Unicode `to_lowercase()`，而不是数据库会话变量、排序规则或 ASCII 专用转换；某些字符的小写结果可能改变长度。
- `CIString::default()` 产生两个空字符串，满足构造不变量；但公开字段允许构造 `O`、`L` 不一致的值。
- `CIString` 自身的派生 `Eq`/`Hash` 同时包含原文和小写值。大小写不敏感身份只对经过键构造器提取 `L` 的 `SchemaTableKey`/`TableAliasKey` 成立。
- 空 schema 的限定别名不能与未限定别名合并，因为 `qualified` 不同。该边界由 `schema_table_key_test.rs::alias_keys_preserve_qualification_independently_of_schema_text` 锁定。
- 构造函数按值接收参数；调用后原 `CIString` 不再可用，除非调用者事先克隆。

## 并发与资源生命周期

类型不包含锁、原子变量、任务、通道、事务、文件句柄或网络资源。值完全拥有字符串，生命周期不依赖外部对象；离开作用域时由 Rust 自动释放。字段类型使这些值通常可跨线程移动或共享，但本文件没有显式并发协议，也没有实现并发容器。

Go 端 `buildingViewStack` 的压栈/删除和预处理 map 的作用域属于调用者生命周期，并非本 Rust 文件当前实现的状态管理。若未来在 Rust 端接线，应由上层规划器保证每次语句/递归构建的容器隔离与清理，而不应把可变全局状态加入这些键类型。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/planner/core/schema_table_key.go`：

- Rust `SchemaTableKey` 对应 Go `schemaTableKey`，字段和构造器都只保存 `ast.CIStr.L`。
- Rust `TableAliasKey` 对应 Go `tableAliasKey`；未限定构造器使用空 schema 和 false，限定构造器使用两个 `L` 字段和 true。
- Rust 自建了 `CIString`；Go 版本直接使用 `pkg/parser/ast.CIStr`。因此 Rust 的 `CIString::New` 是局部兼容载体，不等同于已经完整移植 parser AST 的所有标识符规则。
- Go 的键类型和构造器是包内私有；Rust 符号为 `pub` 并从 crate 根重导出，API 可见性更宽。
- Go 键已在实际规划流程中使用：JOIN 表别名去重、`SELECT ... FOR UPDATE/LOCK IN SHARE MODE` 的限定目标绑定、递归视图检测和视图重命名判断。Rust 当前只具备键模型与单元测试，没有这些生产调用边。

Go 的行为测试证据包括 `pkg/planner/core/preprocess_test.go` 中大小写不同别名仍触发 `ErrNonUniqTable` 的用例；锁目标的用户可见兼容行为还出现在 `tests/integrationtest/t/planner/core/issuetest/planner_issue.test` 及对应结果文件。它们证明 Go 上层用法，不等价于 Rust 测试覆盖。

## 扩展指南

- 若要把键接入 Rust 预处理或计划构建，优先复用三个现有构造器，不要在调用点直接读取 `O` 或重复拼接字符串；同时在对应上层模块的独立 `*_test.rs` 中覆盖真实调用流程。
- 若要改变大小写规则，先确认 parser 标识符、MySQL `lower_case_table_names` 和 Go `ast.CIStr.L` 的兼容要求。修改 `CIString::New` 会影响 `util.rs::getLowerDB` 和 `common_plans.rs` 中持有该类型的字段，不应只更新键测试。
- 若要增加键字段，必须评估派生 `Eq`/`Hash` 会让新字段自动成为身份的一部分，并同步 `schema_table_key_test.rs`；与 Go 对齐时还需同步 `schema_table_key.go` 的字段和所有 map 调用点。
- 若要收紧不变量，可考虑将字段私有化或提供访问器，但这是公开 API 兼容变更；需先盘点 crate 外调用者。
- 不要把测试嵌入本文件。现有独立测试位于 `pkg/planner/core/schema_table_key_test.rs`，crate 根以 `#[cfg(test)] mod schema_table_key_test;` 接入。
- 性能上，每次 `CIString::New` 都分配并计算小写字符串；热路径扩展应复用已有规范化值，避免无依据的重复转换，同时保持 Go 语义一致。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/planner/core/schema_table_key.rs --offset 1 --limit 400`：读取目标文件全部 84 行，并报告直接使用者 `schema_table_key_test.rs`、`util.rs`、`util_test.rs`。
- RustCodeGraph `query SchemaTableKey`、`query TableAliasKey`、`query newTableAliasKey`、`query newQualifiedTableAliasKey`：确认 Rust 与 Go 对应符号及位置。对三个 Rust 构造器执行 `callers`/`callees` 未返回调用边，因此用精确引用搜索补充核验。
- 已读 Rust 文件：`pkg/planner/core/schema_table_key.rs`、`pkg/planner/core/schema_table_key_test.rs`、`pkg/planner/core/util.rs`、`pkg/planner/core/util_test.rs`、`pkg/planner/core/lib.rs`；`pkg/planner/core` 不存在 `doc.go`。
- 已读 crate 配置：`pkg/planner/core/Cargo.toml`，确认 crate 名、库入口、feature 与端口元数据。
- 已读 Go 直接证据：`pkg/planner/core/schema_table_key.go`、`pkg/planner/core/preprocess.go`、`pkg/planner/core/planbuilder.go`、`pkg/planner/core/logical_plan_builder.go`、`pkg/planner/core/preprocess_test.go`，并搜索相关 integration test/test result。
- 按任务约束未运行 Cargo 或代码测试；本次只新增说明文档，结构检查是指定的交付验证。
