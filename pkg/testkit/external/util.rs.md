# `pkg/testkit/external/util.rs`

## 文件定位

将 Go `testing.T`/`require` 的立即失败方式转换为 `ExternalResult<T>`，让 Rust 调用者决定用 `?`、`expect` 或自定义断言终止测试。

## 核心职责

- 用 `TestKitDomain -> Domain -> InfoSchema -> TableMetadata` 的关联类型链隔离具体会话、Domain 和表实现，让辅助函数可用 mock 验证。
- 保留 Go `GetTableByName` 的关键一致性副作：先强制刷新 Domain schema，再从新 InfoSchema 查表。
- 保留 Go `GetModifyColumn` 对两种列视图的区分：普通公开列与包含在线 DDL 过渡态列的全量物理列。
- 保留 Go `GetIndexID` 的当前 schema 查询路径，即不先调用 `Domain::reload`，并在索引不存在时给出可断言的完整限定名错误。
- 将 Go `testing.T`/`require` 的立即失败方式转换为 `ExternalResult<T>`，让 Rust 调用者决定用 `?`、`expect` 或自定义断言终止测试。

## 主要符号

- `ExternalError { message: String }`：本 crate 的轻量错误。`new` 接收任意 `Into<String>`，`message` 暴露可断言文本，并实现 `Display` 和 `std::error::Error`。私有函数 `index_not_found` 统一生成 `index {index} not found(db: {database}, tbl: {table})` 格式。
- `ExternalResult<T>`：`Result<T, ExternalError>` 的公开别名，是所有可失败适配器方法的共同边界。
- `Column`：只保留辅助查询需要的 `name`、`id`、`offset` 和 `hidden`。`Column::new` 构造这个值对象。当前函数仅用 `name` 匹配，其他字段供调用者检查。
- `Index`：保留 `name` 和 `id`，`Index::new` 用于构造。
- `TableMetadata`：表元数据最小接口。`columns()` 返回公开列，`all_columns()` 返回包括 DDL 过渡列的全部物理列，`indices()` 返回索引。三者都借用实现者持有的 slice，不复制容器。
- `InfoSchema`：通过关联类型 `Table: TableMetadata` 绑定表表示；`table_by_name` 用库名和表名查询并返回拥有的表值。
- `Domain`：通过关联类型 `InfoSchema: InfoSchema` 绑定 schema 视图；`reload` 暴露显式刷新副作，`info_schema` 返回当前视图。
- `TestKitDomain`：将 Go `domain.GetDomain(tk.Session())` 压缩为 `domain(&self) -> &Self::Domain`，是三个公开函数的唯一泛型约束。
- `DomainOf<T>`、`InfoSchemaOf<T>`、`TableOf<T>`：沿关联类型链投影出某个 `T` 的具体 Domain、InfoSchema 和 Table 类型；`GetTableByName` 用 `TableOf<T>` 保留实现者的具体表类型。
- `GetTableByName`：刷新 schema 后查表。
- `GetModifyColumn`：查表后在指定列视图中执行不区分大小写的线性查找，返回克隆的 `Column` 或 `None`。
- `GetIndexID`：在当前 InfoSchema 的索引 slice 中按名线性查找，返回 ID 或精确错误。

`#![allow(non_snake_case)]` 使三个 Rust 入口保留 Go 导出函数名，降低对照和迁移时的命名偏差。本文件没有条件编译项；测试模块的 `#[cfg(test)]` 装配在 `lib.rs`。

## 执行流程

`GetTableByName(test_kit, database, table)` 的步骤是：

1. 通过 `TestKitDomain::domain` 取得 Domain 引用。
2. 调用 `Domain::reload`；失败时 `?` 立即返回，不会进行表查询。
3. 调用 `Domain::info_schema` 取刷新后的视图。
4. 将原始库名和表名传入 `InfoSchema::table_by_name`，直接传播其成功值或错误。

`GetModifyColumn` 先调用 `GetTableByName`，因而每次都继承一次 reload。它把输入列名转成小写；`all_column == true` 时选 `all_columns()`，否则选 `columns()`。然后依顺序对每个 `column.name` 执行 `to_lowercase()` 并返回第一个相等项的 clone；没有匹配项是正常结果 `Ok(None)`。

`GetIndexID` 不调用 `reload`。它从 `test_kit.domain().info_schema()` 直接查表，遍历 `indices()`，用 `index.name == index_name` 精确比较，找到时映射为 `index.id`。找不到则延迟构造 `ExternalError::index_not_found`。该路径与 `GetModifyColumn` 的不区分大小写匹配不同，是需保留的兼容边界。

## 数据与状态

本文件没有全局可变状态、缓存或持久化数据。`Column`、`Index` 和 `ExternalError` 都是可 clone、可比较的拥有值。查询所观察到的 schema 时点由调用路径决定：`GetTableByName`/`GetModifyColumn` 要求 reload 后的视图，`GetIndexID` 只使用当前视图。

trait 返回 slice 引用，所以列和索引容器的存储与顺序由 `TableMetadata` 实现者管理。`GetModifyColumn` 克隆命中列，避免把该借用生命周期暴露到返回值；`GetIndexID` 只复制 `i64`。`Column.hidden` 不参与筛选，是否能看到隐藏列只由 `columns` 和 `all_columns` 的集合边界决定。

## 依赖与调用关系

内部调用链为 `GetModifyColumn -> GetTableByName -> TestKitDomain::domain -> Domain::reload/Domain::info_schema -> InfoSchema::table_by_name`。`GetIndexID` 则独立走 `TestKitDomain::domain -> Domain::info_schema -> InfoSchema::table_by_name -> TableMetadata::indices`，最后可能调用 `ExternalError::index_not_found`。RustCodeGraph 的 `node` trail 确认了这些 callee 边。

`lib.rs` 是直接上游门面，它 `mod util` 并 `pub use` 本文件的 API；同文件还在 `#[cfg(test)]` 下装配 `util_aster_unit_test.rs`。RustCodeGraph 对 `GetTableByName` 的 caller trail 包含 `GetModifyColumn` 和两个直接单元测试，对 `GetIndexID` 识别到错误契约测试。某些 Rust 迁移测试文件仍保留了 Go 步骤文本，图或文本命中不应自动等价为可编译的 Rust 直接调用。

`pkg/testkit/external/Cargo.toml` 把 crate 边界声明为 `lib.rs`，并列出 domain、parser AST、table、table/tables 和 testkit 的本地路径依赖，与 Go `util.go` 的概念依赖对应。但 `util.rs` 本身只使用 `std::fmt`，并未引用这些具体 crate 类型；这与“当前以 trait 抽象保留语义、尚缺真实 adapter”的代码事实一致。Cargo feature 在该 manifest 中未定义。

## 错误处理与边界

- reload 错误会从 `GetTableByName` 直接返回，并阻止后续 `table_by_name`；单元测试通过查询记录为空验证了短路。
- 表查询错误保留原 `ExternalError` 文本，三个公开函数都不吞错或改写它。
- `GetModifyColumn` 的“列不存在”不是错误，而是 `Ok(None)`；调用者必须自行决定是否断言。
- 列名使用 Rust Unicode `to_lowercase()` 后比较，Go 使用 `strings.ToLower` 且与已规范化的 `Name.L` 比较。对测试中的 ASCII SQL 标识符语义已验证；非 ASCII 标识符的完全等价性未由现有测试证明。
- 索引名是精确、区分大小写的 String 比较。测试证明 `IDX_NAME` 不会命中 `idx_name`，并检查完整错误文本。这是当前 Rust 契约，不应未经 Go 元数据规范化语义复核就改为忽略大小写。
- `all_column == true` 假定 adapter 能提供完整物理列集。Go 版通过 `tt.(*tables.TableCommon)` 类型断言强制这一点，失配会 panic；Rust 把能力放入 `TableMetadata` trait，所以不会在函数内做动态断言，正确性由实现者保证。

## 并发与资源生命周期

本文件不启动线程、任务或通道，不获取显式锁，也不开启事务。它对 trait 实现者也没有 `Send`/`Sync` 约束，因此 API 本身不承诺跨线程使用。

`GetTableByName` 在一个共享 Domain 引用上先 reload 后取 InfoSchema，两步间的原子性、并发刷新排序和 snapshot 稳定性均由未来 `Domain` 实现者承担，本文件不加锁。`InfoSchema::Table` 按值返回，`GetModifyColumn` 又 clone 列，因此返回结果不借用 Domain 或 InfoSchema。单元测试中的 `Arc<Mutex<_>>` 只用来记录 reload 和 lookup 副作，不是生产实现的并发策略。

## 扩展指南

1. 要让这些 API 服务真实 Rust `TestKit`，应在适当的独立生产文件中实现 `TestKitDomain`、`Domain`、`InfoSchema` 和 `TableMetadata`，不要把适配器或测试塞进 `util.rs`。必须保证 `all_columns` 真的包含 Go `TableCommon.Columns` 可见的 DDL 过渡列，且 `reload` 与真实 Domain 刷新契约一致。
2. 修改刷新时机时，同时检查 `GetTableByName`、`GetModifyColumn` 和 `GetIndexID`；前两者要 reload，后者故意不 reload。应在独立的 `util_aster_unit_test.rs` 增加或调整副作顺序断言。
3. 新增元数据字段时，优先扩展 `Column`/`Index` 及 adapter 映射，并保持类型是测试辅助所需的最小子集。新字段可能提高 clone 成本，需复核 `GetModifyColumn` 返回拥有值的性能取舍。
4. 修改名称匹配规则前，先以 Go `ast.CIStr`/`Name.L` 与真实 Rust parser 标识符规范化语义建立对照证据，覆盖 ASCII、非 ASCII 和索引名大小写。不要因为列查询不区分大小写就顺手改变当前索引精确比较契约。
5. 修改错误时保持 `ExternalError::message` 可稳定断言，并保留查询错误的原始上下文。如要增加错误枚举或 source chain，需同步检查 Go 失败文本兼容性。
6. Rust 源码和测试逻辑应继续分文件放置：生产修改落在 `util.rs` 或新的 adapter 文件，回归测试同步落在 `util_aster_unit_test.rs`。真实接线完成后，除 mock 单元测试外还应增加最小集成测试，证明真实 TestKit 能刷新并读取 DDL 后元数据。
