# `pkg/session/runtime/create_table_resources.rs`

## 文件定位

本文件是 `astersql-session` crate 中系统 DDL 会话与 CREATE TABLE 外部资源之间的适配层。它没有作为普通模块从 `runtime.rs` 公开，而是由 `pkg/session/runtime/system_session.rs:997-998` 通过 `#[path = "create_table_resources.rs"] mod create_table_resources;` 私有装入；对外入口也都只有 `pub(super)` 可见性。

上游主链是 `pkg/ddl/persistent_create_table.rs` 的持久化 CREATE TABLE 状态机调用 `JobExecutionContext`，`pkg/session/runtime/system_session.rs:1370-1398` 的 `ConcreteJobExecutionContext` 实现再把 TiFlash、placement、columnar、affinity 和 auto-ID 操作转发到本文件。标签迁移和删除 affinity 则分别由同一适配器的 `update_table_labels`、`delete_drop_table_affinity` 转入。因而本文件负责的是元数据事务之外的资源接线，不负责表元数据校验、DDL 状态迁移或 schema version 发布。

`pkg/session/Cargo.toml` 表明它属于 `astersql-session`，直接使用的边界包括 `astersql-domain`、`astersql-meta-autoid`、`astersql-autoid_service`、`astersql-domain-affinity`、`astersql-domain-infosync`、`astersql-ddl-placement`、`astersql-tablecodec`、`reqwest` 与 `serde_json`。该 crate 只有 `nextgen` feature；本文件自身没有条件编译项。

## 核心职责

1. 把 canonical storage 的 KV 事务包装成 `autoid::IdStore` / `IdTransaction`，按 TiDB meta hash-key 规则读写 ID 基数，并在 CREATE TABLE 后独立 rebase RowID、AutoIncrement 和 AutoRandom 分配器。
2. 为 `AUTO_ID_CACHE = 1`、表版本至少为 5、存在独立 auto-increment 列的表建立 single-point allocator；真实存储通过 PD/auto-ID service 发现客户端，mock storage 注入本地 mock 客户端。
3. 在 columnar 路径启用时执行 `tidb_columnar_storage_enabled` 门禁，区分普通 TiFlash replica 与 columnar index 的错误。
4. 将表级或分区级 affinity 转成编码后的物理 key range，并通过 mock 全局管理器或真实 PD HTTP 客户端创建/删除 affinity group。
5. 提交 placement bundle，配置表/分区的 TiFlash learner rule，并对 adding partitions 发起加速调度。
6. 在表或 schema 名称变化时读取旧 label rule，按新名称和物理 ID 重建规则，并可选择删除旧规则。

这些操作都返回 `Result<_, String>`，以便 DDL 状态机决定取消、重试或包装用户可见错误；本文件不自行修改 DDL job 状态。

## 主要符号

- `storage_error`：把底层存储、UTF-8、整数解析等错误统一降格为 `autoid::AutoIdError::Storage`。
- `Store(Arc<StorageHandle>)`：canonical domain storage 的共享句柄；实现 `autoid::IdStore`，并额外实现 `autoid::Requirement` 的本地 allocator 存储能力。
- `Txn<'a>(&'a mut dyn kv::Transaction)`：借用一个新 KV 事务，实现 `get`、`put`、`inc`、`copy_to`。`get` 将不存在的 key 解释为基数 0。
- `key`：把 `AutoIdKeyKind` 映射为 meta key 前缀：RowID 和旧版 IncrementID 使用 `TID`，版本至少 5 的独立 IncrementID 使用 `IID`，RandomID 使用 `TARID`，sequence 使用 `SID`/`SequenceCycle`。
- `Requirement`：在 `Store` 之外携带 `ClientDiscover` 和 keyspace ID；其 `single_point_allocator` 总是构造 `SinglePointAllocator`。
- `MockStore`、`UnusedDiscovery`、`UnusedConnector`：mock-storage 的 auto-ID service 适配。客户端必须由 `requirement` 预先 seed；若意外进入 discovery/connector 会明确报错。
- `requirement`：读取 storage name、UUID、DDL keyspace 与 PD endpoints，按 mock/真实存储构建 single-point allocator 所需环境；真实路径可读取集群 CA、证书和私钥，并为非 null keyspace 使用 `/keyspaces/tidb/{id}` namespace。
- `rebase_ids`：从 `TableInfo` 提取 allocator 相关字段，选择本地或 single-point requirement，计算三个候选基数并逐个 `rebase(..., false)`；single-point 分配器随后安装进 `Domain`。
- `check_columnar`：安装 planner expression factory 后检查 TiFlash replica、CSE columnar 配置和全局变量；对 columnar index 返回专用“不支持”错误，否则返回 TiFlash columnar storage 未启用错误。
- `create_affinity` / `delete_affinity`：创建或删除 `_tidb_t_<table>`、`_tidb_pt_<table>_p<partition>` group；分区级创建要求存在非空 definitions。
- `read_optional_tls` / `pd_client`：加载可选 TLS 文件并用 DDL PD endpoints 建立阻塞 HTTP client。
- `put_bundles`：空 bundle 快速成功；mock 路径复用 infosync retry，真实路径向 placement-rule partial API 发送 JSON，并按 infosync 常量重试。
- `replica_rule` / `configure_replica`：生成 TiFlash learner rule；按 CSE 配置、是否存在 replica、mock/真实存储以及表/分区分支执行配置、删除或 adding-partition 加速。
- `update_labels`：选择 keyspace v1/v2 codec，读取旧表/分区规则，保留存在的规则并 `Reset` 到新 schema/table/partition 与物理 ID，最后提交 `LabelRulePatch`。

文件没有模块级常量、自定义 enum、公开 trait 或条件编译项；全部辅助类型和函数仅服务于上述资源操作。

## 执行流程

CREATE TABLE 的顺序由 `pkg/ddl/persistent_create_table.rs:82-170` 决定：

1. `check_create_table_columnar` 先调用 `check_columnar`；失败会取消 job，尚未进入元数据创建。
2. 状态机在自己的事务中验证并写入表元数据、构造 placement bundles；本文件不参与该事务。
3. 若指定 TiFlash replica，`configure_replica` 为普通 definitions 和 adding definitions 配置规则。真实 PD 路径先确保 `tiflash` rule group 的 index/override 正确，再批量提交 rules；adding definitions 还提交 accelerate ranges。
4. 若 bundle 非空，`put_bundles` 通知 PD。真实路径最多尝试 `RequestPDMaxRetry + 1` 次，尝试间按 `RequestRetryInterval` 阻塞等待。
5. 若有 affinity，`create_affinity` 按 level 展开物理 ID，用 `[GenTablePrefix(id), GenTablePrefix(id + 1))` 生成并经 storage codec 编码的 range，然后幂等创建 group。
6. `rebase_ids` 最后在新事务中设置 ID 基数。`AutoIncID` 与 `AutoRandID` 都以期望首值减一作为 base；`AutoIncIDExtra` 非零时同样减一写入 RowID allocator。

`update_labels` 是另一条资源迁移流程：先基于旧 schema/table/partition 名称生成 rule IDs，读取存在的旧规则，再用新名称和当前物理 ID 重建；表规则覆盖表 ID 及全部分区 ID。`delete_affinity` 是清理流程，只根据旧表 metadata 构造 group IDs 并请求删除。

## 数据与状态

- 持久状态主要位于 meta KV、PD placement/affinity/label 配置以及 TiFlash 同步状态中；本文件自身没有全局可变状态。
- auto-ID value 以十进制字符串字节保存在 `transaction_meta_hash_key("DB:<db>", "<prefix>:<table>")` 下。不存在等价于 0；存在但非 UTF-8 或非 `i64` 会报错，不能被当作 0。
- `Txn::inc` 使用 `wrapping_add`，与 Rust debug/release 溢出行为无关；调用者必须维护 auto-ID 不越界的不变量。
- single-point 判定严格要求 `auto_id_cache == 1`、`version >= 5`、存在 auto-increment 列且 `SepAutoInc()` 为真。只有此分支会把 AutoIncrement allocator 安装进 `Domain` 供后续复用。
- keyspace 的 null sentinel 在 auto-ID 侧是 `autoid::NULLSPACE_ID`，在 label/TiFlash rule ID 侧以 `u32::MAX` 判断 v1；非 null keyspace 会进入 namespaced discovery、v2 label codec 和 `keyspace-<id>-table-<id>-r` rule ID。
- affinity group map 以稳定命名关联一个物理 range；partition level 只处理 `Definitions`，不处理 `AddingDefinitions`。
- label 更新只重建 `GetLabelRules` 实际返回的规则，不会凭空创建不存在的旧规则。`delete_old = false` 时 patch 保留旧 IDs，适合仍需旧命名的流程。

## 依赖与调用关系

上游调用边（RustCodeGraph 未解析出这些模块私有 trait 转发边，以下由源码引用补齐）：

- `pkg/ddl/persistent_create_table.rs` → `JobExecutionContext::{check_create_table_columnar, configure_create_table_replica, put_create_table_bundles, create_table_affinity, rebase_create_table_ids}`。
- `pkg/session/runtime/system_session.rs` 的 `ConcreteJobExecutionContext` → 本文件七个 `pub(super)` 入口。
- 同一 `JobExecutionContext` 的 rename/truncate/drop 等持久动作 → `update_table_labels` / `delete_drop_table_affinity` → 本文件对应入口。

主要下游依赖：

- `Domain::storage_handle` 提供 storage identity、事务、keyspace、PD endpoints 和 region-range 编码；`Domain::global_system_variable` 提供 columnar gate；`Domain::install_single_point_auto_id_allocator` 保存 allocator。
- `astersql_meta_autoid` 提供 allocator 类型、key schema、client discovery 与 rebase；`astersql_autoid_service` 提供 mock server 和真实客户端 TLS。
- `astersql_domain_affinity` 提供 group manager 与 PD HTTP client；`astersql_domain_infosync` 提供 mock-compatible placement、TiFlash 与 label API。
- `astersql_tablecodec` 生成表/record key range；storage 的 `EncodeDDLRegionRange` 再应用实际 codec/keyspace 前缀。
- `astersql_config` 决定 TLS、TiFlash 和 columnar 路径；`astersql_planner_core::InstallPlannerExpressionFactory` 是 columnar index 检查前的必要初始化。

RustCodeGraph 明确识别了 `rebase_ids → requirement`、`create_affinity/put_bundles/configure_replica/delete_affinity → pd_client`、`configure_replica → replica_rule` 等文件内调用边。

## 错误处理与边界

- 所有外部错误最终转换成字符串，保留底层消息但丢失结构化错误类型；上游 DDL 状态机负责补充“failed to notify PD”等上下文并设置取消状态。
- auto-ID 事务通过 `kv::RunInNewTxn(..., retryable = true, ...)` 独立提交。闭包错误先变成 KV error，外层再变成 `AutoIdError::Storage`；这保证它不依赖调用者的表元数据事务。
- `requirement` 会传播 DDL keyspace/endpoints 查询失败、TLS 文件读取失败、client discovery 构造失败。mock 的 discovery/connector 是故意不可用的防线。
- `check_columnar` 在没有正数 TiFlash replica或 CSE columnar 未启用时直接成功；需要检查却缺少全局变量时返回 `ErrTiFlashColumnarStorageCheckFailed`。值只有大小写无关的 `ON` 或精确的 `1` 被视为启用。
- `create_affinity` 对未知 level 报错；partition level 没有 definitions 也报错。`delete_affinity` 对缺少 partitions 则产生空 ID 列表并交给删除 API，行为比创建宽松。
- `put_bundles` 的真实 PD 路径会重试，其他直接 HTTP 调用没有本文件级重试；mock affinity API 自身使用 manager 的 retry 函数。
- `configure_replica` 在 TiFlash feature 关闭或 metadata 无 replica 时不做事。分区批次即使为空仍提交 rules batch，但只在 adding ranges 非空时请求 accelerate。非分区删除规则时对 JSON `id` 使用 `unwrap()`；安全性依赖 `replica_rule` 总是生成字符串 ID。
- `update_labels` 的 partition rule ID 数组与当前 definitions 按同一遍历生成，因此索引一致；如果未来将读取和重建使用不同分区集合，这一不变量必须继续保持。

## 并发与资源生命周期

- `Store`、`Requirement` 和 allocator/discovery 都通过 `Arc` 共享；`Txn` 只在 `run_in_transaction` 闭包期间可变借用底层事务，不能逃逸。
- 每次 `IdStore::run_in_transaction` 都建立、执行并提交一个独立 KV 事务，注释与 Go 语义均说明 auto-ID 写入先于 infoschema 发布是安全的，因为新 table ID 在 schema version 更新前不可使用。失败后可能留下已经 rebase 的孤立 ID key，但 table ID 绑定避免其被其他表消费。
- `ClientDiscover` 在 `Requirement` 活着期间共享；single-point allocator 安装到 `Domain` 后由 domain 延长其生命周期。局部 `remote` 仅用于 allocator 构造，不承担安装后所有权。
- PD、infosync 与文件读取均为同步阻塞操作；`put_bundles` 还会 `std::thread::sleep`。这些函数应运行在 DDL worker/system session 的阻塞执行环境，不应直接放入异步 reactor 热路径。
- 本文件不创建线程、异步任务、channel 或显式锁。并发一致性分别委托给 KV 新事务、auto-ID service、PD API、infosync 全局实现及 affinity manager。
- mock 路径依赖进程级 infosync/affinity 测试单例；独立测试在结束时恢复或清空这些全局对象，新增测试也必须这样做，避免并行测试串扰。

## 与 Go 版本的对应关系

- `rebase_ids` 对应 `pkg/ddl/create_table.go:182-213` 的 `handleAutoIncID`：两者都从 table metadata 建 allocator 集合，对 `AutoIncID`、`AutoIncIDExtra`、`AutoRandID` 使用“首值减一”并以 `force = false` rebase。Rust 为跨 crate 边界显式复制所需 `autoid::TableInfo` 字段，并额外显式安装 single-point allocator。
- columnar 门禁对应 Go `worker.checkCreateTableColumnarStorage` 及 `pkg/ddl/create_table.go:63-67` 的调用位置；Rust 流程同样在写表 metadata 前执行，并复用 TiFlash/columnar 专用 dbterror。
- `configure_replica`、`put_bundles`、`create_affinity` 分别对应 `pkg/ddl/create_table.go:118-159` 的 TiFlash 配置、`PutRuleBundlesWithDefaultRetry` 和 `createTableAffinityGroupsInPD`。mock 路径直接复用 Go 兼容的 infosync/affinity API；真实路径在 Rust 中通过 PD HTTP JSON 实现同等外部效果。
- affinity group 命名和物理范围与 `pkg/ddl/affinity.go` 的 `buildAffinityGroupDefinitions`、`createTableAffinityGroupsInPD`、`deleteTableAffinityGroupsInPD` 对齐。Go 注释将创建定义为关键路径、删除定义为 best-effort；本文件只返回删除错误，是否忽略仍由上游动作决定。
- `update_labels` 对应 `pkg/ddl/table.go:1842-1880` 的 `getOldLabelRules` / `updateLabelRules`：都只克隆存在的旧规则，重写 schema/table/partition 名称和物理 IDs，再提交 patch。Rust 额外以 `delete_old` 明确控制是否删除旧 rule IDs。
- Rust 当前实现不是对 Go 文件逐行复制：它将 CREATE TABLE 所需的多个 Go helper 汇聚到 session runtime 边界，并区分 mock 与真实 PD transport。扩展时应保持可观察语义和错误边界一致，而不是要求内部调用形式相同。

## 扩展指南

- 新增一种 auto-ID key 或 allocator 时，必须同步更新 `key` 映射、`rebase_ids` 的精简 `autoid::TableInfo` 字段与 bases 选择，并在独立测试文件覆盖 key 前缀、默认 0、基数减一和重试幂等性。不要把测试内嵌到本生产文件。
- 调整 single-point 条件或 service discovery 时，重点审查 null keyspace、mock seed、TLS 三文件读取、namespace 和 `Domain` allocator 生命周期；同步对照 Go `autoid.Requirement` 行为。
- 新增 PD 资源类型时，应优先沿 `JobExecutionContext` 增加清晰边界，再在 `system_session.rs` 适配；明确它发生在表 metadata 事务之前还是之后，以及失败应取消、重试还是 best-effort。
- 修改 TiFlash/placement HTTP payload 时，保持 keyspace rule ID、learner role、index 120、location labels、partition adding 加速以及 range codec 不变量，并扩展 `normal_ddl_plan_create_table_pd_placement_and_tiflash_requests`。
- 修改 affinity 时，同时覆盖 table/partition、空 definitions、非法 level、编码后的半开区间、PD 失败和删除语义；相关现有独立测试是 `normal_ddl_plan_create_table_affinity_requests_failure_and_range_encoding`。
- 修改 columnar gate 时，同步 Go 的 create-table 检查和错误码，并扩展 `normal_ddl_create_table_columnar_gate`，尤其保留 replica count 0 的绕过以及 `ON`/`1` 接受规则。
- 修改 label 迁移时，应新增或扩展独立 runtime 测试，覆盖 v1/v2 codec、表/分区规则缺失、rename、truncate、`delete_old` 两种模式；当前没有直接以本文件函数命名的单元测试，不能用邻近 CREATE TABLE 测试代替新增回归。
- 性能上要避免在 DDL worker 热路径增加无界重试、逐分区额外连接或重复读取 TLS 文件；兼容性上要保留 Go key 格式和 PD API payload。

## 验证依据

- 目标源码：`pkg/session/runtime/create_table_resources.rs`，RustCodeGraph `files` 确认该文件已索引且含 38 个符号；`node --file ... --offset 1/401` 阅读了全部 618 行。
- 调用图：RustCodeGraph `query` 定位七个 `pub(super)` 入口，`callees --file` 验证了 `requirement`、`pd_client`、`replica_rule` 等关键下游边；图未返回模块私有 trait 转发的 callers，已用 `rg` 和源码补证。
- Rust 上游：`pkg/ddl/persistent_create_table.rs:82-170`、`pkg/ddl/job_worker.rs:326-388`、`pkg/session/runtime/system_session.rs:997-998,1303-1398`。
- crate 边界：`pkg/session/Cargo.toml`；`pkg/session` 当前没有 `doc.go`，因此无可读取的 package doc contract。
- Go 对照：`pkg/ddl/create_table.go:56-213`、`pkg/ddl/affinity.go:104-160`、`pkg/ddl/table.go:1842-1880`。
- 独立 Rust 测试：`pkg/session/runtime/normal_ddl_create_table_test.rs` 中的 `normal_ddl_plan_create_table_auto_id_independent_notification_retry`、`normal_ddl_plan_create_table_extra_row_and_random_bases`、`normal_ddl_plan_create_table_pd_placement_and_tiflash_requests`、`normal_ddl_plan_create_table_affinity_requests_failure_and_range_encoding`、`normal_ddl_create_table_columnar_gate`；materialized-view shadow 的同类资源路径另由 `normal_ddl_create_materialized_view_shadow_test.rs` 覆盖。
- 本任务是纯文档分析，按计划不运行 Cargo 或代码测试；交付验证仅检查文档结构、路径和上述源码证据。
