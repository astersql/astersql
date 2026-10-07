# `pkg/inference/embed_fn.rs`

## 文件定位

本文件是 `astersql-inference` crate 中面向 Domain 的嵌入运行时核心。`pkg/inference/lib.rs` 将模块公开，并重导出 `EmbedFn`、`Embedder` 和 `Options`；crate 边界及依赖见 `pkg/inference/Cargo.toml`。它位于 SQL `EMBED_TEXT()` 求值与具体供应商实现之间：`pkg/domain/domain.rs::init_inference_providers` 创建一个 Domain 级 `EmbedFn` 并注册供应商，`pkg/expression/builtin_inference.rs::EvalEmbedTextArgsToDatum` 再从会话取得该运行时并调用 `embed_with_context_values`。

该文件不是具体 HTTP 供应商实现。OpenAI、Jina、Cohere、HuggingFace、NVIDIA、Gemini 和 TiDB Cloud 的协议与请求逻辑位于同 crate 的对应模块；本文件负责统一的注册、请求合并、短时批处理、单文本缓存、取消和关闭生命周期。

## 核心职责

- 以 `Embedder` trait 定义供应商边界，并为只实现基础 `create_embeddings` 的供应商提供上下文值和 `ProviderContext` 兼容适配。
- 解析 `provider/model`，查找 Domain 启动阶段注册的供应商，并按供应商、模型和选项划分批次（`EmbedFn::request`、`new_batch_key`）。
- 对单文本 `embed*` 调用执行结果缓存和同键在途请求合并；缓存键包含完整模型名、文本、选项及 `config_version`（`embed_with_context_values`）。
- 将兼容的并发调用在默认 100ms 窗口内合并，达到默认 16 条文本时立即唤醒工作线程；超大调用在工作线程中继续按上限切块（`Batch`、`run_batch`）。
- 保证每个等待者独立取消，仅当一个批次所有有效等待者都离开或运行时关闭时取消供应商请求（`release_waiter`、`close`）。
- 校验供应商返回数量，并限制单文本 SQL 向量不超过 16,383 维；只有有效、仍有等待者且运行时未关闭的单文本结果才可进入缓存（`run_batch`、`embed_with_context_values`）。

## 主要符号

- `ContextValues = BTreeMap<String, Arc<dyn Any + Send + Sync>>`：跨供应商边界传递追踪等请求值。批次保留创建该批次的首个调用者快照。
- `Options = BTreeMap<String, serde_json::Value>`：稳定排序的供应商选项；用于批次相等判断、SHA-256 摘要和缓存键序列化。
- `Embedder: Send + Sync`：供应商接口。必需方法是 `create_embeddings`；`create_embeddings_with_values` 默认忽略附加值，`create_embeddings_with_context` 将字符串错误适配成 `ProviderError` 并保留取消原因。
- `Call`：一个逻辑调用的自有文本、是否可缓存、最终结果、完成条件变量、等待者计数、取消标志及所属批次弱引用。`Call::new` 从一个等待者开始。
- `Batch`：一次可共享供应商调用的上下文、供应商、模型、选项、待执行调用列表、提前刷新条件变量及供应商级取消标志。
- `State`：由 `EmbedFn::state` 单锁保护的供应商表、缓存/FIFO 顺序、在途调用、活跃批次、工作线程句柄和关闭标志。
- `EmbedFn`：公开运行时。`new` 使用 `CACHE_CAPACITY=10_000`、`BATCH_WINDOW=100ms`、`MAX_BATCH_SIZE=16`；`new_with_config` 允许测试或调用方覆盖窗口和上限，零值回退默认值。
- `register` / `has_embedder`：规范化供应商名并管理注册。空名、含 `/`、重复注册，以及运行时关闭或已经启动任务后的注册都会失败。
- `embed`、`embed_with_context`、`embed_with_context_values`：单文本、可缓存入口，后两者分别保留字符串取消原因和上下文值。
- `create_embeddings`：多文本入口；空输入立即返回空结果，其余请求使用递增唯一键，因此不使用运行时结果缓存或跨调用同键合并，但仍参与兼容批处理。
- `request`：共同的获取缓存、复用/创建 `Call`、装入/创建 `Batch`、等待结果和处理调用者取消的主流程。
- `run_batch`：工作线程入口；移除批次、过滤无等待者调用、切块调用供应商、校验结果、拆回调用边界并发布结果/缓存。
- `BatchKey` / `new_batch_key`：以供应商、模型及选项 JSON 的 32 字节 SHA-256 摘要划分批次。
- `close` / `Drop`：幂等关闭、取消批次和调用、唤醒等待者、等待线程并清空缓存；析构时自动执行。
- `poll_cancellation`：安全轮询调用者回调；回调 panic 被转换为 `context canceled`。

## 执行流程

1. Domain 初始化时调用 `EmbedFn::new`，逐一 `register` 供应商，并以 `set_config_version` 对齐当前配置版本；见 `pkg/domain/domain.rs::init_inference_providers`。
2. SQL 求值路径 `EvalEmbedTextArgsToDatum` 从 session 取得 Domain 运行时、取消原因及上下文值，调用 `embed_with_context_values`，随后把 `Vec<f32>` 转为 SQL vector datum。
3. 单文本入口读取配置版本，将 `(model_with_provider, text, opts, version)` 序列化为缓存/在途键，再交给 `request`。多文本入口则分配 `batch-call:<序号>` 唯一键并标记不可缓存。
4. `request` 先检查取消，再拆分并规范化 `provider/model`，克隆选项并计算 `BatchKey`。持有 `State` 锁期间依次检查关闭状态、缓存、同键在途调用和供应商表。
5. 新调用寻找尚未取消、选项一致且当前文本数未满的批次；不存在时创建 `Batch` 和工作线程。线程等待批处理窗口，或者在累计文本数达到上限时由条件变量提前唤醒。
6. 调用线程每 10ms 等待 `Call::done`，以便轮询自己的取消回调。取消者只释放自己的 waiter；最后一个 waiter 才将调用标记取消，并在批次所有调用都无人等待时取消整个供应商请求。
7. `run_batch` 从活跃批次表移除自身，过滤已经无人等待的调用，将文本扁平化并按 `max_batch_size` 分块调用 `Embedder::create_embeddings_with_values`。供应商 panic 被捕获并统一转为错误。
8. 返回数量必须与分块文本数一致。成功结果按原调用文本数切分；每个 `Call` 获得独立 `Vec<Vec<f32>>`，然后通知所有等待者。符合条件的单文本结果写入 FIFO 容量缓存。
9. 单文本入口取第一条向量并执行 16,383 维上限检查。关闭时所有批次被唤醒/取消、未完成调用收到关闭错误，线程被 join，缓存被清空。

## 数据与状态

`EmbedFn::state` 是共享可变状态的中心，所有表结构在同一个 `Mutex<State>` 下修改，避免缓存、在途表和批次表之间出现跨锁提交窗口。`Call::result`、`Batch::calls` 和刷新标志另有细粒度锁/条件变量，使等待者不必长期持有全局状态锁。原子字段分别承担高频计数/标志：`waiters` 维护仍关心结果的调用者数，`cancelled` 传递供应商取消，`config_version` 隔离不同配置世代的缓存，`next_call` 为不可缓存调用生成唯一键。

缓存是 `HashMap<String, Vec<f32>>` 加 `VecDeque<String>` 的容量型 FIFO，而非按访问更新的 LRU。新键追加到队尾，超过 10,000 项时从队首移除；缓存读取和返回都会克隆向量，调用方修改结果不会污染缓存。`set_config_version` 不主动清空旧项，而是使后续单文本键带上新版本；旧项会留到 FIFO 淘汰或 `close` 清空，这是明确的空间换取低同步成本的行为。

批次只在 `BatchKey` 相同且 `Options` 完全相等时共享；键摘要固定为 32 字节，但碰撞仍由选项相等判断兜底。供应商名会 trim 并转小写，模型只 trim、不改大小写。`context_values` 和选项均在入队时拥有快照；同一批次实际传给供应商的是创建该批次的首个调用者上下文值。

## 依赖与调用关系

上游主链为 `pkg/domain/domain.rs::init_inference_providers` → `EmbedFn::register`，以及 `pkg/expression/builtin_inference.rs::EvalEmbedTextArgsToDatum` → `EmbedFn::embed_with_context_values`。`pkg/expression/expropt/sessioncontext.rs::SessionContext` 和 `pkg/expression/sessionexpr/sessionctx.rs::SessionContext` 定义运行时、取消原因及上下文值的会话边界。Domain 的配置更新路径调用 `set_config_version`，关闭路径 `close_inference_providers` 调用 `EmbedFn::close`。

下游由 `run_batch` 通过 `Arc<dyn Embedder>` 调用各供应商。`pkg/inference/lib.rs` 暴露本模块；具体实现位于 `openai.rs`、`jina.rs`、`cohere.rs`、`huggingface.rs`、`nvidia.rs`、`gemini.rs`、`tidbcloud.rs` 和 `mock.rs`。本文件直接使用标准库线程/同步原语、`serde_json` 序列化及 `sha2::Sha256`；后两者在 `pkg/inference/Cargo.toml` 明确声明。该文件不直接发起网络请求，也不依赖 Tokio 调度。

RustCodeGraph 的文件节点显示本文件被 Domain、表达式会话上下文、表达式推理测试等多个文件使用；图的独立 callers/callees 命令本次未在超时内返回明细，因此具体边以上述源码入口为准。

## 错误处理与边界

- 输入边界：空多文本请求在解析模型和检查供应商前返回空结果；非空请求必须是 `provider/model` 格式。供应商名无效、未知或重复均返回含上下文的字符串错误，未知供应商列表会排序以保证确定性。
- 选项边界：缓存键和批次键的 JSON 序列化错误向上传播；`Options` 限定为 `serde_json::Value`，避免 Go 任意值深拷贝和类型签名的复杂度。
- 供应商边界：返回零条或返回条数与文本数不符会使批次内所有活跃调用得到同一错误；供应商 panic 被 `catch_unwind` 转为 `embedding batch processing panicked`，不会遗留永久等待者。
- 向量边界：单文本结果超过 16,383 维时返回错误且不缓存。`run_batch` 在访问 `embedding[0]` 前依赖“返回数量与非空 chunk 一致”的校验，因此正常成功路径至少有一条结果。
- 取消优先级：入口、缓存命中后及等待循环都会检查调用者取消；自定义原因原样返回。取消回调 panic 视为取消。一个等待者取消不破坏共享请求，最后一个等待者取消才传播到批次供应商。
- 关闭边界：`close` 幂等；关闭后新请求与注册失败。关闭把尚无结果的调用设为关闭错误并通知等待者，同时取消并 join 工作线程。
- 中毒锁使用 `expect`，因此内部线程持锁 panic 造成的 mutex poisoning 会继续 panic；该策略将内部不变量破坏视为不可恢复编程错误，而非供应商错误。

## 并发与资源生命周期

`EmbedFn` 通过 `Arc<Mutex<State>>` 可被多会话/线程共享。每个新批次创建一个 `std::thread::JoinHandle`；完成句柄会在后续创建批次时用 `is_finished` 清理，剩余句柄在 `close` 中统一取出并 join。批次窗口由 `Condvar::wait_timeout_while` 实现，满批、最后等待者取消和关闭都可提前唤醒。

等待者数量是不变量核心：创建时为 1，复用同键在途调用时递增，每个返回或取消路径恰好调用一次 `release_waiter`。结果已经发布时，最后一个释放者不再触发取消；结果未发布时，会将 `Call` 标记取消并在必要时取消整个 `Batch`。`Weak<Batch>` 避免 `Call` 与 `Batch` 形成强引用环。

供应商调用不持有全局 `State` 锁，避免慢网络请求阻塞注册表/缓存操作；发布结果时才重新加锁。`close` 先在锁内改变状态和发出取消/通知，再在锁外 join，避免工作线程完成时与关闭线程互相等待。`Drop` 调用 `close`，但生产生命周期仍由 Domain 显式关闭，以便在资源销毁前确定性等待供应商线程。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/inference/sqlembed.go`，相关 Go 测试是 `pkg/inference/sqlembed_test.go`。两版共同语义包括：Domain 所有、供应商注册与查找、相同单文本在途请求共享、调用者独立取消、最后等待者取消供应商、缓存结果克隆、配置版本隔离缓存、维度校验和关闭等待在途工作。

结构上并非逐字段复刻。Go `EmbedFn` 把跨文本批处理委托给 `embedding/batcher.Batch`，缓存使用 Ristretto，并用 `context.Context`/goroutine/`WaitGroupWrapper` 管理生命周期；Rust 在本文件中以内建 `Batch`、FIFO 容量缓存、线程、条件变量和原子标志实现同一类行为。Rust 的供应商由 Domain 显式注册，而 Go `NewEmbedFn` 自行注册标准供应商。

选项模型也不同：Go 接受 `map[string]any`，需深拷贝并把运行时类型签名纳入缓存键；Rust 使用 `BTreeMap<String, serde_json::Value>`，天然拥有克隆快照和确定性键顺序，以 JSON 值差异区分选项。Rust 额外公开多文本 `create_embeddings` 并在此文件内负责批次切块。因这些表示与组件边界不同，扩展时应对齐可观察行为和测试意图，不应机械复制 Go 内部字段。

## 扩展指南

- 新增供应商时实现 `Embedder`，在 `pkg/domain/domain.rs::init_inference_providers` 的启动阶段注册，并在同目录独立 `*_test.rs` 中验证错误、取消和选项；不要把测试嵌入本源文件。
- 若供应商需要追踪或请求元数据，覆盖 `create_embeddings_with_values` 或通过 `create_embeddings_with_context` 适配，保持“首个批次调用者的值被保留”语义，并扩展 `embed_fn_test.rs::shared_embedding_call_keeps_first_context_values_and_cancellation_cause` 一类测试。
- 修改批次兼容规则时集中调整 `new_batch_key` 和 `request` 的二次相等检查，同时覆盖供应商、模型、选项值/数值类型、窗口关闭及摘要固定长度；错误地放宽规则可能把不兼容请求发送给同一次供应商调用。
- 修改缓存键或配置失效时同时检查 `embed_with_context_values`、`set_config_version` 和 `run_batch` 的写入条件。必须保留文本、模型、完整选项和配置世代隔离，并评估旧世代项驻留造成的内存风险。
- 修改取消或关闭时首先维护 waiter 计数、只在最后等待者离开时取消，以及锁外 join 三项不变量；同步扩展 `batch_provider_cancels_only_after_all_callers_cancel`、`batch_cancellation_filters_private_texts_and_skips_empty_batches` 和关闭测试。
- 修改批量大小或返回拆分时保留调用者边界、输入顺序、每个供应商分块不超过上限及返回数量校验；对应测试为 `batch_multi_text_chunks_preserve_caller_boundaries_and_isolation` 和 `batch_exact_limit_and_large_single_request_dispatch_immediately`。
- 性能上重点观察每批一个 OS 线程、10ms 取消轮询、全局状态锁和向量克隆；任何优化都需维持取消优先级、错误广播和 Domain 关闭的确定性。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter pkg/inference` 确认目标及相邻文件；`node --file pkg/inference/embed_fn.rs --offset 1/390` 读取全部 636 行并给出 Domain/表达式相关使用者；`query EmbedFn/run_batch/new_batch_key` 定位主要符号。独立 callers/callees 查询超时无输出，未据此臆造调用边。
- 源码：完整阅读 `pkg/inference/embed_fn.rs`；读取 `pkg/inference/lib.rs`、`pkg/inference/Cargo.toml`、`pkg/domain/domain.rs`、`pkg/expression/builtin_inference.rs`、`pkg/expression/expropt/sessioncontext.rs`、`pkg/expression/sessionexpr/sessionctx.rs` 的直接接线。
- Rust 测试：`pkg/inference/embed_fn_test.rs` 覆盖共享/缓存、分批与切块、分区、输入快照、取消、关闭、供应商 panic/错误/数量异常、注册验证、选项摘要、上下文值与取消原因。
- Go 对照：读取 `pkg/inference/sqlembed.go` 的 `EmbedFn`、`EmbedWithContext`、获取/完成/释放调用、缓存键及 `Close`，并读取 `pkg/inference/sqlembed_test.go` 的供应商错误、缓存失效、共享取消和关闭场景。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务指定命令验证本文恰有 11 个固定二级章节，并人工复核所有运行时结论均可追溯到上述符号或测试。
