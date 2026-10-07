# `br/pkg/mock/mocklocal/lib.rs`

## 文件定位

本文件是独立 crate `astersql-br-pkg-mock-mocklocal` 的根入口；crate 边界由同目录 [`Cargo.toml`](Cargo.toml) 的 `[lib] path = "lib.rs"` 确定，根工作区 [`Cargo.toml`](../../../../Cargo.toml) 将 `br/pkg/mock/mocklocal` 列为成员。Cargo 元数据把它对应到 Go 包 `br/pkg/mock/mocklocal`，并将类型标为 library。

它位于 BR 的测试替身层，不是备份恢复或 Lightning ingest 的生产实现。入口通过 `#[path]` 装配 [`stubs.rs`](stubs.rs) 和 [`local.rs`](local.rs)，再将两个模块的公开符号平铺导出；仅在测试构建中加载独立的 [`parity_test.rs`](parity_test.rs)。同目录没有 `doc.go`，Go 包身份与生成来源以 [`local.go`](local.go) 的包声明及 MockGen 头部为准。

## 核心职责

`lib.rs` 只承担 crate 级装配、可见性和兼容策略，不实现任何 mock 方法：

1. 先公开加载 `stubs`，提供 `EngineFileSize`、`Range`、`Codec` 和 `take_ts` 等轻量替身。
2. 再公开加载 `local`，提供 `MockDiskUsage`、`MockTiKVModeSwitcher`、`MockStoreHelper` 及各自 recorder。
3. 通过 `pub use local::*` 与 `pub use stubs::*` 形成平铺 API，使调用方既可使用 `crate::local::NewMockStoreHelper`，也可从 crate 根取得相同公开符号。
4. 用 crate 级 `allow` 保留 Go MockGen 的命名和机械移植形状，避免 `NewMock...`、`EXPECT`、`GetTS` 等符号被 Rust lint 要求改名。
5. 只在 `#[cfg(test)]` 下挂接契约对等测试，保证测试逻辑与生产候选源文件分离。

因此，这个文件存在的价值是定义一个可独立依赖的 mocklocal Rust crate 门面；真实磁盘统计、TiKV 模式切换、TSO、Codec、RPC 与 SST 导入均不在这里执行。

## 主要符号

- `pub mod stubs`：借助 `#[path = "stubs.rs"]` 绑定本地桩模块。主要公开数据为 `EngineFileSize`、`Range`、`Codec`，以及供 `MockStoreHelper::GetTS` 解包动态返回槽的 `take_ts`。
- `pub mod local`：借助 `#[path = "local.rs"]` 绑定 Go MockGen 移植模块。该模块公开三组 mock、三组 recorder 和三个 `NewMock...` 构造器。
- `pub use local::*` / `pub use stubs::*`：将上述公开项重新导出到 crate 根。这里没有选择性白名单，因此今后子模块新增的任何 `pub` 项也会自动进入根 API。
- `mod parity_test`：仅在 `cfg(test)` 生效的私有测试模块，路径显式指向 `parity_test.rs`；它不会进入普通依赖构建的公共 API。
- crate 级 `#![allow(...)]`：允许 `dead_code`、Go 风格大小写、未使用导入/变量以及全部 Clippy lint。这是兼容生成式移植的策略，也意味着 lint 不能替代契约测试。

本文件没有常量、struct、enum、trait、函数或 `impl`；RustCodeGraph 对它只识别出一个文件级符号，与其纯装配职责一致。

## 执行流程

普通编译时，编译器从本文件开始构建 crate：

1. 应用 crate 级 lint 放宽规则。
2. 解析 `stubs.rs` 为公开的 `stubs` 模块。
3. 解析 `local.rs` 为公开的 `local` 模块；其中通过 `crate::stubs::{Codec, EngineFileSize, Range, take_ts}` 使用前一步类型，并依赖父 crate `astersql-br-pkg-mock` 的 `Controller`、`Call`、`Context` 与动态返回值工具。
4. 将两个模块的全部公开项重导出到 crate 根。
5. 若启用测试构建，再解析 `parity_test.rs`。测试创建 `Controller`，通过 `EXPECT()` 登记调用和返回值，调用 mock 方法消费期望，最后以返回值、panic 或 `Controller::remaining()` 验证契约。

典型回放链是 `NewMockStoreHelper` → `EXPECT().GetTS(...).Return(...)` → `GetTS(Context)` → `Controller::Call("GetTS", ...)` → `take_ts`。`lib.rs` 不参与运行期分派；它只保证这条链的符号在同一 crate 中可见。

## 数据与状态

本文件没有运行期数据、全局变量或可变状态。所有状态都在导出模块中：

- `local.rs` 的每个 mock 持有一个 `Controller` 和对应 recorder；构造器 clone 控制器，使录制与回放共享期望队列。
- 父 crate [`../stubs.rs`](../stubs.rs) 的 `Controller` 内部使用 `Arc<Mutex<ControllerInner>>`，`ControllerInner.expected` 是待消费期望的 `VecDeque`；每条期望的动态返回槽也由 `Mutex` 保护。
- 本 crate 的 `stubs.rs` 用拥有所有权的简化值替代 Go 生产类型：`EngineFileSize` 保存 16 字节 UUID、磁盘/内存大小及导入标志；`Range` 保存 start/end 字节串；`Codec` 只保存 id。
- 动态参数和返回值通过 `Box<dyn Any + Send>` 流转。`take_ts` 按 physical、logical、error 三槽取值，缺槽或类型不符会分别回落到 `0`、`0`、`None`。

这些简化类型只足以承载 mock 契约，不能代表 Lightning backend、kvproto Range 或 TiKV Codec 的完整结构与不变量。

## 依赖与调用关系

Cargo 直接依赖只有相邻父 crate `astersql-br-pkg-mock = { path = ".." }`。注释明确该 crate 为 darwin/arm64 精简依赖面，不拉入 `kv`、`domain`、`kvproto`、`grpcio` 或 Lightning；真实外部类型由本地 stubs 替代。Go 的 [`BUILD.bazel`](BUILD.bazel) 则依赖 Lightning backend、kvproto import_sstpb、TiKV client 和 GoMock，这体现了两侧依赖边界差异。

下游关系为：`lib.rs` 加载 `local.rs` 与 `stubs.rs`；`local.rs` 调用父 crate 的 `Controller::{Helper,Call,RecordCallWithMethodType}`、`take_one`，并调用本 crate `take_ts`。测试构建时 `parity_test.rs` 通过 `crate::local` 和 `crate::stubs` 直接导入符号。

RustCodeGraph 的文件查询确认同目录包含 `lib.rs`、`local.rs`、`stubs.rs`、`parity_test.rs` 与 Go 对照 `local.go`；对精确构造器的查询能定位 Go/Rust 两份定义，但索引未给出可靠的跨文件调用边。仓库文本检索进一步确认，当前 Rust 直接使用者仅为本 crate 的 `parity_test.rs`，没有找到其他 crate 对 `astersql_br_pkg_mock_mocklocal`、`mocklocal::` 或三个 Rust 构造器的引用。因此不能把 Go 调用面当成已经接线的 Rust 应用主链。

Go 上游使用更广：`pkg/ingestor/ingestctrl/disk_quota_test.go` 用 `MockDiskUsage` 驱动磁盘配额排序；`pkg/ingestor/ingestctrl/engine_mgr_test.go` 用 `MockStoreHelper` 提供 TSO；`tests/realtikvtest/importintotest/import_into_test.go::TestImportMode` 用 `MockTiKVModeSwitcher` 观测导入模式先于正常模式。这些文件说明生成 mock 的业务意图，但不是 Rust 已接线的证据。

## 错误处理与边界

`lib.rs` 自身没有 `Result`、panic 分支或错误转换；边界行为来自它导出的实现：

- 调用未登记的方法时，父 `Controller::Call` 以 `Unexpected call to {method}` panic；`parity_test.rs::unexpected_call_panics` 明确覆盖该行为。
- `EngineFileSizes` / `GetTiKVCodec` 使用 `take_one`，缺失或类型错误的返回槽回落为类型默认值；`GetTS` 的错误槽可接受 `Option<Error>` 或裸 `Error`，其他类型回落为无错误。
- `ToImportMode` 与 `ToNormalMode` 把上下文和 Range 装箱传入回放侧，但 recorder 当前登记空参数表，父控制器也忽略参数。因此目前只能断言方法名和消费次数，不能验证具体 context、范围内容或参数 matcher。
- 父控制器不支持 GoMock 的 `Times`、`After`、`DoAndReturn` 等高级能力。Go RealTiKV 测试中的回调计时行为不能由当前 Rust mock 复现。
- `pub use ...::*` 会自动扩大根 API；新增公开 helper 时需审查是否应该成为稳定公共契约。

普通构建不会包含 `parity_test`，所以不能依赖测试模块为生产路径提供任何符号或初始化副作用。

## 并发与资源生命周期

入口本身不创建线程、任务、通道、锁、事务、文件或网络资源。并发语义来自父 `Controller`：期望队列和返回槽由 `Mutex` 保护，控制器由 `Arc` 共享，动态值要求 `Send`。这允许句柄跨线程持有，但只证明 mock 控制器的数据竞争受到保护，不证明被模拟的 TiKV/Lightning 行为可并发执行。

每个期望通常登记一次、匹配一次、从队列移除一次；返回向量在消费时通过 `std::mem::take` 移走。`remaining() == 0` 是当前 Rust 测试的显式资源耗尽检查。mock drop 不执行 GoMock `Finish`、真实模式恢复、引擎清理或 RPC 关闭；`parity_test.rs` 的 drop 检查只证明对象析构不挂起或 panic。

不同 mock 可以共享一个控制器，但匹配键仅为方法名。组合测试若出现同名方法，存在跨 mock 消费错误期望的风险；优先使用独立控制器，或严格检查剩余期望。这里也没有真实 TiKV 模式切换的补偿机制，不能用 mock 析构替代生产资源清理。

## 与 Go 版本的对应关系

Go [`local.go`](local.go) 是从 `pkg/ingestor/ingestctrl` 的 `DiskUsage`、`TiKVModeSwitcher`、`StoreHelper` 三个接口生成的 MockGen 文件。Rust `local.rs` 保留三组 mock/recorder、三个构造器、`EXPECT` 以及 `EngineFileSizes`、`ToImportMode`、`ToNormalMode`、`GetTS`、`GetTiKVCodec` 方法；本 `lib.rs` 再把它们组成独立 crate 并平铺导出。

关键差异如下：

- Go mock 使用真实 `backend.EngineFileSize`、`import_sstpb.Range`、`tikv.Codec` 与 `*gomock.Controller`；Rust 使用本地值类型和轻量 `Controller`。
- Go 构造器返回指针，recorder 持有 mock 指针；Rust mock 按值返回，mock 与 recorder 各持有同一控制器的 clone。
- Go recorder 把 matcher/参数和反射方法类型传给 GoMock；Rust recorder 的参数仅保留 API 外形，登记时传空参数，方法类型是描述字符串。
- GoMock 支持重复次数、顺序、回调与完成时校验；Rust 当前是一次性期望队列，以 `remaining()` 手工检查耗尽。
- Go `GetTS` 返回 `(int64, int64, error)`；Rust 返回 `(i64, i64, Option<Error>)`，并对缺失/类型错误返回槽采取宽松默认值。
- Go 侧 mock 被 ingestctrl 与 RealTiKV 测试直接注入真实接口；当前 Rust 文本与图证据只证明 crate 自身 parity 测试使用，尚未证明它实现或被注入对应的 Rust 业务 trait。

所以该 crate 是对当前测试需要的 MockGen 契约移植，不是 Go mock 框架或生产接口的完整等价实现。

## 扩展指南

扩展此 crate 时，应按职责所在位置修改，而不是把逻辑堆入入口文件：

1. 新增被模拟方法时，在 `local.rs` 同步增加 mock 方法与 recorder 方法，保持方法字符串、参数装箱顺序和返回槽顺序一致；同时对照 `local.go` 或重新生成后的 Go API。
2. 新增必要数据外形时放入 `stubs.rs`，明确它是轻量替身还是完整生产类型；不要为方便测试把 kvproto/grpcio 的真实行为伪装进 stub。
3. 只有需要改变模块或公开 API 时才修改 `lib.rs`。若新增模块，先决定是否应 `pub mod`，以及是否真的适合用 glob 重导出。
4. 测试继续放在独立的 `parity_test.rs` 或最近调用方的独立 `*_test.rs`，不要把 Rust 单元测试嵌进 `lib.rs`。至少覆盖正常返回、空集合、显式错误、未登记调用 panic、非空 Range 与期望耗尽。
5. 如果业务代码开始依赖该 crate，应新增最近调用方的 Rust 测试，验证 trait 注入与业务分支；仅有 crate 内 parity 测试不能证明应用接线。
6. 若需要 matcher、`Times`、`DoAndReturn` 或自动 Finish，应在父 crate `br/pkg/mock/stubs.rs::Controller` 统一设计并回归所有 mock，不要在 mocklocal 单独做不兼容实现。

主要正确性风险是方法名或动态返回槽顺序漂移、宽松默认值掩盖错误配置；兼容风险是 glob 重导出意外扩大 API，以及 Rust 桩类型与 Go 生产类型不等价；性能风险很低，但 Range 会逐项 clone 并装箱，扩展到大批量数据时需重新评估。真实 I/O、重试和模式恢复逻辑应留在生产实现及集成测试中。

## 验证依据

- 目标入口：RustCodeGraph `node --file br/pkg/mock/mocklocal/lib.rs` 读取完整 31 行，核对两个 `#[path]` 公开模块、两个 glob 重导出、crate lint 规则和唯一的 `cfg(test)` 模块。
- 实现与类型：RustCodeGraph 分别读取 `local.rs`、`stubs.rs`，核对三组 mock/recorder、五个被模拟方法、`take_ts` 的三槽规则和轻量数据结构。
- 控制器：RustCodeGraph 读取 `br/pkg/mock/stubs.rs` 的 `Controller::{new,Helper,Call,RecordCallWithMethodType,remaining}`、`Call::{Return,Return1}` 与 `take_one`，确认方法名匹配、一次性消费、参数忽略、锁和默认值行为。
- crate 与构建边界：读取本目录 `Cargo.toml`、`BUILD.bazel` 及根 `Cargo.toml`，确认 Rust workspace 成员、唯一 Rust 依赖、Go Bazel 的真实依赖集合和两侧依赖差异。
- Go 对照：读取 `local.go` 完整 MockGen 输出；读取 `pkg/ingestor/ingestctrl/disk_quota_test.go`、`engine_mgr_test.go` 与 `tests/realtikvtest/importintotest/import_into_test.go::TestImportMode`，核对三组 mock 的真实测试意图、参数和 GoMock 高级能力。
- Rust 测试：RustCodeGraph 读取独立 `parity_test.rs`，覆盖正常/空返回、TSO 错误、Codec、单个与多个 Range、期望耗尽、析构和未登记调用 panic。同目录及父目录不存在 `doc.go`。
- 调用面：RustCodeGraph `status` 显示索引覆盖本仓库 7032 个 Rust 文件；`files --filter br/pkg/mock/mocklocal` 确认文件集合；精确 `query NewMockDiskUsage` 定位 Go/Rust 定义。由于精确 callers/callees 未产生可靠输出，最终用 `rg` 复核 Rust 构造器和 crate 名引用，并明确区分 Go 调用面与 Rust 当前接线状态。
- 本任务为纯文档分析，按计划不运行 Cargo；交付结构验证要求本文恰有 11 个规定的二级标题。
