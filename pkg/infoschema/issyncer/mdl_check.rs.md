# `pkg/infoschema/issyncer/mdl_check.rs` 逻辑说明

## 文件定位

`mdl_check.rs` 位于 `astersql-infoschema-issyncer` crate。crate 入口 `pkg/infoschema/issyncer/lib.rs` 通过私有模块 `mod mdl_check` 装配它，再以 `pub use mdl_check::*` 导出其公开项；因此外部代码通常使用 crate 根上的 `JobMDL` 和 `MDLCheckTableInfo`，而不是直接引用模块路径。`pkg/infoschema/issyncer/Cargo.toml` 指定 `lib.rs` 为库入口，并把该 crate 标记为 Go 包 `pkg/infoschema/issyncer` 的移植单元。

该文件不是 MDL 后台循环本身，而是循环与刷新路径共享的内存快照容器。拥有者是 `pkg/infoschema/issyncer/syncer.rs` 中的 `Syncer::mdlCheckTableInfo` 字段：SQL 刷新路径写入它，MDL 检查路径读取它。文件没有条件编译项、异步任务、I/O 或 Cargo feature 分支。

## 核心职责

文件承担三项局部职责：

1. 用 `JobMDL` 表示一条待处理 DDL 作业的目标 schema 版本及其覆盖的物理表 ID。
2. 用 `mdlCheckTableInfo` 在同一把互斥锁下保存“最近观察到的 schema 版本 + 该版本对应的作业映射”，避免读者观察到跨刷新批次的组合状态。
3. 向 `Syncer` 提供整体替换、复制快照和按表 ID 查询三个小型操作；实际读取 `mysql.tidb_mdl_info`、跨 keyspace 过滤、旧事务检查和 schema 版本发布均在 `syncer.rs` 中完成。

这里保存的是最新刷新结果，不是追加日志，也不代表事务当前持有的锁。`InfoSchemaCoordinator::CheckOldRunningTxn` 会处理从快照克隆出的局部 `jobs`，不会回写本容器。

## 主要符号

- `JobMDL`（公开结构体，`mdl_check.rs:16`）：`Ver: i64` 是作业等待发布的 schema 版本；`TableIDs: HashSet<i64>` 是作业涉及的物理表 ID 集合。它实现 `Clone`、`Debug`、`Default`、`Eq` 和 `PartialEq`，便于复制快照和测试比较。集合天然去重，但不保留顺序。
- `mdlCheckTableInfo`（公开但采用 Go 风格小写名称的结构体，`mdl_check.rs:25`）：仅含私有字段 `mu: Mutex<mdlCheckTableInfoState>`，调用者不能绕开锁直接修改状态。
- `mdlCheckTableInfoState`（文件私有结构体，`mdl_check.rs:30`）：把 `newestVer` 与 `jobs: HashMap<i64, JobMDL>` 聚合为单一锁保护单元。默认值是版本 `0` 和空映射。
- `mdlCheckTableInfo::replace`（`mdl_check.rs:36`）：取得互斥锁后同时替换版本与整个作业映射，不做合并、排序或过滤。
- `mdlCheckTableInfo::snapshot`（`mdl_check.rs:43`）：在锁内复制版本和作业映射，返回拥有所有权的一致快照，释放锁后调用者可独立修改它。
- `mdlCheckTableInfo::contains`（`mdl_check.rs:49`）：在锁内扫描所有作业的 `TableIDs`，任一集合包含目标 ID 即返回 `true`。
- `MDLCheckTableInfo`（公开类型别名，`mdl_check.rs:59`）：把对外名称与内部 Go 风格结构名分开；`Syncer` 以该别名声明字段并调用 `Default`。

文件没有 trait、自由函数、常量或 `impl Drop`。

## 执行流程

写入主链如下：

1. `Syncer::RefreshMDLFromSQL`（`syncer.rs:194`）取得当前 InfoSchema 版本、最小 DDL job ID，并通过 `MDLSessionPool::ReadMDLRows` 读取 MDL 行。
2. 它调用 `Syncer::refreshMDLCheckTableInfoWithJobs`（`syncer.rs:363`）。该函数先用 `skipMDLCheck` 删除跨 keyspace Syncer 不关心的用户表作业。
3. 过滤后的完整 `HashMap<i64, JobMDL>` 与 `newestVer` 一次性传入 `replace`。旧映射整体被丢弃，新映射成为后续检查的唯一来源。

读取与发布主链如下：

1. `Syncer::CheckMDL` 进入 `check_mdl_with_context`（`syncer.rs:217`），后者经 `mdlCheckSnapshot` 调用 `snapshot`。
2. `snapshot` 在锁内克隆映射，然后立即释放锁；耗时的旧事务检查和版本发布都发生在锁外。
3. 检查循环根据版本和 `MDLProgress` 跳过无变化的空闲轮次；若存在作业，则让 `InfoSchemaCoordinator::CheckOldRunningTxn` 从局部映射中移除仍被旧事务阻挡的作业。
4. 剩余作业通过 schema version syncer 的 `UpdateSelfVersion` 发布其 `JobMDL::Ver`。本文件不参与发布结果缓存或重试决策。

另有两条辅助路径：`Syncer::refreshMDLCheckTableInfo` 可把 `job_id -> version` 转为 `JobMDL`（表集合为空）后写入；`Syncer::mdlCheckContains` 将表级存在性查询转发给 `contains`。

## 数据与状态

状态不变量是 `newestVer` 与 `jobs` 始终在同一临界区内更新和读取。`replace` 不允许出现“新版本配旧映射”或相反组合；`snapshot` 返回的二元组也来自同一时刻的锁内状态。

`jobs` 的键是 DDL job ID，值是该作业的 `JobMDL`。重复 job ID 在构造 `HashMap` 时只能保留一个值；同一作业内重复表 ID 被 `HashSet` 去重。`newestVer` 没有在容器内部执行单调性校验，调用者可以写入相同或更小的版本；跳过旧轮次的策略属于 `syncer.rs` 的 `MDLProgress`。

`Default` 会创建可用的空状态，因此 `newSyncer`（`syncer.rs:164`）无需额外初始化映射。空映射与版本 `0` 都是合法状态；空 `TableIDs` 的作业仍保存在映射中，但 `contains` 不会为其命中任何表。

## 依赖与调用关系

本文件的直接标准库依赖只有 `HashMap`、`HashSet` 和 `Mutex`，没有直接外部 crate 依赖。crate 边界由 `pkg/infoschema/issyncer/Cargo.toml` 定义；与 MDL 主链直接相关的下游协议位于 `astersql-ddl-schemaver`、`astersql-ddl-systable` 和 `astersql-sessionctx-vardef`，但都由 `syncer.rs` 使用，不由本文件直接调用。

已核对的直接调用边：

- `newSyncer -> MDLCheckTableInfo::default`，创建每个 `Syncer` 私有的状态容器。
- `RefreshMDLFromSQL -> refreshMDLCheckTableInfoWithJobs -> mdlCheckTableInfo::replace`，刷新生产快照。
- `refreshMDLCheckTableInfo -> mdlCheckTableInfo::replace`，写入不带表 ID 的兼容/测试数据。
- `check_mdl_with_context -> mdlCheckSnapshot -> mdlCheckTableInfo::snapshot`，取得供旧事务检查和版本发布使用的副本。
- `mdlCheckContains -> mdlCheckTableInfo::contains`，为测试和上层状态观察提供表级查询。

RustCodeGraph 将目标文件识别为 7 个符号，并定位到 `syncer.rs` 的 `refreshMDLCheckTableInfo`、`refreshMDLCheckTableInfoWithJobs`。当前图索引未返回这三个方法的 caller/callee 边，因此上述精确边由 `rg` 对 Rust 直接引用补齐。

## 错误处理与边界

三个方法都没有 `Result` 返回值。唯一隐式失败点是 `Mutex::lock().unwrap()`：若持锁线程发生 panic 导致互斥锁 poisoned，后续 `replace`、`snapshot` 或 `contains` 会继续 panic，而不是恢复旧状态或返回错误。这与当前实现一致，扩展时不能把它误写成可恢复错误路径。

`replace` 接受调用者提供的任意版本和映射，不校验 job ID、schema 版本、表 ID 是否为负数，也不执行 cross-keyspace 过滤；这些输入契约由 `Syncer` 的调用路径负责。`contains` 是全量线性扫描，复杂度近似为作业数及其表集合查询之和；它不告诉调用者命中了哪个作业。`snapshot` 克隆整张映射和每个 `HashSet`，其时间与额外内存均随当前 MDL 作业及表 ID 数量增长。

`JobMDL::default()` 产生版本 `0` 和空表集合；这只是 Rust 的零值语义，不证明版本 `0` 是业务上可发布的有效 schema 版本。

## 并发与资源生命周期

`Mutex<mdlCheckTableInfoState>` 提供线程间互斥；只要字段类型满足标准库约束，`Syncer` 可通过共享引用并发调用 `replace`、`snapshot` 和 `contains`。没有 `RwLock`：读操作之间也会互斥，但临界区很短。

`replace` 的参数按值移入容器，调用者不能在返回后继续修改已发布映射。`snapshot` 通过深层 `Clone` 切断读取者与容器的所有权联系，保证 `CheckOldRunningTxn` 删除局部条目以及后续发布遍历不会阻塞下一次刷新，也不会改变共享快照。`contains` 在完成扫描前持锁；作业很多时可能短暂阻塞 `replace` 或 `snapshot`。

文件本身不创建线程、channel、定时器、网络连接或会话，也不需要显式清理。容器随所属 `Syncer` 一起销毁；旧映射在成功替换且无其他所有者时释放，而已返回的快照由其调用者独立持有至作用域结束。

## 与 Go 版本的对应关系

Go 对照文件 `pkg/infoschema/issyncer/mdl_check.go` 只定义 `mdlCheckTableInfo`，字段为 `sync.Mutex`、`newestVer int64` 和 `map[int64]*mdldef.JobMDL`。Rust 保留了同样的“单锁保护版本与作业映射”布局意图，但存在以下明确差异：

- Go 在 `syncer.go:181-195` 直接持锁、更新字段并构造 `mdldef.JobMDL`；Rust 把整体替换封装为 `replace`。
- Go 的检查循环在 `syncer.go:234-252` 手工锁定、用 `maps.Clone` 浅克隆 `map[int64]*JobMDL` 后解锁；Rust 的 `snapshot` 克隆 `HashMap<i64, JobMDL>` 及其中集合，是独立深层值快照。
- Go 的表集合类型是 `map[int64]struct{}`，Rust 对应为 `HashSet<i64>`；两者都表达无序去重集合。
- Go 的 `JobMDL` 定义来自子包 `issyncer/mdldef`；当前 Rust 主 crate 在本文件中另行公开定义 `JobMDL`。仓库同时存在 `pkg/infoschema/issyncer/mdldef/mdl.rs` 的同名类型，但活动 `issyncer` Cargo 依赖未启用该 mdldef crate，二者不是可互换的 Rust 类型。
- `snapshot`、`contains` 和 `MDLCheckTableInfo` 别名是 Rust 为封装与调用便利增加的 API，不是 Go 文件中的同名方法或别名。

行为对照还需结合 `pkg/infoschema/issyncer/syncer.go`：Rust 的跨 keyspace 过滤、旧事务检查、无变化轮次跳过、作业发布缓存等逻辑仍位于 `syncer.rs`，不应归因于本文件。

## 扩展指南

若新增字段必须与 `newestVer` 和 `jobs` 保持同一刷新批次，应该把字段加入 `mdlCheckTableInfoState`，并同时更新 `replace` 参数/赋值、`snapshot` 返回模型和 `Syncer` 的调用方；不要另加一把独立锁，否则会破坏快照一致性。同步扩展 `pkg/infoschema/issyncer/syncer_test.rs`，至少覆盖替换后读取、旧快照不受新替换影响、空状态和跨 keyspace 过滤后的状态。

若需要高频按表查询，应先评估在 state 中维护反向索引，而不是直接让 `contains` 返回更多信息；反向索引必须在 `replace` 中原子重建，并验证重复表 ID、多作业覆盖同一表及空集合。任何缓存都会增加刷新成本和一致性风险。

若改变 `JobMDL` 的字段或语义，需要同步检查 `MDLSessionPool::ReadMDLRows`、`InfoSchemaCoordinator::CheckOldRunningTxn`、`refreshMDLCheckTableInfoWithJobs`、`check_mdl_with_context`，以及 `pkg/domain/crossks/coordinator.rs` 和 server/session 集成边界。还应明确与 `issyncer/mdldef/mdl.rs` 的类型关系，避免两个同名模型继续漂移。

若要把 poisoned mutex 改为可恢复错误，必须连同三个方法及 `Syncer` 转发 API 设计错误传播；静默忽略 poisoned 状态会掩盖可能的中途 panic 和状态不变量破坏。性能变更需重点衡量 `snapshot` 深克隆成本与锁持有时间之间的取舍。

## 验证依据

- 目标实现：`pkg/infoschema/issyncer/mdl_check.rs`，完整读取 59 行，确认 3 个类型、3 个方法和 1 个类型别名，无条件编译项。
- crate 装配：`pkg/infoschema/issyncer/lib.rs` 的 `mod mdl_check` / `pub use mdl_check::*`；`pkg/infoschema/issyncer/Cargo.toml` 的库入口、移植元数据和依赖声明。
- Rust 调用方：`pkg/infoschema/issyncer/syncer.rs` 的 `newSyncer`、`RefreshMDLFromSQL`、`check_mdl_with_context`、`refreshMDLCheckTableInfo`、`refreshMDLCheckTableInfoWithJobs`、`mdlCheckSnapshot`、`mdlCheckContains`。
- Rust 测试：`pkg/infoschema/issyncer/syncer_test.rs::test_syncer_skip_mdl_check` 验证跨 keyspace 过滤、版本/作业快照及表查询；`pkg/session/runtime/normal_ddl_test.rs` 中 MDL publication 回归段验证后台 SyncLoop 从 `mysql.tidb_mdl_info` 刷新目标表和版本，并最终发布 job schema 版本。
- Go 对照：`pkg/infoschema/issyncer/mdl_check.go` 的锁保护状态；`pkg/infoschema/issyncer/syncer.go` 的 `refreshMDLCheckTableInfo`、`skipMDLCheck` 与 `MDLCheckLoop`；`pkg/infoschema/issyncer/syncer_test.go::TestSyncerSkipMDLCheck` 的普通/跨 keyspace 分支。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件和 307,296 个节点；`files --filter pkg/infoschema/issyncer` 识别目标文件 7 个符号；`node --file ...` 返回完整源码；`query JobMDL` / `query mdlCheckTableInfo` 定位目标符号及 `syncer.rs` 刷新入口。目标方法的 callers/callees 未由图返回，因此用直接引用搜索补证。
- 本任务只新增说明文档，未运行 Cargo 或代码测试；行为结论来自已有源代码与测试，不把“测试文件存在”表述为本轮已执行通过。
