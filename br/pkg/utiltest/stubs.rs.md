# `br/pkg/utiltest/stubs.rs`

## 文件定位

[`stubs.rs`](stubs.rs) 是 `astersql-br-pkg-utiltest` crate 内部的本地对象存储适配层。crate 入口 `br/pkg/utiltest/lib.rs:19-29` 将本文件声明为公开模块，并重导出 `Context`、`Storage`、`LocalStorage`、`NewLocalStorage` 等接口；直接的运行入口是 `br/pkg/utiltest/suite.rs:83-101` 中的 `CreateRestoreSchemaSuite`，它在临时目录上创建存储并把 `Arc<dyn Storage>` 装入恢复 schema 测试套件。

该文件不是 BR 的通用对象存储实现，也不在生产请求主链中连接 S3、GCS 等后端。文件级注释 `stubs.rs:16-23` 和 `br/pkg/utiltest/Cargo.toml` 表明它是为 Darwin arm64 可构建的测试工具 crate 提供的本地替身，以避免引入 `kv`、`domain`、`kvproto`、`grpcio`、完整 `objstore` 等依赖。crate 只有两个内部路径依赖和 `tempfile`，因此这里应理解为测试夹具边界，而不是 `pkg/objstore` 的 canonical 实现。

## 核心职责

- 在 `Storage` trait（`stubs.rs:127-167`）中复刻恢复 schema 套件实际使用的 Go `storeapi.Storage` 子集，包括整文件读写、存在性检查、删除、遍历、流式打开/创建、重命名、伪预签名 URI 和关闭。
- 由 `LocalStorage`（`stubs.rs:171-198`、`256-427`）把逻辑对象名映射到一个本地根目录，并把标准库 I/O 错误转换为本文件的 `Error`。
- 由 `FileReader` 和 `FileWriter`（`stubs.rs:200-253`）提供带关闭状态的流接口；读流支持 `[StartOffset, EndOffset)` 范围，写流在关闭时刷新缓冲。
- 由 `NewLocalStorage`（`stubs.rs:473-477`）返回可跨测试组件共享的 `Arc<dyn Storage>`，满足 `TestRestoreSchemaSuite.Storage` 的动态分发需要。

## 主要符号

- `Error { message, is_not_exist }` 与 `Result<T>`（`stubs.rs:35-73`）：保留可显示的错误文本，并只对明确的 `NotFound` 分支设置分类位。`new` 构造普通错误，`not_exist` 构造缺失文件错误。
- `Context`（`stubs.rs:75-85`）：可克隆、无状态的 Go `context.Context` 替身；`background` 只返回空值，不携带取消、期限或请求值。
- `WalkOption`（`stubs.rs:87-96`）：仅包含 `SubDir`、`ObjPrefix`、`SkipSubDir`。它没有 Go 接口中的分页、墓碑和 `StartAfter` 能力。
- `ReaderOption`（`stubs.rs:98-104`）：`StartOffset` 为包含端，`EndOffset` 为不包含端；`PrefetchSize` 只保留字段，当前实现不消费它。`WriterOption`（`stubs.rs:106-108`）也是占位类型。
- `Reader`、`Writer`（`stubs.rs:110-123`）：均要求 `Send`，以显式的 `Context` 参数执行 I/O 和关闭。重复关闭或关闭后 I/O 会由具体句柄返回错误。
- `Storage: Send + Sync`（`stubs.rs:125-167`）：定义套件可共享的对象存储表面。方法名保留 Go 风格，crate 根部允许相应命名 lint（`br/pkg/utiltest/lib.rs:9-17`）。
- `LocalStorage { root }`（`stubs.rs:169-198`）：唯一持久状态是根路径；`new` 确保根目录存在，`full_path` 拼接逻辑名，`ensure_open` 恒成功以对齐 Go 本地存储的空操作 `Close`。
- `FileReader { file, pos, end_pos }` 与 `FileWriter { file }`（`stubs.rs:200-253`）：用 `Option` 表示句柄是否已经关闭，避免关闭后继续访问底层文件。
- `walk_dir_recursive`（`stubs.rs:429-471`）：深度优先遍历目录，按相对 `walk_base` 的前缀筛选，但向回调报告相对存储根目录的 `/` 分隔路径。
- `NewLocalStorage`（`stubs.rs:473-477`）：公开工厂，调用 `LocalStorage::new` 后擦除为 `Arc<dyn Storage>`。

## 执行流程

1. `CreateRestoreSchemaSuite` 创建 `TempDir`，调用 `NewLocalStorage(_temp_dir.path())`，并把临时目录所有权与 `Storage` 一起存入 suite（`br/pkg/utiltest/suite.rs:83-101`）。这保证根目录不会在存储仍被使用时提前删除。
2. `NewLocalStorage` 调用 `LocalStorage::new`；后者执行 `create_dir_all`，失败时附带根路径上下文，成功后只保存 `PathBuf`（`stubs.rs:176-183`）。
3. 整文件写入走 `WriteFile`（`stubs.rs:257-280`）：创建父目录，在目标父目录中建立 `NamedTempFile`，写完后 `persist` 重命名到目标；Unix 上再把权限设置为 `0644`。因此对同一文件的提交发生在 rename 边界，而不是直接截断目标后逐段写入。
4. 整文件读取、存在性和删除分别由 `ReadFile`、`FileExists`、`DeleteFile(s)` 完成（`stubs.rs:282-317`）。批量删除按输入顺序串行执行，第一处错误立即终止。
5. `WalkDir` 选择根目录或 `SubDir` 作为起点；起点不存在时返回成功空集，否则递归读取目录。`SkipSubDir` 阻止进入任何子目录，`ObjPrefix` 针对相对遍历起点的路径过滤，回调接收相对存储根的对象名和字节大小（`stubs.rs:319-343`、`429-471`）。
6. `Open` 打开目标并可 seek 到非负起点；`FileReader::Read` 每次把读取长度限制在 `EndOffset - pos` 内，抵达范围终点时返回 `Ok(0)`（`stubs.rs:207-230`、`350-378`）。
7. `Create` 直接创建或截断最终目标，再用 `BufWriter` 包装；`Write` 进入缓冲，`Close` 取走句柄并 flush（`stubs.rs:233-253`、`380-397`）。这条流式写路径与 `WriteFile` 的临时文件原子提交语义不同。
8. `Rename` 先创建新路径的父目录再调用 `fs::rename`；`PresignFile` 只返回 basename；`Close` 不改变存储状态（`stubs.rs:399-427`）。

## 数据与状态

存储对象的共享状态只有不可变的 `root: PathBuf`。`Arc<dyn Storage>` 负责共享所有权，方法都接收 `&self`，文件内容和目录项由操作系统文件系统维护。本文件没有内存索引、缓存、连接池或显式“已关闭”标志，因此一次操作的可见性取决于对应文件系统调用完成的时点。

流对象单独拥有文件句柄。`FileReader.pos` 从所选起点开始，成功读取后递增；`end_pos: None` 表示不限制终点。`FileReader.file` 和 `FileWriter.file` 从 `Some` 变成 `None` 是关闭的唯一状态转换，因而第二次 `Close` 与关闭后的 `Read`/`Write` 都会报错。`WriterOption` 和 `ReaderOption.PrefetchSize` 当前不影响状态。

路径有一个重要边界：`full_path`（`stubs.rs:185-192`）只是按 `/` 分段并跳过空段，没有拒绝 `.` 或 `..`。因此调用者必须传入可信的、相对于存储根的对象名；本桩没有实现路径穿越防护，也不应被当作面向不可信输入的文件服务。

## 依赖与调用关系

上游装配链为 `br/pkg/utiltest/lib.rs` 重导出 → `br/pkg/utiltest/suite.rs::CreateRestoreSchemaSuite` → `stubs.rs::NewLocalStorage` → `LocalStorage::new`。RustCodeGraph 的精确 explore 还确认 `br/pkg/utiltest/parity_test.rs::go_rust_public_contract_matches` 和 `local_storage_matches_go_error_walk_and_lifecycle_contracts` 直接调用 `NewLocalStorage`；文件索引将 `stubs.rs` 标为被 28 个 Rust 文件使用，但精确 `callers/callees` 对 trait 方法未生成边，因此本文不据此虚构额外生产调用链。

下游仅依赖 Rust 标准库的 `fs`、`io`、`path`、`Arc`、`Duration` 和外部 crate `tempfile`。主要内部调用边是：`NewLocalStorage → LocalStorage::new`，所有 `Storage` 操作 → `ensure_open`/`full_path`，`DeleteFiles → DeleteFile`，`WalkDir → walk_dir_recursive`，递归目录分支 → `walk_dir_recursive` 自调用。

`br/pkg/utiltest/Cargo.toml` 说明本 crate 是 library，Go 包映射为 `br/pkg/utiltest`；它没有依赖 `pkg/objstore` 的 Rust crate。canonical Go/Rust 对象存储实现分别位于 `pkg/objstore/local.go` 与 `pkg/objstore/local.rs`，本文件刻意复制测试所需的窄接口，不能作为这些实现的反向依赖或替代入口。

## 错误处理与边界

- 所有 I/O 错误都被压缩为 `Error { message, is_not_exist }`；没有保留原始 `io::Error` 供 source 链或错误码检查。只有 `ReadFile`、`DeleteFile`、`Open` 的 `NotFound` 分支设置 `is_not_exist = true`（`stubs.rs:282-309`、`350-364`）；例如 `Rename` 的源文件缺失仍是普通错误。
- `FileExists` 使用 `Path::exists`（`stubs.rs:295-298`），不能区分“不存在”和权限/元数据查询失败，后者也可能表现为 `false`。这是桩的简化边界。
- `Open` 显式拒绝负 `StartOffset`，但没有验证负 `EndOffset`、`EndOffset < StartOffset` 或起点超过文件长度；读逻辑会把非正剩余长度视作范围结束（`stubs.rs:365-377`、`213-225`）。
- `WalkDir` 对不存在的起点返回空成功（`stubs.rs:331-334`），但遍历过程中发生的 `read_dir`、entry、metadata 或回调错误会立即传播。`read_dir` 顺序未排序，所以回调顺序不是稳定契约。
- `WriteFile` 在目标父目录内创建临时文件并持久化，降低半写目标的风险；但 `persist` 或 Unix `chmod` 失败仍会返回错误。`Create` 则立即创建最终文件，不具备这一原子性。
- `PresignFile` 对本地后端只取文件名，不生成可访问 URL；空路径可得到空字符串。文件顶部已明确真实云厂商预签名逻辑不在此实现。

## 并发与资源生命周期

`Storage: Send + Sync` 和工厂返回的 `Arc<dyn Storage>` 允许多个测试组件共享同一根目录；实现自身没有锁。不同文件上的独立系统调用可并发执行，但同一路径上的写、删、重命名或遍历竞态没有由本文件串行化，结果遵从宿主文件系统。`WriteFile` 的临时文件名由 `tempfile` 生成，减少并发写的临时名冲突；最后提交仍是“最后一次成功 rename 的内容可见”。

`Context` 不会取消阻塞 I/O，也没有超时传播。`LocalStorage::Close` 是空操作，与 Go `(*LocalStorage).Close` 一致，所以关闭 storage 后仍可继续读写；真正资源由每个 `FileReader`/`FileWriter` 的句柄持有。显式 `Close` 会释放句柄，未显式关闭时 Rust 的 drop 仍会释放文件；不过 `FileWriter` 只有显式 `Close` 才报告 flush 错误。suite 通过 `_temp_dir` 持有目录，先于 suite drop 的正常字段销毁过程中保持路径有效（`br/pkg/utiltest/suite.rs:35-43`）。

## 与 Go 版本的对应关系

直接包级对照是 `br/pkg/utiltest/suite.go:27-46`：Go suite 使用 `objstore.NewLocalStorage(t.TempDir())` 得到 `storeapi.Storage`；Rust 的 `suite.rs:35-41,83-101` 保存 `TempDir` 并调用本文件工厂，达到相同的测试生命周期目标。

行为基准来自 `pkg/objstore/local.go`：Go `WriteFile` 同样先写临时文件再 rename，并使用 `0644`（`local.go:86-116`）；`DeleteFiles` 串行且首错返回（`75-84`）；`Open` 使用包含起点、排除终点的范围（`243-287`）；`Create` 直接打开最终目标并缓冲（`307-326`）；`PresignFile` 返回 basename、`Close` 为空操作（`333-341`）。这些是当前桩有意保留的核心语义。

本桩不是逐字段完整移植：Go `WalkOption` 还有 `ListCount`、`IncludeTombstone`、`StartAfter`，Go `WriterOption` 有 `Concurrency`、`PartSize`，完整接口和本地实现还处理墓碑、排序/起始点、非规则文件、failpoint、`CopyFrom` 等（`pkg/objstore/storeapi/storage.go:56-127,141-182`；`pkg/objstore/local.go:60-355`）。Rust 桩也把范围结束表示为 `Ok(0)`，而 Go reader 在结束处返回 `io.EOF`。扩展时必须先判断调用者需要测试替身语义还是 canonical `pkg/objstore` 语义，不能默认二者完全等价。

## 扩展指南

新增存储能力时，优先从 `Storage` trait 和相应 option 类型开始，并在 `impl Storage for LocalStorage` 中同步实现；若新增有状态流行为，则修改独立的 `FileReader`/`FileWriter`，不要把测试逻辑内嵌到本生产候选文件。公开 API 还需同步 `br/pkg/utiltest/lib.rs` 的重导出，套件接线变化则落在 `br/pkg/utiltest/suite.rs`。

测试应扩展同目录独立文件 `br/pkg/utiltest/parity_test.rs`，保持“源文件与 Rust 测试不在同一文件”的仓库约束。涉及 Go 行为对齐时同时核对 `pkg/objstore/storeapi/storage.go`、`pkg/objstore/local.go` 及其独立测试 `pkg/objstore/storeapi/storage_test.go`；不能因本 suite 暂未使用某分支而删减 Go 的必要语义。

需要特别评估的风险包括：改变 `WriteFile` 原子提交或权限会造成兼容性问题；改变 `Close` 为禁用后续 I/O 会破坏 suite 生命周期契约；为并发访问增加全局锁可能降低测试吞吐；新增路径规范化必须考虑现有对象名兼容，同时应补充 `..`、绝对路径和平台分隔符回归用例；扩展范围读需明确 EOF 表示是否继续沿用当前 Rust 测试契约。

## 验证依据

- RustCodeGraph 状态：本地索引包含 11,467 个文件、7,032 个 Rust 文件；`br/pkg/utiltest/stubs.rs` 收录 59 个符号，文件级索引显示 28 个使用文件。
- RustCodeGraph 源码与调用查询：读取了 `stubs.rs:1-477`、`lib.rs:1-33`、`suite.rs:1-106`、`parity_test.rs:1-201`；精确 explore 得到 `local_storage_matches_go_error_walk_and_lifecycle_contracts → NewLocalStorage`，并列出两个 parity 测试及 `CreateRestoreSchemaSuite` 的相关调用边。单独的精确 `callers/callees` 无输出，已作为图覆盖限制记录，而非解释为没有调用者。
- crate 与入口证据：`br/pkg/utiltest/Cargo.toml`、`br/pkg/utiltest/lib.rs`、`br/pkg/utiltest/suite.rs`；目标包不存在 `doc.go`。
- Go 对照证据：`br/pkg/utiltest/suite.go:27-46`、`pkg/objstore/storeapi/storage.go:56-182`、`pkg/objstore/local.go:44-383`。
- Rust 独立测试证据：`br/pkg/utiltest/parity_test.rs:18-201` 覆盖套件构造、`file://` URI、整文件往返、缺失分类、删除缺失、遍历过滤、basename 预签名、范围读、关闭后读失败、直接创建与关闭 storage 后继续读取。任务是纯文档分析，按计划不运行 Cargo。
