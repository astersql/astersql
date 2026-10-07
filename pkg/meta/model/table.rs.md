# `pkg/meta/model/table.rs`

## 文件定位

`table.rs` 是 `pkg/meta/model` 的表级持久化元数据模型实现，定义表、视图、物化视图、序列、分区、外键、表锁、TTL、统计收集选项和亲和性等对象的数据形状及纯内存辅助逻辑。它不执行 DDL、SQL 或存储操作，而是向这些上层流程提供共享协议对象。实际类型所有权位于 `pkg/meta/model/internal/group1/lib.rs` 所代表的 `astersql-meta-model-group1` crate；该 crate 将本文件与 `column.rs`、`index.rs` 放在同一类型边界内，顶层 `astersql-meta-model` 再从 group1 统一导出，group4 只做兼容再导出。

本文件直接迁移并对照 `pkg/meta/model/table.go`。大量 `serde` 字段名属于与 Go 元数据 JSON 的持久化兼容协议，不是普通内部字段命名；例如 `TableInfo::Columns` 使用 `cols`，`Indices` 使用 `index_info`，`DBID` 明确跳过序列化。RustCodeGraph 的文件节点显示该文件被 57 个文件使用，代表性消费者包括 `pkg/ddl/create_table.rs`、`pkg/ddl/job_worker.rs` 和物化视图 DDL 文件。

## 核心职责

- 以 `TableInfo` 聚合一张基表、视图或序列的稳定 ID、列/索引/约束/外键、allocator 水位、分区、锁、放置策略、TiFlash、TTL、软删除、存储层级和表模式等元数据；`TableInfoVersion0..5` 与 `CurrLatestTableInfoVersion` 固化格式演进，`SepAutoInc` 只在版本至少为 V5 且 `AutoIDCache == 1` 时启用分离 allocator。
- 维护表结构局部不变量。`Cols` 按 public 列的 `Offset` 建槽并保留空洞；`MoveColumnInfo` 在移动列后重建所有列偏移，同时修正普通索引列、`AffectColumn` 和 `ChangeStateInfo::DependencyColumnOffset`；`GetPrimaryKey` 优先显式主键，否则选第一个由完整、非隐藏、全 NOT NULL 列组成的 UNIQUE 索引。
- 表达分区 DDL 的当前态和中间态。`PartitionInfo` 同时保存正式、添加中、删除中的定义以及 action/state；它负责状态清理、重叠分区读回退和不同 DDL 阶段应忽略的物理 ID，但不提交 DDL。
- 保存并格式化附属对象：`FKInfo::String` 生成 SHOW CREATE 风格外键片段；`MaterializedViewLogTableName` 生成受最大表名长度约束的日志表名；`TTLInfo::GetJobInterval` 解析调度间隔；`StatsOptions` 自定义扁平 JSON；各枚举包装类型把持久化数值转换为稳定字符串。
- 提供兼容性常量，包括隐藏列 ID/名、序列默认值、外键版本、TTL 新旧默认间隔和物化视图日志列名。

## 主要符号

- `TableInfo`：文件的中心公开结构。查询类方法包括 `GetPartitionInfo`、`GetPkColInfo`、`Find*`、`IsView`/`IsSequence`/`IsBaseTable`；不变量维护方法包括 `MoveColumnInfo`、`ClearPlacement`、`GetNonTempColumns`；`Hash64`/`Equals` 只按稳定表 ID 定义身份。
- `PartitionInfo`、`PartitionDefinition`、`PartitionState`、`UpdateIndexInfo`：分区配置、物理定义和 DDL 中间状态。关键方法为 `SetStateByID`、`GCPartitionStates`、`ClearReorgIntermediateInfo`、`GetOverlappingDroppingPartitionIdx`、`ReplaceWithOverlappingPartitionIdx` 和 `IDsInDDLToIgnore`。
- `ViewInfo`、`MaterializedViewBaseInfo`、`MaterializedViewInfo`、`MaterializedViewShadowInfo`、`MaterializedViewLogInfo`、`MViewInitBuildState`：普通视图与物化视图关系、构建/刷新/清理配置；`EffectiveLogAccumulationAlertRows` 仅把正数阈值视为有效。
- `TimeZoneLocation`、`TimeZone`：保存物化视图定义时区以及进程内惰性解析缓存；`get_location` 返回共享的 `Arc<TimeZone>`。
- `SequenceInfo`：序列的起点、步长、上下界、缓存与循环属性，配套常量给出 Go 版本默认值。
- `TableLockInfo`、`SessionInfo`、`TableLockTpInfo`、`TableLockState`：只描述锁元数据与持锁会话，不实施锁协议。
- `ConstraintInfo`、`FKInfo`、`ReferredFKInfo`：CHECK 约束、子表外键以及父表侧回指信息；`FindFKInfoByName` 按小写名查找。
- `StatsOptions`、`StatsWindowSettings`、`WindowRepeatType`、`TableItemID`、`StatsLoadItem`：统计收集参数、时间窗口和加载项唯一键；`NewStatsOptions` 的重要非零默认值是 `AutoRecalc = true`。
- `TTLInfo`、`SoftdeleteInfo`、`TableAffinityInfo`：行生命周期与调度提示；`NewTableAffinityInfoWithLevel` 将大小写规范化并拒绝非法 level。
- `TableCacheStatusType`、`TempTableType`、`TiFlashReplicaInfo`、`ExchangePartitionInfo` 等：补足表缓存、临时表、列存副本和交换分区的持久化模型。

## 执行流程

1. 元数据由 DDL/元数据读取路径构造或通过 `serde` 从与 Go 兼容的 JSON 恢复为 `TableInfo`。`#[serde(default)]` 使缺失的新字段按零值补齐，`Option` 区分未配置对象。
2. 上层按用途读取局部视图：`GetPartitionInfo` 屏蔽 `Enable == false` 的分区配置；`Cols` 仅暴露 public 列并按 Offset 定位；`Find*` 方法按 ID 或规范化小写名定位列、索引和约束。
3. 修改列顺序时，`MoveColumnInfo(from, to)` 先移动 `Columns` 元素，再建立“旧 Offset -> 新 Offset”映射，最后用同一映射更新 `IndexInfo::Columns`、可选 `AffectColumn` 和列变更依赖。这保证 DDL 后所有引用仍指向同一逻辑列。生产调用点位于 `pkg/ddl/persistent_drop_column.rs` 与 `pkg/ddl/persistent_modify_column.rs`。
4. 分区 DDL 期间，`PartitionInfo` 把正在添加/删除的定义与 `DDLAction`、`DDLState` 一起保存。读路径可调用 `ReplaceWithOverlappingPartitionIdx`：仅在已有错误且下标有效时尝试回退；RANGE 向后寻找首个未删除分区，LIST 回退到不同的 DEFAULT 分区。`IDsInDDLToIgnore` 则按 truncate/drop/add 和 schema state 返回当前版本不可见的物理 ID。
5. DDL 完成后，`ClearReorgIntermediateInfo` 清空 action/state、目标类型/表达式/列、新表 ID 与 changed-index 映射；`GCPartitionStates` 丢弃已不在正式定义中的状态条目。
6. 附属配置按需转换：`TTLInfo::GetJobInterval` 将空字符串解释为升级兼容值 `1h`，否则解析配置；`NewTableAffinityInfoWithLevel` 规范化 `none`/`table`/`partition`；`MaterializedViewLogTableName` 加 `$mlog$` 前缀并按字符截断基表名。后者由 `pkg/session/runtime/mview_ddl.rs` 和 `pkg/session/runtime/mlog_purge.rs` 使用。

## 数据与状态

`TableInfo` 是大而稳定的可序列化快照，而非带事务能力的活动对象。列、索引、约束和外键用 `Vec` 保留展示/匹配顺序；各类最大 ID 和 allocator 水位由外部 DDL/元数据流程推进。`Version` 控制兼容语义，`Revision` 表示修订，`UpdateTS` 是可转换为时间的 TSO。视图、序列、分区、锁、TTL 等互斥或可选角色由 `Option` 表达，`IsBaseTable` 仅检查 View/Sequence 均为空。

`PartitionInfo` 是显式状态机载体：`Definitions` 是当前正式集合，`AddingDefinitions`/`DroppingDefinitions`/`NewPartitionIDs` 是过渡集合，`States` 是按物理 ID 关联的 schema 状态。`GetStateByID` 对缺失状态返回 `StatePublic`；`SetStateByID` 更新或追加；`GetNameByID` 返回原始名而名称查找使用小写名。`OriginalPartitionIDsOrder` 保存重组前顺序快照。

JSON 是重要的数据边界。`StatsOptions` 将可选 `StatsWindowSettings` 的字段扁平到对象顶层；反序列化时只要四个 window 字段任一存在就重建窗口。`sql_mode_json` 把 `mysql::SQLMode` 作为整数传输。`TimeZoneLocation::location` 和 `TableInfo::DBID` 不序列化，分别属于进程缓存和运行时附加信息。

## 依赖与调用关系

编译边界由 `pkg/meta/model/internal/group1/Cargo.toml` 给出：本文件依赖 parser AST/auth/charset/mysql/types 提供 SQL 元数据类型，依赖 planner-base 的 `Hasher`，依赖 `chrono` 表示时间，依赖 `humantime` 经 group1 的 `duration::ParseDuration` 解析间隔，依赖 `astersql-errors` 统一错误，并用 `serde` 实现协议 JSON。`column.rs` 的 `ColumnInfo` 和 `index.rs` 的 `IndexInfo`/`IndexColumn` 与本文件在同一 crate 内直接共享类型身份。

上游主要是 DDL、infoschema、planner、executor、session 和 TTL 等模块。已核实的直接生产边包括：`pkg/ddl/create_table.rs -> NewTableAffinityInfoWithLevel`，`pkg/ddl/persistent_drop_column.rs`/`persistent_modify_column.rs -> TableInfo::MoveColumnInfo`，以及两个 session 物化视图运行模块 `-> MaterializedViewLogTableName`。RustCodeGraph 还将 `pkg/ddl/create_table.rs`、`pkg/ddl/job_worker.rs`、`pkg/ddl/persistent_create_materialized_view.rs` 和 `persistent_create_materialized_view_log.rs` 列为文件级使用者。

下游调用大多是纯值计算：列/索引标志检查委托给 `mysql::*Flag`，名字规范化委托给 `ast::CIStr` 和 `NormalizeTableAffinityLevel`，更新时间委托给 `TSConvert2Time`，TTL 委托给 `duration::ParseDuration`。本文件不访问网络、磁盘、KV、事务或异步运行时。

## 错误处理与边界

大多数查询用 `Option`、空字符串或 `-1` 表示未找到：例如 `FindColumnByID` 返回 `Option`，`FindIndexNameByID` 返回空串，分区名查找返回 `-1`。这些约定与 Go 版本一致但不应混用；新增调用者必须按具体方法契约判断缺失值。

`TTLInfo::GetJobInterval` 和 `NewTableAffinityInfoWithLevel` 显式返回 `Result`。前者保留旧表空 `JobInterval` 对应 `1h` 的升级语义，并把非法时长解析错误向上传播；后者把空串/`none` 返回为 `Ok(None)`，`table`/`partition` 返回规范化对象，其余值返回错误。`TimeZoneLocation::get_location` 在名称为空且 offset 为零时返回字符串错误。

需要调用方保证的前置条件也很重要：`MoveColumnInfo` 直接 `remove(from)`/`insert(to)`，下标越界会 panic；`IsDropping` 直接索引 `Definitions[index]`，只能在已验证的非负有效下标上调用。`GetOverlappingDroppingPartitionIdx` 自身先验证边界。`RwLock::read/write().unwrap()` 遇到锁中毒会 panic。未知数值枚举多数返回空字符串，唯 `TableLockState` 回退为 `none`；这属于兼容行为而非严格校验。

## 并发与资源生命周期

绝大多数结构是拥有数据的同步快照，修改方法需要 `&mut self`，因此并发协调由持有者负责；文件不创建线程、任务、通道或事务。`Clone` 对嵌套 `Vec`、`String` 和 `Option` 做拥有式复制，使 DDL 修改副本不与原快照共享可变集合。

唯一显式共享状态是 `TimeZoneLocation::location: RwLock<Option<Arc<TimeZone>>>`。`get_location` 先持读锁检查缓存，缺失后取得写锁并二次检查，再创建一次 `Arc` 写入；并发读取共享同一不可变时区对象。其自定义 `Clone` 在读锁下复制缓存中的 `Arc`，新对象拥有独立的 `RwLock`。隐藏列名使用 `LazyLock<CIStr>` 做一次性线程安全初始化。除此之外资源均随 Rust 所有权自动释放。

## 与 Go 版本的对应关系

`pkg/meta/model/table.go` 是直接语义基准。Rust 保留了 Go 的公开符号命名、TableInfo V0-V5、字段 JSON 名、零值枚举、主键选择、列移动、分区 DDL、外键格式化、统计窗口和 TTL 升级兼容逻辑。Go 的指针/`nil` 多映射为 Rust 的引用/`Option`，Go 的手写深拷贝循环多由派生 `Clone` 承担，Go 的 `map` 对应 `HashMap`，因此 `GetNonTempColumns` 同样不承诺顺序。

已知实现形态差异包括：Rust `TableInfo::Equals` 接受 `Any` 并仅比较同类型 ID，无法表达 Go nil 接收者；Rust `Cols` 用 `Vec<Option<&ColumnInfo>>` 明确表示 Offset 空洞；Rust `StatsOptions` 通过辅助引用/拥有结构实现 Go 的扁平 JSON；Rust 的时区缓存是 `RwLock<Option<Arc<_>>>`。Go `TTLInfo::GetJobInterval` 可由 failpoint 覆盖，Rust 当前不接该 failpoint，只保留默认值与解析错误路径。任何字段、默认值或序列化键变更都必须先与 `table.go` 和兼容数据核对。

独立测试对应关系明确：Go `pkg/meta/model/table_test.go` 覆盖 `MoveColumnInfo`、基本模型、TTL clone/间隔、storage class、物化视图 clone 和重组清理；Rust `pkg/meta/model/table_test.rs` 覆盖基础克隆、JSON 字段/嵌入、TTL 与重组清理，`pkg/meta/model/table_4_aster_unit_test.rs` 进一步覆盖 Offset 全引用更新、隐式主键、分区重叠/忽略 ID、外键文本、TTL 错误和亲和性规范化。

## 扩展指南

- 新增持久化字段时，优先修改 `TableInfo` 或对应子结构，并同步 `serde(rename/default/skip_serializing_if)`、`pkg/meta/model/table.go` 的 JSON 协议和独立 Rust/Go 测试；不要在兼容 group 建立第二套类型。若字段含嵌套集合，确认派生 `Clone` 是否足够，并评估旧 JSON 缺字段的默认值。
- 新增列重排相关引用时，必须把该 Offset 引用加入 `MoveColumnInfo` 的旧到新映射更新，并在 `table_4_aster_unit_test.rs` 增加级联断言；仅调整 `Columns` 顺序会破坏索引或 online modify-column 状态。
- 扩展分区 DDL action/state 时，应成组审查 `ClearReorgIntermediateInfo`、重叠分区方法与 `IDsInDDLToIgnore`，并为每个 schema state 增加测试。读回退不得被写路径复用，因为写入正在删除的范围必须继续失败。
- 新增时间或调度配置时，明确“缺失、空、零、非法”的兼容语义；TTL 的空值 `1h` 是升级契约，不能简单替换成当前默认 `24h`。涉及并发缓存时避免在锁内调用未知外部代码，并补充锁中毒/缓存共享策略说明。
- 性能上，`Cols`、`MoveColumnInfo`、`GetPrimaryKey` 和多种查找均线性扫描并可能分配临时集合；表宽或索引多时应避免在热循环重复调用。若引入缓存，需要同时解决元数据 clone/修改后的失效问题。
- 测试必须继续放在独立文件，优先扩展 `pkg/meta/model/table_test.rs` 或 `table_4_aster_unit_test.rs`，并用 `pkg/meta/model/table_test.go` 校验 Go 意图；不要把测试内嵌到 `table.rs`。

## 验证依据

- RustCodeGraph：`status` 确认索引含 11,467 文件、目标文件已索引；`node --file pkg/meta/model/table.rs --offset 1/560/1150` 阅读全部 1,705 行；`query TableInfo --kind struct`、`query PartitionInfo --kind struct` 区分 Go/Rust 同名定义。文件节点报告 57 个使用文件。重名符号的 `callers` 未产生边，因此仅对直接入口用精确 `rg` 补证。
- 源码与边界：`pkg/meta/model/table.rs`、`pkg/meta/model/internal/group1/lib.rs`、`pkg/meta/model/internal/group1/Cargo.toml`、`pkg/meta/model/lib.rs`、`pkg/meta/model/Cargo.toml`。
- Go 对照：`pkg/meta/model/table.go`；方法签名逐项核对了 `TableInfo`、`PartitionInfo`、`PartitionDefinition`、`FKInfo`、`StatsOptions`、`TTLInfo` 和亲和性构造函数。
- 测试证据：`pkg/meta/model/table_test.rs`、`pkg/meta/model/table_4_aster_unit_test.rs`、`pkg/meta/model/table_test.go`。关键断言覆盖 public Offset 空洞、列移动引用一致性、显式/隐式主键、分区回退与忽略 ID、外键格式、JSON 字段、TTL 默认/错误和非法亲和性。
- 调用点证据：`pkg/ddl/create_table.rs`、`pkg/ddl/persistent_drop_column.rs`、`pkg/ddl/persistent_modify_column.rs`、`pkg/session/runtime/mview_ddl.rs`、`pkg/session/runtime/mlog_purge.rs`。
- 本任务是只读行为分析和文档新增，按计划不运行 Cargo；最终以任务给出的 11 章节结构命令验证，并人工检查每项结论均指向上述符号或文件。
