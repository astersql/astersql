# `pkg/util/domainutil/repair_vars.rs`

## 文件定位

本文件是 `astersql-util-domainutil` crate 的修复模式状态实现，对应 Go 文件 `pkg/util/domainutil/repair_vars.go`。crate 入口 `pkg/util/domainutil/lib.rs` 将本模块声明为私有模块，再用 `pub use repair_vars::*` 重导出其公开项；元数据类型则由 `lib.rs` 的 `model` 模块从 `astersql-meta-model` 重导出。

它保存 `ADMIN REPAIR TABLE` 所需的进程级状态：修复模式开关、配置指定的 `db.table` 名单，以及从元数据加载过程中隔离出来的待修复 `DBInfo`/`TableInfo`。需要注意迁移状态：该 crate 已在根 workspace 中登记，也被 `pkg/ddl`、`pkg/infoschema` 和 `pkg/infoschema/issyncer` 的 Cargo manifest 声明为依赖，但代码搜索没有找到这些生产 Rust 模块对本文件 API 的直接调用；当前 Rust 服务启动路径 `cmd/tidb-server/main.rs` 使用的仍是 `cmd/tidb-server/stubs.rs` 中独立的精简 `domainutil` 状态桩，而不是这里的 `RepairInfo`。

## 核心职责

- 用 `repairInfo` 集中维护 `repairMode`、规范化后的 `repairTableList` 和按数据库 ID 分组的 `repairDBInfoMap`。
- 依据大小写不敏感的 `db.table` 名称，在元数据加载时判断表是否属于修复名单，并把命中的表缓存到数据库副本中。
- 为规划/执行修复提供按库筛选表 ID、按名称查询缓存、修复完成后清理状态等操作。
- 通过全局 `RepairInfo: LazyLock<RwLock<repairInfo>>` 提供进程级、惰性初始化且受读写锁保护的共享容器。
- 用 `repairKeyType::{RepairedTable, RepairedDatabase}` 复刻 Go session context 的两个键及其稳定字符串表示。

本文件只管理状态与名称匹配，不解析 SQL、不校验修复 DDL 的列/索引/分区兼容性，也不执行元数据事务。Go 版本中这些上层职责分别位于 `pkg/planner/core/preprocess.go` 与 `pkg/ddl/executor.go`。

## 主要符号

- `repairInfo`：内部字段均不公开的状态结构。`repairDBInfoMap: HashMap<i64, model::DBInfo>` 按数据库 ID 保存副本；每个副本的 `Deprecated.Tables` 只放已命中的修复表。`repairTableList: Vec<String>` 保存配置名单；`repairMode: bool` 是总开关。
- `repairInfo::new`：建立空 map、空列表和关闭的模式。它同时服务于全局惰性初始化和公开构造函数 `init`。
- `InRepairMode` / `SetRepairMode`：读取或设置模式开关。
- `GetRepairTableList` / `SetRepairTableList`：借用当前名单，或把传入的所有名称转成小写后整体替换名单。
- `GetMustLoadRepairTableListByDB`：从当前名单选出指定数据库下的表，再遍历调用者提供的大小写敏感 `tableName2ID`，返回匹配的表 ID。
- `CheckAndFetchRepairedTable`：模式开启且 `di.Name.L + "." + tbl.Name.L` 命中名单时，将表放入数据库缓存并返回 `true`。
- `GetRepairedTableInfoByTableName`：返回 `(表, 库)` 两级命中结果；数据库存在但表不存在时特意返回 `(None, Some(db))`，供调用者区分两种错误。
- `RemoveFromRepairInfo`：从配置名单和数据库缓存各删除一个匹配项；数据库没有剩余表时删除数据库，整个 map 为空时自动关闭模式。
- `RepairInfo`：`LazyLock<RwLock<repairInfo>>` 全局实例。类型本身公开，但其守卫内的具体类型 `repairInfo` 非公开，因此 crate 外部的实际可用性还受 Rust 可见性接口约束；当前仓库没有找到生产 Rust 调用点。
- `repairKeyType` 与 `String`：两个 session 缓存键及字符串映射。枚举类型公开，变体分别返回 `"RepairedTable"` 和 `"RepairedDatabase"`。
- `init() -> repairInfo`：公开函数，返回独立的空状态，当前由同 crate 的 `migration_aster_unit_test.rs` 构造隔离测试实例；它不是 Rust 的自动模块初始化钩子。

## 执行流程

1. 初始化时，访问 `RepairInfo` 会触发 `LazyLock`，在 `RwLock` 内创建关闭模式、空名单和空缓存；测试则直接调用 `init()` 获取独立实例。
2. 配置阶段先调用 `SetRepairMode(true)`，再以 `SetRepairTableList` 写入目标 `db.table`；后者立即把名单统一为小写。
3. 若元数据读取采用“名称到 ID”索引，`GetMustLoadRepairTableListByDB` 先以 `dbName + "."` 过滤名单并建立集合，再逐项遍历 `tableName2ID`，对完整名称小写化后匹配，以确保配置中的表仍被显式加载。
4. 每张候选表进入 `CheckAndFetchRepairedTable`：模式关闭立即返回 `false`；模式开启时按 `DBInfo.Name.L`、`TableInfo.Name.L` 组成名称并扫描名单。未命中返回 `false`。
5. 命中时，已有数据库缓存就把同一个 `Arc<TableInfo>` 追加到 `Deprecated.Tables`；首次命中则通过 `DBInfo::Copy()` 建立数据库副本，清空其原表集合并只放当前表，随后按 `DBInfo.ID` 插入 map。
6. 上层可用 `GetRepairedTableInfoByTableName` 查询三态结果：表和库都命中、只命中库、库也未命中。Go 主链据此分别报告“table is not in repair”和“database is not in repair”，并把命中的对象写入 session context。
7. 修复成功后，`RemoveFromRepairInfo` 删除名单中的第一个匹配名称和缓存数据库中的第一个匹配表；空数据库条目被移除，最后一个数据库移除后 `repairMode` 自动变为 `false`。

## 数据与状态

名称比较依赖元数据的规范化字段 `Name.L`，而不是展示用的 `Name.O`。`SetRepairTableList` 已把输入转为小写，但其他方法仍对名单项调用小写转换，保留与 Go 版本相同的宽容匹配；`GetMustLoadRepairTableListByDB` 也会小写化调用者 map 中的原始表名。

`repairDBInfoMap` 拥有 `DBInfo` 值，但表集合持有 `Arc<TableInfo>`，因此缓存数据库副本与调用者共享表对象身份。首次缓存会复制数据库并替换其 `Deprecated.Tables`，不会把原 `DBInfo` 中的其他表带入修复缓存；独立 Rust 测试用 `Arc::ptr_eq` 验证命中表保持同一共享对象。

列表与 map 不强制去重：重复调用 `CheckAndFetchRepairedTable` 可重复追加相同表，`RemoveFromRepairInfo` 也只删除每个集合中的首个匹配项。`HashMap` 遍历没有稳定顺序，所以 `GetMustLoadRepairTableListByDB` 的返回 ID 顺序不应成为 API 契约；测试先排序后断言。

## 依赖与调用关系

直接依赖只有标准库的 `HashMap`、`HashSet`、`Arc`、`LazyLock`、`RwLock`，以及 crate 内 `model::{DBInfo, TableInfo}`。`pkg/util/domainutil/Cargo.toml` 将后者落实为对 `astersql-meta-model`（路径 `pkg/meta/model`）的依赖，没有 feature 条件或外部网络依赖。

RustCodeGraph 将本文件识别为 14 个符号；精确查询确认 `repairInfo`、八个状态方法、`RepairInfo`、`repairKeyType`、`String` 和 `init`。调用边中，`new` 由 `init` 以及文件内需要建立集合/容器的逻辑触达；仓库级 Rust 引用搜索只找到 `cmd/tidb-server/stubs.rs` 中同名但独立的 `SetRepairMode`/`SetRepairTableList`，没有找到本文件其他 API 的生产 Rust 调用者。

Go 对照主链提供了预期接线：`cmd/tidb-server/main.go` 写入启动配置；`pkg/infoschema/issyncer/loader.go` 选择必须加载的表并缓存修复对象；`pkg/planner/core/preprocess.go` 检查模式、查询缓存并写入 session context；`pkg/ddl/executor.go` 修复成功后调用 `RemoveFromRepairInfo`。这些是移植语义证据，不代表当前 Rust 主链已经完成同样接线。

## 错误处理与边界

本文件没有 `Result`、错误类型、日志或显式 panic 路径；查找失败由布尔值或 `Option` 组合表达。模式关闭、名单未命中、数据库未命中都属于正常分支。`GetRepairedTableInfoByTableName` 的 `(None, Some(db))` 不可折叠为完全未命中，因为 Go 上层依赖它生成不同诊断。

输入约定是 `schemaLowerName`、`tableLowerName` 和通常已规范化的 `dbName`。函数不会验证字符串是否包含恰好一个点，也不会拒绝空名称；它只做拼接和大小写比较。Unicode 小写行为采用 Rust `str::to_lowercase`，与 Go `strings.ToLower` 的目标一致，但文档证据没有证明所有 Unicode 边界逐字符完全等价。

锁中毒是 Rust 与 Go 的重要接口边界：方法本身只操作 `&self`/`&mut self`，不负责获取全局锁，也没有恢复 poisoned `RwLock` 的封装。未来生产调用者必须显式处理 `RepairInfo.read()`/`write()` 的 `LockResult`，不能假定这些方法自己加锁。

## 并发与资源生命周期

Go 的 `repairInfo` 内嵌 `sync.RWMutex`，每个方法自行加锁；Rust 把锁提升到全局值外层。这样单个 `repairInfo` 方法不含锁操作，调用者取得一次写守卫后可原子地执行一个方法，但跨多个方法的完整配置或修复流程是否原子，取决于调用者持有守卫的范围。

`LazyLock` 保证全局状态只初始化一次并存活到进程结束。`RwLock` 允许多个并行读者或一个写者；`Arc<TableInfo>` 让缓存表在移除后仍可由其他持有者继续存活。方法中没有异步任务、通道、文件句柄、网络连接或事务资源，也没有阻塞 I/O；主要成本来自名单/表集合的线性扫描、字符串分配与小写转换。

独立 Rust 单测使用 `init()` 而非全局 `RepairInfo`，从而避免并行测试共享状态。若新增针对全局实例的测试，必须提供串行化或可靠的状态复位，且仍应放在独立测试文件中。

## 与 Go 版本的对应关系

字段和主要分支逐项对应 `repair_vars.go`：Go 的 `map[int64]*DBInfo` 对应 Rust 的 `HashMap<i64, DBInfo>`，Go 的 `[]*TableInfo` 对应数据库副本里的 `Vec<Arc<TableInfo>>`，`slices.Delete` 对应 `Vec::remove`，包级 `init` 对应 `repairInfo::new`、`LazyLock` 与供测试使用的 `init()`。

Rust 保留了关键 Go 语义：设置名单时小写化；大小写不敏感地反查表 ID；模式关闭时拒绝缓存；首次命中只复制数据库并重建表集合；查询区分“库存在但表不存在”；清空缓存后关闭模式；两个 session key 的字符串不变。`pkg/util/domainutil/migration_aster_unit_test.rs` 对这些状态转换和边界逐项回归。

差异包括：Go getter 返回 slice，Rust 返回受 `&self` 生命周期约束的切片；Go 指针由 Rust 的值拥有权加 `Arc` 表达；Go 每个方法内部锁定，而 Rust 要求外层调用者持锁；Go 的未知 `repairKeyType` 可返回空字符串，Rust enum 不存在无效变体；Rust `init()` 是普通公开函数而非自动执行的语言钩子。最关键的迁移差异是当前 Rust 服务使用启动桩，尚未由实际 infoschema/planner/DDL Rust 主链完整消费本实现。

## 扩展指南

- 增加或改变修复状态时，优先修改 `repairInfo` 及相应方法，保持一次写守卫下的状态不变量；同步检查 Go 对照文件，不要擅自简化三态查询或自动关闭逻辑。
- 若把真实实现接入 Rust 服务，应替换 `cmd/tidb-server/stubs.rs` 的独立 `domainutil` 桩，并逐个落实 Go 主链的启动配置、infoschema 加载、planner session-key 传递和 DDL 完成清理；仅让服务引用 crate 并不足以证明行为完整。
- 若公开给 crate 外调用者，应审查 `RepairInfo` 暴露私有 `repairInfo` 的可用性，并提供清晰的锁访问 API，明确 poisoned lock 的错误策略。不要通过复制一份无锁状态绕开并发契约。
- 若优化性能，可考虑预规范化结构或集合索引，但必须保留名单顺序/重复项现状、大小写语义以及 Go 可观察行为，并用基准或真实调用规模证明收益。
- 测试应继续放在独立文件 `pkg/util/domainutil/migration_aster_unit_test.rs`，覆盖新增字段或分支；生产接线还应在对应 infoschema、planner、DDL 的独立测试中验证。不得把测试模块内嵌回 `repair_vars.rs`。
- 修改 session key 名称属于兼容性变更，因为 Go planner/executor 用它们在同一 session context 中传递对象；修改缓存对象所有权或锁粒度则需评估并发、内存存活期和锁中毒处理。

## 验证依据

- Rust 源码与符号：RustCodeGraph `node --file pkg/util/domainutil/repair_vars.rs --offset 1 --limit 400` 完整读取 232 行，并用 `query` 核对 `repairInfo`、各状态方法、`repairKeyType` 等定义；`files --filter pkg/util/domainutil` 确认模块内 Rust/Go 文件集合。
- crate 边界：读取 `pkg/util/domainutil/lib.rs` 与 `pkg/util/domainutil/Cargo.toml`；根 `Cargo.toml` workspace/依赖项及 `pkg/ddl/Cargo.toml`、`pkg/infoschema/Cargo.toml`、`pkg/infoschema/issyncer/Cargo.toml` 的反向声明由精确搜索确认。
- Rust 接线：搜索所有 `.rs` 中相关符号，只发现 `cmd/tidb-server/main.rs` 调用 `cmd/tidb-server/stubs.rs` 的独立桩；没有把这些同名边误认为本文件调用者。
- Go 对照与调用链：读取 `pkg/util/domainutil/repair_vars.go`，并核对 `cmd/tidb-server/main.go`、`pkg/infoschema/builder.go`、`pkg/infoschema/issyncer/loader.go`、`pkg/planner/core/preprocess.go`、`pkg/ddl/executor.go` 的直接调用。
- 测试证据：读取独立 Rust 测试 `pkg/util/domainutil/migration_aster_unit_test.rs`；读取 Go 回归 `pkg/ddl/repair_table_test.go` 的模式/名单/缓存/修复流程场景，以及 `tests/realtikvtest/importintotest/import_into_test.go` 的并发导入修复用例。按任务约束未运行 Cargo 或 Go 测试。
- 文档结构通过任务指定命令校验，固定标题数量必须恰为 11；人工复核重点是区分已实现 crate、Go 预期链路和当前 Rust 启动桩三种事实层级。
