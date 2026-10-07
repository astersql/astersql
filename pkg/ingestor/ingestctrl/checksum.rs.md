# `pkg/ingestor/ingestctrl/checksum.rs`

## 文件定位

该文件属于 `astersql-ingestor-ingestctrl` crate；crate 根在 [`lib.rs`](lib.rs)，并通过 `pub mod checksum` 无条件公开本模块。它描述导入完成后的两类远端校验路径：通过 TiDB SQL 执行 `ADMIN CHECKSUM TABLE`，或在取得 PD TSO 后通过抽象的 TiKV 扫描源计算校验和。同时，它提供本地 KV 校验累积、全局 GC lifetime 保护和 PD Service GC Safe Point 保护所需的数据结构。

当前接线必须与实现能力区分开看。RustCodeGraph 对构造器的查询以及仓库内精确引用搜索表明，`NewTiDBChecksumExecutor`、`NewTiKVChecksumManager` 和具体 manager 在 Rust 侧只由 [`checksum_test.rs`](checksum_test.rs) 直接使用；生产 Rust 文件目前只从本模块引用 `MinDistSQLScanConcurrency` 与 `DefaultBackoffWeight`（`pkg/executor/importer/table_import.rs`）。因此，本文件已有可测试的核心抽象和算法，但还不是 Rust 生产导入链实际使用的完整 checksum 后端。Go 生产调用则仍由 `pkg/ingestor/ingestctrl/checksum.go` 及其调用者完成。

`Cargo.toml` 将 crate 的 Go 对照包声明为 `pkg/ingestor/ingestctrl`。本文件自身只依赖标准库和 crate 根的 `CancellationToken`、`Error`、`KvPair`、`Result`，没有直接使用该 manifest 中的平台依赖或外部 Rust crate。

## 核心职责

1. `KVChecksum::update` 对每一对键值计算 CRC-64，并以 XOR 合并，同时累计 KV 数量和键值总字节数；`RemoteChecksum::IsEqual` 对 checksum、KV 数和字节数三项做严格比较。
2. `ChecksumManager` 统一“按表计算远端校验和”和“关闭资源”两个操作，`TiDBChecksumExecutor` 与 `TiKVChecksumManager` 分别实现 SQL 和 TiKV 扫描策略。
3. `GCLifeTimeManager` 用作 SQL checksum 的进程内引用计数保护：首个任务读取并按需将 `tikv_gc_life_time` 提升至 100 小时，最后一个任务尝试恢复原值。
4. `TiKVChecksumManager::Checksum` 获取一次成功的 PD TSO，登记 Service GC Safe Point，然后在同一时间戳上最多执行三次扫描，并把 `KVChecksum` 转成带库表名的 `RemoteChecksum`。
5. `GCTTLManager` 保存活动 `(table, ts)`，在出现更小的最小时间戳时更新 PD Service GC Safe Point，并在关闭时用 `ttl=0` 撤销服务保护。

## 主要符号

- 常量与全局状态：`MAX_ERROR_RETRY_COUNT = 3`；`MinDistSQLScanConcurrency = 4`；`DefaultBackoffWeight = 15`；`DefaultGCLifeTime = 100h`；原子变量 `serviceSafePointTTL` 默认 600 秒。后三个命名保留了 Go 风格，crate 根通过 lint allowance 接受这类名称。
- `KVChecksum { checksum, total_kvs, total_bytes }`：本地累积结果。`update(&KvPair)` 先以当前值 0 对 value 计算 CRC，再把结果作为 key CRC 的初值，最后 XOR 到总 checksum；字节数来自 `KvPair::size()`。
- `RemoteChecksum { Schema, Table, Checksum, TotalKVs, TotalBytes }`：远端返回格式。`IsEqual(&KVChecksum)` 不比较库表名，只比较三项数值。
- `TableInfo { schema, table, table_id, index_ids }`：调用输入。目前本文件只有 `schema`、`table` 被消费；`table_id` 和 `index_ids` 尚未传递给 `ChecksumSource` 之外的具体构建逻辑。
- `ChecksumManager: Send + Sync`：公开策略接口，方法为 `Checksum(&CancellationToken, &TableInfo) -> Result<RemoteChecksum>` 与 `Close()`。
- `SqlChecksumClient: Send + Sync`：把 SQL 查询及 GC lifetime 的读写隔离成可测试边界。`TiDBChecksumExecutor` 持有 `Arc<dyn SqlChecksumClient>` 和共享的 `Arc<GCLifeTimeManager>`。
- `GCLifeTimeManager` / 私有 `GCLifeTimeState`：互斥保护 `running_jobs` 与 `original_lifetime`；`addOneJob` 和 `removeOneJob` 实现首入/末出逻辑。
- `ChecksumSource: Send + Sync`：TiKV 路径的扫描边界，接收固定 TSO 并返回 `KVChecksum`。`PDClient` 抽象 `GetTS` 和 `UpdateServiceGCSafePoint`。
- `TiKVChecksumManager`：持有 source、PD client、GC TTL manager、扫描并发、backoff、resource group 及关闭标志。当前 `dist_sql_scan_concurrency`、`backoff_weight`、`resource_group_name` 只被保存，尚未传入 `ChecksumSource::checksum_table`。
- `GCTTLManager`：用 `Mutex<Vec<(String, u64)>>` 保存活动任务，以 `AtomicI64` 记录上次发给 PD 的 safe point，以 `AtomicBool` 保证幂等关闭。
- 私有辅助函数：`isRetryableChecksumError`、`compose_ts`、`parse_duration`、`format_duration`、`crc64`。

## 执行流程

TiDB SQL 路径从 `TiDBChecksumExecutor::Checksum` 开始：先检查取消令牌；调用 `GCLifeTimeManager::addOneJob`，首个任务读取原始 lifetime 并在小于 100 小时时写入 `100h`；随后对 schema/table 内的反引号做双写转义，拼出 `ADMIN CHECKSUM TABLE \`schema\`.\`table\``；调用 `SqlChecksumClient::query_checksum`；最后调用 `removeOneJob`。最后一个任务会尝试恢复最初的 lifetime，SQL 查询的成功或失败都原样返回。

TiKV 路径从 `TiKVChecksumManager::Checksum` 开始：

1. 以 Acquire 读取 `closed`，关闭后立即返回 `Error::Closed`。
2. 循环检查取消令牌并调用 `PDClient::GetTS`。仅 `Error::Retryable`、`Error::Timeout` 或错误文本包含 `EOF` 时等待 10ms 后继续；其他错误立即返回。取得成功 TSO 后不再重新取时间戳。
3. `compose_ts` 拒绝负的物理/逻辑分量，再按 `(physical << 18) | logical` 合成 `u64` TSO。
4. `GCTTLManager::addOneJob` 登记表和 TSO，并在需要时调用 PD 更新 Service GC Safe Point。
5. 最多三次调用 `ChecksumSource::checksum_table`，每次前检查取消。成功时先移除任务，再把本地三项结果包装为 `RemoteChecksum`；不可重试错误立即停止；可重试错误消耗一次扫描预算后继续。
6. 失败或取消时移除任务并返回最后一个错误。理论上的无错误记录分支回退为 `Error::Retryable("cannot obtain checksum timestamp")`。

`GCTTLManager::addOneJob` 在锁内追加任务并计算全部活动 TSO 的最小值。只有首次更新，或新最小值小于已记录 safe point 时才释放锁、调用 PD 并更新 `last_updated_safe_point`。`removeOneJob` 只删除第一个同名任务，不向上推进 safe point。`close` 仅执行一次，并用上次记录的 safe point、`ttl=0` 请求 PD 删除该 service。

## 数据与状态

校验结果有两层：`KVChecksum` 是无库表身份的本地数值集合，`RemoteChecksum` 在相同三项数值外增加 schema/table。CRC 实现使用 ECMA 多项式 `0x42F0E1EBA9EA3693`，逐字节、逐 bit 计算；合并采用 XOR，因此顺序不影响总值，但 `total_kvs` 与 `total_bytes` 仍用于避免只凭 CRC 判断一致。

SQL 路径的共享状态完全位于 `GCLifeTimeState`。`running_jobs == 0` 表示没有受保护的 checksum；从 0 到 1 时保存 `original_lifetime`，从 1 到 0 时取走并恢复它。`parse_duration` 仅接受整数加 `h`、`m`、`s` 或无单位，使用饱和乘法；`format_duration` 优先选可整除的小时，其次分钟，最后秒。

TiKV 路径的 manager 关闭状态与 GC manager 关闭状态各自使用原子布尔值。活动 GC 任务允许同一个表名出现多次；`removeOneJob` 每次只移除第一个匹配项，测试明确覆盖这一点。`last_updated_safe_point` 以 `-1` 表示尚未更新，成功调用 PD 后保存当前最小 TSO。

## 依赖与调用关系

RustCodeGraph 给出的关键内部调用边包括：`KVChecksum::update -> crc64`；TiDB `Checksum -> SqlChecksumClient::query_checksum` 及 GC lifetime 的 add/remove；TiKV `Checksum -> PDClient::GetTS -> compose_ts -> GCTTLManager::addOneJob -> ChecksumSource::checksum_table -> GCTTLManager::removeOneJob`；`GCTTLManager::addOneJob -> PDClient::UpdateServiceGCSafePoint`。

上游方面，`lib.rs` 暴露模块并在 `#[cfg(test)]` 下挂载独立的 `checksum_test.rs`。仓库搜索没有发现 Rust 生产代码构造这两个具体 manager；`lightning/pkg/importer/checksum_helper.rs` 调用的是其自身 `crate::ingestctrl` 桩接口，不是本 crate 的模块路径。`pkg/executor/importer/table_import.rs` 只读取本模块的两个调参常量。因而“完整应用中的目标位置”是导入后本地/远端结果核验，但当前 Rust 生产调用链尚未接到本实现。

下游全部通过本文件的四个 trait 或 crate 根类型隔离：SQL/GC 数据库交互由 `SqlChecksumClient` 提供，TiKV 扫描由 `ChecksumSource` 提供，PD 操作由 `PDClient` 提供，取消和统一错误来自 `lib.rs`。这种边界让单测无需数据库、TiKV 或 PD 实例，但也意味着扫描并发、backoff、resource group 等设置只有在具体 source 接口扩展后才能实际生效。

## 错误处理与边界

- TiDB 路径在任何数据库操作前检查取消，但查询开始后是否响应取消由 `SqlChecksumClient` 的实现决定；当前 trait 的 `query_checksum` 不接收 token。
- `GCLifeTimeManager` 锁中毒映射为 `Error::Poisoned`。读取、解析或提升 lifetime 失败会阻止任务计数增加；恢复失败被有意忽略，因为 `removeOneJob` 无返回值。
- SQL 标识符只通过反引号双写处理；测试覆盖含反引号的 schema/table。数值比较要求三项完全相等。
- TiKV 取 TSO 的重试没有次数上限，只受取消令牌限制；扫描重试固定最多三次。`EOF` 通过错误字符串包含关系识别，可能比结构化错误分类宽。
- `compose_ts` 检查负值，但没有检查 logical 是否超出 18 bit，也没有检查物理时间左移是否丢失高位。
- `GCTTLManager::addOneJob` 先把任务加入 `jobs`，再调用 PD。若 PD 更新失败，函数返回错误但已加入的任务不会回滚；此时调用方也不会进入后续 remove 路径。这是当前实现的状态保留风险，不应在接线时忽略。
- `removeOneJob` 接受但不使用 token；删除后不立即把 PD safe point 向较新 TSO推进。关闭失败也被忽略。安全性依靠旧 safe point 的 TTL 仍有效，代价是可能延迟 GC。
- `last_updated_safe_point` 使用 `AtomicI64` 存放 `u64` TSO；正常 TSO 在范围内，但代码没有显式拒绝超过 `i64::MAX` 的输入。

## 并发与资源生命周期

公开 trait 均要求 `Send + Sync`，client/source/PD 实例通过 `Arc` 共享。两个 GC 管理器使用 `Mutex` 序列化复合状态变更；原子关闭标志分别用 Acquire/AcqRel 实现可见性与幂等关闭。SQL manager 的引用计数允许多个 checksum 共用一次 lifetime 提升，并只在最后一个完成后恢复。

TiKV manager 的 `Close` 不关闭 source 或 PD client，只将自身标记为关闭并撤销 Service GC Safe Point；重复调用无效果。正在执行的 `Checksum` 只在入口读取 manager 的 `closed`，因此 `Close` 与运行中的操作并没有完整协调：关闭后在途扫描仍可能继续，GC service 保护却已被撤销。Go 源码明确声明 `Close` 不可与 `Checksum` 并发，Rust 类型系统和当前实现没有编码这个前置条件，调用方必须遵守同一限制。

与 Go 不同，Rust `GCTTLManager` 不启动后台 ticker，也不会每 `TTL/3` 续租。它只在增加了更小的 safe point 时更新一次 PD；长于默认 600 秒的扫描可能让 service safe point 过期。这是生产接线前必须补齐或明确由外部层承担的生命周期缺口。

## 与 Go 版本的对应关系

直接对照文件为 [`checksum.go`](checksum.go)，测试为 [`checksum_test.go`](checksum_test.go)。Rust 保留了 Go 的主要概念：`RemoteChecksum`、`ChecksumManager`、TiDB/TiKV 两种 manager、100 小时默认 GC lifetime、三次扫描预算、最小活动 TSO 以及关闭时用 `ttl=0` 删除 Service GC Safe Point。Rust 测试还锁定了 Go 对齐点：构造器保留调用方给出的初始扫描并发（即使小于 4）、扫描可重试错误执行三次、TSO 成功获取一次，以及同表多任务只移除一个。

但当前不是等价移植，主要差异如下：

- Go TiDB executor 管理真实连接、SQL 重试、session backoff weight 的临时提升/恢复、日志与指标；Rust 只通过 `SqlChecksumClient` 发一条拼接 SQL，并管理全局 GC lifetime。
- Go TiKV `checksumDB` 构建真实 BR checksum executor，传入 table/index 元数据、并发、backoff、resource group 和 request source；Rust 把扫描交给 `ChecksumSource`，该 trait 目前没有这些配置参数，失败重试时也不会像 Go 那样把 DistSQL 并发减半至下限 4。
- Go 只对 PD leader-change 错误无限重取 TSO，间隔默认一秒；Rust 以本地通用错误分类判断 `Retryable`/`Timeout`/`EOF`，间隔 10ms。
- Go GC TTL manager 使用最小堆、UUID service ID 和后台 ticker 周期续租；Rust 使用线性扫描的 `Vec`，service ID 为 `"{prefix}-{process_id}"`，同进程相同 prefix 的多个 manager 会冲突，且没有续租线程。
- Go 提供 `NewTiKVChecksumManagerForImportInto` 和不同 request source 语义；Rust 本文件没有对应构造器。
- Go `time.ParseDuration` 支持更丰富格式；Rust `parse_duration` 只支持非负整数小时/分钟/秒。Go 对空 lifetime 会提升，Rust 空字符串会报解析错误。

因此扩展或接线时应以 Go 行为为基准逐项补齐，而不能把当前 Rust manager 当成已具备全部生产语义。

## 扩展指南

若要接入真实 Rust 导入链，优先从 `ChecksumSource`、`SqlChecksumClient` 和 `PDClient` 的具体适配器入手，再让策略选择层构造本文件的 manager；不要把数据库/TiKV 细节硬编码回算法。接线必须验证 `TableInfo.table_id/index_ids`、扫描并发、backoff、resource group 与 request source 都能到达实际请求构建层。

补齐 Go parity 时，最关键的修改点是 `GCTTLManager`：采用唯一 service ID，增加可取消、可 join 的周期续租任务，保证 add 失败回滚，并明确 `Close` 与在途 `Checksum` 的同步协议。扫描重试应在 `TiKVChecksumManager::Checksum` 或具体 source 层实现 Go 的并发减半策略；SQL 路径则需恢复 backoff、重试、连接关闭和可观测性语义。

任何逻辑变更都应同步更新独立测试 [`checksum_test.rs`](checksum_test.rs)，不要把测试内嵌到生产文件。至少增加：PD 更新失败后的 jobs 回滚、超过 TTL 的周期续租、同进程多 manager 的 service ID 唯一性、Close/Checksum 生命周期、并发降低、取 TSO 错误分类、logical 溢出边界，以及 SQL 查询失败仍恢复 lifetime。Go 对照测试在 [`checksum_test.go`](checksum_test.go)，涉及 parity 改动时应核对其 GC lifetime、单/多任务 TTL、service ID、扫描错误和并发行为。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；目标文件被完整读取为 414 行。
- RustCodeGraph 查询/导航：`query Checksum`、`query TiKVChecksumManager`、`query NewTiKVChecksumManager`、`query GCTTLManager`、`query TiDBChecksumExecutor`、`query NewTiDBChecksumExecutor`；并对构造器、`TiKVChecksumManager::Checksum`、`GCTTLManager::addOneJob` 执行 callers/callees 查询。图确认了本节“依赖与调用关系”列出的内部边；常见名称存在多义性时，以文件路径限定的 node 输出和精确仓库引用搜索消歧。
- 源码与边界：[`checksum.rs`](checksum.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)。`lib.rs` 证明模块导出、错误/取消/KV 类型和独立测试挂载；Cargo manifest 证明 crate 名称、入口和 Go package 映射。
- 测试证据：[`checksum_test.rs`](checksum_test.rs) 的五个测试覆盖 SQL 标识符转义与 lifetime 恢复、本地计数/严格相等/取消、同表 GC jobs、不可重试扫描，以及三次重试与单次 TSO；[`checksum_test.go`](checksum_test.go) 提供 Go 的 GC lifetime、TiKV checksum、周期 TTL、多任务和 service ID 行为基准。
- Go 与生产接线：[`checksum.go`](checksum.go)；另以精确引用搜索检查 `pkg/dxf/importinto/subtask_executor.go`、`pkg/executor/importer/table_import.go`、`pkg/ingestor/ingestctrl/engine_mgr.go`、`job_worker.go`、`localhelper.go`，并检查 Rust 侧 `pkg/executor/importer/table_import.rs` 与 `lightning/pkg/importer/checksum_helper.rs`，据此确认当前 Rust manager 尚未接入生产调用链。
- 本任务为纯文档分析，按计划不运行 Cargo；交付验证仅执行任务规定的 11 章节结构检查，并人工复核链接、符号名、已实现/未接线边界及上述 Go 差异。
