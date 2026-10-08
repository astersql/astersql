# `pkg/util/tokenlimiter.rs`

## 文件定位

本文件属于 `astersql-util` crate；crate 由 [`pkg/util/Cargo.toml`](Cargo.toml) 定义，并通过 [`pkg/util/lib.rs`](lib.rs) 中的 `pub mod tokenlimiter;` 公开模块。它是 Go [`pkg/util/tokenlimiter.go`](tokenlimiter.go) 的 Rust 移植，提供一个用预填充有界通道实现的并发许可池。

当前 Rust 侧的直接使用者只有注册到 `security_2_aster_unit_test` 测试目标的 `token_limiter_blocks_until_a_token_is_returned`（[`pkg/util/security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs)）。仓库搜索未发现 Rust 生产模块调用 `NewTokenLimiter`；因此它目前是“已导出、已测试、尚未接入 Rust 生产主链”的通用组件。完整应用中的对应生产位置仍在 Go 服务端：[`pkg/server/server.go`](../server/server.go) 的 `Server.concurrentLimiter` 限制并发连接处理，`getToken` 获取许可，`releaseToken` 归还许可。

## 核心职责

- `NewTokenLimiter` 创建固定容量的许可池，并在返回前放入恰好 `count` 个许可。
- `TokenLimiter::Get` 阻塞等待一个可用许可，从而在许可全部借出时形成背压。
- `TokenLimiter::Put` 将已借出的许可放回池中，使一个等待者能够继续。
- `TokenLimiter::Count` 报告创建时的配置容量，而不是实时剩余许可数。

该类型只控制“同时持有许可的数量”，不负责启动任务、取消等待、超时、公平调度、自动归还或统计等待时间。调用者必须把一次成功的 `Get` 与一次 `Put` 配对。

## 主要符号

- `pub struct Token`：零字段、零大小的许可标记。它没有业务数据；`Box<Token>` 的所有权用于在 API 层表达“取走一个具体许可，再把它归还”。`Debug` 和 `Default` 仅提供常规构造/调试能力。
- `pub struct TokenLimiter`：许可池主体。字段均为私有：`count: usize` 保存固定容量，`sender: Sender<Box<Token>>` 是归还端，`receiver: Receiver<Box<Token>>` 是获取端。
- `pub fn NewTokenLimiter(count: usize) -> Arc<TokenLimiter>`：公开构造函数。创建 `crossbeam_channel::bounded(count)`，同步预填 `count` 个 `Box<Token>`，再用 `Arc` 包装池以支持多线程共享。
- `pub fn TokenLimiter::Get(&self) -> Box<Token>`：从 `receiver` 阻塞接收许可；通道断开时以 `expect` 触发 panic。
- `pub fn TokenLimiter::Put(&self, token: Box<Token>)`：向 `sender` 阻塞发送许可；通道断开时以 `expect` 触发 panic。若池已满，它同样会阻塞。
- `pub fn TokenLimiter::Count(&self) -> usize`：返回构造时记录的 `count`，不会检查通道当前长度。

符号沿用 Go 风格的首字母大写命名；[`pkg/util/lib.rs`](lib.rs) 在 crate 根允许 `non_snake_case`，因此这些名称是迁移兼容选择而非标准 Rust 命名。

## 执行流程

1. 调用者执行 `NewTokenLimiter(count)`；构造函数建立容量为 `count` 的有界 MPMC 通道。
2. 构造函数循环 `count` 次，将新的空 `Token` 装箱后发送进通道。正常的正容量构造结束时，通道恰好装满。
3. 每个并发任务在进入受限区前调用 `Get`。只要池中仍有令牌，`recv` 立即取走一个；最后一个令牌被取走后，后续 `Get` 停在通道接收操作上。
4. 持有者完成受限工作后调用 `Put(token)`。发送成功后池中许可数增加，等待中的接收者可被唤醒并取得该许可。
5. `Arc<TokenLimiter>` 只共享池本身；许可仍以 `Box<Token>` 独占传递，使正常 API 路径不能同时把同一许可交给两个调用者。

测试 `token_limiter_blocks_until_a_token_is_returned` 使用容量 2：主线程先取走两个许可，再让子线程执行第三次 `Get`；确认子线程保持阻塞后，主线程归还一个许可，子线程才继续并归还其所得许可，最后主线程归还第二个许可。

## 数据与状态

稳定状态由两个量构成：配置容量 `count` 和通道中当前可获取的令牌集合。若所有调用都严格配对，则“通道内令牌数 + 已借出令牌数 = count”。源码没有显式计数已借出令牌，也没有运行时检查这一守恒关系。

`count` 在构造后不可修改，`Count` 因此可并发读取且始终返回原始容量。实时可用数量只存在于 `crossbeam_channel` 内部，本文件没有公开查询接口。`Token` 不携带身份、时间或任务信息；不同令牌在语义上等价。

所有权能防止同一个 `Box<Token>` 被重复归还，但公开的 `Token: Default` 允许调用者自行创建额外令牌并传给 `Put`。源码不会验证令牌来源；向已满的有界通道放入额外令牌会永久阻塞，而在池未满时放入外造令牌则会破坏容量守恒，之后也可能导致 `Put` 阻塞。

## 依赖与调用关系

下游依赖只有标准库和 `crossbeam-channel`：`std::sync::Arc` 提供共享所有权，`crossbeam_channel::bounded` 返回可跨线程使用的 `Sender`/`Receiver`。[`pkg/util/Cargo.toml`](Cargo.toml) 明确声明 `crossbeam-channel = "0.5"`；没有 feature 或条件编译改变本文件行为。

RustCodeGraph 将 `NewTokenLimiter` 定位到本文件第 58 行，并给出直接调用边 `token_limiter_blocks_until_a_token_is_returned -> NewTokenLimiter`。对 `Get`/`Put` 这类常见名称，图索引的 callee 消歧结果存在误配，因此具体方法调用以测试源码的 `limiter.Get()`、`cloned.Put(token)` 和类型上下文复核。

Rust 模块接线是 `pkg/util/lib.rs -> tokenlimiter`，测试接线是 [`pkg/util/Cargo.toml`](Cargo.toml) 的 `security_2_aster_unit_test` 测试目标指向 [`pkg/util/security_formal_aster_unit_test.rs`](security_formal_aster_unit_test.rs)，后者通过 `include!` 纳入 `security_2_aster_unit_test.rs`。仓库搜索未发现其他 Rust 调用者。

Go 生产调用链为 `server.NewServer -> util.NewTokenLimiter(cfg.TokenLimit)`，连接处理通过 `Server.getToken -> TokenLimiter.Get` 获取许可，并通过 `Server.releaseToken -> TokenLimiter.Put` 归还；Rust 当前没有对应生产调用链。

## 错误处理与边界

API 不返回 `Result`。`Get` 和 `Put` 分别对 `recv`/`send` 使用 `expect`，通道断开时 panic，消息为 `token limiter channel disconnected`。在当前结构中，`TokenLimiter` 同时持有发送端和接收端，普通借用期间不会只销毁其中一端，因此断开错误通常只会与对象销毁/异常生命周期交叠；没有可供调用者主动关闭通道的接口。

`NewTokenLimiter(0)` 是重要边界：`bounded(0)` 创建零容量同步通道，而构造循环执行零次，所以构造能够返回；之后任意单独的 `Get` 都会无限等待，因为不存在初始令牌，也没有调用者能先取得可归还的令牌。当前测试未覆盖零容量。

`Get` 没有超时和取消分支。遗漏 `Put`、持有者 panic 或丢弃取得的 `Box<Token>` 会永久减少可用许可，最终可能让所有后续获取者阻塞。反之，错误地归还外造令牌或重复设计上的许可数量会让 `Put` 在满通道上阻塞。析构没有自动修复这些情况。

构造阶段的 `send(...).expect("token limiter initialization failed")` 在正常本地通道两端均存活且容量为 `count` 时不应失败或阻塞；该 `expect` 是对内部不变量被破坏的防御性断言。

## 并发与资源生命周期

`crossbeam_channel` 的发送端和接收端承担线程安全同步，`TokenLimiter` 无需额外互斥锁；`Arc` 允许多个 worker 共享同一实例。获取与归还都是可能阻塞的同步操作，不适合直接放在要求不可阻塞的异步执行器线程上，除非外围明确接受阻塞或将其移至阻塞线程池。

许可生命周期为“构造时进入池 -> `Get` 转移给一个持有者 -> `Put` 转回池”。`Box<Token>` 本身几乎没有数据成本，真正的资源语义来自通道槽位和所有权转移。最后一个 `Arc<TokenLimiter>` 被销毁时，内部 sender/receiver 一并销毁，通道中未借出的 `Box<Token>` 随之释放；已借出的令牌独立存在，之后无法通过已销毁的 limiter 归还。

实现不承诺等待者的严格 FIFO 公平性。性能成本主要是通道同步与每个初始令牌的一次装箱分配；容量很大时，构造会执行 `count` 次分配和发送。

## 与 Go 版本的对应关系

Rust 的 `Token`、`TokenLimiter`、`Put`、`Get` 和 `NewTokenLimiter` 逐项对应 [`pkg/util/tokenlimiter.go`](tokenlimiter.go) 的同名符号。两版都使用容量固定的有界通道并预填 `count` 个空令牌；`Get` 在耗尽时阻塞，`Put` 在池已满时阻塞，因此核心并发语义一致。

类型映射为 Go `uint` 到 Rust `usize`、Go `chan *Token` 到 Rust 的 `Sender<Box<Token>>`/`Receiver<Box<Token>>` 分离端点、Go `*TokenLimiter` 到 Rust `Arc<TokenLimiter>`。Go 通道发送/接收由运行时管理；Rust 通过 `crossbeam-channel` 实现同类同步，并在断开结果上显式 panic。

Rust 额外提供 `Count`，Go 的 `count` 字段未导出且没有同名方法。当前 Rust 测试用它验证配置容量，但生产逻辑不依赖它。Rust 的 `Token: Default` 也比 Go 空结构体更显式地暴露外造令牌能力；两种语言实际上都允许包内/公开类型使用者构造空令牌，因此都依赖调用约定维持容量守恒。

Go 版已由 [`pkg/server/server.go`](../server/server.go) 接入 MySQL 服务并围绕获取过程记录 `TokenGauge` 和等待时长；Rust 版尚无对应服务接线或指标逻辑，不能把 Go 生产用法表述成 Rust 已支持的主链。

## 扩展指南

- 若增加超时、取消或非阻塞获取，应优先在 `TokenLimiter` 上新增明确方法（例如返回 `Option`/`Result`），保留现有阻塞 `Get` 的 Go 兼容语义；测试应放在独立测试文件，而不是嵌入 `tokenlimiter.rs`。
- 若需要自动归还，宜引入持有 limiter 引用和 token 的 RAII guard，并仔细规定 guard 移动、析构期 panic 和 limiter 已销毁时的行为；不能仅在 `Token::drop` 中归还，因为 `Token` 当前不知道所属池。
- 若要禁止外造令牌破坏不变量，应收紧 `Token` 的构造能力或让归还句柄携带池身份。此变更会影响公开 API，与 Go 的公开空结构体也可能产生兼容差异。
- 若接入 Rust 服务端生产链，应仿照 Go 的 `NewServer/getToken/releaseToken` 接线，明确指标计数、等待耗时、连接异常退出时归还许可，并添加独立的服务层集成测试。
- 若改变容量类型、阻塞策略或公平性，需要同步审查 `NewTokenLimiter`、`Get`、`Put`、`Count`，并扩展现有阻塞测试以覆盖零容量、多个等待者、持有者异常和错误归还；性能评估应关注大容量初始化分配及高竞争通道开销。

## 验证依据

- 源码事实：[`pkg/util/tokenlimiter.rs`](tokenlimiter.rs)；核对了 `Token`、`TokenLimiter`、`Put`、`Get`、`Count`、`NewTokenLimiter` 的完整实现及无条件编译状态。
- crate 与模块事实：[`pkg/util/Cargo.toml`](Cargo.toml) 的 `astersql-util`、`crossbeam-channel = "0.5"`、独立测试目标，以及 [`pkg/util/lib.rs`](lib.rs) 的 `pub mod tokenlimiter;`。
- Go 对照及生产链：[`pkg/util/tokenlimiter.go`](tokenlimiter.go) 与 [`pkg/server/server.go`](../server/server.go) 的 `concurrentLimiter`、`NewServer`、`getToken`、`releaseToken`。
- Rust 测试：[`pkg/util/security_2_aster_unit_test.rs`](security_2_aster_unit_test.rs) 的 `token_limiter_blocks_until_a_token_is_returned`；[`pkg/util/security_formal_aster_unit_test.rs`](security_formal_aster_unit_test.rs) 证明该文件通过 `include!` 进入 Cargo 声明的测试目标。
- RustCodeGraph：`status` 显示索引可用；`query NewTokenLimiter`、`query TokenLimiter` 定位 Go/Rust 对照符号；`node NewTokenLimiter` 给出 Rust 源码及测试到构造函数的调用边；测试节点和 `callees` 查询用于复核测试流程，并识别常见方法名的消歧限制。
- 仓库引用搜索：`rg` 仅找到 Rust crate 导出和上述独立测试调用；Rust 生产接线未找到，因此本文明确标为未接入，而非推断其已用于完整应用。
- 本任务是纯文档分析，按任务约束未运行 Cargo。交付前使用任务规定的命令验证目标文档存在且恰好包含 11 个固定二级章节，并人工复核所有路径、符号、调用关系与未验证边界的措辞。
