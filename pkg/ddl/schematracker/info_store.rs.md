# `pkg/ddl/schematracker/info_store.rs`

## 文件定位

本文件属于 `astersql-ddl-schematracker` crate，是 schema tracker 的可变内存元数据存储层。crate 入口 `pkg/ddl/schematracker/lib.rs` 将本模块私有装配后公开再导出其符号；上层 `SchemaTracker`（`dm_tracker.rs`）持有一个 `InfoStore`，用它模拟 DDL 对数据库和表元数据的影响，`Checker`（`checker.rs`）再利用该状态做一致性检查。因此它不是持久化 DDL job 存储，也不执行真实集群上的 schema state 转换、回填或版本同步。

`pkg/ddl/schematracker/Cargo.toml` 指定 crate 根为同目录 `lib.rs`，并以 `meta-model` 和 `parser-ast` 提供本文件直接使用的 `DBInfo`、`TableInfo`、`CIStr` 类型；错误类型定义在 crate 根。Cargo 的 `package.metadata.porting.go-package` 指向 `pkg/ddl/schematracker`，明确了 Go 对照包。

## 核心职责

- `InfoStore` 用两层 `HashMap` 保存数据库信息，以及按数据库分桶的表信息；它是面向 schema tracker 的轻量、可覆盖写入的目录，而不是完整 `InfoSchema` 实现。
- `ciStr2Key` 统一所有库名和表名的索引规则：`lowerCaseTableNames == 0` 使用 `CIStr.O` 原始形式，其他值使用 `CIStr.L` 小写形式。写、查、删都经过同一规则，避免同一实例内键语义分裂。
- `InitFromIS` 通过最小化的 `InfoSchemaSource` trait 从外部信息模式批量导入库表；trait 只暴露本流程需要的 `AllSchemas` 与 `SchemaTableInfos`。
- `InfoStoreAdaptor` 将仓库查询包装成接近 Go `InfoSchema` 的返回形状，并以 `TableHandle` 交付一份拥有所有权的表元数据副本。

## 主要符号

- `trait InfoSchemaSource`：初始化数据源边界。`AllSchemas(&self) -> Vec<DBInfo>` 给出数据库快照；`SchemaTableInfos(&self, schema) -> Result<Vec<TableInfo>, Error>` 按库加载表并允许传播失败。
- `struct InfoStore`：公开 `lowerCaseTableNames`，内部保存 `dbs: HashMap<String, DBInfo>` 与 `tables: HashMap<String, HashMap<String, TableInfo>>`。`#[derive(Clone)]` 允许复制整个内存快照。
- `NewInfoStore(i32) -> InfoStore`：创建两个空映射并固定后续名称键策略；`NewSchemaTracker` 是明确的生产调用者。
- `InitFromIS`：按数据源顺序逐库写入，再逐表写入。它返回首个错误，不提供事务性回滚。
- `SchemaByName`、`TableByName`、`TableClonedByName`：分别返回库引用、表引用和表的拥有型克隆。克隆版本是上层“读出—修改—写回”流程的基础。
- `PutSchema`、`PutTable`：按规范化键插入并覆盖旧值。`PutSchema` 同时确保表分桶存在；`PutTable` 不会隐式创建数据库。
- `DeleteSchema`、`DeleteTable`：前者返回存在性布尔值并级联删除表分桶；后者区分缺库与缺表错误。
- `AllSchemaNames`、`AllTableNamesOfSchema`：返回内部键而非元数据中的展示名，且没有排序保证。
- `InfoStoreAdaptor`：提供 `SchemaByName` 的 `(Option, bool)`、`TableExists`、返回 `TableHandle` 的 `TableByName` 和返回引用的 `TableInfoByName`。
- `TableHandle(pub TableInfo)`：拥有克隆表定义的薄句柄；本文件没有为它增加额外行为。

## 执行流程

创建阶段由 `NewSchemaTracker(lower_case_table_names)` 调用 `NewInfoStore`，名称模式在仓库生命周期内作为字段参与每次键转换。普通 DDL 模拟由 `dm_tracker.rs` 先通过 `SchemaByName`、`TableByName` 或 `TableClonedByName` 检查/取得当前定义，再调用 `PutSchema`、`PutTable`、`DeleteSchema` 或 `DeleteTable` 提交内存变更。例如修改表类操作克隆 `TableInfo`，在克隆上变更后用同一 schema 键写回，从而避免持有共享引用时原地修改映射。

批量初始化入口为 `Checker::InitFromIS`（`checker.rs`），它直接转交 `self.tracker.InfoStore.InitFromIS(source)`。`InitFromIS` 对每个 `DBInfo` 依次执行：保存 `db.Name`、先 `PutSchema(db)`、调用 `source.SchemaTableInfos(&name)`，最后逐一 `PutTable(name.clone(), table)`。任一步返回错误即停止；此前插入的数据库和表继续保留。`info_store_test.rs::init_from_is_keeps_schema_inserted_when_table_loading_fails` 专门锁定了“先写库、加载表失败也不回滚”的 Go 对齐行为。

单项查询先用 `ciStr2Key` 计算 schema 键。`TableByName` 先查外层表分桶，缺失时报 `DatabaseNotExists`；分桶存在后再查表键，缺失时报 `TableNotExists`。适配器的 `TableExists` 将任何查找错误折叠为 `false`，而其他查询方法保留原错误。

## 数据与状态

`dbs` 与 `tables` 必须保持一个核心不变量：每个由 `PutSchema` 注册的数据库都应有同键的表分桶。`PutSchema` 用 `entry(key).or_default()` 建桶且不会清空已有表，因此用新 `DBInfo` 覆盖同名数据库时保留现有表集合。`DeleteSchema` 仅在 `dbs` 中确有该键时删除两侧状态；正常 API 路径下这会同时移除库及其全部表。

存储值均为拥有型 `DBInfo`/`TableInfo`，不是外部引用。查询引用的生命周期绑定到 `&self`；`TableClonedByName`、适配器 `TableByName` 和 `InfoStore::clone` 则生成独立拥有型值或快照。覆盖同一规范化键会替换旧元数据：在非零大小写模式下，仅大小写不同的名称会落到同一键。

名称枚举暴露的是 `HashMap` 键：模式 0 返回原始形式，非零模式返回小写形式。结果顺序由 `HashMap` 决定，不构成稳定 API；调用方若需要确定性输出应自行排序。Go 删除测试在两个元素场景中也显式排序后比较。

## 依赖与调用关系

上游生产关系经 RustCodeGraph 与源码核对如下：`NewSchemaTracker -> NewInfoStore`；`SchemaTracker` 的建库、删库、建表、删表、重命名及 alter 流程反复调用 `SchemaByName`、`TableByName`、`TableClonedByName`、`Put*` 和 `Delete*`；`Checker::InitFromIS -> InfoStore::InitFromIS`，而 checker 的库表比对读取 `SchemaByName` 与 `TableByName`。

主要下游边为 `InitFromIS -> InfoSchemaSource::{AllSchemas, SchemaTableInfos} -> PutSchema/PutTable`，以及所有命名 API `-> ciStr2Key`。`TableClonedByName`、`InfoStoreAdaptor::{TableExists, TableByName, TableInfoByName}` 都委托 `InfoStore::TableByName`，没有第二套查找规则。

本文件直接依赖 `std::collections::HashMap`、crate 根的 `Error`，以及经 crate 根再导出的 `ast`/`model`。Cargo manifest 还声明了 `astersql-ddl`、`astersql-expression-exprstatic` 与 `thiserror` 等 crate 级依赖，但本文件本身不直接调用它们。RustCodeGraph 未显示 `InfoStoreAdaptor` 在当前 Rust 生产文件中的外部调用；应把它视为已经公开的兼容适配边界，而不臆测其已接入完整运行主链。

## 错误处理与边界

缺少数据库时，`TableByName`、`PutTable`、`DeleteTable` 和 `AllTableNamesOfSchema` 返回 `Error::DatabaseNotExists`，错误中的名称使用调用参数的 `CIStr.O`。数据库存在但表缺失时，查询/删除返回 `Error::TableNotExists(schema.O, table.O)`。`SchemaByName` 用 `Option` 表示缺失，`DeleteSchema` 用 `bool` 表示是否删除成功；这些不同返回契约与 Go 文件一致。

写入是覆盖语义：`PutSchema` 和 `PutTable` 不报告同名冲突。是否允许对象已存在由 `SchemaTracker` 上层 DDL 语义决定。相反，`PutTable` 坚持数据库必须先存在，防止创建孤立表分桶。

`InitFromIS` 没有清空旧状态，也没有全有或全无保证。数据源加载某库失败，或理论上后续 `PutTable` 失败时，当前库和更早导入的内容仍可见；重试者必须理解覆盖与残留前缀语义。适配器 `TableExists` 有意丢弃错误种类，不能用于区分“库不存在”和“表不存在”。本模块也不校验 ID 唯一性、列/索引合法性或跨对象引用，这些属于上层 tracker/checker 的职责。

## 并发与资源生命周期

与 Go 注释中的“not thread-safe”对应，Rust 类型没有锁、原子变量、通道或后台任务。所有修改方法要求 `&mut self`，安全 Rust 借用规则会阻止同一实例在无额外同步时并发写；如需跨线程共享和修改，调用方必须自行提供 `Mutex`/`RwLock` 等同步，并决定读取一致性。

仓库完全驻留内存，值随 `InfoStore` 释放而释放，没有文件、网络连接、事务或显式关闭流程。`InfoSchemaSource` 借用仅持续一次初始化调用；本文件不保存数据源。`InitFromIS` 顺序调用数据源，未并行加载。克隆完整 `InfoStore` 或较大的 `TableInfo` 会产生与元数据规模相关的内存和复制成本，扩展热路径时应避免无必要的全量克隆。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ddl/schematracker/info_store.go`。Rust 保留了 Go 的两级 map、`lowerCaseTableNames` 键规则、覆盖写、删库级联、缺库/缺表区分、未排序名称枚举，以及 `InfoStoreAdaptor` 的四个查询入口。`InitFromIS` 也保持 Go 的操作顺序：先 `PutSchema`，再取该库表清单，失败即返回且不回滚。

语言适配差异包括：Go 存储指针，Rust 存储拥有型值；Go `TableByName` 接收未使用的 `context.Context`，Rust 核心方法不携带 context；Go 适配器把 `TableInfo` 包成 `tables.MockTableFromMeta`，Rust 当前仅包成 `TableHandle(TableInfo)`；Go 通过嵌入完整 `infoschema.InfoSchema` 补齐未实现方法，Rust 适配器只声明实际提供的局部接口。因此 Rust `InfoStoreAdaptor` 不能据此宣称实现了完整 InfoSchema。

`pkg/ddl/schematracker/info_store_test.go` 的大小写和删除用例在独立 Rust 文件 `info_store_test.rs` 中有对应覆盖。Rust 还增加了初始化加载失败后保留 schema 的回归测试，显式验证 Go 源码中由语句顺序形成但 Go 测试未单列的部分写入语义。

## 扩展指南

- 新增按名读写 API 时必须复用 `ciStr2Key`，并同时覆盖 `lowerCaseTableNames == 0` 与非零模式；不要直接用 `CIStr.O` 或 `CIStr.L` 绕过统一规则。
- 改变库表状态结构时应维持 `dbs`/`tables` 分桶不变量，并明确 `PutSchema` 覆盖时是否仍保留表、`DeleteSchema` 是否仍级联。对应回归测试应放在独立的 `pkg/ddl/schematracker/info_store_test.rs`，不要嵌入生产源文件。
- 若要让初始化具备清空、合并或原子回滚语义，应修改 `InitFromIS` 并同步评估 `Checker::InitFromIS` 调用方；这会偏离 Go 当前的前缀保留行为，必须同时更新 Go 对照判断和失败注入测试。
- 扩充适配器时先确认调用方真正需要的接口。若需要完整 InfoSchema/table 行为，应补真实 Rust 抽象，而不是假设 `TableHandle` 等价于 Go `table.Table`。
- 若引入并发共享，需在上层定义锁粒度、快照一致性与克隆成本；不要仅在单个方法内加锁而留下跨多个读改写步骤的竞态窗口。
- 改变名称枚举的排序属于可观察行为和性能权衡。需要稳定顺序时优先由调用方排序，除非 API 契约明确要求仓库保证顺序。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 11,467 个文件；`files --filter pkg/ddl/schematracker` 确认 Rust/Go 源和独立测试布局；`node --file pkg/ddl/schematracker/info_store.rs` 核对全部 192 行与 23 个索引符号。
- RustCodeGraph 调用证据：`explore` 显示 `NewInfoStore` 的生产调用者 `NewSchemaTracker`，以及 `TableByName` 被 `dm_tracker.rs` 的建表、分区、重命名等流程使用；`callees InitFromIS` 给出 Rust 边 `AllSchemas`、`SchemaTableInfos`、`PutSchema`、`PutTable`；`callees TableByName` 确认键转换与适配器委托关系。
- 已阅读源码/装配：`pkg/ddl/schematracker/info_store.rs`、`lib.rs`、`dm_tracker.rs` 的持有与初始化段、`checker.rs` 的直接引用，以及 `pkg/ddl/schematracker/Cargo.toml`。
- 已阅读对照与测试：`pkg/ddl/schematracker/info_store.go`、`info_store_test.go`、`info_store_test.rs`。它们共同覆盖大小写模式、覆盖/查找、缺库/缺表、删除级联，以及初始化失败的部分写入。
- 人工边界复核：该文件是 metadata-only 的内存模拟组件，不创建 DDL job，不经历 schema state、reorg/backfill、schema version sync、持久化、取消或 GC 生命周期；文档未把 DDL 框架概览误写为本文件行为。
