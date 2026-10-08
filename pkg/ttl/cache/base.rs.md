# `pkg/ttl/cache/base.rs`

## 文件定位

`base.rs` 位于 `astersql-ttl-cache` crate（声明见 `pkg/ttl/cache/Cargo.toml`），由 crate 入口 `pkg/ttl/cache/lib.rs` 以 `pub mod base` 导出。它不保存 TTL 表或作业内容，而是提供一个最小的“刷新节拍”组件 `baseCache`，让具体缓存统一判断何时应重新读取数据。

当前 Rust 代码中，`InfoSchemaCache`（`pkg/ttl/cache/infoschema.rs`）和 `TableStatusCache`（`pkg/ttl/cache/ttlstatus.rs`）各自把 `baseCache` 作为私有字段组合进去。前者缓存开启 TTL 的物理表元数据，后者缓存 `mysql.tidb_ttl_table_status` 的作业状态。RustCodeGraph 的文件关系也显示 `base.rs` 被这两个实现及相应独立测试引用。

需要区分“组件已实现”和“完整调度链已接线”：仓库内 Rust 生产代码对 `NewInfoSchemaCache` / `NewTableStatusCache` 的直接引用目前只在各自定义文件中，尚未看到 Rust TTL JobManager 使用它们；完整的周期调度入口可在 Go 的 `pkg/ttl/ttlworker/job_manager.go::jobLoopWithSession` 中确认。因此本文只把 Rust cache crate 内的组合关系描述为已实现事实。

## 核心职责

本文件只有一项职责：维护缓存刷新所需的两个时间状态，并提供四种操作。

- `newBaseCache` 创建尚未成功刷新过的节拍对象。
- `baseCache::ShouldUpdate` 判断首次刷新或刷新间隔是否已经严格超时。
- `baseCache::SetInterval` / `GetInterval` 修改和读取最短刷新间隔。
- `baseCache::MarkUpdated` 在上层缓存成功同步数据后记录当前单调时钟时刻。

它不负责启动定时器、执行 SQL、读取 InfoSchema、替换缓存内容或处理重试。何时调用 `ShouldUpdate`、何时发起更新，以及更新失败后的策略，均属于上层调用者职责。

## 主要符号

- `pub struct baseCache { interval: Duration, update_time: Option<Instant> }`：公开类型、私有状态。类型名沿用 Go 移植命名，并由 `pkg/ttl/cache/lib.rs` 的 lint 允许项接受非 Rust 惯用命名。
- `pub fn newBaseCache(interval: Duration) -> baseCache`：公开构造函数。保留传入的 `Duration`，把 `update_time` 初始化为 `None`。
- `pub fn baseCache::ShouldUpdate(&self) -> bool`：只读判定。`update_time == None` 时返回 `true`；否则计算 `updated.elapsed() > interval`。
- `pub fn baseCache::SetInterval(&mut self, interval: Duration)`：立即替换间隔，不改动最近更新时间。
- `pub fn baseCache::GetInterval(&self) -> Duration`：按值返回 `Duration`。当前 Rust 生产调用中未发现使用者，直接覆盖在 `pkg/ttl/cache/base_test.rs`。
- `pub fn baseCache::MarkUpdated(&mut self)`：把 `update_time` 更新为 `Some(Instant::now())`。它是 Rust 对 Go 直接写入匿名嵌入字段 `updateTime` 的封装。

文件没有常量、trait、枚举、条件编译项或错误类型。

## 执行流程

典型生命周期如下：

1. 上层通过 `newBaseCache(interval)` 构造节拍；由于 `update_time` 为 `None`，第一次 `ShouldUpdate()` 必然返回 `true`，与间隔大小无关。
2. 上层据此执行真实刷新。`InfoSchemaCache::Update` 重建 `Tables` 并更新 schema version；`TableStatusCache::Update` 执行状态查询、解码所有行并整体替换 `Tables`。
3. 只有刷新路径完成后，上层才调用 `MarkUpdated()`。此后 `ShouldUpdate()` 用 `Instant::elapsed()` 与当前 `interval` 比较。
4. 若运行期调用 `SetInterval()`，下一次判断立即采用新间隔，但计时起点仍是最近一次 `MarkUpdated()`；缩短间隔可能立即触发刷新，延长间隔会推迟刷新。
5. 时间差必须满足严格的 `elapsed > interval` 才返回 `true`；恰好相等时仍为 `false`。

`InfoSchemaCache::Update` 在 schema version 未变化时提前返回，因而不会调用 `MarkUpdated`；这是上层现有语义，而不是 `baseCache` 自身的策略。`TableStatusCache::Update` 则在 SQL 或行解码出错时通过 `?` 提前返回，同样不会标记成功。

## 数据与状态

`interval: Duration` 表示两次成功刷新之间允许的最短等待时间。它不校验零值或极大值：零间隔通常会在时钟产生正向进展后使 `ShouldUpdate` 为真；具体刷新频率仍由调用方轮询频率决定。

`update_time: Option<Instant>` 表示最近一次由调用方确认的成功更新时间。`None` 是显式的“从未更新”状态；`Some(t)` 是单调时钟上的进程内时间点，不是墙上时钟、Unix 时间戳，也不能持久化或跨进程比较。

两个字段都是私有的，外部模块只能通过公开方法维护不变量。`Clone` 会复制同一刷新时刻和间隔，克隆体之后独立演进；`Debug` 仅便于诊断。类型没有实现 `Default`、序列化或相等比较。

## 依赖与调用关系

直接依赖仅有标准库 `std::time::{Duration, Instant}`，因此 `base.rs` 本身不使用 `pkg/ttl/cache/Cargo.toml` 中列出的其他 AsterSQL crate。Cargo 清单表明 `astersql-ttl-cache` 的库入口是 `lib.rs`，且当前业务依赖位于 Windows target 条件段；这一条件不改变本文件纯标准库逻辑。

已核实的 Rust 调用边为：

- `InfoSchemaCache::NewInfoSchemaCache -> newBaseCache`；其 `ShouldUpdate`、`SetInterval` 转发到内部 `baseCache`，`Update` 成功重建时调用 `MarkUpdated`（`pkg/ttl/cache/infoschema.rs`）。
- `NewTableStatusCache -> newBaseCache`；其 `ShouldUpdate`、`SetInterval` 同样转发，`Update` 成功替换状态 map 后调用 `MarkUpdated`（`pkg/ttl/cache/ttlstatus.rs`）。
- `pkg/ttl/cache/base_test.rs::{test_base_cache,test_base_cache_get_interval}` 直接覆盖构造、判定、标记和间隔读写。

Go 完整链路中，`JobManager::jobLoopWithSession` 读取两个具体缓存经匿名嵌入提升出的 `GetInterval()` 来创建 ticker，再由 ticker 驱动 `updateInfoSchemaCache` / `updateTableStatusCache`。Rust 的具体缓存使用命名私有字段，且没有转发 `GetInterval`，所以不能把这条 Go 调度边视为 Rust 当前已经接通。

## 错误处理与边界

本文件的所有 API 都是无失败返回值；没有 I/O、解析、分配型集合操作或业务错误传播。错误边界在上层刷新实现：例如 `TableStatusCache::Update` 的 SQL/解码错误会在 `MarkUpdated` 之前返回，从而保留“仍需刷新”的时间状态。

重要边界包括：

- 初始状态恒需更新，避免新缓存因较长 interval 而在启动后长期保持空内容。
- 判定使用严格大于号，不是大于等于。
- `SetInterval` 不重置计时起点，也不会主动刷新。
- `Instant::elapsed()` 适合进程内单调计时，规避系统墙上时钟跳变；状态不能作为业务时间对外展示或持久化。
- `ShouldUpdate` 只是提示，不提供互斥或“刷新中”标记；多个执行者若共享自己的同步包装，仍可能同时观察到 `true`。

## 并发与资源生命周期

`baseCache` 不含锁、原子变量、通道、任务、线程或外部资源。修改方法要求 `&mut self`，Rust 借用规则可防止同一实例在普通安全代码中同时修改；`ShouldUpdate(&self)` 本身只读。但类型没有实现跨线程刷新协调协议，也没有保证“检查后执行”的原子性。

资源生命周期完全随所有者：`InfoSchemaCache` 或 `TableStatusCache` 析构时，内部 `baseCache` 一并释放；`Duration` 与 `Instant` 无需显式清理。`MarkUpdated` 应在缓存内容完成整体替换后调用，以免其他逻辑把失败或半完成刷新视为成功。

若未来需要共享并发访问，应由上层选择 `Mutex` / `RwLock` 等同步方式，并把“判断、占有刷新权、执行、成功标记”设计成明确协议；仅给本类型增加 `Send`/`Sync` 说明或复制实例不能防止重复刷新。

## 与 Go 版本的对应关系

对应文件是 `pkg/ttl/cache/base.go`，核心语义保持一致：都有 `interval`、最近更新时间、`newBaseCache`、`ShouldUpdate`、`SetInterval` 和 `GetInterval`，并以“距上次更新严格大于 interval”判定超时。

主要差异如下：

- Go 用 `time.Time` 零值表示从未更新；Rust 用 `Option<Instant>::None`，首次判断语义更显式。
- Go 上层通过匿名嵌入 `baseCache` 获得方法提升，并直接写 `updateTime = time.Now()`；Rust 使用命名私有字段和显式转发方法，成功时间通过新增的 `MarkUpdated` 封装。
- Go 使用 `time.Since` / `time.Now`；Rust 使用进程内单调 `Instant`，更明确地把该值限制为刷新节拍而非业务时间。
- Go 的 `InfoSchemaCache`、`TableStatusCache` 因匿名嵌入自然暴露 `GetInterval`，JobManager 用它创建 ticker；Rust 两个包装类型当前未转发 `GetInterval`，且未发现等价 Rust JobManager 生产接线。
- Go `TestBaseCache` 直接修改同包私有字段 `updateTime`；独立 Rust 测试通过公开 `MarkUpdated` 完成同一行为，并额外验证 `GetInterval` / `SetInterval` 一致性。

这些差异说明 Rust 文件是忠实的基础节拍移植，但完整上层可见 API 与调度接线尚不能仅凭本文件推定等价。

## 扩展指南

调整刷新判定时，优先修改 `baseCache::ShouldUpdate`，并在独立文件 `pkg/ttl/cache/base_test.rs` 增加边界用例；不要把测试嵌入 `base.rs`。至少应考虑首次刷新、零间隔、严格边界、动态缩短/延长 interval，以及 `MarkUpdated` 后的行为。

若新增“强制失效”能力，应新增明确方法把 `update_time` 设回 `None`，而不是向外暴露字段。若新增抖动、退避或失败冷却，需要先决定它属于所有缓存共享的节拍策略，还是具体缓存的错误策略；后者应留在 `infoschema.rs` / `ttlstatus.rs`，避免基础类型混入 SQL 或 schema 语义。

若要对齐 Go JobManager 的周期调度，最小接线点是为 Rust 的 `InfoSchemaCache` 和 `TableStatusCache` 明确提供 `GetInterval` 转发（或由构造方单独持有配置），随后在真正的 Rust 调度入口使用；同时应更新各自独立测试，而不能仅依赖 `base_test.rs`。这属于未来实现范围，本文未把它写成已支持能力。

性能风险主要来自过短间隔造成频繁全量扫描；正确性风险来自在刷新失败或内容替换前过早调用 `MarkUpdated`；兼容风险来自改变严格 `>` 判定、首次必刷语义或公开命名，从而偏离 Go 行为及现有调用约定。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；目标目录共 24 个 Go/Rust 文件，`base.rs` 识别出 8 个符号。
- RustCodeGraph `node --file pkg/ttl/cache/base.rs`：核实 `baseCache`、`newBaseCache`、四个方法及全部字段/分支。
- RustCodeGraph `node`：读取 `pkg/ttl/cache/lib.rs`、`infoschema.rs`、`ttlstatus.rs`、`base_test.rs`，核实模块导出、两个组合调用者、成功标记位置与独立测试。
- RustCodeGraph `query`：区分 Go/Rust 同名 `newBaseCache`、`ShouldUpdate`、`SetInterval`、`MarkUpdated`；精确 `callers` 未产生可用输出，因此使用限定 `pkg/ttl` 的 `rg` 补齐嵌入字段的方法调用。
- Cargo 与源码：`pkg/ttl/cache/Cargo.toml`、`pkg/ttl/cache/base.go`、`pkg/ttl/cache/base_test.go`、`pkg/ttl/ttlworker/job_manager.go`；核实 crate 边界、Go 零值/匿名嵌入语义、原测试意图和完整 Go 调度链。
- 人工复核：文档将 Rust 已实现的 cache crate 组合关系与仅在 Go 中确认的完整 JobManager 调度关系分开陈述；没有宣称未找到的 Rust 生产接线已存在。
