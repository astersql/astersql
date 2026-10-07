# `pkg/objstore/objectio/interface.rs`

## 文件定位

本文件定义 `astersql-objstore-objectio` crate 的最小流式对象 I/O 契约。模块入口 [`lib.rs`](lib.rs) 将这里的 `Context`、`Reader`、`Writer`、`IOWriter` 和 `NewIOWriter` 全部公开重导出，因此对象存储后端与上层调用者通常通过 crate 根使用它们，而不直接引用私有 `interface` 模块。crate 边界由 [`Cargo.toml`](Cargo.toml) 声明；本文件自身只直接依赖标准库和 Tokio 的时间能力。

它位于 `storeapi::Storage` 与具体后端之间：[`../storeapi/storage.rs`](../storeapi/storage.rs) 的 `Open`/`Create` 一类接口以 `Box<dyn objectio::Reader>`、`Box<dyn objectio::Writer>` 交付流对象，GCS、Azure、S3、OSS、本地文件和压缩包装等实现再履行本文件定义的契约。本文件不选择后端、不创建对象，也不实现分块上传策略。

## 核心职责

1. `Context` 把可克隆的取消状态带到对象存储操作中，让同步路径通过 `check` 快速失败、异步远程请求通过 `wait_cancelled` 参与等待竞争。
2. `Reader` 在标准 `Read + Seek` 之上补充显式 `close` 和 `file_size`，对应 Go 的 `io.ReadSeekCloser` 加 `GetFileSize`。
3. `Writer` 规定每次写入都显式接收取消上下文，并把“写入可能上传满块、关闭负责尾块及最终提交”的生命周期交给实现者。
4. `IOWriter`/`NewIOWriter` 将带 `Context` 参数的 `Writer` 临时适配为标准库 `io::Write`，供只认识标准写接口的编码器使用。

这些接口是行为边界而非完整实现：错误、短写、关闭幂等性、分块和远程资源清理由具体 `Reader`/`Writer` 实现决定。

## 主要符号

- `Context { cancelled: Arc<AtomicBool> }`：共享取消令牌。`Clone` 只克隆 `Arc`，所以所有副本观察同一标志；`Default` 创建未取消的新令牌。
- `Context::from_cancellation_flag`：接入上层已有的 `Arc<AtomicBool>`，避免复制一套互不相干的取消状态。
- `Context::cancel` / `is_cancelled` / `check`：分别使用 Release 写、Acquire 读，并把已取消状态映射为 `io::ErrorKind::Interrupted`、消息 `operation cancelled`。
- `Context::wait_cancelled`：异步轮询取消标志，每次未取消时休眠 10 ms。它不会自行产生错误，调用者通常在分支胜出后再调用 `check` 取得统一错误。
- `Reader: Read + Seek`：要求实现 `close(&mut self)` 和 `file_size(&self)`；`Close`、`GetFileSize` 是为 Go 命名兼容保留的默认转发方法。
- `Writer`：要求实现 `write(&mut self, &Context, &[u8])` 与 `close(&mut self, &Context)`；`Write`、`Close` 同样只是默认转发别名。
- `IOWriter<'a>`：拥有一个 `Context`，同时以可变借用保存 `&'a mut dyn Writer`；借用期保证适配器存活期间不能从别处并发可变访问该 writer。
- `NewIOWriter`：绑定上下文并构造适配器。名称故意保持 Go 风格；返回具体 `IOWriter`，而 Go 版本返回 `io.Writer` 接口。
- `impl io::Write for IOWriter`：`write` 原样转发底层返回值，`flush` 固定返回 `Ok(())`，且两者都不调用底层 `Writer::close`。

## 执行流程

取消流程为：上层创建 `Context::default()` 或用 `from_cancellation_flag` 绑定共享标志；任务终止方调用 `cancel`；对象后端在进入同步 I/O 前调用 `check`，或在 `tokio::select!` 中同时等待远程 future 和 `wait_cancelled`。例如 [`../azblob.rs`](../azblob.rs) 的 Azure 分块上传在父上下文或组上下文取消时停止等待，并通过 `check` 返回 `Interrupted`。

读取流程由 trait 使用方驱动：`storeapi` 返回 `Box<dyn Reader>`，调用者使用标准 `Read`/`Seek`，必要时查询 `file_size`，最后显式 `close`。例如 Azure `ObjectStoreReader` 的 `close` 丢弃当前范围读取器，`file_size` 返回缓存的对象总长；[`../gcs.rs`](../gcs.rs) 的 `RecordingReader` 则转发关闭和大小查询，同时在标准读/定位操作上记录访问量。

写入流程为：后端返回 `Box<dyn Writer>`；调用者把同一 `Context` 传给每次 `write`；实现可立即缓存、同步上传满块或派生并发任务；最后 `close` 刷出尾块并完成对象。[`writer.rs`](writer.rs) 的 `BufferedWriter` 在缓冲满时调用底层 `Writer::write`，关闭时先上传尾块再调用底层 `close`。

标准写适配流程为：`NewIOWriter(context, &mut writer)` 取得底层 writer 的独占借用；标准 `io::Write::write` 使用已绑定的同一上下文转发；返回的字节数和错误不经改写；`flush` 不做事；丢弃适配器只释放借用，不会提交或关闭对象，因此调用者仍须在适配器离开作用域后显式调用底层 `Writer::close`。

## 数据与状态

`Context` 的唯一状态是堆上共享的布尔原子值。状态只从 `false` 变为 `true`，没有重置 API；因此取消是单向且对所有克隆永久可见的。Acquire/Release 配对为取消前后的跨线程观察提供同步边界，但本文件不携带取消原因、截止时间、键值或父子上下文树。

`Reader` 与 `Writer` trait 本身不保存状态；具体实现负责文件位置、缓冲、上传句柄、错误记忆与关闭状态。例如 Azure writer 保存 `closed`、上传任务集合或首个错误，GCS 记录包装器保存累计字节。调用方不能从 trait 契约推断关闭是否幂等，必须遵守“最后调用一次 `close`”的正常生命周期。

`IOWriter` 按值保存 `Context`（其内部仍共享原子标志），按借用保存底层 writer。它没有缓冲，也不记录写入量；状态变化完全发生在绑定的 `Context` 和底层实现中。

## 依赖与调用关系

- crate 内部：[`lib.rs`](lib.rs) 公开重导出本文件符号；[`writer.rs`](writer.rs) 的 `BufferedWriter` 持有并实现 `Writer`。
- crate 边界：[`Cargo.toml`](Cargo.toml) 声明普通依赖 `tokio` 的 `time` feature，本文件用它实现 10 ms 的异步取消轮询；其余普通依赖由同 crate 的 writer/重导出模块使用。
- 上游抽象：[`../storeapi/storage.rs`](../storeapi/storage.rs) 在打开与创建对象时返回 trait object，使后端可互换。
- 典型实现与装饰器：[`../azblob.rs`](../azblob.rs)、[`../gcs.rs`](../gcs.rs)、`../s3like/io.rs`、`../s3store/client.rs`、`../ossstore/client.rs`、`../local.rs`、`../compress.rs` 实现或包装这些 trait。
- `Context` 下游：多个后端在请求前调用 `check`；Azure 与 S3 接口把 `wait_cancelled` 放入 `tokio::select!`，避免只在请求前检查而无法中断在途等待。
- `NewIOWriter`：Rust 生产代码文本检索未发现直接调用，当前直接 Rust 证据来自独立测试；对应 Go 构造函数由 `dumpling/export/writer_util.go` 用于把对象 writer 交给标准写入链。不能据此断言 Rust 侧 Dumpling 已完成同样接线。

RustCodeGraph 将目标文件列为被 11 个文件使用，并识别到 GCS、本地存储、模块入口和测试等用户；对 `NewIOWriter` 精确节点执行 callers/callees 查询时未返回边，因此上述具体接线另以局部 `rg` 结果和源文件核对，不把空图结果解释成“绝无调用”。

## 错误处理与边界

`Context::check` 只在取消时构造 `Interrupted`，未取消时不验证后端状态。`wait_cancelled` 是轮询 future，取消可见性最多受约 10 ms 睡眠粒度影响；它不会自动取消 Tokio 任务或远程 SDK 请求，必须由调用者通过 `select!` 等机制接线。

所有 I/O 错误使用 `io::Result`。默认 Go 风格别名和 `IOWriter::write` 不包装、不吞掉错误，也不把短写补齐；调用标准 `write_all` 时由标准库根据返回值继续写。独立测试证明底层返回 `Ok(1)` 或自定义错误时适配器原样返回。

`Reader` 只约束 `Read + Seek`，未要求 `Send`/`Sync`。`Writer` 同样未要求 `Send`/`Sync`，因此是否能跨线程移动必须由具体装箱类型和外围 API 另行约束。`file_size` 允许失败；压缩读取器当前明确返回“不支持 GetFileSize”的错误，调用者不能假设所有 reader 都能给出大小。

`IOWriter::flush` 是无操作，不能替代底层的分块上传或最终提交；适配器也没有 `Drop` 自动关闭。忘记显式关闭底层 writer 可能留下未提交对象或尾块。底层 `write` 若错误地返回超过输入长度的数值，适配器不会校验，这属于实现者必须维持的标准 `Write` 不变量。

## 并发与资源生命周期

`Context` 可安全克隆并跨任务共享；原子标志避免为取消检查持锁。其轮询实现会周期性唤醒，适合当前轻量取消桥接，但大量长期等待者会产生定时唤醒成本。它不保存 `Waker`，因此没有事件驱动通知。

`IOWriter` 的 `&mut dyn Writer` 表达独占访问，不自行提供并发写。它的生命周期被编译器限制在底层 writer 的可变借用期内；先 `drop` 适配器后才能再次直接使用或关闭底层 writer。适配器丢弃时仅结束借用，不进行 I/O。

实际网络资源生命周期位于实现层。例如 Azure 并发 writer 用 `JoinSet` 等待分块任务、用派生 `Context` 取消同组任务，并在提交前检查记忆的错误；`Reader::close` 的实现可能丢弃文件句柄或范围读取器。扩展本 trait 时必须检查这些实现和装饰器是否仍能完整转发资源语义。

## 与 Go 版本的对应关系

直接对照文件为 [`interface.go`](interface.go)。`Reader` 对应 Go `Reader interface { io.ReadSeekCloser; GetFileSize() }`；Rust 用 `Read + Seek` 超 trait 加显式 `close`/`file_size` 表达同一能力，并提供 `Close`/`GetFileSize` 兼容别名。Rust 不依赖 `Drop` 代替 Go 的显式关闭。

`Writer` 的两个核心方法与 Go `Write(context.Context, []byte)`、`Close(context.Context)` 对齐。Go 使用功能完整的 `context.Context`；Rust `Context` 只移植了本对象 I/O 所需的共享取消能力，没有 deadline、value 或取消原因，这是明确的能力差异。

Go `NewIOWriter` 返回 `io.Writer`，内部 `ioWriter` 绑定 context 并仅实现 `Write`。Rust 返回具体 `IOWriter` 并实现 `io::Write`；两者都只转发写入，不拥有关闭职责。Rust 额外提供固定成功的 `flush`，是满足 `io::Write` trait 的必要方法，不表示远端已刷新。

Go [`writer_test.go`](writer_test.go) 通过存储创建、逐段 `Write`、最后 `Close` 验证对象内容以及压缩包装链；Rust [`interface_test.rs`](interface_test.rs) 更聚焦适配器边界，验证上下文绑定、取消错误、短写和错误透传。两组证据共同确认接口意图，但不应把 Go 后端测试结果当成 Rust 后端已通过测试的替代品。

## 扩展指南

- 新增后端 reader 时，实现标准 `Read`、`Seek` 与本 trait 的 `close`、`file_size`；若大小不可得，应返回明确错误并在独立测试覆盖，而不是伪造零值。
- 新增 writer 时，在可能阻塞或发起远程 I/O 前检查 `Context`；若请求可长时间在途，应像 Azure/S3 路径一样把 `wait_cancelled` 接入异步竞争。确保 `write` 报告真实接受字节数，`close` 刷出尾块、等待必要任务并只在完整成功后提交对象。
- 新增装饰器时完整转发 `close`、`file_size` 和原始错误，并明确它是否改变短写、定位、统计或幂等语义；参考 `RecordingReader`、`RecordingWriter` 和 `BufferedWriter`。
- 若扩展 `Context`，必须保持克隆共享、取消单向与现有 `Interrupted` 映射兼容，并评估所有 `select!` 分支；若改成事件驱动通知，应为多等待者和“先取消后等待”增加独立测试。
- 若修改 `IOWriter`，保持“不隐式关闭底层对象”的所有权边界。需要提交语义时应设计单独的适配器或显式完成方法，不宜让 `flush` 或 `Drop` 悄然改变远端对象状态。
- 测试仍应放在独立文件 [`interface_test.rs`](interface_test.rs)，不要内嵌进生产源文件；后端行为应同步到各自独立测试。重点覆盖未取消/已取消、在途取消、短写、底层错误、显式关闭、不可用文件大小以及资源回收。

## 验证依据

- RustCodeGraph：`status` 显示索引包含目标工程；`files --filter pkg/objstore/objectio` 找到 9 个相关 Go/Rust 文件；`node --file pkg/objstore/objectio/interface.rs --offset 1 --limit 260` 读取了目标文件全部 131 行并报告 11 个使用文件；另读取 `writer.rs`、`gcs.rs`、`azblob.rs` 的直接实现片段。
- RustCodeGraph：查询 `NewIOWriter` 得到 Go 与 Rust 两个精确节点；对 Rust 节点运行 callers、callees、impact 未返回边，故以 `rg` 补充调用/实现证据，并在“依赖与调用关系”中保留该限制。
- 已读生产与声明文件：[`interface.rs`](interface.rs)、[`lib.rs`](lib.rs)、[`Cargo.toml`](Cargo.toml)、[`interface.go`](interface.go)、[`writer.rs`](writer.rs)、[`../storeapi/storage.rs`](../storeapi/storage.rs)，以及 Azure/GCS/S3/OSS/本地/压缩相关的局部实现与引用。
- 已读测试：[`interface_test.rs`](interface_test.rs) 验证适配器上下文、取消、短写、错误和无操作 flush；[`writer_test.go`](writer_test.go) 验证 Go 侧 writer 的写入—关闭生命周期及压缩包装。目标目录不存在 `doc.go`。
- 文档只描述已核实的接口与直接调用证据；未运行 Cargo，符合本任务纯文档且明确禁止 Cargo 的约束。
