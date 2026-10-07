# [`pkg/ingestor/ingestctrl/tikv_mode.rs`](./tikv_mode.rs)

## 文件定位

本文件属于 `astersql-ingestor-ingestctrl` crate。`pkg/ingestor/ingestctrl/Cargo.toml` 以 `[lib] path = "lib.rs"` 指定 crate 根，并通过 `package.metadata.porting.go-package = "pkg/ingestor/ingestctrl"` 声明对应的 Go 包；`lib.rs` 的 `pub mod tikv_mode` 将本模块公开，同时在独立文件 `tikv_mode_test.rs` 中注册单元测试。上层 `pkg/ingestor/doc.go` 将“在写入 KV 和 ingest 前准备环境，包括把 TiKV 切到 import mode”列为 ingestor 子系统职责之一。

本文件提供 TiKV Store 模式切换的 Rust 抽象和默认批量实现：从 `StoreCatalog` 获取 Store 快照，筛选可参与切换的节点，并通过 `ModeClient` 并发下发 Import/Normal 请求。源码检索显示，当前 Rust 生产代码尚未调用 `NewTiKVModeSwitcher` 或本文件的 `TiKVModeSwitcher`；直接实例化只出现在 `tikv_mode_test.rs`。因此它是已具备核心行为、但尚未接入 Rust 导入主链的迁移边界，不能视为 Go 生产接线已经完整移植。

## 核心职责

- `SwitchMode`、`StoreState`、`Store` 和 `KeyRange` 共同描述“对哪些节点、哪些 key 范围切换到哪种模式”的数据边界。
- `StoreCatalog` 隔离 PD/元数据枚举能力；`ModeClient` 隔离向单个 TiKV 节点发送切换请求的传输实现。本文件本身不包含 PD HTTP、TLS 或 gRPC 连接代码。
- `TiKVModeSwitcher` 提供无返回值的 `ToImportMode`/`ToNormalMode` 门面，使调用方不依赖枚举及并发细节。
- `switcher::switchTiKVMode` 对 `Up` 和 `Offline` 节点各启动一个 scoped thread；`Tombstone` 和 `Disconnected` 节点被跳过。
- 枚举失败和单节点切换失败都不向调用方返回；单节点错误尽量写入内部 `failures` 缓冲，保持“模式切换失败不致命”的 Go 语义方向。

## 主要符号

- `SwitchMode::{Import, Normal}`：目标工作模式。`ToImportMode` 和 `ToNormalMode` 分别固定映射到这两个枚举值。
- `StoreState::{Up, Offline, Tombstone, Disconnected}`：本地状态模型。实际筛选条件是 `Up | Offline`，与 Go `ForAllStores(..., StoreState_Offline, ...)` 的“最大状态（含）”一致。
- `Store { id, address, state }`：Store 快照。默认实现用 `id` 记录失败，并把包含 `address` 的整个 `Store` 交给 `ModeClient`；如何使用地址由具体 client 决定。
- `StoreCatalog::Stores(&CancellationToken) -> Result<Vec<Store>>`：获取一次 Store 列表。trait 要求 `Send + Sync`，以允许被共享。
- `ModeClient::SwitchMode(&CancellationToken, &Store, SwitchMode, &[KeyRange]) -> Result<()>`：单节点切换边界，也要求 `Send + Sync`。
- `TiKVModeSwitcher::{ToImportMode, ToNormalMode}`：公开门面；两者没有返回值，调用方无法直接观察枚举或切换错误。
- 私有结构体 `switcher`：持有 `Arc<dyn StoreCatalog>`、`Arc<dyn ModeClient>` 和 `Mutex<Vec<(u64, String)>>`。
- `NewTiKVModeSwitcher(...) -> Arc<dyn TiKVModeSwitcher>`：构造默认实现并擦除具体类型。由于返回 trait object，而 `failures` 不属于 `TiKVModeSwitcher`，正常构造调用方不能调用具体实现的 `failures()`。
- `switcher::switchTiKVMode`：统一执行器；负责枚举、状态过滤、并发调用和错误记录。
- `switcher::failures()`：克隆内部失败列表；当前仅对本模块具体类型可用，且没有生产或测试调用者。

## 执行流程

1. 调用方以共享的 catalog/client 调用 `NewTiKVModeSwitcher`，得到 `Arc<dyn TiKVModeSwitcher>`；失败列表初始为空。
2. `ToImportMode` 或 `ToNormalMode` 选择对应的 `SwitchMode`，把原始 `CancellationToken` 和 `&[KeyRange]` 传给 `switchTiKVMode`。
3. `switchTiKVMode` 同步调用 `StoreCatalog::Stores`。若返回错误，`let Ok(...) else { return; }` 立即结束，既不记录也不暴露该错误。
4. `std::thread::scope` 为返回的每个 Store 建立借用作用域；仅 `Up` 与 `Offline` 进入任务提交，`Tombstone`/`Disconnected` 直接跳过。
5. 每个合格 Store 启动一个线程，调用同一个 `ModeClient::SwitchMode(token, &store, mode, ranges)`。scope 允许线程安全借用 token、ranges 和 `self`，无需把它们提升为 `'static`。
6. client 成功时线程直接结束；失败时尝试锁住 `failures`，追加 `(store.id, error.to_string())`。锁若已 poisoned，则本次错误记录也被静默丢弃。
7. 离开 `thread::scope` 前会 join 全部子线程，因此公开方法虽然内部并发，整体仍是同步阻塞的；全部线程结束后才返回。

## 数据与状态

模式和 Store 状态都是可复制/比较的值类型。`Store` 与 `KeyRange` 可克隆，但批量切换过程中只借用同一份 `ranges` 切片，不为每个线程复制范围；测试在 mock client 内复制范围只是为了记录断言。

`switcher` 的 catalog/client 使用 `Arc` 共享，适合多个切换器调用或线程访问。唯一可变内部状态是 `failures: Mutex<Vec<(u64, String)>>`：记录会跨多次 Import/Normal 调用累计，源码没有清空、去重或容量上限。记录顺序取决于并发任务的完成顺序，不等同于 Store 枚举顺序。

传入的 `CancellationToken` 定义在 `lib.rs`，能检查本地原子取消、父令牌和 worker context；本文件不主动调用 `check()`/`is_cancelled()`，只把令牌交给 catalog/client。因此取消能否中止枚举或网络操作，取决于这两个 trait 实现是否遵守令牌。`ranges` 也不在此校验：空范围、倒序边界或重叠范围都会原样下传。

## 依赖与调用关系

直接依赖只有标准库的 `Arc`、`Mutex`、scoped threads，以及 crate 根的 `CancellationToken`、`KeyRange`、`Result`。目标文件没有直接引用 `Cargo.toml` 中的第三方 crate；实际 PD/gRPC/TLS 依赖被留在未来的 `StoreCatalog`/`ModeClient` 适配器边界之外。

已验证的内部调用边为 `TiKVModeSwitcher::ToImportMode` → `switcher::switchTiKVMode(..., Import, ...)`、`ToNormalMode` → `switchTiKVMode(..., Normal, ...)`，以及 `switchTiKVMode` → `StoreCatalog::Stores` / `ModeClient::SwitchMode` / `Mutex::lock`。`lib.rs` 负责模块公开和独立测试装配。

RustCodeGraph 对目标文件报告了大量“used by”文件，但精确符号检索和仓库文本检索表明这些多数是宽粒度 crate/file 关系或同名符号，不是本实现的直接调用者。Rust 源码中 `NewTiKVModeSwitcher` 的唯一调用在 `tikv_mode_test.rs`；`local.rs` 当前也没有模式切换调用。相对地，Go `tikv_mode.go` 已由 `pkg/ingestor/ingestctrl/local.go`、`lightning/pkg/importer/import.go` 和 `pkg/dxf/importinto/scheduler.go` 接入导入流程。

## 错误处理与边界

- `StoreCatalog::Stores` 失败会被完全吞掉，`failures` 也不会记录 catalog 级错误；调用方无法区分“没有 Store”和“枚举失败”。
- 单 Store 的 client 错误不中止其他线程，也不由公开方法返回。该行为符合 Go `tikv_mode.go` 忽略 `ForAllStores` 返回值的非致命策略，但 Rust 额外尝试保存诊断信息。
- `failures` 锁中毒时，失败记录静默丢失；`failures()` 自身在锁中毒时返回空列表，可能掩盖此前已有记录。
- `std::thread::scope` 会等待全部任务；若 client 长时间阻塞且不响应 token，整个模式切换也会阻塞。本文件没有超时、重试或并发上限。
- 若某个 scoped 子线程 panic，scope 在退出时会继续传播 panic，而不是转换成 crate `Error`；这与普通 `Result` 错误被吞掉的处理不同。
- 空 Store 列表是成功的空操作。`Offline` 并非“不可用即跳过”：当前实现明确包含 `Offline`，只排除 `Tombstone` 和 `Disconnected`。
- `NewTiKVModeSwitcher` 返回的 trait object 不公开失败诊断；若要让上层读取失败，应先设计稳定接口，不能假设现有 `failures()` 已可用。

## 并发与资源生命周期

一次调用为每个合格 Store 创建一个 OS 线程，没有 worker pool 或最大并发数；Store 数量直接决定瞬时线程数。线程通过 `thread::scope` 借用调用栈数据，并在方法返回前全部 join，因此不会遗留后台任务，也不存在 detach 生命周期。Import 与 Normal 两次公开调用之间没有全局串行锁：同一个 `Arc<dyn TiKVModeSwitcher>` 可被多个线程同时调用，两个批次可能交叠，最终 Store 模式取决于各请求到达顺序。

共享 client/catalog 的并发安全由 `Send + Sync` trait bound声明，具体实现仍需保证内部正确性。失败缓冲由 `Mutex` 保护，不会发生数据竞争，但追加顺序不确定且永久累计。`Arc` 负责 switcher、catalog 和 client 的所有权；最后一个引用释放后这些对象被回收，本文件没有连接关闭、线程池关闭或显式清理钩子。

作用域线程共享同一 `CancellationToken` 与 ranges 借用。令牌可由外部并发置为取消，但本层不检查；资源及时释放仍依赖下游 client/catalog。由于公开方法同步等待，调用方可以在返回后安全释放 ranges 和 token。

## 与 Go 版本的对应关系

Go 对照文件是 `pkg/ingestor/ingestctrl/tikv_mode.go`。两端都提供 `TiKVModeSwitcher`、`NewTiKVModeSwitcher`、Import/Normal 两个门面，并把失败视为非致命；都选择状态不高于 Offline 的 Store，即包含 `Up` 和 `Offline`、排除 `Tombstone`。`pkg/lightning/tikv/tikv_test.go::TestForAllStores` 用 Up、Offline、Tombstone 三种状态验证了该筛选语义。

当前差异包括：

- Go 构造器持有 TLS、PD HTTP client 和 logger，`switchTiKVMode` 调用真实的 `tikv.ForAllStores` 与 `tikv.SwitchMode`；Rust 构造器接收两个抽象 trait，仓库内尚无本文件对应的生产适配器或接线。
- Go `ForAllStores` 用 `errgroup.WithContext` 并发，首个 action 错误会取消派生 context，随后其返回值又被 `tikv_mode.go` 忽略；Rust 为每个节点启动 OS 线程，不做错误驱动的批次取消，保证已创建的所有任务完成。
- Go helper 的 `SwitchMode` 负责连接、日志和忽略 Unimplemented（用于潜在 TiFlash）；Rust 是否具备这些行为完全取决于 `ModeClient` 实现，本文件不能据此声称已经支持。
- Go 接口接收可变参数 `...*sstpb.Range`，Rust 接收 `&[KeyRange]`；两者都把范围传到节点请求，但 Rust 使用 crate 自定义字节范围类型。
- Go 仅通过日志保留切换诊断；Rust 有累计失败缓冲，却因构造器返回 trait object而未向正常调用方开放。
- Go 已在 local backend、Lightning importer 和 Import Into scheduler 中运行；Rust 当前只有 `tikv_mode_test.rs` 的局部行为验证，属于未接线迁移状态。

## 扩展指南

- 接入生产流程时，应在独立模块实现真实 `StoreCatalog` 和 `ModeClient`，明确 PD 枚举、TLS/gRPC、TiFlash/Unimplemented、超时与取消策略，再在 Rust local backend 的导入前、周期刷新和恢复路径成对调用 Import/Normal；同步补充独立集成测试，不能只依赖当前 mock 测试。
- 若改变 Store 筛选规则，修改 `switchTiKVMode` 的 `matches!` 条件，并同步 `tikv_mode_test.rs::switches_up_and_offline_stores_concurrently`；至少覆盖四种状态，保持与 Go `StoreState_Offline` inclusive 语义的兼容性，或明确记录有意差异。
- 若需要可观察错误，优先扩展 `TiKVModeSwitcher` 的返回/诊断契约，区分 catalog 错误、单 Store 错误、锁中毒和 panic；同时决定失败是否累计、何时清空、顺序是否稳定。仅扩展私有 `failures()` 无法服务构造器返回的 trait object。
- Store 数量可能很大时，应把“一节点一 OS 线程”替换为有界 worker pool或异步批处理，并验证取消、慢节点、部分失败和批次完成语义；这是主要性能与资源风险。
- 若要序列化 Import/Normal 批次，应增加独立的调用级互斥或状态机，并测试同时切入/切回时的确定性。不要复用 `failures` 锁充当生命周期锁。
- 所有测试继续放在 `tikv_mode_test.rs`，不要嵌入生产文件。建议新增 catalog 失败、client 部分失败、Normal 模式、空 Store、空 ranges、失败记录与取消传播测试；若改动 Rust 行为，保持 Go 版本意图，不用简化实现换取测试通过。

## 验证依据

- RustCodeGraph `status`：索引可用，包含 11,467 个文件、307,296 个节点和 1,848,419 条边。
- RustCodeGraph `node --file pkg/ingestor/ingestctrl/tikv_mode.rs --offset 1 --limit 500`：读取目标全部 133 行，核对枚举、三个 trait、构造器、默认实现、并发和失败记录。
- RustCodeGraph `query NewTiKVModeSwitcher --kind function`、`query switchTiKVMode --kind function`、`query ToImportMode --kind method`：定位 Rust/Go 同名符号。精确 callers/callees 消歧未返回边，因此又用限定为 `*.rs` 的源码检索核实；目标 Rust 构造器仅由 `tikv_mode_test.rs` 调用，公开门面仅在该实现和测试出现。
- 已读 crate/模块证据：`pkg/ingestor/ingestctrl/Cargo.toml`、`pkg/ingestor/ingestctrl/lib.rs`；包约定证据：`pkg/ingestor/doc.go`。
- 已读 Go 对照与下游实现：`pkg/ingestor/ingestctrl/tikv_mode.go`、`pkg/lightning/tikv/tikv.go`；仓库检索核对了 Go 生产调用点 `pkg/ingestor/ingestctrl/local.go`、`lightning/pkg/importer/import.go`、`pkg/dxf/importinto/scheduler.go`。
- 已读独立 Rust 测试 `pkg/ingestor/ingestctrl/tikv_mode_test.rs`：`switches_up_and_offline_stores_concurrently` 用 barrier 证明 Up/Offline 两个调用发生重叠，同时证明 Tombstone/Disconnected 被过滤、ranges 与 Import 模式原样传递。该测试没有覆盖 Normal、错误记录、枚举失败、取消或生产接线。
- 已读相关 Go 测试 `pkg/lightning/tikv/tikv_test.go::TestForAllStores`：验证 Up/Offline 被包含、Tombstone 被排除；同路径没有 `tikv_mode_test.go`，故本文未声称 Go 对照文件有专属测试。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前只运行任务指定的 11 章节结构检查，并人工复核唯一新增生产物、源文件链接、调用边和未接线边界。
