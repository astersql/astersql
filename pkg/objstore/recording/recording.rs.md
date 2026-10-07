# `pkg/objstore/recording/recording.rs`

## 文件定位

本文件是 `astersql-objstore-recording` crate 的核心实现，提供对象存储访问统计的内存计数器。crate 入口 [`lib.rs`](lib.rs) 公开 `recording` 模块并再导出本文件的全部公开类型；[`Cargo.toml`](Cargo.toml) 表明该 crate 只直接依赖 `http = "1"`，并以 `pkg/objstore/recording` 为 Go 移植来源。

它不执行对象存储 I/O，也不负责上报指标，而是位于 I/O 实现与计量消费者之间：S3/OSS 接口调用 `AccessStats::rec_request`，reader/writer 调用 `rec_read`、`rec_write`，上层计量组件读取或合并累计值。源码入口见 [`recording.rs`](recording.rs)。

## 核心职责

1. 用 `Requests` 将 HTTP `GET`、`HEAD` 归入读请求，将 `PUT`、`POST` 归入写请求；其他方法当前忽略。
2. 用 `Traffic` 分别累计从对象存储读出和写入对象存储的字节数。
3. 用 `AccessStats` 组合上述两组计数，并提供可选统计对象上的记录入口，使未配置统计时为 no-op。
4. 提供 `merge`，把另一份统计的当前逐字段快照累加到接收者。
5. 通过 `Display` 保持与 Go 版本一致的稳定文本格式，便于日志、调试和迁移兼容。

本文件只维护累计总量，没有清零、差值计算、持久化、标签、时间窗口或网络上报逻辑。

## 主要符号

- `Requests { get: AtomicU64, put: AtomicU64 }`：公开的读类/写类请求累计值。`snapshot(&self) -> (u64, u64)` 依次读取两个字段；`rec<T>` 是私有分类入口；`merge(&self, other)` 累加另一实例的当前值。
- `impl Display for Requests`：输出 `{get: N, put: N}`。
- `Traffic { read: AtomicU64, write: AtomicU64 }`：公开的读/写字节累计值。本文件没有为它单独定义记录或合并方法，操作由 `AccessStats` 完成。
- `impl Display for Traffic`：输出 `{r: N, w: N}`。
- `AccessStats { requests: Requests, traffic: Traffic }`：对外使用的聚合统计类型。
- `AccessStats::merge(&self, other: &AccessStats)`：依次合并请求计数、读字节和写字节。
- `AccessStats::rec_request<T>(Option<&AccessStats>, Option<&http::Request<T>>)`：记录请求分类；两个 `Option` 分别表达 Go 的 nil 接收者和 nil 请求。
- `AccessStats::rec_read(Option<&AccessStats>, usize)`、`rec_write(...)`：累计成功处理的字节数。
- `impl Display for AccessStats`：组合两个子对象，输出 `{requests: ..., traffic: ...}`。

三个结构均实现 `Default`，初始原子值为零；它们没有实现 `Clone`，共享使用由调用方通过 `Arc<AccessStats>` 完成。

## 执行流程

请求计数流程如下：对象存储适配层持有可选的 `Arc<AccessStats>`，构造仅需携带 HTTP 方法的 `http::Request`，再把 `Arc` 转为 `Option<&AccessStats>` 调用 `rec_request`。当统计对象或请求不存在时立即返回；否则 `Requests::rec` 根据方法对 `get` 或 `put` 执行一次原子加一。真实调用可见 [`../s3store/interface.rs`](../s3store/interface.rs) 的 `record_get`/`record_put` 和 [`../ossstore/interface.rs`](../ossstore/interface.rs) 的 `record`。

流量计数发生在实际数据路径。`BufferedWriter::write` 先执行写入，再以成功返回的实际接收字节数调用 `rec_write`，错误时计零（[`../objectio/writer.rs`](../objectio/writer.rs)）。`S3ObjectReader::read` 在成功读取后以返回的 `n` 调用 `rec_read`，最终失败分支计零（[`../s3like/io.rs`](../s3like/io.rs)）。因此这里累加的是调用方传入的已处理字节，而不是请求声明长度或对象总大小。

聚合时，`AccessStats::merge` 读取来源的四个原子累计值并分别 `fetch_add` 到目标。DXF 的 `Recorder::MergeObjStoreAccess` 使用该入口收集对象存储包装器的统计，`curr_data` 再读取各字段生成计量快照（[`../../dxf/framework/metering/recorder.rs`](../../dxf/framework/metering/recorder.rs)）。

## 数据与状态

全部状态是四个单调累加的 `AtomicU64`：请求维度的 `get`、`put`，流量维度的 `read`、`write`。字段公开，消费者可以直接 `load`，测试和计量代码也确实这样读取。

所有读写使用 `Ordering::Relaxed`。这保证单个原子字段在并发更新时不会发生数据竞争或丢失一次 `fetch_add`，但不建立与对象 I/O 的 happens-before 关系，也不把多个字段组成事务快照。`snapshot`、`merge` 和各 `Display` 实现都逐字段读取；并发更新期间看到的新旧值组合是允许的。

计数没有上限检查、饱和或溢出报告，遵循 `AtomicU64::fetch_add` 的模 $2^{64}$ 累加语义。`rec_read`/`rec_write` 接收 `usize` 后转换为 `u64`；与 Go 的 `int` 入参相比，Rust API 在类型层面排除了负字节数。

## 依赖与调用关系

- crate 内部：[`lib.rs`](lib.rs) 以 `pub use recording::*` 将 `Requests`、`Traffic`、`AccessStats` 暴露到 crate 根。
- 直接外部依赖：标准库的 `fmt` 与 `AtomicU64`，以及 `http` crate 的 `Request`、`Method`。请求正文类型参数 `T` 不被读取，分类只依赖方法。
- 主要上游生产调用者：S3 和 OSS API 包装器记录 GET/PUT 类请求；`objectio::BufferedWriter` 与 `s3like::S3ObjectReader` 记录实际读写字节；DXF metering 的 `Recorder` 合并和读取统计。
- 共享方式：对象存储实现通常保存 `Option<Arc<AccessStats>>`，调用时以 `as_deref()` 借用；本文件自身不创建线程、不持有 `Arc`，也不了解统计对象的所有者。
- RustCodeGraph 的文件关系查询显示本文件被 26 个 Rust 文件使用；精确符号查询确认 `AccessStats`、`rec_request`、`rec_read`、`rec_write` 均定义于本文件。调用方细节由上述直接源码位置交叉核验。

## 错误处理与边界

记录与合并 API 均无 `Result` 返回值，也不产生本地 I/O 错误。`rec_request` 对缺少统计对象、缺少请求以及未识别方法静默 no-op；当前明确忽略包括 `DELETE` 在内的非 GET/HEAD/PUT/POST 方法。

`merge` 要求来源引用有效，不接受 `Option`；它合并调用瞬间逐字段读到的值，而非冻结来源后的统一快照。重复合并同一累计对象会重复计数，调用方必须自行保证合并语义是增量还是只执行一次。把对象与自身合并会按逐字段读取结果扩大当前值，本文件不阻止这种调用。

格式化不会锁定整体快照，因此并发变化时字符串内部字段可能来自不同时间点。请求构造失败等问题发生在上游；例如 S3/OSS 包装器用固定合法方法构造请求并在那里处理 `expect`，不属于本文件的错误边界。

## 并发与资源生命周期

四个计数器均可被多个线程通过共享引用并发更新；[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 使用 `Arc<AccessStats>`、8 个线程和每线程 2,000 次循环，验证四项原子累计不丢更新。`Relaxed` 适用于只关心累计数值、无需借计数同步其他内存状态的场景。

本文件不分配堆资源、不打开连接、不创建任务、锁或通道，也没有显式关闭流程。统计对象的生命周期完全由拥有者管理，通常随持有它的 `Arc` 最后一个引用释放。`Option<&AccessStats>` 只在调用期间借用，不延长对象生命周期；`merge` 也不会保存来源引用。

## 与 Go 版本的对应关系

直接对照文件是 [`recording.go`](recording.go)，结构与行为保持一一对应：`Requests`、`Traffic`、`AccessStats` 对应同名 Go 类型；Rust `Display` 对应 Go `String`；Rust `merge`、`rec_request`、`rec_read`、`rec_write` 对应 Go `Merge`、`RecRequest`、`RecRead`、`RecWrite`。HTTP 分类以及三层文本格式保持一致。

主要语言映射差异如下：

- Go 使用 `atomic.Uint64`，Rust 使用 `AtomicU64` 且明确选择 `Ordering::Relaxed`。
- Go 允许 nil 方法接收者；Rust 无 nil 引用，静态记录函数通过 `Option<&AccessStats>` 显式模拟该 no-op 语义。
- Go 的 nil `*http.Request` 映射为 `Option<&http::Request<T>>`，正文泛型不影响方法分类。
- Go 字段名导出为 `Get/Put/Read/Write`；Rust 使用公开的 snake_case 字段。
- Go `RecRead`/`RecWrite` 接收 `int` 再转 `uint64`；Rust 接收 `usize`，正常 I/O 返回值可直接传入，并避免负数输入。
- Rust 额外提供 `Requests::snapshot`，供安全读取二元计数；它仍不承诺跨字段一致性。

[`recording_test.go`](recording_test.go) 与 [`recording_test.rs`](recording_test.rs) 都验证 nil 请求、GET/HEAD、PUT/POST 和 DELETE 忽略规则；Rust 的迁移补充测试进一步覆盖 nil 统计对象、合并、格式化和并发累加。

## 扩展指南

- 新增或调整 HTTP 方法分类时，修改私有 `Requests::rec`，并同步更新独立的 [`recording_test.rs`](recording_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs) 和 Go 对照测试；不要把 Rust 测试嵌入生产源文件。
- 新增统计维度时，需要同时更新结构字段、`Default` 可用性、`merge`、`Display`、上游记录点和计量快照消费者。尤其检查 DXF `DataValues` 映射以及所有直接读取公开原子字段的位置。
- 若需要一致快照，不能只复用当前逐字段 `Relaxed` 读取；应先定义一致性的实际要求，再评估序列锁、锁或版本化快照，避免无依据地仅提高内存序。
- 若改为记录失败次数、重试次数或按操作标签分类，应在真正知道结果的 I/O 层触发；本文件只接收事实，不应引入对象存储 SDK 或重试策略依赖。
- 保持可选统计的 no-op 特性，可避免未启用计量的调用路径分叉。新增入口应继续接受安全借用，且不得把请求或统计引用保存到调用之后。
- 性能风险主要来自热路径上的额外原子操作和潜在缓存行竞争；兼容风险包括文本格式、公开字段、HTTP 分类和重复合并语义变化。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录查询识别 `recording.rs` 的 14 个符号及其 Rust/Go 测试和对照文件。
- RustCodeGraph `node --file pkg/objstore/recording/recording.rs`：核对完整 171 行实现；文件关系报告 26 个 Rust 使用者。
- RustCodeGraph 精确查询：确认 `AccessStats` 结构及 `rec_request`、`rec_read`、`rec_write` 定义位置；精确 callers 查询未返回细粒度结果，因而用定向源码引用搜索核对上述直接调用边。
- 已读实现与边界文件：[`recording.rs`](recording.rs)、[`Cargo.toml`](Cargo.toml)、[`lib.rs`](lib.rs)、[`recording.go`](recording.go)。目标目录没有 `doc.go`，包级 Rust 装配契约由 `lib.rs` 提供。
- 已读独立测试：[`recording_test.rs`](recording_test.rs)、[`migration_aster_unit_test.rs`](migration_aster_unit_test.rs)、[`recording_test.go`](recording_test.go)。本任务是纯文档分析，按计划未运行 Cargo。
- 已读直接调用证据：[`../s3store/interface.rs`](../s3store/interface.rs)、[`../ossstore/interface.rs`](../ossstore/interface.rs)、[`../objectio/writer.rs`](../objectio/writer.rs)、[`../s3like/io.rs`](../s3like/io.rs)、[`../../dxf/framework/metering/recorder.rs`](../../dxf/framework/metering/recorder.rs)。
- 文档结构以任务指定命令验证，要求目标存在且恰有 11 个固定二级标题；交付前另行检查仅产生指定文档和任务文件删除。
