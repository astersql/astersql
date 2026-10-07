# `pkg/meta/model/db.rs`

## 文件定位

本文件定义持久化层使用的数据库（SQL schema）元数据模型，而不是 `pkg/infoschema/infoschema.rs` 中面向查询期缓存的同名 `DBInfo`。它由 `pkg/meta/model/internal/group1/lib.rs` 通过 `#[path = "../../db.rs"] mod db` 编入 `astersql-meta-model-group1`，再经 group1 和根 `pkg/meta/model/lib.rs` 的公开再导出成为 `astersql_meta_model::DBInfo`。`pkg/meta/model/Cargo.toml` 表明根 crate 名为 `astersql-meta-model`，并以内部 group1–group4 crate 组织 Go 模型的移植边界；其中本文件属于拥有完整表模型身份的 group1。

在完整应用中，`DBInfo` 是数据库记录在元存储、DDL 参数和 InfoSchema 装载链之间传递的库级载体。直接证据包括：`pkg/meta/model/metadata_codec.rs::{EncodeDBInfo, DecodeDBInfo}` 负责 JSON 字节转换；`pkg/meta/reader.rs::{get_database, list_databases, create_database, update_database}` 从 `DBs` hash 读写它；`pkg/domain/canonical_domain.rs`、`pkg/infoschema/issyncer/loader.rs` 和 `pkg/ddl/persistent_actions.rs` 继续消费该模型。

## 核心职责

- `DBInfo` 保存数据库 ID、大小写不敏感名称、默认字符集/校对规则、DDL schema 状态和可选 placement policy 引用（`pkg/meta/model/db.rs::{DBInfo}`）。
- `DeprecatedDBInfo` 保留历史内嵌表列表，但将其排除在 JSON 之外；源码注释和 Go 对照均说明 InfoSchema v2 不设置该列表，应使用 InfoSchema 的表查询路径（`DeprecatedDBInfo::Tables`、`pkg/meta/model/db.go`）。
- `TableName2ID` 是进程内按小写表名查 ID 的派生索引，不进入持久化 JSON（`DBInfo::TableName2ID`）。
- `DBInfo::Clone` 为历史表列表提供表对象级深拷贝；`DBInfo::Copy` 提供共享 `Arc<TableInfo>` 的拷贝；`LessDBInfo` 以规范化小写库名提供三路比较结果。
- `Serialize`/`Deserialize` 与字段重命名保持 Go JSON 字段兼容，`#[serde(default)]` 允许旧数据或空对象缺少新增字段时按默认值解码（`DBInfo`、`DeprecatedDBInfo`，以及 `pkg/meta/model/db_test.rs::db_info_json_matches_go_field_names_and_omissions`）。

## 主要符号

- `pub struct DeprecatedDBInfo`：只有 `Tables: Vec<Arc<TableInfo>>`。结构体和字段容器可克隆，但 `#[serde(skip)]` 使表列表既不编码也不从输入恢复；反序列化后使用默认空列表。
- `pub struct DBInfo`：
  - `ID: i64` 对应 JSON `id`；`Name: ast::CIStr` 对应 `db_name`，同时保存原串 `O` 与小写串 `L`。
  - `Charset`、`Collate` 对应 JSON `charset`、`collate`。
  - `Deprecated` 没有 rename；编码时保留 `Deprecated` 对象本身，但其中 `Tables` 被跳过，因此当前测试期望其为 `{}`。
  - `State: SchemaState` 对应 JSON `state`。group1 中 `SchemaState` 是透明 `u8` 包装，`Public` 值为 5。
  - `PlacementPolicyRef: Option<PolicyRefInfo>` 对应 `policy_ref_info`，记录策略 ID 与 `CIStr` 名称。
  - `TableName2ID: HashMap<String, i64>` 使用 `#[serde(skip)]`，仅存在于内存。
- `pub fn DBInfo::Clone(&self) -> Self`：先派生克隆全部字段，再逐个调用 `TableInfo::Clone`，将每张表包装为新的 `Arc`，从而替换 `Deprecated.Tables`。
- `pub fn DBInfo::Copy(&self) -> Self`：直接调用派生 `Clone`；向量容器独立，但其中的 `Arc<TableInfo>` 仍指向相同表对象。
- `pub fn LessDBInfo(a: &DBInfo, b: &DBInfo) -> i32`：比较 `a.Name.L` 与 `b.Name.L`，严格返回 `-1`、`0` 或 `1`。

本文件没有 trait、模块级常量、条件编译项或内部私有函数；上述类型与函数均通过 group1 的 `pub use db::*` 对外公开。

## 执行流程

1. 创建或恢复数据库时，上层构造 `DBInfo`，填写持久化字段；`TableName2ID` 和 `Deprecated.Tables` 可按运行期需要填充，但不会进入 JSON。
2. 写元存储时，`pkg/meta/model/metadata_codec.rs::EncodeDBInfo` 把模型交给 `ast::metadata_json::encode`。例如 `pkg/meta/reader.rs::create_database` 检查 ID 不存在后，把结果写入 `DBs` hash 的 `DB:<id>` 字段；`update_database` 在确认记录存在后覆盖该字段。
3. 读取时，`pkg/meta/reader.rs::get_database` 或 `list_databases` 取得字节并调用 `DecodeDBInfo`。缺失字段由 `#[serde(default)]` 补为 `Default`；被跳过的两个运行期容器为空。
4. 需要隔离表元数据修改时调用 `DBInfo::Clone`：普通字段先克隆，随后每个历史表经 `TableInfo::Clone` 产生新对象和新 `Arc`。需要保留表对象共享时调用 `Copy`。
5. 需要按库名排序时调用 `LessDBInfo`。比较只使用 `CIStr::L`，因此 `Zoo` 与 `zoo` 比较为相等；它不以 ID 或原始大小写打破平局。

## 数据与状态

`DBInfo` 本身是一个普通拥有型值，没有内部可变性。持久化数据可分为三组：身份与显示属性（`ID`、`Name`、`Charset`、`Collate`）、DDL 生命周期（`State`）、以及策略关联（`PlacementPolicyRef`）。`SchemaState` 的合法命名状态在 group1 入口中为 None、DeleteOnly、WriteOnly、WriteReorganization、DeleteReorganization、Public、ReplicaOnly、GlobalTxnOnly；本文件只保存状态，不推进状态机或校验转换。

`Deprecated.Tables` 和 `TableName2ID` 是明确的非持久化状态。JSON 往返不会保存它们，调用者必须从表元数据/InfoSchema 重新建立所需内容。`#[serde(default)]` 还意味着 `{}` 可解码为 ID 0、空名称/字符集/校对规则、默认 None 状态、无策略引用及两个空容器；这由 `pkg/meta/model/db_test.rs` 直接验证。

拷贝的不变量如下：`Clone` 后 `Deprecated.Tables` 中的每个 `Arc` 与源对象不指向同一分配，并依赖 `TableInfo::Clone` 继续深拷贝表内部需要隔离的内容；`Copy` 后向量是新容器，但对应表 `Arc` 指针相等。`pkg/meta/model/bdr_1_aster_unit_test.rs::db_clone_is_deep_copy_while_copy_shares_tables` 和 `pkg/meta/model/table_test.rs` 对这些性质有断言。

## 依赖与调用关系

本文件的直接依赖都来自 group1 同一模型边界：`ast::CIStr` 表示大小写不敏感标识符，`SchemaState` 表示 DDL 状态，`PolicyRefInfo` 表示 placement policy 引用，`TableInfo` 及其 `Clone` 实现提供表级深拷贝；标准库 `Arc` 和 `HashMap` 分别承载共享表与名称索引。Serde derive/属性定义持久化格式。相应依赖由 `pkg/meta/model/internal/group1/Cargo.toml` 中的 `parser-ast`、`serde`（含 `derive`、`rc`）等声明提供。

已核实的上游/下游链包括：

- `pkg/meta/model/metadata_codec.rs` 直接以 `DBInfo` 为输入/输出，并下调 `ast::metadata_json`；这是本模型的编码边界。
- `pkg/meta/reader.rs` 通过 codec 在事务快照和可写事务中获取、枚举、创建、更新数据库记录。
- `pkg/domain/canonical_domain.rs`、`pkg/infoschema/issyncer/{lib.rs,loader.rs}` 对 DBInfo 编解码，并转换到各自的运行期 InfoSchema 表示。
- `pkg/meta/model/job.rs::HistoryInfo::add_db_info` 保存 `Arc<DBInfo>`，`pkg/meta/model/job_args.rs::{RecoverSchemaInfo, CreateSchemaArgs}` 把 DBInfo 放入 DDL job 参数。
- `pkg/meta/model/internal/group3/lib.rs` 从 group1 再导出同一 `DBInfo` 身份，避免维护第二套结构；根 crate 最终公开 group1。

RustCodeGraph 的文件节点报告 `pkg/meta/model/db.rs` 被 9 个文件使用，并能定位本文件 9 个符号；但本次精确 `callers/callees` 命令在 30 秒内没有返回可用边，且按内部 symbol ID 的 `node` 导航发生错配。因此以上调用关系只采用随后通过源码引用核实的直接边，不把图工具的模糊候选当作事实。

## 错误处理与边界

本文件自身不返回 `Result`，也不主动报错。`Clone`/`Copy` 的常规失败面仅是内存分配失败；Rust 标准分配失败不由这里恢复。`LessDBInfo` 对任意 `CIStr::L` 都有确定结果，但不检查 `L` 是否确实由 `O` 规范化而来。

序列化错误在外层 codec 变为 `Result<_, String>`；元存储读取、写入及“数据库已存在/不存在”错误由 `pkg/meta/reader.rs` 处理。因为 `serde(default)` 接受缺字段输入，结构完整性（例如非零 ID、支持的字符集/校对规则、placement policy 是否存在）必须由更高层验证，不能假定反序列化成功即业务有效。

兼容边界包括：`Deprecated.Tables` 与 `TableName2ID` 永远不随 JSON 往返；未知的 `SchemaState` 数值可由透明数值表示承载，但本文件不会解释其业务意义；`LessDBInfo` 只给名称比较结果，不保证集合中的名称唯一。修改字段名、跳过规则或默认值会直接影响 Go 元数据兼容。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务或 I/O 资源，也没有 `Drop` 生命周期逻辑。并发共享只通过 `Arc<TableInfo>` 表达：`Copy` 和派生克隆会增加强引用计数，所有副本释放后表对象才回收；`Clone` 为历史列表创建新表对象，因此后续基于拥有型替换的修改不会共享该表分配。

`DBInfo` 的 `HashMap`、字符串和策略引用都是拥有型字段。Rust 派生 `Clone` 会为这些字段建立独立的拥有型副本；并发读写策略仍由外层容器（如 InfoSchema 的 `Arc`、锁或事务）负责。特别是，`Arc<TableInfo>` 只提供共享所有权，不在本文件内提供可变并发访问。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/meta/model/db.go`。字段和 JSON 名称逐项对应：`ID/id`、`Name/db_name`、`Charset/charset`、`Collate/collate`、`State/state`、`PlacementPolicyRef/policy_ref_info`；Go 的 `Deprecated.Tables` 与 `TableName2ID` 使用 `json:"-"`，Rust 对应字段使用 `serde(skip)`。Go `strings.Compare(a.Name.L, b.Name.L)` 与 Rust `cmp` 后映射为 `-1/0/1` 的结果一致。

`Clone` 对历史表列表的意图一致：都分配新列表并逐表调用表级 `Clone`。Rust 测试进一步用 `Arc::ptr_eq` 验证表对象不共享；Go `pkg/meta/model/table_test.go` 验证克隆值相等。`Copy` 的表列表语义也对应：容器被复制而表对象仍共享。

仍需注意语言所有权造成的精确差异。Go 的结构体浅复制会共享 `TableName2ID` map 和 `PlacementPolicyRef` 指针；Rust 的 `self.clone()` 会复制 `HashMap` 内容并按值复制 `Option<PolicyRefInfo>`。Rust 字符串也拥有独立缓冲，而 Go 字符串值复制可共享不可变底层数据。因此 Rust `Copy` 的可靠契约应表述为“历史表 `Arc` 浅共享”，不能扩大为所有字段都具备 Go 指针级浅共享。当前测试覆盖 JSON、表深/浅复制和大小写无关比较，未直接覆盖这一跨语言引用身份差异。

## 扩展指南

- 新增持久化字段时，应在 `DBInfo` 上明确 Go 兼容的 serde 名称与默认行为，同步 `pkg/meta/model/db.go` 的真实语义，并扩展独立的 `pkg/meta/model/db_test.rs` JSON 往返测试；不要把测试写入本源文件。
- 新增仅运行期缓存时，应像 `TableName2ID` 一样明确 `serde(skip)`，并记录由哪个装载路径重建，避免读取旧元数据后静默缺少必需状态。
- 改动复制语义时，应同时更新 `DBInfo::{Clone, Copy}`，在 `pkg/meta/model/db_test.rs` 或现有独立迁移测试中验证容器与元素的指针身份；还要评估 `TableInfo::Clone` 的成本以及 Go/Rust 在 map、指针字段上的所有权差异。
- 改动排序时应修改 `LessDBInfo` 并覆盖小于、等于、大于和仅大小写不同的名称。若增加平局键，必须先确认 Go 调用方和稳定排序约定，避免破坏跨语言顺序。
- 改动 JSON 形状后应检查 `metadata_codec.rs`、`meta/reader.rs`、`domain/canonical_domain.rs` 和 `infoschema/issyncer` 的编解码消费者；持久化兼容性风险高于本地结构调整。
- 大量数据库或含历史表列表的对象执行 `Clone` 会逐表深拷贝，成本与表元数据总量相关；若性能敏感，应先确认调用者是否只需要 `Copy` 或只读 `Arc`，不能为了性能把深拷贝契约静默改浅。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/meta/model/db.rs` 定位目标并报告 9 个符号；`query` 定位 `DBInfo`、`DeprecatedDBInfo`、`Clone`、`Copy`、`LessDBInfo`。精确调用边查询的限制已在“依赖与调用关系”中披露。
- 目标源码：`pkg/meta/model/db.rs`（全部 88 行），用于核实字段、serde 属性、复制流程和比较逻辑。
- crate 与模块边界：`pkg/meta/model/Cargo.toml`、`pkg/meta/model/lib.rs`、`pkg/meta/model/internal/group1/{Cargo.toml,lib.rs}`；目标包不存在 `pkg/meta/model/doc.go`，因此没有可读取的 Go 包级契约文件。
- Go 对照：`pkg/meta/model/db.go`；补充 Go 克隆断言来自 `pkg/meta/model/table_test.go`。
- 独立 Rust 测试：`pkg/meta/model/db_test.rs` 验证 JSON 字段、跳过项和缺字段默认值；`pkg/meta/model/bdr_1_aster_unit_test.rs` 验证深/浅表引用与小写名称比较；`pkg/meta/model/table_test.rs` 再验证深克隆表引用。
- 直接集成证据：`pkg/meta/model/metadata_codec.rs`、`pkg/meta/reader.rs`，以及源码检索确认的 `pkg/domain/canonical_domain.rs`、`pkg/infoschema/issyncer/{lib.rs,loader.rs}`、`pkg/meta/model/{job.rs,job_args.rs}` 引用。
- 本任务是纯文档分析，按计划未运行 Cargo。交付前执行任务指定的 11 章节结构检查，并人工复核本文只描述有上述文件或符号支持的当前行为。
