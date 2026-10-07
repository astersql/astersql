# `br/pkg/conn/lib.rs`

## 文件定位

`br/pkg/conn/lib.rs` 是 Cargo 包 `astersql-br-pkg-conn` 的 crate 根，而不是连接算法的实现文件。`br/pkg/conn/Cargo.toml` 通过 `[lib] path = "lib.rs"` 将它指定为库入口，并用 `package.metadata.porting.go-package = "br/pkg/conn"` 声明其 Go 对照包。生产构建中，本文件用 `#[path = "conn.rs"] pub mod conn` 装配 canonical 实现模块，再以 `pub use conn::*` 将其公开项平铺到 crate 根，因此调用方既可写 `astersql_br_pkg_conn::Mgr`，也可经内部模块路径理解其来源。

文件顶部的 crate 级 `allow` 放宽未使用项和 Go 风格命名等 lint，反映该 crate 正在保持 Go API 名称与分阶段移植面的现实；它不改变运行时行为。`parity_test.rs`、`conn_test.rs`、`main_test.rs` 仅在 `cfg(test)` 下成为 crate 内部模块，不进入普通依赖方的生产构建。

## 核心职责

本文件只有三项装配职责：

1. 把 `br/pkg/conn/conn.rs` 声明为公开子模块 `conn`。
2. 用 glob re-export 把 `conn.rs` 的公开常量、类型、trait、函数和 `units` 子模块暴露为 crate 根 API。
3. 在当前 crate 的测试构建中纳入三份独立测试文件，使测试能够通过 `crate::...` 或 `crate::conn::...` 检查公共契约与少量 crate-private 细节。

它自身不创建连接、不持有状态、不执行重试，也不实现关闭逻辑。所有这些行为的事实来源是 `br/pkg/conn/conn.rs`；把本文件视为业务实现会夸大它的职责。

## 主要符号

- `pub mod conn`：公开模块声明，借助 `#[path = "conn.rs"]` 将非默认目录布局中的实现文件绑定为 `conn`。这是本文件唯一的生产模块定义。
- `pub use conn::*`：公开门面。当前会再导出 `DefaultMergeRegionSizeBytes`、`DefaultMergeRegionKeyCount`、`DefaultImportNumGoroutines`、`NullspaceID` 等常量；`VersionCheckerType`、`StoreBehavior`、`StoreState`、`GrpcCode` 等枚举；`Store`、`StatusUrl`、`KVConfig`、`NewMgrDeps`、`Mgr` 等类型；`CancelContext`、`HttpClient`、`StoreMeta`、`StoreManagerHandle`、`PdControllerHandle`、`GcManagerHandle`、`MgrLifecycleHandle` 等边界 trait；以及 `NewMgr`、`GetAllTiKVStores*`、`GetConfigFromTiKV`、`HandleTiKVAddress` 等函数。完整签名和语义属于 `conn.rs`，不是在此重复定义。
- `mod parity_test`：仅测试态纳入 `br/pkg/conn/parity_test.rs`，检查 Go/Rust 公共契约清单与源码形态。
- `mod conn_test`：仅测试态纳入 `br/pkg/conn/conn_test.rs`，承载连接、重试、配置聚合、地址处理和关闭顺序等行为回归。
- `mod main_test`：仅测试态纳入 `br/pkg/conn/main_test.rs`，保持 Go `main_test.go` 暴露私有测试入口的意图；Rust 侧验证 `CheckStoresAlive`、`HandleTiKVAddress` 可达。

本文件没有函数、结构体、trait、常量或条件 feature；条件编译只作用于上述三个测试模块。

## 执行流程

编译期流程如下：Cargo 读取 `Cargo.toml` 并载入本文件；编译器解析 crate 级 lint 设置；`#[path]` 将 `conn.rs` 编译为公开的 `conn` 模块；glob re-export 将其公开符号加入 crate 根名称空间。若执行该 crate 的测试，编译器还会装配三份独立测试模块；普通库构建则在 `cfg(test)` 处停止，不包含这些测试代码。

运行期没有从 `lib.rs` 开始的独立控制流。外部调用实际落入 `conn.rs`：典型路径是调用 `NewMgr` 完成 PD 控制器、版本/存活性检查、storage/domain 与 manager 组装；调用 `GetAllTiKVStoresWithRetry` 获取并按 `StoreBehavior` 过滤节点；调用 `GetConfigFromTiKV` 或 `Mgr::ProcessTiKVConfigs` 访问各个存活 TiKV 的 `/config`；最终由 `Mgr::Close` 幂等地按 StoreManager、Domain/owner/storage、PD 的顺序释放资源。`br/pkg/task/stream.rs` 直接从 crate 根使用 `Mgr`、`HttpClient`、`ConfigTerm`、`KVConfig` 和 `BackgroundContext`，证明 re-export 是实际跨 crate 接线，而不只是便利别名。

## 数据与状态

`lib.rs` 不分配或保存数据。它暴露的状态模型均来自 `conn.rs`：`Mgr` 持有 `Arc<dyn PdControllerHandle>`、可选 `Domain`/`Storage`、store/GC/lifecycle 句柄、`ownsStorage` 以及由 `Mutex<bool>` 保护的关闭标志；`CancelledContext` 用 `Arc<AtomicBool>` 传播取消；进程级 `STORE_IS_TIKV` 用原子布尔量记录成功绑定 TiKV 的事实；`KVConfig` 聚合导入并发与 region 分裂阈值，并用 `ConfigTerm::Modified` 区分用户显式值和自动探测值。

由于 `pub use conn::*`，任何新增或删除的 `pub` 项都可能无须修改本文件便改变 crate 根公共面。这是该门面的主要兼容性属性，也是审查 `conn.rs` 公共符号变化时必须关注的隐式状态。

## 依赖与调用关系

上游方面，`br/pkg/task/Cargo.toml` 以路径依赖引入本 crate；`br/pkg/task/stream.rs`、`restore.rs`、`restore_lifecycle.rs` 直接使用根级再导出。仓库内其他 BR 模块也通过 `conn::...` 使用实现；RustCodeGraph 将 `conn.rs` 标为被 58 个文件使用，说明真实影响面集中在实现模块及其导出 API，而 `lib.rs` 只负责名称空间接线。RustCodeGraph 对 `lib.rs` 的文件级索引只识别到一个模块装配符号，并显示 `tools/tazel/parity_test.rs` 的结构性引用；这不能替代 Cargo 依赖和根级符号引用证据。

下游方面，门面直接依赖 `conn.rs`。后者再依赖 `astersql-br-pkg-errors`、`astersql-br-pkg-glue`、`astersql-br-pkg-version`、`astersql-errors`、`fail`、`serde` 与 `serde_json`，均由 `br/pkg/conn/Cargo.toml` 声明。该清单特意保持平台精简：注释明确 darwin arm64 不直接接入 kv/domain/kvproto/grpcio，而以本地 trait 和轻量句柄表达 PD、HTTP、storage 与客户端边界。因此不能仅凭 Go 版本依赖推断 Rust 已拥有完整 gRPC/TiKV 实现。

## 错误处理与边界

本文件没有可失败操作，也不捕获或改写错误；来自 `conn.rs` 的 `SharedError` 及相关结果类型被原样再导出。实际边界包括：PD/store 查询错误经 `Trace` 或注解传播；激进重试只对取消/未知类 gRPC 状态有限重试并聚合错误；无 StoreManager 的客户端请求返回明确错误；无效 URL、TiKV 配置 JSON 或尺寸后缀返回解析错误；取消上下文使配置抓取和客户端获取快速失败。

门面层特有的边界是 API 暴露范围。`pub use conn::*` 只导出 `pub` 项，不会导出 `pub(crate)` 的 `keyspace_id_for_gc` 或私有解析辅助函数。测试模块因位于同一 crate 内可验证这些 crate-private 细节，例如 `conn_test.rs` 通过 `crate::conn::keyspace_id_for_gc` 检查 keyspace 投影；外部依赖方不能依赖它们。

## 并发与资源生命周期

`lib.rs` 自身没有线程、锁、任务、通道、网络句柄或析构动作。它暴露的并发与资源约束来自 `conn.rs`：取消标志使用顺序一致的原子读写；`Mgr::Close` 用互斥量保证幂等关闭；共享 PD、storage 与 manager 句柄使用 `Arc`；重试路径会同步休眠；测试中的全局 failpoint 由独立互斥锁串行化，避免并行用例互相污染。

资源所有权由 `Mgr::ownsStorage` 决定。只有拥有 storage 时，关闭流程才触发 Domain、DDL owner、TiKV shutdown 和 storage 关闭；PD 控制器始终在末尾关闭。门面不额外包装这些类型，因此调用者从 crate 根取得的 `Mgr` 与 `conn::Mgr` 是同一类型，不存在第二套生命周期。

## 与 Go 版本的对应关系

Go 的 `br/pkg/conn/conn.go` 是单个 package 文件，公开名称天然属于 `conn` 包；Rust 将相同职责拆为 `lib.rs` crate 根和 `conn.rs` 实现，再由 `pub use conn::*` 恢复近似的平坦公共面。`Cargo.toml` 的 porting metadata 明确把该 crate 映射到 `br/pkg/conn`。

核心语义映射包括：Go `Mgr` 对应 Rust `Mgr`；Go `VersionCheckerType`、默认配置常量和 store 过滤/重试入口保留同名或近同名 API；Go `context.Context`、`http.Client`、PD client、StoreManager、GC manager 等具体接口在 Rust 精简移植中由 `CancelContext`、`HttpClient`、`StoreMeta` 等 trait 表达。Go `main_test.go` 通过包变量暴露私有函数，Rust `main_test.rs` 则直接再导出已经公开的对应函数。

差异必须如实保留：Rust 当前的本地 trait/句柄不是完整 kvproto/grpcio 客户端；`CheckStoresAlive` 当前只统计 Up store 而不强制数量非零；`GcManagerHandle` 是空边界 trait；部分 StoreManager 方法默认返回“未实现”。这些是 `conn.rs` 和 Cargo 平台裁剪的当前事实，`lib.rs` 的平铺导出不意味着它们已经达到 Go 全功能实现。

## 扩展指南

- 新增连接能力应在 `conn.rs` 的 owning 类型或函数中实现；仅当需要新的模块边界时才修改 `lib.rs`。公开项会被 glob 自动导出，新增 `pub` 前应评估 crate 根兼容面，避免无意公开内部 helper。
- 若拆分新的生产子模块，应显式决定它是仅作为 `conn` 内部实现、公开子模块，还是需要根级再导出；不要依赖多层 glob 形成难以追踪的 API。
- 修改 PD/store 抽象、重试、地址或配置逻辑时，同步扩展独立的 `br/pkg/conn/conn_test.rs`；公共 Go/Rust 名称和契约变化同步更新 `parity_test.rs`；测试入口可达性变化更新 `main_test.rs`。不要把测试写进 `lib.rs` 或 `conn.rs`。
- 对照 Go 时先核对 `br/pkg/conn/conn.go` 与 `conn_test.go` 的可观察行为，再评估 Rust 平台裁剪是否允许等价接线；不得用门面存在来推断具体依赖已经可用。
- 变更关闭顺序或所有权时重点检查重复 `Close`、`ownsStorage` 两个分支和部分初始化失败；变更配置抓取时检查多 store、非 Up/TiFlash、取消、HTTP 重试、无效 JSON 与保守聚合规则。性能风险主要来自逐 store 串行 HTTP 和同步退避，不在本门面文件中修复。

## 验证依据

- RustCodeGraph：`status` 确认索引包含 7,032 个 Rust 文件；`files --filter br/pkg/conn` 确认目标、实现及独立测试文件；`node --file br/pkg/conn/lib.rs` 核对 36 行 crate 根和唯一生产装配关系；`node --file br/pkg/conn/conn.rs` 核对公开类型、函数、`Mgr` 状态与主要流程；`explore`/文件使用信息确认 `conn.rs` 的跨模块影响面。
- crate 边界：`br/pkg/conn/Cargo.toml` 验证包名、`[lib]` 路径、Go package metadata、平台裁剪说明和直接依赖；`br/pkg/task/Cargo.toml` 验证实际上游路径依赖。
- Rust 调用证据：`br/pkg/task/stream.rs` 使用根级 `Mgr`、`HttpClient`、`ConfigTerm`、`KVConfig`、`BackgroundContext`；`br/pkg/task/restore.rs` 使用根级 `DefaultImportNumGoroutines`；`br/pkg/restore/log_client/import.rs` 与 `br/pkg/backup/client.rs` 使用 store 查询入口。
- Go 对照：读取 `br/pkg/conn/conn.go`，核对 `Mgr`、`NewMgr`、store 重试、配置处理、日志备份检查、地址处理和关闭顺序；读取 `br/pkg/conn/conn_test.go` 与 `main_test.go` 的测试入口清单。
- Rust 测试：`br/pkg/conn/conn_test.rs` 覆盖取消与重试、store 筛选、配置聚合、log-backup、地址与生命周期；`br/pkg/conn/parity_test.rs` 覆盖公开契约；`br/pkg/conn/main_test.rs` 覆盖测试导出可达性。它们由本文件的三个 `cfg(test)` 模块声明独立装配。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前以任务规定的命令确认目标文件存在且恰有 11 个固定二级标题，并人工复核文档将门面装配与 `conn.rs` 真实逻辑明确分开。
