# `pkg/ddl/affinity.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；crate 根模块 `pkg/ddl/lib.rs` 通过 `pub mod affinity` 将其公开。它把 DDL 对 PD（Placement Driver）亲和组的需求收敛为三层纯 Rust 能力：亲和元数据的最小视图、物理表/分区 key 范围与组 ID 的确定性构造、以及创建/删除后端的 trait 边界。对应的独立单元测试位于 `pkg/ddl/affinity_test.rs`，由 `lib.rs` 的 `#[cfg(test)] mod affinity_test` 挂载。

当前仓库内除该测试外，没有其他 Rust 文件调用本文件的公开函数；因此它已经实现并测试了 Go 逻辑的核心算法与后端抽象，但尚未接入 Rust DDL 作业执行主链。实际完整生产接线仍可在 Go 对照 `pkg/ddl/affinity.go` 以及其调用方 `pkg/ddl/create_table.go`、`pkg/ddl/table.go`、`pkg/ddl/schema.go` 中看到。它不是一个独立 DDL 作业状态机，不直接持久化 job、推进 schema state、回填数据或更新 schema version。

## 核心职责

1. `get_table_affinity_group_id` 与 `get_partition_affinity_group_id` 生成与 Go/PD 约定一致、基于稳定物理 ID 的组名。
2. `build_affinity_group_key_range` 将物理 ID 映射为左闭右开区间 `[t{id}, t{id+1})`，必要时再经 `AffinityCodec` 加入 keyspace/Region 编码。
3. `build_affinity_group_definitions` 根据 `AffinityLevel::Table` 或 `AffinityLevel::Partition` 生成完整的“组 ID → key 范围列表”映射，并拒绝缺少分区定义的分区级配置。
4. `create_table_affinity_groups`、`delete_table_affinity_groups` 和 `batch_delete_table_affinity_groups` 负责空操作短路、批量整理和后端错误映射；真正的 PD I/O 留给 `AffinityGroupManager` 实现。

这些职责的边界是“计算应创建/删除什么并发起抽象调用”，而不是决定外层 DDL 是否回滚或忽略清理失败。尽管函数注释记录了 Go 侧创建为关键路径、删除为尽力清理，Rust API 仍把两者错误都返回给调用方，由未来生产接线决定策略。

## 主要符号

- `AffinityLevel::{Table, Partition}`：亲和粒度。它是闭合枚举，因此当前不会从外部值产生未知分支；`AffinityError::InvalidLevel` 目前没有构造点，是为未来解析/接线保留的错误变体。
- `AffinityTable { id, name, affinity, partition_ids }`：从 Go `model.TableInfo` 抽出的最小输入。`name` 当前没有参与计算或错误格式化；`affinity: None` 表示无事可做。
- `AffinityGroupKeyRange { start_key, end_key }`：PD key 范围的本地表示，语义为左闭右开。
- `AffinityError::{MissingPartitions, InvalidLevel, Backend}`：分别表示分区元数据不完整、预留的非法级别和后端字符串错误。`Display` 当前直接输出 `Debug` 形式。
- `AffinityCodec::encode_region_range`：可选的存储编码边界。调用方不提供 codec 时保留逻辑 table key；提供时同时重写起止 key。
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

组 ID 不随名称变化，只取决于表/分区物理 ID：表级为 `_tidb_t_{table_id}`，分区级为 `_tidb_pt_{table_id}_p{partition_id}`。表级范围取表 ID；分区级范围取每个分区 ID。`AffinityTable.name` 当前只是为未来更友好的诊断保留。

范围结束值使用 `wrapping_add(1)`，因此 `i64::MAX` 的结束 ID 回绕为 `i64::MIN`；`test_affinity_key_range_max_physical_id_wraps_like_go` 明确锁定了这一与 Go `int64` 运算相同的边界。该行为不能改成饱和加法而不破坏兼容性。

## 依赖与调用关系

内部调用主链为：

`create_table_affinity_groups` / `delete_table_affinity_groups` / `batch_delete_table_affinity_groups` → `build_affinity_group_definitions` → 组 ID 函数 + `build_affinity_group_key_range` → 可选 `AffinityCodec::encode_region_range`；创建/删除入口最后调用 `AffinityGroupManager`。

RustCodeGraph 将 `build_affinity_group_definitions` 定位在 `pkg/ddl/affinity.rs:151`，并给出三个同文件调用者及其对 ID/range 构造函数的下游边。仓库级 Rust 搜索未发现测试以外的外部调用者，说明当前 Rust 生产接线缺失，而不是图中存在一条已经落地的 DDL job 链。

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

codec 仅在范围构造期间以共享借用存在；构造出的 key 拥有字节缓冲区，不依赖 codec 生命周期。manager 借用只覆盖一次创建或删除调用。网络连接、超时、重试退避和 PD 资源的持久生命周期全部由未来的 manager 实现负责。创建幂等性及删除重试同样是 trait 契约；`MemoryGroups` 测试替身只验证集合效果和调用次数，不模拟真实并发或重试。

从 DDL 生命周期看，Go 对照在 owner 执行 job step 时使用 `jobContext.stepCtx` 和 store codec：创建失败属于关键路径并返回错误，部分删除清理由更外层记录后继续。Rust 文件尚未绑定 job context、owner failover、取消/回滚、schema version 同步或系统表，因此不能单独证明集群故障恢复语义。

## 与 Go 版本的对应关系

`pkg/ddl/affinity.go` 是直接语义基准：Rust 的两个 ID 函数、key range 构造、定义构造、ID 收集以及创建/单表删除/批量删除入口分别对应 Go 的同名 CamelCase 或非导出函数。表级和分区级命名、codec 可选编码、显式分区优先、空配置短路、批量去重等核心行为保持一致。

有几处需要明确的表示或接线差异：

- Go 直接使用 `model.TableInfo`、`model.PartitionDefinition`、`tikv.Codec` 和 `pdhttp.AffinityGroupKeyRange`；Rust 使用本地简化结构与 trait，尚无这些生产类型的转换层。
- Go `switch` 有 `default`，能报告包含 level、表名和表 ID 的非法级别；Rust 闭合枚举没有未知值，`InvalidLevel` 未使用，`MissingPartitions` 也只携带表 ID。
- Go 空结果通常为 `nil` map/slice；Rust 返回空 `BTreeMap`/`Vec`。调用语义相同，但 Rust 额外保证排序确定性。
- Go 入口从 `jobContext` 取得 step context 和 store codec，并直接调用 `domain/affinity`；Rust manager/codec 由调用者注入，当前没有生产适配器。
- Go 的生产调用边包括：CREATE TABLE 创建组，ALTER/TRUNCATE 创建新组并尽力清理旧组，DROP TABLE 清理，DROP DATABASE 批量清理；Rust 搜索目前未发现对应调用边。

Go 测试 `pkg/ddl/affinity_test.go` 同时包含算法测试和经 mock store/domain 的 PD 交互测试；Rust `pkg/ddl/affinity_test.rs` 对纯算法和内存 manager 有较细覆盖，但不等价于真实 PD/domain 集成验证。

## 扩展指南

要把本模块接入 Rust DDL 主链，最小安全路径是：实现从真实表元数据到 `AffinityTable` 的无损转换；为真实 store codec 实现 `AffinityCodec`；为 `astersql-domain-affinity` 提供 `AffinityGroupManager` 适配器；再在与 Go 对应的 CREATE、ALTER、TRUNCATE、DROP TABLE 和 DROP DATABASE job step 中调用。创建失败必须阻止相应关键步骤，旧组删除则必须逐调用点核对 Go 的尽力清理策略，不能把所有错误统一吞掉或统一升级为 job 失败。

修改组名或 key 编码时，应同时更新 `get_*_group_id`、`encode_table_prefix`/`build_affinity_group_key_range` 及独立测试，并验证已存在 PD 组的向后兼容与清理可达性。修改分区选择规则时，应保持 `None` 与显式空 slice 的区别，并覆盖部分分区、重复 ID、TRUNCATE 替换 ID。引入异步或并发 manager 前应定义幂等键、重试可见性、超时/取消和同一组并发创建删除的顺序语义。

测试仍应放在独立的 `pkg/ddl/affinity_test.rs`，不要内嵌回生产源文件。生产适配器落地后还需要新增独立集成测试，覆盖后端错误、重试、owner/job 重放、CREATE 失败回滚以及删除失败继续策略；现有 `MemoryGroups` 测试不足以验证这些行为。

## 验证依据

- 源码与模块边界：`pkg/ddl/affinity.rs`、`pkg/ddl/lib.rs`、`pkg/ddl/Cargo.toml`。
- Rust 独立测试：`pkg/ddl/affinity_test.rs`，覆盖 ID/range、codec、分区选择、`i64::MAX` 回绕、缺少分区、幂等创建、部分删除、TRUNCATE 新组和 DROP DATABASE 批量删除。
- Go 对照与生产调用：`pkg/ddl/affinity.go`、`pkg/ddl/create_table.go`、`pkg/ddl/table.go`、`pkg/ddl/schema.go`；Go 测试为 `pkg/ddl/affinity_test.go`。
- RustCodeGraph：索引状态为 11,467 个文件、307,296 个节点；`query build_affinity_group_definitions --kind function --json` 定位到 `pkg/ddl/affinity.rs:151`，`node build_affinity_group_definitions` 确认三个同文件上游入口与 ID/range 下游构造。`explore` 同时显示 Go 生产调用边和 Rust 本地调用边。
- 仓库搜索：排除 `pkg/ddl/affinity.rs` 与 `pkg/ddl/affinity_test.rs` 后，对六个公开操作函数名的 `*.rs` 搜索无匹配，据此限定“Rust 尚未生产接线”的结论。
- DDL 协议背景仅用作定位，来自 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`；具体行为结论均回查上述源码和测试，而非依赖文档推断。
