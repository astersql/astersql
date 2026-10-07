# `pkg/ddl/storage_class_transition.rs`

## 文件定位

本文件属于 `astersql-ddl` crate（`pkg/ddl/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/ddl/lib.rs` 以 `pub mod storage_class_transition` 对外导出。它不是 DDL owner、轮询线程或 SQL 表访问层，而是存储类型迁移（storage-class transition）的共享领域模型：定义运行中迁移的状态与物理目标，提供从 `astersql_meta_model::TableInfo` 推导迁移操作的纯函数，并用进程内缓存保存最近一次观测。

在实际链路中，上游分为两条。DDL 执行侧的 `pkg/ddl/persistent_actions.rs::modify_engine_attribute` 在表/分区 storage class 改变时调用 `snapshot_physical_storage_classes`，随后由 `stage_storage_class_transitions` 调用本文件的差异、校验、分组函数并写入 `mysql.tidb_storage_class_transition_history`。owner 服务侧的 `pkg/session/runtime/normal_ddl_service.rs::{transition_statuses,poll_storage_class_transitions}` 读取运行中历史行，恢复本文件的操作对象，轮询 TiKV 状态并维护 `StorageClassTransitionManager`。

## 核心职责

1. 统一字符串协议：`DIRECTION_TO_IA`、`DIRECTION_TO_STANDARD` 和三种状态常量定义持久化记录与展示使用的值；`normalized_target`、`direction`、`target_for_direction` 在空 tier、目标 tier 和迁移方向之间转换。
2. 建模持久化操作：`StorageClassTransitionTarget` 对应历史表 `physical_targets` JSON 中的一个物理范围；`StorageClassTransitionOperation` 把 SQL 可见状态、目标 tier 和完整目标集合绑定在一起。
3. 从表元数据计算增量：`snapshot_physical_storage_classes` 同时纳入逻辑表 ID 与所有公开分区定义，`changed_physical_ids` 只报告新旧快照中仍存在且规范化 tier 确实变化的 ID。
4. 生成确定性的运行中操作：`build_operations` 按目标 tier 聚合物理 ID，`set_targets` 排序并决定是否能展示单分区身份，最后按方向排序，保证持久化和测试结果稳定。
5. 提供目标合法性、拓扑替代和进度判断辅助函数，并以 `StorageClassTransitionManager` 隔离“持久化 RUNNING 行”与“最近成功观测”之间的瞬时状态。

## 主要符号

- `StorageClassTransitionStatus`：SQL/InfoSchema 可消费的状态快照。身份字段为表、可选单分区、`direction`、`schema_version` 和 `start_ts`；观测字段为副本计数、进度、时间与有效性；`physical_table_ids` 保留聚合操作覆盖的全部物理 ID。
- `StorageClassTransitionTarget`：可序列化/反序列化的持久化目标。`partition_id == 0` 与空 `partition_name` 通过 serde 省略；`physical_id` 必须非零且在同一操作内唯一，这一约束由 `validate_targets` 执行。
- `PhysicalStorageClass`：元数据快照中的内部值，将一个可持久化 target 与当前 tier 组合起来。
- `StorageClassTransitionOperation`：完整运行单元；`status` 用于展示和缓存，`target` 是 `IA` 或 `STANDARD`，`targets` 是轮询和持久化的物理范围。
- `StorageClassTransitionKey`：私有缓存键 `(table_id, direction, start_ts)`。相同表可同时按不同方向或不同开始 TSO 区分运行。
- `StorageClassTransitionManager`：`RwLock<BTreeMap<StorageClassTransitionKey, StorageClassTransitionStatus>>` 包装的最近观测缓存；公开 `observe`、`cached_observation`、`remove`、`retain_active`、`clear`。
- 构建函数：`snapshot_physical_storage_classes`、`changed_physical_ids`、`build_operations`、`set_targets`。
- 防御与协调函数：`validate_targets`、`targets_exist`、`replacement_physical_ids`、`topology_is_stable`、`add_current_targets`、`touches`、`update_progress`、`schema_published`。

## 执行流程

DDL staging 的当前 Rust 流程如下：

1. `persistent_actions.rs::modify_engine_attribute` 在修改前调用 `snapshot_physical_storage_classes` 保存旧状态，修改完成并取得新 schema version 后进入 `stage_storage_class_transitions`。
2. `stage_storage_class_transitions` 对新表信息再次快照，并由 `changed_physical_ids` 比较规范化后的 tier。空字符串视为 `STANDARD`，因此“空值”和显式 `STANDARD` 不产生迁移。
3. 若变化与现有 RUNNING 行相交，staging 代码解析其 `physical_targets`，用 `validate_targets` 防止空、零 ID 或重复 ID，读取匹配的缓存观测并将旧行条件更新为 `SUPERSEDED`；`add_current_targets` 只把仍存在的旧目标加入新一轮集合。
4. `build_operations` 先拒绝非正 schema version、零 start TSO 和快照中不存在的物理 ID，再按规范化目标 tier 分组。每组初始化为无有效观测、零计数，开始时间由 `model::TSConvert2Time(start_ts)` 转换。
5. `set_targets` 令物理 ID 有序；仅当一组恰好包含一个真实分区时写入分区 ID/名称，多分区组或包含逻辑表范围的组保持表级展示。staging 最后把每组插入历史表为 `RUNNING`。

轮询与读取的当前 Rust 流程位于 `normal_ddl_service.rs`：后台每 10 秒加载 RUNNING 行，反序列化 targets 并调用 `validate_targets`、`set_targets`，向各 store 收集每个物理 ID 的 ready/total，随后 `observe` 缓存状态；全部目标都有非零总数且 ready 等于 total 时，以带 `state='RUNNING'` 条件的更新结束记录并 `remove` 缓存。`transition_statuses` 重新加载 RUNNING 行，仅在 `cached_observation` 精确匹配时合并最近计数，然后计算当前 duration。轮询末尾 `retain_active` 删除不再对应 RUNNING 行的观测。

## 数据与状态

持久状态位于 `mysql.tidb_storage_class_transition_history`；本文件只定义记录内容和转换规则，不直接执行 SQL。`RUNNING` 由 staging 创建，当前 DDL 修改可将相交记录改为 `SUPERSEDED`，轮询完成后改为 `COMPLETED`。三个状态常量中，Rust 生产接线直接以字符串 SQL 使用状态值；常量本身当前主要承担共享协议声明。

目标集合使用 `BTreeMap`/`BTreeSet`，因此物理 ID、方向分组和缓存遍历具有稳定顺序。快照总是包含 `table.ID`，即使表已分区；分区条目另带相同的 `physical_id`/`partition_id` 及原始名称。`changed_physical_ids` 不把新出现或已经消失的物理 ID当作普通 tier 变化：只有 current 和 old 都有该 ID 且 tier 不同才入集。

缓存键不足以独立证明同一运行实例完全相同，因此 `cached_observation` 还通过 `same_status` 比较 table ID、schema version、start TSO、partition ID、direction、start time 与有序 physical IDs，并要求缓存项 `status_valid`。表名、库名、分区名和瞬时计数不参与一致性判定。零副本的成功观测可以被缓存（`status_valid=true`、`progress_valid=false`），与尚未观测区分。

## 依赖与调用关系

直接依赖很小：标准库 `BTreeMap`/`BTreeSet` 保证确定性集合语义，`RwLock` 保护进程内缓存；`chrono::{DateTime, Duration, Utc}` 表示时间；`serde` 为目标 JSON 提供编解码；`astersql_meta_model` 提供 `TableInfo`、分区 DDL 状态和 TSO 时间转换。这些依赖均由 `pkg/ddl/Cargo.toml` 声明，文件本身没有 feature 或条件编译分支。

RustCodeGraph 给出的关键生产边包括：

- `persistent_actions.rs::modify_engine_attribute -> snapshot_physical_storage_classes -> stage_storage_class_transitions`。
- `stage_storage_class_transitions -> changed_physical_ids / validate_targets / target_for_direction / set_targets / add_current_targets / build_operations`。
- `build_operations -> snapshot_physical_storage_classes / normalized_target / direction / set_targets`。
- `normal_ddl_service.rs::poll_storage_class_transitions -> target_for_direction / validate_targets / set_targets / StorageClassTransitionManager::{observe,remove,retain_active}`。
- `normal_ddl_service.rs::transition_statuses -> target_for_direction / set_targets / cached_observation`。

`JobExecutionContext::cached_storage_class_observation`（`pkg/ddl/job_worker.rs`）是 staging 获取上次 owner 观测的抽象边界；具体系统会话持有 `Arc<StorageClassTransitionManager>`（`pkg/session/runtime/system_session.rs`），让 DDL worker 与轮询/展示路径共享缓存而不把 session crate 的实现反向放进 ddl crate。

RustCodeGraph 未发现 `targets_exist`、`replacement_physical_ids`、`topology_is_stable`、`touches`、`update_progress`、`schema_published` 的 Rust 生产调用者；它们当前由 `storage_class_transition_test.rs` 验证或完全预留。不能据这些符号推断 Rust 运行时已经执行拓扑协调、统一进度辅助或 schema 发布门控。

## 错误处理与边界

公共转换/构建函数使用 `Result<_, String>`：未知 tier/direction、不可用 schema version、零 start TSO、缺失物理 ID、空目标、零 ID 和重复 ID都会立即返回可读错误。调用方通过 `?` 将错误上送至 DDL job 或 owner 服务；本文件不做重试、记录日志或回滚。

`build_operations` 对空 `physical_ids` 返回空操作数组而不是错误；这与 staging 的“无变化直接成功”相容。`targets_exist` 要求所有旧目标仍存在且 tier 与操作目标一致，但不要求快照没有额外目标。`replacement_physical_ids` 会排除其他 RUNNING 操作已经 claim 的 ID，并确保原本只跟踪分区的操作不会扩展到父表 ID。`topology_is_stable` 仅把无分区或 `Partition.DDLState == StateNone` 视为稳定。

`update_progress` 的完成条件是 `observed && total != 0 && ready == total`，但它不校验 `ready <= total`，且当新样本无效时不会主动把旧 `progress` 清零；调用者必须把 `progress_valid` 当作读取前提。当前 Rust 生产轮询未调用此函数，而是在 `normal_ddl_service.rs` 中独立计算进度。缓存锁中毒使用 `expect("storage class transition cache poisoned")`，会 panic，而非返回业务错误。

## 并发与资源生命周期

`StorageClassTransitionManager` 的共享状态只有 `observed` map。每次方法只在一次短小的 map 读、写、删除或 retain 期间持有 `std::sync::RwLock`；网络请求、SQL 查询和 JSON 解析均在调用者中完成，不在锁内执行。返回缓存值时 clone 状态，从而避免锁守卫或内部引用越过方法边界。

生产环境由 `SystemSessionPool` 资源持有一个 `Arc<StorageClassTransitionManager>`。轮询成功后 `observe` 覆盖同 key 观测；RUNNING 行变为终态后 `remove` 删除；每轮末尾 `retain_active` 清理数据库中已消失的运行实例；DDL 服务关闭路径调用 `clear`。因此该缓存可丢失、不可作为真相来源，owner 重建后应由历史表重新发现运行操作，尚未持久化的计数会回到未知状态。

持久化层以 `(table_id, start_ts, direction)` 和 `state='RUNNING'` 条件更新作为并发保护，相关 SQL 位于调用者而非本文件。当前 Rust 文件自身不启动线程、不拥有网络连接、事务或 channel，也不控制轮询取消；这些生命周期分别属于 `normal_ddl_service.rs` 和 DDL job/session 层。

## 与 Go 版本的对应关系

`pkg/ddl/storage_class_transition.go` 是逐符号对照来源：Rust 的 status/key/target/physical state/operation、tier/方向转换、快照、变化检测、操作分组、目标设置与校验、拓扑辅助、缓存匹配、进度和 schema version 判断都能找到同名语义的 Go 实现。`pkg/ddl/storage_class_transition_test.rs` 也复现了 Go 单测的核心意图：按方向分组、父表为真实物理目标、只比较存活目标、替代目标过滤、一次完整观测完成、严格目标校验与精确缓存身份。

但当前接线并非完整等价。Go 的 `storageClassTransitionManager` 同时维护 `active` 与 `observed`，能从 InfoSchema 刷新当前名称并在 schema version 发布后才展示；Rust manager 只有 `observed`，Rust `transition_statuses` 直接使用历史行中的名称，且 `schema_published` 暂无生产调用。Go `poll` 会用 `storageClassTransitionTargetsExist`、`storageClassTransitionTopologyIsStable` 和 `reconcileStorageClassTransitionTopology` 等待/替换发生变化的分区拓扑；Rust `normal_ddl_service.rs::poll_storage_class_transitions` 当前没有调用对应辅助函数。Go 的观测还使用请求超时并拒绝 `ready > total`；Rust 当前轮询直接调用 store helper，没有在本文件或所读轮询段中体现同等校验。

因此本文件可视为 Go 领域模型和部分协调算法的 Rust 移植基础，而不能单凭这些 helper 的存在宣称完整 Go 运行行为已接通。Go 的 `storage_class_transition_poll_test.go` 与 `storage_class_transition_history_test.go` 覆盖拓扑稳定、协调失败、保留最后观测、owner 切换和终态条件更新等集成语义；Rust 独立单测目前主要覆盖纯函数与缓存。

## 扩展指南

- 新增 tier 或方向时，应同时修改方向常量、`normalized_target`、`direction`、`target_for_direction`，并同步历史表协议、所有 SQL 读写者及 `pkg/ddl/storage_class_transition_test.rs`；未知值必须继续显式失败，避免把无法轮询的记录持久化为 RUNNING。
- 改变操作身份或缓存复用条件时，应一起审查 `StorageClassTransitionKey`、`StorageClassTransitionOperation::key` 和 `same_status`。键过宽会覆盖并行运行，匹配过松会把旧拓扑计数套到新操作，匹配过严则会丢失可安全复用的最后观测。
- 改变物理目标分组时，优先修改 `snapshot_physical_storage_classes`、`changed_physical_ids`、`build_operations`、`set_targets`，并保持目标/操作排序确定性及“单分区才展示分区身份”的契约。
- 若要完成 Go 拓扑协调语义，接入点应在 `normal_ddl_service.rs::poll_storage_class_transitions`，复用 `targets_exist`、`topology_is_stable`、`replacement_physical_ids`、`touches`，并补充独立 Rust 集成测试；不能只扩写本文件 helper 后宣称行为完成。
- 若统一进度计算，应让生产轮询调用 `update_progress` 或删除重复逻辑，同时补充 `ready > total`、部分目标未观测、零副本及旧 progress 清理用例。若启用 `schema_published`，还需明确当前 InfoSchema/name 解析的所有权。
- 测试逻辑应继续放在独立的 `pkg/ddl/storage_class_transition_test.rs` 或更高层独立测试文件，不嵌入本生产文件。兼容风险集中在历史 JSON/字符串协议和已有 RUNNING 行；性能风险主要是每轮对目标数乘 store 数的轮询，而非这些本地 `BTree*` 操作。

## 验证依据

- RustCodeGraph 索引状态：仓库索引可用，目标文件 `pkg/ddl/storage_class_transition.rs` 被识别为 405 行、39 个符号；通过 `node --file` 阅读了文件全貌。
- RustCodeGraph 精确符号证据：查询/读取了 `build_operations`、`snapshot_physical_storage_classes`、`changed_physical_ids`、`set_targets`、`validate_targets`、拓扑辅助函数、进度/schema 辅助函数和 manager 方法；`node` trail 验证了 `persistent_actions.rs::stage_storage_class_transitions` 及 `normal_ddl_service.rs::{transition_statuses,poll_storage_class_transitions}` 的调用边。通用 `callers` 命令在本地长时间无输出后中止，随后用 `node` 的 `Called by`/`Calls` trail 获得等价图证据。
- 已读 Rust 文件：`pkg/ddl/storage_class_transition.rs`、`pkg/ddl/storage_class_transition_test.rs`、`pkg/ddl/persistent_actions.rs` 的 staging 函数、`pkg/ddl/job_worker.rs` 的执行上下文边界、`pkg/session/runtime/normal_ddl_service.rs` 的状态读取/轮询、`pkg/session/runtime/system_session.rs` 的 manager 持有处、`pkg/ddl/lib.rs` 和 `pkg/ddl/Cargo.toml`。
- 已读 Go 对照与测试：`pkg/ddl/storage_class_transition.go`、`pkg/ddl/storage_class_transition_test.go`、`pkg/ddl/storage_class_transition_poll_test.go`、`pkg/ddl/storage_class_transition_history_test.go`。
- 人工复核结论：本文件存在的原因是让 DDL staging、owner 轮询和状态读取共享同一操作身份、目标序列化和缓存匹配规则；安全扩展必须同时维护持久化协议、调用者接线与独立测试，并区分已存在 helper 和已接通生产行为。
- 本任务为纯文档分析，按计划未运行 Cargo；最终结构通过任务规定的 11 标题命令验证。
