# `pkg/infoschema/isvalidator/validator.rs`

## 文件定位

本文件是 `astersql-infoschema-isvalidator` crate 的核心实现，crate 入口 `pkg/infoschema/isvalidator/lib.rs` 将它作为 `validator` 模块导出。它位于 InfoSchema 同步与事务提交之间：`pkg/infoschema/issyncer/syncer.rs` 在 reload、PD 重连和 schema 版本变化时维护校验器状态，`pkg/session/runtime/schema_validation.rs::SharedValidator` 则在提交前把事务时间戳、起始 schema 版本和相关物理表交给 `Validator::check`。

该文件解决的是“事务仍能否安全使用开始时看到的 schema”这一问题。它不加载 InfoSchema，也不执行 DDL；它只保存最近的 schema 变更摘要和 lease 窗口，并输出 `ResultSucc`、`ResultFail` 或 `ResultUnknown`。接口契约来自 `pkg/infoschema/validatorapi/interface.rs::Validator`，具体数据来源和生命周期接线位于 `pkg/infoschema/issyncer/syncer.rs`。

## 核心职责

1. `Validator::update` 用本次 lease 授予的 TSO 刷新有效期，记录最新 schema 版本，并在版本变化时把 `RelatedSchemaChange` 转为 `DeltaSchemaInfo` 入队。
2. `Validator::check` 在事务提交前进行三态判定：明确安全返回 `ResultSucc`，明确看到相关 schema 冲突或重连前旧版本返回 `ResultFail`，本地信息不可用或 lease 超窗返回 `ResultUnknown`。
3. `Validator::stop`、`restart` 和 `reset` 管理 PD/同步器异常恢复期间的服务状态；`restart_schema_ver` 阻止重连前开始的旧写事务继续提交。
4. `enqueue_locked`、`contain_in` 和 `find_newer_deltas` 维护按 schema 版本升序排列的有界 delta 历史，在不扩大误判风险的前提下压缩相邻摘要。
5. `VALIDATOR_METRICS` 与日志记录 stop/restart/reset、缓存空/缺失和 lease 到期时间；`snapshot` 与 `validator_metrics_snapshot` 提供只读诊断视图。

## 主要符号

- `RelatedSchemaChange { phy_tbl_ids, action_types }`：一次 schema 变化涉及的物理表 ID 与 DDL action 编码。两数组必须按下标一一对应；`is_related_tables_changed_locked` 和 `contain_in` 都依赖该不变量。
- `DeltaSchemaInfo { schema_version, related_ids, related_actions }`：写入有界历史队列的版本摘要。它保存拥有该变化的 schema 版本，而不是完整 InfoSchema。
- `ValidatorState`：锁内状态，包含 `is_started`、`latest_schema_ver`、`restart_schema_ver`、`latest_schema_expire` 和升序 `delta_schema_infos`。
- `ValidatorSnapshot`：`Validator::snapshot` 克隆出的完整一致状态，只用于诊断与聚焦测试，不参与提交判断。
- `Validator { lease, state }`：公开实现。不可变的 lease 放在锁外，可变状态由单个 `RwLock<ValidatorState>` 保护。
- `ValidatorMetricsSnapshot`、`ValidatorMetrics`、`VALIDATOR_METRICS`：进程级原子指标镜像；读写均使用 `Ordering::Relaxed`，只保证独立计数/数值的原子性，不提供跨字段一致快照。
- `new` / `Validator::new`：按 lease 构造并立即启动。初始版本和重连下界为 0，过期时间为 `UNIX_EPOCH`，delta 容量以 `vardef::DefTiDBMaxDeltaSchemaCount` 预分配。
- `time_from_ts`、`system_time_to_ts`：在 TiKV TSO 与 `SystemTime` 间转换；低 18 位是逻辑时钟，物理部分以毫秒计。
- `unix_seconds`：把墙钟时间转成 Unix 秒，并为 epoch 之前的亚秒时间保持 Go `time.Time.Unix` 的向负无穷取整语义。
- `stop` / `restart` / `reset`：分别停止并清空、带重连版本下界恢复、恢复初始状态。`restart` 不清理队列或最新版本；正常接线中它位于 stop 和成功 reload 之后。
- `update`：唯一正常生产写入口；停机时忽略更新，运行时刷新最新版本与 lease，到版本变化时调用 `enqueue_locked`。
- `check`：事务侧主入口，返回 `(Option<RelatedSchemaChange>, Result)`。当前实现所有分支的 change 都为 `None`，但保留接口形状以兼容上层契约。
- `is_related_tables_changed_locked`：保守判断某版本之后相关表是否变化，特殊 ID `-1` 表示关注全部表。
- `enqueue_locked`：读取运行时 `vardef::GetMaxDeltaSchemaCount()`；配置不大于 0 时禁用缓存，超限时淘汰队首。
- `contain_in`：若旧摘要中的每一个 `(table_id, action)` 对都能在新摘要中找到，则允许用新摘要替换队尾；它有意保持 Go 的嵌套循环和重复项处理方式。
- `impl ValidatorApi for Validator`：以 Go 风格方法名把公开 trait 调用转发到上述 Rust 风格方法。

## 执行流程

### 构造与同步

1. `pkg/session/runtime/session_factory.rs` 通过 crate 的 `new(lease)` 创建 `Arc<Validator>`，同步器与会话提交路径共享它。
2. `pkg/infoschema/issyncer/syncer.rs::ReloadWithContext` 加载新 InfoSchema；当 load 没有可用 change 时先 `Reset`，随后总是以 loader 返回的时间戳、旧版本、新版本和可选 change 调用 `Update`。
3. `update` 先取得写锁；若已 stop，记录日志后返回。否则写入 `latest_schema_ver`，计算 `time_from_ts(lease_grant_ts) + lease - 1ms` 作为 `latest_schema_expire`。
4. 仅当 `curr_ver != old_ver` 时构造 delta。`enqueue_locked` 可压缩队尾、追加新项，并在超过当前最大容量时淘汰最旧项。
5. `issyncer::SyncLoop` 发现 schema version syncer 断开时调用 `Stop`，等待 PD/syncer 恢复并 reload，最后以已加载 InfoSchema 版本调用 `Restart`。

### 提交前校验

`Validator::check(txn_ts, schema_ver, related_ids, need_check_schema_by_delta)` 在一个读锁周期内按以下顺序判定：

1. `is_started == false`：本地状态不可用，返回 `ResultUnknown`。
2. `schema_ver < restart_schema_ver`：事务使用的是重连前版本，确定不安全，返回 `ResultFail`。
3. `schema_ver < latest_schema_ver`：schema 已推进。
   - `related_ids == None` 保留 Go 的 nil slice 语义，表示只比较版本；直接返回 `ResultFail`。
   - `Some(&[])` 与 `None` 不同，代表非 nil 空集合，例如仅操作临时表的事务。
   - 当 `need_check_schema_by_delta` 为真，或运行时 MDL 已关闭时，扫描更新版本的 delta；看到相关变化或无法证明历史完整便返回 `ResultFail`。
   - MDL 开启且调用方明确无需 delta 校验，或扫描确认无相关表变化时，返回 `ResultSucc`。
4. schema 未落后时，把 `txn_ts` 转成墙钟。若事务时间晚于 `latest_schema_expire`，返回 `ResultUnknown`；否则返回 `ResultSucc`。

`pkg/session/runtime/schema_validation.rs::SharedValidator` 将三态映射为 domain 层的 `SchemaCheckResult`。`pkg/domain/schema_checker.rs::SchemaChecker` 对 `Unknown` 默认最多重试 10 次、每次间隔 500ms，最终转换为 InfoSchema expired；`Fail` 则转换为 InfoSchema changed。

### delta 扫描与压缩

`find_newer_deltas` 从队尾向前寻找 `schema_version > curr_ver` 的连续后缀。若队列为空，或整个保留队列都比事务版本新，就无法排除更早 delta 已被淘汰，函数保守地认为相关表已变化。否则逐项比较 `related_ids` 与事务表集合，匹配具体 ID 或事务侧通配符 `-1`；action 小于 64 时形成位标记，action 不小于 64 时按 Go 左移行为记为 0，但表匹配本身仍使结果为“已变化”。

入队压缩只比较当前队尾和新 delta，且永不合并队首，因为保留最旧版本能覆盖更大的可验证历史窗口。队尾的每个 `(表, action)` 对被新 delta 包含时，用新版本摘要替换队尾；否则追加。长度超过上限时删除队首一项。

## 数据与状态

- `lease` 在构造后不变；实际有效期每次由授予 TSO 重新计算。减去 1ms 与 Go `leaseGrantTime.Add(lease - time.Millisecond)` 对齐。
- `latest_schema_ver` 是最近一次成功 `update` 的当前版本；即使版本没有变化也会更新 lease。
- `restart_schema_ver` 是 PD 重连并 reload 后接受的最低版本，用于保护重连窗口内的旧写事务。`reset` 清零，`restart` 设置，`stop` 不清零它。
- `delta_schema_infos` 必须按版本升序写入；本文件不对乱序调用重新排序，正确顺序由 InfoSchema reload 链保证。
- delta 的容量不是固定构造参数：构造仅按默认值预分配，实际每次入队读取 `GetMaxDeltaSchemaCount()`，因此测试或配置变化可立即改变行为。
- `Option<&[i64]>` 刻意保存 Go `nil` 与非 nil 空 slice 的差异；上层适配器若一律传 `Some`，将无法表达“只检查版本”的 nil 路径。
- 当前返回值中的 `Option<RelatedSchemaChange>` 总是 `None`；类型仍按 `validatorapi::Validator` 保留，调用方不得假设未来永远没有负载。

## 依赖与调用关系

- crate 边界由 `pkg/infoschema/isvalidator/Cargo.toml` 定义：生产依赖只有 `astersql-util-logutil`、`astersql-infoschema-validatorapi` 和 `astersql-sessionctx-vardef`；`crossbeam-channel` 仅为独立测试模拟 lease server 使用。
- `logutil` 提供后台 logger 和结构化字段；`vardef` 提供默认/运行时 delta 上限和 MDL 开关；`validatorapi` 提供三态 `Result` 与 trait。
- 上游状态生产者是 `pkg/infoschema/issyncer/syncer.rs`：reload 调用 `Reset`/`Update`，PD/version-syncer 恢复流程调用 `Stop`/`Restart`，并把 loader 的 `RelatedSchemaChange` action code 转成这里的 `u64`。
- 上游事务消费者是 `pkg/session/runtime/schema_validation.rs`，它以 `Arc` 共享校验器并构造 KV 层 `TransactionSchemaChecker`；domain 层 `SchemaChecker` 负责 Unknown 的重试策略和用户可见错误映射。
- `ValidatorApi` 的 Go 风格实现是另一条兼容入口；Rust 同步器还实现了自身的 `issyncer::SchemaValidator` 适配 trait，二者最终都委托给本文件的小写方法。
- `validator.rs` 本身不访问 PD、KV、InfoSchema 缓存或事务对象，也不启动后台线程；所有外部 I/O 和重试都在调用者中完成。

## 错误处理与边界

- 业务不确定性不使用 Rust `Result` 错误，而由三态枚举表达：停机或 lease 证据不足是 `Unknown`，明确旧版本/相关变化是 `Fail`。
- delta 为空、事务版本早于所有保留 delta 都采用 fail-closed 策略，避免因缓存缺失错误放行事务。
- `related_ids` 与 `related_actions` 长度不等会在扫描或 `contain_in` 中 `expect` panic。这是调用者必须维持的数据契约，当前代码没有把畸形输入降级为 `Fail`。
- `system_time_to_ts` 对 epoch 前时间、毫秒超出 `u64`、左移溢出 panic；`update` 对 lease 到期时间加减越界也 panic。这些函数假设输入来自正常 TiKV TSO/配置域。
- 零 lease 在普通 Rust 构造中允许，以对齐未启用 Go `intest.Assert` 的默认构建；但 `lease - 1ms` 的 Go 表达在 Rust 中通过先加 lease 再减 1ms 实现，若授予时间靠近 epoch 或系统时间范围边界可能 panic。
- action 编码大于等于 64 时位图值为 0，以对齐 Go 的无类型常量左移后转 `uint64` 的结果；冲突判断仍由 map 是否非空决定，因此不会因此漏掉已经匹配的表。
- `RwLock` poisoning 被 `read_state`/`write_state` 显式恢复为内层 guard，避免一个持锁 panic 让后续访问持续 panic；这不等于回滚或验证被中断写入的状态。

## 并发与资源生命周期

`Validator` 的所有复合可变状态都放在同一个 `std::sync::RwLock` 中。`check`、`snapshot`、`is_started`、`is_lease_expired` 使用读锁；`update`、`stop`、`restart`、`reset` 和测试入口 `enqueue` 使用写锁。因此一次检查看到的是同一状态版本，不会把某次 update 的版本与另一次 update 的 lease 混用。

`check` 在扫描整个 delta 队列期间持续持有读锁，保证队列不会被更新或淘汰；代价是长表 ID 列表与较大 delta 窗口会延迟 reload 的写锁。当前扫描为 delta × delta-table × transaction-table 的嵌套循环，容量由 `GetMaxDeltaSchemaCount` 限制。

指标采用独立原子变量和 Relaxed 顺序，适合观测计数，不参与状态同步。日志在持锁路径内调用，慢日志后端可能延长锁持有时间。对象本身没有显式 `Drop`、线程、channel 或异步任务；生命周期由外层 `Arc`、session factory 与 syncer 管理。测试文件 `validator_test.rs` 中的 channel/线程只是模拟远端 lease/TSO 服务，不属于生产对象。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/infoschema/isvalidator/validator.go`，Rust 基本保留其字段、判定顺序和日志意图：

- Go `validator` / `deltaSchemaInfo` 对应 Rust `Validator` / `DeltaSchemaInfo`；`sync.RWMutex` 对应 `RwLock<ValidatorState>`。
- Go `New` 返回 `validatorapi.Validator`，Rust同时提供 `new -> Box<Validator>` 和 `Validator::new`，并通过 trait impl 保留 Go 风格方法名。
- Go `oracle.GetTimeFromTS` 对应 `time_from_ts`；Rust额外提供反向转换和可测试的 Unix 秒转换。
- Go slice 的 nil/empty 差异由 Rust `Option<&[i64]>` 明确编码，这是 `check` 行为正确的关键兼容点。
- Go `metrics` collector 在 Rust 中变为本 crate 的原子镜像，需由上层集成层导出；因此指标存储位置不同，判定行为相同。
- Rust 增加 `ValidatorSnapshot` 和 `ValidatorMetricsSnapshot` 供诊断/测试，并对 poisoned lock 继续取内层状态；Go 没有这两个公开快照层。
- Go 默认构建中的 lease 正值断言只在 `intest`/`enableassert` 生效；Rust构造不做无条件断言，`migration_aster_unit_test.rs::zero_lease_is_accepted_without_intest_assertions` 固化了这一差异。
- Go 与 Rust 当前都返回空的 related-change 负载，但共同保留接口返回槽位。

语义证据主要来自 `pkg/infoschema/isvalidator/validator_test.go`、Rust 端口 `validator_test.rs` 和补充迁移测试 `migration_aster_unit_test.rs`。这些测试覆盖生命周期、lease 过期、nil/empty、MDL 切换、缓存缺失、队列压缩/淘汰、action 差异以及 epoch 前 Unix 秒行为。

## 扩展指南

- 新增判定分支应优先放在 `Validator::check`，并保持“停止 → 重连下界 → schema 落后/delta → lease”顺序；调整顺序会改变 `Fail` 与 `Unknown` 的优先级。
- 修改 delta 数据结构时必须同步 `RelatedSchemaChange`、`DeltaSchemaInfo`、`enqueue_locked`、`contain_in`、扫描逻辑，以及 `issyncer.rs` 的 action 转换适配；同时保持 ID/action 等长不变量。
- 修改压缩算法或容量规则时，要保留队首覆盖窗口的安全理由，并评估 `is_related_tables_changed_locked` 的 cache-miss 判定；不能只以减少内存为目标合并历史。
- 扩展 MDL 交互时需要同时核对事务开始时记录的 `need_check_schema_by_delta` 和运行时 `vardef::IsMDLEnabled()`，以覆盖事务期间开关切换。
- 若要返回实际 `RelatedSchemaChange`，需同步 `validatorapi`、`SharedValidator` 的映射、domain 错误载荷和 Go 对照；当前测试大多只断言三态，必须新增负载断言。
- 性能优化应关注读锁内三重扫描、队首 `Vec::remove(0)` 的移动成本和锁内日志，但任何索引化/容器替换都要保持版本顺序、重复项和 `-1` 通配语义。
- 测试必须继续放在独立文件。Go 对照回归更新 `validator_test.go`；Rust Go 风格端口更新 `validator_test.rs`；Rust 特有边界与迁移一致性更新 `migration_aster_unit_test.rs`。不要把测试内嵌到 `validator.rs`。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件，目标目录中的 `validator.rs`、`validator_test.rs`、`migration_aster_unit_test.rs`、`lib.rs` 及 Go 对照均已索引。
- 目标实现：`pkg/infoschema/isvalidator/validator.rs`，重点核对了 `Validator::{new, stop, restart, reset, update, check, is_related_tables_changed_locked, find_newer_deltas, enqueue_locked}`、`contain_in`、时间转换与 trait impl。
- crate/API：`pkg/infoschema/isvalidator/Cargo.toml`、`pkg/infoschema/isvalidator/lib.rs`、`pkg/infoschema/validatorapi/interface.rs`。
- 生产调用链：`pkg/infoschema/issyncer/syncer.rs::{SyncLoop, ReloadWithContext}`、该文件对 `astersql_infoschema_isvalidator::Validator` 的适配实现、`pkg/session/runtime/schema_validation.rs::{SharedValidator, checker}`、`pkg/domain/schema_checker.rs::SchemaChecker`。
- Go 对照：`pkg/infoschema/isvalidator/validator.go::{New, Update, Check, enqueue, containIn}`。
- 测试证据：`pkg/infoschema/isvalidator/validator_test.go`、`pkg/infoschema/isvalidator/validator_test.rs`、`pkg/infoschema/isvalidator/migration_aster_unit_test.rs`。其中独立 Rust 测试验证后台 lease 模拟、生命周期与队列行为，迁移测试补充 MDL、nil/empty、零 lease、时间边界和 trait 契约。
- 本任务为纯文档分析，按任务要求未运行 Cargo。结构验证要求本文恰有十一个规定的二级标题；链接、符号名和行为陈述均以上述源码及 RustCodeGraph 节点/调用流人工复核。
