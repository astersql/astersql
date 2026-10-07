# `pkg/ddl/split_region.rs`

## 文件定位

本文件属于 `astersql-ddl` crate；模块由 `pkg/ddl/lib.rs` 以 `pub mod split_region` 公开。它负责把已经存在于 `astersql_meta_model::TableInfo` 中的预切分配置转换为 TiKV Region split key，通过 `astersql_kv::SplittableStore` 请求切分，并按会话保存下来的 scatter scope 等待新 Region 打散。

生产入口是 `pkg/session/runtime/ddl.rs::pre_split_and_scatter`：CREATE TABLE 元数据持久化后，以及 ADD/REORGANIZE PARTITION 的相应路径上，该入口在排除临时表、检查启动模式与 `EnableSplitTableRegion`/显式配置后，取得 storage 的 `region_splitter()`，再调用本文件的 `split_table_regions`。因此它是 DDL 完成元数据动作后的物理存储优化路径，不负责创建或推进 DDL job，不改变 schema state，不执行 reorg/backfill，也不更新 schema version。

`pkg/ddl/Cargo.toml` 将本目录定义为 `astersql-ddl`（`[lib] path = "lib.rs"`），并直接依赖本文件使用的 `astersql-kv`、`astersql-meta-model`、`astersql-meta-autoid`、`astersql-expression`、`astersql-tablecodec`、`astersql-types`、`astersql-util-regionsplit`、`astersql-util-chunk` 和 `astersql-store-driver-error` 等 workspace crate。本文件没有 feature gate 或条件编译项；独立测试由 `pkg/ddl/lib.rs` 中的 `#[cfg(test)] mod split_region_test` 挂载。

## 核心职责

1. `split_table_regions` 实现生产预切分主流程：识别分区物理 ID、区分全局/本地索引、选择显式 split policy、shard/auto-random 预切分或仅表前缀切分，并收集存储返回的 Region ID。
2. `policy_bounds` 重新解析持久化 policy 的表达式文本，在当前表达式上下文中求值，并在边界列数匹配时转换为 handle/index 列类型；表级 policy 拒绝 NULL，索引 policy 允许 NULL。
3. `get_scatter_config` 将 `"table"`、`"global"` 或其他值映射为是否 scatter 以及逻辑表级 group ID。物理键落在哪个 partition ID 与 scatter group 属于哪个逻辑表是两套独立维度。
4. `normalize_split_policy`、`encode_record_key`、`encode_index_key`、`pre_split_record_keys`、`policy_split_keys`、`scatter_scope`、`wait_scatter_finished` 提供较轻量的兼容/算法辅助 API。全仓搜索显示这些 API 当前没有生产调用者；其中部分只被 `pkg/ddl/split_region_test.rs` 或 benchmark 使用，生产路径使用真实 `TableInfo`、`tablecodec`、`regionsplit` 与 KV 错误类型，不能把这些辅助 API 当作完整生产实现。

从 DDL 执行框架看，本文件是 metadata 已提交后的 fast path/best-effort side effect：没有 schema state transition、持久化 checkpoint、取消回滚或 delete-range GC；切分失败不会回滚建表，因为 TiKV 后续仍可按负载自动切分。持久化输入仅是 `TableInfo` 内的 `TableSplitPolicy`、各 `IndexInfo::RegionSplitPolicy`、shard/auto-random 位数和分区/索引元数据。

## 主要符号

- `ScatterScope::{Global, Table}`：轻量 scatter 枚举，只被 `scatter_scope` 返回，未进入生产 `split_table_regions`；生产入口直接传递字符串 scope。
- `RegionSplitPolicy`：轻量策略结构，保存字符串上下界、Region 数、value lists 和可选索引名。它不同于生产使用的 `astersql_meta_model::RegionSplitPolicy`。
- `SplitTableInfo`：轻量表描述，保存逻辑/分区 ID、shard 位数、预切位数和索引 ID；`index_ids` 当前未被本文件算法读取。
- `SplitError`：辅助 API 的错误枚举，涵盖边界、Region 数、预切位数、表达式和 scatter 失败；`Display` 直接输出 `Debug` 形式。生产主流程不返回该类型。
- `normalize_split_policy(...) -> Result<RegionSplitPolicy, SplitError>`：互斥校验区间模式与 value-list 模式，将索引名转为 ASCII 小写；不负责真实 SQL AST 恢复、表达式求值或列类型转换。
- `encode_record_key` / `encode_index_key`：辅助编码器。前者翻转有符号 handle 的符号位以形成可比较字节；后者用零字节分隔字符串值。生产流程使用 `astersql_tablecodec`，所以扩展生产键格式时不应只修改这两个函数。
- `pre_split_record_keys(&SplitTableInfo)`：验证 `pre_split_regions <= shard_row_id_bits`、`pre_split_regions <= 15`、`shard_row_id_bits <= 63`，按输入物理 ID 顺序产生表前缀及 shard 边界记录键。
- `policy_split_keys(...)`：轻量策略生成器；把形如 `idx{N}` 的名字解释为索引 ID，否则把 value list 第一项或区间第一列解析为 `i64`，最后排序去重。它不支持生产 policy 的完整多列/类型语义。
- `wait_scatter_finished(...)`：测试友好的简化等待器，首个 `Err(())` 即返回 `ScatterFailed(1)`；它无法表示生产代码对 `PdError` 的“记录后继续”规则。
- `GLOBAL_SCATTER_GROUP_ID = -1` 与 `get_scatter_config(scope, table_id)`：全局 scatter 固定使用 `-1`，表级 scatter 使用逻辑表 ID，其他 scope 关闭 scatter 但仍返回表 ID 作为 group 参数。
- `split_table_regions(context, expressions, store, table, scope) -> Vec<u64>`：生产主入口。返回成功切分调用累计得到的 Region ID；单次存储失败、policy 求值失败或等待失败只记录到标准错误，不作为函数错误返回。
- `policy_bounds(...)`：私有的持久化 policy 求值器。要求 `Regions > 0`，使用 `ParseSimpleExpr` 和空行求值；值数与列数相等时执行 `ConvertValueToColumnType`，并允许负数按 unsigned 列规则转换。

## 执行流程

`split_table_regions` 的流程如下：

1. 从 `expressions.GetEvalCtx().Location()` 建立 `astersql_util_regionsplit::StatementContext`，然后由 `get_scatter_config(scope, table.ID)` 固定本次所有 split 请求的 scatter 开关和逻辑表 group。
2. 建立局部 `split` 闭包。每批 key 调用 `store.SplitRegions(context, &keys, scatter, Some(group))`；成功时追加 Region ID，失败时打印错误并继续后续批次。
3. 从 `TableInfo` 判断是否存在表级或任一索引级 policy；从 `GetPartitionInfo` 取得物理分区 ID，非分区表则只使用 `table.ID`。`applicable_index` 保证分区表的 global index 只落在逻辑表 ID，本地 index 只落在各分区 ID；普通本地索引前缀使用 `index.ID + 1`，global index 使用原 ID。
4. 若存在任一 policy，则 policy 模式优先于 shard/auto-random 配置。分区表的 policy ID 列表会在分区 ID 前插入逻辑表 ID：逻辑表位置只处理 global index policy，各分区位置处理表 policy 和 local index policy。聚簇主键索引会被跳过，因为其键空间由记录键表达。
5. 对表 policy，`policy_bounds(..., reject_null = true)` 使用 handle 列信息解析边界，再调用 `GetSplitTableKeysForModel`；对索引 policy，按 `IndexInfo::Columns[*].Offset` 收集列信息，允许 NULL 边界，再调用 `GetSplitIndexKeysForModel`。任一 policy 解析/生成失败只跳过该 policy，其余 policy 继续。
6. 若没有 policy，则优先使用 `AutoRandomBits`，否则使用 `ShardRowIDBits`。位数与 `PreSplitRegions` 均大于零时，`ShardIdFormat` 结合主键 unsigned 属性及 `AutoRandomRangeBits` 计算增量位数；每个物理表先生成表前缀，再生成等距 shard handle 键，随后单独切索引前缀。分区表还先为逻辑表切 global index 前缀。
7. 若既没有 policy 也没有显式 shard 预切配置，则每个物理表只以 `GenTablePrefix(physical)` 发起一次切分。
8. scatter 开启时，按累计 Region ID 顺序调用 `WaitScatterRegionFinish(context, region, 0)`。PD 错误只记录并继续等待后续 Region；非 PD 错误记录后中止等待。最终无论等待结果如何都返回累计 Region ID。

上游 `pkg/session/runtime/ddl.rs::pre_split_and_scatter` 在调用前添加 `InternalTxnDDL` 来源标记，并从会话状态读取持久化/有效 scatter scope；存储不支持 `region_splitter()` 时直接跳过物理切分。调用后 session runtime 仍会更新其 Region 数量记账，随后 CREATE/分区路径更新本节点 schema version。

## 数据与状态

生产输入状态来自只读的 `TableInfo`：逻辑表 ID、分区 definitions、索引 ID/列/Global/Primary 标记、表/索引 `RegionSplitPolicy`、`AutoRandomBits`、`AutoRandomRangeBits`、`ShardRowIDBits`、`PreSplitRegions` 及主键类型。函数不修改 `TableInfo`。

物理键必须遵守两项不变量：记录键和 local-index 键使用真实 physical table/partition ID；global-index 键使用逻辑表 ID。scatter group 则始终由逻辑表 ID 和 scope 决定，与物理键 ID 无关。`pkg/ddl/split_region_test.rs::partition_presplit_separates_physical_keys_from_scatter_groups` 与 `partitioned_split_policies_keep_record_keys_on_physical_ids` 分别验证这两项约束。

函数内唯一可变状态是本次调用的 `regions: Vec<u64>` 以及局部生成的 key 列表。每次成功的 `SplitRegions` 才向 `regions` 追加 ID；失败批次可能完全不贡献 ID，但不影响后续批次。返回值因此表示存储报告成功的 Region ID 集合，不是期望 Region 总数，也不表示 scatter 全部完成。

持久化 policy 保存的是表达式文本及定义时信息；当前 Rust `policy_bounds` 使用调用方提供的 expression context 时区构造 statement context。Go 版另有 `policy.TimeZone` 的恢复流程；Rust 生产入口目前创建默认 `NewExprContext`，因此除现有测试覆盖的类型转换外，定义时区重放的一致性不能仅由本文件证明。

轻量 `RegionSplitPolicy`/`SplitTableInfo`/`SplitError` 与生产模型并存，但没有相互转换；它们的状态不能自动进入 `split_table_regions`。

## 依赖与调用关系

上游调用链可由源码与 RustCodeGraph 共同复核：

- CREATE TABLE：`pkg/session/runtime/ddl.rs` 在 `ddl_create_table` 成功后调用 `pre_split_and_scatter`，再调用 `update_self_version_with_retry`。
- 分区变更：同文件 ADD PARTITION 与 REORGANIZE PARTITION 分支也调用 `pre_split_and_scatter`。
- `pre_split_and_scatter` 经 `domain.stats_table` 读取 `TableInfo`，经 `domain.storage_handle().region_splitter()` 取得 `SplittableStore`，调用 `astersql_ddl::split_region::split_table_regions`。

RustCodeGraph 对 `split_table_regions` 识别出的直接下游包括 `get_scatter_config`、`policy_bounds`、`SplittableStore::SplitRegions`、`SplittableStore::WaitScatterRegionFinish`、`tablecodec::EncodeRecordKey` 与列 flag 读取；源码还明确调用 `regionsplit::GetSplitTableKeysForModel`、`GetSplitIndexKeysForModel`、`GetHandleColumnInfos`、`autoid::ShardIdFormat::new` 及多个 tablecodec 前缀函数。图的文件摘要只报告测试使用者，未捕获跨 crate 的 session 调用，因此生产上游以全仓 `rg` 和 `pkg/session/runtime/ddl.rs` 源码为补充证据。

`pkg/ddl/Cargo.toml` 中上述依赖均为本地 workspace path 依赖，没有本文件专属 feature。模块公开后，benchmark 还使用 `encode_index_key`；独立 Rust 测试直接调用生产入口和部分轻量辅助函数。

## 错误处理与边界

- 生产主流程是 best effort：`SplitRegions` 错误、policy 表达式/类型转换错误和 split-key 生成错误都被打印并吞掉，函数继续处理其他物理表或 policy；这与 Go 的“让 TiKV 后续自动切分”设计一致，但 Rust 当前使用 `eprintln!`，没有 Go DDL logger 的表名、成功数量等结构化字段。
- scatter 等待对 PD 错误继续，对其他错误停止；任何等待错误均不会从 `split_table_regions` 返回。`pkg/ddl/split_region_test.rs::presplit_wait_continues_for_pd_errors_but_stops_for_other_errors` 覆盖该分支。
- 表 policy 的 NULL 边界被 `policy_bounds` 拒绝，索引 policy 的 NULL 边界允许；不可解析、不可求值或不可转换的持久化边界导致整项 policy 被跳过且不会调用存储。`persisted_policy_bounds_are_converted_to_handle_column_types` 覆盖整数 handle、字符串索引等价输入和不可转换表 policy。
- 只有当边界值个数恰好等于目标列数时才做列类型转换；数量不匹配时错误最终由下游 key 生成器决定。索引列 offset 直接索引 `table.Columns`，依赖 `TableInfo` 内部 offset 合法这一元数据不变量。
- shard 分支计算 `1_i64 << (shard_bits - PreSplitRegions)`，依赖上游元数据校验保证位数关系和移位范围。与轻量 `pre_split_record_keys` 不同，生产函数自身不返回 `TooManyPreSplitRegions`。
- 空 key 批次没有在本文件中被过滤，会原样交给 `SplittableStore`；其语义由存储实现决定。
- `get_scatter_config` 对未知字符串静默视为 scatter off；当前约定值是 `"table"`、`"global"`、`"off"`。

由于本操作发生在元数据动作之后且错误不外传，它没有回滚语义：切分可部分成功，不能通过 DDL cancel 撤销，也没有 delete-range GC。这个容错取舍是此文件最重要的兼容边界之一。

## 并发与资源生命周期

本文件没有线程、异步任务、锁、channel 或共享可变全局状态。`split_table_regions` 同步执行每批 split，再在 scatter 开启时顺序等待每个 Region；同一表的多个 key 批次不会在本函数内并发发出。

`context: &astersql_kv::Context` 的创建和超时/取消生命周期由上游负责；session runtime 为它添加 `InternalTxnDDL` 来源标记。本文件只借用该 context，并把它传给 split 和 wait。与 Go 的 `splitTableRegion`/`splitPartitionTableRegion` 在内部用 session 配置创建 timeout context 不同，Rust 文件自身不创建 deadline，也不拥有取消句柄。

表达式上下文、store 与 `TableInfo` 均按共享引用借用，不跨调用保存。生成的 key vector 在每次同步 `SplitRegions` 调用后释放；Region ID 保留到等待结束并作为返回值移交调用者。发生部分失败时，已经创建的 Region 不回收，后续批次仍可继续，符合 best-effort 生命周期。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ddl/split_region.go`，相关测试是 `pkg/ddl/split_region_test.go`。Rust `split_table_regions` 合并对应了 Go 的 `splitTableRegion`、`splitPartitionTableRegion`、`preSplitPhysicalTableByShardRowID`、`SplitRecordRegion`、`splitIndexRegion`、`applySplitPoliciesForTable` 和 `WaitScatterRegionFinish` 的核心路径；`get_scatter_config` 对应 `getScatterConfig`，`policy_bounds` 覆盖 `prepareSplitPolicy`/`parseValuesToDatums` 的部分职责。

已对齐的关键语义包括：policy 优先于 shard 配置；auto-random 位优先于 shard-row-id 位；分区表的 global/local index 键空间分离；普通索引用 `ID + 1`、global index 用原 ID；scatter table/global group 分别为逻辑表 ID/`-1`；单批存储失败后继续；等待 PD 错误后继续而非 PD 错误后停止；边界向目标列类型转换；表 policy 禁止 NULL 而索引 policy 允许 NULL。

当前差异或验证限制包括：

- Go 在入口内创建带 `GetSplitRegionTimeout()` 的 context；Rust 依赖上游提供 context，当前 session 调用片段未设置等价 deadline。
- Go 的 policy preparation 使用 `policy.TimeZone` 恢复定义时区，并对 prefix common handle 等有更完整处理；Rust `policy_bounds` 使用传入 expression context，Rust 独立测试没有覆盖 Go 测试中的 timestamp 定义时区、负数转 unsigned、common-handle prefix 等全部场景。
- Go 使用结构化 DDL logger 并保留部分成功数量；Rust 仅 `eprintln!`，生产函数错误不可观测为返回值。
- 文件前半的轻量策略/编码 API 是 Rust 特有的简化层，不等价于 Go 的完整 `normalizeSplitPolicy`。尤其 `policy_split_keys` 仅支持 `i64` 第一列或 `idx{N}` 约定，不能用于声称多列、SQL 类型、时区或 value-list 语义已经完整移植。

## 扩展指南

- 新增生产切分策略应优先修改 `split_table_regions` 及真实模型/`astersql_util_regionsplit` 接口；不要只扩展轻量 `policy_split_keys`。若两套 API 均需保留，应明确转换关系并避免键编码逻辑继续分叉。
- 修改分区或索引处理时，必须保持“物理 key ID”和“逻辑 scatter group ID”分离，并同步覆盖 global index、local index、聚簇主键跳过规则及普通索引 `ID + 1` 边界。
- 修改 policy 求值时，应在 `policy_bounds` 处理定义时区、列数、NULL 和类型转换；需要与 Go `prepareSplitPolicy`、`parseValuesToDatums`、`splitPolicyApplyCtx` 的现行语义逐项核对。对持久化旧 policy 要维持“整项跳过，不发部分错误 key”的兼容行为。
- 修改错误策略时需谨慎：把 best-effort 改为返回错误会改变 CREATE/分区 DDL 的用户可见成功条件；修改 PD/non-PD 分类会改变阻塞时间和热点风险。应同时检查 `astersql_kv::errors::Cause` 和 driver `PdError` 的包装链。
- 修改 shard 位运算前应确认上游对 `PreSplitRegions <= shard_bits` 的校验仍然成立，并覆盖 signed/unsigned 主键、auto-random range bits、无分区及多分区。
- 回归测试放在独立文件 `pkg/ddl/split_region_test.rs`，不要内嵌到生产源文件。生产接线变化还应同步检查 `pkg/session/runtime/ddl.rs` 的相关独立测试；Go 语义依据在 `pkg/ddl/split_region_test.go`。若涉及真实 TiKV scatter/timeout，再选择相应 integration/RealTiKV 测试，而不是用编译成功代替行为证据。
- 性能风险主要来自 key 数量随 `2^PreSplitRegions` 增长、分区数乘以索引数的同步请求，以及逐 Region 串行等待；扩展时避免无界复制、重复求值和无必要的空 split 请求。

## 验证依据

- 目标实现：`pkg/ddl/split_region.rs`，完整读取 481 行；符号包括 1 个常量、4 个公开类型、10 个公开函数/辅助入口、1 个私有函数及 2 个 trait impl，无条件编译项。
- crate/模块边界：`pkg/ddl/Cargo.toml`、`pkg/ddl/lib.rs`；确认 crate 名、`lib.rs` 入口、直接 workspace 依赖、公开模块和独立测试挂载。
- 生产上游：`pkg/session/runtime/ddl.rs::pre_split_and_scatter` 及其 CREATE TABLE、ADD PARTITION、REORGANIZE PARTITION 调用点；全仓 `rg` 确认生产直接调用位于该 session runtime 文件。
- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；对 `split_table_regions`、`policy_bounds`、`normalize_split_policy`、`pre_split_record_keys`、`policy_split_keys`、`get_scatter_config`、`wait_scatter_finished` 执行 `query/node/callers/callees`。图确认了目标定义与核心下游边；跨 crate 上游漏报由源码/全仓搜索补齐，未把图缺失当作“无生产调用”。
- Rust 独立测试：`pkg/ddl/split_region_test.rs`，重点核对 policy 的最小 Region 数和非词法边界、shard key/物理 ID 顺序、scatter 失败停止、分区键与 group 分离、policy 物理键归属、存储失败后继续、PD/non-PD 等待差异、持久化边界类型转换与无效 policy 跳过。
- Go 对照：`pkg/ddl/split_region.go`、`pkg/ddl/split_region_test.go`，并通过 `pkg/ddl/executor.go`、`pkg/ddl/partition.go` 的调用位置确认 Go 主链。Go 测试还揭示时区、unsigned、common-handle prefix 等 Rust 独立测试尚未覆盖的语义范围。
- DDL 契约：读取 `pkg/ddl/doc.go` 与 `docs/agents/ddl/README.md`，据源码确认本文件是元数据动作后的 best-effort 存储优化，而非 job 状态机、schema transition 或 reorg/checkpoint 实现。
- 本任务只生成文档，按计划不运行 Cargo。交付前另执行任务指定的 11 章结构检查，并人工复核本文件为何存在、如何运行、失败边界及安全扩展位置。
