# `pkg/infoschema/context/infoschema.rs`

源文件：[`infoschema.rs`](./infoschema.rs)

## 文件定位

本文件属于独立 crate `astersql-infoschema-context`，crate 边界由 [`Cargo.toml`](./Cargo.toml) 定义，入口 [`lib.rs`](./lib.rs) 将本模块全部公开导出。它位于完整 InfoSchema 实现与上层 SQL 组件之间，提供两类低耦合契约：一类是对 `TableInfo` 的特殊属性过滤器及其分组结果，另一类是只暴露元数据查询能力的 `MetaOnlyInfoSchema` trait 族。这样，表达式、规划、表会话等调用方可以依赖窄接口，而不必依赖完整 `pkg/infoschema` 实现。

本 crate 只直接依赖 `astersql-meta-model` 与 `astersql-ddl-placement`；`ast`、`model` 和 `placement` 名称由 [`lib.rs`](./lib.rs) 再导出。因此，本文件定义的是元数据边界和纯查询逻辑，不负责加载、持久化或更新 InfoSchema。

## 核心职责

1. 以 `SpecialAttributeFilter = fn(&model::TableInfo) -> bool` 统一表达表属性筛选条件。
2. 实现 TTL、TiFlash、放置策略、表锁、分区和亲和性六类筛选，并由 `HasSpecialAttributes` 按固定短路顺序组合。
3. 用 `TableInfoResult` 表达“一个 schema 名及其匹配表列表”，供完整 InfoSchema 的枚举实现返回结果。
4. 用 `SchemaAndTable`、`Misc` 和组合 trait `MetaOnlyInfoSchema` 描述元数据只读能力，避免上层组件绑定完整 InfoSchema 类型。
5. 用 `DBInfoAsInfoSchema` 将一组 `DBInfo` 轻量适配为 `SchemaAndTable`，主要服务测试或只需遍历库表的场景。

本文件没有缓存构建、DDL 变更、版本推进或权限判断逻辑；这些能力属于具体 InfoSchema 实现或其调用方。

## 主要符号

- `SpecialAttributeFilter`：普通函数指针，而非捕获环境的闭包或 trait object；调用方只能传入签名为 `fn(&TableInfo) -> bool` 的无状态函数。
- `TTLAttribute`：仅当 `TableInfo.State == StatePublic` 且 `TTLInfo` 存在时返回 `true`。public 状态检查是 TTL 特有的可见性约束。
- `TiFlashAttribute`、`TableLockAttribute`、`AffinityAttribute`：分别检查 `TiFlashReplica`、`Lock`、`Affinity` 是否存在。
- `PlacementPolicyAttribute`：先查表级 `PlacementPolicyRef`，再通过 `GetPartitionInfo()` 查启用中的分区定义；分区未启用时不会看到分区级策略。
- `AllPlacementPolicyAttribute`：表级检查相同，但直接读取 `TableInfo.Partition`，刻意忽略 `PartitionInfo.Enable`，用于需要覆盖禁用分区定义的场景。
- `PartitionAttribute`：以 `GetPartitionInfo().is_some()` 判断当前是否为启用的分区表。
- `HasSpecialAttributes` / `AllSpecialAttribute`：前者依次对 TTL、TiFlash、放置策略、分区、表锁、亲和性求逻辑或；后者是指向前者的公开常量函数指针。注意组合条件使用 `PlacementPolicyAttribute`，不使用 `AllPlacementPolicyAttribute`。
- `TableInfoResult`：包含 `DBName: ast::CIStr` 和 `TableInfos: Vec<Arc<TableInfo>>`；按 schema 分组，同时共享而非深拷贝表元数据。
- `SchemaAndTable`：声明关联类型 `Context`、`Error`，以及 `AllSchemas`、`SchemaTableInfos` 两个遍历入口。
- `Misc`：集中策略、资源组、脱敏策略、placement bundle 和临时表状态等旁路查询。
- `MetaOnlyInfoSchema`：以 `SchemaAndTable + Misc` 为 supertrait，并补充版本、按名/ID 查库表、分区反查、简单表信息、特殊属性枚举和被引用外键查询。
- `DBInfoAsInfoSchema`：`Vec<Arc<DBInfo>>` 的新类型包装；其 `Context = ()`、`Error = Infallible`。

## 执行流程

特殊属性枚举的典型流程是：具体 InfoSchema 实现遍历当前可见的 schema/表，将每个 `Arc<TableInfo>` 借用给某个 `SpecialAttributeFilter`；命中的表按 schema 组装为 `TableInfoResult`。例如 `pkg/infoschema/infoschema.rs` 的 V1 默认实现从 `AllSchemas()` 取表后调用过滤器，`pkg/infoschema/infoschema_v2.rs` 则先在读锁下选出当前版本并应用过滤器，释放锁后排序、分组。`pkg/lock/lock.rs` 直接传入 `TableLockAttribute` 来枚举带表锁的表。

`HasSpecialAttributes` 从 `TTLAttribute` 开始顺序短路：任何条件命中即结束；只有前一条件失败才检查下一项。放置策略的两个版本都先检查表级引用，再逐个扫描分区定义，因此最坏复杂度与分区数线性相关。

`DBInfoAsInfoSchema::AllSchemas` 克隆内部 `Vec` 及其中的 `Arc`，不会复制 `DBInfo` 内容。`SchemaTableInfos` 线性扫描库列表，以 `CIStr` 相等判断命中；命中后克隆 `Deprecated.Tables` 中的 `Arc` 列表，未命中返回 `Ok(Vec::new())`。

`MetaOnlyInfoSchema`、`SchemaAndTable` 和 `Misc` 本身不执行查询，只规定具体实现必须提供的调用入口与返回形状。上层如 `pkg/expression/expropt/infoschema.rs`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/planner/planctx/context.rs`、`pkg/table/tblctx/table.rs` 通过这些 trait 约束读取会话或规划所需的元数据。

## 数据与状态

过滤器只借用一份 `TableInfo`，没有内部可变状态。`TableInfo` 中的 `Option` 字段表达属性是否存在；`StatePublic` 和 `PartitionInfo.Enable` 则额外控制 TTL 与启用分区的可见性。`CIStr` 保留原始形式与大小写不敏感形式，`GetTableReferredForeignKeys` 明确要求调用方传入小写 schema/table 字符串。

跨接口返回的元数据主要使用 `Arc`：`DBInfo`、`TableInfo`、策略、资源组、脱敏策略、分区定义和 placement bundle 都可在多个只读消费者之间共享。`ClonePlacementPolicies` 与 `CloneResourceGroups` 返回新的 `HashMap` 容器，但 value 仍是共享的 `Arc`。trait 不承诺返回集合的排序、去重方式或快照一致性；这些由具体实现决定。

`DBInfoAsInfoSchema` 自身只保存一个 schema 向量，没有索引、版本号或锁。它读取 `DBInfo.Deprecated.Tables`，这是当前适配器与元模型的明确耦合点。

## 依赖与调用关系

下游依赖只有三组再导出类型：`crate::ast::CIStr` 负责标识符，`crate::model` 提供全部数据库元数据，`crate::placement::Bundle` 表示 placement rule bundle；标准库提供 `Arc`、`HashMap` 和 `Infallible`。

RustCodeGraph 将本文件识别为 42 个符号，并显示被 15 个文件使用。直接可复核的生产调用包括：

- `pkg/lock/lock.rs` 调用 `ListTablesWithSpecialAttribute(TableLockAttribute)`。
- `pkg/infoschema/infoschema.rs` 与 `pkg/infoschema/infoschema_v2.rs` 接受 `SpecialAttributeFilter` 并产生 `TableInfoResult`。
- `pkg/expression/expropt/infoschema.rs` 以 `MetaOnlyInfoSchema` 约束可选求值属性提供者。
- `pkg/expression/sessionexpr/sessionctx.rs`、`pkg/planner/planctx/context.rs`、`pkg/table/tblctx/table.rs` 将关联的 InfoSchema 类型限制为该窄接口。
- `pkg/meta/metabuild/context.rs` 用 `Arc<dyn MetaOnlyInfoSchema<Context = C, Error = E>>` 保存构建上下文中的 InfoSchema 引用。

Cargo 反向引用还覆盖 session、DDL、executor、distsql、TTL、statistics、server handler 等 crate；依赖本 crate 不等于每个 crate 都直接调用本文件的每个符号，应以各自源码中的 import 和 trait bound 为准。

## 错误处理与边界

布尔过滤器不返回错误，并假定传入的 `TableInfo` 是有效引用。它们不会验证属性内容是否合法，也不会解析 placement policy 或检查 TiFlash 副本状态；只判断字段存在性及少量可见性标志。

`SchemaAndTable::Error` 是实现者定义的关联类型，查询上下文也是 `Context: ?Sized`，因此生产实现可以接入可取消上下文及自己的错误类型。`MetaOnlyInfoSchema::TableInfoByName` 和两个表列表查询显式传播错误；按 ID/名称的其他查询多用 `Option` 表达未命中。调用方必须区分 `None` 与 `Err`，不能把未命中统一当作查询失败。

`DBInfoAsInfoSchema` 使用 `Infallible`，不会产生查询错误；未命中 schema 返回空向量。这与 Go 适配器返回 `nil, nil` 在“无结果且无错误”的语义上一致，但 Rust 调用方观察到的是非空指针语义的空 `Vec`。线性查找只返回第一个同名 schema；本文件不检测重复 schema。

## 并发与资源生命周期

本文件不创建线程、异步任务、通道、事务或锁，也不持有外部资源。过滤器是同步、只读、无状态函数；其生命周期仅限于一次借用调用。

`Arc` 让返回的元数据可以跨所有者共享并延长生命周期，但本文件没有为 trait 添加 `Send`/`Sync` 上界，也没有定义内部可变性的同步规则。需要跨线程使用的调用方会自行叠加约束，例如 `pkg/expression/expropt/infoschema.rs` 要求 `MetaOnlyInfoSchema + Send + Sync + 'static`。具体 InfoSchema 的锁粒度、快照版本与缓存淘汰不属于本接口保证。

`DBInfoAsInfoSchema` 的克隆操作只增加 `Arc` 引用计数；返回集合销毁时引用计数递减，底层元数据在最后一个 `Arc` 释放后回收。

## 与 Go 版本的对应关系

直接对照文件是 [`infoschema.go`](./infoschema.go)。Rust 保留了 Go 的符号命名、过滤顺序、TTL public 限制、`GetPartitionInfo` 的启用分区语义，以及 `AllPlacementPolicyAttribute` 忽略 `Partition.Enable` 的差异。

主要语言映射如下：Go 的包级函数变量映射为公开函数，`AllSpecialAttribute` 映射为 `const` 函数指针；Go 指针切片映射为 `Vec<Arc<T>>`；`nil`/布尔命中结果映射为 `Option`；Go 的 `context.Context` 与 `error` 分别抽象为 `SchemaAndTable::Context` 和 `Error` 关联类型。Rust 将 Go 接口嵌入明确表达为 `MetaOnlyInfoSchema: SchemaAndTable + Misc`。

`DBInfoAsInfoSchema` 的 Go 版本是 `[]*DBInfo` 的定义类型，Rust 是 `Vec<Arc<DBInfo>>` 的元组结构体。二者都按名称线性查找并返回 `Deprecated.Tables`；Go 未命中返回 `nil, nil`，Rust 返回 `Ok(Vec::new())`。Rust 独立测试还确认返回值与输入共享同一 `Arc` 指向的元数据对象。

## 扩展指南

新增一种特殊属性时，应先决定它是否属于 `HasSpecialAttributes` 的“常用属性”集合：若属于，新增同签名过滤函数并把它插入具有明确兼容顺序的位置；若需忽略可见性或 Enable 标志，像两种 placement 过滤器一样提供名称清晰的独立入口，不要暗改现有过滤器。同步扩展 [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，至少覆盖属性不存在、存在、可见性/Enable 边界和组合过滤器。

扩展元数据查询能力时，应把核心库表遍历放在 `SchemaAndTable`，策略/资源组等旁路能力放在 `Misc`，其余仅元数据通用查询放在 `MetaOnlyInfoSchema`。新增必需方法会影响所有实现者和测试桩，应先用 RustCodeGraph/`rg` 枚举 `impl MetaOnlyInfoSchema`、`impl SchemaAndTable`、`impl Misc` 以及 trait object 类型别名；同时对齐 [`infoschema.go`](./infoschema.go) 的接口意图。若方法可能失败，应使用实现者的 `Self::Error`，不要以空集合掩盖错误。

修改返回集合或 `Arc` 策略前，要评估 V1/V2 枚举实现、顺序兼容、元数据复制成本和跨线程约束。测试逻辑必须继续放在独立的 `migration_aster_unit_test.rs` 等测试文件中，不应嵌入生产源文件。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件，目标 `infoschema.rs` 含 42 个符号；`files --filter pkg/infoschema/context` 确认 Go/Rust 对照、crate 入口和独立测试；`node --file ...` 读取了目标文件全部 223 行；`query` 确认 `MetaOnlyInfoSchema`、`SchemaAndTable`、`Misc`、`DBInfoAsInfoSchema` 和 `HasSpecialAttributes` 的目标定义。精确 `callers/callees` 未返回这些 trait/函数的可用明细，因此引用关系用 `rg` 及图的 `explore` 结果补证。
- 源码与 crate：[`infoschema.rs`](./infoschema.rs)、[`lib.rs`](./lib.rs)、[`Cargo.toml`](./Cargo.toml)。目标包不存在 `doc.go`。
- Go 对照：[`infoschema.go`](./infoschema.go)，逐项核对过滤器、trait 对应接口、结果结构与适配器行为。
- 独立 Rust 测试：[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs)，覆盖 TTL public 条件、TiFlash、表级/分区级 placement、禁用分区差异、表锁、亲和性、组合过滤器、schema 命中/未命中及 `Arc` 共享身份。
- 生产调用证据：`pkg/infoschema/infoschema.rs`、`pkg/infoschema/infoschema_v2.rs`、`pkg/lock/lock.rs`、`pkg/expression/expropt/infoschema.rs`、`pkg/expression/sessionexpr/sessionctx.rs`、`pkg/planner/planctx/context.rs`、`pkg/table/tblctx/table.rs`、`pkg/meta/metabuild/context.rs`。
- 本任务只新增说明文档，按计划不运行 Cargo；交付前以任务指定命令验证恰有 11 个固定二级章节，并人工复核没有把 trait 契约描述成具体实现保证。
