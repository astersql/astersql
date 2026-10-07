# `pkg/ddl/notifier/events.rs`

## 文件定位

本文件属于 `astersql-ddl-notifier` crate，是 DDL schema-change 通知的事件模型与 JSON 线协议实现。crate 入口 `pkg/ddl/notifier/lib.rs` 将本文件的公开项整体再导出，并把 `meta-model` 暴露为 `model`、把 `parser-ast` 暴露为 `ast`；`pkg/ddl/notifier/Cargo.toml` 表明本文件直接依赖这两个元数据 crate，以及 `serde`、`serde_json`。它不执行 DDL 状态机，也不负责订阅调度：上游 DDL worker 构造事件，`publish.rs`/`store.rs` 负责持久化，订阅者再按事件类型读取载荷。

在当前 Rust 生产接线中，`pkg/ddl/persistent_create_table.rs`、`persistent_create_materialized_view.rs`、`persistent_create_materialized_view_log.rs`、`persistent_modify_column.rs`、`persistent_masking_actions.rs` 和 `persistent_mview_out_of_place_cutover.rs` 已直接调用本文件的部分构造器。`pkg/ddl/persistent_actions.rs::async_notify_event` 排除系统库、解析 multi-schema 子任务编号后，通过 `PubSchemaChangeInTransaction` 把事件写入 `mysql.tidb_ddl_notifier`。其余构造器仍完整保留 Go 兼容 API，但不能仅凭定义推断所有 DDL 路径都已在 Rust worker 中接线。

## 核心职责

1. 用 `SchemaChangeEvent` 隐藏内部 `JsonSchemaChangeEvent`，让调用方通过 `GetType`、类型专用 getter 和 `NewXxxEvent` 构造器访问事件。
2. 为建表、截断/删除表、增改列、分区变更、加索引、删库、集群闪回和物化视图元数据变更编码各自所需的元数据快照。
3. 通过 `WireEvent` 明确定义与 Go `jsonSchemaChangeEvent` 一致的 JSON 字段名、默认值和 `omitempty` 行为；完整的 `TableInfo`、`PartitionInfo`、`ColumnInfo`、`IndexInfo` 由 `meta-model` 自身的 serde 实现承载。
4. 为日志和诊断提供稳定的 `String`/`Display`/`Debug` 表示，并为存储层提供 `MarshalJSON`、`UnmarshalJSON` 和复用读取槽位所需的 `overwrite_from`。
5. 对删库事件使用 `MiniDBInfoForSchemaEvent` 及其表/分区子结构，只保存 ID、名称和分区列表，避免把每张表的完整元数据写入通知记录。

## 主要符号

- `SchemaChangeEvent { inner: Option<JsonSchemaChangeEvent> }`：公开包装类型。`Default` 产生 `inner == None`；`GetType` 将这种状态视为 `ActionNone`。`PartialEq` 比较双方 `MarshalJSON` 的结果，`Display` 输出 `String()`，`Debug` 也以该字符串为主体。
- `JsonSchemaChangeEvent`：crate 内部统一载荷，字段包括 `MiniDBInfo`、新旧表、增删分区、列、索引、`Analyzed`、分区转换前表 ID 与 `Tp`。它不是线协议类型，避免直接把 Rust 字段命名规则泄露到持久化格式。
- `WireEvent`、`MiniDbWire`、`MiniTableWire`、`MiniPartitionWire`：serde 线格式。`type`、`Analyzed`、`old_table_id_for_partition` 等名称与 Go JSON tag 对齐，空向量、`None`、`false` 和零值按字段规则省略。
- `String` / `action_name`：按事件类型、新旧表、旧表 ID、增删分区、列、索引的固定顺序拼接诊断文本；分区名称为空时只输出 ID。`action_name` 委托 `model::group_3::action_type_string`，因此未知于本文件但已被模型登记的 action 仍使用模型名称。
- `expect(expected)`：所有类型专用 getter 的共同前置检查。空事件会以 `nil SchemaChangeEvent` panic，类型不匹配会以 `unexpected schema change event type` panic；成功后 getter 克隆所需元数据返回，不暴露内部可变引用。
- `event_constructor!`：生成只需设置 action 与若干字段的公开构造器。手写构造器用于还需携带 `Analyzed`、旧表 ID、压缩删库元数据或无载荷的场景。
- 表/列/索引 API：`NewCreateTableEvent`/`GetCreateTableInfo`，`NewTruncateTableEvent`/`GetTruncateTableInfo`，`NewDropTableEvent`/`GetDropTableInfo`，`NewAddColumnEvent`/`GetAddColumnInfo`，`NewModifyColumnEvent`/`GetModifyColumnInfo`，`NewAddIndexEvent`/`GetAddIndexInfo`。
- 分区 API：`NewAddPartitionEvent`、`NewTruncatePartitionEvent`、`NewDropPartitionEvent`、`NewExchangePartitionEvent`、`NewReorganizePartitionEvent`、`NewAddPartitioningEvent`、`NewRemovePartitioningEvent` 及对应 getter。增/移除分区化额外保存 `OldTableID4Partition`，用来标识转换前的物理表。
- 其他 API：`NewFlashbackClusterEvent` 仅保存 action；`NewDropSchemaEvent` 构造精简库表树；四个 materialized-view 构造器同时保存新旧完整表，其中 cutover getter 返回两者，另外三个 getter 与 Go 一样只返回新表。

## 执行流程

典型生产流程如下：

1. DDL worker 在元数据变更达到应通知的阶段后，从当前表、列、索引或分区元数据构造 `SchemaChangeEvent`。例如 RustCodeGraph 与源码搜索确认 `persistent_create_table.rs` 调用 `NewCreateTableEvent`，`persistent_modify_column.rs` 调用 `NewModifyColumnEvent`，`persistent_masking_actions.rs` 调用删表/截断表构造器。
2. worker 将事件交给 `persistent_actions.rs::async_notify_event`。该函数对内存库/系统库直接返回；普通 job 使用 `sub_job_id == -1`，multi-schema job 可回退到 `MultiSchemaInfo.seq`。
3. `PubSchemaChangeInTransaction` 构造 `SchemaChange`，再由 `InsertSchemaChangeSQL` 调用 `SchemaChangeEvent::MarshalJSON`。JSON 字节与 job ID、sub-job ID 一起在 worker 已存在的事务中插入 notifier 表；本文件自身不开始或提交事务。
4. `store.rs` 列表读取路径从 SQL 行或内存表取回 JSON，以默认事件调用 `UnmarshalJSON`，再通过 `overwrite_slot` 更新调用方缓冲区。已有槽位使用 `overwrite_from`，同时刷新 job ID、sub-job ID 和处理位图。
5. 订阅方先调用 `GetType` 分派，再调用匹配 getter。Go 的 `pkg/statistics/handle/ddl/subscriber.go::handle` 展示了完整消费模式：建表、表/分区变更、列变更和删库都在 action 分支内取出对应载荷并更新统计元数据。

序列化时 `JsonSchemaChangeEvent -> WireEvent -> serde_json::to_vec`；反序列化时先读成 `Option<WireEvent>`。JSON `null` 会转成一个 `Some(JsonSchemaChangeEvent::default())`，因此随后表现为类型 `ActionNone`、字符串 `(Event Type: none)`；初始的真正空包装则输出 `nil SchemaChangeEvent`。

## 数据与状态

事件保存的是构造时的拥有型快照：表、列、索引和分区由 `Box`/`Vec` 持有，getter 返回 clone。调用方修改 getter 返回值不会反向修改事件，但完整元数据的克隆和 JSON 编解码成本随表结构大小增长。

字段组合由 action 决定而非 Rust 枚举变体强制：例如截断表使用 `TableInfo + OldTableInfo`，交换分区使用 `TableInfo + AddedPartInfo + OldTableInfo`，重组分区使用 `TableInfo + AddedPartInfo + DroppedPartInfo`，改列和加索引还使用 `Analyzed`。因此核心不变量是“构造器写入的 `Tp` 必须与 getter 的 expected action 一致”，而不是所有 `Option` 都必然有值；getter 保留 `Option`，允许线协议中缺失载荷被如实表达。

`overwrite_from` 专为 `Store::List` 的缓冲区复用服务：源事件为 `None` 时清空目标；否则，只用源中 `Some` 的对象字段和非空向量覆盖目标，而布尔、旧表 ID 与 action 总是覆盖。这个行为会有意保留目标槽位中源事件未提供的对象或向量残留，`store.rs` 的注释也明确称其为“合并”；使用复用槽位的代码不能把未被当前 action getter选中的字段当作当前事件事实。

删库精简结构仅包含库/表/分区的 ID 与 `CIStr` 名称。`NewDropSchemaEvent` 遍历输入表以及可选 `Partition.Definitions` 生成新向量，不保留列、索引、TTL 等其它表属性。

## 依赖与调用关系

- 上游构造：RustCodeGraph 将 `NewCreateTableEvent` 连接到 Go DDL 路径和 Rust `persistent_create_table.rs` 等实现；仓库搜索进一步确认 Rust 已接线的生产调用点位于 `persistent_create_table.rs`、`persistent_create_materialized_view*.rs`、`persistent_modify_column.rs`、`persistent_masking_actions.rs`、`persistent_mview_out_of_place_cutover.rs`。
- 发布边界：`pkg/ddl/persistent_actions.rs::async_notify_event` -> `pkg/ddl/notifier/publish.rs::PubSchemaChangeInTransaction` -> `pkg/ddl/notifier/store.rs::InsertSchemaChangeSQL` -> `SchemaChangeEvent::MarshalJSON`。
- 读取边界：`store.rs::{SqlListResult,TableListResult}::Read` -> `SchemaChangeEvent::UnmarshalJSON` -> `overwrite_slot` -> `overwrite_from`（仅复用已有槽位时）。
- 下游消费：RustCodeGraph 显示各 getter 的主要 Go 调用者为 `pkg/statistics/handle/ddl/subscriber.go::handle`；例如 `GetModifyColumnInfo`、各分区 getter 和 `GetDropSchemaInfo` 都进入统计更新分支。物化视图事件另有 statistics DDL 测试覆盖。
- 类型依赖：`model::ActionType` 及完整元数据来自 `meta-model`，`ast::CIStr` 来自 `parser-ast`；JSON 错误经 crate 的 `Error`（定义于 `store.rs`，含 `Json(serde_json::Error)`）传播。
- crate 边界：`pkg/ddl/notifier/Cargo.toml` 没有 feature 条件；`events.rs` 也没有条件编译项。测试由 `lib.rs` 使用 `#[cfg(test)] #[path = "events_test.rs"]` 独立装配，符合源文件与测试文件分离要求。

## 错误处理与边界

`MarshalJSON` 和 `UnmarshalJSON` 返回 `Result<_, Error>`，serde 语法或模型解码错误通过 `Error::Json` 向存储/发布调用者传播。`UnmarshalJSON` 先完整解码再赋值；解析失败不会覆盖原有 `inner`。线格式对缺失字段使用默认值，因此旧记录缺少新字段时可读成零值，这也是新增可选字段时应保持的兼容模式。

类型专用 getter 是断言式 API而非可恢复校验：空 `inner` 或 action 不匹配会 panic。订阅者必须先按 `GetType` 分派；若需要处理不可信外部事件，应在调用 getter 前显式验证类型。相反，载荷字段本身缺失不会 panic，而是返回 `None`/空向量，后续业务层必须决定是否接受。

`PartialEq` 比较的是两次 JSON 编码的 `Option<Vec<u8>>` 结果；正常可序列化模型下这给出线协议相等性。若双方恰好都编码失败，两个 `None` 也会被视为相等，因此它适合当前测试/值比较，不应替代显式序列化错误检查。

本文件不检查 DDL 是否已提交、是否属于系统库、job/sub-job 是否重复或订阅者是否已处理；这些边界分别属于 `persistent_actions.rs`、发布/存储层及订阅处理位图。也没有对大表事件设置大小上限，完整表快照的存储成本需要由调用场景控制；删库路径通过 Mini* 结构专门缓解这一问题。

## 并发与资源生命周期

本文件没有锁、线程、异步任务、通道、文件句柄或网络资源。`SchemaChangeEvent` 是拥有型且 `Clone` 的值，资源生命周期由 Rust 所有权管理；getter 的深克隆将返回值与内部事件解耦。

并发和事务语义发生在外层：`async_notify_event` 把插入绑定到 DDL worker 的现有事务；`store.rs` 的 SQL/内存 store 负责事务、锁和游标，订阅层负责 owner 生命周期及 processed 位图。事件 JSON 是跨这些边界的稳定快照。若多个 owner 或 handler 并发处理，正确性依赖 store 的事务与处理位校验，而不是本文件中的同步原语。

复用列表槽位时，`overwrite_from` 会修改已有事件值，但调用发生在持有 `&mut [Option<SchemaChange>]` 的单个读取流程中；它本身不提供跨线程共享保证。需要跨线程传递时，应把事件作为独立拥有值移动或 clone，并由上层容器提供同步。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/notifier/events.go`，Rust 基本保留了 Go 的公开命名、action 到字段的映射、诊断字符串顺序和 JSON tag：`mini_db_info`、`table_info`、`old_table_info`、`added_partition_info`、`dropped_partition_info`、`columns`、`indexes`、`Analyzed`、`old_table_id_for_partition`、`type`。

主要语言差异如下：

- Go 使用 `*SchemaChangeEvent`/内部指针，Rust 用拥有型 `SchemaChangeEvent` 与 `Option<JsonSchemaChangeEvent>`；Go nil receiver 的 `String`/`GetType` 对应 Rust 默认空包装的同类行为，但 Rust 的方法不存在 nil receiver。
- Go getter 返回内部指针/切片，Rust getter clone 后返回 `Option<Box<_>>`/`Vec<Box<_>>`，避免借用内部实现，也带来深拷贝开销。
- Go 用 `intest.Assert` 检查 action，Rust `expect` 始终用 `expect`/`assert_eq!`；错误调用在 Rust 所有构建模式下都可能 panic。
- Go 直接让内部结构实现 `encoding/json`，Rust 单独维护 `WireEvent` 以锁定 Go 线格式。`events_test.rs::event_json_preserves_complete_go_model_payload` 验证完整表模型中的列类型、索引、分区、placement policy 与 TTL 字段不会被简化丢失。
- Go 对 `null` 反序列化为零值内部结构；Rust 测试 `event_json_null_and_empty_mini_slices_match_go` 验证相同行为。空删库表列表在两边都因 `omitempty`/`skip_serializing_if` 不输出 `tables`。
- Rust 新增 `overwrite_from` 以配合其 store 复用缓冲区的实现；这不是 Go `events.go` 的公开事件 API。

`pkg/ddl/notifier/events_test.go::TestEventString` 与 Rust `events_test.rs::test_event_string` 验证相同的字符串契约；两边也都有物化视图新旧表构造器测试。Rust 测试额外覆盖 JSON 往返、完整模型载荷、null/空切片和模型 action 名称。

## 扩展指南

新增事件类型时，应按以下顺序接入：

1. 先确认 `meta-model` 已有对应 `ActionType` 与人类可读名称；若没有，这属于本文件之外的模型变更。
2. 决定是否可复用现有统一字段。简单组合使用 `event_constructor!`；含额外语义字段时手写构造器。同步新增严格匹配 action 的 getter，并明确返回新表、旧表或两者。
3. 若需要新载荷字段，同时更新 `JsonSchemaChangeEvent`、`WireEvent`、两个 `From` 转换和 `overwrite_from`。字段名、默认值、省略规则必须与 Go JSON 契约兼容；新增字段应优先可缺省，确保旧记录仍能读取。
4. 在真正完成 DDL 元数据变更的 worker 阶段构造事件，经 `async_notify_event`/同事务发布路径接线；不要在 SQL executor 前端提前发布，也不要绕过 notifier store 的 job/sub-job 键。
5. 在独立的 `pkg/ddl/notifier/events_test.rs` 增加构造器/getter、错误 action、JSON 往返与 Go 兼容字段测试；不要把测试嵌入 `events.rs`。若事件被统计订阅者消费，还要同步对应 subscriber 的独立测试。Go 仍是对照基线时，也应检查 `events.go`/`events_test.go` 的字段意义和输出格式。
6. 评估兼容性与成本：修改现有 JSON 字段名或 action 映射会破坏持久化记录；把完整大对象加入高频事件会增加 clone、JSON 和系统表空间开销；错误使用 getter 会 panic；复用槽位若未更新 `overwrite_from` 可能残留旧字段或丢失新字段。

对于现有但尚未在 Rust 生产 worker 中直接找到调用点的构造器，应先搜索真实 DDL action 接线并补最小调用链，不能仅以 API 已存在宣称功能完成。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`node --file pkg/ddl/notifier/events.rs --offset 1 --limit 1200` 返回目标文件完整 675 行及直接使用文件；`explore 'pkg/ddl/notifier/events.rs Event EventType SchemaChangeEvent notifier'` 给出构造器、getter、`MarshalJSON` 与 store/statistics/DDL 调用关系。精确 `callers/callees` 命令未输出额外结果，因此未用其推断未显示的边。
- 源与 crate：`pkg/ddl/notifier/events.rs`、`pkg/ddl/notifier/lib.rs`、`pkg/ddl/notifier/Cargo.toml`。
- 发布和存储：`pkg/ddl/notifier/publish.rs::{PubSchemeChangeToStore,PubSchemaChangeInTransaction}`，`pkg/ddl/notifier/store.rs::{Insert,InsertSchemaChangeSQL,SqlListResult::Read,TableListResult::Read,overwrite_slot}`，`pkg/ddl/persistent_actions.rs::async_notify_event`。
- Rust 生产调用点：`pkg/ddl/persistent_create_table.rs`、`persistent_create_materialized_view.rs`、`persistent_create_materialized_view_log.rs`、`persistent_modify_column.rs`、`persistent_masking_actions.rs`、`persistent_mview_out_of_place_cutover.rs`。
- Go 对照与消费：`pkg/ddl/notifier/events.go`、`pkg/statistics/handle/ddl/subscriber.go`，以及 Go DDL 的 `schema.go`、`partition.go`、`index.go`、`cluster.go` 调用点。
- 独立测试：`pkg/ddl/notifier/events_test.rs` 与 `pkg/ddl/notifier/events_test.go`；Rust 测试覆盖字符串、JSON 往返、完整 Go 模型载荷、null/空 Mini 切片、物化视图新旧表。
- 本任务是纯文档分析，按计划不运行 Cargo；最终使用任务规定的 `test` + `rg -c` 命令验证目标存在且恰有 11 个固定二级章节，并人工复核只新增本说明文档、不修改 Rust/Go/Cargo/`plan.md`。
