# `pkg/ddl/affinity.rs`

## 文件定位

`create_table_affinity_groups`、`delete_table_affinity_groups` 和 `batch_delete_table_affinity_groups` 负责空操作短路、批量整理和后端错误映射；真正的 PD I/O 留给 `AffinityGroupManager` 实现。

## 核心职责

1. `get_table_affinity_group_id` 与 `get_partition_affinity_group_id` 接收可选 codec，生成与 Go/PD 约定一致、基于物理 ID 和可选 keyspace ID 的组名。
2. `build_affinity_group_key_range` 将物理 ID 映射为左闭右开区间 `[t{id}, t{id+1})`，必要时再经 `AffinityCodec` 加入 keyspace/Region 编码。
3. `build_affinity_group_definitions` 根据 `AffinityLevel::Table` 或 `AffinityLevel::Partition` 生成完整的“组 ID → key 范围列表”映射，并拒绝缺少分区定义的分区级配置。
4. `create_table_affinity_groups`、`delete_table_affinity_groups` 和 `batch_delete_table_affinity_groups` 负责空操作短路、批量整理和后端错误映射；真正的 PD I/O 留给 `AffinityGroupManager` 实现。

这些职责的边界是“计算应创建/删除什么并发起抽象调用”，而不是决定外层 DDL 是否回滚或忽略清理失败。尽管函数注释记录了 Go 侧创建为关键路径、删除为尽力清理，Rust API 仍把两者错误都返回给调用方，由未来生产接线决定策略。

## 主要符号

- `AffinityLevel::{Table, Partition}`：亲和粒度。它是闭合枚举，因此当前不会从外部值产生未知分支；`AffinityError::InvalidLevel` 目前没有构造点，是为未来解析/接线保留的错误变体。
- `AffinityTable { id, name, affinity, partition_ids }`：从 Go `model.TableInfo` 抽出的最小输入。`name` 当前没有参与计算或错误格式化；`affinity: None` 表示无事可做。
- `AffinityGroupKeyRange { start_key, end_key }`：PD key 范围的本地表示，语义为左闭右开。
- `AffinityError::{MissingPartitions, InvalidLevel, Backend}`：分别表示分区元数据不完整、预留的非法级别和后端字符串错误。`Display` 当前直接输出 `Debug` 形式。
- `AffinityCodec::encode_region_range`：可选的存储编码边界；`keyspace_id` 默认返回 `None`，NextGen 实现可返回 `Some(u32)` 来同时隔离组名。调用方不提供 codec 时保留逻辑 table key；提供时同时重写起止 key。
- `AffinityGroupManager::{create_groups_if_not_exists, delete_groups_with_retry}`：PD 管理边界。创建契约要求幂等，删除契约名称要求实现重试，但本文件自身不实现网络、重试或日志。
- `encode_table_prefix`：私有函数，产生 `b't' + ((id as u64) xor 2^63)` 的大端字节序，与 Go `tablecodec.EncodeTablePrefix` 使用的可比较有符号整数编码对齐。
- `build_affinity_group_definitions`：核心分派函数；表级产生一个范围，分区级按显式或表内分区 ID 逐一产生范围。
- 三个后端入口：`create_table_affinity_groups`、`delete_table_affinity_groups`、`batch_delete_table_affinity_groups`。

## 执行流程

构造流程从 `build_affinity_group_definitions(codec, table, partition_ids)` 开始：

1. `table` 为 `None`，或表的 `affinity` 为 `None` 时，返回空 `BTreeMap`。
2. 表级亲和调用 `get_table_affinity_group_id(table.id)`，并以表 ID 调用 `build_affinity_group_key_range`，得到唯一组和唯一范围。
3. 分区级亲和优先使用调用方显式给出的 `partition_ids`；仅当参数为 `None` 时才回退到 `table.partition_ids`。注意 `Some(&[])` 是明确的空覆盖，会触发 `MissingPartitions`，不会回退。
4. 对每个分区 ID，组名同时包含表 ID 与分区 ID，而 key 范围只按分区的物理 ID 构造。重复分区 ID 会被 `BTreeMap::insert` 合并为同一项。
5. `build_affinity_group_key_range` 先构造起始前缀，再以 `physical_id.wrapping_add(1)` 构造结束前缀；如果有 codec，则对完整 `[start, end)` 一次编码。

创建入口先构造定义，空集合直接成功，否则只调用一次 `create_groups_if_not_exists`。单表删除入口同样先构造定义，再通过 `collect_affinity_group_ids` 传给一次删除调用。批量删除入口逐表构造定义，把所有组名放入 `BTreeSet` 去重排序，最终最多发起一次删除调用。`pkg/ddl/affinity_test.rs` 分别覆盖表级、分区级、显式分区覆盖、创建幂等、部分删除、TRUNCATE 后新组创建和 DROP DATABASE 单次批量删除。

## 数据与状态

本文件没有全局可变状态。所有定义都在调用栈上构造并按值返回；`AffinityTable`、key 字节和组 ID 都是拥有所有权的数据。`BTreeMap`/`BTreeSet` 使组遍历和交给后端的 ID 顺序确定，便于测试与日志稳定；这比 Go `map` 的无序遍历更强，但不改变组集合语义。

组 ID 不随表名称变化。Classic（无 codec 或 `keyspace_id=None`）为 `_tidb_t_{table_id}` / `_tidb_pt_{table_id}_p{partition_id}`；NextGen（`Some(keyspace_id)`，包括 0）为 `_tidb_ks{keyspace_id}_t_{table_id}` / `_tidb_ks{keyspace_id}_pt_{table_id}_p{partition_id}`。两个 `*_for_keyspace` helper 接受已解析 ID，供 session 运行时复用，避免不同 keyspace 的相同物理 ID 冲突。表级范围取表 ID；分区级范围取每个分区 ID。`AffinityTable.name` 当前只是为未来更友好的诊断保留。

范围结束值使用 `wrapping_add(1)`，因此 `i64::MAX` 的结束 ID 回绕为 `i64::MIN`；`test_affinity_key_range_max_physical_id_wraps_like_go` 明确锁定了这一与 Go `int64` 运算相同的边界。该行为不能改成饱和加法而不破坏兼容性。

## 依赖与调用关系

内部调用主链为：

`create_table_affinity_groups` / `delete_table_affinity_groups` / `batch_delete_table_affinity_groups` → `build_affinity_group_definitions` → 组 ID 函数 + `build_affinity_group_key_range` → 可选 `AffinityCodec::encode_region_range`；创建/删除入口最后调用 `AffinityGroupManager`。

冻结提交的 `git grep` 确认 session 运行时调用两个 `*_for_keyspace` helper；组定义及 manager 入口仍是本文件内部组合和独立测试使用。运行时从 domain 获取 keyspace 并自行编码范围、调用亲和后端，应与本文件抽象层区分。

`pkg/ddl/Cargo.toml` 确认本文件属于 `astersql-ddl`，但源码只使用标准库；它没有直接引用 manifest 中的 `astersql-domain-affinity`。该依赖目前只位于 `target.'cfg(windows)'.dependencies`，而本文件通过本地 trait 隔离后端。模块公开性来自 `pkg/ddl/lib.rs`；测试装配也在同一文件。Go 生产侧则直接依赖 `pkg/domain/affinity`、TiKV codec、PD HTTP 类型和 DDL `jobContext`。

## 错误处理与边界

- `None` 表、未设置 affinity、空的待创建/删除集合均视为成功的空操作，并避免后端调用。
- 分区级配置在最终选中的分区 ID 列表为空时返回 `AffinityError::MissingPartitions { table_id }`，后端不会被调用。
- codec 不返回 `Result`，所以编码失败无法在此层表达；其实现必须保证编码操作本身可完成。
- 后端 trait 用 `Result<(), String>`，入口将字符串无损包入 `AffinityError::Backend`，但不保留结构化错误源、重试次数或错误上下文。
- `InvalidLevel` 当前不可达；若未来从 AST/元数据解析 level，应在解析边界显式产生它或改成携带原值的错误，不能假称现有枚举匹配已经验证外部非法值。
- `wrapping_add` 是刻意的 Go 兼容边界。负 ID 也会按相同 memcomparable 规则编码，本文件不负责验证物理 ID 是否业务合法。
- 删除注释中的“best effort”是外层策略，不是函数行为：本函数会返回后端错误。生产调用方若要对齐 Go 的 DROP/TRUNCATE/ALTER 行为，必须在作业层选择记录并继续还是向上失败。

## 并发与资源生命周期

本文件不创建线程、异步任务、锁、通道或事务。函数同步执行；`&mut dyn AffinityGroupManager` 保证单次调用期间对管理器的独占可变借用，但不声明管理器跨线程安全，也不防止不同管理器实例并发操作同一 PD 组。

codec 在组名及范围构造期间以共享借用存在；构造出的 key 拥有字节缓冲区，不依赖 codec 生命周期。manager 借用只覆盖一次创建或删除调用。网络连接、超时、重试退避和 PD 资源的持久生命周期全部由未来的 manager 实现负责。创建幂等性及删除重试同样是 trait 契约；`MemoryGroups` 测试替身只验证集合效果和调用次数，不模拟真实并发或重试。

从 DDL 生命周期看，Go 对照在 owner 执行 job step 时使用 `jobContext.stepCtx` 和 store codec：创建失败属于关键路径并返回错误，部分删除清理由更外层记录后继续。Rust 文件尚未绑定 job context、owner failover、取消/回滚、schema version 同步或系统表，因此不能单独证明集群故障恢复语义。

## 与 Go 版本的对应关系

来源 d0665b0713432cb7a38d283885d61a6c252f56f3：`collect_table_affinity_group_ids` 在逐表扫描时向既有集合追加组 ID；`batch_delete_affinity_group_ids` 只接收去重后的集合，空集合不调用 PD，后端错误原样交给调用者决定是否继续。既有 `batch_delete_table_affinity_groups` 复用这两个入口，保留调用方兼容性，不要求保存全部表才能删除。扫描失败前已收集的集合由调用者丢弃，失败表本身不追加部分组。它们复用原真实组编码逻辑，manager 抽象的生产适配边界保持原状。

有几处需要明确的表示或接线差异：

- Go 直接使用 `model.TableInfo`、`model.PartitionDefinition`、`tikv.Codec` 和 `pdhttp.AffinityGroupKeyRange`；Rust 使用本地简化结构与 trait，尚无这些生产类型的转换层。
- Go `switch` 有 `default`，能报告包含 level、表名和表 ID 的非法级别；Rust 闭合枚举没有未知值，`InvalidLevel` 未使用，`MissingPartitions` 也只携带表 ID。
- Go 空结果通常为 `nil` map/slice；Rust 返回空 `BTreeMap`/`Vec`。调用语义相同，但 Rust 额外保证排序确定性。
- Go 入口从 `jobContext` 取得 step context 和 store codec，并直接调用 `domain/affinity`；Rust manager/codec 由调用者注入，当前没有生产适配器。
- Go 的生产调用边包括：CREATE TABLE 创建组，ALTER/TRUNCATE 创建新组并尽力清理旧组，DROP TABLE 清理，DROP DATABASE 批量清理；Rust session 资源代码已复用组名 helper，但不是直接调用本文件的 manager 入口。

Go 测试 `pkg/ddl/affinity_test.go` 同时包含算法测试和经 mock store/domain 的 PD 交互测试；Rust `pkg/ddl/affinity_test.rs` 对纯算法和内存 manager 有较细覆盖，但不等价于真实 PD/domain 集成验证。

## 扩展指南

在现有 session 组名 helper 复用之外，若要接入本模块的 manager/codec 抽象，需：实现从真实表元数据到 `AffinityTable` 的无损转换；为真实 store codec 实现 `AffinityCodec`；为 `astersql-domain-affinity` 提供 `AffinityGroupManager` 适配器；再在与 Go 对应的 CREATE、ALTER、TRUNCATE、DROP TABLE 和 DROP DATABASE job step 中调用。创建失败必须阻止相应关键步骤，旧组删除则必须逐调用点核对 Go 的尽力清理策略，不能把所有错误统一吞掉或统一升级为 job 失败。

修改组名或 key 编码时，应同时更新 `get_*_group_id`、`encode_table_prefix`/`build_affinity_group_key_range` 及独立测试，并验证已存在 PD 组的向后兼容与清理可达性。修改分区选择规则时，应保持 `None` 回退与显式空 slice 报缺少分区的区别，并覆盖部分分区、重复 ID、TRUNCATE 替换 ID。引入异步或并发 manager 前应定义幂等键、重试可见性、超时/取消和同一组并发创建删除的顺序语义。

测试仍应放在独立的 `pkg/ddl/affinity_test.rs`，不要内嵌回生产源文件。生产适配器落地后还需要新增独立集成测试，覆盖后端错误、重试、owner/job 重放、CREATE 失败回滚以及删除失败继续策略；现有 `MemoryGroups` 测试不足以验证这些行为。

## 验证依据

本提交独立测试：`port_d0665b0713_` 前缀，分别位于 `affinity_test.rs`、`events_test.rs` 和 `infoschema_v2_test.rs`，按所属 crate 聚焦执行。物理 ID 回归修复前断言得到 [10,20]，修复后得到 [10,11,12,20]；其它新增 API 使用当前绿灯验证。测试与实现分文件；最终命令和有效数量保存在本提交正文。
