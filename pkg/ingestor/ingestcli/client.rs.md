# `pkg/ingestor/ingestcli/client.rs`

## 文件定位

本文件是 `astersql-ingestor-ingestcli` crate 的 HTTP 实现层。crate 根 `pkg/ingestor/ingestcli/lib.rs` 将本文件的公开项重新导出，并用 `interface.rs` 定义的 `Client`、`WriteClient`、`SplitClient`、请求和响应类型约束它。包级说明 `pkg/ingestor/doc.go` 将 `ingestor` 定位为“整理编码 KV、准备 Region、直接导入 SST 到底层存储”的基础设施；本文件负责其中面向下一代 TiKV worker 的最后两段协议：先向 `/write_sst` 流式写 KV 生成 SST，再向 Region leader 所在 store 的 `/ingest_s3` 发起 ingest。

`pkg/ingestor/ingestcli/Cargo.toml` 声明 crate 名为 `astersql-ingestor-ingestcli`，直接依赖 `errdef`（HTTP 状态错误）和 `ingestmetric`（写入/导入耗时指标）；其余 `util`、`logutil`、`redact`、`lightning-common` 依赖目前均为 optional，本文所述文件未使用它们。工作区根 `Cargo.toml` 以 `facade_ingestor_ingestcli` 暴露该 crate，`pkg/ingestor/ingestctrl/Cargo.toml` 也声明了依赖；但仓库搜索只发现 Rust 生产侧的依赖声明与 `pkg/lib.rs` 再导出，没有发现 Rust 生产代码直接构造本文件的 `NewClient`。因此它是已实现、已单测的协议客户端，Rust 生产主链接线状态仍应以实际调用搜索为准，不能仅由 Cargo 依赖推断为已启用。

## 核心职责

1. `StdHttpTransport` 通过 `TcpStream` 实现最小的 HTTP/1.1 客户端：对写入使用 chunked PUT，对 ingest 使用带 `Content-Length` 的 POST，并解析普通或 chunked 响应体。
2. `WriteClientImpl` 将每批 KV 编成 TiKV worker 要求的二进制帧，通过容量为 8 的同步通道交给后台线程，后台线程调用 `/write_sst?cluster_id=...&commit_ts=...`，最后把 JSON 响应还原为 `NextGenSstMeta`。
3. `ClientImpl` 根据配置补齐 worker URL schema，创建写流客户端，并用 `SplitClient` 查询 Region leader store 的 `status_address`，向 `/ingest_s3` 提交 SST 元数据。
4. `Error` 将取消、网络、URL、HTTP、JSON、protobuf、管道和 ingest 业务错误统一到 crate 的错误边界；`observe_write_duration` 与 `IngestDurationGuard` 分别记录写流和 ingest 的耗时。
5. 为避免引入通用 JSON 依赖，`JsonParser` 只实现响应所需的 JSON 子集，同时保留 Go `encoding/json` 对缺失字段、未知字段和 `null` 的关键兼容语义。

## 主要符号

- `Error`：本文件的总错误枚举。其 `Io`、`HttpStatus`、`IngestApi` 保留 `source()`；`From` 实现使 I/O、`errdef::HTTPStatusError` 和 `IngestAPIError` 可用 `?` 传播。另有 `WorkerPanicked`、`MissingWriteResponse`、`MissingSstMeta`、`MissingLeader` 覆盖 Rust 所有权与可选值边界。
- `JsonByteSlice(Option<Vec<u8>>)`：把 Go 的 nil/非 nil 字节切片区分映射为 JSON `null` 或整数数组；`NextGenSstMeta` 保存 `id`、键范围、`meta_offset`、`commit_ts`。其 `Display` 通过 `redacted_key` 只暴露键长度。
- `HttpTransport: Send + Sync`：可注入传输抽象，只有 `put_stream` 和 `post` 两个操作。`SharedHttpTransport = Arc<dyn HttpTransport>` 允许 `ClientImpl` 与后台线程共享实现。
- `StdHttpTransport`、`ParsedUrl`、`read_http_response`、`decode_chunked_body`：无第三方 HTTP 库的明文 HTTP 实现。`ParsedUrl::parse` 只接受 `http://`，支持默认 80 端口、显式端口和括号 IPv6。
- `WriteClientImpl`：持有 `SyncSender`、共享完成结果和 `JoinHandle`。`WriteClientImpl::new` 启动请求线程；`write` 编码一批 KV；`finish` 关闭通道、等待线程并取走一次性结果；`recv` 返回 `WriteResponse`；`close` 与 `Drop` 负责兜底收尾。
- `ClientImpl`：保存 URL schema、worker URL、cluster ID、HTTP transport 与 `SharedSplitClient`。`new` 支持注入 HTTP/TLS transport，`new_std` 只提供明文标准实现。
- `NewClient`：Go 风格工厂，返回 `Arc<dyn crate::Client>`；`ClientImpl::write_client` 和 `ClientImpl::ingest` 是对外行为入口。
- `encode_sst_meta`、`encode_json_bytes`、`decode_next_gen_response`：SST 元数据的 JSON 线格式适配。字段名严格使用 `meta-offset` 与 `commit-ts`。
- `JsonValue`、`JsonParser`：内部 JSON AST 和递归下降解析器。字符串内容仅在对象键解析时保留；普通字符串值、布尔值、浮点值只需能被跳过，整数值才供 SST 字段提取。
- `IngestDurationGuard`、`observe_write_duration`：分别以 RAII 和显式函数写入 `IngestAPIDuration`、`WriteAPIDuration` 直方图。

## 执行流程

写入流程：

1. 调用者通过 `ClientImpl::write_client(context, commit_ts)` 创建 `WriteClientImpl`。入口只在创建时检查一次 `RequestContext::is_cancelled()`。
2. `WriteClientImpl::new` 建立容量为 8 的 `sync_channel`，拼出 `/write_sst?cluster_id={cluster_id}&commit_ts={commit_ts}`，启动后台线程执行 `send_write_request`。
3. 每次 `write(request)` 遍历 `request.pairs`，按 `u16` 小端 key 长度、key 字节、`u32` 小端 value 长度、value 字节的顺序追加到单个 buffer，再发送到同步通道。长度转换使用 `as`，调用方必须保证 key/value 不超过协议字段可表达范围。
4. 默认 transport 在后台建立 TCP 连接，发送 `Transfer-Encoding: chunked` 请求；通道中的每个 buffer 成为一个 HTTP chunk。通道关闭后写入零长度终止块。
5. `recv()` 调用 `finish()`：先丢弃 sender 使后台消费结束，再 join worker，随后从共享 `outcome` 中取出结果。成功响应必须是 HTTP 200，并由 `decode_next_gen_response` 解析 `sst_meta`；结果包装成 `WriteResponse`。
6. `close()` 或对象析构同样调用 `finish()`，但忽略错误，用于保证后台线程和通道不泄漏。

ingest 流程：

1. `ClientImpl::ingest` 先检查取消，再要求 `request.region.leader` 存在。
2. 用 leader 的 `store_id` 调用 `SplitClient::get_store`，从返回的 `Store.status_address` 构造 `{schema}{address}/ingest_s3`；query 参数带 cluster ID、Region ID 和 Region epoch version，epoch 缺失时 version 为 0。
3. 要求 `WriteResponse.next_gen_sst_meta` 存在，经 `encode_sst_meta` 生成 JSON 数组线格式，启动 `IngestDurationGuard` 后调用 transport `post`。
4. HTTP 200 直接成功。非 200 响应先由 `ingest_err::decode_error_pb` 解码 `errorpb.Error`，再把 SST ID 附加到 message，最后经 `NewIngestAPIError` 分类为可供上层重试/诊断的业务错误。

Go 生产链可在 `pkg/ingestor/ingestctrl/local.go:newRegionJobWorker` 与 `pkg/ingestor/ingestctrl/job_worker.go:objStoreRegionJobWorker.write/ingest` 复核：next-gen 分支构造 `ingestcli.NewClient`，按批调用 `WriteClient.Write`，`Recv` 得到 SST 元数据，再把结果和 Region 组合后调用 `Ingest`。当前 Rust 侧尚未搜索到同等生产调用点。

## 数据与状态

- `ClientImpl` 是长期共享配置对象；`Arc<dyn Client>`、`Arc<dyn HttpTransport>` 与 `Arc<dyn SplitClient>` 使其可跨线程共享。它不保存单次请求进度。
- `WriteClientImpl` 是一次写流的可变状态机：`sender: Some` 且 `worker: Some` 表示流仍可写；第一次 `finish` 后二者被 `take`，后续 `write` 返回 `ClosedPipe`，后续 `finish` 因 outcome 已被取走而返回 `MissingWriteResponse`。
- `SharedOutcome = Arc<Mutex<Option<Result<NextGenSstMeta, Error>>>>` 是后台线程到前台的单值交接点。后台只写一次，前台只取一次；锁中毒目前用 `expect` 触发 panic，而不是转换为 `Error`。
- 容量为 8 的 `sync_channel` 提供背压：网络消费者落后时，第九个未消费 buffer 起会阻塞 `write`，从而限制待发送批次数，但每个 buffer 的大小仍由调用者请求决定。
- `NextGenSstMeta.smallest/biggest` 的 `Option` 保留 JSON `null`/缺失与空数组的区别。`decode_next_gen_response` 对缺失或 `null` 的 `sst_meta` 返回全零默认结构，对缺失数值字段使用 0，并忽略未知字段，匹配 Go 解码默认值。
- 指标是 `ingestmetric` crate 中的进程级可选直方图。锁成功且指标已初始化时才记录；未初始化或读锁失败不会令业务请求失败。

## 依赖与调用关系

上游接口来自 `pkg/ingestor/ingestcli/interface.rs`：`ClientImpl` 实现 `Client`，`WriteClientImpl` 实现 `WriteClient`，`ingest` 通过 `SplitClient` 查询 store。`pkg/ingestor/ingestcli/lib.rs` 将本文件公开项与接口、ingest 错误分类一并再导出。

文件内部的关键调用边：

- `NewClient -> ClientImpl::new`；`ClientImpl::write_client -> WriteClientImpl::new`。
- `WriteClientImpl::new -> thread::spawn -> send_write_request -> HttpTransport::put_stream -> decode_next_gen_response`。
- `WriteClientImpl::write -> SyncSender::send`；发送端断开时 `cause_closed_pipe -> finish`，优先返回后台请求的真实错误。
- `ClientImpl::ingest -> SplitClient::get_store -> encode_sst_meta -> HttpTransport::post`；非 200 时继续到 `ingest_err::decode_error_pb -> NewIngestAPIError`。
- `StdHttpTransport::{put_stream,post} -> ParsedUrl::{parse,connect} -> read_http_response`；chunked 响应再进入 `decode_chunked_body`。

RustCodeGraph 将 `client.rs` 标记为被 43 个文件使用，并能精确列出本文件 96 个节点；但本次 `explore/callers/callees` 查询在 30 秒窗口内没有返回结果。为避免把文件级索引关系误写成真实调用边，上述跨文件结论又用精确源码搜索核验：Rust 侧直接行为调用集中在 `client_test.rs`，mock crate 实现相同 trait，生产侧目前只有工作区再导出和 `ingestctrl` Cargo 依赖；Go 侧则存在上述完整生产调用链。

## 错误处理与边界

- URL 无 `://`、authority 为空、端口非法、括号 IPv6 不闭合分别产生 `InvalidUrl`；非 `http` schema 产生 `UnsupportedScheme`。因此 `ClientImpl::new(..., is_https = true, StdHttpTransport, ...)` 虽会生成 `https://` URL，实际请求必然失败；HTTPS 必须注入能够处理该 schema 的自定义 `HttpTransport`。
- `read_http_response` 读取到 EOF 后才解析，要求存在 `\r\n\r\n` 和可解析状态码；它不校验 HTTP 版本、不按 `Content-Length` 截断，也不处理连接复用。chunked 解码支持 chunk extension，但零长度 chunk 后立即返回，不解析 trailer。
- 写接口用 `u16`/`u32` 表示长度，却在转换前没有范围检查；超长 key/value 会截断长度字段而仍附上完整字节，这是扩展或接入不可信输入时必须优先修复并回归的协议风险。
- 请求取消只在 `write_client`/`ingest` 入口检查。后台 TCP 连接、通道阻塞、响应读取与 POST 过程中不会再次观察 context，也没有显式超时；默认 transport 可能长期阻塞。
- 后台线程 panic 映射为 `WorkerPanicked`；共享结果缺失映射为 `MissingWriteResponse`。但 outcome mutex 中毒会在后台写入或前台读取时 panic。
- `/write_sst` 非 200 被包装为 `HTTPStatusError` 并保留响应文本；`/ingest_s3` 非 200 必须是可解码的 protobuf，否则转换成带原状态码的 `HTTPStatusError`。成功状态严格等于 200，其他 2xx 也视为失败。
- `ingest` 在 leader、SST meta 缺失时分别返回 `MissingLeader`、`MissingSstMeta`。Region epoch 缺失不会失败，而是发送 version 0。
- 手写 JSON 解析器支持对象、数组、字符串转义、整数、浮点、布尔和 null；`\\uXXXX` 只接受单个 Unicode scalar，不组合 UTF-16 surrogate pair。被消费的 SST 数值字段必须是整数并能转换到目标 Rust 类型，字节数组元素必须落在 `u8` 范围。

## 并发与资源生命周期

每个 `WriteClientImpl` 恰好创建一个 OS 线程。发送端与后台 receiver 通过有界同步通道通信，`recv`、`close` 或 `Drop` 丢弃 sender 后，receiver 迭代结束，transport 写入 HTTP 终止 chunk 并等待响应。`finish` join 线程后才访问结果，因此正常路径没有“线程尚未写 outcome”的竞态。

错误路径也依赖此所有权顺序：若 receiver 已关闭导致 `send` 失败，`cause_closed_pipe` 会调用 `finish`，等待 worker 完成后优先返回 worker 的网络/HTTP/JSON 错误；只有后台看似成功时才返回 `ClosedPipe`。`close` 和 `Drop` 设计为幂等式清理，但第二次 `finish` 会内部得到 `MissingWriteResponse`，随后被调用者忽略。

资源方面，`StdHttpTransport` 每个请求新建 `TcpStream` 并发送 `Connection: close`；没有连接池。`read_http_response` 将完整响应读入内存，JSON 解析也构造完整树。写请求通过 channel 分批，不聚合全部写入体，但单个 `WriteRequest` 会先编码为完整 `Vec<u8>`。指标 guard 覆盖 ingest 的 transport 调用与错误解码，写指标覆盖整个后台 PUT 请求。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/ingestor/ingestcli/client.go`，独立测试是 `client_test.go` 与 `client_test.rs`。

- Go `jsonByteSlice.MarshalJSON` 对应 `JsonByteSlice`/`encode_json_bytes`；两者都把字节编码为 JSON 整数数组，nil 编码为 `null`。
- Go `nextGenSSTMeta` 对应 `NextGenSstMeta`，字段和 JSON 名一致；Rust 的 `Display` 用长度脱敏代替 Go `redact.Key`，仍避免输出原始 key。
- Go `writeClient` 的 `io.Pipe + WaitGroupWrapper + atomic.Error` 对应 Rust 的 `sync_channel + JoinHandle + SharedOutcome`。两者都异步发送 chunked PUT、在 `Recv/Close` 关闭写端并等待请求结束，也都让服务端错误主要在 `Recv` 暴露。
- Go 用 `http.Client` 同时承担 HTTP/TLS、context、超时和连接管理；Rust 把这些能力抽象成 `HttpTransport`，默认 `StdHttpTransport` 只有明文 TCP。TLS 与请求期取消必须由注入 transport 补足。
- Go 用标准 `encoding/json`；Rust 使用本文件的精简解析器。Rust 测试额外固定了 Go 兼容点：缺失 `sst_meta` 或字段得到零值，未知布尔/浮点字段被忽略。
- Go `client.Ingest` 与 Rust `ClientImpl::ingest` 都经 split client 找 leader store、构造同一 endpoint、序列化 SST meta、解析非 200 的 `errorpb.Error` 并追加 SST ID。Rust 对缺 leader、缺 meta、缺 epoch 做了显式可选值处理，避免 Go 指针解引用式 panic；epoch 缺失取 0。
- Go 测试用真实 `httptest.Server`；Rust 测试注入进程内 `MockTransport`，验证 URL 和完整 body，但没有覆盖 `StdHttpTransport` 的 socket、HTTP 响应解析与 TLS 行为。因此两组测试表达的业务协议相近，传输实现覆盖面并不等价。

## 扩展指南

- 新增或调整 worker 线协议时，优先修改 `WriteClientImpl::write`、`send_write_request`、`encode_sst_meta`/`decode_next_gen_response`，并同步独立测试 `pkg/ingestor/ingestcli/client_test.rs`；不得把测试嵌回生产文件。还应对照 `client.go` 与 `client_test.go`，除非变更明确只属于 Rust。
- 若增加 TLS、超时、代理、认证或连接池，推荐实现新的 `HttpTransport` 并通过 `ClientImpl::new` 注入；不要让 `StdHttpTransport` 宣称支持尚未实现的 `https://`。若扩展默认实现，需为真实 socket 请求、证书与取消路径增加独立测试。
- 若改变 KV 帧格式，必须保留小端序与服务端字段宽度契约，并先为 key 超过 `u16::MAX`、value 超过 `u32::MAX` 定义显式错误，避免当前截断风险；同步验证多 pair、多批和空批行为。
- 若增强 JSON 兼容性，修改 `JsonParser` 时应增加畸形 chunk/JSON、UTF-8、surrogate、数字溢出、未知嵌套字段测试，保持缺失字段零值和未知字段忽略语义。
- 若改变生命周期或重试，重点审查 `finish`、`cause_closed_pipe`、`Drop` 与 channel 容量，避免重复取 outcome、线程泄漏或在析构时永久阻塞；需要独立并发测试覆盖 worker panic、transport 提前关闭、重复 `recv/close`。
- 若把该客户端接入 Rust 生产主链，应在 `pkg/ingestor/ingestctrl` 中复刻而非简化 Go 的 `objStoreRegionJobWorker` 行为：分批写、错误路径 close、`Recv` 结果传给 ingest、Region/commit TS 约束与重试分类都需保留。当前只有 Cargo 依赖不能作为已接线证据。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/ingestor/ingestcli` 找到 16 个相关文件；`node --file pkg/ingestor/ingestcli/client.rs --offset 1/261/601` 完整读取 936 行并报告该文件有 96 个节点、被 43 个文件使用。`query` 唯一定位 `WriteClientImpl`、`send_write_request`、`decode_next_gen_response`，并在多候选中定位本文件的 `NewClient`。`explore/callers/callees` 在 30 秒窗口无输出，因此未把缺失的图结果当成事实，转用精确搜索核验调用关系。
- 源码与边界：完整读取 `pkg/ingestor/ingestcli/client.rs`；读取模块入口 `pkg/ingestor/ingestcli/lib.rs` 与公共接口 `pkg/ingestor/ingestcli/interface.rs`；按仓库预检要求读取最近的包契约 `pkg/ingestor/doc.go`。
- crate 与接线：读取 `pkg/ingestor/ingestcli/Cargo.toml`；搜索工作区根 `Cargo.toml`、`pkg/lib.rs`、`pkg/ingestor/ingestctrl/Cargo.toml` 以及 Rust 源文件中的 crate/API 引用，确认公开与依赖关系，并明确记录 Rust 生产调用点未发现的验证限度。
- Go 对照与生产链：完整读取 `pkg/ingestor/ingestcli/client.go`、`pkg/ingestor/ingestcli/client_test.go`；读取 `pkg/ingestor/ingestctrl/local.go:newRegionJobWorker` 和 `pkg/ingestor/ingestctrl/job_worker.go:objStoreRegionJobWorker.write/ingest`，核对构造、分批写、Recv 和 ingest 的真实链路。
- Rust 测试：完整读取 `pkg/ingestor/ingestcli/client_test.rs`。用例覆盖字节数组 JSON、Go 零值/未知字段兼容、KV 帧、延迟暴露 HTTP 500、两类耗时指标、成功/失败 ingest、SST ID 注释和 URL schema 补全；未运行 Cargo，符合本任务纯文档约束。
- 人工复核：本文区分默认 transport 与注入 transport、Rust 已实现 API 与当前生产接线、业务协议测试与真实 socket 覆盖，不以设计预期替代代码事实；扩展建议指向独立测试文件。
