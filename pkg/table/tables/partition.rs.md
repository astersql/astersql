# `pkg/table/tables/partition.rs`

## 文件定位

本文件位于 `astersql-table-tables` crate，并由 [`lib.rs`](lib.rs) 中的公开模块 `pub mod partition` 无条件导出。它提供一个仅依赖 `mutation_checker::Datum`、标准集合和 `crc32fast` 的轻量分区路由模型：根据一行 `Datum` 计算 HASH、KEY、RANGE、RANGE COLUMNS、LIST 或 LIST COLUMNS 分区下标，再把下标映射为物理 `Partition`。

它不等同于 Go [`partition.go`](partition.go) 中实现 `table.Table`/`table.PartitionedTable` 的完整 `partitionedTable`。当前 Rust 完整表达式元数据、排序规则编码和构造路径另见受 `expression-runtime` feature 控制的 [`partition_expr.rs`](partition_expr.rs) 与 [`canonical_partition_expr.rs`](canonical_partition_expr.rs)。RustCodeGraph 对 `writable_partition_ids` 和 `partition_record_key` 未找到生产调用边；精确引用搜索显示本文件模型目前主要由 [`tables_test.rs`](tables_test.rs)、[`test/partition/partition_test.rs`](test/partition/partition_test.rs) 及测试辅助 [`testutil.rs`](testutil.rs) 使用。因此它应被理解为已公开、可独立验证的简化路由/重组数据模型，而不能据此宣称完整 DML 主链已经切换到该实现。

crate 边界由 [`Cargo.toml`](Cargo.toml) 确认：crate 名为 `astersql-table-tables`，默认启用 `expression-runtime`，但本文件自身只直接使用无条件依赖 `crc32fast` 和 crate 内 `Datum`；其代码不受条件编译控制。

## 核心职责

1. `PartitionExpr::locate_partition` 统一分派六种分区策略，并使所有策略返回 `definitions` 中的下标，而不是物理表 ID。
2. `ForKeyPruning`、`ForRangePruning`、`ForRangeColumnsPruning`、`ForListPruning` 和 `ForListColumnPruning` 保存各策略所需的最小状态并执行定位。
3. `ListPartitionGroup`/`ListPartitionLocation` 对 LIST 裁剪候选集合执行并集和交集，使用有序集合保证结果稳定。
4. `PartitionedTable` 将逻辑下标解析为 `PartitionDefinition.id`，再从物理分区映射取出实体；它还保存 DDL 重组与双写过渡态集合。
5. `partition_record_key` 提供一个轻量、可预测的测试键格式。

本文件不负责解析 SQL 分区表达式、从 catalog 元数据构造分区定义、执行真实 KV 写入、维护索引、检查指定分区集合或按事务原子地完成重组双写。这些能力存在于 Go 完整实现或其他 Rust 模块中，不能由本文件当前 API 推导出来。

## 主要符号

- `BTREE_DEGREE: usize = 32`：与 Go `btreeDegree` 数值一致的预留常量；本文件当前没有用它构造 BTree。
- `PartitionType`：六种策略标签。当前 `PartitionExpr` 自身已携带具体变体，`PartitionType` 在本文件内没有参与分派。
- `PartitionDefinition { id, name }` 与 `Partition { physical_id, definition }`：分别描述逻辑定义和轻量物理实体。两者允许分别保存 ID，调用方构造时应维持一致性；代码没有自动校验二者相等。
- `PartitionError`：区分没有任何分区、值无匹配分区、列偏移越界和非法定义。`InvalidDefinition(String)` 当前没有产生点，是为构造/校验扩展预留的错误类型。
- `PartitionExpr`：公共分派枚举。`Hash` 内联列偏移和分区数，其余变体持有专用裁剪器。
- `ForKeyPruning::locate_key_partition`：按声明的列顺序把值写入 IEEE CRC32 字节流，再对 `partition_count` 取模。
- `ForRangePruning::locate`：在单列上界数组上用 `slice::partition_point` 找首个严格大于当前值的边界；`None` 代表 `MAXVALUE`。
- `ForRangeColumnsPruning::locate` 与私有 `compare_range_columns`：先按 `column_offsets` 提取行值，再逐列做字典序比较；边界缺位或 `None` 视为 `MAXVALUE`。
- `ForListPruning::locate`、`ForListColumnPruning::locate`：分别以单个 `Datum` 或重排后的 `Vec<Datum>` 查映射，未命中时尝试默认分区。
- `ListPartitionGroup::{intersect,union}` 与 `ListPartitionLocation::{is_empty,union,intersect}`：组合裁剪候选。此处的数据形状是 `group_index -> partition_indexes`，与 Go 的 `PartIdx -> GroupIdxs` 命名和方向并非逐字段同构。
- `PartitionedTable::{locate_partition,writable_partition_ids}`：前者完成“策略下标 → definition ID → 物理分区”的两级查找；后者返回主分区和当前全部双写分区的 ID，并排序去重。
- `partition_record_key`：生成 UTF-8 字节串 `t{partition_id}_r{handle}`。这不是 Go `tablecodec.EncodeRecordKey` 的二进制编码替代品。

## 执行流程

行定位从 `PartitionExpr::locate_partition(row)` 开始：

1. HASH 检查分区数非零并读取 `column_offset`。`NULL` 取 0，整数直接使用，字节串按 UTF-8 有损转换后尝试解析为 `i64`，解析失败回退 0；之后对有符号值取无符号绝对值并取模。
2. KEY 检查分区数非零，依 `column_offsets` 顺序取值。`hash_datum` 对 NULL 写入单字节零，对整数写入小端八字节，对 Bytes 写入原字节；最终 CRC32 对分区数取模。
3. RANGE 先取指定列。NULL 固定进入第一个现有分区；非 NULL 通过 `partition_point` 跳过所有“小于或等于当前值”的有限上界，返回首个更大上界或 `MAXVALUE` 所在位置。等于边界会进入下一个分区。
4. RANGE COLUMNS 按列偏移组装键，并遍历上界，返回首个满足 `values < bound` 的位置。比较从第一列开始，遇到不等立即结束；边界中缺失元素或 `None` 使该位置成为 MAXVALUE。
5. LIST/LIST COLUMNS 直接查映射；没有精确项时使用 `default_partition`，二者都不存在则报错。

`PartitionedTable::locate_partition` 随后用返回下标读取 `definitions[index]`，再以定义 ID 查 `partitions`。`writable_partition_ids` 先要求主定位成功，然后合并 `double_write_partitions` 的所有 key，排序并去重。注意它没有使用 `reorganize_partitions` 重新计算第二套表达式，所以只表达“主分区加已配置双写目标”的轻量语义。

LIST 位置组合独立于行定位：`union` 按 `group_index` 合并候选集合并排序；`intersect` 只保留双方同组且分区下标交集非空的组。单个 `ListPartitionGroup::intersect` 收到不同组时会清空自身并返回 `false`。

## 数据与状态

所有类型都持有普通值或集合，没有内部可变性：定位方法只借用输入；集合交并方法通过 `&mut self` 显式修改接收者。`Datum` 同时承担哈希输入、范围比较键和 LIST 映射键，因此其 `Eq`、`Ord` 与 `Hash` 实现直接决定匹配语义。

关键不变量如下：

- `PartitionExpr` 产生的下标必须落在 `PartitionedTable.definitions` 范围内。
- 每个参与定位的列偏移必须小于行长度，否则返回 `InvalidColumnOffset(offset)`。
- 每个 `definitions` 中可定位的 ID 都应存在于 `partitions`；缺失会返回 `NoPartition`。
- 上界应按分区定义顺序单调排列；RANGE 使用二分前提但不会验证排序，RANGE COLUMNS 也只按给定顺序线性查找。
- LIST 映射中的下标和默认下标也必须对应有效 definition；裁剪器自身不检查，直到 `PartitionedTable::locate_partition` 才暴露越界。
- `double_write_partitions` 的 map key 被当作可写物理 ID；`writable_partition_ids` 不读取 map value 的 `physical_id`，构造者必须保证二者一致。
- `ListPartitionLocation` 通过 `BTreeSet` 自动去重并保持分区下标有序；组向量在 `union` 后按 `group_index` 排序。

`PartitionedTable` 派生 `Clone` 而非共享句柄。克隆会复制 definitions、表达式和所有 map；[`testutil.rs`](testutil.rs) 的 `swap_reorg_part_fields` 则逐字段交换两个具体 `PartitionedTable` 的全部状态。

## 依赖与调用关系

下游依赖很窄：

- `crate::mutation_checker::Datum` 提供 NULL、`i64`、`u64`、字节串四种值及比较/哈希能力。
- `crc32fast::Hasher` 实现 KEY 路由的 CRC32。
- `HashMap` 保存 O(1) 的物理分区和单列 LIST 映射，`BTreeMap<Vec<Datum>, usize>` 保存可按序比较的多列 LIST 键，`BTreeSet` 保存稳定有序的候选下标。

RustCodeGraph 的文件查询确认 `partition.rs` 入图且含 51 个符号；对 `PartitionedTable`、`locate_partition`、`writable_partition_ids`、`partition_record_key` 的查询用于消除仓库内同名 Go/Rust 类型。图查询没有给出 `writable_partition_ids` 的调用者。补充的精确 Rust 引用检查找到以下直接关系：

- [`lib.rs`](lib.rs) 公开模块；
- [`testutil.rs`](testutil.rs) 对 `PartitionedTable` 做动态下转并交换重组字段；
- [`tables_test.rs`](tables_test.rs) 验证全部轻量裁剪器、集合运算、双写 ID 与键格式；
- [`test/partition/partition_test.rs`](test/partition/partition_test.rs) 以 Go 测试场景验证 HASH/RANGE/LIST 行路由。

相比之下，[`tables.rs`](tables.rs) 的完整表实现使用的是 [`partition_expr.rs`](partition_expr.rs) 中另一组 `PartitionExpr`/裁剪器类型。新增生产调用前必须先决定复用哪套模型，避免继续形成名称相同但语义不同的平行实现。

## 错误处理与边界

- 分区数为零时，HASH 和 KEY 明确返回 `NoPartition`，避免模零。
- 行长度不足时，各策略返回具体的 `InvalidColumnOffset`；不会 panic。
- RANGE 没有上界、没有 MAXVALUE 且值超过全部边界时，以及 RANGE COLUMNS 无上界严格大于行键时，返回 `NoPartitionForValue`。
- LIST 未命中且无 DEFAULT 时返回 `NoPartitionForValue`。
- 下标存在但 definition 缺失返回 `NoPartitionForValue`；definition 存在但物理 map 缺项返回 `NoPartition`。
- HASH 对不能解析成整数的 Bytes 静默按 0 处理；这是可观察的兼容边界，若要改成错误必须同步调整 API 和回归测试。
- KEY 的 Bytes 使用原字节，整数使用小端二进制；它没有 Go `datumToHashKey` 的字符串转换与 collation key 处理，因而不能假设跨类型、跨排序规则的结果与 Go 完全一致。
- RANGE 对 `Datum::Uint` 保留完整 `u64` 值进行比较，避免先窄化到 `i64` 后把 `u64::MAX` 错路由到最低分区。独立回归 [`partition_test.rs`](partition_test.rs) 明确覆盖该问题。
- `compare_range_columns` 依赖 `Datum::cmp`，不执行 SQL 类型转换或 collation 归一化；这也是与完整表达式运行时的边界。
- `partition_record_key` 仅拼接可读字符串，负 handle 会产生如 `t20_r-2`；Go 对照函数调用 `tablecodec` 生成真实 KV 二进制键。

`InvalidDefinition` 当前未使用，说明本文件尚无元数据构造校验阶段。调用者不能把该变体的存在当成定义已被验证的证据。

## 并发与资源生命周期

本文件不创建线程、任务、锁、通道、事务、文件或网络资源，也没有 `unsafe`。共享并发策略由调用方决定：定位方法只读，因此在外部安全共享不可变实例时可并发调用；`ListPartitionLocation::{union,intersect}` 和重组 map 的修改需要调用方提供独占可变访问或外部同步。

CRC32 `Hasher` 只在一次 `locate_key_partition` 调用栈内创建并结束，没有跨请求状态。`writable_partition_ids` 每次分配新 `Vec<i64>`，排序去重后交给调用方；返回值不借用表内 map。

Go 完整实现的 `partitionedTable` 具有 `sync.Pool` 复用表达式求值行缓冲，并在同一事务中处理实际 add/remove/update 与重组双写。本文件没有这些生命周期或原子性保证；特别是返回一组 writable IDs 不会自行执行写入，也不能保证多个目标间事务一致性。

## 与 Go 版本的对应关系

[`partition.go`](partition.go) 是主要对照文件，但 Rust 本文件只移植了其中一小部分可独立表达的语义：

- `BTREE_DEGREE` 对应 `btreeDegree = 32`。
- KEY 都使用 IEEE CRC32，NULL 都写一个零字节；但 Go `datumToHashKey` 会按排序规则生成字符串 key，Rust 本文件按 Datum 变体直接写原始/小端字节，所以只是结构对应，不是所有输入的位级等价。
- RANGE 都按“首个严格大于值的上界”定位，NULL 进入首分区，MAXVALUE 接住剩余值。Rust 回归测试保留了 Go `ForRangePruning.Compare` 对无符号全范围比较的意图。
- RANGE COLUMNS 与 LIST/LIST COLUMNS 在 Rust 中是预构造 `Datum`/map 的轻量形式；Go 会构建和求值 SQL expression、执行类型转换、collation 编码并处理更多元数据错误。
- Rust `ListPartitionGroup { group_index, partition_indexes }` 的集合方向不同于 Go `ListPartitionGroup { PartIdx, GroupIdxs }`，不可按字段名直接互换。完整 Rust 对应形状实际也存在于 [`partition_expr.rs`](partition_expr.rs)。
- Rust `PartitionedTable` 保存 definitions、物理 map、重组 map 和双写 map，呼应 Go 结构的过渡态字段；但 Go 还包含 `TableCommon`、表达式缓冲池、重组表达式、索引状态和完整 Table trait 行为。
- Go `partitionedTableAddRecord` 先按行定位主分区，再只在当前分区属于重组集合时用 `reorgPartitionExpr` 重新定位并双写。Rust `writable_partition_ids` 则无条件附加 map 中全部双写 ID，语义明显更粗，不应视作完整等价移植。
- Go `PartitionRecordKey` 通过 `tablecodec` 编码真实记录键；Rust函数只是测试格式帮助器。

Go 测试 [`test/partition/partition_test.go`](test/partition/partition_test.go) 的对应场景被 Rust [`test/partition/partition_test.rs`](test/partition/partition_test.rs) 摘取为行路由回归；这些测试证明本文件列出的轻量行为，不证明 Go 文件其余两千余行功能均已移植。

## 扩展指南

若要安全扩展此文件：

1. 新增分区策略时，同时修改 `PartitionType`、`PartitionExpr` 和 `PartitionExpr::locate_partition`，并在独立测试文件中覆盖零分区、NULL、负数/无符号极值、边界等值、越界列和无匹配分区。
2. 修改 KEY 编码时优先对齐 Go `ForKeyPruning::datumToHashKey` 的类型转换与 collation 语义，并增加跨 Datum 类型、字符集/排序规则和 NULL 的固定 CRC32 向量；不能只验证结果小于分区数。
3. 修改 RANGE/RANGE COLUMNS 时维持上界有序、`LESS THAN` 严格性、MAXVALUE 和 `u64` 全范围语义。若引入构造器，应在构造阶段验证上界顺序并开始实际使用 `InvalidDefinition`。
4. 修改 LIST 位置结构前先确认要延续本文件的 `group -> partitions` 模型，还是统一到 [`partition_expr.rs`](partition_expr.rs) 的 `partition -> groups` 模型；两套方向不能机械互换。
5. 扩展重组双写时，应增加独立的重组表达式和按行选择目标，而不是把 `writable_partition_ids` 返回的全部 ID 直接接入生产写路径；还需验证事务原子性、更新/删除、隐藏分区和 DDL 状态切换。
6. 若把本文件接入真实 KV，必须替换或隔离 `partition_record_key`，复用 tablecodec 的规范编码，避免产生与 Go/TiKV 不兼容的 key。
7. Rust 测试逻辑必须保留在独立文件。就近同步 [`partition_test.rs`](partition_test.rs) 的无符号 RANGE 回归、[`tables_test.rs`](tables_test.rs) 的单元边界，以及 [`test/partition/partition_test.rs`](test/partition/partition_test.rs) 的 Go 场景对齐；不要把测试内嵌回生产文件。

兼容风险集中在哈希字节编码、SQL 类型/排序规则比较、definition 与物理 map 一致性和 DDL 双写选择；性能风险集中在 RANGE COLUMNS 线性扫描、每次多列键克隆、每次 writable ID 分配排序，以及大 LIST map 的内存占用。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标 [`partition.rs`](partition.rs) 已索引。
- RustCodeGraph `files --filter pkg/table/tables` 与 `node --file pkg/table/tables/partition.rs --offset 1 --limit 1200`：确认目标文件 387 行、51 个符号及全部实现。
- RustCodeGraph `query PartitionedTable`、`query locate_partition`、`query writable_partition_ids`、`query partition_record_key`、`query locatePartition`、`query locateKeyPartition`、`query ListPartitionLocation` 和 `query ForRangePruning`：区分仓库内同名 Rust/Go 实现并定位直接对照符号；`callers writable_partition_ids` 未返回调用边。
- RustCodeGraph 读取的 [`lib.rs`](lib.rs)、[`partition_expr.rs`](partition_expr.rs)、[`partition_test.rs`](partition_test.rs)、[`tables_test.rs`](tables_test.rs)、[`test/partition/partition_test.rs`](test/partition/partition_test.rs)、[`testutil.rs`](testutil.rs) 和 Go [`partition.go`](partition.go)：核对模块导出、平行完整实现、边界测试、重组字段和 Go 行路由/双写语义。
- [`Cargo.toml`](Cargo.toml)：核对 crate 名、默认 feature、依赖和 Go package 元数据。`pkg/table/tables` 下没有 `doc.go`，因此无额外包级契约文件可读。
- 精确引用搜索：确认生产 Rust 中只有 `testutil.rs` 直接使用本文件 `PartitionedTable`，完整表达式构造则引用 `partition_expr.rs` 类型；测试引用集中在上述独立测试文件。

人工复核结论：本文件存在的直接价值是提供小依赖、可确定测试的分区定位和重组状态模型；它的运行方式是“行值 → 策略下标 → definition ID → 物理分区”，安全扩展必须先处理与完整表达式/Go 主实现之间的语义差距。本文没有把公开导出或测试覆盖误写为已接入完整生产 DML 主链。
