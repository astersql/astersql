# `pkg/meta/metadef/system.rs`

## 文件定位

本文件属于 Cargo crate `astersql-meta-metadef`（入口为 [`lib.rs`](lib.rs)，清单为 [`Cargo.toml`](Cargo.toml)），由根模块以 `pub mod system` 声明并用 `pub use system::*` 重新导出。它不创建或持久化元数据，而是集中定义物理 schema 对象的全局 ID 边界、`mysql`/`sys` 数据库和 `mysql` 系统表的稳定 ID，以及判定一个 ID 是否位于系统保留区间的 `IsReservedID`。

这些定义处在元数据协议层：bootstrap 用固定 ID 把系统表名、建表 SQL 与物理对象绑定，DDL、Domain 和 InfoSchema 同步代码则用相同边界区分用户对象与系统对象。因而 ID 数值是跨启动、升级以及 Go/Rust 实现共享的兼容约定，而非可随意重排的内部枚举。

## 核心职责

1. 用 `ReservedGlobalIDUpperBound`、`ReservedGlobalIDLowerBound` 和 `MaxUserGlobalID` 划分用户对象与系统对象的 ID 空间。
2. 从上界开始按固定偏移分配系统数据库和系统表 ID：`SystemDatabaseID` 使用偏移 0，表 ID 使用偏移 1 至 59，`SysDatabaseID` 使用偏移 60，后续系统表继续使用偏移 61 至 68。
3. 通过 `IsReservedID(id)` 实现唯一的运行时逻辑，严格表达保留区间 `(ReservedGlobalIDLowerBound, ReservedGlobalIDUpperBound]`。
4. 为其他 crate 提供无状态、零分配的公共常量。表结构 SQL 不在本文件中，而在相邻的 [`system_tables_def.rs`](system_tables_def.rs)；bootstrap 在 `pkg/session/bootstrap.rs` 中把两者配对。

## 主要符号

- `ReservedGlobalIDUpperBound: i64 = 0x0000FFFFFFFFFFFF`：物理 schema 对象 ID 的全局上界，也是 `mysql` 的 `SystemDatabaseID`。
- `ReservedGlobalIDLowerBound = ReservedGlobalIDUpperBound - 1000`：1000 个 ID 的系统保留区间下界；下界本身不保留。
- `MaxUserGlobalID = ReservedGlobalIDLowerBound`：用户对象最大合法 ID，强调下界为闭区间端点。
- `SystemDatabaseID` 与 `SysDatabaseID`：分别是 `mysql` 和 `sys` 的固定数据库 ID，偏移为 0 和 60。
- `TiDBDDLJobTableID` 至 `TiDBWorkloadValuesTableID`：偏移 1 至 59 的固定表 ID，覆盖 DDL、权限、统计、GC、SQL binding、TTL、分布式任务、恢复、资源治理和内核配置等系统表。
- `TiDBSoftDeleteTableStatusTableID` 至 `TiDBMLogPurgeHistTableID`：偏移 61 至 68 的后续系统表 ID，覆盖软删除、脱敏、存储类别转换历史及物化视图/物化日志维护元数据。
- `pub fn IsReservedID(id: i64) -> bool`：返回 `lower < id && id <= upper`。它没有 I/O、错误类型或隐式状态。

文件没有 struct、enum、trait、impl、宏或条件编译项；所有符号均为公开 API。命名沿用 Go 的导出标识符，因此 crate 根在 `lib.rs` 中允许 `non_snake_case` 和 `non_upper_case_globals`。

## 执行流程

常量在编译期求值，运行时不存在初始化顺序。典型流程如下：

1. `pkg/session/bootstrap.rs` 构造 `TableBasicInfo`/`DatabaseBasicInfo`，把本文件的固定 ID、表名和 `system_tables_def.rs` 的建表 SQL 配成一组，并按 bootstrap 版本创建或升级系统对象。
2. 元数据目录接收显式 ID 时，`pkg/domain/canonical_domain.rs` 用 `IsReservedID` 判断该 ID 是否属于系统对象；只有非保留 ID 才推进普通对象分配器的 `next_id`，避免系统固定 ID 抬高用户 ID 序列。
3. 跨 keyspace 的 `pkg/infoschema/issyncer/loader.rs` 只加载与保留 ID 有关的 schema diff，并以 `SystemDatabaseID` 定位 `mysql`；`syncer.rs` 也用该判定决定系统表是否仍须进行 MDL 检查。
4. DDL 会话路径（例如 `pkg/session/runtime/ddl.rs` 的交换分区处理）拒绝把系统保留表作为普通操作对象。

对 `IsReservedID` 而言，执行仅是两次有序比较和一次逻辑与：小于等于下界返回 `false`，大于上界返回 `false`，其余返回 `true`。

## 数据与状态

本文件只有 `i64` 编译期常量，不拥有堆内存、缓存、全局可变变量或持久化句柄。十六进制上界保留高 16 位空间；当前系统区间总宽度为 1000，而已命名对象只使用上界至上界减 68，仍留有后续固定 ID 空间。

核心不变量是：

- 用户可用范围包含 `ReservedGlobalIDLowerBound`，系统范围不包含它。
- 系统范围包含 `ReservedGlobalIDUpperBound`，超出上界的值也不被视为系统 ID。
- 每个系统数据库/表常量必须具有唯一且稳定的偏移；新增项不能复用既有偏移或移动旧项。
- 所有 ID 均保持 `i64`，从而与 Go `int64` 的边界和减法语义一致。

常量名表达逻辑对象，但实际表名须以 bootstrap 配对和 `system_tables_def.rs` 为准。例如源注释中的历史命名可能与 SQL 表名略有差异，不能仅从 Rust 常量名推导建表名称。

## 依赖与调用关系

本文件本身不导入任何 crate，`IsReservedID` 的直接被调用依赖仅是本文件的 `ReservedGlobalIDLowerBound` 与 `ReservedGlobalIDUpperBound`。`Cargo.toml` 声明的 `parser-ast`、`parser-mysql` 是整个 `metadef` crate 供 `db`/根模块使用的依赖，并非本文件实现所需。

RustCodeGraph 将 `system.rs` 标记为被多个文件使用；直接引用检索确认的主要上游包括：

- `pkg/session/bootstrap.rs`：把固定数据库/表 ID 纳入系统 schema bootstrap 清单。
- `pkg/session/ddl_tables.rs`、`pkg/ddl/job_worker.rs`：定位 `tidb_ddl_job` 的固定表 ID。
- `pkg/domain/canonical_domain.rs`：分配数据库、表及分区 ID 时保护用户 `next_id` 序列。
- `pkg/infoschema/issyncer/loader.rs` 与 `syncer.rs`：跨 keyspace 过滤 schema diff、加载 `mysql`、决定 MDL 检查。
- `pkg/session/runtime/ddl.rs`：按表名选择固定 ID，并阻止对系统保留表执行不允许的 DDL。

crate 根的 `pub use system::*` 使调用方可写 `astersql_meta_metadef::SystemDatabaseID`，也可在别名导入后写 `metadef::IsReservedID`。

## 错误处理与边界

`IsReservedID` 是总函数，不返回 `Result`，也不会 panic。已由 `system_test.rs::test_is_reserved_id` 覆盖四个代表点：上界为真、下界加一为真、下界为假、普通 ID `123` 为假。由谓词还可直接推出负数、0 和大于上界的值均为假。

本文件不验证“ID 是否为某个已登记系统表”，只验证它是否落在整个预留段。因此尚未分配的保留值也会返回 `true`；调用方不能把该函数当作系统表注册表。反过来，固定 ID 与错误表名/建表 SQL 的配对也不会在此处报错，该一致性必须由 bootstrap 清单和测试保障。

修改风险集中在协议兼容：移动既有偏移会使同一个持久化 ID 被解释为不同对象；改变区间开闭性会影响用户 ID 分配、跨 keyspace schema 同步和 DDL 限制；缩小边界可能把已有系统对象重新分类为用户对象。

## 并发与资源生命周期

本文件没有锁、原子变量、线程、异步任务、通道、事务或需要释放的资源。常量是只读值，`IsReservedID` 仅读取参数和常量，因此天然可在任意线程并发调用。

生命周期实际由消费者承担：bootstrap 负责系统对象的创建/升级，Domain 负责目录状态与 ID 分配器，InfoSchema syncer 负责 schema 缓存和 MDL 状态。本文件只提供这些生命周期共同依赖的稳定分类协议，不观察也不改变其状态。

## 与 Go 版本的对应关系

直接对照文件是 [`system.go`](system.go)。Rust 保留了 Go 的 `int64`/`i64` 上界、下界、`MaxUserGlobalID`、偏移 0 至 68 的所有固定 ID，以及 `IsReservedID` 的 `lower < id && id <= upper` 表达式。独立测试 [`system_test.rs`](system_test.rs) 对齐 [`system_test.go`](system_test.go) 的四个边界断言。

当前 Rust 还包含针对偏移 63 至 68 的 `go_merge_12_system_ids_and_sql` 回归测试，并与相邻 SQL 常量联合核对新系统表；这比 Go 的 `TestIsReservedID` 覆盖面更广，但没有改变生产语义。Rust 源码为多数常量补充了中文用途说明；最后六个物化视图/日志相关常量在 Rust 中尚无逐项 doc comment，不过数值与 Go 完全对应，真实表名可由 bootstrap 和 `system_tables_def.rs` 核验。

## 扩展指南

新增系统表时，应在保留区间内选择尚未使用的唯一偏移，优先延续当前最大偏移递增，且不得改动既有数值。随后必须同步：

1. 在本文件增加公开 ID 常量，并在 `system.go` 保持 Go/Rust 数值和语义一致。
2. 在 `system_tables_def.rs`（及 Go 对照）增加建表 SQL，在 `pkg/session/bootstrap.rs` 将 ID、准确表名和 SQL 配对；如属版本化能力，还要接入相应 bootstrap 版本清单。
3. 在独立测试 `pkg/meta/metadef/system_test.rs` 增加偏移唯一性/准确性及 SQL 配对断言；不要把测试嵌入生产源文件。
4. 搜索所有依赖系统表清单、跨 keyspace 过滤或升级路径的消费者，确认新表的生命周期和兼容策略。

若只新增普通用户对象，不应修改本文件。若要改变保留区间大小或上下界，必须先评估持久化元数据、ID 分配器、旧集群升级、备份恢复和 Go/Rust 双实现兼容；这不是局部常量调整。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 `pkg/meta/metadef/system.rs`；`files --filter pkg/meta/metadef` 确认 Rust/Go 源及独立测试均被索引；`node --file pkg/meta/metadef/system.rs --offset 1 --limit 260` 读取完整 241 行源码；`query` 分别定位 Rust/Go 的 `IsReservedID`、`ReservedGlobalIDUpperBound` 和 `SystemDatabaseID`；`callees system.rs::IsReservedID` 确认函数引用两个边界常量。精确 Rust `callers` 未返回完整结果，因此上游关系再由直接引用检索和下列消费者源码核验。
- 源与 crate 边界：`pkg/meta/metadef/system.rs`、`pkg/meta/metadef/lib.rs`、`pkg/meta/metadef/Cargo.toml`。
- Go 对照与测试：`pkg/meta/metadef/system.go`、`pkg/meta/metadef/system_test.go`、`pkg/meta/metadef/system_test.rs`。
- 直接调用证据：`pkg/session/bootstrap.rs`、`pkg/domain/canonical_domain.rs`、`pkg/infoschema/issyncer/loader.rs`、`pkg/infoschema/issyncer/syncer.rs`、`pkg/session/runtime/ddl.rs`。
- 本任务是纯文档分析，按计划不运行 Cargo；交付结构检查要求本文恰好包含上述 11 个固定二级标题。
