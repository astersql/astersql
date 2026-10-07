# `br/pkg/mock/task_register.rs`

## 文件定位

该文件属于 Cargo 包 `astersql-br-pkg-mock`（见 `br/pkg/mock/Cargo.toml`），由 `br/pkg/mock/lib.rs` 以 `pub mod task_register` 装入，并通过 `pub use task_register::*` 将公开符号平铺到 crate 根。它是 Go 生成文件 `br/pkg/mock/task_register.go` 的 Rust 轻量移植，服务于隔离测试；它不连接 etcd，也不执行真实任务注册、续租或撤销。

虽然文件位于生产候选目录并参与库编译，它的角色仍是测试替身。真实注册协议定义在 `br/pkg/utils/register.rs` 的 `TaskRegister` trait 中，实现由 `TaskRegisterImpl` 承担；本文件的 `MockTaskRegister` 没有实现该 trait，且方法签名也不是 trait 的直接实现形状，因此不能把它视为生产注册器或可透明注入的 trait object。

## 核心职责

文件只承担三项职责：用 `MockTaskRegister` 暴露 `Close`、`RegisterTask`、`RegisterTaskOnce` 三个被测调用入口；用 `MockTaskRegisterMockRecorder` 记录同名期望；用 `NewMockTaskRegister` 保证调用对象和 recorder 共享同一个 `Controller` 状态。调用结果不在本文件计算，而由测试通过 `Call::ReturnError` 等接口预先写入共享期望，再由 `Controller::Call` 取出。

这套实现保留 Go mockgen 的“先 `EXPECT`、后调用”使用形状，但只覆盖仓库本地 `stubs.rs` 实现的最小契约。它不提供真实任务 key、TTL、lease、keepalive、取消传播或撤销行为。

## 主要符号

- `MockTaskRegister { pub ctrl, pub recorder }`：被调用侧替身。`ctrl` 与 recorder 内部的控制器共享同一 `Arc<Mutex<_>>` 状态；两个字段虽公开，通常应通过方法和 `EXPECT` 使用。
- `MockTaskRegisterMockRecorder { ctrl }`：期望记录器。字段私有，调用方由 `MockTaskRegister::EXPECT` 借用它。
- `NewMockTaskRegister(Controller) -> MockTaskRegister`：构造函数。克隆控制器给 recorder，并把原控制器放入 mock；克隆不会复制期望队列。
- `MockTaskRegister::EXPECT(&self)`：返回 recorder 引用，用于链式调用 `Close`、`RegisterTask` 或 `RegisterTaskOnce` 并配置返回值。
- `MockTaskRegister::ISGOMOCK(&self)`：无返回值、无副作用的识别标记，对应 Go 生成物的同名标记方法。
- `MockTaskRegister::{Close, RegisterTask, RegisterTaskOnce}`：均接收一个拥有所有权的 `stubs::Context`，先调用无操作的 `Controller::Helper`，再以固定方法名调用 `Controller::Call`，最后用 `take_error` 转成 `Result<()>`。
- `MockTaskRegisterMockRecorder::{Close, RegisterTask, RegisterTaskOnce}`：均返回可链式配置的 `stubs::Call`。签名接收 `&dyn Any`，但当前实现没有把 `_arg0` 传给控制器；记录时参数向量为空。

文件没有模块级常量、trait、枚举、条件编译项或异步函数。

## 执行流程

典型成功或注入错误流程如下：

1. 测试以 `Controller::new()` 创建空期望队列，并调用 `NewMockTaskRegister`。mock 与 recorder 因控制器克隆而指向同一队列。
2. 测试调用 `reg.EXPECT().RegisterTask(&matcher)`。recorder 调用 `Helper`，再以 `"RegisterTask"` 和描述字符串 `"MockTaskRegister.RegisterTask"` 记录期望，获得 `Call`。
3. 测试用 `ReturnError(None)` 配置成功，或用 `ReturnError(Some(Error))` 配置失败。返回值写入期望条目的互斥锁。
4. 被测路径调用 `reg.RegisterTask(Context)`。该方法把 context 装箱后交给 `Controller::Call`；控制器实际只按方法名寻找并移除第一个同名期望，当前不比较参数。
5. `take_error` 取返回槽：`Some(Error)` 变为 `Err`，`None` 变为 `Ok(())`。同样流程适用于 `Close` 与 `RegisterTaskOnce`。

`br/pkg/mock/parity_test.rs::gomock_expectations_match_out_of_registration_order` 还证明：不同方法的期望无需按登记顺序调用；控制器会在队列中搜索首个同名项。该行为用于贴近未显式使用 `gomock.InOrder` 时的 Go 行为。

## 数据与状态

本文件自身只保存两个控制器句柄，不保存任务名称、注册类型、lease ID 或生命周期状态。实际可变状态位于 `br/pkg/mock/stubs.rs`：`Controller` 内部是 `Arc<Mutex<ControllerInner>>`，`ControllerInner.expected` 是待消费期望的 `VecDeque`；每个 `ExpectedCall` 又用 `Mutex` 保存类型擦除的返回值列表。

每次成功的业务方法调用会从期望队列移除一个同名条目，因此期望是一次性消费的。`Controller::remaining()` 可在测试末尾检查是否仍有未消费期望；`MockTaskRegister` 本身没有析构检查，也不会自动断言 remaining 为零。

传入的 `Context` 只被装箱；`cancelled` 字段不会在该调用链中读取。recorder 接收的 matcher/参数也被忽略。因此当前不变量是“方法名和预置返回值驱动行为”，而不是“方法名加参数匹配驱动行为”。

## 依赖与调用关系

直接依赖全部来自 `crate::stubs`：`Controller` 负责记录与消费调用，`Call` 提供返回值配置，`Context` 和 `Result` 提供签名形状，`take_error` 解码错误槽。`br/pkg/mock/Cargo.toml` 的 `[dependencies]` 为空，说明这些能力完全由同 crate 的本地桩提供，没有引入 `gomock` 或真实注册客户端。

上游装配入口是 `br/pkg/mock/lib.rs`，它加载并再导出本文件。当前仓库中直接的 Rust 使用证据在 `br/pkg/mock/parity_test.rs`：`go_rust_public_contract_matches` 构造 mock、为 `RegisterTask` 注入错误并断言错误文本；`gomock_expectations_match_out_of_registration_order` 为 `Close` 和 `RegisterTask` 登记成功返回，并验证逆登记顺序调用及 `remaining() == 0`。

RustCodeGraph 对该文件识别出 12 个符号，并显示 `RegisterTask` 的 mock 方法到 recorder 同名方法的文件级关联；但对 `NewMockTaskRegister` 未解析出被调用者，且文件节点报告 `used by 0 files`。因此本说明以上述 `lib.rs` 模块装配和 `parity_test.rs` 的文本引用作为直接补充证据，不把图索引缺边解释成没有调用方。

真实应用侧的注册主链位于 `br/pkg/utils/register.rs`，Lightning 的实际构造调用可见于 `lightning/pkg/importer/import.rs`；它们使用真实 `TaskRegister` 实现，不经过本 mock。Go 侧集成使用示例位于 `tests/realtikvtest/importintotest/import_into_test.go::TestRegisterTask`，该测试将 Go mock 注入 Import Into 构造钩子；当前 Rust realtikv 测试使用自己的 `task_register` harness，而不是本文件的类型。

## 错误处理与边界

三个业务方法都把 `Controller::Call` 返回的首槽交给 `take_error`。首槽为 `Option<Error>` 时，`Some` 原样成为失败、`None` 成功；首槽为裸 `Error` 时也返回失败。空返回列表或未知首槽类型会被 `take_error` 当作成功，这是本地轻量桩的宽松边界，不等同于 Go 的完整运行时类型检查。

若没有同名期望，`Controller::Call` 以 `Unexpected call to <method>` panic；若互斥锁中毒，相关 `expect` 也会 panic。文件不捕获这些 panic。由于参数和 `_method_type` 当前均被控制器忽略，错误 context、不同 matcher 或不同方法签名描述不会阻止匹配；扩展测试时不能依赖参数校验发现误调用。

该 mock 不读取 context 的取消状态，也不模拟真实实现中的 lease-not-found、grant/re-put、keepalive 或 revoke 错误。需要验证这些边界时，应使用 `br/pkg/utils/register_test.rs` 的真实注册状态机测试，而不是扩张本文件的结论。

## 并发与资源生命周期

`Controller` 的期望队列和每条期望的返回值都由 `Arc<Mutex<_>>` 保护，克隆控制器后共享状态；因此构造器不会形成两套独立期望。一次调用在持有队列锁时查找并移除期望，返回值随后从独立锁中 `take`，同一条期望不会被两个调用重复消费。

本文件不创建线程、任务、通道、网络连接、lease 或后台 keepalive，也没有 `Drop` 实现。`Close` 只是一个可配置的 mock 调用，不会自动清理控制器或真实资源。并发测试仍需自行安排足够的同名期望；这里没有调用次数上限、顺序约束或自动等待机制，且锁中毒会直接 panic。

## 与 Go 版本的对应关系

Rust 的两个结构体、构造函数、`EXPECT`、`ISGOMOCK` 以及三组“业务方法/recorder 方法”逐项对应 `br/pkg/mock/task_register.go` 的 mockgen 输出。两端业务方法名称和单个 context 参数保持一致，错误返回也通过预置值传播。

关键差异必须保留在使用预期中：Go recorder 保存 `*MockTaskRegister`，Rust recorder 直接保存共享 `Controller`；Go 将 receiver、反射方法类型和实参传给 `RecordCallWithMethodType`，Rust 只记录方法名，描述字符串和参数目前不参与匹配；Go `ISGOMOCK` 返回 `struct{}`，Rust 返回单元；Go controller 提供 matcher、`Times`、`DoAndReturn` 等完整能力，本地 `stubs.rs` 明确不支持这些高级能力。

此外，真实 Rust `TaskRegister` trait 的方法使用 `&mut self`，并在 `Close`/`RegisterTaskOnce` 中借用 `&Context`、返回 `SharedError`；本 mock 使用 `&self`、拥有的 `Context` 和本地 `Error`。因此它是 API 形状测试替身，不是对真实 Rust trait 的可替换实现。

## 扩展指南

新增或修改任务注册接口时，应同步检查四处：Go 来源 `br/pkg/mock/task_register.go`、本文件的 mock 与 recorder 成对方法、`br/pkg/mock/lib.rs` 的导出边界，以及独立测试 `br/pkg/mock/parity_test.rs`。若真实接口也变化，还应核对 `br/pkg/utils/register.rs::TaskRegister` 及其独立测试 `br/pkg/utils/register_test.rs`，但不要把真实状态机逻辑复制进 mock。

新增方法时，业务方法应继续调用 `Helper`、使用唯一且一致的方法名调用 `Controller::Call`，并选择与返回形状匹配的 `take_*` 解码器；recorder 方法必须使用同一个方法名并返回 `Call`。应在 `parity_test.rs` 增加成功、错误、未登记调用或多次同名期望中与变更最相关的覆盖，Rust 测试逻辑继续放在独立测试文件中。

若要支持参数 matcher、调用次数、动作回调或严格顺序，这属于共享 `stubs::Controller` 的能力扩展，会影响同 crate 的所有生成式 mock；应先评估兼容性与锁竞争，而不是只在本文件做特例。若目标是让 mock 实现真实 `TaskRegister` trait，则必须显式处理 receiver、context 借用及错误类型差异，并为 trait-object 注入补充专门测试。

## 验证依据

- 源文件：`br/pkg/mock/task_register.rs`，核对全部 104 行、两个结构体、一个构造函数、两个 impl 和七个公开方法；无条件编译项。
- 装配与 crate：`br/pkg/mock/lib.rs`、`br/pkg/mock/Cargo.toml`，确认模块加载、平铺导出、library 边界及空依赖表。
- 共享运行机制：`br/pkg/mock/stubs.rs` 的 `Context`、`Controller::{new, Helper, Call, RecordCallWithMethodType, remaining}`、`Call::{Return, ReturnError}` 与 `take_error`。
- Go 对照：`br/pkg/mock/task_register.go`，以及真实接口与实现 `br/pkg/utils/register.rs::TaskRegister` / `TaskRegisterImpl`。
- 独立测试：`br/pkg/mock/parity_test.rs::go_rust_public_contract_matches` 和 `gomock_expectations_match_out_of_registration_order`；真实状态机边界另见 `br/pkg/utils/register_test.rs`。Go 集成注入例见 `tests/realtikvtest/importintotest/import_into_test.go::TestRegisterTask`。
- RustCodeGraph：`status` 显示索引含 7032 个 Rust 文件；`files --filter br/pkg/mock` 收录目标、Go 对照和相关测试；`query MockTaskRegister --kind struct` 与 `query NewMockTaskRegister --kind function` 定位 Rust/Go 对应符号；`node --file br/pkg/mock/task_register.rs --offset 1 --limit 180` 核对完整文件；`callees NewMockTaskRegister` 未发现下游调用，图的缺边由模块与文本引用补证。
- 未运行 Cargo：任务是纯文档分析，计划明确 Cargo 共享槽位规则不适用。交付前按任务给定命令验证文档存在且恰有 11 个固定二级标题。
