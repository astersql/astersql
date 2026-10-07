# `br/pkg/utils/filter.rs`

## 文件定位

本文件属于 Cargo crate `astersql-br-pkg-utils`（`br/pkg/utils/Cargo.toml`），由 `br/pkg/utils/lib.rs` 通过 `#[path = "filter.rs"] pub mod filter` 纳入 crate，并在 crate 根再次导出 `MatchSchema`、`MatchTable`、`NewPiTRIdTracker` 与 `PiTRIdTracker`。它对应 Go 文件 `br/pkg/utils/filter.go`，职责是为 PiTR（Point-in-Time Recovery）保存待恢复库、表、分区的身份集合，并在普通 table-filter 外增加 BR 临时系统库名称处理和系统库开关。

RustCodeGraph 的文件节点显示本文件共 132 行、14 个符号，静态文件使用边指向 `br/pkg/utils/lib.rs` 和 `br/pkg/utils/parity_test.rs`。仓库直接搜索进一步确认：当前 Rust 生产文件没有直接引用此处的 `PiTRIdTracker`/`MatchSchema`/`MatchTable`；`br/pkg/task/restore.rs`、`br/pkg/registry/stubs.rs` 等仍保有各自的局部实现。因此本文件目前是已公开的 utils 规范实现和测试契约，不应把相似局部类型误认为它的直接调用者。

## 核心职责

- `PiTRIdTracker` 同时维护四类集合：库 ID、表 ID 到所属库 ID 集合的映射、分区物理 ID，以及库名到表名集合的映射（`PiTRIdTracker`）。ID 路径用于已解析元数据，名称路径为尚未分配或不便依赖 ID 的阶段保留选择信息。
- `TrackTableId` 建立精确的 `(db_id, table_id)` 关系，并把 `db_id` 同步写入 `DBIds`；这保证仅跟踪表时，后续 `ContainsDB` 也能识别其所属库。
- `ContainsDBAndTableId` 按库表二元组查询，避免只凭 table ID 将跨库对象误判为已选中；`ContainsTableId` 则有意提供忽略库归属的较宽查询。
- `MatchSchema` 与 `MatchTable` 先调用 `StripTempDBPrefixIfNeeded` 还原 `__TiDB_BR_Temporary_` 库名，再在 `with_sys == false` 时拒绝 `mysql`、`sys`、`workload_schema`，最后委托 `astersql_util_table_filter::Filter` 执行用户规则。

## 主要符号

- `pub struct PiTRIdTracker`：可克隆、可调试且有空集合默认值。四个字段均为公开字段，以保持 Go 结构体可直接观察的契约：`DBIds: HashSet<i64>`、`TableIdToDBIds: HashMap<i64, HashSet<i64>>`、`PartitionIds: HashSet<i64>`、`DBNameToTableNames: HashMap<String, HashSet<String>>`。
- `pub fn NewPiTRIdTracker() -> PiTRIdTracker`：返回 `Default::default()` 构造的空跟踪器。与 Go 返回指针不同，Rust 返回拥有所有权的值，调用方在修改时持有 `&mut self`。
- `TrackTableId(&mut self, db_id, table_id)`：记录日志，将库加入 `DBIds`，并以 `entry(table_id).or_default()` 创建/复用所属库集合后插入库 ID。
- `TrackPartitionId(&mut self, partition_id)` 与 `AddDB(&mut self, db_id)`：分别向分区集合和库集合做幂等插入；`AddDB` 额外记录日志。
- `ContainsDBAndTableId`、`ContainsTableId`、`ContainsPartitionId`、`ContainsDB`：均为只读成员查询，不改变状态；前者检查内层库集合，其余检查对应集合/键是否存在。
- `TrackTableName(&mut self, db_name, table_name)`：记录库表名称日志，并在库名对应的表名集合中幂等插入。
- `GetDBNameToTableName(&self) -> &HashMap<...>`：借用内部名称映射，不复制也不允许调用方修改。
- `MatchSchema(filter, schema, with_sys)` 与 `MatchTable(filter, schema, table, with_sys)`：接受 `&dyn Filter`，执行临时前缀标准化、系统库门禁和底层过滤器委托。

本文件没有模块级可变状态、条件编译项、自定义 trait 或错误类型。

## 执行流程

构建和填充 ID 跟踪器的典型流程如下：

1. `NewPiTRIdTracker` 创建四个空集合。
2. 上层发现独立库时调用 `AddDB`；发现表时调用 `TrackTableId`，后者自动补记库 ID；发现分区时调用 `TrackPartitionId`。
3. 如恢复选择还需按原始名称参与注册冲突检查或序列化，上层调用 `TrackTableName`，随后可用 `GetDBNameToTableName` 读取整个名称映射。
4. 消费方按语义选择精确的 `ContainsDBAndTableId`、仅 table ID 的 `ContainsTableId`、分区 ID 的 `ContainsPartitionId` 或库级 `ContainsDB`。

过滤流程对 schema 和 table 保持同一前置规则：先剥离 BR 临时库前缀；若标准化后的名称是系统库且 `with_sys` 为假，立即返回 `false`，不调用底层过滤器；否则 `MatchSchema` 委托 `Filter::MatchSchema`，`MatchTable` 委托 `Filter::MatchTable`。本层不会主动转换大小写；`IsSysDB` 的契约要求小写名称，普通规则的大小写策略由传入的 `Filter`（例如 table-filter 的大小写包装器）负责。

## 数据与状态

所有状态都封装在单个 `PiTRIdTracker` 值中，没有静态变量或持久化副作用。`HashSet` 使重复的库、表归属、分区和名称插入天然幂等；`HashMap<i64, HashSet<i64>>` 允许同一 table ID 对应多个 DB ID，以覆盖跨库重命名或极端测试场景。该结构因此保留“某个 table ID 出现过”和“某个库是否拥有该 table ID”两种不同查询视图。

名称键和值保持调用方传入的原始字符串，不进行大小写折叠、临时前缀剥离或合法性验证。ID 也不限制正数；集合只表达成员关系。`GetDBNameToTableName` 返回共享借用，其生命周期受 tracker 约束，且不能绕过方法直接修改（不过四个公开字段本身仍允许拥有 tracker 的调用方修改）。

日志是唯一的外部可观察副作用：`TrackTableId`、`AddDB`、`TrackTableName` 通过 `astersql_br_pkg_logutil::{log, Field}` 记录所跟踪的标识。`TrackPartitionId` 和所有查询方法不记录日志。

## 依赖与调用关系

- 上游装配：`br/pkg/utils/lib.rs` 声明 `filter` 模块并在 crate 根公开再导出四个主入口；`br/pkg/utils/Cargo.toml` 将该目录定义为 `astersql-br-pkg-utils` library crate。
- 标准库下游：`HashSet` 提供幂等成员集合，`HashMap` 提供 table-to-DB 与 DB-name-to-table-name 索引。
- 日志下游：`astersql-br-pkg-logutil` 的 `log::L().Info` 与 `Field::{int,string}`。
- 过滤下游：`astersql-util-table-filter::Filter`；该 trait 要求 `Debug + Send + Sync`，并暴露 `MatchSchema`、`MatchTable` 和 `toLower`。
- schema 下游：同 crate 的 `schema::{StripTempDBPrefixIfNeeded, IsSysDB}`；前者剥离 `__TiDB_BR_Temporary_`，后者识别 `mysql`、`sys`、`workload_schema`。
- 已确认的直接 Rust 使用：`br/pkg/utils/filter_test.rs` 构造并验证 tracker；`br/pkg/utils/parity_test.rs` 通过 crate 根再导出做公开契约冒烟。RustCodeGraph 精确 `callers/callees` 命令在本次 30 秒查询窗口内未返回结果，因此生产侧“未直接接线”的结论同时用全仓 Rust 引用搜索复核。

## 错误处理与边界

本文件所有 API 均不返回 `Result`，集合插入和成员查询没有业务错误分支。空 tracker 上的所有 `Contains*` 返回 `false`；Rust `Default` 总会初始化集合，因此不需要 Go 版本为 nil map 设置的防御分支。

关键边界包括：同一个 table ID 可同时属于多个 DB；`ContainsDBAndTableId` 必须同时命中两级键，而 `ContainsTableId` 只检查外层键；跟踪表必然隐式跟踪库；重复插入不会改变集合基数。系统库门禁发生在用户过滤器之前，`with_sys=false` 时底层规则即使允许也不能重新放行。临时前缀仅剥离一次，且是区分大小写的前缀匹配。

需要注意，`MatchSchema`/`MatchTable` 没有先把 schema 转成小写，而 `IsSysDB` 明确按小写常量比较；若调用方传入 `MYSQL` 等非小写形式且过滤器本身未标准化，系统库保护可能不命中。这是当前源码事实，扩展时不能在无兼容性评估的情况下悄然改变。

## 并发与资源生命周期

`PiTRIdTracker` 没有内部锁、原子量、任务、通道、文件句柄或网络资源。写方法要求独占 `&mut self`，Rust 借用规则阻止同一值在安全代码中并发修改；共享查询只需 `&self`。如上层确需跨线程共同更新，必须显式放入 `Mutex`/`RwLock` 等同步容器，本文件没有承诺无锁并发写。

`MatchSchema`/`MatchTable` 只在调用期间借用 `&dyn Filter`。由于 `Filter: Send + Sync`，具体过滤器可由上层安全共享，但本文件既不取得所有权，也不缓存过滤器。临时 schema `String` 在函数返回时释放；`GetDBNameToTableName` 的返回引用不会超过 tracker 生命周期。

## 与 Go 版本的对应关系

Rust 文件逐项移植 `br/pkg/utils/filter.go`：四个状态字段、构造器、八个 tracker 方法以及两个过滤包装函数的核心语义一致。`br/pkg/utils/filter_test.rs` 对齐 `filter_test.go` 的空状态、`AddDB`、同库多表、跨库同 table ID 不误命中，以及 `TrackTableId` 隐式登记库等断言。

主要语言差异是：Go 构造器返回 `*PiTRIdTracker`，并在写方法中为 nil map 延迟初始化；Rust 构造器返回值，`Default` 一次性创建空集合，方法通过借用表达读写权限。Go 的 `GetDBNameToTableName` 返回 map 引用语义，Rust 明确返回不可变借用。Go `TrackTableName` 接受两个字符串值，Rust 同样取得 `String` 所有权并在日志字段中克隆后存入集合。

迁移接线尚未完全统一：`br/pkg/task/restore.rs`、`br/pkg/registry/stubs.rs` 和 real-TiKV 测试 harness 中存在名称相同但类型独立的 tracker；它们不能作为本文件方法的直接调用证据。后续若改为复用本实现，应先比较字段布局、字符串参数形式和当前局部行为，不能仅替换类型名。

## 扩展指南

- 新增身份维度时，应在 `PiTRIdTracker` 增加明确集合/索引，并同步构造默认值、写入方法、精确与宽松查询方法；不要复用现有集合塞入语义不同的 ID。
- 修改 table ID 语义时，重点保持 `TrackTableId` 同步写 `DBIds` 和一对多 DB 映射这两个不变量，并扩展 `br/pkg/utils/filter_test.rs`，至少覆盖重复插入、同 table ID 跨库和错误库不命中。
- 修改名称路径时，同步验证 `TrackTableName`/`GetDBNameToTableName` 与注册冲突检查所需的大小写、所有权和序列化契约；相关 Rust 测试必须保持在独立 `*_test.rs` 文件中。
- 修改系统库过滤时，同时核对 `br/pkg/utils/schema.rs`、`pkg/util/table-filter/table_filter.rs`、Go 的 `br/pkg/utils/filter.go`，并为普通库、三个系统库、BR 临时系统库、`with_sys` 两种取值和大小写输入增加独立测试。
- 若将其他 BR 模块的局部 tracker/过滤函数接线到本 crate，应逐个清除重复实现并验证真实调用边；不能把本文件当前公开再导出等同于已经进入恢复主链。
- 性能风险主要来自大量名称/ID 的哈希集合内存占用和高频日志；扩展批量写入时应评估日志量。兼容风险主要是公开字段布局、系统库识别顺序和 `ContainsTableId` 与精确二元组查询之间的语义差异。

## 验证依据

- RustCodeGraph：`status` 显示索引含 7,032 个 Rust 文件；`files --filter br/pkg/utils` 确认目标、Go 对照和测试均已索引；`node --file br/pkg/utils/filter.rs --offset 1 --limit 260` 返回完整 132 行与 14 个符号，并报告 `lib.rs`、`parity_test.rs` 文件使用边；`query` 核对了 `PiTRIdTracker`、`NewPiTRIdTracker`、`TrackTableId`、`TrackTableName`、`ContainsDBAndTableId`、`MatchSchema`、`MatchTable` 的定义候选。精确 `callers/callees` 查询超出 30 秒窗口且没有输出，未将其当作完成证据。
- 源码：`br/pkg/utils/filter.rs`（全部实现）、`br/pkg/utils/lib.rs`（模块声明和再导出）、`br/pkg/utils/schema.rs`（临时前缀和系统库定义）、`pkg/util/table-filter/table_filter.rs`（`Filter` trait 与匹配委托契约）。
- crate 边界：`br/pkg/utils/Cargo.toml`（package 名、library 入口、`astersql-br-pkg-logutil` 与 `astersql-util-table-filter` 依赖）。
- Go 对照：`br/pkg/utils/filter.go`；独立测试：`br/pkg/utils/filter_test.rs`、`br/pkg/utils/filter_test.go`；公开契约抽样：`br/pkg/utils/parity_test.rs`。
- 调用/迁移状态复核：全仓 Rust 引用搜索只找到本 crate 测试对真实实现的直接使用；`br/pkg/task/restore.rs`、`br/pkg/registry/stubs.rs`、`tests/realtikvtest/brietest/harness.rs` 中的同名局部实现作为“尚未统一接线”的证据，而非本文件调用者。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务给定命令验证目标文档存在且恰有 11 个固定二级标题，并人工检查未建议把 Rust 测试内嵌到生产源文件。
