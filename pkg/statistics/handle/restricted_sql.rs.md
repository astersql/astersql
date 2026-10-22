# `pkg/statistics/handle/restricted_sql.rs`

## 文件定位

提供 `KvStatsStore` 高层 API，并实现 `StatsSession` 与 `storage::SqlStore`，供 lockstats、统计 GC 和 Domain 调用。

## 核心职责

1. 用 `ROOT = __astersql/stats/v1/` 下的独立前缀保存 meta、锁表 delta、直方图、FM Sketch、bucket、历史和系统变量。
2. 提供 `KvStatsStore` 高层 API，并实现 `StatsSession` 与 `storage::SqlStore`，供 lockstats、统计 GC 和 Domain 调用。
3. 在单个 KV 事务内解释有限的 SELECT/DML；识别范围之外的表、列、谓词或语句必须返回 `StatsError`，不能静默当作成功。
4. 维护 KV 与内存缓存的一致性：事务提交后才应用 `pending_cache_meta` / `removed_tables`；失败时回滚，缓存不变。
5. 承担 schema 对账、lite 初始化、整表统计替换、批量 flush、锁表 delta、历史清理和 GC 时间戳持久化。

它当前确有生产接线，不是门面或桩。不过 SQL 支持刻意限定为仓库内统计路径使用的模板；例如部分 Go 表清理语句（`stats_top_n`、`column_stats_usage`、`analyze_options`）当前是成功空操作，不能据此推断对应数据已在本 KV 布局中实现。

## 主要符号

- `StatsKvStorage`：存储抽象，暴露 `current_version`、指定版本 `snapshot` 和 `begin`。本文件实际写路径使用 `begin`，`storage::SqlStore::start_ts` 使用 `current_version`；`snapshot` 是对外契约的一部分。
- `StatsMetaRecord`、`StatsLockedRecord`、`StatsHistogramRecord`：分别表示 `stats_meta`、`stats_table_locked` 和直方图身份。`StatsHistogramPayload` 是内部载荷，兼容旧的一字节 analyzed 格式。
- `KvStatsStore<B>`：线程安全入口，持有 KV、`Arc<Mutex<Handle<B>>>`、全局 `operation_lock` 和两个故障注入原子量。公开方法覆盖受限 SQL、字符串查询、持久化、schema/lite 对账、批量 flush 和未分析直方图身份写入。
- `KvRestrictedExecutor<'a, B>`：绑定一个可变 KV 事务；`pending_meta` 提供事务内 read-your-writes，`pending_cache_meta` 与 `removed_tables` 延迟到提交后更新 Handle。
- `RestrictedSQLExecutor for KvRestrictedExecutor`：为 lockstats 提供 `ExecRestrictedSQL`、`StartTS`、锁 ID 扫描、插锁、版本更新、delta 合并和删锁。
- `storage::SqlStore for KvStatsStore`：为 GC 提供版本范围扫描、直方图身份、表/直方图/历史删除和 GC 时间戳读写。
- 键与编码函数：`record_key`、`histogram_key`、`fm_sketch_key`、`bucket_key`、`history_key` 负责有序大端键；相应 `encode_*` / `decode_*` 校验固定长度或变长边界。
- SQL 子集函数：`normalize`、`sql_table`、`filter_rows`、`condition_matches`、`project_rows` 处理规范化、表分派、AND 谓词、排序/限制及 `count(*)`、`hex()`、`truncate()` 投影。

## 执行流程

常规入口 `KvStatsStore::execute` 先取得统计查询超时配置，再持有 `operation_lock` 并进入 `with_executor`。后者开启事务，以 `max(transaction.StartTS(), current_stats_timestamp())` 作为统计版本，构造 `KvRestrictedExecutor`。SELECT 经 `ExecRestrictedSQL -> query` 按表扫描 KV，再执行 WHERE、ORDER BY、LIMIT 和投影；DML 经 `mutate` 匹配固定模板并修改事务。闭包成功时 `commit` 先提交 KV、再更新 Handle；失败时调用 `Rollback`。

`system_sql_in_transaction` 是例外：只允许 `mysql.tidb` 的 SELECT/INSERT/UPDATE/DELETE，使用调用方已有事务，不提交也不回滚，因而 bootstrap 调用者拥有 SQL 与版本的原子边界。

持久化整表统计时，`persist_table_stats_result` 先在未持有 `operation_lock` 时克隆 Handle profile，避免与提交路径形成锁顺序反转；随后 `replace_table_stats` 重写 meta、histogram、FM Sketch 与 bucket。`apply_stats_flush_batch` 在一个事务里先应用锁表 delta、再替换未锁表统计；任一步失败会让整批回滚并保留 Domain 的待重试批次。

schema 路径分两类：`reconcile_tables_impl` 会删除 schema 中已不存在的 histogram 键并 merge/replace 缓存；`init_tables_lite` 是只读加载，仅为已有 meta 的表重建轻量 profile，缺失 meta 继续保持 pseudo。`persist_unanalyzed_histograms` 只建立零值身份，不创建 `stats_meta`。

## 数据与状态

所有键位于 `__astersql/stats/v1/`：单行 meta/locked 使用 `table_id` 大端尾缀；histogram/FM/bucket 再附加 `is_index`、`hist_id`，bucket 还附加 `bucket_id`；history 追加 version、sequence、create_time。大端编码保证前缀扫描和数字顺序一致。

meta 值固定 32 字节（version、count、modify_count、last_histogram_version），锁表值固定 16 字节。完整 histogram payload 为 49 字节，同时兼容长度不超过 1 的旧 analyzed 标记。bucket 值保存三个 i64、上下界长度和原始字节；读取必须精确消费全部字节。FM Sketch 与 bucket 边界在 SQL 文本面以大写十六进制暴露，避免二进制损坏。

事务状态由 `pending_meta`、`pending_cache_meta`、`removed_tables` 隔离。`pending_meta` 使同一事务后续读取看到刚写的 meta；只有需要刷新 Handle 的更新才进入 `pending_cache_meta`。计数使用饱和加法，meta count 被钳制为非负；锁表 delta 自身允许带符号累加。

## 依赖与调用关系

上游主链是 `Domain / Session -> KvStatsStore -> KvRestrictedExecutor -> astersql_kv::Transaction`。RustCodeGraph 确认 `with_executor` 被常规执行、持久化、锁表、对账、GC 和 `StatsSession::WithSession` 等入口调用；其被调用边包含 `StatsKvStorage::begin`、`Transaction::StartTS`、`KvRestrictedExecutor::commit`、失败时 `Rollback`。

直接生产调用证据包括：[`../../domain/domain.rs`](../../domain/domain.rs) 的构造、`reconcile_tables`、`apply_stats_flush_batch`、`persist_unanalyzed_histograms` 与 `restricted_system_sql_in_transaction`；[`../../session/runtime/admin.rs`](../../session/runtime/admin.rs) 和 [`../../session/runtime/statistics.rs`](../../session/runtime/statistics.rs) 使用借用事务或直方图身份接口。`StatsSession` 对接 [`lockstats`](lockstats/)，`storage::SqlStore` 对接 [`storage`](storage/) 的 GC/读路径。

SQL 数据类型与错误来自 `lockstats::{SqlRow, SqlValue, StatsError}`；统计缓存实体来自 `TableStats`、`ColumnStats`、`IndexStats`、`Handle`。超时入口来自 `astersql-statistics-handle-util::ExecRowsTimeout`。

## 错误处理与边界

KV 错误统一经 `stats_error` 转为 `StatsError`；NotFound 由 `get` 映射为 `None`。损坏的 key/value 长度、UTF-8 系统变量、数字谓词、投影括号、未知列/表/谓词和未支持 SQL 都显式报错。`mysql.tidb` 普通 INSERT 会模拟唯一键冲突；INSERT IGNORE 保留旧值，ON DUPLICATE KEY UPDATE 才覆盖。

`with_executor` 的错误边界是事务级：闭包或提交前操作失败会回滚；KV `Commit` 成功后才触碰 Handle。锁中毒使用 `expect`，视为进程内不变量破坏而 panic。迭代扫描会显式 `Close`，但当前实现会先把前缀全部物化到 `Vec`，大前缀下有内存和延迟风险。

受限解析器不是完整 MySQL parser：参数 SELECT 通过逐个替换 `%?` 渲染；VALUES 和 SET 仅支持当前模板需要的简单字面量，元组拆分不处理任意嵌套/复杂表达式。扩展调用方前必须先扩展并测试解析边界，不能把任意 SQL 直接传入。

## 并发与资源生命周期

`KvStatsStore` 以 `operation_lock: Mutex<()>` 串行化所有受限 SQL和对账/GC写路径；底层存储与 Handle 都通过 `Arc` 共享。原子故障开关用 Acquire/Release 或 AcqRel 一次性消费，只服务测试。

锁顺序是重要不变量：一般提交路径先持有 `operation_lock`，KV 提交后再锁 Handle；`persist_table_stats_result` 特意先短暂锁 Handle 克隆数据、释放后才取 `operation_lock`，避免双锁重叠和锁反转。`init_tables_lite` 在事务闭包结束后才锁 Handle。事务由 `with_executor` 负责提交/回滚；借用事务入口则明确把生命周期交还调用方。

`reconcile_tables_impl` 直接创建事务并设置 `AllowedOnAlmostFull`，提交后才发布缓存。前缀迭代器在 `scan` 中关闭；本文件不创建后台线程或 channel，异步/重试生命周期位于 Domain 的 stats outbox 和上层 worker。

## 与 Go 版本的对应关系

仓库没有同名 `restricted_sql.go`；该 Rust 文件把 Go 中依赖真实内部 SQL session 与 `mysql.stats_*` 表的多处分散语义集中成 KV 适配器。

差异是 Rust 目前用私有 KV layout 模拟这些系统表，而 Go 通过完整 SQL/事务/session 执行；Rust 仅实现已接线模板，且 TopN 等部分语句为空操作。

## 扩展指南

新增系统表或 SQL 模板时，应同时修改：表前缀与键编码、`query`/`mutate` 分派、行列映射、`project_rows` 的 `*` 列序，以及删除/GC 路径；若数据进入 Handle，还必须明确它属于事务内可见状态还是提交后缓存状态。新增编码字段需保留版本/长度兼容，并为损坏输入添加拒绝测试。

新增公开存储能力优先接入 `KvStatsStore`；lock/unlock 行为接入 `RestrictedSQLExecutor`；GC 行为接入 `storage::SqlStore`。不要绕过 `operation_lock` 或在 KV 提交前更新 Handle。若必须同时接触 Handle 与 operation lock，应沿用“不重叠持锁”或既有的 operation-lock-before-Handle 顺序。

测试必须放在独立 Rust 测试文件。最直接的集成面是 [`../../testkit/mockstore_domain_stats_test.rs`](../../testkit/mockstore_domain_stats_test.rs)；GC trait 算法位于 [`storage/gc_test.rs`](storage/gc_test.rs)，异步损坏 bucket 读取位于 [`../asyncload/async_load_test.rs`](../asyncload/async_load_test.rs)，lockstats 语义位于 [`lockstats/lock_stats_test.rs`](lockstats/lock_stats_test.rs) 与 [`lockstats/unlock_stats_test.rs`](lockstats/unlock_stats_test.rs)。涉及 Go 对齐时同步核对对应 `.go` 测试，而不是缩减 Rust 行为。

兼容风险主要是键格式/载荷升级和 SQL 模板漂移；正确性风险是提交后缓存同步、锁表 delta 重复应用或丢失；性能风险是全前缀物化扫描、整表 delete-and-rewrite 和全局 operation lock 串行化。
