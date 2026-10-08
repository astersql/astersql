# `pkg/planner/core/resolve/result.rs`

源码：[`result.rs`](result.rs)

## 文件定位

`result.rs` 位于 `astersql-planner-core-resolve` crate 中，定义名称解析结果中的列绑定元数据 `ResultField`。crate 入口 [`lib.rs`](lib.rs) 通过 `pub mod result` 声明模块，并以 `pub use result::ResultField` 从 crate 根重新导出该类型；因此调用方通常使用 `astersql_planner_core_resolve::ResultField`，无需经过 `result` 子模块。

该 crate 的 [`Cargo.toml`](Cargo.toml) 将库入口设为 `lib.rs`，直接依赖 `astersql-parser-ast` 和 `astersql-meta-model`。前者提供大小写不敏感标识符 `ast::CIStr`，后者经 `lib.rs::model` 重新导出 `ColumnInfo`、`TableInfo` 和 `DBInfo`。Cargo 元数据把对应 Go 包标为 `pkg/planner/core/resolve`，移植追踪项为 `task-432`，没有为本文件声明 feature 开关。

当前 Rust 文件是一个纯数据模型文件：除导入外仅有一个公开结构体，无常量、trait、函数、`impl` 或条件编译项。它既不执行名称查找，也不负责生成协议列；它保存这些阶段之间需要传递的绑定结果。

## 核心职责

- 用 `column` 和 `table` 保存解析后关联的列、表元数据；二者均为可选值，以便表示表达式列或没有原始表信息的结果列（`ResultField`）。
- 用 `column_as_name`、`table_as_name` 和 `db_name` 保存对外呈现的列别名、表别名和数据库名；字段类型为 `ast::CIStr`，同目录测试同时检查其原始形式 `O` 和规范化形式 `L`（`migration_aster_unit_test.rs::result_field_preserves_binding_metadata_and_empty_org_name`）。
- 用独立布尔量 `empty_org_name` 区分“原始列名应为空”与普通列。Go 注释解释了保留该位的原因：表达式列需要空 `org_name`，但不能假定把 `Column.Name` 清空是安全的（`result.go::ResultField.EmptyOrgName`）。
- 作为服务端协议元数据转换的输入。`pkg/server/internal/column/convert.rs::ConvertColumnInfo` 从该类型读取别名、库表名和列类型信息，`empty_org_name` 为真时清空协议 `OrgName`。

需要区分设计语义和当前接线事实：Go 文件把 `ResultField` 描述为每个已解析 `ColumnNameExpr` 的关键绑定对象；Rust 类型和字段完整保留了这一模型，但 RustCodeGraph 对当前索引的直接实例化边只识别到同目录测试以及 `pkg/server/runtime.rs::protocol_result_column`，没有证明完整 Rust 名称解析主链已经为每个 `ColumnNameExpr` 构造该类型。

## 主要符号

### `pub struct ResultField`

结构体派生 `Clone` 和 `Default`，六个字段全部公开：

| 字段 | 类型 | 语义与边界 |
| --- | --- | --- |
| `column` | `Option<Rc<model::ColumnInfo>>` | 原始列元数据。表达式列可以没有列元数据；但协议转换 `ConvertColumnInfo` 要求这里为 `Some`，否则会以明确消息 panic。 |
| `column_as_name` | `ast::CIStr` | 结果列对外名称或 `AS` 别名；协议转换使用其原始形式 `O`。 |
| `empty_org_name` | `bool` | 是否强制把协议原始列名输出为空；它不能由 `column.is_none()` 或空名称替代。 |
| `table` | `Option<Rc<model::TableInfo>>` | 原始表元数据。存在时转换为协议 `OrgTable`；缺失不会阻止使用表别名。 |
| `table_as_name` | `ast::CIStr` | 对外表名或表别名，转换为协议 `Table`。 |
| `db_name` | `ast::CIStr` | 数据库名，转换为协议 `Schema`。 |

`Clone` 对 `Rc` 字段只增加引用计数，克隆后的多个 `ResultField` 共享同一份 `ColumnInfo`/`TableInfo`；`Default` 则产生两个 `None`、三个默认 `CIStr` 和 `empty_org_name == false`。本文件没有自定义构造器或校验器，调用方必须自行维持字段组合的不变量。

## 执行流程

该文件没有可执行函数；实际数据流由构造方和消费方组成：

1. 名称解析/结果集构造方准备列元数据、可选表元数据以及库表列名称，然后直接构造 `ResultField`。Go 的对应流程把它作为名称绑定产生的属性；Rust 的同目录测试以完整字面量构造它并验证信息不丢失。
2. 普通结果集协议路径中，`pkg/server/internal/resultset/resultset.rs::TidbResultSet::Columns` 遍历 `RecordSet::Fields()`，对每个 `ResultField` 调用 `ConvertColumnInfo`，并缓存转换后的 `Info`。
3. `pkg/server/internal/column/convert.rs::ConvertColumnInfo` 要求 `column` 存在，然后读取列别名、原始列名、表别名、数据库名、字段类型、字符集、默认值等；若 `empty_org_name` 为真则清空 `OrgName`，若 `table` 存在则填写 `OrgTable`。
4. 生产协议工作线程还有一条桥接路径：`pkg/server/runtime.rs::protocol_result_column` 将可跨线程的 `ConcreteResultField` 临时转换为本类型，再交给 `protocol_column`/`ConvertColumnInfo`。它固定设置 `empty_org_name = false`、`table = None`，随后用 `ConcreteResultField.table_name` 补写 `org_table`。
5. 转换后的协议列元数据进入 MySQL 返回列描述；本类型本身不持有行值，也没有逐行更新逻辑。

## 数据与状态

`ResultField` 是某一结果列的描述性快照，不含全局状态、内部可变性或缓存。重要状态组合如下：

- `column = Some(...)` 是进入 `ConvertColumnInfo` 的前置条件；`Default` 只适合先构造后填充或不经过该转换的场景。
- `table = None` 合法，意味着没有可供填写 `OrgTable` 的原始表元数据；`table_as_name` 仍可独立存在并作为协议 `Table`。
- `empty_org_name = true` 表示有意隐藏原始列名，典型场景是标量表达式列。它不会修改共享的 `ColumnInfo.Name`，只影响消费方生成的协议信息。
- 名称字段使用 `CIStr` 同时保留原始拼写和小写规范化值；协议层读取 `O`，比较或解析逻辑可使用 `L`。测试以 `"total"`、`"o"`、`"app"` 验证这些值被原样保留。

结构体没有生命周期参数；元数据通过 `Rc` 共享所有权，释放时机由最后一个本线程引用决定。它也不拥有查询行、事务、锁、通道或后台任务。

## 依赖与调用关系

直接类型依赖：

- `std::rc::Rc`：共享 `ColumnInfo` 和 `TableInfo`，避免复制元数据。
- `crate::ast`：由 crate 根把 `astersql-parser-ast` 重导出为 `ast`，提供 `CIStr`。
- `crate::model`：由 crate 根从 `astersql-meta-model::group_1` 重导出元数据类型。

已核验的上游构造/持有者：

- `pkg/planner/core/resolve/migration_aster_unit_test.rs::result_field_preserves_binding_metadata_and_empty_org_name`：直接构造完整对象，覆盖六个字段中的绑定语义。
- `pkg/server/runtime.rs::protocol_result_column`：把 `ConcreteResultField` 桥接成本类型，随后生成协议列。
- `pkg/server/driver_tidb_test.rs::canonical_convert_column_info_preserves_mysql_display_width_rules` 及 `pkg/server/internal/column/migration_aster_unit_test.rs::result_field`：为协议转换测试构造本类型。

已核验的下游消费者：

- `pkg/server/internal/column/convert.rs::ConvertColumnInfo`：核心消费者，把绑定元数据映射为 MySQL 协议列信息。
- `pkg/server/internal/resultset/resultset.rs::TidbResultSet::Columns`：从 `RecordSet::Fields()` 获取本类型切片，逐项转换并缓存。
- `pkg/server/runtime.rs::protocol_column`：生产服务端路径上的薄包装，调用同一转换函数后映射为运行时协议结构。

RustCodeGraph 显示 crate 根 `lib.rs` 公开重导出该类型，并显示目标文件被 `pkg/server/runtime.rs` 使用。仓库中还有多个 crate 在 Cargo 层依赖整个 resolve crate；仅凭依赖声明不能推断它们都使用 `ResultField`，因此不把这些 crate 列为本类型的直接调用者。

## 错误处理与边界

本文件没有返回 `Result`、错误枚举或显式校验，所有字段组合都能通过公开字段或 `Default` 构造。这带来几项调用边界：

- `column = None` 在数据模型中可表达“没有原始列元数据”，但 `ConvertColumnInfo` 会在此条件下以 `ResultField.column must be present when converting protocol metadata` panic。需要协议输出的构造路径必须先补齐列元数据，不能把 `Default` 直接送入转换器。
- `table = None` 是被消费方显式支持的分支：只是不填 `OrgTable`，不会报错。
- `empty_org_name` 优先于 `ColumnInfo.Name`：即使列元数据包含名称，该位为真时协议原始列名仍为空。Go 的 `TestEmptyOrgName` 从完整查询响应验证表达式别名 `YEAR` 的 `org_name` 为空。
- 本类型不验证别名长度、标识符合法性、字段类型、字符集或显示宽度；这些规则由解析器、元数据生产方和 `ConvertColumnInfo` 等下游负责。
- `Clone` 是共享而非深复制；如果底层元数据类型将来获得内部可变性，多个克隆之间的可见性需要重新审视。

## 并发与资源生命周期

`ResultField` 使用单线程引用计数 `Rc`，因此该类型不能作为 `Send`/`Sync` 数据直接跨线程传递。引用计数由构造、克隆和析构自动管理，没有手工关闭或回收动作，也没有循环引用字段。

生产服务端显式设置了并发边界：`pkg/session/runtime/session.rs::ConcreteResultField` 使用按值持有的 `ColumnInfo`，注释将其标为 “Send-safe result-column metadata”；`pkg/server/runtime.rs::protocol_result_column` 在协议工作侧再将其包装进 `Rc` 版 `ResultField`。扩展跨线程路径时应继续传递可发送的拥有型结构，而不是把本类型或其 `Rc` 字段塞入 `Arc<Mutex<_>>` 来绕过边界。

本文件不创建锁、异步任务、通道、事务或外部资源。`TidbResultSet` 对转换结果的缓存和完成锁属于消费者生命周期，不由 `ResultField` 管理。

## 与 Go 版本的对应关系

对应文件为 [`result.go`](result.go)，两侧字段一一映射：

| Go | Rust | 差异 |
| --- | --- | --- |
| `Column *model.ColumnInfo` | `column: Option<Rc<model::ColumnInfo>>` | `nil` 映射为 `None`；非空指针映射为共享所有权 `Rc`。 |
| `ColumnAsName ast.CIStr` | `column_as_name: ast::CIStr` | 仅命名风格不同。 |
| `EmptyOrgName bool` | `empty_org_name: bool` | 语义一致。 |
| `Table *model.TableInfo` | `table: Option<Rc<model::TableInfo>>` | `nil` 映射为 `None`。 |
| `TableAsName ast.CIStr` | `table_as_name: ast::CIStr` | 语义一致。 |
| `DBName ast.CIStr` | `db_name: ast::CIStr` | 语义一致。 |

Rust 额外派生了 `Clone` 和 `Default`，但没有复刻 Go 文件的长篇类型注释所描述的完整运行时绑定过程。字段层面的迁移由 `migration_aster_unit_test.rs` 验证；协议层语义由 Rust 的 `ConvertColumnInfo` 测试和 Go 的 `pkg/server/driver_tidb_test.go::TestConvertColumnInfo`、`pkg/server/conn_test.go::TestEmptyOrgName` 交叉核对。

当前已验证的差异是所有权和线程模型，而不是字段含义：Go 指针可由运行时管理并发可达性，Rust 明确使用本线程 `Rc`；跨线程生产路径因此另设 `ConcreteResultField`。此外，Rust 协议转换对缺失 `column` 使用显式 `expect`，而 Go 转换器直接解引用 `fld.Column`，两者都要求协议转换前列信息非空。

## 扩展指南

新增或修改结果字段元数据时，应按以下接入面同步处理：

1. 修改 `result.rs::ResultField` 字段及其文档，并与 `result.go::ResultField` 核对是否属于 Go 对齐语义；不要只为通过测试引入 Rust 独有的简化模型。
2. 检查 `lib.rs` 的公开导出是否仍合适；若新增依赖，更新本 crate 的 `Cargo.toml`，但普通字段扩展通常无需更改模块出口。
3. 更新所有直接字面量构造点，重点包括 `protocol_result_column`、同目录 `migration_aster_unit_test.rs` 和 server column 测试辅助函数。公开字段新增且无默认展开时会在编译期暴露漏改点。
4. 如果字段需要进入 MySQL 协议，更新 `pkg/server/internal/column/convert.rs::ConvertColumnInfo`，并在其独立测试 `pkg/server/internal/column/migration_aster_unit_test.rs` 中覆盖存在、缺失和边界值；不要把测试代码嵌入生产源文件。
5. 如果字段必须跨协议线程传递，同步扩展 `ConcreteResultField` 及其各构造点，再在 `protocol_result_column` 中桥接；保持发送型对象按值/线程安全所有权持有，避免把 `Rc` 泄漏到跨线程接口。
6. 若改变 `empty_org_name` 或别名规则，除 Rust 单元测试外还应核对 Go `TestEmptyOrgName` 所代表的线协议兼容行为。此类变化可能影响客户端显示、ORM 字段映射和旧客户端兼容性。

性能风险主要来自把目前的共享元数据改为深复制，或在每列协议转换中增加昂贵计算；兼容性风险集中在列名、原始列名、库表名和字段类型的线协议表示；正确性风险集中在允许 `column = None` 流入要求非空的协议转换路径。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标目录列出 `lib.rs`、`resolve.rs`、`result.rs`、对应 Go 文件和独立测试。
- RustCodeGraph `node --file pkg/planner/core/resolve/result.rs`：确认文件共 42 行，唯一生产符号为 `ResultField`，并显示 `pkg/server/runtime.rs` 使用该文件。
- RustCodeGraph `query/node ResultField`：确认 Rust/Go 同名结构及字段，并给出 `protocol_result_column` 的实例化边；同名查询结果已按路径消歧。
- RustCodeGraph 对 `lib.rs`、`protocol_result_column`、`ConvertColumnInfo`、`TidbResultSet::Columns` 和 `ConcreteResultField` 的源码节点：确认公开导出、协议转换调用链、非空列前置条件、结果缓存以及线程边界。
- 源与配置：`pkg/planner/core/resolve/result.rs`、`lib.rs`、`Cargo.toml`、`result.go`。
- Rust 测试：`pkg/planner/core/resolve/migration_aster_unit_test.rs::result_field_preserves_binding_metadata_and_empty_org_name`；`pkg/server/driver_tidb_test.rs::canonical_convert_column_info_preserves_mysql_display_width_rules`；`pkg/server/internal/column/migration_aster_unit_test.rs::{convert_column_info_matches_go_length_decimal_and_alias_rules, convert_column_info_covers_decimal_unknown_charset_and_unspecified_flen, convert_column_length_keeps_go_uint32_wrapping_semantics}`。
- Go 对照测试：`pkg/server/driver_tidb_test.go::TestConvertColumnInfo` 和 `pkg/server/conn_test.go::TestEmptyOrgName`。

本任务是纯文档分析，按计划不运行 Cargo 或代码测试。完成前应以任务给定的结构命令确认文件存在且恰有十一个固定二级标题，并人工检查所有当前接线结论均有上述符号或路径支撑。
