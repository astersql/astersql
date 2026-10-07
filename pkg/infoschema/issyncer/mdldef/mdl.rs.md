# `pkg/infoschema/issyncer/mdldef/mdl.rs`

源文件：[mdl.rs](./mdl.rs)

## 文件定位

本文件属于独立 crate `astersql-infoschema-issyncer-mdldef`，crate 边界由同目录 `Cargo.toml` 定义，入口 `lib.rs` 通过 `pub mod mdl` 声明模块并用 `pub use mdl::*` 导出其中的类型。根工作区 `Cargo.toml` 同时把该目录列为 workspace member，并以 `facade_infoschema_issyncer_mdldef` 暴露门面依赖。

文件从同路径 Go 文件 `mdl.go` 移植，只定义 DDL 元数据锁（Metadata Lock，MDL）检查所需的数据结构 `JobMDL`。把该结构放在独立子 crate 中延续了 Go 子包用来避开上层包循环依赖的边界：例如 `pkg/session/sessmgr/Cargo.toml` 依赖此 crate，`pkg/session/sessmgr/lib.rs::mdldef` 再将其重导出给会话和服务端代码。

需要特别区分 `pkg/infoschema/issyncer/mdl_check.rs::JobMDL`：后者是 issyncer 主 crate 中读取和缓存 `mysql.tidb_mdl_info` 行的同名结构，字段为 `Ver`/`TableIDs`；本文件的类型字段为 `ver`/`table_ids`，用于会话侧事务 MDL 检查。两者在 `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator::CheckOldRunningTxn` 等边界显式转换，并非同一 Rust 类型。

## 核心职责

`JobMDL` 把一个待推进 DDL 作业的两项约束组合在一起：实例至少应加载到的 schema 元版本 `ver`，以及该作业涉及的物理表 ID 集合 `table_ids`。下游检查者用表 ID 找出与当前事务重叠的表，再比较事务持有的版本是否小于 `ver`；仍有旧事务时保留该作业，否则从待阻塞集合移除。实际筛选逻辑位于 `pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs`，不在本文件中。

本文件只承担跨 crate 共享的数据契约，不负责从系统表读取作业、不维护按作业 ID 索引、不加锁、不推进 DDL，也不直接判断某个事务是否阻塞作业。

## 主要符号

- `pub struct JobMDL`：唯一的生产符号，派生 `Debug` 和 `Default`。它没有自定义构造器或方法，也没有派生 `Clone`、`Eq`、序列化等额外能力。
- `pub ver: i64`：DDL 作业要求会话所用 schema 达到的最低版本。`Default` 将其置为 `0`；类型本身不限制负值，独立测试也以 `-7` 证明字段是原样保存的普通有符号整数。
- `pub table_ids: HashSet<i64>`：作业涉及的物理表 ID 集合。集合消除重复 ID，查找不依赖顺序；`Default` 产生可直接插入的空集合。

两个字段均公开且可变，类型自身不验证版本范围、表 ID 合法性或集合是否为空。调用者必须维护这些语义约束。

## 执行流程

本文件没有可执行函数；它参与的典型运行链如下：

1. `pkg/infoschema/issyncer/syncer.rs` 读取并缓存待检查的 DDL 作业，使用主 crate 的 `issyncer::JobMDL` 表示系统表行。
2. `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator::CheckOldRunningTxn` 将每个主 crate 作业转换为本文件的 `JobMDL`：`Ver` 映射到 `ver`，`TableIDs` 克隆到 `table_ids`，并放入 `Arc`。
3. 普通 Domain 通过 `InfoSchemaCoordinator::CheckOldRunningTxn` 交给会话管理器；`pkg/server/server.rs::Server::CheckOldRunningTxn` 遍历连接，取得其 `TransactionMDL` 并调用 `check_jobs`。跨 Keyspace 路径在 `pkg/domain/crossks/coordinator.rs::RegisteredMDLSession::remove_lock_ddl_jobs` 做等价转换与检查。
4. `pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs` 对每个作业的 `table_ids` 查找事务已访问表；任一相关表版本小于 `job.ver` 时保留该作业，表示它仍被旧事务阻塞。没有这种表时删除该作业，调用方据此判断 DDL 是否可继续。

因此 `JobMDL` 是读取侧作业快照与事务侧版本状态之间的数据桥梁，而不是流程控制器。

## 数据与状态

`JobMDL` 的状态完全由两个拥有所有权的值组成：一个 `i64` 和一个 `HashSet<i64>`。它不持有引用、句柄或外部资源。集合表达的是成员关系而非顺序，重复插入同一表 ID不会增加元素数；任何依赖稳定遍历顺序、重复次数或原始输入顺序的逻辑都不应建立在此类型上。

默认值为 `ver == 0` 且 `table_ids` 为空，已由 `migration_aster_unit_test.rs::job_mdl_default_matches_go_zero_value` 覆盖。`job_mdl_tracks_each_table_once` 覆盖公开字段赋值和集合去重。因为没有私有字段或构造器，不变量不能由类型封装强制保证。

生产调用通常把值包装为 `Arc<JobMDL>`，使多个连接检查过程可共享只读作业描述；`JobMDL` 本身并不包含引用计数或同步原语。

## 依赖与调用关系

直接依赖只有标准库 `std::collections::HashSet`；同目录 `Cargo.toml` 没有声明第三方依赖或 feature。`lib.rs` 是直接上游模块入口，`pkg/session/sessmgr/lib.rs::mdldef` 是主要重导出门面。

已验证的直接消费位置包括：

- `pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs` 读取 `table_ids` 和 `ver` 完成阻塞作业筛选。
- `pkg/session/sessmgr/processinfo.rs::InfoSchemaCoordinator::CheckOldRunningTxn` 在接口签名中传递 `HashMap<i64, Arc<mdldef::JobMDL>>`。
- `pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator::CheckOldRunningTxn` 从 issyncer 同名类型构造本类型。
- `pkg/server/server.rs::Server::CheckOldRunningTxn` 遍历活动连接并调用事务 MDL 检查。
- `pkg/domain/crossks/coordinator.rs::RegisteredMDLSession::remove_lock_ddl_jobs` 在跨 Keyspace 内部会话链中构造并检查本类型。

RustCodeGraph 对目标文件给出的文件级使用边只指向同 crate 测试；跨 crate 使用经 `lib.rs` 和 `session/sessmgr` 的重导出发生，需结合 Cargo 依赖和上述限定路径解析。不要因文件级图只显示测试就判定生产未接线。

## 错误处理与边界

该类型不返回 `Result`、不产生自定义错误，也不会自行失败。所有公开字段都接受任意 `i64`：负版本、负表 ID、空表集合以及极值都能被构造；这些值是否有效由数据读取和调用链负责。

下游 `TransactionMDL::check_jobs` 的核心边界是严格小于比较：事务表版本 `version < job.ver` 才阻塞，相等版本满足要求。找不到某表的事务版本时，该表本身不会令作业保留；受限事务则在 `check_jobs` 开头直接返回，不改变待检查作业集合。这些是消费者行为，不应误加为 `JobMDL` 内部逻辑。

`HashSet` 无稳定迭代顺序，若未来需要持久化或日志的确定顺序，应在输出边界排序，而不应假定当前集合顺序。增加字段或收紧校验还会影响所有结构体字面量构造点及 Go/Rust 转换点。

## 并发与资源生命周期

`JobMDL` 不含锁、原子量、通道、任务、事务句柄或析构逻辑，生命周期遵循普通 Rust 所有权。生产链通常在构造后以 `Arc<JobMDL>` 共享只读数据；是否可跨线程使用来自成员类型的自动 trait，而非本文件中的显式并发协议。

真正的并发状态位于 `pkg/session/sessmgr/mdl.rs::TransactionMDL`：它用 `Mutex<HashMap<i64, i64>>` 保存事务访问的表版本，用 `AtomicBool` 标记受限会话。`pkg/server/server.rs::Server::CheckOldRunningTxn` 还在读锁保护下遍历连接。修改 `JobMDL` 时应保持它适合短期快照和只读共享，避免把连接或事务资源生命周期塞入该数据对象。

## 与 Go 版本的对应关系

Go 对照为 `pkg/infoschema/issyncer/mdldef/mdl.go::JobMDL`。映射关系为：Go `Ver int64` 对应 Rust `ver: i64`，Go `TableIDs map[int64]struct{}` 对应 Rust `table_ids: HashSet<i64>`。两者都把表 ID 表达为无附加值的集合，并保留 64 位有符号版本和 ID 语义。

Rust 的 `Default` 与 Go 零值在“版本为 0、集合为空/无成员”这一读取语义上相符，但内存与写入语义并不完全相同：Go 零值 `TableIDs` 是 `nil` map，直接写入前必须初始化；Rust 默认值是已分配语义上的空 `HashSet`，可直接插入。独立 Rust 测试只验证了可观察的零版本、空集合和去重，并未证明两者的所有构造行为完全等价。

Go 注释明确说明独立子包用于避免上层目录的 import cycle；Rust 通过独立 crate、Cargo 依赖和重导出保留了这个依赖方向。字段命名按 Rust snake_case 转换，当前没有 serde/wire 格式，因此字段名不是跨语言序列化协议。

## 扩展指南

若增加作业级约束，首先修改 `JobMDL`，随后逐一检查所有结构体字面量和转换点，尤其是 `NormalSchemaCoordinator::CheckOldRunningTxn`、`RegisteredMDLSession::remove_lock_ddl_jobs`、服务端测试和 session 测试。Go 行为仍是基准时，还应同步评估 `mdl.go::JobMDL`，不要只在 Rust 侧引入无法从系统表或 Go 路径填充的字段。

若改变版本或表集合判断，应优先修改真正拥有算法的 `TransactionMDL::check_jobs` 及其独立测试，而不是把运行逻辑塞进这个定义文件。若只是扩充本类型的默认值、集合语义或调试表现，应继续在同目录独立测试 `migration_aster_unit_test.rs` 中覆盖，遵守“源文件与测试文件分离”。跨链行为可同步覆盖 `pkg/server/runtime_test.rs`、`pkg/session/runtime/normal_ddl_test.rs` 和 `pkg/domain/crossks/*_test.rs` 的现有 MDL 场景。

兼容性风险包括：新增非默认字段会破坏字面量构造；改变 `i64` 或集合表示会影响 Go 对齐及比较语义；增加内部可变性会改变当前廉价只读共享模型；依赖集合遍历顺序会带来不确定测试或日志。性能上，当前从 issyncer 类型到本类型会克隆表集合，大型作业扩展应评估克隆成本，但本文件没有证据表明当前存在性能瓶颈。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标 Rust/Go 文件；`files --filter pkg/infoschema/issyncer/mdldef` 确认 `lib.rs`、`mdl.rs`、`mdl.go` 和独立测试；`node --file .../mdl.rs` 核对完整 35 行源码；`query JobMDL` 区分三处同名定义；对 `session/sessmgr`、`normal_ddl_service`、`server`、`domain/crossks` 的文件节点核对重导出、转换和消费链。
- crate 与装配：`pkg/infoschema/issyncer/mdldef/Cargo.toml`、同目录 `lib.rs`、`pkg/session/sessmgr/Cargo.toml`、`pkg/session/sessmgr/lib.rs`、根 `Cargo.toml`。
- Rust 生产证据：`pkg/session/sessmgr/mdl.rs::TransactionMDL::check_jobs`、`pkg/session/sessmgr/processinfo.rs::InfoSchemaCoordinator`、`pkg/session/runtime/normal_ddl_service.rs::NormalSchemaCoordinator`、`pkg/server/server.rs::Server::CheckOldRunningTxn`、`pkg/domain/crossks/coordinator.rs::RegisteredMDLSession`。
- Go 对照：`pkg/infoschema/issyncer/mdldef/mdl.go`；相关使用路径包括 `pkg/infoschema/issyncer/mdl_check.go`、`pkg/infoschema/issyncer/syncer.go`、`pkg/session/sessmgr/processinfo.go` 和 `pkg/server/server.go`。
- 测试证据：`pkg/infoschema/issyncer/mdldef/migration_aster_unit_test.rs` 直接覆盖默认值与去重；`pkg/infoschema/issyncer/syncer_test.rs::test_syncer_skip_mdl_check` 覆盖上游作业快照筛选；`pkg/server/runtime_test.rs` 的 MDL 生命周期场景覆盖本类型向 `TransactionMDL::check_jobs` 的真实接线。
- 本任务是纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令和人工事实复核验证。
