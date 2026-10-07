# `br/pkg/mock/common.rs`

## 文件定位

[`common.rs`](common.rs) 属于 Cargo crate `astersql-br-pkg-mock`。该 crate 由 [`Cargo.toml`](Cargo.toml) 将 `lib.rs` 设为库入口，且 `[package.metadata.porting]` 明确对应 Go 包 `br/pkg/mock`；它没有外部 Cargo 依赖，依靠同 crate 的本地替身工作。模块入口 [`lib.rs`](lib.rs) 通过 `pub mod common` 装载本文件，再用 `pub use common::*` 将其符号平铺导出。

本文件是 Go MockGen 产物 [`common.go`](common.go) 的手写 Rust 移植，只提供 `ChunkFlushStatus.Flushed` 的期望录制与返回值注入。它是测试辅助模块，不执行刷盘，也不属于备份恢复或 Lightning 的生产执行链。当前 Rust 仓库中直接构造它的独立测试是 [`parity_test.rs`](parity_test.rs) 的 `go_rust_public_contract_matches`；未发现 Rust 生产代码构造 `MockChunkFlushStatus`。

## 核心职责

- `MockChunkFlushStatus` 充当可注入返回值的刷盘状态对象，并实现本 crate 的 `crate::stubs::ChunkFlushStatus`。
- `MockChunkFlushStatusMockRecorder` 通过 `EXPECT().Flushed()` 向共享 `Controller` 登记一次名为 `Flushed` 的预期调用。
- 实际 `Flushed()` 调用交给 `Controller::Call` 匹配期望，再由 `take_one::<bool>` 取出布尔返回值。
- `ISGOMOCK()` 保留 Go gomock 生成物的身份标记外形，但在 Rust 中只是无返回值、无副作用的方法。

这里的“mock”只复刻调用形状与基本返回传播。参数匹配、`AnyTimes`、`Times`、`After`、`DoAndReturn` 等完整 gomock 能力不在本文件内，也未由当前 `stubs::Controller` 实现。

## 主要符号

- `pub struct MockChunkFlushStatus { pub ctrl: Controller, pub recorder: MockChunkFlushStatusMockRecorder }`：被测对象。两个字段共同指向同一个控制器会话；字段公开性沿用此 crate 其他生成式 mock 的测试便利性。
- `pub struct MockChunkFlushStatusMockRecorder { ctrl: Controller }`：期望录制器。其控制器字段私有，调用者应从 `MockChunkFlushStatus::EXPECT` 获取并使用它。
- `pub fn NewMockChunkFlushStatus(ctrl: Controller) -> MockChunkFlushStatus`：构造入口。克隆 `Controller` 给 recorder，原值留给 mock，因此两者共享 `Controller` 内部的 `Arc<Mutex<ControllerInner>>`，而不是维护两份期望队列。
- `impl crate::stubs::ChunkFlushStatus for MockChunkFlushStatus`：使该 mock 可作为本地 `ChunkFlushStatus` trait 对象使用。trait 方法显式转发到固有方法 `MockChunkFlushStatus::Flushed(self)`，避免递归调用 trait 方法本身。
- `MockChunkFlushStatus::EXPECT(&self) -> &MockChunkFlushStatusMockRecorder`：返回长期隶属于 mock 的 recorder 引用，不创建新的录制会话。
- `MockChunkFlushStatus::ISGOMOCK(&self)`：gomock 形状标记；当前没有运行时判断逻辑。
- `MockChunkFlushStatus::Flushed(&self) -> bool`：调用 `Controller::Helper`，以方法名 `Flushed` 消费一条期望，再使用 `take_one::<bool>` 解析首个返回槽。
- `MockChunkFlushStatusMockRecorder::Flushed(&self) -> Call`：登记无参数调用，传入诊断字符串 `MockChunkFlushStatus.Flushed`，返回可继续调用 `Return1(true)` 等方法的 `Call`。

本文件没有模块级常量、枚举、条件编译项或内部测试模块。

## 执行流程

典型测试流程由 [`parity_test.rs`](parity_test.rs) 第 91–95 行给出：

1. 测试以 `Controller::new()` 创建空期望队列。
2. `NewMockChunkFlushStatus(ctrl.clone())` 将同一控制器共享给 mock 和 recorder。
3. `flush.EXPECT()` 返回 recorder；随后 `Flushed()` 调用 `RecordCallWithMethodType("Flushed", "MockChunkFlushStatus.Flushed", vec![])`，把期望加入控制器队列。
4. `Return1(true)` 把一个动态类型的 `bool` 写入该期望的返回值槽。
5. `flush.Flushed()` 调用 `Controller::Call("Flushed", vec![])`。控制器查找并移除第一条同名期望，然后移出其返回值。
6. `take_one::<bool>` 将首个动态返回值向下转换成 `bool`，测试得到 `true`。

当通过 `dyn crate::stubs::ChunkFlushStatus` 调用时，先进入 trait 实现，再转发到上述固有 `Flushed` 流程。每条当前控制器期望只消费一次；重复调用必须重复登记期望，因为本地控制器没有 Go gomock 的 `AnyTimes()`。

## 数据与状态

本文件自身不保存真实刷盘进度。唯一业务返回值来自测试预先写入的 `Call` 返回槽；因此 `true` 或 `false` 表示测试场景设定，而不是对磁盘、引擎或后台任务的观测。

可变状态位于 [`stubs.rs`](stubs.rs) 的控制器中：`ControllerInner.expected` 是 `VecDeque<Arc<ExpectedCall>>`，每个 `ExpectedCall` 保存方法名和受 `Mutex` 保护的动态返回值数组。`NewMockChunkFlushStatus` 克隆的是持有该状态的 `Controller` 句柄。record 阶段追加期望，call 阶段按方法名查找并移除期望，`Call::Return1` 则覆盖对应期望的返回槽。

`Flushed` 没有参数，故录制与调用都传入空参数数组。控制器目前也会忽略其他 mock 方法传入的参数；本文件不能表达基于参数的分支。

## 依赖与调用关系

直接下游依赖只有 `crate::stubs::{Call, Controller, take_one}`：

- `Controller` 提供 `Helper`、`Call` 和 `RecordCallWithMethodType`。
- `Call` 提供链式返回值设置；本文件的正常用法是 `Return1(bool)`。
- `take_one<bool>` 负责动态返回值解包及默认回退。
- `crate::stubs::ChunkFlushStatus` 是本文件实际实现的 trait，定义为 `ChunkFlushStatus: Send`。

直接上游 Rust 证据是 [`lib.rs`](lib.rs) 的模块装载/再导出，以及 [`parity_test.rs`](parity_test.rs) 对构造器、recorder 和调用返回值的覆盖。RustCodeGraph 对文件给出了符号与模块使用信息，但对精确 `NewMockChunkFlushStatus`、`MockChunkFlushStatus::Flushed` 执行 `callers`/`callees` 未返回可用调用边；因此上述直接使用关系又以仓库局部引用搜索核验。

生产语义来源于 Go：[`pkg/lightning/common/util.go`](../../../pkg/lightning/common/util.go) 定义真实 `ChunkFlushStatus` 接口；[`lightning/pkg/importer/table_import.go`](../../../lightning/pkg/importer/table_import.go) 在引擎 writer 关闭后持有 data/index 两个状态，并在多个恢复/收尾分支调用 `Flushed()` 判断两者是否都完成。Go 测试 [`lightning/pkg/importer/table_import_test.go`](../../../lightning/pkg/importer/table_import_test.go) 两处构造本 mock，并把它作为 `MockEngineWriter.Close` 的返回值。

需要注意 trait 边界：Rust 本文件实现的是 `br/pkg/mock/stubs.rs` 的本地替身 trait，不是 [`pkg/lightning/common/util.rs`](../../../pkg/lightning/common/util.rs) 中的同名 trait。当前证据不能证明该 mock 已接入 Rust Lightning 生产 importer；它只证明 mock crate 的局部测试契约可用。

## 错误处理与边界

本 API 不返回 `Result`，失败边界由本地控制器和宽松解包规则决定：

- 未先登记 `Flushed` 期望时，`Controller::Call` 以 `panic!("Unexpected call to Flushed")` 失败。
- 控制器或返回槽的互斥锁若已中毒，`.expect("gomock lock")` / `.expect("rets lock")` 会 panic。
- 已登记期望但没有设置返回值时，`take_one::<bool>` 因空数组返回 `bool::default()`，即 `false`。
- 返回槽首项不是 `bool` 时，向下转换失败，同样静默回退为 `false`。
- 多余返回值不会被检查；首项解析后其余值随局部数组丢弃。
- 同名期望按当前队列中首个匹配项消费，但不同方法之间不强制全局录制顺序。

这与 Go MockGen 代码有细节差异：Go 的 `ret[0].(bool)` 使用“逗号 ok”并在类型不匹配时得到 `false`，但缺少返回槽时访问 `ret[0]` 会越界；Rust 对缺槽也回退 `false`。此外，Go gomock 会在测试结束时验证未满足期望，而 Rust 侧需要测试显式检查 `Controller::remaining()` 才能发现剩余期望。

## 并发与资源生命周期

`Controller` 通过 `Arc<Mutex<...>>` 共享期望队列，单个期望的返回槽也由 `Mutex` 保护；因此 recorder 写入和 mock 消费通过互斥锁串行化。`crate::stubs::ChunkFlushStatus: Send` 使 `MockChunkFlushStatus` 必须可跨线程移动，但 trait 没有要求 `Sync`，本文件也不承诺任意并发调用语义。

一个 `MockChunkFlushStatus` 的 recorder 与执行端从构造起共享控制器，直到各自的 `Controller` 克隆和相关 `Call` 句柄被丢弃。`Call` 用 `Arc` 保持对应期望存活，因此录制后可在调用前链式填入返回值。调用成功后，控制器从队列移除该期望并移走返回数组；本文件没有后台任务、通道、文件句柄、事务或显式清理函数。

并发测试若同时登记或消费多个同名 `Flushed` 期望，锁能避免内存数据竞争，但哪个线程取得哪一条同名期望取决于获得控制器锁的先后；不要用它验证确定性的线程调度顺序。

## 与 Go 版本的对应关系

Rust 的两个结构体、构造器、`EXPECT`、`ISGOMOCK`、mock 方法和 recorder 方法逐项对应 [`common.go`](common.go) 的 MockGen 输出。两端正常路径都是“共享控制器 → 录制 `Flushed` → 设置布尔返回 → 调用时由控制器分发”。方法名保留 Go 风格大小写，以便迁移代码与 parity 检查直接对照。

主要差异如下：

- Go 构造器接收 `*gomock.Controller` 并返回指针；Rust 按值接收/返回轻量句柄，内部通过 `Arc` 共享。
- Go recorder 保存回指 mock 的指针；Rust recorder 直接保存克隆后的 `Controller`，故 `RecordCallWithMethodType` 用字符串描述接收者/方法类型。
- Go `ISGOMOCK` 返回空结构体；Rust 方法返回单元值 `()`。
- Go `ChunkFlushStatus` 来自 `pkg/lightning/common`；Rust mock 实现本 crate 的精简替身 trait，且额外要求 `Send`。canonical Rust 文件 [`pkg/lightning/common/util.rs`](../../../pkg/lightning/common/util.rs) 另有不带 `Send` 的同名 trait，两者不是同一类型。
- Go 测试使用 `.Return(true).AnyTimes()`；当前 Rust `Call` 只有 `Return1(true)`，每项期望只匹配一次。Rust parity 测试目前只调用一次，因此没有覆盖 `AnyTimes` 语义。
- Go 的完整 gomock 负责生命周期校验、匹配器和调用次数；Rust 本地控制器只按方法名匹配并需显式检查 `remaining()`。

## 扩展指南

若 `ChunkFlushStatus` 新增查询方法，应同步修改本地 trait `stubs::ChunkFlushStatus`、`MockChunkFlushStatus` 的 trait/固有方法、recorder 方法和 `lib.rs` 导出所需类型。每个方法应保持“`Helper` → `Call`/`RecordCallWithMethodType` → 类型化解包”的既有模式，并在独立测试文件 [`parity_test.rs`](parity_test.rs) 添加正常返回、缺失期望以及必要的类型/默认值边界验证；不要把测试写进 `common.rs`。

若目标是支持重复调用、参数匹配或结束时自动验证，应优先扩展 [`stubs.rs`](stubs.rs) 的通用 `Controller`/`Call`，而不是在本文件中特判 `Flushed`。此类变化会影响同 crate 的所有生成式 mock，需要检查 `backend.rs`、`encode.rs`、`importer.rs` 和 `task_register.rs` 的共享控制器用法及相关独立测试。

若要把本 mock 接到 Rust Lightning importer，必须先决定统一使用 canonical `pkg/lightning/common/util.rs::ChunkFlushStatus` 还是 mock crate 的本地替身；直接假定两个同名 trait 可互换会造成类型不兼容。还应同步验证 `EngineWriter::Close` 的 trait 对象类型、跨线程约束，以及 Go importer 中 data/index 状态必须同时 flushed 的行为。性能风险很低但非零：每次录制和调用都获取互斥锁并进行 `Any` 向下转换；该设计适用于测试，不应进入高频生产路径。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、7,032 个 Rust 文件；`files --filter br/pkg/mock` 确认本文件、Go 对照、crate 入口与独立测试均已索引。
- RustCodeGraph 源码节点：`br/pkg/mock/common.rs`（完整 80 行）、`br/pkg/mock/lib.rs`、`br/pkg/mock/stubs.rs`、`br/pkg/mock/parity_test.rs`、`br/pkg/mock/common.go`、`pkg/lightning/common/util.go`、`pkg/lightning/common/util.rs`、`lightning/pkg/importer/table_import_test.go`。
- RustCodeGraph 符号查询：`query MockChunkFlushStatus --kind struct`、`query NewMockChunkFlushStatus --kind function`、`query Flushed --kind method`。对精确构造器/方法执行的 `callers` 与 `callees` 没有产生可用边，故没有据此扩大调用关系结论。
- Cargo 边界：[`Cargo.toml`](Cargo.toml) 的 package 名为 `astersql-br-pkg-mock`，库入口为 `lib.rs`，porting 元数据指向 `br/pkg/mock`，依赖表为空。
- Rust 行为测试：[`parity_test.rs`](parity_test.rs) 的 `go_rust_public_contract_matches` 录制 `Flushed().Return1(true)` 并断言实际调用为真。
- Go 使用证据：[`lightning/pkg/importer/table_import_test.go`](../../../lightning/pkg/importer/table_import_test.go) 第 406–411、974–979 行以 `AnyTimes` mock 刷盘状态；[`lightning/pkg/importer/table_import.go`](../../../lightning/pkg/importer/table_import.go) 第 722–723、766、839–858、902 行保存并查询 data/index 刷盘状态。
- 仓库引用搜索只找到上述 Rust parity 测试直接使用 `NewMockChunkFlushStatus`；没有发现 Rust 生产调用者。该结论限定于当前工作树和本次局部搜索。
- 本任务是纯文档分析，按计划不运行 Cargo。交付前使用任务指定命令验证文档恰有 11 个固定二级章节，并人工复核源文件链接、事实边界和独立测试位置。
