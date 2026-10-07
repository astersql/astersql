# `pkg/objstore/batch.rs`

源码：[batch.rs](batch.rs)；独立 Rust 测试：[batch_test.rs](batch_test.rs)；Go 对照：[batch.go](batch.go)。

## 文件定位

`batch.rs` 属于 `astersql-objstore` crate，由 [lib.rs](lib.rs) 以公开模块 `pub mod batch` 暴露。它位于统一对象存储接口 `storeapi::Storage` 与具体存储后端之间，是一个“延迟副作用”包装器：读操作仍访问真实存储，整文件写入、删除和重命名先记录为 `Effect`，随后可丢弃、导出为 JSON，或由 `Batched::commit` 回放到底层存储。

[Cargo.toml](Cargo.toml) 表明本文件直接使用的 crate 依赖包括 `anyhow`、`base64`、`serde`、`serde_json`、`tempfile`，并依赖本地 `objectio` 与 `storeapi` crate。另一个边界是 [objectio/lib.rs](objectio/lib.rs)：它只在 `cfg(test)` 下用 `#[path = "../batch.rs"]` 复用本文件，为 objectio 的外部包风格测试组装支撑模块；这不是 objectio 的生产 API 接线。

当前 Rust 仓库中，RustCodeGraph 只给出了本文件内部序列化调用边和测试调用边；对全仓 Rust 源码的精确检索也没有发现 BR 等生产模块调用 `objstore::batch`。因此，“服务于 BR dry-run”是由 Go 生产链和移植目标证明的架构用途，而 Rust 当前已验证的接线范围主要是公开 crate API 与测试，不能写成已经接入 Rust BR 主链。

## 核心职责

- 用闭合枚举 `Effect` 表示四类可延迟副作用：完整文件写入、批量删除、单文件删除、重命名；相关载荷类型是 `EffPut`、`EffDeleteFiles`、`EffDeleteFile`、`EffRename`。
- 让 `Batched<S>` 实现完整的 `storeapi::Storage` trait：四类副作用只入队；读取、存在性检查、打开 reader、遍历、URI、预签名和关闭直接透传；流式 `Create` 明确拒绝。
- 通过 `read_only_effects` 和 `clean_effects` 支持检查或放弃 dry-run 结果，通过 `commit` 按记录顺序回放结果。
- 通过 `json_effects`、`write_json_effects`、`save_json_effects_to_tmp` 生成与 Go tagged-union 格式兼容的审计/预览文件。

它不提供事务原子性、崩溃恢复、回滚、去重、合并相邻操作或“读到未提交写”的视图；队列只是进程内存中的顺序日志。

## 主要符号

- `pub enum Effect`：副作用的闭合联合。四个 variant 的类型在编译期固定，因此 Rust 版 `commit` 没有 Go type switch 的未知动态类型分支。
- `pub struct EffPut { file: String, content: Vec<u8> }`：`WriteFile` 的拥有型快照。
- `pub struct EffDeleteFiles { files: Vec<String> }`：`DeleteFiles` 的路径列表快照。
- `pub struct EffDeleteFile(pub String)`：单个删除路径的新类型。
- `pub struct EffRename { from: String, to: String }`：重命名的源、目标路径。
- `pub fn json_effects(&[Effect]) -> anyhow::Result<Vec<u8>>`：把整个队列编码成 JSON 数组，并补一个结尾换行。
- `pub fn write_json_effects<W: Write>(..., &mut W) -> anyhow::Result<()>`：将完整编码结果一次写入任意同步 `Write`。
- `pub fn save_json_effects_to_tmp(&[Effect]) -> anyhow::Result<String>`：在系统临时目录创建 `br-effects-*.json`，写入后持久化该临时文件并返回有损 UTF-8 路径字符串。
- `fn typed_json_effect(&Effect) -> serde_json::Value`：内部 tagged-union 映射；`Put.content` 显式使用标准 Base64。
- `pub struct Batched<S: storeapi::Storage>`：持有底层 `storage: S` 和 `Mutex<Vec<Effect>>`。
- `Batched::new` / `batch`：分别是关联构造器和便捷自由函数，均创建空队列。
- `Batched::storage`：只借用底层存储，不暴露队列。
- `Batched::read_only_effects`：在锁内克隆队列，返回与内部状态脱离的快照。
- `Batched::clean_effects`：在锁内清空队列但不访问底层存储。
- `Batched::commit`：持锁、顺序执行所有 effect、收集但不返回操作错误、清空队列并返回 `Ok(())`。
- `impl Storage for Batched<S>`：定义入队、透传和禁止 `Create` 的边界。
- `_assert_file_is_write`：未被调用的编译期类型约束，证明 `std::fs::File` 可传给基于 `Write` 的输出函数；它不参与运行流程。

## 执行流程

1. 调用方通过 `batch(storage)` 或 `Batched::new(storage)` 创建空包装器。
2. `WriteFile`、`DeleteFiles`、`DeleteFile`、`Rename` 忽略传入的 `Context`，取得 `effects` 互斥锁，将字符串、字节或字符串列表复制成拥有型值，再按调用先后追加一个 `Effect`，随即返回 `Ok(())`。此时底层存储未发生对应写操作。
3. `ReadFile`、`FileExists`、`Open`、`WalkDir`、`URI`、`PresignFile` 直接调用同名底层方法。因此读路径只能看到底层当前状态，不会把排队中的 `Put`、删除或重命名叠加成一个事务视图。
4. dry-run/检查路径可调用 `read_only_effects` 获得快照；调用者修改该 `Vec` 或其中数据不会反向修改队列。`clean_effects` 则直接放弃全部待执行操作。
5. 导出路径从 `save_json_effects_to_tmp` 进入，依次调用 `write_json_effects`、`json_effects` 和 `typed_json_effect`。输出是 tagged-union JSON 数组，末尾含换行；成功 `keep` 后文件不会随临时文件对象析构而删除，清理由调用者负责。
6. 提交路径由 `commit(ctx)` 取得队列锁，对每个 effect 调用底层 `WriteFile`、`DeleteFiles`、`DeleteFile` 或 `Rename`。即使某一步失败也继续尝试后续操作；循环完成后无条件清空队列并返回成功。
7. `Close` 直接关闭底层存储，不会自动提交或清空 effect。先 `Close` 再 `commit` 的后果由具体底层实现决定。

## 数据与状态

唯一可变状态是 `effects: Mutex<Vec<Effect>>`。`Vec` 保持入队顺序，代码没有排序、压缩或冲突消解。例如先写后删会保留两个独立 effect，并在 `commit` 时依次执行。

入队时均获取输入的拥有型快照：路径经 `to_owned`，写入内容经 `to_vec`，批量删除列表也经 `to_vec`。因此调用方随后修改原始切片不会改变队列。`Effect` 及载荷实现 `Clone + Debug + Eq + PartialEq`，便于生成只读快照和做确定性断言；四个载荷还派生 `Serialize/Deserialize`，但顶层 `Effect` 没有派生 serde，实际外部格式完全由 `typed_json_effect` 控制。

JSON 不直接使用 Rust variant 名：固定写出 Go 类型字符串 `objstore.EffPut`、`objstore.EffDeleteFiles`、`objstore.EffDeleteFile`、`objstore.EffRename`。`Vec<u8>` 内容编码为 RFC 4648 标准 Base64；空 effect 列表编码为 `[]\n`。

`Batched<S>` 按值拥有 `S`。泛型约束来自 `Storage: Send + Sync`；本类型本身没有自定义 `Drop`，也没有后台任务、通道或持久化队列。

## 依赖与调用关系

上游边界：

- [lib.rs](lib.rs) 将本文件作为 `astersql_objstore::batch` 暴露；[batch_test.rs](batch_test.rs) 通过该公开路径使用 `batch`、effect 类型和临时文件导出函数。
- [azblob_1_aster_unit_test.rs](azblob_1_aster_unit_test.rs) 直接调用 `json_effects` 和 `Batched::new`，验证提交前不可见、提交后可见。RustCodeGraph 明确给出 `batched_json_commit_and_compression_match_go -> json_effects` 调用边。
- RustCodeGraph 曾把同名 `batch` 的普通引用误关联到无关的 `TakeForeignKeyCascades`；全仓精确 `rg` 未发现该文件的 Rust 生产调用。此处以精确文本检索结果为准，不将模糊同名边当成业务调用证据。
- Go 生产对照链位于 [br/pkg/stream/stream_metas.go](../../br/pkg/stream/stream_metas.go)：`MigrationExt.DryRun` 和 `StreamMetadataSet.hook` 用 `objstore.Batch` 替换真实写路径，随后读取 effect；`RemoveDataFilesAndUpdateMetadataInBatch` 可把 effect 保存供检查。[br/pkg/task/operator/migrate_to.go](../../br/pkg/task/operator/migrate_to.go) 也把 dry-run effect 导出并向操作者显示路径。这些证明设计用途，但不证明 Rust BR 已接线。

下游边界：

- [storeapi/storage.rs](storeapi/storage.rs) 的 `Storage` trait 定义所有透传和回放操作；`Batched` 没有绕过该抽象访问具体云 SDK。
- `crate::objectio::Context`、`Reader`、`Writer` 以及 `storeapi::{ReaderOption, WriterOption, WalkOption}` 构成 I/O 接口形状。
- `crate::azblob::lock_unpoisoned` 统一处理 mutex poisoning；其具体恢复策略属于 `azblob.rs`，本文件只复用锁辅助函数。
- `serde_json` 与 `base64` 形成 Go 兼容 wire format；`tempfile` 和 `std::io::Write` 负责临时文件生命周期及写出。

RustCodeGraph 确认的内部调用链为：`save_json_effects_to_tmp -> write_json_effects -> json_effects -> typed_json_effect`；`json_effects` 还被 `batched_json_commit_and_compression_match_go` 调用。

## 错误处理与边界

- JSON 序列化、输出写入、临时文件创建和 `keep` 的错误均通过 `anyhow::Result` 与 `?` 原样向上传播。写出失败时尚未 `keep` 的 `NamedTempFile` 会按 tempfile 语义清理；成功返回的文件则由调用方清理。
- `Create` 总是 `bail!`，错误文本为 `ExternalStorage.Create isn't allowed in batch mode for now.`。这是为了避免半开流式 writer 无法被表达成完整、可回放的 effect；当前错误没有保留 Go 版 `ErrStorageUnknown` 的结构化错误类别。
- 四种入队方法只可能因进程级故障而不能完成常规返回；它们不调用底层存储，也不验证路径、权限、容量或 context 取消状态，所以“入队成功”不表示操作最终可执行。
- `commit` 是最重要的兼容边界：它把每个底层错误放入局部 `_errors`，但从不读取或返回这些错误，仍继续回放、清空队列并返回 `Ok(())`。这与当前 Go `Commit` 最终无条件 `return nil` 的可观察行为对齐，但会隐藏部分或全部提交失败，而且失败 effect 无法重试。
- 提交不是原子的：前几个操作可能成功，后续操作失败；清空之后无法从本对象判断哪些已生效。调用方不能把它当成事务提交。
- `Effect` 是闭合枚举，所以不像 Go 的 `any` 那样能包含未知 effect 类型；新增 variant 时编译器会迫使 `typed_json_effect` 和 `commit` 的 match 更新，但测试与跨语言格式仍需人工同步。
- `read_only_effects` 名称沿用 Go，但 Rust 返回深克隆而非内部 slice；这消除了 Go 注释中“不得修改返回 slice”的别名风险，代价是按队列总数据量复制。

## 并发与资源生命周期

`Mutex<Vec<Effect>>` 让并发入队、读取快照、清理和提交在队列层面串行化。单次入队的相对位置由实际获得锁的顺序决定，而非线程启动顺序。`read_only_effects` 在锁内完成深克隆，大内容会延长临界区。

`commit` 在整个底层 I/O 回放期间一直持有互斥锁。这保证提交看到固定队列，且不会与新的入队或清理交错；同时意味着慢存储、重试或阻塞 I/O 会阻塞其他线程对 effect 队列的所有访问。代码没有超时、取消轮询或把队列先交换到锁外的优化。

读类透传方法不取得 effect 锁，可与入队或提交并行调用底层 `S`；安全性依赖 `Storage: Send + Sync`，但读写之间没有快照隔离。`storage()` 也可让调用者绕过队列直接操作底层存储，因此使用方需要自行维持 dry-run 不变量。

临时 JSON 文件在 `save_json_effects_to_tmp` 成功后脱离自动删除生命周期，必须由调用者删除；[batch_test.rs](batch_test.rs) 明确用 `fs::remove_file` 清理。`Close` 只透传到底层，不隐式提交；丢弃 `Batched` 也只丢弃内存队列。

## 与 Go 版本的对应关系

[batch.go](batch.go) 是逐项语义基准：两边都有相同四种 effect、同顺序队列、相同写操作入队/读操作透传划分、禁止 `Create`、可清理/检查/提交，以及 `objstore.Eff*` tagged-union JSON。

已确认的等价点：

- Go `JSONEffects` 的 `json.Encoder.Encode` 会追加换行；Rust `json_effects` 显式追加 `b'\n'`。
- Go `[]byte` 默认 JSON 表示为标准 Base64 字符串；Rust 显式采用 `base64::STANDARD`，测试期望 `Hello, world -> SGVsbG8sIHdvcmxk`。
- Go `Commit` 尝试所有已知 effect、清空队列并最终返回 `nil`；Rust 尝试所有 enum variant、清空队列并返回 `Ok(())`，同样不向调用方暴露底层操作错误。
- 两边都在提交期间持有队列 mutex，并按记录顺序访问底层存储。

需注意的差异：

- Go `Effect = any` 可在运行时出现未知类型并提前返回错误；Rust enum 不允许构造未知 variant。
- Go 入队保存传入 slice/string header 所代表的值，`ReadOnlyEffects` 返回内部 slice；Rust 对路径、内容、列表做拥有型复制，读取时再深克隆，隔离性更强但复制成本更高。
- Go `Batched` 通过嵌入 `Storage` 自动获得未覆写方法；Rust 必须显式实现完整 trait，因此 `AccessRequestSnapshot` 等带默认实现的新方法会采用 trait 默认值，除非专门覆写。当前 `Storage` 的 `AccessRequestSnapshot` 默认返回 `None`，这意味着包装后可能丢失底层统计能力。
- Go `Create` 返回带 `ErrStorageUnknown` 类别的包装错误；Rust 仅返回 `anyhow` 文本错误。
- Go `SaveJSONEffectsToTmp` 的注释提到可通过符号链接重定向临时目录；Rust 仅使用 `std::env::temp_dir()` 和 tempfile builder，未在本文件证明相同重定向细节。

## 扩展指南

新增一种可批处理副作用时，至少需要同步修改 `Effect`、新增/调整载荷类型、`typed_json_effect` 的稳定 Go 类型名与字段、`Batched::commit` 的回放分支，并在独立 [batch_test.rs](batch_test.rs) 增加入队顺序、JSON 和提交行为测试；同时核对 [batch.go](batch.go) 及 [batch_test.go](batch_test.go)，避免破坏跨语言 dry-run 文件格式。不要把 Rust 单元测试内嵌回生产文件。

若要改变错误策略，优先在 `Batched::commit` 设计清楚兼容性：返回组合错误、保留失败 effect、停止于首错或继续执行分别会改变 Go 可观察语义及重试安全性。任何方案都应加入“中间操作失败、后续仍执行、队列最终状态”的独立测试；当前测试没有覆盖底层失败。

若要支持流式 `Create`，不能简单透传，因为这会绕过 dry-run。需要先定义 writer 缓冲上限、`Complete`/关闭语义、失败清理和大对象内存策略，再把完成后的内容转换成 effect；同步测试半写、关闭、并发与超大对象。

若要改善并发性能，可考虑锁内取走当前队列、锁外执行，但必须先规定提交期间新入队 effect 属于当前批次还是下一批次，以及失败时怎样恢复顺序。当前“提交全程持锁”是简单而明确的不变量。

若要保持包装透明性，应审查 `Storage` trait 新增的默认方法。例如 `AccessRequestSnapshot` 当前未透传；新增 trait 能力时需要决定它是只读透传、写入入队、禁止，还是需要新 effect。

性能与兼容风险集中在：写内容被入队复制且快照再次深克隆、JSON 构造完整 `Vec<Value>` 与完整字节缓冲、提交持锁执行慢 I/O、固定 type 字符串构成跨语言协议。优化这些路径时应保留顺序和 JSON 兼容测试。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 `11467` 个文件、`307296` 个节点、`1848419` 条边，目标 `pkg/objstore/batch.rs` 在索引内并报告 `44` 个符号。
- RustCodeGraph `node --file pkg/objstore/batch.rs`：核对了完整 267 行源码、所有公开/内部符号、四类 effect、Storage 实现和条件编译情况（本文件没有条件编译项）。
- RustCodeGraph `node batch.rs::{json_effects,write_json_effects,save_json_effects_to_tmp,batch}`：确认序列化内部调用链、`json_effects` 的测试调用边；同名 `batch` 的模糊引用边经精确 `rg` 判定不属于本模块调用。
- 已读生产/配置入口：[batch.rs](batch.rs)、[Cargo.toml](Cargo.toml)、[lib.rs](lib.rs)、[objectio/lib.rs](objectio/lib.rs)、[objectio/Cargo.toml](objectio/Cargo.toml)、[storeapi/storage.rs](storeapi/storage.rs)。包内没有 `doc.go`。
- 已读 Go 对照及生产使用：[batch.go](batch.go)、[br/pkg/stream/stream_metas.go](../../br/pkg/stream/stream_metas.go)、[br/pkg/task/operator/migrate_to.go](../../br/pkg/task/operator/migrate_to.go)。
- 已读独立测试：[batch_test.rs](batch_test.rs)、[batch_test.go](batch_test.go)、[azblob_1_aster_unit_test.rs](azblob_1_aster_unit_test.rs)。现有 Rust 证据覆盖 effect 顺序、清理、JSON/Base64/临时文件和成功提交可见性；未覆盖 `Create` 拒绝、读方法透传、`Close`、底层提交失败、并发竞争与空队列。
- 按任务约束未运行 Cargo；本任务是纯文档分析，最终仅执行固定 11 章节的结构验证，并人工复核 Rust 当前接线与 Go 设计用途没有混写。
