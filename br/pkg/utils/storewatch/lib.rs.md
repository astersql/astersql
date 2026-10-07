# `br/pkg/utils/storewatch/lib.rs`

## 文件定位

[`lib.rs`](lib.rs) 是 Cargo 包 `astersql-br-pkg-utils-storewatch` 的 crate 根。根工作区的 `Cargo.toml` 将 `br/pkg/utils/storewatch` 列为 workspace member，而本目录的 `Cargo.toml` 通过 `[lib] path = "lib.rs"` 指定本文件为库入口，并用 `package.metadata.porting.go-package = "br/pkg/utils/storewatch"` 记录 Go 来源包。

这个文件不是 store 生命周期状态机的实现文件。它负责建立 crate 边界、挂载 [`watching.rs`](watching.rs)，并把该模块的公开项扁平再导出到 crate 根。当前仓库没有任何其他 Rust `Cargo.toml` 声明依赖 `astersql-br-pkg-utils-storewatch`；Rust 的 `br/pkg/backup/store.rs` 和 `br/pkg/restore/data/data.rs` 仍分别通过同模块的 `stubs.rs` 使用局部替身。因此，本 crate 当前属于已独立移植并可单独验证、但尚未接入这些 Rust 主链的库。Go/Bazel 路径则由本目录 `BUILD.bazel` 的 `go_library(name = "storewatch")` 接入 `br/pkg/backup` 和 `br/pkg/restore/data`。

## 核心职责

本文件只有四项边界职责：

1. 用 crate 级 `#![allow(...)]` 暂时放宽迁移代码中的未使用项和 Go 风格命名告警，包括 `dead_code`、三类命名 lint、`unused_imports` 与 `unused_variables`。
2. 通过 `#[path = "watching.rs"] pub mod watching;` 声明唯一生产模块，令调用方既可使用 `watching::Watcher`，也可经根再导出访问。
3. 通过 `pub use watching::*;` 将 `StoreState`、`Store`、`Callback`、`DynCallback`、`DynCallbackOpt`、`StoreMeta`、`Watcher` 以及 `WithOn*`、`MakeCallback`、`New` 等公开 API 暴露在 crate 根。
4. 仅在测试构建中挂载 `parity_test.rs` 与 `watching_test.rs`；测试与生产源码保持分文件，普通库构建不会编译这两个模块。

真正的轮询、快照差分和回调触发都在 `watching.rs`，不能把 `lib.rs` 视为另一份实现。

## 主要符号

- `mod parity_test`：受 `#[cfg(test)]` 保护的私有测试模块。它从 crate 根导入公开项，验证 Go/Rust 公共契约、回调顺序、缓存清理和错误注解，也间接证明 `pub use watching::*` 生效。
- `pub mod watching`：公开生产模块，文件由显式 `#[path = "watching.rs"]` 绑定。其核心类型是泛型 `Watcher<C: Callback, M: StoreMeta>`，核心入口是 `New(cli, cb)` 与 `Watcher::Step(&mut self)`。
- `mod watching_test`：受 `#[cfg(test)]` 保护的私有测试模块，逐项对照 Go 的注册、掉线和重启场景。
- `pub use watching::*`：通配公开再导出。测试中的 `use crate::{MakeCallback, New, Store, ...}` 是该导出面的直接编译期证据。

本文件不定义常量、结构体、枚举、trait、函数或 `impl`，也没有 feature gate；公开业务符号全部来自 `watching` 模块。

## 执行流程

普通库构建时，编译器先应用 crate 级 lint 配置，再把 `watching.rs` 作为公开模块编译，最后将其全部公开项再导出到 crate 根。两个测试模块因 `cfg(test)` 为假而不进入产物。

测试构建时，两个独立测试文件同时作为 crate 内私有模块编译。它们使用 crate 根导出的 API 构造 `StoreMeta` 替身与回调，再推进 `Watcher::Step`。直接实现中的单步流程是：

1. `StoreMeta::GetAllTiKVStores` 拉取当前 store 快照；失败时 `Step` 加上 `failed to update store list` 前缀并立即返回。
2. 遍历快照，`updateStore` 先用 store ID 替换 `lastStores` 中的记录：首次出现触发 `OnNewStoreRegistered`；上一状态为 `Up`、新状态为 `Offline` 时触发 `OnDisconnect`；`StartTimestamp` 变化时独立触发 `OnReboot`。
3. 记录本轮出现的 ID，并由 `retain` 删除本轮消失的缓存项。相同 ID 以后重新出现会再次被当作新注册。

以上流程属于 `watching.rs` 的公开 API 语义；`lib.rs` 的作用是让调用方无需知道实现文件布局即可使用这些入口。

## 数据与状态

`lib.rs` 自身不持有运行时数据或全局状态。经它导出的实现以 `Store { Id, State, StartTimestamp }` 表示差分所需的最小 store 视图，以 `HashMap<u64, Store>` 保存上一轮快照，以临时 `HashSet<u64>` 记录本轮仍存在的 store ID。

状态机的重要不变量来自 `Watcher::updateStore` 和 `Watcher::retain`：`lastStores` 在一次成功 `Step` 结束后只包含本轮返回的 ID；每个 ID 保存最近一次观察值；首次观察先触发注册回调；掉线判定只接受 `Up -> Offline`；重启判定只比较启动时间戳，且与状态迁移判定相互独立。因此同一次更新若同时发生 `Up -> Offline` 和时间戳变化，会按“掉线、重启”的顺序触发两个回调。

## 依赖与调用关系

crate 边界由根 `Cargo.toml` 的 workspace member 与本目录 `Cargo.toml` 的 `[lib]` 声明确定。本目录 Cargo manifest 没有 `[dependencies]`，实现仅使用标准库的 `HashMap`、`HashSet` 和函数 trait；PD/kvproto 类型被裁剪为本地 `Store`，PD 查询被抽象为本地 `StoreMeta` trait。

文件内部的结构边为 `lib.rs -> watching.rs`，公开面为 `lib.rs::pub use -> watching::*`，测试边为 `lib.rs[cfg(test)] -> parity_test.rs` 和 `watching_test.rs`。RustCodeGraph 将 `watching.rs` 的 `Step` 关联到状态查询、`updateStore` 和 `retain`，并显示 `GetId`、`GetState` 由这些路径调用。

当前 Rust 应用层没有依赖本 crate 的 Cargo 接线。`br/pkg/backup/store.rs::ObserveStoreChangesAsync` 使用 `br/pkg/backup/stubs.rs::storewatch` 的局部 `MakeCallback`/`Watcher`；`br/pkg/restore/data/data.rs::SpawnTiKVShutDownWatchers` 使用 `br/pkg/restore/data/stubs.rs` 的 `StoreWatcher`。这些同名替身不是本 crate 的调用者。Go 版本的真实上游包括 `br/pkg/backup/store.go` 和 `br/pkg/restore/data/data.go`，其 Bazel 目标分别依赖 `//br/pkg/utils/storewatch`。

## 错误处理与边界

crate 根不产生或转换错误。公开实现的错误边界是 `StoreMeta::GetAllTiKVStores() -> Result<Vec<Store>, String>`：`Watcher::Step` 将下游错误格式化为 `failed to update store list: {e}`，并在更新任何快照或触发回调前返回。`parity_test.rs::step_annotates_store_list_errors` 固定了该文本和提前返回行为。

回调未配置不是错误：`DynCallback` 的三个字段为 `Option`，对应方法在 `None` 时静默跳过。回调闭包、用户提供的 `StoreMeta` 实现和锁若 panic，当前接口不捕获 panic。列表中的重复 ID 会按遍历顺序多次执行 `updateStore`，最后一次值留在缓存；实现没有显式拒绝重复 ID。`StoreState` 只建模 `Up`、`Offline`、`Tombstone`，不是完整 kvproto 类型。

与 Go 相比，Rust `Step` 不接收 `context.Context`，也不在本 crate 内调用 `conn.GetAllTiKVStoresWithRetry(..., SkipTiFlash)`；取消、重试、TiFlash 过滤及真实 PD 适配必须由未来的 `StoreMeta` 实现或上层接线承担。目前不能声称本 crate 已提供这些 Go 运行时能力。

## 并发与资源生命周期

`lib.rs` 不创建线程、任务、通道、锁、计时器或网络连接。`Watcher::Step` 需要 `&mut self`，因此单个 watcher 的快照更新在类型层面要求可变独占访问；实现本身没有内部同步。`Watcher` 拥有 `StoreMeta`、回调和缓存，随 watcher 一起析构，没有显式 `Close`/`Stop` 生命周期。

闭包选项要求 `Fn(&Store) + Send + Sync + 'static`，所以捕获环境必须满足跨线程约束；但这只让回调对象具备相应能力，不代表 watcher 会自行并发调用它。`DynCallbackOpt` 是 `FnOnce(&mut DynCallback)`，只在 `MakeCallback` 构造期间消费一次。测试使用 `Arc<Mutex<_>>`、`AtomicBool` 只是为了安全观察回调结果，不是生产实现内部的同步机制。

Go 上游 `ObserveStoreChangesAsync` 用后台 goroutine 和 ticker 周期调用 `Step(ctx)`；Rust `br/pkg/backup/store.rs` 的局部替身版本用 `thread::spawn` 和 sleep 驱动。由于它们当前没有接入本 crate，这些线程与取消行为不能归属于 `lib.rs`。

## 与 Go 版本的对应关系

Rust 的 crate 根对应 Go 包声明与包级公开面；业务语义逐项移植在 `watching.rs`。`Callback`、`DynCallback`、三个 `WithOn*` 选项、`MakeCallback`、`Watcher`、`New`、`Step`、`updateStore` 和 `retain` 都有直接的 Go 对应物。`watching_test.rs` 对照 `watching_test.go` 的三项基础场景，`parity_test.rs` 额外固定了消失后重现、同轮掉线与重启顺序以及错误注解。

仍存在明确差异：

- Go 直接使用 `metapb.Store` 和 `util.StoreMeta`；Rust 定义裁剪后的本地 `Store` 与 `StoreMeta`。
- Go `MakeCallback(opts ...DynCallbackOpt) Callback` 返回接口值；Rust 接收 `Vec<DynCallbackOpt>` 并返回具体 `DynCallback`。
- Go `New` 返回 `*Watcher`，内部使用动态接口；Rust 返回按回调和元数据类型单态化的 `Watcher<C, M>`。
- Go `Step(ctx)` 通过 `conn.GetAllTiKVStoresWithRetry` 带上下文重试并跳过 TiFlash；Rust `Step()` 只调用抽象方法一次。
- Go 包已被备份与恢复 Go 代码实际引用；独立 Rust crate 尚无其他 Cargo 消费者，相关 Rust 路径使用局部替身。

因此当前 Rust 实现适合验证状态差分核心，但不能被描述为 Go 包在完整 BR 主链中的等价接线。

## 扩展指南

若只是新增公开实现项，应在 `watching.rs` 实现；只要保持为 `pub`，当前 `pub use watching::*` 会自动暴露它。新增生命周期事件时需要同步修改 `Callback`、`DynCallback` 字段与委托、构造选项、`Watcher::updateStore` 判定，并在独立的 `watching_test.rs` 或 `parity_test.rs` 中补场景，不能把测试内嵌回 `lib.rs`。

若接入真实 Rust BR 主链，优先让消费 crate 的 `Cargo.toml` 依赖 `astersql-br-pkg-utils-storewatch`，实现从真实 PD 客户端到 `StoreMeta` 的适配，并明确放置上下文取消、重试和 TiFlash 过滤。随后替换 `br/pkg/backup/stubs.rs::storewatch` 与 `br/pkg/restore/data/stubs.rs::StoreWatcher` 的局部替身；这属于跨 crate 接线工作，超出本文件说明任务，不能只改 `lib.rs` 就宣称完成。

修改 crate 根时还应注意：通配再导出会让新增公开符号自动进入根 API，可能发生名称冲突；移除 `cfg(test)` 模块会失去 Go 契约验证；收紧 `#![allow]` 前应先确认整个模块能通过对应 lint。性能风险主要位于 `Step` 每轮克隆 `Store`、构造 ID 集合和清理缓存，而不在本入口文件。

## 验证依据

- `br/pkg/utils/storewatch/lib.rs`：crate 级 lint、两个 `cfg(test)` 模块、公开 `watching` 模块和通配再导出。
- `br/pkg/utils/storewatch/Cargo.toml` 与根 `Cargo.toml`：包名、`[lib]` 入口、Go 包映射、library 类型和 workspace 成员身份；全仓 Cargo 搜索未发现其他 manifest 引用该包名或路径。
- `br/pkg/utils/storewatch/watching.rs`：公开符号、`Step -> GetAllTiKVStores -> updateStore -> retain` 流程、差分条件、错误前缀与缓存生命周期。
- `br/pkg/utils/storewatch/watching.go`：Go API、真实 `context`/重试/`SkipTiFlash` 路径以及回调与差分语义。
- `br/pkg/utils/storewatch/watching_test.rs`、`parity_test.rs` 和 `watching_test.go`：注册、掉线、重启、事件顺序、消失后重现与错误行为。
- `br/pkg/utils/storewatch/BUILD.bazel`、`br/pkg/backup/BUILD.bazel`、`br/pkg/restore/data/BUILD.bazel`：Go 库边界及 Go 上游依赖。
- `br/pkg/backup/store.rs`、`br/pkg/backup/stubs.rs`、`br/pkg/restore/data/data.rs`、`br/pkg/restore/data/stubs.rs`：Rust 应用路径当前使用局部替身而非独立 storewatch crate。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/utils/storewatch` 确认生产与测试文件集合；`node --file` 读取 `lib.rs`、`watching.rs` 与两个 Rust 测试；`explore` 核对 `GetId`/`GetState`、三个回调、`GetAllTiKVStores`、`Step`、`updateStore`、`retain` 的直接调用关系。

本任务是纯文档分析，没有运行 Cargo。结构验证需确认目标文件存在且恰好包含任务约定的十一个二级标题。
