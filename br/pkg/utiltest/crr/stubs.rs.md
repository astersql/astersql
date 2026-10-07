# `br/pkg/utiltest/crr/stubs.rs`

## 文件定位

本文说明的真实源文件是 [`stubs.rs`](./stubs.rs)。它属于 Cargo crate `astersql-br-pkg-utiltest-crr`，由同目录 `lib.rs` 以 `#[path = "stubs.rs"] pub mod stubs` 纳入，并选择性再导出 `ArcMemStorage`、`CancelHandle`、`Context`、`Error`、`LocalStorage`、`MemStorage`、`Result` 和 `Storage`。`Cargo.toml` 将该 crate 标记为 Go 包 `br/pkg/utiltest/crr` 的迁移库，并明确为了 darwin/arm64 测试路径不引入 `kv`、`domain`、`kvproto`、`grpcio` 或完整 `objstore`，因此这里是 CRR（跨区域复制）测试夹具的本地边界适配层，不是真实对象存储或数据库存储实现。

直接使用者集中在同一 crate：`crr_sim.rs` 用它抽象复制上下游及流式读写，`flush_sim.rs` 用它写模拟日志和元数据，`harness.rs` 用 `LocalStorage` 组装隔离的上下游目录，`pd_sim.rs` 与 `pd_sim_service.rs` 使用其上下文和错误类型。`parity_test.rs`、`harness_test.rs` 是最接近的独立 Rust 测试；源文件自身没有内嵌测试模块。

## 核心职责

1. 用 `Error`/`Result` 提供足够表达普通失败与“对象不存在”的最小错误协议，使复制工人可以区分可忽略的删除竞态。
2. 用 `Context`/`CancelHandle` 提供跨线程共享的取消标志和首个取消原因，支持 `crr_sim.rs::emitNewVersionEvent` 在有界通道背压期间终止重试。
3. 定义 CRR 夹具所需的 `Reader`、`Writer`、`Storage` 最小接口，隔离完整 Go `objectio`/`storeapi` 依赖。
4. 提供两类测试存储：`MemStorage`/`ArcMemStorage` 用共享内存表驱动快速复制测试，`LocalStorage` 用真实临时目录驱动 harness 的端到端文件路径。
5. 维持确定性的遍历和可观察的提交边界：内存路径排序后回调；流式 writer 在 `Close` 时才把缓冲提交到存储。

这些职责只覆盖当前 CRR 夹具需要的接口子集。它不实现真实云存储签名、分页遍历、范围读取、seek、文件大小查询、强一致性标记或完整取消传播。

## 主要符号

- `Error { message, is_not_exist }`：`new` 构造普通错误，`not_exist` 构造缺失对象错误；实现 `Display` 和 `std::error::Error`。`Result<T>` 固定使用该错误类型。
- `Context`、`ContextInner`、`CancelHandle`：`background` 创建未取消上下文，`with_cancel` 返回共享同一 `Arc<ContextInner>` 的上下文/句柄；`is_done` 读取原子标志，`err_message` 克隆首个原因；`cancel`/`cancel_with` 负责置位和唤醒。
- `ReaderOption`、`WriterOption`：当前为空的兼容占位。`WalkOption` 当前只有 `SubDir`，字段名保持 Go 风格。
- `Reader`：定义 `Read` 和 `Close`；`MemReader` 读取打开时复制出的快照，`FileReader` 包装 `std::fs::File`。
- `Writer`：定义 `Write` 和 `Close`；`MemWriter` 在内存累计字节，`FileWriter` 写同目录临时文件并在关闭时重命名。
- `Storage`：定义整文件读写、存在性、单个/批量删除、遍历、URI、流式打开/创建、重命名、预签名和关闭，是 `crr_sim.rs::CRRUpstreamStorage`、`CRRWorker`、`FlushSim` 与 `TestHarness` 的注入边界。
- `MemStorage`：以 `Mutex<HashMap<String, Vec<u8>>>` 保存对象，以 `AtomicBool` 记录关闭；`paths` 返回排序快照。
- `ArcMemStorage`：持有 `Arc<MemStorage>` 的可克隆句柄；`as_storage` 转成 `Arc<dyn Storage>`，并使 `Create` 能构造持有共享存储的 `MemWriter`。
- `LocalStorage`：持有根目录、`IgnoreEnoentForDelete` 开关和关闭标志；`new` 确保根目录存在，`full_path` 把逻辑名称逐段追加到根路径。
- `walk_collect`：递归、排序地收集普通文件，生成相对根目录且以 `/` 分隔的路径，再把路径与大小交给回调。

## 执行流程

整文件内存路径为：调用 `WriteFile` 时锁定 `files` 并覆盖同名值；`ReadFile` 返回字节克隆，缺失时产生 `is_not_exist=true`；`Open` 同样先克隆完整内容，再由 `MemReader::Read` 推进私有游标，因此打开后的 reader 不受后续覆盖影响。`WalkDir` 在锁内建立已排序的 `(path, size)` 快照，释放锁后逐项调用外部回调，避免回调重入造成同一互斥锁死锁。

流式内存路径必须经 `ArcMemStorage::Create`：它创建持有内部 `Arc<MemStorage>` 的 `MemWriter`；多次 `Write` 仅追加私有缓冲，`Close` 才调用 `WriteFile` 使完整对象可见。裸 `MemStorage::Create` 因无法从 `&self` 安全取得拥有权而明确返回错误。`crr_sim.rs::CRRUpstreamStorage::Create` 再包装该 writer，并在底层 `Close` 成功后发出新版本事件。

本地文件路径为：`LocalStorage::new` 创建根目录；整文件 `WriteFile` 创建父目录后直接 `fs::write`；流式 `Create` 创建固定 `.tmp` 扩展名文件，`FileWriter::Write` 写入它，`Close` 先 `flush` 再 `rename` 到最终路径。`Open` 返回真实文件 reader。`WalkDir` 从根目录或 `SubDir` 开始，缺失目录视为空结果，随后由 `walk_collect` 深度优先遍历并在每层排序。`Rename` 会为新路径创建父目录后调用文件系统重命名。

取消路径为：`Context::with_cancel` 创建共享状态；首次 `cancel_with` 在互斥锁内保存原因，然后以 `SeqCst` 写入取消标志并 `notify_all`。当前直接消费者 `emitNewVersionEvent` 轮询 `is_done`/`err_message`；本文件没有公开等待 `Condvar` 的方法。

## 数据与状态

`MemStorage.files` 是对象名到完整字节的唯一状态，所有表操作由一个 `Mutex` 串行化；reader 与 writer 都持有独立字节缓冲，因此它们的游标/未提交内容无需额外同步。`ArcMemStorage` 的克隆共享同一文件表，而 `MemStorage::new` 每次创建独立表。

`ContextInner.cancelled` 使用 `AtomicBool`，`err` 使用 `Mutex<Option<String>>`。先写错误再置位保证观察到 `is_done=true` 的调用者能够读取原因；后续取消不会覆盖首个原因。`pair` 保存条件变量，但当前 API 只有通知端，没有等待端。

`LocalStorage.root` 决定 URI 和所有磁盘路径；`IgnoreEnoentForDelete` 仅改变删除缺失文件的结果。`MemStorage.closed` 与 `LocalStorage.closed` 都只由 `Close` 置位，当前任何读写方法都不检查它们，所以关闭不释放数据、不关闭已打开句柄，也不禁止后续操作。

`WalkOption.SubDir` 被规范化为无首尾 `/` 的前缀（内存）或拼接为遍历起点（本地）。本地遍历输出相对 `root` 的路径，而不是相对 `SubDir` 的路径；两种实现都力求稳定排序。

## 依赖与调用关系

本文件只依赖 Rust 标准库：集合、文件系统、同步原语、路径、I/O trait 与 `Duration`；`Cargo.toml` 没有为它引入外部对象存储依赖。crate 级依赖用于其他 CRR 模拟器，而非本文件实现。

主要上游关系如下：

- `lib.rs` 声明模块并再导出常用类型。
- `crr_sim.rs` 为 `CRRWorker` 保存两个 `Arc<dyn Storage>`，为 `CRRUpstreamStorage` 实现同一 trait，并使用 `Context` 终止事件通道背压循环。
- `flush_sim.rs` 经 `Arc<dyn Storage>::WriteFile` 生成 log/meta 对象。
- `harness.rs::NewLocalTestHarnessWithTestContext` 创建两个 `LocalStorage`，把上游包装为 `CRRUpstreamStorage`，把下游交给复制工人；`TestHarness::Close` 调用两个存储的 `Close` 后删除临时根目录。
- `pd_sim.rs`、`pd_sim_service.rs` 复用 `Context`、`Error`、`Result` 统一夹具 API，但其 fakecluster 上下文是另一个别名 `FcContext`，不可混同。

主要下游关系是标准库的 `HashMap`、`Arc`、`Mutex`、`AtomicBool`、`Condvar` 与 `std::fs`。RustCodeGraph 的文件节点将 `stubs.rs` 标记为被 10 个文件使用；精确符号查询对部分 trait 方法/再导出没有生成可用调用边，因此上述直接关系又由同目录 `use crate::stubs::{...}`、字段类型和方法调用复核。

## 错误处理与边界

对象缺失通过 `Error.is_not_exist` 而非错误枚举或 source chain 表达。`MemStorage::ReadFile`、`LocalStorage::ReadFile` 与 `LocalStorage::Open` 会设置该标志；本地删除缺失文件仅在 `IgnoreEnoentForDelete=true` 时成功。批量删除按输入顺序执行，首错即停，已经删除的对象不回滚。遍历回调首错同样立即返回。

互斥锁均使用 `unwrap`，线程持锁 panic 会污染锁并导致后续调用 panic；这符合轻量测试桩定位，但不适合生产恢复逻辑。大部分方法忽略传入的 `Context`，只有上层事件发送循环主动检查取消，所以文件 I/O、递归遍历和内存操作不可由上下文中断。

`ReaderOption`/`WriterOption` 被忽略；`Reader` 不支持 Go `objectio.Reader` 的 seek 和文件大小；`WalkOption` 缺少 `SkipSubDir`、`ObjPrefix`、`ListCount`、`IncludeTombstone`、`StartAfter`。因此不得把该 trait 当作完整 `storeapi.Storage` 替代。

路径安全需要特别注意：`LocalStorage::full_path` 只忽略空段，不拒绝 `.` 或 `..`，所以注释中“防止绝对路径逃逸 root”只对前导 `/` 成立，不能防止含 `..` 的逻辑名越出根目录。调用者当前使用受控测试路径；若输入边界扩大，必须先做组件校验。`Create` 使用固定 `path.with_extension("tmp")`，同一目标的并发 writer 会冲突；writer 未 `Close` 或关闭失败时临时文件会保留。`WriteFile` 是直接 `fs::write`，与 Go `Storage` 所声明的原子写契约及 Go `LocalStorage` 的随机临时文件后 rename 策略不同。

`MemStorage::Rename` 是读、写、删三步，不具原子性；`LocalStorage::PresignFile` 只返回 basename，`MemStorage::PresignFile` 则返回 `mem://` 加完整逻辑名，后者与当前 Go `MemStorage` 返回 basename 的行为并不一致。`Close` 标志不参与行为判断。

## 并发与资源生命周期

`MemStorage` 的对象表由互斥锁保护，适合多个 `ArcMemStorage` 句柄并发读写；打开 reader 获得不可变快照，writer 的缓冲在关闭前不共享。`WalkDir` 先生成快照再执行回调，因此回调可再次访问存储，但遍历不会看到快照之后的修改。

`Context`/`CancelHandle` 可跨线程克隆，`SeqCst` 提供清晰的可见性；首个错误原因由互斥锁保护。条件变量目前只有通知，没有等待接口，实际事件发送等待采用 `crr_sim.rs` 中每 1 ms 重试一次的轮询。

`LocalStorage` 自身不持有长生命周期文件句柄；`Open`/`Create` 返回的 reader/writer 分别拥有句柄。`FileReader::Close` 是空操作，真正关闭依赖对象析构；`FileWriter::Close` flush 后 rename，但没有显式 `sync_all`，也没有关闭幂等保护。重复 `Close` 可能再次 rename 已不存在的临时文件而失败。存储 `Close` 只置位；harness 的实际清理由 `TestHarness::Close`/`Drop` 删除根目录完成。

## 与 Go 版本的对应关系

Go `br/pkg/utiltest/crr` 没有同名 `stubs.go`；Rust 文件把 Go 标准库 `context`、`pkg/objstore/objectio`、`pkg/objstore/storeapi` 和 `pkg/objstore.LocalStorage` 的必要表面集中成本地桩。业务对应关系由 `crr_sim.go` 和 `harness.go` 验证：两端都通过 `Storage` 注入上下游，写入/流式关闭/重命名成功后产生新版本事件，harness 都使用本地目录存储装配复制流程。

接口层面，Rust `Storage` 复刻 Go `storeapi.Storage` 在 CRR 包装器中使用的方法；Rust `Writer` 对应 Go `objectio.Writer`。Rust `Reader` 只复刻顺序读取与关闭，不具备 Go 的 `ReadSeekCloser`、`GetFileSize`。Rust 选项也是缩减版，其中只有 `WalkOption.SubDir` 保留真实语义。

`LocalStorage` 的 URI（`file://<root>`）、basename 形式的 `PresignFile`、`IgnoreEnoentForDelete` 分支与 Go 实现及 `parity_test.rs::local_storage_matches_go_uri_presign_and_delete_contracts` 对齐。差异包括：Go `WriteFile` 使用带 UUID 的临时文件后 rename，Rust 直接写最终文件；Go `Create` 直接创建目标并使用缓冲 writer，Rust先写固定临时文件；Go `WalkDir` 支持更多筛选/游标选项；Go reader 支持范围/seek/大小；Go local `Close` 是空操作而 Rust仅记录标志；Rust `MemStorage::PresignFile` 当前也不同于 Go basename 行为。扩展时应以这些已验证差异为边界，不能假定完全等价。

## 扩展指南

新增 CRR 夹具所需存储能力时，先判断它是否属于当前最小接口：若只是新的对象操作，应扩展 `Storage` 并同步 `MemStorage`、`ArcMemStorage`、`LocalStorage` 和 `CRRUpstreamStorage` 的实现；若涉及完整对象存储特性，应优先引入正式 canonical crate，而不是继续扩大桩到生产级子系统。

修改提交语义时应保持“完整对象可见后再发事件”：`MemWriter::Close`/`FileWriter::Close` 必须先成功提交，`crr_sim.rs::CrrEventWriter::Close` 才能发送事件。为 `LocalStorage::WriteFile` 补原子性时应采用唯一临时名、错误清理和同目录 rename，并增加并发 writer 回归。若允许非受控路径，必须在 `full_path` 拒绝根目录、前缀、`.`/`..` 等逃逸组件，并测试 Unix/Windows 分隔符。

若补齐取消或关闭语义，应明确哪些操作在 `Context::is_done` 或 `closed` 后失败，并避免破坏现有测试依赖的可观察数据；若公开等待 API，可复用现有 `Condvar`，同时处理虚假唤醒。错误模型若从布尔标志升级为枚举，应同步 `crr_sim.rs::replicateOne` 的缺失竞态分支。

测试应继续放在独立文件，不能嵌入 `stubs.rs`。契约级变更优先扩展 `parity_test.rs`；harness 目录隔离/清理扩展 `harness_test.rs`；事件与复制交互仍在 `parity_test.rs` 或新增同目录独立 `*_test.rs`。至少覆盖正常读写、缺失对象、遍历排序/回调错误、流式提交前不可见、重复/失败关闭、并发创建、取消与路径逃逸。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 7032 个 Rust 文件；`files --filter br/pkg/utiltest/crr` 确认目标与同目录 Rust/Go 文件均在索引；`node --file br/pkg/utiltest/crr/stubs.rs --offset 1 --limit 500` 及 `--offset 501 --limit 300` 覆盖全部 751 行，并报告该文件被 10 个文件使用；另执行了 `query ArcMemStorage`、`callers ArcMemStorage`、`query/callers/callees as_storage` 与 `walk_collect`，后几项未返回足够精确的符号边，因此没有据此推断未显示的关系。
- crate/入口：`br/pkg/utiltest/crr/Cargo.toml`、`br/pkg/utiltest/crr/lib.rs`。
- 直接 Rust 消费者：`br/pkg/utiltest/crr/crr_sim.rs`、`flush_sim.rs`、`harness.rs`、`pd_sim.rs`、`pd_sim_service.rs`。
- 独立 Rust 测试：`br/pkg/utiltest/crr/parity_test.rs` 验证 `ArcMemStorage` 复制路径和 `LocalStorage` URI/预签名/删除契约；`harness_test.rs` 验证同种子 harness 目录隔离及单独关闭。
- Go 对照：`br/pkg/utiltest/crr/crr_sim.go`、`harness.go`；正式边界与实现来自 `pkg/objstore/storeapi/storage.go`、`pkg/objstore/objectio/interface.go`、`pkg/objstore/local.go`、`pkg/objstore/memstore.go`。
- 人工复核：逐项核对所有公开类型/trait/函数、三个 `Storage` 实现、reader/writer 私有实现、错误和关闭分支；文档明确区分当前事实、Go 对齐点和已知缩减/差异，未把测试桩描述为真实对象存储。
