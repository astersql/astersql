# `br/pkg/utils/common.rs`

## 文件定位

`br/pkg/utils/common.rs` 属于 `astersql-br-pkg-utils` library crate；crate 根由 `br/pkg/utils/Cargo.toml` 的 `[lib] path = "lib.rs"` 指定，`br/pkg/utils/lib.rs` 再通过 `#[path = "common.rs"] pub mod common;` 公开该模块。它是 Go `br/pkg/utils/common.go` 的 Rust 对照文件，集中承载两类 BR 公共能力：在 BR 内部事务语境中批量分配全局 ID，以及在恢复开始前判断 classic/next-gen 内核与 keyspace 参数是否兼容。

该模块当前是公开子模块，但 `lib.rs` 没有把 `NewMutator`、`GenGlobalIDs` 或 `CheckNextGenCompatibility` 再导出到 crate 根，调用者需要经过 `astersql_br_pkg_utils::common` 路径。RustCodeGraph 显示该文件直接被 `br/pkg/utils/common_test.rs` 与 `br/pkg/utils/stubs.rs` 关联；全仓 Rust 精确引用搜索没有找到测试之外的生产调用点，因此本文会区分“已实现的 API”与“已接入的应用主链”，不把 Go 侧调用关系误写成 Rust 侧现状。

## 核心职责

1. `NewMutator` 把 BR 的抽象 `Transaction` 转换为 `astersql_meta::Mutator`。转换最终调用 `pkg/meta/meta.rs::new_mutator`，后者为事务设置高优先级与 `AllowedOnAlmostFull` 磁盘满策略，并以 meta 前缀构建结构化访问器。
2. `GenGlobalIDs` 给上下文附加 `InternalTxnBR` 来源标记，在一个由 `RunInNewTxn` 提供的事务闭包中一次分配 `n` 个连续全局 ID，并把 meta 层错误收敛为 crate 通用的 `SharedError`。
3. `CheckNextGenCompatibility` 根据编译期内核类型和 `keyspace_name` 判定恢复配置：classic 构建通常返回 `false`，next-gen 且有 keyspace 时返回 `true`；不兼容组合会告警或 panic。

边界限制很重要：`common.rs` 调用的是 `br/pkg/utils/stubs.rs` 中的 KV 抽象。该桩的 `WithInternalSourceType` 只原样返回上下文，`RunInNewTxn` 只在共享内存 meta 事务上同步调用一次闭包，不连接真实 KV、没有提交/回滚，也没有按 `_retry` 重试。因此当前 Rust 实现能验证分配算法和状态连续性，但尚不能证明 Go 生产路径的真实事务生命周期已经接通。

## 主要符号

- `fn map_meta_error(err: astersql_meta::errors::Error) -> SharedError`：模块私有适配器。它取 meta 错误的字符串，包装为 `std::io::ErrorKind::Other`，再构造 `SharedError`，使 `RunInNewTxn` 闭包能使用统一错误类型。此转换保留可读消息，但不保留原始 meta 错误的具体类型。
- `pub fn NewMutator(txn: &mut dyn Transaction) -> astersql_meta::Mutator`：公开的 Go 风格命名入口。它从 `txn.meta_transaction()` 取得 meta 事务并调用 `astersql_meta::new_mutator(..., vec![])`；空 option 列表表示不额外改变 mutator 选项。
- `pub fn GenGlobalIDs(ctx: Context, n: i32, storage: &dyn Storage) -> Result<Vec<i64>, SharedError>`：公开批量 ID 分配入口。输入拥有所有权的轻量 `KvContext`、请求数量及共享借用的存储抽象，成功返回连续 `i64` ID，失败返回 `SharedError`。
- `pub fn CheckNextGenCompatibility(keyspace_name: &str, check_requirements: bool) -> bool`：公开恢复兼容性检查。返回值只表达“是否为合法 next-gen 恢复”；不兼容情况可能通过 panic 终止当前执行流，而不是全部编码为 `false`。
- `InternalTxnBR`、`Context`、`RunInNewTxn`、`Storage`、`Transaction`、`WithInternalSourceType`：均来自 `br/pkg/utils/stubs.rs`，是本文件的事务边界。
- `IsClassic`、`IsNextGen`：来自 `pkg/config/kerneltype`。`classic.rs` 和 `nextgen.rs` 分别受 `cfg(not(feature = "nextgen"))` 与 `cfg(feature = "nextgen")` 控制，所以分支由构建 feature 决定，不是运行时探测集群。

## 执行流程

`GenGlobalIDs` 的执行顺序如下：

1. 创建空的 `Vec<i64>` 作为闭包结果槽。
2. 调用 `WithInternalSourceType(ctx, InternalTxnBR)` 标记 BR 内部事务语境；当前桩实现不保存该标记，只返回原上下文。
3. 以 `retry = true` 调用 `RunInNewTxn`。当前桩从 `Storage::meta_transaction()` 得到共享 meta 状态，构造 `EmptyTxn`，然后同步执行闭包一次。
4. 闭包通过 `NewMutator(txn)` 构造 meta mutator，再调用 `Mutator::gen_global_ids(n)`。
5. `pkg/meta/meta.rs::gen_global_ids` 先经 `advance_global_ids(n)` 在 `GLOBAL_ID_MUTEX` 保护下推进 `NEXT_GLOBAL_ID_KEY`，检查 `MAX_USER_GLOBAL_ID` 上限，再展开为从 `old + 1` 到 `old + n` 的连续区间。
6. meta 错误经 `map_meta_error` 转换；任一错误通过 `?` 离开闭包及 `RunInNewTxn`。成功后返回闭包写入的 `ids`。

`CheckNextGenCompatibility` 的分支顺序如下：

1. 若 `IsClassic()` 且 keyspace 非空，生成“不支持 keyspace 恢复、可能造成高磁盘占用”的消息。`check_requirements = true` 时 panic；为 `false` 时记录 Warn 日志并继续，最终返回 `false`。
2. 否则若 `IsNextGen()`，keyspace 为空时无条件 panic，避免后续 SST ingest 风险；非空时返回 `true`。
3. 其余情况返回 `false`，对应未携带 keyspace 的 classic 恢复。

## 数据与状态

`common.rs` 自身没有模块级可变状态；`ids` 是 `GenGlobalIDs` 每次调用的局部所有值，闭包同步写入后返回。真正的 ID 状态位于下游 meta 事务的 `NEXT_GLOBAL_ID_KEY`。`pkg/meta/meta.rs` 用进程级 `GLOBAL_ID_MUTEX` 串行化 `advance_global_ids`，并通过事务的 `inc` 更新计数器，返回严格连续的区间。

当前 BR utils 桩会在 `shared_meta_transaction()` 的 `OnceLock<astersql_meta::kv::Transaction>` 中保存一个共享初始事务，各 `Storage` 默认克隆这个句柄；因此同一进程中的连续测试调用能够观察到递增状态。`Storage` 也可覆盖 `meta_transaction()` 来提供自己的状态。此行为是本地替身的状态模型，不等同于真实集群中持久化、隔离和提交后的 KV 状态。

`KvContext` 内部只有共享的 `AtomicBool` 取消位，但本文件调用链没有读取取消状态；内部来源常量值为 `"br"`，当前 `WithInternalSourceType` 也没有把它持久化。兼容性检查不缓存结果，其输入是借用字符串和布尔值，内核类型由编译 feature 固定。

## 依赖与调用关系

上游与模块装配：

- `br/pkg/utils/lib.rs` 声明公开 `common` 模块，并在 `cfg(test)` 下把 `br/pkg/utils/common_test.rs` 挂入同一 crate。
- RustCodeGraph 的文件关系只确认 `common_test.rs` 与 `stubs.rs` 使用该文件；`rg` 对全仓 Rust 的符号搜索未发现当前生产调用者。因此 Rust 应用恢复主链尚未直接消费这里的 API。
- Go 对照主链中，`br/pkg/task/restore.go` 调用 `utils.CheckNextGenCompatibility` 决定恢复类型；`br/pkg/restore/log_client/client.go::LogClient.GenGlobalIDs` 委托 `utils.GenGlobalIDs`，随后可由 `br/pkg/task/stream.go` 的 table mapping 流程传入批量 ID 生成器。这些是迁移目标与语义背景，不是 Rust 已接线证据。

下游依赖：

- `crate::stubs` 提供上下文、存储、事务及新事务执行边界。
- `astersql_meta::new_mutator` 设置 meta 写事务策略；`Mutator::gen_global_ids` 完成锁内计数与区间生成。
- `astersql_config_kerneltype::{IsClassic, IsNextGen}` 提供编译期内核分支。
- `astersql_br_pkg_logutil::log::Warn` 只用于 classic + keyspace 且关闭严格检查的降级路径。
- `astersql_errors::SharedError` 是跨层错误返回类型。

`br/pkg/utils/Cargo.toml` 直接声明了上述 `astersql-meta`、`astersql-config-kerneltype`、`astersql-br-pkg-logutil` 和 `astersql-errors` 路径依赖。清单注释同时明确该 crate 为 darwin arm64 精简过 KV/domain/kvproto/grpcio 等边界并用本地 stubs 代替，这与当前未接真实 KV 的代码事实一致。

## 错误处理与边界

`GenGlobalIDs` 有两层可返回错误：`Mutator::gen_global_ids` 的 meta 错误，以及 `RunInNewTxn` 的事务执行错误；当前实现都归并成 `SharedError` 并直接传播。meta 层会在全局 ID 超过 `MAX_USER_GLOBAL_ID` 时失败。`map_meta_error` 的字符串化包装会丢失原始错误类型和可供精确匹配的结构信息，扩展重试分类或错误码时不能假设还能下转为 meta 错误。

该函数没有在本层校验 `n` 是否为正数；数量语义完全委托给 `Mutator::gen_global_ids/advance_global_ids`。新增调用方应遵守“请求正数量”的业务前置条件，若需要拒绝零或负数，应同时核对 Go 行为并在独立测试中固定兼容约束，不能只为 Rust 添加不同语义。

兼容性检查包含两个非 `Result` 的失败路径：classic + 非空 keyspace + 严格检查，以及 next-gen + 空 keyspace。Rust 用 `panic!` 表达，而 Go 对照用 `log.Fatal`；两者都表示不可继续，但 Go 会结束进程，Rust panic 则可能被 unwind/catch。classic + 非空 keyspace 且关闭严格检查仅告警并返回 `false`。next-gen 缺 keyspace 即使 `check_requirements = false` 也仍然 panic，这一点由分支结构和 Rust 测试共同确认。

当前 `RunInNewTxn` 桩忽略 `_retry`，也没有提交、回滚或取消检查；因此不能用现有成功测试证明失败后的原子性、重试次数或上下文取消行为。`common_test.rs` 也没有覆盖 meta 上限错误、存储自定义错误、警告文本或 classic 严格 panic 分支。

## 并发与资源生命周期

本文件没有异步任务、线程、通道、锁守卫字段或需要显式关闭的资源。`GenGlobalIDs` 的闭包同步执行；它借用 `storage` 和临时事务，闭包完成后事务包装即离开作用域。局部 `ids` 由闭包赋值并在调用结束时转移给调用者。

并发安全主要来自下游：`Storage` 要求 `Send + Sync`，`Transaction` 要求 `Send`；meta 的 ID 推进使用 `GLOBAL_ID_MUTEX` 防止同一进程内并发分配交错。桩的共享事务由 `OnceLock` 初始化并以可克隆句柄复用，使测试中的多个调用共享计数状态。由于该桩没有真实事务隔离、提交和跨进程协调，不能把进程内互斥推导为分布式环境中的完整一致性保证。

日志调用只在兼容性降级分支发生，不持有资源。panic 会提前中断当前调用栈；若未来在其前后加入资源获取，应使用能在 unwind 时自动释放的 RAII 类型，并避免依赖 panic 后的手工清理。

## 与 Go 版本的对应关系

`GenGlobalIDs` 保留了 Go 的骨架：建立空结果、用 `InternalTxnBR` 标记上下文、以允许重试的新事务执行 `meta.NewMutator(txn).GenGlobalIDs(n)`、最后返回结果和错误。Rust 额外暴露了 `NewMutator` 包装器，因为本地 `Transaction` 必须先转换成 `astersql_meta::kv::Transaction`。Go 使用真实 `kv.Storage`/`kv.RunInNewTxn`，Rust 当前使用 `stubs.rs` 的共享内存替身，这是最关键的迁移差异。

`CheckNextGenCompatibility` 的四种结果与 Go 对齐：classic 无 keyspace 为 `false`；classic 有 keyspace 在严格模式终止、宽松模式告警并返回 `false`；next-gen 无 keyspace 终止；next-gen 有 keyspace 返回 `true`。Rust 的 `IsClassic/IsNextGen` 同样由构建配置选择，但终止手段从 Go `log.Fatal` 变为 `panic!`，因此进程级副作用并不完全等价。

测试对应关系目前只有 Rust 独立测试 `br/pkg/utils/common_test.rs`，没有同名 `common_test.go`。Rust 的 `gen_global_ids_reuses_storage_state_like_go` 验证两次各分配两个 ID，长度正确且第二批紧接第一批；`compatibility_matches_go_kernel_specific_paths` 根据编译内核验证 classic 宽松路径，或验证 next-gen 空 keyspace panic 与非空 keyspace 成功。Go 的生产调用证据来自 `br/pkg/task/restore.go`、`br/pkg/restore/log_client/client.go` 与 `br/pkg/task/stream.go`，而不是该目录的 Go 单测。

## 扩展指南

- 若要把 Rust API 接入真实 BR 恢复主链，优先替换或桥接 `stubs.rs::{Storage, Transaction, RunInNewTxn, WithInternalSourceType}` 到真实 KV 实现，保持 `InternalTxnBR` 标记、可重试事务与提交/回滚语义；不要只在 `common.rs` 中模拟成功结果。
- 若新增 ID 分配策略，修改点应集中在 `GenGlobalIDs` 与 `pkg/meta/meta.rs::Mutator` 的既有能力之间。必须同步扩展独立的 `br/pkg/utils/common_test.rs`，覆盖数量边界、错误传播、连续性和并发分配；Rust 测试逻辑应继续对照 Go，不应内嵌到生产文件。
- 若改变兼容矩阵或提示文本，应同步核对 `br/pkg/utils/common.go`、Go 调用者的命令行 `--check-requirements` 语义，以及 classic/next-gen 两种 feature 构建。尤其不能把 next-gen 空 keyspace 误降级为宽松告警。
- 若调用者需要可恢复错误而不是 panic，应先决定如何保持 Go `log.Fatal` 的用户可见终止契约，再统一调整返回类型与所有调用点；单独修改 Rust 会形成兼容分叉。
- 若错误分类需要保留 meta 错误身份，应重新设计 `map_meta_error`，避免仅保留字符串，并为上限错误与事务错误分别增加断言。
- 性能上，批量申请应继续一次推进计数器后展开连续区间，避免退化为逐 ID 加锁/事务；兼容性检查应保持纯分支加至多一次日志，不引入网络探测。

## 验证依据

- RustCodeGraph 索引状态：11467 个文件、7032 个 Rust 文件；`files --filter br/pkg/utils` 确认目标、Go 对照与独立测试均在索引中。
- RustCodeGraph `node --file br/pkg/utils/common.rs --offset 1 --limit 260`：确认文件共 85 行、四个函数符号、导入依赖及文件关系。
- RustCodeGraph `query`：确认 `NewMutator` 位于第 41 行、`GenGlobalIDs` 位于第 47 行、`CheckNextGenCompatibility` 位于第 60 行、`map_meta_error` 位于第 32 行。对这些符号执行 `callers/callees` 未得到边，因此又以全仓精确 `rg` 搜索补齐调用证据，未把空图结果直接解释为无调用。
- RustCodeGraph `node new_mutator`：确认 `pkg/meta/meta.rs::new_mutator` 设置高优先级、近满盘允许写入和 meta 前缀；读取同文件 `gen_global_id`、`advance_global_ids`、`gen_global_ids` 段确认互斥、上限检查与连续区间算法。
- RustCodeGraph 文件节点：读取 `br/pkg/utils/stubs.rs` 的 `KvContext`、`Storage`、`Transaction`、`RunInNewTxn`，以及 `pkg/config/kerneltype/{classic,nextgen}.rs` 的 feature 分支，确认当前事务替身和编译期内核选择。
- 清单与模块证据：`br/pkg/utils/Cargo.toml`、`br/pkg/utils/lib.rs`。
- Go 对照与生产调用证据：`br/pkg/utils/common.go`、`br/pkg/task/restore.go`、`br/pkg/restore/log_client/client.go`、`br/pkg/task/stream.go`。
- Rust 独立测试：`br/pkg/utils/common_test.rs`；它由 `br/pkg/utils/lib.rs` 的 `#[cfg(test)] #[path = "common_test.rs"] mod common_test;` 挂载。
- 人工复核结论：文档明确回答了文件为何存在、ID 分配和兼容性分支如何运行、当前 Rust 主链与事务桩的限制，以及安全扩展时应修改和同步验证的位置；未声称 Cargo 或运行时测试已执行。
