# `pkg/inference/mock.rs`

## 文件定位

`pkg/inference/mock.rs` 属于 Cargo crate `astersql-inference`（见 `pkg/inference/Cargo.toml`），实现一个供推理运行时和 SQL 测试使用的确定性 embedding provider。模块由 `pkg/inference/lib.rs` 公开为 `pub mod mock`，其中的 `MockEmbedder` 又在 crate 根通过 `pub use mock::MockEmbedder` 再导出，因此调用方可以直接写 `astersql_inference::MockEmbedder`。

它不是线上远程模型客户端：文件不发起网络请求，也不保存服务配置，而是把每段输入文本当作 JSON 浮点向量解析。它的作用是让 `EmbedFn` 的批处理、缓存、取消以及上层 `EMBED_TEXT()` SQL 路径能在没有外部服务的情况下得到可重复结果。RustCodeGraph 显示该文件直接被 `pkg/inference/lib.rs`、`pkg/inference/embed_fn_test.rs`、`pkg/expression/builtin_inference_test.rs` 和 `pkg/session/runtime/inference_test.rs` 使用。

## 核心职责

- `MockEmbedder::create_embeddings` 实现 `Embedder` provider 边界，只接受模型名 `json`，逐个把 `texts` 解码成 `Vec<f32>`。
- 支持测试选项 `plus` 和 `delay`：前者给每个向量元素加同一个偏移量，后者按 Go `time.ParseDuration` 风格的字符串模拟远程调用延迟。
- 拒绝未知模型、未知选项、错误的选项类型、非法 duration、非法 JSON 和不能表示为有限 `f32` 的数值，使错误路径同样可用于上层回归测试。
- 在存在 `delay` 时轮询 `AtomicBool` 取消标志，模拟 Go provider 在等待 timer 时观察 `context.Context` 取消的行为。
- 私有函数 `parse_go_delay` 提供本文件所需的 Go duration 兼容子集；它不是 crate 的通用 duration 解析 API。

## 主要符号

- `pub struct MockEmbedder`：零字段、零大小的 provider 类型，不需要构造函数或实例状态。它通过 crate 根再导出，是本文件唯一公开类型。
- `impl Embedder for MockEmbedder`：实现 trait 必需方法
  `create_embeddings(&self, cancel: &AtomicBool, model: &str, texts: &[String], opts: &Options) -> Result<Vec<Vec<f32>>, String>`。`Embedder` 和 `Options` 定义于 `pkg/inference/embed_fn.rs`；`Options` 是 `BTreeMap<String, serde_json::Value>`，因此选项遍历与序列化具有稳定顺序。
- `fn parse_go_delay(value: &str) -> Option<Duration>`：私有解析器。接受可选正负号、复合片段、最多用于计算的 18 位小数，以及 `ns`、`us`、`µs`、`μs`、`ms`、`s`、`m`、`h` 单位；用 checked arithmetic 和有符号纳秒边界拒绝溢出。
- 文件没有模块级常量、枚举、其他 trait 或条件编译项。

## 执行流程

1. `EmbedFn` 在 `pkg/inference/embed_fn.rs::run_batch` 中把同 provider、model 和 options 的请求聚合，再调用 `batch.provider.create_embeddings_with_values(...)`。trait 的默认实现最终转发到本文件的 `create_embeddings`。
2. `create_embeddings` 首先要求 `model == "json"`，随后遍历全部 option key，仅允许 `plus` 和 `delay`；这些检查发生在解析任何文本之前。
3. 若有 `plus`，从 JSON number 读取为 `f64` 后转换成 `f32`；类型不匹配立即返回错误。缺省偏移量为 `0.0`。
4. 若有 `delay`，先要求它是字符串，再经 `parse_go_delay` 转成 `Duration`。正 duration 以最多 5ms 的短睡眠分段等待，每段前以 Acquire ordering 检查取消；等待结束后还会再检查一次取消，使零值、负值和恰好结束时的取消也返回 `context canceled`。
5. 对每个 `text`，以 `Option<Vec<Option<f64>>>` 解码 JSON：顶层 `null` 变为空向量，数组元素 `null` 变成 `0.0`。每项转换成 `f32` 后必须保持有限，否则返回 `invalid float32 value in embedding`。
6. 若 `plus != 0.0`，原地给该向量每个元素加偏移量，然后按输入顺序收集所有向量。任何一项失败都会终止整个批次并返回首个错误，不返回部分结果。

## 数据与状态

`MockEmbedder` 自身无字段、无缓存、无锁，所有输入都来自一次方法调用。输入文本和 options 只借用不修改；输出是新分配的 `Vec<Vec<f32>>`。`plus` 在读取 JSON number 后转换为 `f32`，文本中的元素则先以 `f64` 解码、再逐项转换并检查 `is_finite()`。

唯一跨线程可见状态是调用方提供的 `AtomicBool cancel`。本文件只读取它，不负责创建、置位或释放；其真实所有者是 `EmbedFn` 的 batch。`pkg/inference/embed_fn.rs::State` 管理 provider 注册、批次、缓存和工作线程，本文件不参与这些状态机。

`parse_go_delay` 用 `u128` 累加纳秒，正数上限为 `i64::MAX`，负数绝对值上限为 `1 << 63`。Rust `Duration` 不能表示负值，因此合法负 duration 被折叠为 `Duration::ZERO`，对应 Go `time.NewTimer` 对非正 duration 立即到期的可观察用途。

## 依赖与调用关系

上游关系如下：

- `pkg/inference/lib.rs` 声明并再导出模块和类型。
- `pkg/inference/embed_fn.rs::run_batch` 经 `Arc<dyn Embedder>` 调用 trait 默认入口 `create_embeddings_with_values`，再落到本实现；它负责校验 provider 返回数量、传播错误以及按调用拆分批量结果。
- `pkg/expression/builtin_inference_test.rs::context` 注册 `MockEmbedder`，验证 `EMBED_TEXT()` 表达式的 options、NULL、错误和向量维度限制。
- `pkg/session/runtime/inference_test.rs` 在启用 `nextgen` 的测试中把它注册到 Domain 的 `EmbedFn`，覆盖 SQL 查询及存储生成列的 DML/LOAD DATA 路径。

下游直接依赖仅包括标准库的 atomics、线程和时间，以及 `serde_json::from_str`。`pkg/inference/Cargo.toml` 将 `serde_json = "1"` 声明为 crate 依赖；虽然该 Cargo manifest 还声明 HTTP/TLS 等远程 provider 依赖，本文件并不使用它们。

## 错误处理与边界

方法统一返回 `Result<_, String>`，与 `Embedder` trait 保持一致。明确错误包括 `unknown model`、`unknown option`、`invalid type for 'plus' option`、`invalid type for 'delay' option`、`invalid delay duration`、`context canceled`、serde JSON 解码文本，以及非有限 `f32` 错误。

边界行为由 `pkg/inference/embed_fn_test.rs` 固定：顶层 JSON `null` 得到空向量；`[null,1]` 得到 `[0.0,1.0]`；`[1e100]` 因转换为无限 `f32` 而失败；非法 JSON 失败。duration 接受 `0`、带正负号的零、负 duration、`.5us` 和 `1ms2us`，并在 `±2^63` 纳秒附近执行与 Go 有符号 duration 一致的范围检查。

一个刻意保留的 Go 语义是：没有 `delay` option 时，本实现不读取 `cancel`，即使标志已为 true 仍会解析并返回向量；测试 `mock_without_delay_does_not_check_context_like_go_provider` 明确锁定此行为。向量最大 16,383 维的限制不在本文件中实施，而由 `EmbedFn::embed_with_context_values` 及缓存接线检查。

## 并发与资源生命周期

`MockEmbedder` 没有可变实例状态，因此可安全地作为 `Arc<dyn Embedder + Send + Sync>` 被多个 batch 调用；线程安全约束来自 `Embedder: Send + Sync`。每次调用的向量和解析中间值独立拥有，没有共享缓冲区。

延迟模拟是同步阻塞的：当前 provider 工作线程调用 `thread::sleep`，不会创建新线程、异步任务、通道、timer 句柄或需要显式清理的资源。睡眠片段最长 5ms，决定了正延迟期间取消被观察到的粒度。`Ordering::Acquire` 与 batch 侧的取消发布配合；provider 返回后，工作线程与批次的 join、缓存和关闭生命周期由 `EmbedFn` 管理。

## 与 Go 版本的对应关系

直接对照为 `pkg/inference/embedding/mock/mock.go` 的 `Embedder.CreateEmbeddings`。两版都只接受 `json` 模型，只允许 `plus`/`delay`，将输入 JSON 解码为 `float32` 向量，应用偏移量，并通过延迟路径模拟可取消的远程调用。Go 的 `NewMockEmbedder` 返回无状态 `*Embedder`；Rust 直接构造零大小值 `MockEmbedder`，没有单独构造函数。

Rust 的 `parse_go_delay` 显式复刻 Go `time.ParseDuration` 在本测试面所需的语法和 int64 纳秒边界，因为标准库 `Duration` 的语法与负值模型不同。Go 用 `time.NewTimer(dur)` 和 `select { timer.C, ctx.Done() }`；Rust 将非正 duration 归零，并仅在提供 `delay` 时检查 `AtomicBool`。`pkg/inference/embed_fn_test.rs` 对复合单位、微秒拼写、符号、边界溢出和取消结果提供了移植证据。

Rust 还显式以 `Option<Vec<Option<f64>>>` 保留 Go `encoding/json` 对 JSON `null` 的测试语义，并在 `f64 -> f32` 后检查有限性，从而拒绝 Go 解码到 `float32` 时也无法表示的巨大数值。错误字符串不要求逐字复制 Go 的动态类型格式，但错误类别和触发顺序保持对应。

## 扩展指南

新增 mock option 时，应同时修改 `create_embeddings` 的 allowlist 与解析/执行分支；否则新 key 会在任何文本解析前被拒绝。若 option 会影响批处理或缓存结果，还要确认 `pkg/inference/embed_fn.rs` 的 `new_batch_key` 和单文本 cache key 已把完整 `Options` 纳入身份，目前二者会基于 options 生成区分键。

扩展 duration 语法应修改私有 `parse_go_delay`，并在独立的 `pkg/inference/embed_fn_test.rs` 增补有效、无效、溢出和取消用例；不要把测试内嵌到 `mock.rs`。改变 JSON/null/浮点转换语义时，还应同步检查 Go 对照 `pkg/inference/embedding/mock/mock.go`，并更新表达式测试 `pkg/expression/builtin_inference_test.rs`；若行为通过 SQL 可见，再更新 `pkg/session/runtime/inference_test.rs` 的 `nextgen` 场景。

性能上，`delay` 会占用同步工作线程，新增长时间或高频模拟行为需评估批次阻塞；兼容性上，不应把无 `delay` 时的取消检查“顺手”提前，因为现有测试把它视为 Go 对齐契约。任何新增错误都应保持先验证模型/option、后处理输入的当前顺序，以免改变调用方观察到的首个错误。

## 验证依据

- RustCodeGraph 索引状态：11,467 个文件、307,296 个节点、1,848,419 条边；`files --filter pkg/inference` 确认目标及相关 Rust/Go 文件均已索引。
- RustCodeGraph 源码与符号查询：`node --file pkg/inference/mock.rs`、`query MockEmbedder --kind struct`、`query parse_go_delay --kind function`、`node mock.rs::MockEmbedder`；确认公开类型、trait 实现、私有解析器和四个直接使用文件。
- 调用链证据：`pkg/inference/embed_fn.rs` 中的 `Embedder` trait、`EmbedFn::embed_with_context_values` 与 `run_batch`；后者在分块后调用 `create_embeddings_with_values` 并校验结果数量。
- crate 边界：`pkg/inference/Cargo.toml` 与 `pkg/inference/lib.rs`；确认 crate 名、`serde_json` 依赖、模块公开性和根级再导出。目标目录没有 `doc.go`。
- Go 对照：`pkg/inference/embedding/mock/mock.go`；上层 Go 行为参考 `pkg/inference/sqlembed_test.go::TestEmbedFnProvidersAndErrors`。
- Rust 独立测试：`pkg/inference/embed_fn_test.rs` 中 `go_merge_43_mock_provider_validates_model_options_and_applies_offset`、`mock_json_null_is_an_empty_vector`、`mock_provider_rejects_invalid_inputs_and_handles_duration_boundaries`、`mock_duration_parse_observes_signed_nanosecond_limits`、`mock_without_delay_does_not_check_context_like_go_provider`、`mock_json_null_elements_decode_as_zero_and_float_overflow_fails`。
- 上层接线测试：`pkg/expression/builtin_inference_test.rs` 和 `pkg/session/runtime/inference_test.rs`。本任务为纯文档分析，按计划不运行 Cargo；最终以固定十一章节结构命令和人工事实复核验收。
