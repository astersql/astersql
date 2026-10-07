# `br/pkg/metautil/statsfile_test_support.rs`

## 文件定位

[`statsfile_test_support.rs`](statsfile_test_support.rs) 是 `astersql-br-pkg-metautil` crate 的测试专用辅助模块。crate 根在 [`lib.rs`](lib.rs) 中用 `#[cfg(test)]` 和 `#[path = "statsfile_test_support.rs"]` 挂载它，因此普通库构建不会包含本模块；模块也没有从 crate 根再导出。它直接服务于同 crate 的 [`statsfile_test.rs`](statsfile_test.rs) 和 [`parity_test.rs`](parity_test.rs)，负责保护这两个测试会临时改写的 stats 文件阈值。

crate 边界由 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定。本文件只依赖 Rust 标准库和同 crate 的 `statsfile` 模块，不引入新的外部依赖或 feature。

## 核心职责

本文件把对 `statsfile.rs` 两个包级原子阈值的测试改写包装成一个作用域守卫：

1. `StatsConfigTestGuard::acquire` 先独占进程内互斥锁，使所有遵循该守卫协议的测试串行修改阈值。
2. 获锁后把 `maxStatsJsonTableSize` 和 `inlineSize` 重置为 Go 版本的默认值，清理此前失败测试可能留下的状态。
3. 守卫离开作用域时，`Drop::drop` 在释放互斥锁前再次恢复默认值。

它不实现 stats 文件的写入或恢复；实际阈值消费点位于 `statsfile.rs` 的 `StatsWriter::BackupStats` 和 `StatsWriter::writeStatsFileAndClear`。它也不是通用配置 API，而是测试之间共享全局状态的隔离设施。

## 主要符号

- `DEFAULT_MAX_STATS_JSON_TABLE_SIZE: usize = 32 * 1024 * 1024`：测试恢复用的最大 stats JSON 缓冲默认值，等于 Go `statsfile.go` 中的 32 MiB。
- `DEFAULT_INLINE_SIZE: usize = 8 * 1024`：测试恢复用的首个 stats 文件内联阈值，等于 Go 默认的 8 KiB。
- `STATS_CONFIG_TEST_LOCK: Mutex<()>`：模块私有的零数据互斥锁。锁保护的是“谁可以改写两个全局阈值”这一测试协议，而不是业务数据。
- `pub(crate) struct StatsConfigTestGuard`：crate 内可见的 RAII 守卫；唯一字段 `_guard: MutexGuard<'static, ()>` 让互斥锁持有期与守卫生命周期一致。
- `StatsConfigTestGuard::acquire() -> Self`：唯一构造入口。它接管 poisoned mutex 的内部 guard，再用 `Ordering::SeqCst` 恢复两个默认值。
- `impl Drop for StatsConfigTestGuard::drop`：在字段 `_guard` 被销毁并解锁之前恢复默认值。

本文件没有 trait、枚举、异步函数或条件编译项；条件编译发生在上游 `lib.rs` 的模块声明处。

## 执行流程

典型流程可由 `statsfile_test.rs::test_stats_writer` 和 `parity_test.rs::stats_writer_inline_and_file_flush_matches_go` 复核：

1. 测试首先调用 `StatsConfigTestGuard::acquire`，阻塞等待 `STATS_CONFIG_TEST_LOCK`。
2. `acquire` 用 `PoisonError::into_inner` 处理之前测试 panic 导致的 poisoned 状态，因此不会因锁中毒再次 panic。
3. 在锁仍被持有时，`acquire` 将两个原子值恢复为 32 MiB 和 8 KiB，然后返回守卫。
4. 测试把两个阈值都写成 `1`。随后 `StatsWriter::BackupStats` 读取 `maxStatsJsonTableSize`，因累计大小严格大于 1 而刷盘；`writeStatsFileAndClear` 读取 `inlineSize`，因内容不满足内联条件而写对象存储。
5. 测试可以在作用域内手工恢复默认值并继续覆盖后续分支；无论正常返回还是 unwind panic，守卫的 `drop` 都再次恢复默认值。
6. `drop` 返回后，Rust 才销毁 `_guard` 并释放互斥锁，所以下一个遵循协议的测试在获锁时不会观察到上一个测试的临时值。

## 数据与状态

受协调的真实状态不存放在守卫中，而是 `statsfile.rs` 中两个 `AtomicUsize`：

- `maxStatsJsonTableSize`：`StatsWriter::BackupStats` 每次追加 block 后读取；只有 `totalSize > threshold` 才触发刷盘，等于阈值时不刷。
- `inlineSize`：`StatsWriter::writeStatsFileAndClear` 在“尚无 index”时读取；只有序列化内容长度严格小于阈值才内联。

守卫自身只保存 `MutexGuard`，不保存旧阈值快照。因此退出时总是恢复固定的 Go 默认值，而不是恢复调用前任意值。这一设计符合现有测试只在单层作用域内暂时改阈值的用法，但不支持嵌套获取：同一线程持有守卫时再次调用 `acquire` 会等待同一非重入 mutex。

所有原子读写均使用 `Ordering::SeqCst`，与 `statsfile.rs` 的读取顺序一致。互斥锁负责组合改写的测试级排他性；两个原子变量仍是分别写入，不构成可供无锁观察者读取的原子二元快照。

## 依赖与调用关系

上游关系如下：

- `lib.rs` 仅在 `cfg(test)` 下声明 `statsfile_test_support`。
- `statsfile_test.rs::test_stats_writer` 获取守卫，覆盖明文与 AES-128/192/256-CTR 的备份/恢复闭环。
- `parity_test.rs::stats_writer_inline_and_file_flush_matches_go` 获取守卫，覆盖刷盘、非内联索引、下载重写和缺失 rewrite 的错误契约。

下游关系如下：

- `std::sync::Mutex` / `MutexGuard` 提供测试间串行化和作用域解锁。
- `std::sync::atomic::Ordering::SeqCst` 指定阈值写入顺序。
- `crate::statsfile::{maxStatsJsonTableSize, inlineSize}` 是被重置的共享原子状态。
- `statsfile.rs::StatsWriter::BackupStats` 与 `StatsWriter::writeStatsFileAndClear` 是阈值的业务读取方。

RustCodeGraph 的文件节点报告本文件包含 4 个符号，并显示 `parity_test.rs` 使用该文件；图查询未枚举 `statsfile_test.rs` 的边，因此实际调用集合另由全仓符号检索确认，不能把单条图边解释为唯一调用方。

## 错误处理与边界

`acquire` 不返回 `Result`。普通锁中毒不会阻止后续测试：`unwrap_or_else(PoisonError::into_inner)` 取得中毒锁中的 guard，随后立即覆盖两个阈值，从已知默认状态继续。该处理只恢复配置，不能修复导致前一次 panic 的其他共享状态。

RAII 恢复适用于正常离开作用域和启用 unwinding 的 panic；若进程直接 abort、被强制终止或使用 `panic = "abort"`，`Drop` 不会执行。不过进程退出后这些内存原子值也不会跨进程保留。

排他性只约束也使用 `StatsConfigTestGuard` 的代码。任何绕过守卫直接写入公开原子的测试仍可能并发干扰，因此新增阈值改写测试必须先获取守卫。固定默认值还意味着不要用该守卫表达“临时覆盖后恢复任意调用前配置”的通用需求。

## 并发与资源生命周期

`STATS_CONFIG_TEST_LOCK` 是进程级静态 mutex，生命周期覆盖整个测试进程。`acquire` 返回的 `MutexGuard<'static, ()>` 被封装在 `StatsConfigTestGuard` 中；测试通常把它绑定为 `_stats_config_guard`，让生命周期自然延伸到测试函数末尾。

析构顺序是关键不变量：自定义 `StatsConfigTestGuard::drop` 先执行两次原子 `store`，之后结构体字段 `_guard` 才被释放并解锁。这保证等待者获得锁时默认值已经恢复。锁不覆盖 `StatsWriter`、对象存储或下载线程本身；这些资源由各测试自行创建、join 或释放。守卫也不参与 channel 关闭和 worker 生命周期。

## 与 Go 版本的对应关系

Go 生产文件 `statsfile.go` 定义普通包变量 `maxStatsJsonTableSize = 32 * 1024 * 1024` 与 `inlineSize = 8 * 1024`。Go 测试 `statsfile_test.go::TestStatsWriter` 在第一张表前把两者设为 `1`，随后在第二张表前手工恢复默认值，以强制覆盖刷盘和非内联路径。

Rust 的生产语义保持相同阈值和值比较，但因为 Rust 测试默认可并行执行，`statsfile.rs` 将变量实现为 `AtomicUsize`，本文件再增加 mutex + RAII 清理。这是 Rust 测试隔离所需的局部接线，不对应独立 Go 源文件。Rust `statsfile_test.rs::test_stats_writer` 直接移植 Go 用例；`parity_test.rs` 复用同一守卫做额外契约验证。

Go 用例依靠测试过程中的显式恢复，若中途断言失败可能留下修改；Rust 守卫则在 unwind 时恢复，并在下一次 `acquire` 后再次初始化，增强了同进程测试的抗污染能力。两边生产路径的默认值仍一致。

## 扩展指南

新增或修改 stats 阈值相关测试时，应遵循以下接入点：

1. 在修改 `maxStatsJsonTableSize` 或 `inlineSize` 之前调用 `StatsConfigTestGuard::acquire`，并让绑定至少活到最后一次依赖临时值的断言或线程 join 之后。
2. 若生产默认值变化，同时更新 `statsfile.rs` 的原子初值、本文件两个 `DEFAULT_*` 常量、Go 对照（若 Go 契约也变化）以及两个现有 Rust 调用测试，避免清理值与生产默认值漂移。
3. 若增加第三个可变的 stats 测试配置，将其重置加入 `acquire` 和 `drop`，并确认读取方使用兼容的同步语义；必要时把多字段配置封装成单一快照，避免跨字段观察到过渡状态。
4. 不要在本生产候选文件内嵌 `#[test]`。按仓库约定，把回归测试放在同目录的独立 `*_test.rs` 文件，并由 `lib.rs` 的 `#[cfg(test)]` 模块声明挂载。
5. 若需要嵌套覆盖或恢复调用前值，应设计独立的可嵌套机制；当前非重入 mutex 与固定默认值恢复不能安全满足该需求。

兼容性风险主要是默认值与 Go 漂移；并发风险是新增测试绕过守卫；性能影响仅限测试进程中的串行等待，生产构建不包含本模块。

## 验证依据

- `br/pkg/metautil/statsfile_test_support.rs`：4 个核心符号、锁中毒恢复、两次默认值重置和析构顺序的直接依据。
- `br/pkg/metautil/lib.rs`：模块只在 `cfg(test)` 下挂载，且未公开再导出的依据。
- `br/pkg/metautil/Cargo.toml`：`astersql-br-pkg-metautil` crate 边界、库入口和依赖范围的依据。
- `br/pkg/metautil/statsfile.rs`：两个 `AtomicUsize` 定义，以及 `BackupStats`、`writeStatsFileAndClear` 用 `SeqCst` 读取阈值的依据。
- `br/pkg/metautil/statsfile_test.rs`：`test_stats_writer` 获取守卫、临时写入 1、恢复默认值并覆盖多种 cipher 的直接调用证据。
- `br/pkg/metautil/parity_test.rs`：`stats_writer_inline_and_file_flush_matches_go` 获取守卫并验证刷盘、索引及恢复错误契约的直接调用证据。
- `br/pkg/metautil/statsfile.go` 与 `br/pkg/metautil/statsfile_test.go`：Go 默认值、生产比较条件及测试改写顺序的对照依据。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter` 确认目标文件已索引；`query StatsConfigTestGuard` 定位结构体；`node --file ...` 返回完整 58 行源码和 `parity_test.rs` 使用边。精确 `callers/callees` 查询在本次执行中超时且无输出，因此调用方结论由上述两个 Rust 测试文件的直接引用检索补齐。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档存在且恰有 11 个固定二级章节，并用 `git diff --check` 检查 Markdown 变更。
