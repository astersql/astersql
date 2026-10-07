# `pkg/ddl/util/dead_table_lock_checker.rs`

## 文件定位

该文件属于 `astersql-ddl-util` crate（见 `pkg/ddl/util/Cargo.toml`），实现“失效表锁”检测：这里的“失效”是表锁登记的 TiDB `server_id` 已不在 etcd schema-version 发布者集合中，不是事务等待形成的死锁。`pkg/ddl/util/lib.rs` 将本模块声明为私有模块后通过 `pub use dead_table_lock_checker::*` 公开其 API。

当前 Rust 仓库中，生产侧只找到 crate 再导出，没有找到 `NewDeadTableLockChecker`、`DeadTableLockChecker` 或 `GetDeadLockedTables` 的 Rust 调用者；唯一直接调用位于独立测试 `pkg/ddl/util/dead_table_lock_checker_test.rs`。因此该文件已具备局部检测算法和测试夹具，但尚无证据表明它已接入 Rust DDL owner 的周期清理主链。Go 对照实现则已由 `pkg/ddl/ddl.go` 的 `newDDL` 构造并由 `startCleanDeadTableLock` 每 10 秒在 owner 节点调用。

## 核心职责

- `get_alive_servers` 从 `DDLAllSchemaVersions` 前缀读取当前仍发布 schema version 的节点键，并把键后缀整理为 `HashSet<String>`，用于常数期望时间的存活判断。
- `GetDeadLockedTables` 遍历 infoschema 提供的带锁表，将持锁会话的 `server_id` 与存活集合比对，并按 `SessionInfo` 聚合该会话持有的 `TableLockTpInfo`。
- `MetaOnlyInfoSchema` 把检查器与具体 infoschema 实现隔离；`InMemoryInfoSchema` 是本文件提供的最小内存实现，会过滤无锁表和过滤后为空的库分组。
- 本文件只“发现并描述”失效锁，不删除锁、不提交 DDL job，也不更新表元数据。Go 中后续清理由 `pkg/ddl/ddl.go::cleanDeadTableLock` 完成；Rust 中未找到对应接线证据。

## 主要符号

- `DEFAULT_RETRY_COUNT: usize = 5`：etcd 查询最多尝试五次。
- `DEFAULT_RETRY_INTERVAL: Duration = 200ms`：每次失败后当前线程的等待时长。
- `DEFAULT_TIMEOUT: Duration = 1s`：为对齐 Go 常量而保留，但 Rust 当前实现没有把它传给 `EtcdClient::get`，实际不构成单次读取超时。
- `DatabaseTables { table_infos: Vec<TableInfo> }`：按数据库组织的表元数据容器，本身不保存库 ID；表的 `db_id` 来自每个 `TableInfo`。
- `MetaOnlyInfoSchema::list_tables_with_locks(&self) -> Vec<DatabaseTables>`：检查器依赖的最小元数据接口。接口名称虽说 “with locks”，`GetDeadLockedTables` 仍用 `let Some(lock)` 防御性复核。
- `InMemoryInfoSchema { databases }` 及其 trait 实现：克隆带锁 `TableInfo`，丢弃无锁表及空分组，主要供测试和本地校验使用。
- `DeadTableLockChecker`：保存可选 `Arc<EtcdClient>`、重试次数和重试间隔；字段私有，策略只能通过构造函数和 builder 修改。
- `NewDeadTableLockChecker`：以默认重试策略创建检查器；命名保留 Go 风格，因此 crate 根允许 `non_snake_case`。
- `DeadTableLockChecker::with_retry_policy`：按值接收并返回检查器，便于测试用零间隔/自定义次数缩短失败场景。
- `DeadTableLockChecker::get_alive_servers`：私有 etcd 查询和 server ID 解析入口。
- `DeadTableLockChecker::GetDeadLockedTables`：公开检测入口，返回 `HashMap<SessionInfo, Vec<TableLockTpInfo>>`。

## 执行流程

1. 调用者以可选的共享 `EtcdClient` 创建检查器；`None` 表示无法使用 etcd 判活。
2. `GetDeadLockedTables` 遇到 `None` 直接返回空 `HashMap`，不会访问 infoschema，也不会报错。
3. 有客户端时，`get_alive_servers` 在每轮查询前调用 `CancellationToken::check`。若已取消，立即返回 `DdlUtilError::Cancelled`。
4. 每轮调用 `EtcdClient::get(DDLAllSchemaVersions, true)` 做前缀读取。成功后，对每个键尝试去掉 `"/tidb/ddl/all_schema_versions/"`；不带该斜杠后缀前缀的键保持原值。这与 Go 的 `strings.TrimPrefix(key, DDLAllSchemaVersions+"/")` 一致。
5. 查询失败时保存最近错误，并在非零间隔下执行 `thread::sleep`，随后重试；全部尝试失败后返回最后一次错误。`retry_count == 0` 时不会查询，返回内部构造的 `DdlUtilError::Etcd("get was not attempted")`。
6. 取得存活节点集合后，调用 `info_schema.list_tables_with_locks()`，逐库、逐表、逐持锁会话遍历。
7. 若会话的 `server_id` 不在存活集合中，则用完整 `SessionInfo`（`server_id + session_id`）作为聚合键，追加 `{ schema_id: table.db_id, table_id: table.id, lock_type }`。
8. 返回聚合映射；同一失效会话在多张表上的锁会进入同一向量，多个失效会话会形成不同 map 项。函数不排序、不去重，结果顺序继承 infoschema 的遍历顺序。

## 数据与状态

检查器自身只有不可变使用的客户端引用和重试配置；检测调用不会缓存存活节点或结果。`Arc<EtcdClient>` 允许多个所有者共享客户端，当前 `EtcdClient` 是 `pkg/ddl/util/util.rs` 中基于 `Arc<Mutex<EtcdState>>` 的进程内模型，而不是外部 etcd SDK 客户端。

输入表元数据由 `TableInfo`、`TableLockInfo` 和 `SessionInfo` 表达：锁包含持有会话列表和字符串形式的锁类型；输出 `TableLockTpInfo` 只保留 schema ID、table ID、锁类型。`SessionInfo` 实现 `Eq + Hash`，所以同一节点上的不同 `session_id` 不会混并。

存活节点集合来自键名而非键值：`EtcdValue.value` 和 revision 均不参与判断。空前缀结果意味着没有存活节点，于是所有持锁会话都会被视为失效；这是当前算法的直接语义，调用环境必须保证 schema-version 前缀的完整性和时效性。

## 依赖与调用关系

直接标准库依赖为 `HashMap`、`HashSet`、`Arc`、`thread::sleep` 和 `Duration`。业务类型均由同 crate 的 `util.rs` 经 crate 根再导入：`CancellationToken`、`DDLAllSchemaVersions`、`DdlUtilError`、`EtcdClient`、`SessionInfo`、`TableInfo`、`TableLockTpInfo`。`pkg/ddl/util/Cargo.toml` 的 `[dependencies]` 为空，说明该实现目前没有外部 crate 依赖。

RustCodeGraph 给出的文件内关键边为：`GetDeadLockedTables → get_alive_servers`、`GetDeadLockedTables → MetaOnlyInfoSchema::list_tables_with_locks`，以及 `InMemoryInfoSchema::list_tables_with_locks` 对 trait 方法的实现。全仓 Rust 文本复核只发现 `dead_table_lock_checker_test.rs` 通过 `NewDeadTableLockChecker → GetDeadLockedTables` 调用该功能；其他名为 `MetaOnlyInfoSchema` 或 `DatabaseTables` 的类型位于不同模块，不是本文件 trait/struct 的实现或调用者。

Go 的已接线调用链是 `pkg/ddl/ddl.go::newDDL → util.NewDeadTableLockChecker`，然后 `startCleanDeadTableLock → GetDeadLockedTables → cleanDeadTableLock`。定时循环仅在 DDL owner 上执行，并把检测结果转换为解锁表的 DDL job。该 Go 主链只能解释设计位置，不能作为 Rust 已接线的证据。

## 错误处理与边界

- 无 etcd 客户端是正常降级：返回空映射；Go 返回的是 `nil, nil`，Rust 返回可迭代的空 map，调用语义相近但容器表示不同。
- 取消只在每轮请求开始前检查。etcd 读取期间和 `thread::sleep` 期间不会再次检查，所以取消响应可能被一次读取或最多一个重试间隔延迟。
- etcd 失败不会记录日志；它保留并最终返回最后一个 `DdlUtilError`。Go 版本每次失败会写 DDL info 日志，并通过子 context 对每次请求施加一秒超时。
- Rust 的 `DEFAULT_TIMEOUT` 当前未使用，`EtcdClient::get` 也不接收 context/timeout；不能据此声称单次查询有一秒截止时间。
- `get_alive_servers` 通过 `expect("caller checks the optional etcd client")` 取客户端。它是私有函数，当前唯一调用点先检查 `is_none`，因此不触发；若未来重构调用关系，必须保持该前置条件或改为显式错误。
- 返回值不去重。同一会话若在输入中重复出现，或相同表重复出现，输出也会重复；当前文件没有验证 schema/table ID 或锁类型合法性。
- 前缀读取会匹配所有以 `DDLAllSchemaVersions` 开头的键；只有解析时才要求额外的 `/`。独立测试覆盖“键恰好等于前缀”时保持原键的 Go `TrimPrefix` 语义。

## 并发与资源生命周期

`DeadTableLockChecker` 没有内部可变缓存；共享状态位于 `Arc<EtcdClient>` 内部的 `Mutex`。每次 `get` 在持锁期间克隆匹配值，返回后释放互斥锁。该文件不创建后台任务、通道、定时器或长期资源，检测结果及临时集合在调用结束后按 Rust 所有权规则释放。

重试使用同步 `thread::sleep`，会阻塞执行该检查的 OS 线程；它不是异步等待，也没有退避或抖动。多个线程可以持有相同 `Arc<EtcdClient>` 并调用检查器，但会在内存客户端的单个互斥锁处串行访问共享 etcd 状态。`MetaOnlyInfoSchema` trait 未声明 `Send + Sync`，所以本接口自身不承诺可跨线程传递 trait object。

Go 生产链通过 DDL 实例的 ticker 和 wait group 管理周期任务生命周期，并由 `d.ctx.Done()` 终止；这些生命周期机制不在本 Rust 文件中，且当前未发现 Rust 对应调用链。

## 与 Go 版本的对应关系

核心算法按 `pkg/ddl/util/dead_table_lock_checker.go` 对齐：默认 5 次重试、200ms 间隔；从 `DDLAllSchemaVersions` 前缀收集 server ID；按 infoschema 中 `Lock != nil` 的表扫描；以完整会话为键聚合失效表的 schema/table ID 和锁类型。

主要差异如下：

- Go 使用真实 `clientv3.Client`、`context.Context` 和每次请求的一秒子超时；Rust 使用 crate 内内存 `EtcdClient` 与协作式 `CancellationToken`，`DEFAULT_TIMEOUT` 尚未生效。
- Go 依赖共享的 `infoschema.MetaOnlyInfoSchema.ListTablesWithSpecialAttribute`；Rust 在本文件另定义最小 trait 和 `InMemoryInfoSchema`，尚未桥接 `pkg/infoschema/context/infoschema.rs` 中同名但不同的 trait。
- Go 重试失败会记录日志；Rust 只返回最后错误。Go 休眠同样不受 context 直接打断，但下一轮会检查取消。
- Go 无 etcd 时返回 `nil` map；Rust 返回空 map。
- Go 已在 `pkg/ddl/ddl.go` 的 owner 周期任务中使用并继续提交解锁 job；Rust 只有 crate 导出和独立单元测试调用，不能视为完整迁移完成。

独立 Rust 测试 `pkg/ddl/util/dead_table_lock_checker_test.rs::exact_schema_version_prefix_key_matches_go_trim_prefix_semantics` 验证键恰好等于 `DDLAllSchemaVersions` 时不会错误地剥离前缀，且相同字符串的 `server_id` 被判为存活。仓库内没有找到直接针对 Go `DeadTableLockChecker` 的 `_test.go` 用例。

## 扩展指南

- 接入真实 Rust DDL 清理链时，优先复用现有 DDL owner 生命周期，并明确区分“检测”和“提交解锁 job”；不要在本检查器内直接改元数据。需为生产 infoschema 增加适配，避免与 `pkg/infoschema/context/infoschema.rs` 的同名 trait 混淆。
- 接入真实 etcd 时，应让单次 `get` 实际应用 `DEFAULT_TIMEOUT`，保留前缀读取和 Go `TrimPrefix` 边界语义，并决定日志由本层还是调用层负责。注意超时/取消行为属于兼容性契约。
- 修改重试逻辑时，重点处理 `retry_count == 0`、首次成功、部分失败后成功、全部失败返回最后错误、休眠期间取消等分支；若改为异步 API，应消除 `thread::sleep` 对执行线程的阻塞。
- 修改表扫描或聚合时，应保持完整 `SessionInfo` 分组以及 `TableInfo.db_id/id`、`lock_type` 的逐项映射；若增加去重或排序，必须说明对调用者和 Go 对齐的行为变化及额外时间/内存成本。
- 测试必须继续放在独立文件 `pkg/ddl/util/dead_table_lock_checker_test.rs`，不要内嵌回生产源文件。建议补充：无客户端、取消、重试恢复/耗尽、空存活集合、存活与失效会话混合、多表同会话聚合、无锁表过滤和零次重试。
- 若新增外部依赖或改变 crate 边界，需同步 `pkg/ddl/util/Cargo.toml`；当前该 manifest 没有外部依赖。

## 验证依据

- Rust 源码：`pkg/ddl/util/dead_table_lock_checker.rs`；依赖类型实现：`pkg/ddl/util/util.rs`；模块声明与公开再导出：`pkg/ddl/util/lib.rs`。
- crate 边界：`pkg/ddl/util/Cargo.toml`，包名为 `astersql-ddl-util`，`lib.path = "lib.rs"`，`go-package = "pkg/ddl/util"`，且 `[dependencies]` 为空。
- Rust 独立测试：`pkg/ddl/util/dead_table_lock_checker_test.rs`。该文件当前只有 `exact_schema_version_prefix_key_matches_go_trim_prefix_semantics` 一个测试。
- Go 对照：`pkg/ddl/util/dead_table_lock_checker.go`；生产接线与后续清理：`pkg/ddl/ddl.go` 中的 `ddlCtx.tableLockCkr`、`newDDL`、`startCleanDeadTableLock`、`cleanDeadTableLock`。
- RustCodeGraph：`status` 显示索引包含目标文件；`files --filter pkg/ddl/util` 显示该文件有 11 个符号；`explore` 返回完整源码及 `GetDeadLockedTables → get_alive_servers/list_tables_with_locks` 关系；`query` 分别定位 `GetDeadLockedTables`、`get_alive_servers`、`NewDeadTableLockChecker`、`MetaOnlyInfoSchema`。`callers/callees` 未返回额外外部边，随后用全仓 Rust 搜索确认仅独立测试调用。
- 人工边界复核：文档区分 Rust 当前事实与 Go 设计位置，未把未生效的 `DEFAULT_TIMEOUT`、未接线的 owner 清理链或内存 etcd 模型描述成生产能力。
