# `pkg/sessionctx/variable/embedding_vars.rs`

## 文件定位

本文件属于 `astersql-sessionctx-variable` crate（见同目录 `Cargo.toml`），由 `lib.rs` 以公开模块 `embedding_vars` 挂载并再导出全部公开项。它位于系统变量注册层与 `sessionctx/vardef` 的进程级存储之间：`sysvar_builtins.rs::register_builtin_sysvars` 在一次性初始化内建变量表时调用 `register_embedding_vars`，后续 SQL 全局变量读写通过注册到 `SysVar` 的 `SetGlobal`/`GetGlobal` 钩子进入本文件。

这里管理六个实验性 embedding 服务 API key，以及 OpenAI 兼容 API base。它不是 embedding 请求执行器；Go 版本的实际消费者在 `pkg/inference/sqlembed.go::NewEmbedFn` 和缓存键构造处。仓库内 Rust 生产代码目前只直接使用本文件的变量识别、脱敏、URL 规范化与注册能力，未发现 Rust embedding 推理实现读取这些 key 或 `embedding_config_version()`。

## 核心职责

1. 用 `EMBEDDING_API_KEYS`、`EMBEDDING_API_BASE` 和 `DEFAULT_EMBEDDING_API_BASE` 定义本模块处理的系统变量集合及默认 endpoint。
2. 通过 `config_holder` 把变量名映射到 `vardef` 中对应的进程级 `AtomicStringValue`，并提供 key/base 读取能力。
3. 通过 `NormalizeOpenAIEmbeddingAPIBase` 强制 HTTPS、host 白名单、无 query/fragment，并把 endpoint 形式归一为 base URL。
4. 通过 `mask_embedding_api_key` 对 SQL 读取结果保留至多末四字节用于辨认；空值保持为空。
5. 通过 `set_config` 只在有效配置发生变化时递增 `vardef::EmbeddingConfigVersion`，使上层能够让依赖配置的缓存失效。
6. 通过 `register_embedding_vars` 构造七个仅全局作用域的字符串 `SysVar`，安装读写钩子并加入全局注册表。

安全边界需要区分两类脱敏：本文件的 getter 返回 `******` 加末四位，适合 SQL 查询识别当前凭据；`pkg/executor/set.rs` 的审计/日志路径对非空 embedding key 使用完整 `******`，避免尾部信息进入日志。

## 主要符号

- `EMBEDDING_API_KEYS: [&str; 6]`：Jina AI、OpenAI、Cohere、HuggingFace、NVIDIA NIM、Gemini 的 key 变量名列表，同时驱动注册循环和 `is_embedding_api_key`。
- `EMBEDDING_API_BASE` / `DEFAULT_EMBEDDING_API_BASE`：OpenAI 兼容 endpoint 的变量名与空配置的等价值 `https://api.openai.com/v1`。
- `OpenAIEndpointWhitelistErrMsg`：host 不在允许集合时返回的兼容错误文本。
- `config_holder(name) -> &vardef::AtomicStringValue`：内部穷举映射；未知名称会 `panic!("unknown embedding variable")`，因此只应接收上述七个受控名称。
- `embedding_config_version() -> u64`：读取全局配置版本。目前 Rust 直接调用证据仅在 `embedding_vars_test.rs`。
- `embedding_api_key(name) -> String`：从映射后的全局 holder 克隆当前值；名称契约与 `config_holder` 相同，虽然函数公开，但未知值会 panic。
- `GetOpenAIEmbeddingBaseURL() -> String`：读取 base；底层为空时由 `resolve_base` 返回默认 URL。
- `is_embedding_api_key(name) -> bool`：ASCII 大小写不敏感地识别六个 key 名，不包含 API base。
- `mask_embedding_api_key(value) -> String`：空串原样返回，字节长度不超过 6 时返回六个星号，否则返回六个星号与末四字节。
- `NormalizeOpenAIEmbeddingAPIBase(value) -> Result<String, String>`：校验、解码 path、去末尾 `/`，再去末尾 `/embeddings`，最后重建 HTTPS URL。
- `set_config(name, value)`：原子替换对应字符串；key 按原字符串比较，base 按 `resolve_base` 后的有效值比较，只有语义变化才递增版本。
- `register_embedding_vars()`：内部注册入口；由内建系统变量总入口调用，为每个变量安装闭包。

## 执行流程

注册阶段：

1. `register_builtin_sysvars` 受 `Once` 保护，调用 `register_embedding_vars`。
2. 注册函数串联六个 `EMBEDDING_API_KEYS` 和一个 `EMBEDDING_API_BASE`，逐一创建 `SysVar`。
3. 每项均设为 `ScopeGlobal`、`TypeStr`、允许空值；key 默认值为空，base 的公开默认值为 `DEFAULT_EMBEDDING_API_BASE`。
4. `SetGlobal` 闭包对 base 先调用 `NormalizeOpenAIEmbeddingAPIBase`，将字符串错误转换为 `VariableErrorKind::WrongValue`；key 直接采用输入。随后统一调用 `set_config`。
5. `GetGlobal` 闭包对 base 返回空值回退后的 URL，对 key 返回脱敏结果，最后由 `RegisterSysVar` 加入注册表。

设置 API base 时，规范化流程为：先 `trim`；空串立即成功并代表默认值；使用 `url::Url::parse` 验证绝对 URL；要求 host 存在、scheme 为 HTTPS、query/fragment 为空；允许 `api.openai.com`、三个 DashScope host 和以 `.openai.azure.com` 结尾的 host。之后从原始输入保留 authority 的大小写和显式端口、去掉 userinfo，百分号解码 path，拒绝坏转义或非 UTF-8 path，再依次移除一个末尾 `/` 和一个末尾 `/embeddings`。

运行时 SQL 链路的直接证据位于 `pkg/session/runtime/control.rs`：读取 key 时调用 `is_embedding_api_key` 与 `mask_embedding_api_key`，读取空 base 时回退默认值；设置时拒绝非 GLOBAL 作用域，校验并规范化值，持久化到 `mysql.global_variables`，同时更新 domain 的全局变量视图。通用 executor 的 `pkg/executor/set.rs` 则在写全局变量后的审计与日志阶段识别 key 并完全脱敏。

## 数据与状态

真实值不存放在 `SysVar` 对象本身，而存放于 `pkg/sessionctx/vardef/tidb_vars.rs` 的七个静态 holder：`EmbedJinaAPIKey`、`EmbedOpenAIAPIKey`、`EmbedOpenAIAPIBase`、`EmbedCohereAPIKey`、`EmbedHuggingFaceAPIKey`、`EmbedNvidiaNIMAPIKey`、`EmbedGeminiAPIKey`。它们均为惰性初始化的 `AtomicStringValue`；该类型内部是 `RwLock<String>`，`Load` 在读锁下克隆，`Store`/`Swap` 在写锁下替换。

`EmbeddingConfigVersion` 是 `AtomicU64Value`，其 `Load` 与 `Inc` 使用 `SeqCst` 原子顺序。版本号不包含秘密本身；Go 的 `vardef/tidb_vars.go` 明确说明它用于动态凭据或 endpoint 变化时让 embedding 缓存失效。Rust 代码保留了同一状态及递增语义，但当前仓库没有 Rust 生产消费者读取该版本。

空 base 与显式默认 base 被视为同一有效配置：`set_config` 比较 `resolve_base(old)` 和 `resolve_base(new)`，所以二者互换不会增加版本。key 则严格按字符串比较，相同值重复设置不会增加版本。

## 依赖与调用关系

上游直接关系：

- `sysvar_builtins.rs::register_builtin_sysvars -> register_embedding_vars -> RegisterSysVar`，建立系统变量元数据及钩子。
- `pkg/session/runtime/control.rs` 调用 `is_embedding_api_key`、`mask_embedding_api_key`、`NormalizeOpenAIEmbeddingAPIBase`，并读取三个公开常量完成 SQL 的展示和设置分支。
- `pkg/executor/set.rs` 调用 `is_embedding_api_key`，决定审计和日志是否完整遮蔽值。
- `embedding_vars_test.rs` 直接覆盖规范化、脱敏、注册钩子、版本变化和作用域。

下游依赖：

- crate 内部的 `SysVar`、`RegisterSysVar`、`VariableError`、`VariableErrorKind` 提供注册、钩子与错误模型。
- `astersql-sessionctx-vardef` 提供变量名/holder、全局作用域、字符串类型和版本计数；在 `Cargo.toml` 中以路径依赖 `vardef = { package = "astersql-sessionctx-vardef", path = "../vardef" }` 引入。
- 外部 crate `url = "2"` 仅用于 URL 语法、host、scheme、query 和 fragment 的解析校验；后续 authority/path 兼容处理由本文件完成。
- 标准库 `Arc` 使 `SetGlobal`、`GetGlobal` 闭包满足共享钩子类型。

Go 主链更完整：`pkg/inference/sqlembed.go::NewEmbedFn` 把各 holder 的 `Load` 和 `variable.GetOpenAIEmbeddingBaseURL` 交给 provider，并在 embedding 缓存键中加入 `EmbeddingConfigVersion.Load()`。这证明本文件所对齐的设计意图，但不能作为 Rust 推理链已经接线的证据。

## 错误处理与边界

`NormalizeOpenAIEmbeddingAPIBase` 的可恢复错误都使用字符串返回：相对 URL/无 host、非 HTTPS、query/fragment、非白名单 host、非法百分号转义、非 UTF-8 path。注册钩子将其包装为 `VariableErrorKind::WrongValue`，所以非法值在 `set_config` 前返回，不会修改 holder 或版本。

host 白名单采用完整匹配或 Azure 后缀匹配：`api.openai.com.evil` 不会通过，任意 `*.openai.azure.com` 会通过；显式端口不影响 `host_str()` 的白名单判断并会保留在结果中。user info 不进入规范化结果。path 的行为刻意对齐 Go `url.URL.Path`：先解码转义、不清理 `..` 段，再处理末尾 `/embeddings`。

`config_holder` 对未知名称 panic，这是内部映射的不变量而非用户输入错误通道。新增变量若加入调用侧却未加入该 match，会把配置读写变成进程 panic。

`mask_embedding_api_key` 按 UTF-8 字节长度切取最后四字节；当非 ASCII 值的切点不在字符边界时，Rust 字符串切片会 panic。现有 key 场景及测试使用 ASCII，当前没有对任意 Unicode 安全的保证。另需注意 getter 的末四位脱敏不能用于日志；日志路径必须维持 `executor/set.rs` 的完全遮蔽。

## 并发与资源生命周期

变量注册只在 `register_builtin_sysvars` 的一次性初始化闭包中发生，避免并发重复注册。注册后的 `Arc` 钩子是进程生命周期对象，闭包仅捕获静态变量名，不持有会话资源或异步任务。

配置字符串由 `RwLock<String>` 序列化写入并允许并发克隆读取；`Swap` 在同一个写锁临界区取得旧值并写入新值，使“本次设置是否变化”的判断基于实际被替换的值。字符串锁释放后才递增独立的原子版本，因此读者可能在极短窗口内看到新字符串与旧版本；文件没有提供跨 holder 与版本的事务快照保证。版本计数使用 `SeqCst`，但溢出行为没有额外处理。

本文件不创建线程、任务、通道、网络连接或事务。URL 解析值和临时字节缓冲都局限于单次调用；全局 holder 和注册表则持续整个进程生命周期。

## 与 Go 版本的对应关系

Rust 的 `NormalizeOpenAIEmbeddingAPIBase` 和 `GetOpenAIEmbeddingBaseURL` 对齐同目录 `embedding_vars.go`；系统变量注册、key getter/setter、脱敏与版本递增则对应 `sysvar.go::newEmbeddingAPIKeySysVar` 及 `SysVars` 中的七个条目。常量和存储对应 `pkg/sessionctx/vardef/tidb_vars.go`。

已验证的一致语义包括：六种 provider、仅 GLOBAL 字符串变量、允许空值、相同 key 不递增版本、空 base 与默认 base 等价、SQL getter 保留末四位、HTTPS/host/query 白名单限制、移除末尾 `/embeddings`。Rust 独立测试还覆盖保留显式端口、保留未清理的点路径以及百分号解码。

存在两项当前差异/边界：Go 的 `SysVar` key 条目显式设置 `IsSensitive: true`，Rust 的 `register_embedding_vars` 没有设置该字段，而是依赖本文件 getter、session runtime 和 executor 审计路径的专门脱敏；扩展诊断接口时不能假定元数据已标敏。其次，Go 推理代码实际读取 key/base 与版本构造 provider 和缓存键，Rust 仓库中尚未发现对应生产消费链，故 Rust 当前属于变量基础设施已接线、推理消费者未验证的状态。

Go 测试 `embedding_vars_test.go` 覆盖规范化、默认值、版本和六个 key；Rust 独立测试 `embedding_vars_test.rs` 保留相同核心意图，但测试用例集合并非逐项完全相同。

## 扩展指南

新增 embedding provider key 时，应同步修改 `EMBEDDING_API_KEYS`、`config_holder`、`vardef/tidb_vars.rs` 的名称常量与 holder，并确认 Go 的 `vardef/tidb_vars.go`、`sysvar.go` 和 provider 注册仍保持对应。遗漏 `config_holder` 会 panic，遗漏数组会导致变量未注册且识别/脱敏路径失效。测试应放在独立的 `embedding_vars_test.rs`，不要内嵌到生产文件，并至少覆盖注册、GLOBAL-only 校验、读回脱敏、相同值不递增版本及恢复全局状态。

修改 endpoint 策略时，主要入口是 `NormalizeOpenAIEmbeddingAPIBase`；应同步更新 Rust/Go 独立测试，检查 host 混淆、端口、大小写、user info、query/fragment、转义路径、`/embeddings` 后缀以及空值等价性。白名单扩展属于安全变更，不能只改错误消息或测试。

若让更多诊断路径通用地识别敏感变量，应先评估为 Rust `SysVar` 设置与 Go `IsSensitive` 对等的元数据，并补充 `pkg/server/handler/tests/global_variables_test.go` 所代表的诊断遮蔽回归；SQL getter 的“保留末四位”与日志的“完全遮蔽”要分别验证。

若接入 Rust embedding 执行器，应读取 `vardef` holder 或本文件 getter，并把 `EmbeddingConfigVersion` 纳入缓存失效设计；在实际调用边出现前，不应仅凭这些变量存在就宣称 Rust 推理链支持动态配置。

性能上，读 key 会克隆完整字符串，设置会取得写锁；这些操作适合低频配置路径。不要把 `embedding_api_key` 或 getter 放入无缓存的高频逐行执行路径，也不要把秘密本身放进缓存键、错误或日志。

## 验证依据

- 目标源码：`pkg/sessionctx/variable/embedding_vars.rs`，逐项核对 4 个常量、10 个函数及注册闭包。
- crate 与模块边界：`pkg/sessionctx/variable/Cargo.toml`（crate 名、`url` 与 `vardef` 依赖）及 `pkg/sessionctx/variable/lib.rs`（公开挂载、再导出、独立测试模块）。该目录没有 `doc.go`，最近的 Rust crate 说明来自 `lib.rs`。
- 状态定义：`pkg/sessionctx/vardef/tidb_vars.rs` 的 `AtomicStringValue`、`AtomicU64Value`、七个 holder 和 `EmbeddingConfigVersion`。
- 上游调用：`pkg/sessionctx/variable/sysvar_builtins.rs::register_builtin_sysvars`、`pkg/session/runtime/control.rs` 的读取/设置分支、`pkg/executor/set.rs` 的审计脱敏分支。
- Rust 测试：`pkg/sessionctx/variable/embedding_vars_test.rs`；测试位于独立文件，覆盖 URL 正反例、key 掩码、GLOBAL-only、重复设置、默认 base 等价及版本不误增。
- Go 对照：`pkg/sessionctx/variable/embedding_vars.go`、`embedding_vars_test.go`、`sysvar.go::newEmbeddingAPIKeySysVar` 与相关 `SysVars` 条目、`pkg/sessionctx/vardef/tidb_vars.go`、`pkg/inference/sqlembed.go`。
- RustCodeGraph：`status` 显示目标已索引（全库 11,467 文件，目标文件报告 18 个符号）；`query` 定位了 `register_embedding_vars`、`NormalizeOpenAIEmbeddingAPIBase`、`GetOpenAIEmbeddingBaseURL`、`embedding_config_version`、`embedding_api_key`、`is_embedding_api_key`、`mask_embedding_api_key`、`set_config`、`config_holder`、`resolve_base`。本次 `explore/node` 无输出，`callers/callees` 在限定时间内未返回，因此调用边改由上述直接引用搜索和源码核验，未将图缺失推断为无调用者。
- 结构验证按任务命令执行；本任务是纯文档分析，依计划不运行 Cargo。
