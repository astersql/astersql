# `pkg/ddl/ingest/backend_mgr.rs`

## 文件定位

本文件属于 Cargo crate `astersql-ddl-ingest`（入口见 `pkg/ddl/ingest/Cargo.toml` 的 `[lib] path = "lib.rs"`），位于 DDL add-index/modify-column 的 ingest 快速回填子系统。它处在“创建单个 DDL job 的内存后端上下文”和“把 job ID 映射为本地排序目录名”这两个边界上：前者通过 `BackendContextBuilder` 组装 `crate::backend::BackendContext`，后者通过 `generate_job_sort_path`、`encode_backend_tag`、`decode_backend_tag` 统一目录命名。

当前 Rust 接线必须与设计目标区分：`lib.rs` 公开了 `backend_mgr` 模块，`env.rs` 已直接调用标签编码/解码函数；但仓库内除测试外没有 `BackendContextBuilder` 或 `generate_job_sort_path` 的 Rust 调用者。因此，它目前是部分可用的移植组件，而不是 Go `pkg/ddl/ingest/backend_mgr.go` 所实现的完整后端创建、注册和全局资源登记流程。

## 核心职责

1. `BackendContextBuilder` 保存 job 标识与可选 checkpoint 参数，并用链式 API 区分普通构建、查重标记与断点恢复配置；内存/磁盘配额根则在最终 `build` 调用时传入。
2. `BackendContextBuilder::build` 在配置了 `CheckpointStorage` 时创建 `CheckpointManager`，随后把 job ID、内存配额根、磁盘配额根和可选 checkpoint 交给 `BackendContext::new`。
3. `encode_backend_tag` 为普通后端生成十进制 job ID，为查重后端追加 `-dup`；`generate_job_sort_path` 把该标签安全拼接到给定基目录。
4. `decode_backend_tag` 只解析纯整数标签。这个非对称约定是有意的：环境清理逻辑先用普通目录识别 job，再同时处理普通目录和对应的 `-dup` 目录（`pkg/ddl/ingest/env.rs::stale_temp_directories`）。

本文件不负责创建 Lightning backend、不注册 engine、不执行导入、不维护全局 backend map，也不直接读写检查点；这些能力分别属于其他 Rust 模块，或仍只存在于 Go 实现中。

## 主要符号

- `pub struct BackendContextBuilder`：公开的构建参数容器。字段均为 `pub`：`job_id: i64` 标识 DDL job；`check_duplicates: bool` 记录查重模式；`import_ts: u64`、`start_key: Key`、`instance_addr: String` 是 checkpoint 初始化参数；`checkpoint_storage: Option<Arc<dyn CheckpointStorage>>` 是可跨线程共享的持久化抽象。
- `BackendContextBuilder::new(job_id)`：建立默认状态；`check_duplicates=false`、`import_ts=0`、无 checkpoint storage、空 start key 和空实例地址。
- `BackendContextBuilder::for_duplicate_check(self)`：消费并返回 builder，把 `check_duplicates` 置为 `true`。当前 `build` 并不读取该字段，故它只保存意图，尚不会改变生成的 `BackendContext` 或目录。
- `BackendContextBuilder::with_checkpoint(self, storage, start_key, import_ts, instance_addr)`：一次性填充 checkpoint 所需参数。`instance_addr` 接受 `impl Into<String>`。
- `BackendContextBuilder::build(self, config, mem_root, disk_root)`：返回 `Result<BackendContext, String>`。`config` 当前被显式忽略；存在 storage 时调用 `CheckpointManager::new`，成功后调用 `BackendContext::new`。
- `generate_job_sort_path(base, job_id, check_duplicates)`：通过 `Path::join` 拼路径，再以 `to_string_lossy().into_owned()` 返回 UTF-8 `String`。
- `encode_backend_tag(job_id, check_duplicates)`：普通标签为 `{job_id}`，查重标签为 `{job_id}-dup`。
- `decode_backend_tag(name)`：使用 Rust `str::parse::<i64>()`，失败时返回 `invalid backend tag {name}`。它接受带正负号的合法 `i64`，拒绝空串、空白、溢出值和 `-dup` 后缀。

文件没有模块级常量、trait、自定义 enum、条件编译项或私有函数；所有行为入口都是公开符号。

## 执行流程

构建后端上下文的流程如下：

1. 调用者以 `BackendContextBuilder::new(job_id)` 创建默认 builder。
2. 如需标记查重，可调用 `for_duplicate_check`；如需恢复/推进 checkpoint，则调用 `with_checkpoint` 注入 storage、起始 key、import TSO 与实例地址。两者都按值消费并返回 builder，适合链式调用。
3. `build` 将 `Option<Arc<dyn CheckpointStorage>>` 映射为 `Option<Result<CheckpointManager, String>>`，再用 `transpose()` 转换为 `Result<Option<CheckpointManager>, String>`；`?` 保证 storage 加载失败时立即返回，不创建半初始化的 backend context。
4. checkpoint 不存在或成功创建后，`BackendContext::new(job_id, mem_root, disk_root, checkpoint)` 初始化空 engine map、配额引用、刷新时钟等后续运行状态（直接实现见 `pkg/ddl/ingest/backend.rs`）。

目录流程独立于 builder：`generate_job_sort_path` 先调用 `encode_backend_tag`，再使用平台路径规则拼接 base 与标签。`env.rs::generate_ingest_temp_data_dir` 直接复用同一编码规则；清理流程通过 `decode_backend_tag` 只从普通目录收集 job ID，再为过期 job 分别重新编码普通标签和查重标签。

## 数据与状态

`BackendContextBuilder` 是一次性值对象，没有内部锁或全局注册表。`new` 给出确定的默认状态，链式方法只修改自身字段；`build(self, ...)` 消费 builder，阻止同一组参数被意外重复构建。

checkpoint 状态通过 `Arc<dyn CheckpointStorage>` 共享。`CheckpointManager::new` 会尝试加载既有 `ReorgCheckpoint`，或以 `start_key`、`import_ts`、`instance_addr` 创建新水位线；本文件只决定是否创建该管理器，不推进或持久化水位线。生成的 `BackendContext` 持有 `Arc<dyn MemRoot>` 和可克隆的 `DiskRoot`，并在自身内部管理 engine、导入次数、关闭标记和刷新时刻。

`check_duplicates` 当前存在状态断层：它由 `for_duplicate_check` 设置，却没有传入 `BackendContext::new`，也没有在 `build` 中用于目录选择。只有显式调用 `encode_backend_tag` 或 `generate_job_sort_path` 时，调用参数才真正影响目录名。扩展时不能假定 builder 的查重标记已经产生运行时效果。

## 依赖与调用关系

下游直接依赖如下：

- `crate::backend::BackendContext::new`：接收 builder 的核心结果。
- `crate::checkpoint::{CheckpointManager, CheckpointStorage, Key}`：提供可选 checkpoint 构建、存储 trait 与 key 类型。
- `crate::config::IngestConfig`：保留在 `build` 签名中，但当前实现未读取。
- `crate::mem_root::MemRoot` 与 `crate::disk_root::DiskRoot`：把共享内存计量器和磁盘状态交给 backend context。
- `std::path::Path`：提供跨平台路径拼接；`std::sync::Arc`：共享 trait object。

已验证的 Rust 上游调用边是：`env.rs::generate_ingest_temp_data_dir -> encode_backend_tag`、`env.rs::processing_job_ids -> decode_backend_tag`、`env.rs::stale_temp_directories -> decode_backend_tag/encode_backend_tag`。`generate_job_sort_path -> encode_backend_tag` 是文件内调用边。RustCodeGraph 将本文件标为被 `env.rs`、`backend_mgr_test.rs`、`env_test.rs` 使用；仓库文本检索没有找到 builder 的生产调用者。

应用层语境来自 Go 主链：`pkg/ddl/backfilling.go`、`backfilling_read_index.go`、`backfilling_import_cloud.go` 和 `index.go` 调用 Go `NewBackendCtxBuilder`，服务 add-index/modify-column reorg。但这些是 Go 对照证据，不代表 Rust builder 已接入相同调用链。

## 错误处理与边界

`build` 唯一显式错误源是 `CheckpointManager::new`，错误类型为 `String`，原样通过 `?` 返回。checkpoint 创建失败时不会构造 `BackendContext`。没有 storage 时完全跳过 checkpoint 加载，因此 `start_key`、`import_ts` 和 `instance_addr` 不产生效果。

`decode_backend_tag` 把所有整数解析失败统一映射成带原始名字的错误字符串，不暴露 `ParseIntError` 的细分类别。它故意不解码 `42-dup`，所以不能用它独立发现“只有查重目录、没有普通目录”的 job；这与 Go `strconv.ParseInt(name, 10, 64)` 及清理策略一致。负数和前导 `+` 可被解析，测试明确固定了这一契约；前后空白不可接受。

`generate_job_sort_path` 不访问文件系统，不检查 base 是否存在、可写或容量充足；非 UTF-8 平台路径经 `to_string_lossy` 可能发生替换，因此返回值不是无损的 `PathBuf`。它也不会从全局 ingest 环境取根路径，这一点不同于 Go `genJobSortPath`。

## 并发与资源生命周期

builder 本身没有并发行为。checkpoint storage 要求 `Send + Sync`，并由 `Arc` 持有，因此可安全共享所有权；内存根同样以 `Arc<dyn MemRoot>` 传入。`DiskRoot` 内部使用共享同步状态（定义见 `disk_root.rs`），按值交给 `BackendContext` 不等于复制独立配额。

生命周期边界是 `build`：构建成功后，资源所有权进入 `BackendContext`；后续 engine 注册、配额决策、checkpoint 刷新与关闭由 `backend.rs`/`checkpoint.rs` 处理。本文件不启动线程、异步任务或通道，也不创建/删除排序目录。它没有实现 Go 侧的 `LitDiskRoot.Add`、backend 计数器、failpoint、PD client、etcd 分布式锁或关闭失败后的清理，因此这些生命周期保证不能从当前 Rust 文件推导。

## 与 Go 版本的对应关系

直接对照文件为 `pkg/ddl/ingest/backend_mgr.go`：

- Rust `BackendContextBuilder` 对应 Go `BackendCtxBuilder` 的缩减版本；`new`/`for_duplicate_check`/`build` 分别近似对应 `NewBackendCtxBuilder`/`ForDuplicateCheck`/`Build`。
- Rust `encode_backend_tag` 与 Go `encodeBackendTag` 的字符串规则一致；Rust `decode_backend_tag` 与 Go `decodeBackendTag` 的十进制 `int64` 解析意图一致。`backend_mgr_test.rs` 和 `env_test.rs` 固定了符号、空白与 `-dup` 行为。
- Rust `generate_job_sort_path(base, ...)` 只拼接显式 base；Go `genJobSortPath` 先从 `GenIngestTempDataDir` 获取已初始化的全局根目录，因而还可能返回环境未初始化错误。
- Go builder 持有 context、KV storage、完整 `model.Job`、etcd client、session pool、physical/subtask ID 与 dist-task 回调，并会校验 job 类型和 `ReorgMeta`、创建普通或分布式 checkpoint manager、处理 failpoint/mock、注册 `LitDiskRoot` 与计数器。Rust builder 没有这些字段和行为。
- Go `checkDup` 会影响 `Build` 使用的 job sort path；Rust `check_duplicates` 当前不被 `build` 使用。Go `Build` 接收真实 ingest backend/config，Rust `build` 只把本 crate 的配额与 checkpoint 装入简化的 `BackendContext`，且 `IngestConfig` 当前未生效。

因此当前移植状态应描述为：目录标签契约和基础 checkpoint/context 组装已具备，完整 Lightning backend 创建、分布式任务 checkpoint、锁、全局登记和生产主链接线尚未由本文件实现。

## 扩展指南

- 接入 builder 到生产 Rust 主链时，应先明确调用者拥有的是 job ID 还是完整 job 元数据；如果要对齐 Go，需补齐 job 类型/ReorgMeta 校验、排序根目录获取、backend 资源及失败清理，而不能仅让现有 `build` 被调用就声称完成移植。
- 若让 `check_duplicates` 生效，应在 `BackendContextBuilder::build` 与路径/backend 创建处形成可验证的数据流，避免同一字段只记录不消费；同步扩展独立文件 `pkg/ddl/ingest/backend_mgr_test.rs`，不要把测试内嵌进生产源文件。
- 若启用 `IngestConfig`，应在真正消费配置的构造边界验证并发、缓存与目录配置；当前 `_ = config` 是明确的未接线信号，删除它前必须增加行为测试。
- 修改标签格式必须同步检查 `env.rs` 的活跃 job 识别和双目录清理，并更新 `backend_mgr_test.rs`、`env_test.rs`；格式还与已有磁盘目录兼容性相关，尤其不能让 `-dup` 目录被误判为独立普通 job。
- 扩充 checkpoint 参数时，优先复用 `CheckpointManager::new_with_resume_options` 的物理表 ID 和本地数据有效性语义；需要验证实例切换、physical ID 不匹配、storage load 失败以及无本地数据时的恢复边界。
- 保持资源失败原子性：任何新增的目录、backend、全局登记或计数操作都应在后续步骤失败时可回滚/关闭，并与 DDL owner 转移、重试和取消语义兼容。

## 验证依据

- RustCodeGraph：`status` 显示索引包含 11,467 个文件；`files --filter pkg/ddl/ingest/backend_mgr.rs` 显示目标文件有 16 个符号并被 `backend_mgr_test.rs`、`env.rs`、`env_test.rs` 使用；`node --file ... --offset 1 --limit 420` 核对了目标文件全部 150 行；`query` 唯一定位 `BackendContextBuilder`，并定位三个公开辅助函数。`callers/callees` 没有返回可用边，调用关系随后以已定位文件的直接引用补证。
- 目标源码：`pkg/ddl/ingest/backend_mgr.rs`，核对 builder 字段、三个方法、三个路径/标签函数以及不存在条件编译项的事实。
- crate 与模块边界：`pkg/ddl/ingest/Cargo.toml`、`pkg/ddl/ingest/lib.rs`；Cargo 元数据将该 crate 对应到 Go package `pkg/ddl/ingest`，当前大量完整依赖只在 `cfg(windows)` 下声明。
- 直接 Rust 依赖：`pkg/ddl/ingest/backend.rs`、`checkpoint.rs`、`config.rs`、`mem_root.rs`、`disk_root.rs`；用于核对构造参数、checkpoint 恢复和共享资源含义。
- Rust 调用者与独立测试：`pkg/ddl/ingest/env.rs`、`backend_mgr_test.rs`、`env_test.rs`；覆盖标准/查重标签、整数解析、常见 base 路径、活跃 job 筛选及过期双目录清理。按任务约束，本次纯文档分析未运行 Cargo 测试。
- Go 对照与主链：`pkg/ddl/ingest/backend_mgr.go`、`env.go`，以及 `pkg/ddl/backfilling.go`、`backfilling_read_index.go`、`backfilling_import_cloud.go`、`index.go` 中的 builder 调用；用于界定已对齐逻辑和未移植职责。
- DDL 语境：`pkg/ddl/doc.go`、`docs/agents/ddl/README.md`、`03-reorg-backfill.md`、`06-add-index.md`；仅作为入口假设，具体结论均由上述源码和测试复核。
