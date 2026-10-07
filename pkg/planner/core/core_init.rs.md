# `pkg/planner/core/core_init.rs`

## 文件定位

源文件是 [`core_init.rs`](core_init.rs)，属于 `astersql-planner-core` crate。crate 根在 [`lib.rs`](lib.rs) 中以私有模块 `mod core_init` 挂载它，再通过 `pub use core_init::*` 导出两个公开函数；对应的独立测试由 `#[path = "core_init_test.rs"] mod core_init_test` 接入。crate 边界由 [`Cargo.toml`](Cargo.toml) 声明，库入口是 `lib.rs`，默认 feature 为空，`nextgen` feature 也不改变本文件的编译路径。

它当前位于 Go 规划器初始化逻辑向 Rust 迁移的兼容核对层：保存 Go `pkg/planner/core/core_init.go::init` 所做 111 项接线的名称清单，但不保存或安装这些接线对应的 Rust 函数指针。因而它不是 SQL 优化主链中的回调分派器，也不能单凭名称存在证明相关规划器行为已经接通。

## 核心职责

- `CALLBACKS` 按 Go `init()` 的语句顺序列出 111 个初始化副作用名称，覆盖受限 hint 检查、物理计划枚举、任务附着、索引解析、两版代价模型、访问路径与统计推导、MV index、表达式/编码辅助和默认逻辑规则等类别。
- `init()` 将上述名称并入进程内全局注册表，允许重复调用而不产生重复元素。
- `RegisteredPlannerCallbacks()` 提供注册表快照，供迁移完整性测试或诊断读取。

这里的“注册”仅指注册**名称**；Go 对照文件则把包级变量实际赋值为函数、方法或对象。两者的行为层级不同。

## 主要符号

- `const CALLBACKS: &[&str]`：私有静态字符串切片。源文件中的顺序精确对应 Go `init()` 的 111 个副作用，但它本身不是可调用回调表。
- `static REGISTRY: OnceLock<RwLock<BTreeSet<&'static str>>>`：私有全局状态。`OnceLock` 只初始化一次锁和空集合，`RwLock` 负责并发读写，`BTreeSet` 去重并让查询快照按字符串排序。
- `pub fn init()`：公开、无返回值的幂等填充入口。它取得写锁，然后执行 `extend(CALLBACKS.iter().copied())`。
- `pub fn RegisteredPlannerCallbacks() -> BTreeSet<&'static str>`：公开查询入口。命名沿用 Go 风格并由 crate 根的 `#![allow(non_snake_case)]` 接受；它先调用 `init()`，再在读锁下克隆集合，所以调用方获得独立快照而非锁守卫或可变全局引用。

RustCodeGraph 显示 `init()` 的直接调用者是 `RegisteredPlannerCallbacks()`；`RegisteredPlannerCallbacks()` 的已知调用者仅为 `core_init_test.rs` 中两个测试。

## 执行流程

1. 调用方进入 `RegisteredPlannerCallbacks()`。
2. 查询入口先调用 `init()`，因此调用者无需安排单独的包级初始化时机。
3. `init()` 通过 `REGISTRY.get_or_init` 首次建立 `RwLock<BTreeSet<_>>`；后续调用复用同一实例。
4. `init()` 获取写锁，把 `CALLBACKS` 的 111 个名称扩展进集合。集合语义会消除重复项，所以重复执行结果不变。
5. 查询入口在 `init()` 释放写锁后取得读锁，克隆完整集合并返回。

没有生产入口在进程启动时主动调用本文件的 `init()`；目前只有读取注册名称时才惰性填充。实际优化、代价估算或 `Attach2Task` 流程不会通过本注册表调用任何实现。

## 数据与状态

所有名称都是编译期 `&'static str`，注册表不拥有动态字符串，也没有释放单个元素的生命周期问题。全局容器从第一次调用起存活到进程结束。

`CALLBACKS` 保留 Go 赋值顺序；`REGISTRY` 使用 `BTreeSet`，因此对外快照只表达“有哪些名称”，迭代顺序是词典序，不能用来恢复 Go 初始化次序。集合中也没有函数地址、签名、初始化目标模块或“已真实接线”的状态位。

测试断言当前集合长度为 111，并抽查 `DefaultDisabledLogicalRulesList`。对 Go/Rust 两个文件的名称提取进一步确认：111 个名称数量相同、顺序相同。

## 依赖与调用关系

本文件的直接代码依赖全部来自标准库：`std::collections::BTreeSet` 与 `std::sync::{OnceLock, RwLock}`；它没有直接使用 `Cargo.toml` 中列出的其他 planner、expression、statistics 或 operator crate 依赖。

模块关系为 `lib.rs -> mod core_init -> pub use core_init::*`。当前调用边为 `core_init_test.rs -> RegisteredPlannerCallbacks() -> init() -> {REGISTRY, CALLBACKS}`，以及查询函数对 `REGISTRY` 的读取。RustCodeGraph 未显示 SQL 构建、逻辑优化或物理优化生产路径调用这两个函数。

Go 对照的下游依赖更广：`core_init.go::init()` 会写入 `utilfuncp`、`cardinality`、`statistics`、`base`、`expression`、`plannerutil` 等包的槽位，并调用 hint 注册函数。Rust 本文件只把这些目标的最后一级名称记录为字符串，不形成对应调用边。

## 错误处理与边界

两个公开函数都不返回 `Result`。`RwLock::write()` 或 `RwLock::read()` 若发现锁已 poisoned，会通过带固定消息的 `expect` 触发 panic；正常情况下，闭包内只有创建空 `BTreeSet`，写入也没有业务错误分支。查询中 `REGISTRY.get().expect("registry initialized")` 依赖紧邻的 `init()` 后置条件；除非代码以后改变初始化流程，否则该分支不可达。

关键边界是语义能力：名称完整不等于回调已实现、类型签名匹配或运行时已安装。新增 Go 初始化副作用若未同步到 `CALLBACKS`，现有“长度 + 单点抽查”测试未必精确指出缺失名称；反之，即使同步了字符串，也仍需在真实 Rust 实现所在模块完成接线和行为测试。

## 并发与资源生命周期

`OnceLock` 保证多个线程首次访问时只创建一个 `RwLock`；`RwLock` 序列化 `init()` 的集合扩展，并允许初始化完成后的并发读取。因为每次查询都会先取得一次写锁再取得读锁，即使集合早已填充，高频查询仍有一次不必要的独占锁开销；当前调用者只有测试，所以这不是已观察到的执行热点。

锁守卫都局限在函数内部：`init()` 在返回前释放写锁，查询函数在克隆完成后释放读锁，返回值不携带锁。没有线程、异步任务、通道、事务、文件或网络资源。若持锁期间未来加入可能 panic 的逻辑，poisoning 会使后续读写按当前 `expect` 策略继续 panic。

## 与 Go 版本的对应关系

直接对照是 [`core_init.go`](core_init.go) 的包级 `init()`。Rust `CALLBACKS` 的 111 个字符串与 Go 中第一项 `hint.RegisterRestrictedHintChecker(...)` 以及其后 110 个赋值的名称和顺序完全一致。

主要差异如下：

- Go 在包初始化阶段自动执行，并把真实函数、绑定方法、`RootTask` 对象和 `atomic.Value` 等值写入跨包槽位；Rust 没有等价包级副作用，本文件仅在显式查询时写入字符串集合。
- Go 的执行顺序可能有语义，例如先建立优化器和表达式辅助槽位；Rust 的公开结果是 `BTreeSet`，不暴露原始次序。
- Go 的 `DefaultDisabledLogicalRulesList` 会创建 `atomic.Value` 并存入空字符串集合；Rust 这里只记录同名条目，没有复制该状态对象。
- Go 文件真实依赖 expression、cardinality、physical operator、statistics、hint、SEM 等包；Rust 文件只依赖标准库。因此本文件应被视为迁移覆盖清单和测试支点，而不是 Go 初始化行为的运行时等价实现。

## 扩展指南

当 Go `core_init.go::init()` 增删、重命名或重排副作用时，应同步修改 `CALLBACKS`，并在独立的 [`core_init_test.rs`](core_init_test.rs) 中更新精确覆盖断言；不要把测试写回生产源文件。建议测试直接比较从 Go 对照维护出的完整预期名称序列或集合，而不只检查总数和单个哨兵项。

若目标是让 Rust 真正执行某项规划器接线，应修改该行为所属的实现模块与独立测试，并设计类型安全的函数/trait 边界；不要把字符串注册表扩展成依赖字符串分派的执行框架，除非有明确的兼容协议。若保留顺序有运行时意义，也不应从 `BTreeSet` 读取顺序，而应显式使用 `CALLBACKS` 或专门的有序结构。

修改并发或错误策略时要保持 `init()` 幂等，并评估锁 poisoning、热路径写锁和快照克隆成本。任何新增公开 API 仍会经 `lib.rs` 的通配重导出暴露，应同时检查命名和 crate 边界兼容性。

## 验证依据

- RustCodeGraph `node --file pkg/planner/core/core_init.rs`：读取完整 163 行源码，确认 111 项常量、全局注册表和两个函数。
- RustCodeGraph `node core_init.rs::init`、`node RegisteredPlannerCallbacks`、`callees RegisteredPlannerCallbacks`：确认 `RegisteredPlannerCallbacks -> init -> CALLBACKS/REGISTRY`，以及测试调用者。
- RustCodeGraph `node --file pkg/planner/core/lib.rs`：确认私有模块声明、公开重导出和独立测试挂载位置。
- [`Cargo.toml`](Cargo.toml)：确认 crate 名称 `astersql-planner-core`、`lib.rs` 入口、feature 边界及 Go 包迁移元数据。
- [`core_init.go`](core_init.go)：核对 Go 包级初始化的真实赋值目标、对象初始化和跨包依赖。
- [`core_init_test.rs`](core_init_test.rs)：确认 111 项覆盖断言、哨兵名称和重复查询幂等性。
- 名称对照检查分别从 Rust 字符串列表与 Go 注册调用/赋值中提取 111 项，按顺序比较退出码为 0。
- 本任务是纯文档分析，未运行 Cargo；结构验证按任务文件给出的 11 个固定标题命令执行。
