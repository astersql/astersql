# `pkg/testkit/external/util.rs`

## 文件定位

`util.rs` 属于 Cargo crate `astersql-testkit-external`（见 `pkg/testkit/external/Cargo.toml`），是 Go 包 `pkg/testkit/external` 的 Rust 语义边界。它不执行 SQL，而是把测试会话后的 Domain、InfoSchema 和表元数据压缩成小型 trait，再提供按库表查表、查 ALTER TABLE 过渡列和查索引 ID 的测试辅助函数。`pkg/testkit/external/lib.rs` 将本文件的全部公开类型、trait、类型别名和三个函数重新导出为 crate API。

当前接线状态需要与 API 定义分开理解：Cargo workspace 和多个 Rust 测试 crate 已声明 `astersql-testkit-external` 依赖，但代码搜索只在 `pkg/testkit/external/util_aster_unit_test.rs` 找到本文件四个 trait 的实现。因此，已验证的是辅助函数在 mock 边界上的 Go 语义；“真实 Rust `TestKit` 已能直接调用”未由生产 adapter 证据支持。

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

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/testkit/external/util.go`。Rust `GetTableByName` 对应 Go 的 `domain.GetDomain(tk.Session()) -> dom.Reload() -> dom.InfoSchema().TableByName(...)`；Rust 用 `TestKitDomain` 等 trait 代替具体类型，用 `ExternalResult` 代替 `require.NoError` 对 `testing.T` 的立即终止。

Rust `GetModifyColumn` 保留 Go 的先刷新、列名小写匹配和首个命中结果。Go 在 `allColumn` 分支中把 `table.Table` 断言为 `*tables.TableCommon` 并读其 `Columns`，Rust 改由 `TableMetadata::all_columns` 显式表达这项能力。Go 返回指针或 `nil`，Rust 返回拥有的 `Option<Column>`。

Rust `GetIndexID` 与 Go 一样直接使用当前 InfoSchema，不 reload。Go 在缺失时 `require.FailNow` 并保留一个实际不可达的 `return -1`；Rust 返回 `Err(ExternalError)`，不使用 `-1` 哨兵值。两者的错误文本格式对齐。

`pkg/testkit/external/util_aster_unit_test.rs` 是直接 Rust 回归证据：覆盖 reload 顺序、reload/lookup 错误短路、公开/全量列集、列名大小写、不存在列、索引查询不 reload、索引名大小写以及查表/缺索引错误。它不证明真实 Domain/TestKit adapter 的存在或集成行为。

## 扩展指南

1. 要让这些 API 服务真实 Rust `TestKit`，应在适当的独立生产文件中实现 `TestKitDomain`、`Domain`、`InfoSchema` 和 `TableMetadata`，不要把适配器或测试塞进 `util.rs`。必须保证 `all_columns` 真的包含 Go `TableCommon.Columns` 可见的 DDL 过渡列，且 `reload` 与真实 Domain 刷新契约一致。
2. 修改刷新时机时，同时检查 `GetTableByName`、`GetModifyColumn` 和 `GetIndexID`；前两者要 reload，后者故意不 reload。应在独立的 `util_aster_unit_test.rs` 增加或调整副作顺序断言。
3. 新增元数据字段时，优先扩展 `Column`/`Index` 及 adapter 映射，并保持类型是测试辅助所需的最小子集。新字段可能提高 clone 成本，需复核 `GetModifyColumn` 返回拥有值的性能取舍。
4. 修改名称匹配规则前，先以 Go `ast.CIStr`/`Name.L` 与真实 Rust parser 标识符规范化语义建立对照证据，覆盖 ASCII、非 ASCII 和索引名大小写。不要因为列查询不区分大小写就顺手改变当前索引精确比较契约。
5. 修改错误时保持 `ExternalError::message` 可稳定断言，并保留查询错误的原始上下文。如要增加错误枚举或 source chain，需同步检查 Go 失败文本兼容性。
6. Rust 源码和测试逻辑应继续分文件放置：生产修改落在 `util.rs` 或新的 adapter 文件，回归测试同步落在 `util_aster_unit_test.rs`。真实接线完成后，除 mock 单元测试外还应增加最小集成测试，证明真实 TestKit 能刷新并读取 DDL 后元数据。

## 验证依据

- RustCodeGraph 索引状态：`status` 报告 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/testkit/external` 识别 `lib.rs`、`util.go`、`util.rs` 和 `util_aster_unit_test.rs`。
- RustCodeGraph 符号查询：`query` 定位 Rust/Go 同名的 `GetTableByName`、`GetModifyColumn`、`GetIndexID`，并定位 `ExternalError`、`TableMetadata`、`TestKitDomain`。
- RustCodeGraph `node` 调用边：`GetTableByName` 调用 `domain/reload/info_schema/table_by_name` 并被 `GetModifyColumn` 与直接测试调用；`GetModifyColumn` 调用 `GetTableByName/columns/all_columns`；`GetIndexID` 调用 `domain/info_schema/table_by_name/indices/index_not_found` 并被错误契约测试调用。独立 `callers/callees` CLI 在本次查询中未在 30 秒内返回，因此不将其当作额外证据。
- 已读源与边界：`pkg/testkit/external/util.rs`、`pkg/testkit/external/lib.rs`、`pkg/testkit/external/Cargo.toml`、`pkg/testkit/external/BUILD.bazel`。Cargo 工作区成员和若干测试 crate 的 manifest 引用通过 `rg` 核对。
- 已读 Go 对照：`pkg/testkit/external/util.go`，核对了 reload 时机、`TableCommon.Columns`/`Cols()` 分支、`Name.L` 匹配、InfoSchema 直读和 `require.FailNow` 文本。
- 已读独立 Rust 测试：`pkg/testkit/external/util_aster_unit_test.rs`，其五个测试覆盖刷新顺序与错误、列集分支、索引查询的当前 schema 语义和缺失错误。
- 生产 adapter 搜索：对 `impl (TableMetadata|InfoSchema|Domain|TestKitDomain) for` 的 Rust 搜索只在 `util_aster_unit_test.rs` 命中本文件的四个 trait；其他同名 `Domain`/`InfoSchema` 实现属于不同模块的 trait，不是这个 crate 的适配。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定的 `test -f` 加固定二级标题计数命令做结构验证，并人工复核文档不把 Cargo 依赖声明误写为已完成的真实接线。
