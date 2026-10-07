# `pkg/executor/internal/pdhelper/pd.rs`

## 文件定位

`pd.rs` 是 `astersql-executor-internal-pdhelper` crate 的核心实现文件，由同目录的 `lib.rs` 以私有模块 `mod pd` 装配并通过 `pub use pd::*` 再导出。它把 Go 包 `pkg/executor/internal/pdhelper` 中“从 PD 估算表行数、必要时回退到受限 SQL，并短期缓存结果”的行为移植为独立 Rust API。

crate 边界由 `pkg/executor/internal/pdhelper/Cargo.toml` 定义：生产依赖只有启用 failpoint 的 `fail` 与 `lru`，测试依赖配置、autoid 和 testsetup crate；`autotests = false`，所以独立测试文件由 `lib.rs` 的 `#[cfg(test)]` 路径显式挂载。根 `Cargo.toml` 将该 crate 纳入 workspace，并登记别名 `facade_executor_internal_pdhelper`。

当前接线状态必须与实现能力分开看：RustCodeGraph/源码搜索只发现本文件和同 crate 测试直接调用这些 Rust API，没有发现其它 Rust 生产文件使用 `GlobalPDHelper`。`pkg/executor/builder.rs::getApproximateTableCountFromStorage` 目前从 `dependencies.analyze_table_counts(task)` 取得可选值，并非调用本 crate。Go 运行主链则已接线：`pkg/executor/select.go` 启停全局 helper，`pkg/executor/builder.go` 和 `pkg/executor/infoschema_reader.go` 查询它。

## 核心职责

1. 用 `PDHelper::GetApproximateTableCountFromStorage` 提供带缓存的近似行数查询入口。
2. 用 `get_approximate_table_count_from_storage` 实现无缓存决策树：先请求指定物理表 ID 的 PD Region 统计；Region 数大于 2 时直接采用 `storage_keys`，否则执行 `select count(*)`。
3. 用 `InternalSourceContext` 与 `SessionContext` 抽象真实上下文、PD 存储和受限 SQL 执行器，避免该 crate 直接依赖完整 session/store 类型。
4. 用有容量上限的 `LruCache`、每项绝对过期时刻和后台线程实现 TTL/LRU 缓存。
5. 保留 Go 兼容表面：包级全局实例、包级“一次启动”语义、驼峰 API 别名、相同缓存键及 failpoint 名称。

它不负责连接真实 PD 客户端、创建真实 session 适配器、刷新统计元数据或决定采样率；调用者需要实现两个 trait，并解释返回的 `(f64, bool)`。

## 主要符号

- `DEFAULT_CACHE_TTL` / `DEFAULT_CACHE_CAPACITY`：默认 30 秒、1,048,576 项，与 `pd.go::defaultPDHelper` 的 ttlcache 配置一致。
- `GLOBAL_PD_HELPER_ONCE: AtomicBool`：整个进程内共享的启动闩锁，对应 Go 的包级 `sync.Once`，不是每个 `PDHelper` 各自一次。
- `RegionStats { count, storage_keys }`：查询算法实际使用的 PD Region 统计子集，两字段均为 `i64`。
- `PdHelperError`：仅保存消息的适配层错误；实现 `Display` 与 `Error`。公共查询边界不会把它向外传播。
- `InternalSourceContext::with_internal_stats_foreground`：消费并返回上下文，用于给小表的 COUNT 查询打上内部统计前台来源标记。
- `SessionContext::get_pd_region_stats`：`Ok(None)` 表示存储不支持所需 PD 能力；`include_stats` 在本文件恒为 `true`。
- `SessionContext::exec_restricted_count`：执行构造出的 COUNT SQL；`Ok(None)` 表示空或非法 COUNT 结果。
- `CacheEntry`：缓存数值和 `expires_at: Instant`，不保存原查询的 `has_pd`。
- `PDHelperInner`：集中持有缓存锁、TTL、清理状态锁和条件变量；`cache()`、`cleanup()` 会在 mutex poison 时取回内部值继续运行。
- `PDHelper`：公开 façade，内部以 `Arc<PDHelperInner>` 共享状态；`Default` 委托 `with_cache_config`。
- `GlobalPDHelper: LazyLock<PDHelper>`：惰性创建的默认全局实例。
- `approximate_table_count_key`：生成 `{tid}_{db}_{table}_{partition}`；`approximateTableCountKey` 是 Go 风格转发别名。
- `get_approximate_table_count_from_storage`：无缓存核心算法；`getApproximateTableCountFromStorage` 是 Go 风格转发别名。
- `count_sql` / `quote_identifier`：生成 COUNT SQL，并按 MySQL 规则用反引号包裹标识符、把内部反引号加倍。
- `pd_region_stats_with_failpoint`：处理 `calcSampleRateByStorageCount` 注入；启用时强制产生 `count = 1, storage_keys = 1_000_000`。

## 执行流程

带缓存入口 `PDHelper::GetApproximateTableCountFromStorage` 的流程如下：

1. `approximate_table_count_key` 将物理表 ID、库名、表名、分区名用下划线拼成键。
2. `get_cached` 在缓存 mutex 下执行 LRU `get`。未过期则返回数值并固定报告 `(value, true)`；已过期则立即 `pop`，按未命中处理。
3. 未命中时调用 `get_approximate_table_count_from_storage`。
4. 无缓存函数先调用 `sctx.get_pd_region_stats(&ctx, tid, true)`，再经过同名 failpoint 包装。存储不支持、PD 错误或无统计均立即返回 `(0.0, false)`，不会尝试 SQL。
5. 若 `RegionStats.count > 2`，认为表不小，直接把有符号 `storage_keys` 转为 `f64` 并返回 `(value, true)`。
6. Region 数不超过 2 时，`count_sql` 生成带可选 `partition(...)` 的 SQL；随后 `with_internal_stats_foreground` 标记上下文，再调用 `exec_restricted_count`。
7. COUNT 返回单值时转换成 `f64` 并报告成功；错误、空结果或非法结果返回 `(0.0, false)`。
8. 外层不论成功与否都调用 `insert_cached`。因此失败的数值 `0.0` 也会缓存，而 `has_pd` 不进入缓存。

清理流程独立运行：`Start` 用全局原子闩锁保证全进程只有第一次调用真正创建线程；线程持有 `Weak<PDHelperInner>`，每隔一个 TTL 或收到条件变量通知后检查停止位，正常超时则扫描并删除所有过期键。`Stop` 设置停止位、通知线程、取走句柄并 `join`。`Drop` 再调用 `Stop`，为未显式停止的 helper 收尾。

## 数据与状态

- 缓存键包含 `tid`、数据库、表和分区四部分；这是与 Go 一致的简单字符串协议。它没有长度前缀或转义分隔符，因此不同字段中出现下划线时理论上可能形成相同拼接结果，扩展时不能擅自改格式而破坏兼容性。
- 缓存值只有 `f64`。原始 `i64` 超出 `f64` 精确整数范围时可能损失精度；负的 `storage_keys` 会保留负号，相关行为由 `pd_storage_keys_preserve_go_signed_i64_semantics` 锁定。
- 每次 `put` 记录 `Instant::now() + ttl`；命中不会在本文件中显式延长 TTL。容量由 `LruCache` 控制，插入新键会驱逐最久未使用项。
- `with_cache_config` 要求 TTL 与容量均非零，否则分别通过 `assert!` 和 `NonZeroUsize::expect` panic。这是构造期不变量，不是可恢复错误。
- `CleanupState` 的 `stop` 初始为 `false`，线程句柄初始为空。全局 once 一旦置位不会复位，因此停止后不能通过再次 `Start` 重启；先启动任意自建 helper 也会占用全局启动机会。
- 缓存和清理状态使用两把独立 mutex；过期扫描先收集键再逐个移除，全程持有缓存锁。

## 依赖与调用关系

上游关系：

- Rust crate 内，`pd_test.rs` 与 `migration_aster_unit_test.rs` 直接构造 `PDHelper` 并调用缓存入口；`lib.rs` 再导出公共符号。
- 当前未找到 Rust 生产调用者或 `SessionContext` 的真实生产实现。这表示本文件是可测试的已移植组件，但不能据此声称它已进入 Rust executor 主链。
- Go 对照主链中，`pkg/executor/select.go` 的 executor 启停调用 `pdhelper.GlobalPDHelper.Start/Stop`；`pkg/executor/builder.go` 的分析采样路径和 `pkg/executor/infoschema_reader.go` 的信息模式读取路径调用近似计数。

下游关系：

- 标准库提供 `Arc`/`Weak`、`Mutex`/`Condvar`、原子量、线程与单调时间。
- `lru::LruCache` 提供容量驱逐和访问顺序维护；TTL 判断、过期清扫由本文件自己实现。
- `fail::fail_point!` 提供与 Go `failpoint.Inject("calcSampleRateByStorageCount")` 同名的测试注入点。
- 真正的 PD Region 查询和受限 SQL 执行完全委托给 `SessionContext`；上下文优先级标记委托给 `InternalSourceContext`。

RustCodeGraph 对精确入口执行 `callers`/`callees` 未返回跨文件静态调用边；源码级搜索补充证明生产接线缺失。`pkg/executor/builder.rs` 中同名方法是独立实现，不能因名称相同误判为本 crate 调用者。

## 错误处理与边界

- PD 不支持、PD 请求报错、受限 SQL 报错、COUNT 空/非法均被压平为 `(0.0, false)`；调用者拿不到具体 `PdHelperError`。这与 Go 包边界的 fail-closed 行为一致。
- PD 阶段失败不会降级执行 SQL；只有成功取得 Region 统计且 Region 数不超过 2 才走 COUNT。这避免在无法确认存储能力时意外发起全表计数。
- 非直观兼容行为是“失败也缓存”：第一次失败返回 `(0.0, false)`，同键在 TTL 内再次命中返回 `(0.0, true)`。原因是缓存只保存数值，测试 `failed_lookup_is_cached_exactly_like_go` 明确锁定该语义。
- `count > 2` 是严格阈值；`count == 2` 仍运行 COUNT。大表路径对 `storage_keys` 不做非负校验。
- 数据库、表和分区名均经过 `quote_identifier`，内部反引号加倍；空分区名不生成 partition 子句。值没有作为 SQL 字面量拼入。
- mutex poison 不会导致后续访问 panic，而是继续使用被 poison 的内部值；清理线程的 `wait_timeout_while` 也采用相同恢复方式。清理线程自身 panic 时，`Stop` 忽略 `join` 错误。
- `Instant::now() + ttl` 对极端大 duration 存在标准库时间溢出风险；正常默认和测试 TTL 不触及该边界。

## 并发与资源生命周期

缓存访问由 `PDHelperInner.cache` 串行化，适合多个线程共享同一 helper；`SessionContext` 被约束为 `Send + Sync`，上下文值要求 `Clone`。查询在释放缓存锁后访问 PD/SQL，避免慢 I/O 长时间阻塞其它缓存命中；写回时再短暂加锁，因此并发同键 miss 可能重复查询，代码没有 singleflight 去重。

清理线程不强持有 `PDHelperInner` 作为永久所有权，而是每轮从 `Weak` 临时升级。`Stop` 在持有 cleanup 锁时取出句柄，然后释放锁再 `join`，避免与线程检查停止位互锁。条件变量让停止请求可立即唤醒等待，不必等待完整 TTL。`Drop` 提供最终兜底，但明确调用 `Stop` 更能表达生命周期。

包级 `GLOBAL_PD_HELPER_ONCE` 使多个实例共享启动权：只有进程中第一个 `Start` 的实例拥有 worker，其余实例即使有独立缓存也没有后台清扫，只能在读取时惰性删除过期项或靠 LRU 驱逐。该限制来自 Go 兼容语义，测试 `cleanup_worker_start_and_stop_are_idempotent` 已验证；新增实例化方式时必须评估这一全局耦合。

## 与 Go 版本的对应关系

- `GlobalPDHelper`、`defaultPDHelper`、`PDHelper::Start/Stop`、缓存入口和无缓存入口逐项对应 `pd.go` 同名符号；Rust 另提供 snake_case 核心函数和 Go 风格转发别名。
- Go `ttlcache.Cache[string,float64]` 对应 Rust `Mutex<LruCache<String, CacheEntry>>` 加自管 TTL；默认 TTL、容量、命中返回 `true`、失败结果也缓存的行为保持一致。
- Go `sync.Once` 对应 `AtomicBool::swap`。效果上均只允许包级首次启动；Rust 的清理线程、条件变量与显式句柄承担 Go `WaitGroupWrapper` 和 ttlcache 后台循环的生命周期职责。
- Go 对 `sessionctx.Context`、`helper.Storage` 和 restricted executor 的直接调用，在 Rust 中被压缩为 `SessionContext` trait；`kv.WithInternalSourceType(...StatsForegroundPriority)` 对应 `InternalSourceContext::with_internal_stats_foreground`。
- Go 使用 `sqlescape.MustFormatSQL(..., %n)`；Rust `quote_identifier` 实现本任务涉及的反引号标识符规则。`migration_aster_unit_test.rs` 用包含反引号和空格的名字核对生成文本。
- Go failpoint 会原地覆盖 `err` 和 Region 统计；Rust 包装函数直接返回固定 `RegionStats`，对后续分支的可观察效果相同。
- Go 测试 `pd_test.go::TestTTLCache` 只覆盖命中、容量驱逐和过期序列；Rust `pd_test.rs::test_ttl_cache` 对齐该序列，`migration_aster_unit_test.rs` 进一步覆盖大小表分支、转义、失败、失败缓存和 worker 生命周期。
- 关键迁移差异是生产接线：Go helper 被 executor 实际调用；当前 Rust crate 只有测试调用，且 Rust builder 的同名方法通过 `AnalyzeBuilderDependencies` 获取预计算值。

## 扩展指南

- 接入 Rust executor 主链时，应实现真实的 `InternalSourceContext` 与 `SessionContext` 适配器，并在拥有明确启动/停止生命周期的位置使用 `GlobalPDHelper`；不要仅凭同名 builder 方法假设接线已经完成。
- 修改大小表阈值或回退策略时，主要入口是 `get_approximate_table_count_from_storage`；必须同步 `migration_aster_unit_test.rs` 的大表、小表、PD 失败和 SQL 失败用例，并与 `pd.go` 保持逻辑一致。
- 修改 SQL 生成时，集中调整 `count_sql`/`quote_identifier`，补充数据库、表、分区各自含反引号及无分区场景；不要把测试塞回 `pd.rs`，继续使用同目录独立测试文件。
- 修改缓存键需评估现有键兼容、下划线碰撞与缓存迁移；修改缓存值结构时尤其要决定是否继续保留“失败命中后 `has_pd = true`”的 Go 行为。
- 修改清理策略时，关注全局 once、停止后不可重启、非首个实例无 worker、并发同键重复查询和锁持有时间；对应更新 `cleanup_worker_start_and_stop_are_idempotent` 及 TTL/LRU 测试。
- 增加可传播错误属于公共契约变化：当前所有错误都折叠为布尔值。若要改变，需同时审查 Go 调用者、Rust builder 依赖接口和信息模式路径，而不是只改本文件。
- 性能上，大表路径应维持一次 PD 查询；小表 COUNT 虽被限制在不超过两个 Region 的判断下，仍可能昂贵。任何阈值、TTL、容量或并发去重改动都应配套基准或调用次数测试。

## 验证依据

本说明基于以下直接证据：

- RustCodeGraph 状态：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录中的 `pd.rs`、`lib.rs`、两份 Rust 测试以及 Go 对照均在索引中。
- 目标实现：`pkg/executor/internal/pdhelper/pd.rs` 的 `PDHelper`、`get_approximate_table_count_from_storage`、`cleanup_loop`、`count_sql`、failpoint 和全局实例。
- crate 装配：`pkg/executor/internal/pdhelper/lib.rs` 与 `Cargo.toml`；workspace 成员及 façade 别名来自根 `Cargo.toml`。
- Go 对照：`pkg/executor/internal/pdhelper/pd.go`、`pd_test.go`，以及生产调用位置 `pkg/executor/select.go`、`builder.go`、`infoschema_reader.go`。
- Rust 测试：`pkg/executor/internal/pdhelper/pd_test.rs` 和 `migration_aster_unit_test.rs`；`main_test.rs` 仅提供包级测试配置与 failpoint 场景，不改变生产算法。
- Rust 主链核对：`pkg/executor/builder.rs::getApproximateTableCountFromStorage` 从 builder dependency 读取 `approximate_storage_count`；全仓 Rust 搜索未找到本 crate 的生产调用或真实 trait 适配器。
- RustCodeGraph 查询覆盖 `status`、目标目录 `files`、目标/Go/测试/模块文件 `node`、主要入口 `query` 以及入口的 `callers`/`callees`；精确图查询没有返回跨文件生产调用边，因此使用源码搜索确认当前接线边界。

这是纯文档分析，按任务约束未运行 Cargo。交付结构检查要求本文恰有十一个规定的二级标题；内容人工复核重点是区分 Go 已接线主链与 Rust 当前仅 crate 内测试可达的事实。
