# `pkg/executor/partition_runtime.rs`

## 文件定位

`partition_runtime.rs` 属于 `astersql-executor` crate（`pkg/executor/Cargo.toml` 的 `[lib] path = "lib.rs"`），并由 `pkg/executor/lib.rs` 通过 `pub mod partition_runtime` 公开导出。它不是 TiDB 线上执行器将 SQL 请求下发到 TiKV 的实现，而是一个只依赖 Rust 标准库集合的内存分区表语义模型：它把分区路由、点查/扫描、全局唯一索引、DML、行锁以及连接/聚合/Apply 等行为压缩到可确定验证的本地数据结构中。

当前仓库中的直接使用主要来自独立测试 `pkg/executor/partition_runtime_test.rs`、`pkg/executor/partition_table_test.rs` 和外部测试 crate `pkg/executor/test/issuetest/executor_issue_test.rs`。代码搜索未发现线上 builder、reader 或 session 调用这组 API，因此应把它视为测试支撑的语义模型，不应将其能力等同于完整的生产执行路径。

## 核心职责

- 用 `Value`、`Row` 建立足够表达测试整数、文本和 SQL `NULL` 的轻量行模型。
- 用 `Partitioning::{Range,List,Hash}` 校验分区定义，并由 `partition_for` 将行映射到物理分区下标。
- 用 `PartitionTable` 维护主键行、全局唯一索引、已计算的分区下标和事务行锁，并实现点查、批量点查、分区扫描、过滤、排序限制、IndexMerge 及 DML。
- 用 `AccessPath` 和 `QueryResult` 保留“这次查询模拟了哪种物理路径、触及哪些分区”的可断言信息。
- 提供与分区表回归场景相关的纯行级辅助函数：`join_rows`、`union_rows`、`grouped_count_sum`、`split_region_keys` 和 `correlated_apply`。

## 主要符号

- `Value::{Null, Int, UInt, Text}` 是列值集合；私有的 `Value::partition_number` 仅将有符号/无符号整数提升为 `i128`，避免 RANGE 边界和聚合加法立即窄化。
- `Row(Vec<Value>)` 保留列顺序；`Row::value(column)` 是所有需要边界检查的列访问入口。
- `RangePartition { name, less_than }` 中 `less_than: None` 表示 `MAXVALUE`。
- `Partitioning::range/list/hash` 是三种校验型构造器；`partition_for`、`partition_name` 和私有 `len` 分别负责路由、命名和分区数量。
- `AccessPath` 枚举 `TableDual`、`PointGet`、`BatchPointGet`、`PartitionScan`、`GlobalIndex`、`IndexMerge`；`QueryResult` 将路径、分区名和行一起返回。
- `PartitionError` 统一表达定义非法、列越界、类型不匹配、无匹配分区、键冲突、未知分区和行锁冲突，并实现 `Display`/`Error`。
- 私有 `StoredRow { row, partition }` 把原始行和写入时确定的物理分区绑定。
- `PartitionTable` 是主状态容器：`rows` 按主键有序存储，`unique_indexes` 是“唯一列 -> 唯一值 -> 主键”的二级映射，`locks` 是“主键 -> 事务 ID”。
- `JoinKind::{Inner,LeftOuter,Semi,AntiSemi}` 驱动 `join_rows` 的等值嵌套循环连接。
- 模块级公开函数 `union_rows`、`grouped_count_sum`、`split_region_keys`、`correlated_apply` 分别模拟 UNION、COUNT/SUM 分组、Region 均匀分裂键和相关 Apply。本文件没有宏、trait 或条件编译项。

## 执行流程

1. 调用者先通过 `Partitioning::range/list/hash` 建立分区策略。RANGE 要求非空、有限上界严格递增且 `MAXVALUE` 仅在末尾；LIST 要求每组非空且所有常量不重复；HASH 要求分区数为正。
2. `PartitionTable::new` 记录主键列，并为每个指定的唯一列预建空索引。`insert` 先读主键、计算分区，再检查所有非 `NULL` 唯一值；只有全部校验成功才写二级索引和主表，避免半写入。
3. `point_get` 直接查主键；未命中返回 `TableDual` 空结果。`batch_point_get` 按输入键顺序收集命中行，用 `BTreeSet` 去重并排序分区名。`global_index_get` 先由唯一值找主键，再复用 `point_get`，最后把路径标记改为 `GlobalIndex`。
4. `scan_partitions` 在分区信息开启时先校验所有请求名称，空名称集表示全分区，再按 `StoredRow.partition` 过滤。关闭信息后，它故意忽略名称校验、返回全表行且不报告分区名。`ordered_limit` 在扫描后验证排序列，然后排序、`skip(offset)`、`take(limit)`。
5. `filter` 对全部存储行执行调用者闭包；`index_merge` 合并主键直接命中和唯一索引命中的主键，先去重，再复用批量点查。
6. `update` 先保存旧锁持有者，通过 `delete` 移除旧行/唯一索引/锁，再调用 `insert` 重新路由新行。新行插入失败时重插旧行并恢复旧锁，使该操作对这个内存模型呈现回滚语义。
7. `lock` 对存在行进行主键级加锁：同一事务重入成功，不同事务立即返回冲突；`unlock_transaction` 线性删除该事务的所有锁。
8. 独立辅助流程不依赖 `PartitionTable`：`join_rows` 先检查键列再连接；`union_rows` 选择保留或去除重复；`grouped_count_sum` 每行累加计数与 `i128` 和；`split_region_keys` 用整数比例计算内部分割点；`correlated_apply` 每个外层行重新调用内层闭包并拼接行。

## 数据与状态

`PartitionTable` 的核心不变式是：每个 `rows` 条目的键等于其行主键列，`StoredRow.partition` 是该行最近一次成功写入时 `partition_for` 的结果，每个非 `NULL` 唯一值在对应 `unique_indexes` 中恰好指向其主键。`insert`、`delete`、`update` 一起维护这些不变式；`drop_unique_index` 必须同时从 `unique_columns` 和 `unique_indexes` 移除目标才返回 `true`。

RANGE 路由对 `NULL` 固定返回第一分区，其他值选择第一个满足 `value < bound` 的上界或 `MAXVALUE`；LIST 对包含 `NULL` 在内的 `Value` 做精确集合匹配；HASH 把 `NULL` 当 0，其他数值按无符号绝对值对分区数取模。`BTreeMap`/`BTreeSet` 使主键扫描、分区名、IndexMerge 和 DISTINCT 结果具有可重现的排序；这是测试模型的确定性，不代表真实分布式执行的返回顺序承诺。

## 依赖与调用关系

下游仅有 `std::collections::{BTreeMap, BTreeSet, HashMap}` 和 `std::fmt`；该文件没有使用 `pkg/executor/Cargo.toml` 列出的 TiDB/AsterSQL 子 crate，也没有 TiKV 客户端、异步 runtime 或 SQL 表达式依赖。

上游装配是 `pkg/executor/lib.rs` 的 `pub mod partition_runtime`。仓库精确搜索显示：

- `pkg/executor/partition_runtime_test.rs` 直接验证分区定义/路由、索引与 DML、锁冲突和失败更新回滚。
- `pkg/executor/partition_table_test.rs` 系统性验证 `PartitionTable` 和所有模块级辅助函数，与 Go 分区表回归项目对齐。
- `pkg/executor/test/issuetest/executor_issue_test.rs` 从外部 crate 通过 `astersql_executor::partition_runtime` 使用 `grouped_count_sum`、`split_region_keys` 和 `union_rows`，证明公开模块边界可被下游测试 crate 调用。

RustCodeGraph 对 `PartitionTable`、`join_rows`、`split_region_keys`、`correlated_apply` 定位到了本文件的精确符号，但 callers/callees 未返回有效边；因此调用者结论使用 `rg` 对模块路径和符号名做了补充核验，不把图中缺边解读为“没有调用者”。

## 错误处理与边界

- 构造器把非法分区元数据转为 `InvalidDefinition`；RANGE 边界不递增、中间出现 `MAXVALUE`，LIST 空集/重复，HASH 零分区都不会产生可用对象。
- 所有需要验证列号的入口通过 `Row::value` 返回 `ColumnOutOfBounds`。`ordered_limit` 先遍历所有结果验证列，再使用直接下标排序。
- RANGE/HASH 分区键和聚合值只接受数字，文本返回 `TypeMismatch`；LIST 则允许任意 `Value` 精确匹配。无 RANGE/LIST 落点返回 `NoPartition`。
- 重复主键（包括 `Value::Null`）或重复的非 `NULL` 唯一值返回 `DuplicateKey`；唯一索引对 `NULL` 不建条目，所以允许多个 `NULL`。主键本身未特别禁止 `Value::Null`，这是轻量模型与完整 SQL schema 约束的一个差异。
- `scan_partitions` 在开关开启时对任一错误名称返回 `UnknownPartition`，不返回部分结果。
- `update` 的恢复路径使用 `expect`，其前提是内部不变式未被破坏；`insert` 中获取已初始化唯一索引也使用 `expect`。这些 panic 表示编程错误，不是用户输入错误。
- `join_rows` 的 SQL `NULL` 键永不匹配；右侧为空时，LeftOuter 无法从 schema 获取宽度，因而默认补一个 `NULL`。它不处理列类型强制转换、排序规则或三值逻辑的其他表达式。
- `grouped_count_sum` 对 `NULL`/`Text` 聚合值报错，没有实现 SQL `SUM` 忽略 `NULL` 的完整规则。`split_region_keys` 只拒绝零分片或非正区间；区间小于分片数时整数除法可能生成重复分割键，函数不额外去重。

## 并发与资源生命周期

本文件没有线程、异步任务、通道、文件/网络句柄或真实事务。可变操作要求 `&mut PartitionTable`，Rust 借用规则使同一实例的写入串行化；若上层放入锁中共享，外部同步协议仍由上层负责。

`locks` 是对数据库行锁的行为模拟，而不是内存并发原语：加锁不等待，冲突立即返回 `LockConflict`；事务完成时调用者必须显式执行 `unlock_transaction`。`delete` 会删除该行的锁，失败 `update` 会恢复原锁，成功 `update` 则不把原锁迁移到新主键。`correlated_apply` 虽有“parallel apply”对应测试，实现本身是严格顺序循环，不应据此推断已有并行调度。

## 与 Go 版本的对应关系

Go 树中没有 `pkg/executor/partition_runtime.go` 的一对一生产实现。Rust 文件是为了在不搭建完整 SQL/session/TiKV 栈的情况下，保留 `pkg/executor/partition_table_test.go` 中多类回归的核心可观测语义。

对应关系包括：Go `TestPointGetwithRangeAndListPartitionTable` 对应 Rust 点查、LIST/RANGE 路由和 `TableDual`；Go `TestBatchGetforRangeandListPartitionTable` 对应 `batch_point_get`；Go `TestPartitionTableWithDifferentJoin` 的结果意图由 `join_rows` 的四种 `JoinKind` 缩减表达；Go `TestSelectLockOnPartitionTable` 及 issue 26251/31024 类场景由主键级 `lock`/`unlock_transaction` 表达冲突与释放；全局索引、IndexMerge、UnionScan、Apply、聚合和 Region split 场景则分别由相应的轻量函数承载。`pkg/executor/partition_table_test.rs` 中的同名/近同名测试记录了这些对应。

两者不是等价实现。Go 测试经过 DDL、planner、executor、事务、failpoint 和 mock store 的真实 SQL 路径，会检查物理计划、动态剪枝、不同 reader、乐观/悲观事务和阻塞；Rust 模型仅保留行结果、简化路径标签和立即锁冲突，不执行 SQL、不建计划、不访问 TiKV、不等待锁，也不模拟 MVCC。因此它是 Go 测试意图的局部语义复刻，不是 Go 生产执行器的替代品。

## 扩展指南

- 增加分区方式或边界规则时，同步修改 `Partitioning` 枚举、对应构造器、`partition_for`、`partition_name`、`len`，并在 `pkg/executor/partition_runtime_test.rs` 中添加定义失败和边界落点用例。若源自 Go 行为，还应在 `pkg/executor/partition_table_test.rs` 中注明对应 Go 回归。
- 增加值类型时，首先明确其总顺序、Hash/Eq 语义、能否做分区数字和聚合值；`Value` 的派生 `Ord` 会直接影响主键、UNION DISTINCT、分组和输出顺序。
- 扩展 DML 或索引时，保持“先全量检查，后统一写入”以及更新失败恢复旧行/旧索引/旧锁的不变式；重点回归 `failed_update_restores_the_existing_row_lock`、重复唯一值和删索引后的行为。
- 扩展查询路径时，决定新行为是 `PartitionTable` 方法还是独立纯函数，并同步更新 `AccessPath`/`QueryResult`。要警惕 `BTree*` 隐含的确定排序被测试误当成生产 SQL 顺序。
- 扩展锁语义时，先区分“测试用行锁状态机”与“真实事务等待/MVCC”。如果需要后者，不应继续把复杂分布式行为塞入本文件，而应接入生产事务子系统并使用相应集成测试。
- 测试必须保持在独立文件中：精确不变式放入 `partition_runtime_test.rs`，Go 分区表场景对齐放入 `partition_table_test.rs`，跨 crate issue 回归放入相关 `pkg/executor/test/*/*_test.rs`；不要在 `partition_runtime.rs` 内嵌测试模块。

## 验证依据

- 源文件：`pkg/executor/partition_runtime.rs`，已核对全部 750 行，包括所有类型、impl、函数与错误分支。
- crate/模块：`pkg/executor/Cargo.toml` 的 package、lib、feature 和 porting metadata；`pkg/executor/lib.rs:164` 的公开模块声明以及 `lib.rs:283-292` 的独立测试模块接线。
- Rust 测试：`pkg/executor/partition_runtime_test.rs`、`pkg/executor/partition_table_test.rs`、`pkg/executor/test/issuetest/executor_issue_test.rs`。
- Go 对照：`pkg/executor/partition_table_test.go`，重点核对 `TestPointGetwithRangeAndListPartitionTable`、`TestBatchGetforRangeandListPartitionTable`、`TestPartitionTableWithDifferentJoin`、`TestSelectLockOnPartitionTable` 及相关全局索引/锁回归。仓库中不存在与本 Rust 文件一对一的 Go 生产文件。
- RustCodeGraph：`status` 显示索引包含 7,032 个 Rust 文件；`node --file pkg/executor/partition_runtime.rs` 读取了全文；`query` 定位 `partition_runtime.rs::PartitionTable`、`partition_runtime.rs::join_rows`、`partition_runtime.rs::split_region_keys`、`partition_runtime.rs::correlated_apply`。`callers`/`callees` 对这些符号未返回边，已用精确文本搜索补足上游证据。
- 人工边界复核：确认本文件不是线上执行器接线，不含并行 Apply、锁等待、MVCC、SQL 完整 `NULL` 聚合规则或 TiKV Region 操作；文档未把这些预期设计写成当前已支持能力。
