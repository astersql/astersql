# `br/pkg/mock/mocklocal/local.rs`

## 文件定位

本文件是 `astersql-br-pkg-mock-mocklocal` crate 中的 MockGen 风格实现层，由 [`lib.rs`](./lib.rs) 以 `pub mod local` 装入并平铺导出。它逐组移植 Go 生成文件 [`local.go`](./local.go) 中的 `DiskUsage`、`TiKVModeSwitcher` 和 `StoreHelper` mock，供 Lightning ingest 控制面相关契约测试使用；它不是磁盘管理、TiKV 模式切换、TSO 或 key codec 的生产实现。

crate 边界由 [`Cargo.toml`](./Cargo.toml) 确定：库入口是 `lib.rs`，唯一直接依赖是父 crate `astersql-br-pkg-mock`。本地 [`stubs.rs`](./stubs.rs) 代替 `kvproto`、`grpcio`、Lightning backend 等较重依赖，因此该 crate 被刻意保持为可在 darwin/arm64 上独立验证的轻量测试组件。

## 核心职责

文件提供三组“mock 对象 + recorder”以及对应构造函数：

- `MockDiskUsage` 回放 `EngineFileSizes`，返回预置的 `Vec<EngineFileSize>`。
- `MockTiKVModeSwitcher` 回放 `ToImportMode` / `ToNormalMode`，记录并消费模式切换调用。
- `MockStoreHelper` 回放 `GetTS` / `GetTiKVCodec`，返回预置时间戳、可选错误和 codec 替身。

三组对象都遵循相同协议：构造时让 mock 与 recorder 共享同一个 `Controller`；测试先通过 `EXPECT()` 获取 recorder 并登记期望，再调用 mock 方法消费期望。真正的期望队列、返回值槽和意外调用 panic 位于父 crate 的 [`../stubs.rs`](../stubs.rs) 中，而本文件负责把各接口方法翻译成统一的 `Controller::RecordCallWithMethodType` / `Controller::Call` 操作。

## 主要符号

- `NewMockDiskUsage(Controller) -> MockDiskUsage`：克隆 controller 给 `MockDiskUsageMockRecorder`，原 controller 存入 mock。`MockDiskUsage::EXPECT` 只借用 recorder；`MockDiskUsage::EngineFileSizes` 调用 `Controller::Call("EngineFileSizes", [])`，再由 `take_one` 解包单返回值。recorder 的同名方法登记方法标识 `MockDiskUsage.EngineFileSizes`。
- `NewMockTiKVModeSwitcher(Controller) -> MockTiKVModeSwitcher`：建立模式切换 mock。回放侧的 `ToImportMode(Context, &[Range])` 与 `ToNormalMode(Context, &[Range])` 把 context 和克隆后的每个 range 装箱后传给 controller；两项 recorder 方法返回可继续 `.Return(...)` 的 `Call`。
- `NewMockStoreHelper(Controller) -> MockStoreHelper`：建立存储辅助 mock。`GetTS(Context)` 使用本地 `take_ts` 解出 `(physical, logical, Option<Error>)`；`GetTiKVCodec()` 使用 `take_one::<Codec>` 解出单返回值。对应 recorder 分别登记 `GetTS` 和 `GetTiKVCodec`。
- `MockDiskUsageMockRecorder`、`MockTiKVModeSwitcherMockRecorder`、`MockStoreHelperMockRecorder`：仅持有共享 controller，不保存独立期望状态；所有状态都集中在 controller 内部。

这些符号是公开 API，但保留了 Go 风格的 PascalCase / `EXPECT` 命名。`lib.rs` 在 crate 级显式允许相关 Rust lint，目的在于维持与 Go MockGen 表面的可辨识对应关系。

## 执行流程

典型调用按以下顺序发生：

1. 测试创建 `Controller::new()`，再把 cloneable controller 传入某个 `NewMock*` 构造函数。
2. 构造函数把同一个 controller 状态分别放入 mock 与 recorder；`Controller` 内部用 `Arc<Mutex<...>>` 共享期望队列。
3. `mock.EXPECT().Method(...)` 调用 recorder。recorder 先执行无操作的 `Helper()`，再用方法名和诊断用方法类型字符串登记一项 `ExpectedCall`，得到 `Call` 句柄。
4. 测试通过 `Call::Return`、`Return1` 等写入返回值槽。
5. 业务侧式样的 mock 方法再次调用 `Helper()`，把实参装箱，然后按方法名调用 `Controller::Call`。controller 找出并移除第一项同名期望，交出其返回值。
6. `EngineFileSizes` / `GetTiKVCodec` 用 `take_one` 解包；`GetTS` 用 `take_ts` 解包；无返回值的两个模式切换方法丢弃返回向量。
7. 测试用 `Controller::remaining() == 0` 确认所有期望均已消费。若没有同名期望，`Controller::Call` 立即以 `Unexpected call to <method>` panic。

## 数据与状态

每个 mock 有两个公开字段：`ctrl: Controller` 和 `recorder: ...MockRecorder`。每个 recorder 内部也保存一个 `Controller` clone；这些 clone 不是复制期望列表，而是共享父 crate controller 中的 `Arc<Mutex<ControllerInner>>`。

`EngineFileSize`、`Range`、`Codec` 来自本 crate 的 [`stubs.rs`](./stubs.rs)：它们只保留测试所需字段形状。`Range` 的 `start` / `end` 不执行 protobuf 解析或顺序校验，`Codec` 只有用于区分实例的 `id`，`EngineFileSize` 只承载 UUID、磁盘/内存大小与导入状态。

返回值以 `Vec<Box<dyn Any + Send>>` 动态保存。`take_one` 在槽位缺失或类型不符时返回类型的 `Default`；`take_ts` 对缺失或错误类型的 physical/logical 返回 `0`，对错误槽接受 `Option<Error>` 或裸 `Error`，其它类型回落为 `None`。因此“返回默认值”可能表示测试未正确配置期望，而不代表真实服务给出了该值。

## 依赖与调用关系

直接下游依赖如下：

- `astersql_br_pkg_mock::{Controller, Call, Context}`：期望登记、调用回放、上下文替身。
- `astersql_br_pkg_mock::stubs::take_one`：单返回值动态解包。
- `crate::stubs::{EngineFileSize, Range, Codec, take_ts}`：本地数据替身和 `GetTS` 三槽解包。
- `std::any::Any`：把 context、range 和返回值装入动态槽。

RustCodeGraph 将本文件识别为 23 个符号；对三个 `NewMock*` 的精确查询同时找到了 Go 与 Rust 对应构造函数。仓库引用搜索表明，Rust 侧直接调用者集中在 [`parity_test.rs`](./parity_test.rs)，`lib.rs` 则负责模块装配与再导出。未发现生产 Rust 路径实例化这些 mock。

概念上的被替代接口位于 `pkg/ingestor/ingestctrl`：`disk_quota.rs::DiskUsage`、`tikv_mode.rs::TiKVModeSwitcher` 和 `engine_mgr.rs::StoreHelper`。但本文件没有为三个 mock 编写这些 Rust trait 的 `impl`，并且部分轻量替身类型与 canonical trait 不同（例如本地 `Context` 对比 `CancellationToken`、本地 `Codec` 对比当前 `String` 返回）。因此它目前是独立的 Go/Rust mock 契约移植，不可直接作为 canonical ingestctrl trait object 注入生产链。

## 错误处理与边界

- 未登记的方法调用由 `Controller::Call` panic；[`parity_test.rs`](./parity_test.rs) 的 `unexpected_call_panics` 固定了该行为。
- `GetTS` 不返回 Rust `Result`，而是把 Go 的第三个 `error` 返回槽表示为 `Option<Error>`。具体错误和 `None` 都有独立覆盖。
- 空 `EngineFileSizes`、空 ranges 与多 ranges 均可回放；模式切换方法自身不验证 range 内容。
- recorder 的 `ToImportMode` / `ToNormalMode`、`GetTS` 当前接收参数占位，却向 `RecordCallWithMethodType` 传入空参数向量；父 controller 的 `Call` 也忽略实参。因此当前匹配只依赖方法名，不具备 Go MockGen/gomock 的参数匹配能力。
- controller 会查找并消费队列中第一项同名期望，而不是强制全局登记顺序；不同方法间的严格先后关系不能由这个轻量实现证明。
- `take_one` / `take_ts` 的宽松默认值策略避免类型转换 panic，但也可能掩盖返回槽配置错误。新增测试应同时断言具体返回值与 `remaining()`。

## 并发与资源生命周期

父 `Controller` 通过 `Arc<Mutex<ControllerInner>>` 保护期望队列，每个返回值列表也放在 `Mutex` 中，所以 controller clone 可跨共享状态登记和消费。`Context`、range 与返回值要求能装入 `Any + Send`；模式切换回放会克隆每个 `Range`，调用期间不借用调用方切片中的元素。

本文件不创建线程、异步任务、网络连接、文件句柄或后台 worker，也没有自定义 `Drop`。资源生命周期由普通 Rust 所有权决定：mock 与 recorder 释放各自的 controller clone；最后一个 clone 释放时共享队列随之销毁。当前实现不会在 drop 时自动断言期望耗尽，所以调用方必须显式检查 `Controller::remaining()`；`parity_test.rs` 也验证了期望耗尽后 drop 不 panic、不阻塞。

虽然 controller 内部有锁，文档不能据此推断本 mock 已满足 canonical traits 的 `Send + Sync` 注入要求；文件中没有相应 trait impl，跨线程业务语义也未由该 crate 的 parity test 覆盖。

## 与 Go 版本的对应关系

[`local.go`](./local.go) 是 `mockgen` 针对 `pkg/ingestor/ingestctrl` 三个接口生成的文件。本文件保留了三组类型、构造函数、`EXPECT`、方法名以及主要返回值形状：

- Go `[]backend.EngineFileSize` 对应 Rust `Vec<stubs::EngineFileSize>`。
- Go `context.Context` 与 `...*import_sstpb.Range` 对应轻量 `Context` 与 `&[stubs::Range]`。
- Go `(int64, int64, error)` 对应 `(i64, i64, Option<Error>)`。
- Go `tikv.Codec` 对应本地字段型 `stubs::Codec`。

差异必须保留在评审视野内：Go recorder 保存 mock 引用并通过反射记录真实 receiver、方法类型和实参；Rust recorder 直接保存 controller，方法类型只是诊断字符串，参数匹配被省略。Go 的类型断言失败通常得到零值，Rust 的 `take_one` / `take_ts` 也采取宽松回落，但 Rust 错误槽使用显式 `Option<Error>`。此外，canonical Rust ingestctrl traits 已采用自己的 `CancellationToken`、`KeyRange`、`Result` 或 `String` 类型，本 mock 尚未与它们接线。

Go 的真实使用证据位于 `pkg/ingestor/ingestctrl/disk_quota_test.go` 与 `engine_mgr_test.go`：前者用 `NewMockDiskUsage` 测磁盘配额，后者用 `NewMockStoreHelper` 测 engine manager。Rust 当前对应行为主要由本 crate 的 `parity_test.rs` 自包含验证，而 ingestctrl 自己的 Rust 测试使用各自的手写替身。

## 扩展指南

新增或修改 mock 方法时，应同步完成以下局部工作：

1. 先核对 `local.go` 与 `pkg/ingestor/ingestctrl` 中对应接口，明确是在保持 Go MockGen 契约，还是要接入 canonical Rust trait；两者类型模型当前并不相同。
2. 在 mock impl 增加回放方法，在 recorder impl 增加同名登记方法，并确保方法名字符串完全一致；多返回值应提供专用解包函数，避免错误复用 `take_one`。
3. 若新增替身类型，只在 `stubs.rs` 保留测试确实需要的字段，并明确它不具备生产类型的行为；不要让轻量 crate 重新引入其刻意避开的 `kvproto` / `grpcio` 重依赖。
4. 在独立测试文件 [`parity_test.rs`](./parity_test.rs) 增加正常值、零值/空集合、错误、意外调用和 `remaining()` 断言；不要把测试内嵌回 `local.rs`。
5. 若需要真实 gomock 参数语义，应同时改造父 crate `Controller::RecordCallWithMethodType` 和 `Controller::Call`，并评估所有使用该轻量 controller 的 mock；仅在本 recorder 填入参数不足以生效。
6. 若目标是让这些类型实现 canonical `DiskUsage`、`TiKVModeSwitcher` 或 `StoreHelper`，需要先解决类型差异并在 ingestctrl 的独立测试中验证注入链。这属于跨 crate 接线，不能通过把本地 stand-in 强制包装成生产类型来简化。

主要兼容风险是偏离 Go 生成 API 或进一步扩大“只按方法名匹配”的语义差距；主要正确性风险是宽松解包把错误配置变成默认值；当前数据量很小，性能风险主要来自每次调用的动态装箱、range 克隆和 mutex，但该文件只用于测试，不应进入性能敏感的生产路径。

## 验证依据

- RustCodeGraph `status`：索引包含 7,032 个 Rust 文件；`files --filter br/pkg/mock/mocklocal` 返回 `lib.rs`、`local.rs`、`stubs.rs`、`parity_test.rs` 和 Go 对照 `local.go`。
- RustCodeGraph `node --file br/pkg/mock/mocklocal/local.rs --offset 1 --limit 400`：读取本文件 244 行全貌，确认三组 mock/recorder、三个构造函数和全部方法实现。
- RustCodeGraph 对 `NewMockDiskUsage`、`NewMockTiKVModeSwitcher`、`NewMockStoreHelper` 的 function query：每项均同时定位到 `local.go` 与 `local.rs` 对应符号。
- RustCodeGraph file node：核对 [`local.go`](./local.go)、[`lib.rs`](./lib.rs)、[`stubs.rs`](./stubs.rs)、[`parity_test.rs`](./parity_test.rs) 以及父 controller 的 [`../stubs.rs`](../stubs.rs)。
- Cargo 证据：[`Cargo.toml`](./Cargo.toml) 声明 `go-package = "br/pkg/mock/mocklocal"`、`kind = "library"`，库入口为 `lib.rs`，直接依赖仅 `astersql-br-pkg-mock`。
- 调用与测试搜索：Rust 直接构造调用只在 `br/pkg/mock/mocklocal/parity_test.rs`；Go 使用位于 `pkg/ingestor/ingestctrl/disk_quota_test.go`、`engine_mgr_test.go`；canonical Rust 接口定义位于 `disk_quota.rs`、`tikv_mode.rs`、`engine_mgr.rs`。
- 独立 Rust 测试覆盖：预置磁盘大小、空列表、空/多 range、TSO 的 `None` 与具体错误、codec 返回、期望耗尽、drop，以及未登记调用 panic。本任务为纯文档分析，按任务约束未运行 Cargo。
