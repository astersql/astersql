# `pkg/util/deadlockhistory/deadlock_history.rs`

## 文件定位

本文件是 `astersql-util-deadlockhistory` crate 的业务实现；对应源码为 [`deadlock_history.rs`](./deadlock_history.rs)，crate 入口 [`lib.rs`](./lib.rs) 通过 `pub mod deadlock_history` 声明模块并以 `pub use deadlock_history::*` 公开全部 API。它承担两类职责：把 TiKV 死锁错误转换成可诊断的 `DeadlockRecord`，以及用有界环形缓冲保存最近的记录并投影为 `INFORMATION_SCHEMA.DEADLOCKS` / `CLUSTER_DEADLOCKS` 所需的 `Datum` 列值（`ErrDeadlockToDeadlockRecord`、`DeadlockHistory`、`DeadlockRecord::ToDatum`）。

`pkg/util/deadlockhistory/Cargo.toml` 将其声明为独立库 crate；直接运行依赖包括 `chrono`/`chrono-tz`、`hex`、`log`、`protobuf`，以及本仓库的 `mysql`、`resourcegrouptag` 和 `types` crate。根 `pkg/lib.rs` 又通过 `facade_util_deadlockhistory` 将该 crate 纳入总 facade。当前 Rust 生产源码尚未找到使用本文件 `GlobalDeadlockHistory`、`ErrDeadlockToDeadlockRecord` 或其他公开 API 的真实调用点：`cmd/tidb-server/main.rs` 中同名的 `deadlockhistory` 实际指向 `cmd/tidb-server/stubs.rs` 的容量控制桩，不是本 crate。相对地，Go 版本已经从悲观锁错误采集、启动/HTTP 调容到 information schema 查询形成完整主链；因此本文件目前是已实现且有独立测试覆盖、但生产接线仍不完整的迁移模块。

## 核心职责

- `ErrDeadlockToDeadlockRecord` 遍历 protobuf `Deadlock.wait_chain`，保留事务关系与原始键，尝试从 resource group tag 解出 SQL digest，并记录转换时刻及 `IsRetryable`。
- `DeadlockHistory` 维护固定容量的最近记录。未满时追加，满时覆盖最旧记录；`GetAll` 始终按从旧到新的逻辑顺序返回快照；`Resize` 扩容时保留全部现有项，缩容时仅保留最新项。
- `Push` 是 ID 的唯一分配点。ID 从 1 开始、在实例内单调递增；`Clear`、`Resize` 都不重置分配器，零容量 `Push` 也不消耗 ID。
- `DeadlockRecord::ToDatum` 将一条记录的公共字段和指定等待链项投影为系统表列。空 digest、空 key、两个当前未实现的富文本列及未知列返回 SQL `NULL`；非空 key 使用大写十六进制。

这些职责由 `pkg/util/deadlockhistory/deadlock_history_test.rs` 覆盖 Go 对齐行为，并由 `pkg/util/deadlockhistory/migration_aster_unit_test.rs` 补充零容量、坏 tag、未知列和并发写入边界。

## 主要符号

- 九个 `Col*Str` 常量定义系统表列名契约：`DEADLOCK_ID`、`OCCUR_TIME`、`RETRYABLE`、`TRY_LOCK_TRX_ID`、`CURRENT_SQL_DIGEST`、`CURRENT_SQL_DIGEST_TEXT`、`KEY`、`KEY_INFO`、`TRX_HOLDING_LOCK`。
- `WaitChainItem` 表示等待图的一条有向边：`TryLockTxn` 等待 `TxnHoldingLock` 持有的 `Key`；`SQLDigest` 是当前 SQL 指纹，`AllSQLDigests` 是预留的近期指纹列表。当前转换函数总把 `AllSQLDigests` 置空。
- `DeadlockRecord` 表示一次死锁事件，包含 `OccurTime`、完整 `WaitChain`、历史分配的 `ID` 和 `IsRetryable`。其 `ToDatum(waitChainIdx, columnName)` 按列名读取公共字段或 `WaitChain[waitChainIdx]`。
- `null_datum` 和 `occur_time_datum` 是内部投影辅助函数。后者把 `DateTime<Tz>` 拆成日历字段并构造 MySQL `TIMESTAMP`，以 `types::MaxFsp` 保存到微秒精度。
- `DeadlockHistoryState` 是锁内状态：`deadlocks: Vec<Option<Arc<DeadlockRecord>>>`、最旧槽位 `head`、有效长度 `size`、下一 ID `current_id`。
- `DeadlockHistory` 以 `RwLock<DeadlockHistoryState>` 提供线程安全外壳；`read_state` / `write_state` 负责锁获取，`get_all` 在已持锁状态上展开环形缓冲。
- `NewDeadlockHistory(capacity)` 构造一个空实例；`GlobalDeadlockHistory` 是惰性初始化、初始容量为 0 的进程级实例。
- `Resize`、`Push`、`GetAll`、`Clear` 是主要公共操作；`Head`、`Len`、`Capacity` 暴露内部指标，现有 Rust 测试也用它们验证不变量。
- `ErrDeadlock` 是 TiKV client-go 同名错误载荷在本 crate 的 Rust 对应物，包装 `kvrpcpb::Deadlock` 与可重试标志；`ErrDeadlockToDeadlockRecord` 完成载荷转换。

本文件没有 trait、宏或条件编译项；测试模块的条件编译位于 `lib.rs`，不在本文件中。

## 执行流程

1. 上游收到 TiKV 死锁错误后，应把它包装为 `ErrDeadlock` 并调用 `ErrDeadlockToDeadlockRecord`。函数为 protobuf 等待链预分配空间，逐项读取 `txn`、`wait_for_txn`、`key` 和 `resource_group_tag`。
2. 每个 tag 交给 `DecodeResourceGroupTag`。成功且含 digest 时保存 digest 字节；无 digest 或解码失败时保存空字节，失败分支同时写 warning。digest 最终用小写十六进制写入 `WaitChainItem::SQLDigest`，而等待链项不会因解码失败被丢弃。
3. 转换函数用当前 UTC 时间构造 `OccurTime`，令 `ID = 0`，复制 `IsRetryable`，返回尚未进入历史缓冲的记录。
4. 调用方把 `Box<DeadlockRecord>` 交给 `Push`。函数先取得写锁；容量为 0 时直接丢弃。否则分配并递增 `current_id`，再把记录转为 `Arc`。缓冲未满时写入 `(head + size) % capacity` 并增加 `size`；已满时覆盖 `head` 并把 `head` 前移一格。
5. `GetAll` 在读锁内调用 `get_all`。若有效区间未跨数组尾部，就复制一个连续切片；若已环绕，则先复制 `head..capacity`，再复制数组开头的余段，因而结果始终从最旧到最新。
6. 系统表读取者应对每个记录的每个等待链项调用 `ToDatum`。记录级列（ID、时间、可重试）在每行重复，边级列从相应 `waitChainIdx` 读取；空等待链本身不会产生系统表行，这一点由 Rust/Go 测试的行生成辅助逻辑证实。
7. 配置变化调用 `Resize` 时，函数持写锁取得当前有序快照并把 `head` 归零。扩容重建带空槽的数组并从下标 0 回填；缩容取快照尾部的最新 `newCapacity` 条。`Clear` 则清空槽位和逻辑长度，但有意保留 `current_id`。

## 数据与状态

环形缓冲的核心不变量是 `0 <= size <= deadlocks.len()`；容量非零时 `head` 指向最旧记录，逻辑第 `i` 条记录位于 `(head + i) % capacity`。有效范围内的槽位应为 `Some`，所以 `get_all` 使用 `flatten`；若内部不变量被其他实现破坏，`flatten` 会静默跳过空洞，不过当前状态字段私有且只由本文件修改。`Push` 中 `size > capacity` 被视为不可能并走 `unreachable!()`。

记录进入缓冲后由 `Arc<DeadlockRecord>` 共享。`GetAll` 克隆的是 `Arc` 而不是深拷贝记录，因此返回值是当次有序集合的所有权快照：后续覆盖、清空或缩容不会使调用方手中的记录失效。`Push` 接受 `Box` 并在分配 ID 后转为 `Arc`，使已存记录不再能通过本 API 被可变访问，这比 Go 版要求“Push 后不要修改指针内容”更强。

`current_id` 从 1 起，只在容量大于 0 的成功插入前递增。覆盖、`Clear` 和 `Resize` 不会回退它；因此 ID 表示该历史实例接受记录的先后次序，不保证跨进程或跨实例唯一。普通 `u64` 加法没有显式溢出处理，极端长期运行到达上限时行为取决于 Rust 构建的整数溢出设置。

`OccurTime` 保存 `chrono_tz::Tz` 时区，但转换自 TiKV 错误时固定使用 UTC。`ToDatum` 按保存时间的日历字段构造 `TIMESTAMP`，纳秒除以 1000，低于微秒的部分会截断。`AllSQLDigests`、`CURRENT_SQL_DIGEST_TEXT` 与 `KEY_INFO` 尚未由本实现填充或投影。

## 依赖与调用关系

下游依赖如下：`chrono` / `chrono-tz` 提供事件时间和字段拆分；`types` 与 `mysql` 构造系统表使用的 `Datum` / `TIMESTAMP`；`resourcegrouptag` 同时提供 kvproto `Deadlock` 类型和 tag 解码器；`hex` 分别用于 digest 的小写编码与 key 的大写编码；`log` 记录 tag 解码失败；标准库 `RwLock`、`Arc`、`LazyLock` 实现共享状态与全局生命周期。虽然 Cargo 直接列出 `protobuf`，本生产文件没有直接调用该 crate API，它主要参与 protobuf 类型生态及测试构造。

RustCodeGraph 对目标文件列出 `deadlock_history_test.rs` 与 `migration_aster_unit_test.rs` 等使用面，并确认 `lib.rs` 公开该模块。文本核验还显示根 `pkg/lib.rs` 通过 facade 再导出 crate，`pkg/executor`、`pkg/infoschema`、`pkg/server/handler/tikvhandler` 等 manifest 声明了 Cargo 依赖；但当前这些 Rust 生产文件没有实际引用本文件符号。唯一搜索到的 Rust 生产同名调用 `cmd/tidb-server/main.rs` → `deadlockhistory::GlobalDeadlockHistory.Resize` 解析到 `cmd/tidb-server/stubs.rs` 的桩类型，只记录事件，不修改这里的环形缓冲。

Go 对照主链提供“预期接线位置”的直接证据：`pkg/executor/select.go` 的悲观锁 `OnDeadlock` 回调调用 `ErrDeadlockToDeadlockRecord` 后 `Push`；`pkg/executor/infoschema_reader.go` 调用 `GetAll` 生成系统表行；`cmd/tidb-server/main.go` 在启动配置中 `Resize`；`pkg/server/handler/tikvhandler/tikv_handler.go` 在 HTTP 配置变更后再次 `Resize`。这些是 Go 当前事实，不应表述为 Rust 已完成接线。

## 错误处理与边界

- `ErrDeadlockToDeadlockRecord` 将 resource group tag 解码失败降级为 warning 与空 digest，并继续保留等待链项；`Ok(None)` 同样得到空 digest但不告警。这保证诊断记录不会因辅助标签损坏而整体丢失。
- `ToDatum` 对空 digest、空 key、未支持列和未知列返回 `NULL`。但是它直接索引 `WaitChain[waitChainIdx]`；对需要等待链字段的列传入越界下标会 panic。调用者必须只遍历真实等待链下标。记录级列不访问该下标。
- 容量 0 是合法的禁用状态：`Push` 无副作用且不分配 ID，`GetAll` 返回空；`Resize(0)` 清空已有记录并保留下一 ID。
- `Resize` 到相同容量立即返回；缩容会永久丢弃最旧记录，`Clear` 会永久丢弃全部已缓存记录。二者都没有持久化或恢复机制。
- `read_state` / `write_state` 遇到 poisoned `RwLock` 时通过 `into_inner()` 继续使用状态，而不向调用方返回错误。这样保持 API 无 `Result`，但若先前 panic 发生在状态更新中途，后续调用看到的状态不保证自动修复。
- `occur_time_datum` 不返回转换错误；它假定 `DateTime<Tz>` 给出的年月日时分秒能由 `types::FromDate` 接受。当前 `ErrDeadlockToDeadlockRecord` 使用正常 UTC 当前时间。

## 并发与资源生命周期

所有缓冲公共操作都通过一个 `RwLock` 串行化状态变化：`Push`、`Resize`、`Clear` 持独占写锁，`GetAll`、`Head`、`Len`、`Capacity` 持共享读锁。ID 分配与插入处于同一写锁临界区，因此并发 `Push` 不会获得重复 ID；`migration_aster_unit_test.rs::public_apis_are_thread_safe` 用 4 个线程共插入 100 条记录并验证 ID 集合为 `1..=100`。

`GetAll` 在读锁内完成 `Arc` 克隆，返回后不再持锁，调用方可长期读取记录而不阻塞写入。被覆盖或清除的槽位只释放缓冲持有的那一个 `Arc`；记录会在最后一个外部 `Arc` 离开作用域时释放。`Resize` 在整个快照、重建和回填期间持写锁，容量越大暂停并发读写的时间越长，复杂度和额外内存均为 O(当前记录数或新容量)。`Clear` 逐槽释放引用，复杂度为 O(容量)。

`GlobalDeadlockHistory` 由 `LazyLock` 首次访问时创建，生命周期覆盖整个进程，没有显式关闭动作、后台任务、通道或 I/O 资源。该全局对象初始容量为 0；只有真实调用 `Resize` 后才会接收记录，而当前 Rust 生产主链尚未接到本对象。

## 与 Go 版本的对应关系

Rust 数据模型、列常量、环形算法、ID 规则、清空/调容语义和错误转换总体逐项复刻 `pkg/util/deadlockhistory/deadlock_history.go`。`deadlock_history_test.rs` 对齐 Go 的 `TestDeadlockHistoryCollection`、`TestGetDatum`、`TestErrDeadlockToDeadlockRecord`、`TestResize`；两边都验证覆盖顺序、系统表列、当前时间与 tag digest。Go 的 `columnValueGetterMap` 在 Rust 中改为 `ToDatum` 的 `match`，未知列仍返回 `NULL`。

值得注意的语言与迁移差异：

- Go 构造器返回指针，内部保存可变的 `*DeadlockRecord`；Rust 构造器返回值，`Push` 接受 `Box` 后存为 `Arc`，避免 Push 后经公共 API 修改记录。
- Go 用嵌入式 `sync.RWMutex`；Rust 把全部可变字段封装进 `RwLock<DeadlockHistoryState>`，并显式选择在锁 poisoned 后继续。
- Go `capacity uint` 对应 Rust `usize`，保持目标平台指针宽度；补充测试 `history_capacity_uses_platform_uint_width` 防止错误收窄。
- Go 解码错误时告警后仍对返回字节做十六进制编码；Rust 明确把 `Err` 与 `Ok(None)` 归为空 digest。补充测试验证非法 tag 不丢等待项。
- Go 全局对象已接入 executor、information schema、启动配置和 HTTP 配置；Rust crate 的实现与测试已经存在，但这些真实生产调用尚未迁移，启动入口目前只调用同名 stub。

## 扩展指南

若新增系统表列，先增加或复用列名常量，再在 `DeadlockRecord::ToDatum` 中明确该列属于记录级还是等待链级；同步扩展 `deadlock_history_test.rs::test_get_datum` 和 `migration_aster_unit_test.rs::datum_conversion_matches_go_columns_and_nulls`，并核对 Go 的 `columnValueGetterMap`。实现 `CURRENT_SQL_DIGEST_TEXT`、`KEY_INFO` 或 `AllSQLDigests` 时，还要确定信息来源、缺失时的 `NULL` 契约、解码成本及敏感信息暴露边界。

若改变历史保留策略，应集中修改 `DeadlockHistoryState`、`Push`、`get_all`、`Resize` 与 `Clear`，保持“结果从旧到新”“有效槽无空洞”“ID 只由 Push 分配”三项不变量；同步两个独立 Rust 测试文件，而不要把测试嵌入生产源文件。改变锁粒度或返回所有权时必须评估 `Arc` 快照语义、写锁暂停时间与 poison 策略。

若补齐生产接线，应按 Go 证据分别处理四个边界：悲观锁回调负责过滤可重试配置并转换/写入，information schema 负责取得快照并逐等待边投影，启动配置负责初始 `Resize`，HTTP 配置负责运行时 `Resize`。接线时必须引用本 crate 而不是 `cmd/tidb-server/stubs.rs` 的同名桩，并为采集到查询的端到端行为增加各所属目录的独立测试；Cargo manifest 已有依赖不等于代码已经接线。

兼容性方面，列名、`NULL` 规则、key 大写十六进制、digest 小写十六进制、ID 延续和缩容保最新均是可观察行为。性能方面，增大容量会增加 O(capacity) 常驻槽位，`GetAll` 每次克隆 O(size) 个 `Arc`，`Resize` 会在写锁下分配并搬移引用；新增富文本解码不应放大每次系统表查询或锁内临界区。

## 验证依据

- RustCodeGraph：`status` 显示本仓库索引包含目标文件；`node --file pkg/util/deadlockhistory/deadlock_history.rs` 读取 342 行完整实现并列出使用文件；`query DeadlockRecord` / `query DeadlockHistory` 核对 Go/Rust 同名类型与方法；`node` 读取 `lib.rs`、`deadlock_history_test.rs`、`migration_aster_unit_test.rs`，核对模块导出、Go 对齐用例与补充边界用例。精确 `callers ErrDeadlockToDeadlockRecord` 在本地索引上持续无输出，已中止，未据此推断不存在调用者。
- Rust 源与 crate：`pkg/util/deadlockhistory/deadlock_history.rs`、`pkg/util/deadlockhistory/lib.rs`、`pkg/util/deadlockhistory/Cargo.toml`、根 `Cargo.toml`、`pkg/lib.rs`；目标包下不存在 `doc.go`。
- Rust 测试：`pkg/util/deadlockhistory/deadlock_history_test.rs`；补充测试 `pkg/util/deadlockhistory/migration_aster_unit_test.rs`。本任务按计划是纯文档分析，未运行 Cargo。
- Go 对照：`pkg/util/deadlockhistory/deadlock_history.go`、`pkg/util/deadlockhistory/deadlock_history_test.go`；生产主链证据来自 `pkg/executor/select.go`、`pkg/executor/infoschema_reader.go`、`cmd/tidb-server/main.go`、`pkg/server/handler/tikvhandler/tikv_handler.go`。
- Rust 接线核验：Cargo 依赖声明见 `pkg/executor/Cargo.toml`、`pkg/infoschema/Cargo.toml`、`pkg/server/handler/tikvhandler/Cargo.toml` 等；源码符号搜索仅发现 facade 再导出、测试引用，以及 `cmd/tidb-server/main.rs` 对 `cmd/tidb-server/stubs.rs` 同名桩的调用，故文档将 Rust 生产接线标为未完成而非“已支持”。
- 交付检查以任务文件指定的结构命令确认文档存在且恰有十一个固定二级标题；另人工复核章节顺序、源码链接、Go/Rust 事实边界、扩展点，以及改动范围只包含本说明文档与完成后删除的编号任务文件。
