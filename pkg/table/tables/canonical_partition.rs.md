# `pkg/table/tables/canonical_partition.rs`

## 文件定位

本文件属于 `astersql-table-tables` crate，是基于规范目录模型 `model_dependency::TableInfo` 的轻量分区路由器。模块由 `pkg/table/tables/lib.rs` 无条件公开为 `canonical_partition`；它不依赖 `expression-runtime` feature，而是让调用者以闭包提供分区表达式求值，因此可被表层和会话运行时共同使用。

主要入口是 `CanonicalPartitionedTable::locate`。`pkg/session/runtime/dml.rs` 的 `ConcreteSession::row_physical_id` 用它把逻辑行映射到物理表 ID；该结果继续用于 DML 分组、更新前后分区迁移、关系扫描、查询过滤、行编码和部分 DDL 数据筛选。`pkg/table/tables/tables.rs` 的 `TableCommon::canonical_partition_router` 则把表实例保存的 `use_new_collation` 绑定到路由器。

## 核心职责

- `CanonicalPartitionedTable::new` 借用一份目录表元数据，并固定本次路由使用的新旧排序规则模式。
- `CanonicalPartitionedTable::locate` 从 `PartitionInfo` 读取 HASH、KEY、RANGE 或 LIST 定义，选择首个匹配定义并返回其物理分区 ID。
- `list_value_matches` 实现 LIST/LIST COLUMNS 的 NULL、字面量去引号和按列排序规则比较。
- `range_columns_row_is_below` 实现 RANGE COLUMNS 的逐列字典序比较，包括 NULL、MAXVALUE、整数和字符串排序规则。

它只负责“依据现有规范元数据做路由”，不解析 SQL 表达式、不构造分区元数据、不维护分区对象，也不负责产生 TiDB 的分区错误。表达式语义由 `locate` 参数 `expression_value` 注入；会话侧对应实现是 `partition_expression_value`。

## 主要符号

- `CanonicalPartitionedTable<'a> { table: &'a model::TableInfo, use_new_collation: bool }`：只读借用表元数据，生命周期 `'a` 保证路由器不能长于元数据；布尔值控制字符串归一化使用哪套 collator。
- `new(table, use_new_collation) -> Self`：无校验构造函数。调用者必须传入与表执行上下文一致的排序规则模式。
- `locate(&self, row, expression_value) -> i64`：公开主入口。`row` 以规范化列名为键、`Option<String>` 为值；闭包按表达式文本和整行返回可选 `i64`。
- `list_value_matches(actual, configured, collation) -> bool`：公开辅助入口，也被 `pkg/session/runtime/dml.rs::unmatched_list_partition_warning` 复用以判断是否需要警告。
- `range_columns_row_is_below(row, columns, upper_bound) -> bool`：公开辅助入口，也被 `pkg/session/runtime/dml.rs::unmatched_range_partition_value` 复用。

文件没有模块级常量、trait、枚举、条件编译项或可变全局状态。

## 执行流程

`locate` 的流程如下：

1. 调用 `TableInfo::GetPartitionInfo`。非分区表或空定义直接返回逻辑表 `TableInfo.ID`。
2. 确定求值文本：有 `PartitionInfo.Expr` 时去掉反引号并转成小写；否则取第一项 `PartitionInfo.Columns[*].L`；两者都没有时得到空字符串。
3. 调用 `expression_value`，求值失败以 `0` 代替。该值供 HASH、表达式 RANGE 和表达式 LIST 使用。
4. 按 `PartitionInfo.Type` 路由：
   - HASH：使用 `value.rem_euclid(定义数)`，所以负值也得到非负下标。
   - KEY：按声明列顺序累积 IEEE CRC32。缺列与 NULL 都写入单字节 `0`；非空字符串先依据目录列的 collation 生成 key，再参与哈希；最终对定义数取模。
   - RANGE：表达式分区按定义顺序寻找首个 `value < LessThan[0]` 的定义，`MAXVALUE` 无条件作为上界；RANGE COLUMNS 委托 `range_columns_row_is_below`。
   - LIST：逐定义、逐值元组查找。表达式 LIST 只接受单元素配置；LIST COLUMNS 要求元组长度恰好等于分区列数，并逐列调用 `list_value_matches`。
   - 其他类型：视为不匹配。
5. 匹配时返回 `PartitionDefinition.ID`；没有匹配时返回逻辑表 ID。

`range_columns_row_is_below` 按 `(columns, upper_bound)` 的 `zip` 顺序比较。遇到 `MAXVALUE` 立即返回真；实际 NULL 小于非 MAXVALUE；双方都能解析为 `i128` 时做数值比较，否则按目录列 collation key 比较，找不到列元数据时退化为 Rust 字符串比较。首个非相等列决定结果，所有已比较列相等时返回假。

## 数据与状态

路由器自身只有两个不可变字段，不复制 `TableInfo`，也不缓存解析结果。输入行是 `HashMap<String, Option<String>>`：键必须与 `CIStr.L` 对齐；缺失键在 KEY、LIST COLUMNS 和 RANGE COLUMNS 路径上均会像 NULL 一样被观察到。

HASH/RANGE/LIST 表达式共享闭包返回的 `Option<i64>`，其中 `None` 被主流程折叠成 `0`。KEY 使用 `crc32fast::Hasher` 的局部状态；字符串比较使用 `collate_dependency::GetCollatorWithCollate(use_new_collation, collate).Key(...)` 生成临时排序键。没有跨调用状态或持久缓存。

重要不变量是定义顺序具有语义：RANGE 选择首个满足上界的定义，LIST 选择首个包含匹配值的定义。元数据若包含重叠定义，本文件不会检测或拒绝。

## 依赖与调用关系

直接依赖由 `pkg/table/tables/Cargo.toml` 声明：

- `model-dependency` 提供 `TableInfo`、`PartitionInfo`、`PartitionDefinition`、`CIStr` 和分区类型常量。
- `collate-dependency` 提供新旧排序规则选择及字符串 key。
- `crc32fast` 提供 KEY 分区的 IEEE CRC32 累积器。
- 标准库 `HashMap` 保存行值，`Ordering` 表达 RANGE COLUMNS 的三向比较。

直接上游包括 `TableCommon::canonical_partition_router` 和 `ConcreteSession::row_physical_id`。后者的调用点可见于 `pkg/session/runtime/dml.rs`、`relational_scan.rs`、`query.rs`、`row_codec.rs` 与 `ddl.rs`：它们分别用于写入分区分组、扫描选择、查询过滤、存储键编码和 DDL 数据筛选。会话侧的 `unmatched_range_partition_value`、`unmatched_list_partition_warning` 还直接调用两个公开比较辅助方法，以弥补 `locate` 不返回错误的接口设计。

RustCodeGraph 将 `locate` 到 `list_value_matches`、`range_columns_row_is_below` 的调用边识别出来，并将两个辅助方法到 `pkg/util/collate/collate.rs::Key` 的边识别出来。图索引能查询目标符号，但 `files --filter` 未命中该文件，且 callers 查询未输出边；上游关系因此以仓库内 `rg` 的精确引用为准。

## 错误处理与边界

本文件所有 API 都不返回 `Result`。它采用宽松回退：无分区信息、空定义、未知分区类型或没有匹配定义时均返回逻辑表 ID；表达式闭包返回 `None` 时按 `0` 路由；非法 RANGE 整数上界由 `parse::<i64>().unwrap_or(i64::MAX)` 当作最大上界处理。这些行为不会产生 Go 侧的 `ErrNoPartitionForGivenValue`。

LIST 配置字面量会先 `trim` 并去除首尾单/双引号；大小写不敏感的 `NULL` 只匹配 `None`。未提供 collation 时字符串必须逐字节相等；提供 collation 时比较 collator key。表达式 LIST 会把整数求值结果转回十进制字符串，因此不覆盖 Go `Datum` 的全部类型转换语义。

RANGE COLUMNS 使用 `zip`，不会显式检查列数与上界数一致；较短一侧会截断比较。所有比较项相等时返回假，符合严格“小于上界”，但畸形元数据不会在这里报错。目录中找不到列时会使用 `binary`（KEY）或普通字符串比较（RANGE COLUMNS），调用者不应把这种容错当作元数据校验。

## 并发与资源生命周期

`CanonicalPartitionedTable` 只持有共享不可变借用，没有锁、原子量、线程、异步任务、通道、事务或 I/O。每次调用只分配局部 CRC32 状态、表达式字符串或 collator key；调用结束即释放。

并发安全性取决于被借用的 `TableInfo` 和闭包满足调用现场的 Rust 类型约束，本类型本身没有内部可变性。`use_new_collation` 在构造时快照化，避免一次路由过程中全局模式变化；会话直接构造时读取 `astersql_tablecodec::collate::NewCollationEnabled()`，表对象路径则使用 `TableCommon` 已保存的模式。

## 与 Go 版本的对应关系

Go 的真实实现集中在 `pkg/table/tables/partition.go`，不是一个同名文件。语义对应关系包括：

- Rust KEY 分支对应 Go `ForKeyPruning.LocateKeyPartition` 与 `datumToHashKey`：两者都按分区列顺序累计 IEEE CRC32，NULL 写入单字节零，字符串经过所选 collator key，再对分区数取模。
- Rust RANGE/RANGE COLUMNS 分支对应 Go `partitionedTable.locateRangePartition`、`locateRangeColumnPartition` 及 `ForRangeColumnsPruning`；`MAXVALUE` 是无穷上界，列分区做字典序比较。
- Rust LIST 分支对应 Go `ForListPruning.LocatePartition`、`ForListColumnPruning.LocatePartition` 及 `locateListPartition` 的按值定位意图。
- Rust `locate` 的总分派对应 Go `partitionedTable.locatePartitionCommon` / `locatePartition`，最终都以 `PartitionDefinition.ID` 表示物理分区。

当前 Rust 文件是会话规范行模型的简化适配层，并非 Go 完整执行器的逐类型移植。Go 使用 `Datum`、表达式上下文、类型转换和编码 key，能传播求值/转换错误、处理默认或正在 dropping 的分区，并在无匹配时返回 `ErrNoPartitionForGivenValue`；本文件使用字符串行与 `i64` 闭包并回退逻辑表 ID。扩展时必须保持这些差异显式，不能仅凭接口名称假定完全等价。

## 扩展指南

新增分区类型或改变路由规则时，首要修改点是 `CanonicalPartitionedTable::locate` 的类型分派；新增列值比较语义分别落在 `list_value_matches` 或 `range_columns_row_is_below`。若需要支持非整数表达式，应先调整 `expression_value` 契约及会话侧 `partition_expression_value`，避免只在本文件做字符串猜测。

涉及字符串的修改必须同时验证新旧 collation、大小写/重音等价、缺失列与 NULL；涉及 RANGE COLUMNS 必须覆盖多列前缀相等、MAXVALUE、负数、大整数、字符串和畸形边界长度；涉及 KEY 必须用 Go `crc32.NewIEEE` 结果交叉校验。若把宽松回退改为错误传播，需要同步审查 `row_physical_id` 的所有调用者以及 DML 的 unmatched 警告逻辑，否则可能改变写入、扫描和编码的物理 ID。

测试应放在独立文件而非本源文件。直接路由测试应扩展 `pkg/table/tables/partition_expr_test.rs`（现有 `go_merge_49_table_common_routes_canonical_partition_metadata` 已覆盖 TableCommon→HASH 路径）；会话可见的错误或警告应扩展 `pkg/session/runtime` 下相应独立 `*_test.rs`。Go 对照回归位于 `pkg/table/tables/partition_test.go` 及相关分区测试目录。

## 验证依据

- 目标实现：`pkg/table/tables/canonical_partition.rs`，逐项核对结构体、构造函数、四种分区分支和两个比较辅助方法。
- crate 边界：`pkg/table/tables/Cargo.toml` 与 `pkg/table/tables/lib.rs`，核对公开模块、feature 和 `model`/`collate`/`crc32fast` 依赖。
- Rust 上游：`pkg/table/tables/tables.rs::TableCommon::canonical_partition_router`；`pkg/session/runtime/dml.rs::{row_physical_id, unmatched_range_partition_value, unmatched_list_partition_warning}`；并以精确引用确认 `ddl.rs`、`relational_scan.rs`、`query.rs`、`row_codec.rs` 的消费位置。
- Rust 测试：`pkg/table/tables/partition_expr_test.rs::go_merge_49_table_common_routes_canonical_partition_metadata` 验证表对象绑定与 HASH 定位；同文件 `key_partition_uses_ieee_crc32_and_null_marker` 验证底层 KEY 裁剪语义；`pkg/table/tables/partition_test.rs::unsigned_range_values_do_not_wrap_into_the_first_partition` 记录 Go RANGE 数值边界意图。未发现专门覆盖本文件 LIST/RANGE 辅助方法的独立 Rust 测试。
- Go 对照：`pkg/table/tables/partition.go` 中 `LocateKeyPartition`、`datumToHashKey`、`locatePartitionCommon`、`locatePartition`、`LocatePartition` 和 `GetPartitionByRow`；相关回归入口为 `pkg/table/tables/partition_test.go` 与 `pkg/table/tables/test/partition/`。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`query CanonicalPartitionedTable` 定位目标及 `canonical_partition_router`、会话 router 变量；`callees locate` 给出两个内部辅助调用，两个辅助方法的 `callees` 均指向 collator `Key`。由于 callers 查询无输出，调用者另由 `rg` 核对。
- 本任务为纯文档分析，按计划不运行 Cargo；交付前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节。
