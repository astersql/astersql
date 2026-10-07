# `pkg/ingestor/testutils/util.rs`

## 文件定位

该文件属于 `astersql-ingestor-testutils` crate，由 [`pkg/ingestor/testutils/lib.rs`](./lib.rs) 以 `mod util` 纳入并通过 `pub use util::*` 重导出公开类型。crate 的 [`Cargo.toml`](./Cargo.toml) 关闭了自动测试发现（`autotests = false`），并只依赖工作区内的 `objectio`、`objstore` 和 `storeapi`。

它不是 ingest 业务主链的生产存储实现，而是测试辅助层：在内存对象存储上追踪 reader 打开与关闭情况，使迭代器、重试和引擎测试能检查资源是否泄漏。包级 [`pkg/ingestor/doc.go`](../doc.go) 将 ingestor 定位为 SST 直接导入、KV 排序与导入环境准备的边界；本文件只为这些路径的测试提供可观测存储。

## 核心职责

- `TrackOpenMemStorage` 包装 `objstore::memstore::MemStorage`，在每次 `Open` 尝试时维护“当前未确认关闭数” `Opened` 和“累计打开尝试数” `TotalOpened`。
- `ObjectReaderAdapter` 在两套已迁移的 reader trait 之间做窄适配：将 `objstore::storage::ObjectReader` 暴露为 `objectio::Reader`，并转发 `Read`、`Seek`、关闭和文件大小查询。
- `TrackOpenFileReader` 在 reader 成功关闭后回减 `Opened`，从而把底层 reader 生命周期与测试可观测计数绑定。

该文件不负责写入、列举、删除对象，也不实现通用 `storeapi::Storage`；它只提供一个与 Go 测试工具对齐的 `Open` 入口和 reader 包装。

## 主要符号

- `pub struct TrackOpenMemStorage`：公开存储包装器。`MemStorage: Arc<...>` 使存储与已返回 reader 共享所有权；`Opened` 和 `TotalOpened` 是 `AtomicI32`，两者均公开，供测试直接读取。
- `TrackOpenMemStorage::Open(self: &Arc<Self>, ctx, path, opt) -> io::Result<Box<dyn objectio::Reader>>`：公开的 Go 风格打开方法。`self: &Arc<Self>` 是必要的，因为成功返回的 reader 需要持有包装存储的 `Arc` 克隆。
- `struct ObjectReaderAdapter`：文件私有适配器，内部保存 `Box<dyn objstore::storage::ObjectReader>`。它实现标准 `Read`/`Seek` 和 `objectio::Reader`。
- `pub struct TrackOpenFileReader`：公开 reader 包装类型；`Reader` 字段公开，`store` 私有，防止外部替换计数归属。
- `Read for TrackOpenFileReader` 与 `Seek for TrackOpenFileReader`：不改变数据面行为，直接转发给 `Reader`。
- `objectio::Reader for TrackOpenFileReader`：`close` 先关闭底层 reader，只有成功时才将 `Opened` 减一；`file_size` 直接转发。`objectio::Reader` 本身还提供 Go 风格别名 `Close` 和 `GetFileSize`（`pkg/objstore/objectio/interface.rs:71-90`）。

## 执行流程

1. 调用者用 `Arc<TrackOpenMemStorage>` 调用 `Open`。方法以 `SeqCst` 同时将 `Opened` 和 `TotalOpened` 加一，因此后者记录的是尝试而非成功数。
2. 若 `storeapi::Context::is_cancelled()` 已为真，立即回减 `Opened`，并返回 `io::ErrorKind::Interrupted`；`TotalOpened` 保留本次尝试。
3. 将高层 `storeapi::ReaderOption` 的 `StartOffset`/`EndOffset` 映射为底层 `objstore::storage::ReaderOption` 的 `start_offset`/`end_offset`。`PrefetchSize` 没有对应字段，在此路径不会传递。
4. 以 `objstore::storage::Context::background()` 调用底层 `MemStorage.Open`。底层失败时回减 `Opened`，将其错误文本包装为 `io::Error::other`，并保留 `TotalOpened`。
5. 底层成功时，先用 `ObjectReaderAdapter` 统一 trait，再用 `TrackOpenFileReader` 携带 `Arc::clone(self)`，最终返回 `Box<dyn objectio::Reader>`。
6. 读取、定位和文件大小查询逐层转发。关闭时从外层进入 `TrackOpenFileReader::close`，底层成功后 `Opened` 减一；失败则计数不变且错误向上传播。

## 数据与状态

`Opened` 表示经该包装器成功打开但尚未“成功执行 `Close`”的 reader 数；取消或底层打开失败会回滚它。`TotalOpened` 是单调增加的打开尝试计数，失败也计入。这两个值是测试信号，不是配额控制器。

底层 `MemStorage::Open` 在打开时复制对象快照，并按半开区间 `[start_offset, end_offset)` 限制读取（`pkg/objstore/memstore.rs:136-166`）。`get_file_size`/`file_size` 返回完整对象大小，不是选定范围的长度；迁移测试用 `[1,4)` 读出 `bcd` 同时断言大小仍为 6。

## 依赖与调用关系

- 上游：`lib.rs` 公开重导出本文件符号。全仓 Rust 文本搜索显示，目前直接构造和调用 `TrackOpenMemStorage` 的 Rust 代码只有同 crate 的 `migration_aster_unit_test.rs`；未发现其他 Rust 消费者。
- Go 实际测试用户：`pkg/ingestor/simplesst/iter_test.go` 在多文件、单上游、空文件和中途关闭等场景断言 `Opened`；`pkg/ingestor/globalsort/engine_test.go` 用 `TotalOpened >= 3` 观察重试已进入对象存储，再等待 `Opened == 0` 确认等待下游前已释放 reader。
- 下游：`objstore::memstore::MemStorage::Open` 创建 `ObjectReader`；`objstore::storage::{Storage, ObjectReader, ReaderOption, Context}` 提供底层接口；`storeapi::{Context, ReaderOption}` 和 `objectio::Reader` 提供对外兼容形状；标准库 `Arc` 与原子整数管理共享寿命和计数。
- RustCodeGraph 的文件级索引将 `util.rs` 标为被 9 个文件间接使用，但限定符号的 `callers/callees` 未产生可用符号边；因此本文档只把文本搜索确认的测试写为具体调用者，不将文件级“used by”误解为 `Open` 调用边。

## 错误处理与边界

- 打开前已取消被显式映射为 `io::ErrorKind::Interrupted`，并且当前打开数回滚。迁移测试 `cancelled_open_preserves_cancellation_error_classification` 固定了这一分类。
- 底层 `Open`、`close` 和 `get_file_size` 的 `anyhow` 错误在适配边界通过 `to_string()` 转为 `io::Error::other`，因此保留人类可读消息，不保留原错误的具体类型链。
- 该实现只在调用底层之前采样一次 `ctx.is_cancelled()`，然后使用 background context；打开过程中新发生的取消不会继续传入底层。这与 Go 版直接传递原 `context.Context` 有语义差异。
- `ReaderOption::PrefetchSize` 被丢弃，因为底层选项只有起止偏移。对当前内存测试路径无影响，但不应将此包装器当作完整的预取语义实现。
- 没有 `Drop` 实现：仅丢弃 reader 而不显式 `Close` 不会回减 `Opened`。这是故意暴露资源收尾缺失的测试语义，也意味着调用者必须显式关闭。
- 外层没有“已回减”标志。当前 `MemFileReader::close` 重复调用仍返回成功（`pkg/objstore/memstore.rs:305-315`），因而对同一 `TrackOpenFileReader` 重复 `Close` 会重复减少 `Opened`。现有测试只覆盖单次关闭，调用方不应依赖关闭幂等性。

## 并发与资源生命周期

`MemStorage` 和追踪包装器通过 `Arc` 共享；只要任意 `TrackOpenFileReader` 存活，它持有的 `store: Arc<TrackOpenMemStorage>` 就防止包装器被提前释放。`Opened` 和 `TotalOpened` 所有更新使用 `Ordering::SeqCst`，为多线程测试提供单一全局顺序的计数观察；代价是比较弱的内存序更严格，但这个工具面向测试而非高频生产数据面。

资源状态转换是：打开尝试前两计数加一；打开失败只回滚 `Opened`；打开成功后由返回的 reader 承担回减责任；底层关闭失败则保持“仍未确认释放”状态。没有后台任务、通道、锁或事务生命周期由本文件管理；底层内存存储的锁与快照细节属于 `objstore::memstore`。

## 与 Go 版本的对应关系

Go 直接对照文件是 [`pkg/ingestor/testutils/util.go`](./util.go)：

- Go `TrackOpenMemStorage` 匿名嵌入 `*objstore.MemStorage`；Rust 改为显式的公开 `Arc<MemStorage>` 字段。Go `atomic.Int32` 的 `Inc`/`Dec`/`Load` 对应 Rust `AtomicI32` 操作。
- 两版 `Open` 都先增加当前数和累计数，打开失败时只回滚当前数，成功时返回携带所属存储的 reader 包装。
- 两版 `Close` 都以“底层先成功，然后回减计数”为不变式；关闭失败不回减。
- Rust 需要 `ObjectReaderAdapter` 弥合当前 `objstore::storage::ObjectReader` 与 `objectio::Reader` 的 trait 边界，Go 通过接口嵌入无需对应类型。
- Rust 额外将已取消映射为 `Interrupted`，但后续使用 background context；Go 将原 context 直接传给 `MemStorage.Open`。Rust 还没有传递 `PrefetchSize`。这两点是现有迁移边界，不应被文档化为完全等价。

## 扩展指南

- 若增加打开阶段的观测项，优先修改 `TrackOpenMemStorage::Open`，并保持“尝试数不回滚、当前数在任何失败路径回滚”的分离语义。
- 若扩展 reader 能力或更换底层 trait，同步检查 `ObjectReaderAdapter` 和 `TrackOpenFileReader` 的 `Read`/`Seek`/`objectio::Reader` 实现，避免某一层遗漏错误或大小语义。
- 若需要完整取消或预取语义，不应只在此文件强行填补；先确认 `objstore::storage::Context/ReaderOption` 的边界是否需扩展，然后在独立上游实现与本适配层同步接线。
- 若要支持幂等关闭或 drop 自动记账，需明确选择“检测未显式关闭”还是“自动释放”的测试契约，并为重复 `Close`、仅 drop、底层关闭失败增加独立回归测试。
- Rust 回归测试应继续放在独立的 `pkg/ingestor/testutils/migration_aster_unit_test.rs`，由 `lib.rs` 的 `#[cfg(test)]` 模块接入，不应内嵌到 `util.rs`。需要保持 Go 语义时，同步对照 `util.go`、`simplesst/iter_test.go` 和 `globalsort/engine_test.go`。
- 性能风险主要来自每次打开/关闭的 `SeqCst` 原子操作、打开时数据快照拷贝和动态分派；此工具不应无评估进入生产热路径。

## 验证依据

- RustCodeGraph 状态：索引包含 11,467 个文件；`files --filter pkg/ingestor/testutils` 列出 `lib.rs`、`util.rs`、`util.go` 和 `migration_aster_unit_test.rs`。
- RustCodeGraph 源码/符号查询：`node --file pkg/ingestor/testutils/util.rs` 读取全部 154 行；`query TrackOpenMemStorage`、`query TrackOpenFileReader`、`query ObjectReaderAdapter` 确认主要类型及 Go/Rust 对照；限定路径的 `callers/callees` 未返回可用边。
- RustCodeGraph 直接依赖源码：`pkg/objstore/objectio/interface.rs:71-90`、`pkg/objstore/storeapi/storage.rs:126-135`、`pkg/objstore/storage.rs:107-132,144-171`、`pkg/objstore/memstore.rs:136-166,268-315`。
- crate 和模块边界：`pkg/ingestor/testutils/Cargo.toml`、`pkg/ingestor/testutils/lib.rs`、`pkg/ingestor/doc.go`。
- Go 对照与真实用法：`pkg/ingestor/testutils/util.go`、`pkg/ingestor/simplesst/iter_test.go`、`pkg/ingestor/globalsort/engine_test.go`。
- Rust 独立测试：`pkg/ingestor/testutils/migration_aster_unit_test.rs` 覆盖范围读取和完整大小、成功关闭计数、缺失文件回滚、取消回滚与 `Interrupted` 分类。
- 本任务为纯文档分析，按计划不运行 Cargo；结构验证以任务文件指定的 11 章节命令为准。
