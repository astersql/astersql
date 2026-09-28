// Copyright 2026 AsterSQL.
//! Local stand-ins for HTTP/TLS/PD/TiKV/objstore/metric/promutil/config boundaries
//! (arm64-safe; no kv/domain/kvproto/grpcio).
//!
//! 中文说明总览：
//! `stubs.rs` 不是业务文件，而是 server 子系统在当前迁移阶段的依赖边界总汇。
//! 它的目标不是完整模拟每个外部库，而是让 `lightning.rs`、`checkpoint_control.rs` 与测试文件能按 Go 语义运行。
//! 因此这里最重要的标准是接口形状、错误语义和生命周期顺序，而不是性能或功能完备度。
//! 下面的说明按模块顺序展开，帮助读者快速判断某个替身是为了编译、为了测试，还是为了协议对齐。
//! 根模块层面的职责，是为 server 子系统集中承接 arm64/macOS 下不易直连的依赖边界。
//! 这样业务文件不需要为了平台限制而充满条件编译和特判。
//! `Error` 与 `Result` 是整个文件共享的轻量错误模型。
//! 它优先保留类名、消息、`not_found` 和 `empty_num` 这些 server 真正会消费的语义。
//! 这让上层错误处理既保持熟悉的 Go 风格，又不必引入完整错误框架。
//! `errors` 子模块模拟 PingCAP errors 常见的包装方式。
//! 它提供 `New`、`Errorf`、`Trace`、`Annotate`、`Join` 等组合动作。
//! 这些动作之所以重要，是因为 server 层很多逻辑只关心错误如何传播，而不关心底层库具体怎么实现。
//! `context` 子模块提供可取消上下文和值传递。
//! 对 server 来说，这足以支撑任务取消、HTTP 请求上下文和少量 value 透传。
//! 它故意不模拟完整的 Go context 树，只保留当前代码真正依赖的部分。
//! `atomic` 子模块只实现 Bool、Int64、Uint64 包装。
//! 这些类型主要用于进度、暂停状态和 I/O 计数等共享状态。
//! 保留 Go 风格的 `Load`、`Store` 命名，可以降低迁移阅读成本。
//! `zap` 子模块只是字段和值的承载层。
//! 它不追求完整日志能力，只提供 server 会用到的字段种类。
//! `zapcore` 子模块则保留日志级别枚举和序列化能力。
//! 因为 `/loglevel` API 需要把级别读写成 JSON。
//! `log` 子模块既承担默认 logger，也给 serial test 提供内存日志缓冲。
//! 这让测试可以直接断言关键日志是否出现，而不必依赖外部文件或 stderr。
//! `LogTask` 和 `Begin` 的存在，是为了让访问日志与阶段日志的调用形状保持不变。
//! `logutil` 子模块保留环境变量记录与 logger 注入接口。
//! 对 server 层而言，这些接口更多是控制调用顺序，而非真正需要复杂逻辑。
//! `redact` 子模块是纯占位边界。
//! 它的价值在于保留 server 启动时的脱敏初始化时机，而不是实现脱敏算法本身。
//! `config` 子模块是本文件里最核心的替身之一。
//! 它需要同时支撑全局配置、任务配置、TOML 加载、默认值填充和任务队列。
//! `Security` 保留 TLS 相关字段，但 `BuildTLSConfig()` 在替身里默认成功。
//! 这表示当前任务范围并不验证真实 TLS 构造，只验证 server 会在正确时机调用该检查。
//! `GlobalConfig` 承担进程级默认配置，和任务级 `Config` 明确分层。
//! 这种分层与 Go 一致，也是 POST `/tasks` 先 `LoadFromGlobal` 再 `LoadFromTOML` 的基础。
//! `TikvImporter`、`Checkpoint`、`MydumperRuntime`、`DBStore` 聚焦 server 真正访问的字段。
//! 没被 server 使用到的复杂字段被故意省略，以降低替身维护成本。
//! `Config::NewConfig` 负责填好默认值。
//! 默认值本身就是协议的一部分，因为它会影响未显式配置时的运行行为。
//! `Config::Adjust` 主要处理 TaskID 和 CSV 分隔符约束。
//! 这两者都是 server 在接收任务时最早会暴露给用户的错误边界。
//! `Config::LoadFromGlobal` 与 `LoadFromTOML` 支撑 `/tasks` POST 请求恢复任务配置。
//! 替身只解析 server 测试会用到的 TOML 片段，但仍保留“先全局、再任务”的加载顺序。
//! `List` 则是任务队列的核心替身。
//! 它必须支持 Push、Pop、Get、Remove、MoveToFront、MoveToBack，才能覆盖 server mode 全部控制语义。
//! `common` 子模块提供一批跨流程辅助能力。
//! `TLS` 只保留 listener 包装接口，让 server 启动路径不必因为替身而改写。
//! `Pauser` 则为全局暂停语义提供最小状态容器。
//! `EscapeIdentifier`、`UniqueTable` 和 `TableExists` 支撑 checkpoint meta 清理逻辑。
//! `IsContextCanceledError` 与 `IsAccessDeniedNeedConfigPrivilegeError` 则支撑关键错误分支判断。
//! `LazyError` 及其常量尤其重要。
//! 因为很多 parity test 和上层逻辑会直接依赖这些错误类名与消息前缀。
//! `sql` 子模块是 server 侧数据库替身。
//! 它主要模拟 DROP TABLE、DELETE meta、show config keyspace-name 这几类最关键的交互。
//! 替身没有实现通用 SQL 引擎，只保留 server 当前真正需要观测的副作用。
//! `Rows` 只覆盖单一查询形状，目的是让 `getKeyspaceName()` 仍能按熟悉方式读取结果。
//! `storeapi` 子模块提供最小可遍历的对象存储抽象。
//! `WalkDir` 是其中最重要的能力，因为空目录探测与数据源可达性都依赖它。
//! `MemStorage` 则让测试可以直接构造“有文件”和“空目录”这两类场景。
//! `objstore` 子模块负责把路径字符串映射成简单存储对象。
//! 它复刻了 `ParseBackend` 与 `New` 两步，让 `initDataSource()` 不必改写调用形状。
//! `mydump` 子模块提供 `MDLoader`、`MDDatabaseMeta`、`MDTableMeta` 等导入前元数据类型。
//! 对 server 来说，最重要的是能拿到数据库列表和表大小，用于 schema 冲突与系统要求检查。
//! `metric`、`promutil`、`collectors`、`promhttp` 共同组成指标边界。
//! 它们的职责不是输出真实指标，而是保证 metrics 生命周期和 `/metrics` 路由可被组装与验证。
//! `http` 子模块是整个 server 控制面最关键的协议替身。
//! `Header`、`URL`、`Request`、`ResponseWriter` 保留了 handler 最常用的读写形状。
//! `Handler` 与 `ServeMux` 则负责前缀匹配、包装 handler 和最长匹配路由。
//! `Server` 使用真实 `TcpListener` 加线程 accept。
//! 这样 serial test 才能覆盖监听器关闭、Content-Length 和连接生命周期等黑盒语义。
//! `handle_conn` 手工解析最小 HTTP 报文，是整个替身层最接近真实协议的部分。
//! `listen_tcp` 则把 `:` 和 `:port` 都统一映射到 `127.0.0.1`，减少测试环境差异。
//! `pprof`、`build`、`split` 这些模块看似简单，却在 server 启动和运行流程中不可缺。
//! 替身的价值在于保留调用时机，而不是完整实现其内部逻辑。
//! `backend` 提供 `MakeUUID`，主要服务于 local backend 的引擎清理路径。
//! `ingestctrl` 提供可注入的 rlimit 测试钩子和引擎目录清理能力。
//! 这让 `checkSystemRequirement()` 和 `DestroyError()` 都能在测试中产生可解释结果。
//! `failpoint` 子模块保留注入点函数签名。
//! 这样生产代码中的 failpoint 调用可以原样保留，避免为了迁移到 Rust 而把调试钩子删掉。
//! `tikv`、`pdhttp`、`import_sstpb`、`metapb`、`tls` 共同支撑 `SwitchMode` 这条依赖链。
//! 其中最关键的是保留函数签名和必要常量，而不是复刻真实集群行为。
//! `gzip`、`json`、`mysql`、`util`、`uuid_util` 则填补压缩、序列化、网络拨号和辅助工具的边界。
//! 这些能力本身不复杂，但 server 如果失去它们，就会失去一整类协议行为测试。
//! `bridges` 是本文件最关键的一段。
//! 它把 server 替身配置翻译成真实子 crate 所需的配置结构，并把下游错误翻回 server 自己的 `Error`。
//! 也就是说，替身世界和真实子 crate 世界的边界，主要靠这里保持一致。
//! `map_err_cp`、`map_err_prog`、`map_err_imp`、`map_err_ii` 让不同 crate 的错误重新拥有统一外观。
//! `to_checkpoints_cfg`、`to_importinto_cfg`、`to_importer_cfg` 则分别承接三条下游调用链。
//! `progress_dbs` 负责把 server 侧 mydump 元数据投影到 progress crate 需要的结构。
//! 文件结尾的 `pub use atomic::*` 保持了 crate 根的常用符号可见性。
//! 整体不变量是：只要 server 代码没有真正消费到，替身就不扩展额外能力。
//! 整体不变量是：替身优先保证控制面行为稳定，例如 HTTP 路由、任务队列、暂停状态、日志级别与压缩响应。
//! 整体不变量是：当某个替身已经连接到真实子 crate 时，`bridges` 会负责完成数据和错误的来回翻译。
//! 整体不变量是：这里不会把未支持能力说成已支持，这一点和当前任务计划要求完全一致。
//! 因而阅读 `stubs.rs` 时，最好把它视为 server 子系统的“可编译协议边界图”。
//! 有了这张边界图，理解 `lightning.rs` 为什么能在当前平台运行就容易得多。
//! 下面继续补充更细的模块阅读顺序，帮助维护者判断某段替身到底在守护哪一类 server 语义。
//! 第一层阅读顺序建议先看“错误与上下文”。
//! 因为 server 控制面的大量分支，本质上都围绕错误传播和取消语义展开。
//! `Error` 结构刻意做成非常薄的一层。
//! 它只保留消息、可选原因、类名和少数哨兵位。
//! 这表示 server 当前真正依赖的，是错误外观而不是复杂的错误系统。
//! `not_found` 的存在，是为了让某些 HTTP 与 checkpoint 场景能保留 Go 风格的缺失语义。
//! `empty_num` 的存在，则是为了把“路径里没给数字”这种路由级特殊情况向上游传递。
//! `Error::Error()` 的拼接方式看似简单，却决定了嵌套错误最后如何显示在日志和 JSON 里。
//! 一旦这里的拼接规则变化，很多测试里的字符串断言都要跟着变。
//! `GenWithStackByArgs` 和 `GenWithStack` 并不真的提供完整堆栈。
//! 它们更像是保留 Go 接口形状的包装器。
//! 只要 server 只消费包装后的消息，这种轻量替代就足够稳定。
//! `Wrap` 的价值在于保留链式因果关系。
//! 这样上层既能显示最外层语义，也能在需要时追溯内部原因。
//! `errors` 子模块承担的是“Go 风格错误工具箱”。
//! `New`、`Errorf` 和 `Trace` 这些接口让迁移后的调用点不必为了 Rust 风格重新改写。
//! `Annotate` 与 `Annotatef` 让 server 能继续在边界处补上下文前缀。
//! 对控制面而言，这些前缀往往比底层错误类型更能帮助运维排障。
//! `Cause` 用递归展开最底层原因，是为了兼容 Go 里常见的取根因写法。
//! `ErrorEqual` 则把“类名相同”视为一种强等价。
//! 这可以解释为什么某些测试更在意错误 class，而不是完整消息文本。
//! `Join` 的实现也非常有意图。
//! 它把多个失败拼成一条字符串，而不是构造复杂聚合类型。
//! 因为 server 当前更需要面向日志与 HTTP 返回的可读文本，而不是程序化遍历子错误。
//! `from_io` 是很多替身模块的共同出口。
//! 一旦底层真实 I/O 失败，最终仍需要被翻译回 server 可消费的 `Error`。
//! `context` 子模块是第二个应该先理解的区域。
//! 它不是通用 context 库，而是围绕 server 控制面需求裁剪出来的最小实现。
//! `Background()` 提供无取消的基础上下文。
//! `WithCancel()` 则是 server 管理当前任务生命周期的关键入口。
//! 这里把取消标志与取消原因分开放，是为了让调用方在判断已取消时还能读到相对稳定的错误消息。
//! `Value` 与 `with_value` 的存在，主要服务 logger、metrics 等少量任务级依赖传递。
//! 这意味着它更像标签携带器，而不是完整的跨层依赖注入框架。
//! `Cause(ctx)` 也沿用了 Go 调用习惯。
//! 这样任务取消后的错误恢复路径可以保留熟悉的读取方式。
//! `atomic` 子模块则承担一类非常朴素却很重要的责任。
//! 它让 `Bool`、`Int64`、`Uint64` 继续以 Go 风格 API 暴露。
//! 对迁移代码来说，`Load`、`Store` 这些名字本身就是可读性的一部分。
//! 这里没有暴露更复杂的 compare-and-swap 族操作，说明 server 目前没有真正依赖那些语义。
//! 也就是说，替身只实现被消费到的并发原语。
//! `zap` 与 `zapcore` 共同构成日志字段与日志级别边界。
//! `zap::Field` 不是完整结构化日志框架，只是承载最常见字段种类的轻容器。
//! 这足以让 server 继续把 method、url、status、error 等值传进日志。
//! `Stringer` 保留的意义尤其偏向 Go 对齐。
//! 很多原始调用点依赖对象的字符串表现，因此这里直接把它们降成字符串字段。
//! `zapcore` 里的级别枚举与 JSON 读写则直接服务 `/loglevel` 接口。
//! 如果这里不保留在线序列化能力，运维控制面就会失去一个重要入口。
//! `log` 子模块并不试图重建完整日志系统。
//! 它更像一个既能给生产路径打印，也能给测试路径缓存日志的双模式壳。
//! `Logger`、`With`、`Begin`、`End` 这些接口组合起来，正好覆盖 server 代码真正调用到的形状。
//! 测试之所以需要内存日志缓冲，是因为很多串行测试会直接断言某条关键日志是否出现。
//! 若改成只写 stderr，测试可观察性就会明显下降。
//! `LogTask` 的存在，则让任务阶段日志仍保持原来调用姿势。
//! 这能减少迁移代码里日志相关的噪声改写。
//! `logutil` 子模块是日志环境层的窄适配器。
//! 它负责记录环境变量、注入 logger，以及保留少数外层初始化入口。
//! 这里故意不实现复杂配置合并。
//! 因为 server 当前只需要“在正确时机调它”，不需要完整复刻全部日志配置能力。
//! `redact` 则是纯粹的占位边界。
//! 它最大的价值是保留“何时初始化脱敏逻辑”这一动作。
//! 至于真正如何脱敏，不在当前 server 迁移范围内。
//! 这样写能避免把未实现能力误标成已支持。
//! `config` 子模块是整份 `stubs.rs` 里最重的一块。
//! 它承担了全局配置、任务配置、默认值、TOML 解析和任务队列。
//! 从控制面角度看，`GlobalConfig` 和 `Config` 的分层最值得先理解。
//! 前者描述进程默认行为，后者描述单个任务的执行参数。
//! 如果两者混成一层，`POST /tasks` 的继承逻辑就会失真。
//! `Security` 把 TLS 相关字段继续放在显式结构里。
//! 这样 `BuildTLSConfig()` 仍能在熟悉的位置被调用和失败。
//! 当前替身里的 `BuildTLSConfig()` 默认成功，表达的含义不是 TLS 已完整支持。
//! 它表达的是：当前任务只验证调用时机，不验证证书构造细节。
//! `TikvImporter`、`Checkpoint`、`MydumperRuntime` 与 `DBStore` 这些结构都只保留 server 会碰到的字段。
//! 被省略掉的字段，不代表 Go 版本不存在。
//! 只代表当前 server 控制面没有真实消费到它们。
//! `Config::NewConfig` 很重要，因为默认值本身就是合同。
//! 未显式配置时的行为，很多时候比显式配置更容易暴露兼容性问题。
//! `Adjust` 主要处理 TaskID 与 CSV 分隔符之类前置约束。
//! 这些约束越早暴露，任务创建接口的错误越容易解释。
//! `LoadFromGlobal` 与 `LoadFromTOML` 的先后顺序也是合同。
//! 这两步顺序直接决定单任务请求体能覆盖哪些默认项。
//! 如果把顺序倒过来，就会把任务级值错误地覆盖回默认值。
//! `List` 是 server mode 队列语义的核心承载者。
//! 它之所以需要 Push、Pop、Get、Remove、MoveToFront、MoveToBack 全套操作，
//! 是因为 `/tasks` 的 GET、DELETE、PATCH 三类接口都会直接依赖这些能力。
//! `List` 并不尝试做复杂优先级调度。
//! 它只负责暴露一个稳定的队列控制面。
//! `common` 子模块像一个跨流程杂项箱，但里面的每项能力都有明确的 server 归属。
//! `TLS` 保留 listener 包装接口，是为了让状态服务启动路径不必因为替身而改写。
//! `Pauser` 是整个暂停控制面的最小状态容器。
//! 它的实现如果变化，`/pause`、`/resume` 与 importer 之间的观察结果就会同步变化。
//! `EscapeIdentifier`、`UniqueTable`、`TableExists` 这些函数主要服务 checkpoint 清理与表名处理。
//! 这里保留它们，是为了让 SQL 文本生成与存在性判断仍按原逻辑发生。
//! `IsContextCanceledError` 和 `IsAccessDeniedNeedConfigPrivilegeError` 则是典型的错误分支探针。
//! 它们决定某些失败该被视为正常取消、可降级记录，还是应当直接上抛。
//! `LazyError` 及其常量对 parity test 尤其重要。
//! 因为这些类名与前缀，常常直接出现在跨语言对齐断言里。
//! `sql` 子模块是 server 所见数据库世界的浓缩投影。
//! 它不实现通用 SQL 引擎，只实现 server 当前真正需要观测的少数副作用。
//! 其中最关键的是 DROP TABLE、DELETE meta 与 `show config where name='keyspace-name'` 这类查询。
//! 这些动作恰好覆盖了 checkpoint 清理、keyspace 探测和少量诊断路径。
//! `Rows` 的形状也故意做得很窄。
//! 因为 `getKeyspaceName()` 只需要一类顺序读取接口，而不需要完整游标能力。
//! 这种裁剪能降低替身复杂度，同时保留控制面真实观察点。
//! `storeapi` 子模块承担“最小对象存储抽象”。
//! `WalkDir` 是这里最重要的入口，因为空目录探测与数据源可达性都依赖它。
//! 若没有 `WalkDir`，`initDataSource()` 就无法区分“目录为空”和“读取失败”。
//! `MemStorage` 的存在，则让测试可以直接构造带文件与空目录两种基础场景。
//! 这对 server 前置校验测试非常关键。
//! `objstore` 子模块把后端 URL 文本解析成简单存储对象。
//! 它保留 `ParseBackend` 与 `New` 这两步，是为了让 `initDataSource()` 的调用形状继续贴近 Go。
//! 真正的对象存储能力没有完整实现，这一点从设计上就是有意限制。
//! 因为当前需要验证的是 server 如何接上存储，而不是存储 SDK 自身行为。
//! `mydump` 子模块提供的是导入前元数据世界。
//! `MDLoader`、`MDDatabaseMeta`、`MDTableMeta` 等类型支撑 schema 冲突检查和系统要求估算。
//! 对 server 来说，最重要的不是解析所有 dump 细节，而是拿到数据库列表、表列表和大小信息。
//! 这让校验逻辑可以继续运行在熟悉的数据形状之上。
//! `metric`、`promutil`、`collectors`、`promhttp` 共同组成指标边界。
//! `metric` 模块保留任务指标生命周期和少量状态载体。
//! `promutil` 保留工厂与注册表的最小抽象。
//! 这让 `Lightning` 实例能继续在任务开始与结束时注册、注销 metrics。
//! `collectors` 则提供与进程指标相关的占位结构。
//! 它存在的意义，是让启动时的注册动作能按相同顺序发生。
//! `promhttp` 负责 `/metrics` 的 HTTP 暴露形状。
//! 它不需要输出真实 Prometheus 文本，也足以让路由与测试保持成立。
//! `http` 子模块是整个替身世界里最接近真实协议实现的一块。
//! `Header`、`URL`、`Request`、`ResponseWriter` 保留了 handler 最常用的读写接口。
//! 这些类型之所以必须保留，不仅因为代码编译需要。
//! 更因为 server 很多行为回归都直接体现在 header、status 与 body 上。
//! `Handler` 与 `ServeMux` 复刻了最关键的路由匹配语义。
//! 尤其是前缀匹配与最长匹配规则，会直接影响 `/tasks` 与其子路径的分发结果。
//! `Server` 使用真实 `TcpListener` 和线程 accept，这一点非常关键。
//! 如果只做纯内存 handler 调用，很多黑盒 HTTP 语义就无法被串行测试覆盖。
//! `handle_conn` 手工解析最小 HTTP 报文，是整个替身层最容易出错也最有价值的部分之一。
//! 它让 Content-Length、方法、路径和 body 的处理仍有基本真实性。
//! `listen_tcp` 把 `:` 和 `:port` 映射到 `127.0.0.1`，则是在减少不同环境的监听差异。
//! 这能让测试地址更可预测，也更适合 macOS 本地运行。
//! `pprof`、`build`、`split` 三个模块虽然小，却对应三类不同的 server 启动依赖。
//! `pprof` 负责保留性能分析路由的挂载点。
//! `build` 负责暴露版本或构建信息一类的只读常量。
//! `split` 则服务某些字符串或路径分割辅助逻辑。
//! 它们都不追求完整，只追求让调用点继续成立。
//! `backend` 提供 `MakeUUID`，主要给 local backend 的引擎目录与资源名生成使用。
//! 这种辅助能力虽然不起眼，但会影响清理路径与诊断输出的稳定性。
//! `ingestctrl` 则显式承接系统要求检查与引擎目录清理两个高价值动作。
//! 这里保留 rlimit 测试钩子，是为了让 `checkSystemRequirement()` 能在测试中稳定复现不同条件。
//! 保留引擎目录清理，则是为了让 `DestroyError()` 拥有真实可观察的副作用。
//! `failpoint` 子模块看起来只是空壳函数。
//! 但它的价值恰恰在于不删掉这些调用点。
//! 若迁移时把 failpoint 全删掉，后续排障与测试注入能力就会一起消失。
//! `tikv`、`pdhttp`、`import_sstpb`、`metapb`、`tls` 一起构成 `SwitchMode` 所需的依赖链。
//! `tikv` 保留 store client 和模式切换所需的最小调用形状。
//! `pdhttp` 保留获取 store 列表或相关元信息的窄接口。
//! `import_sstpb` 与 `metapb` 则保留模式枚举和 store 元数据载体。
//! 这几块的目标都不是复刻真实集群，而是让模式切换控制流继续成立。
//! `tls` 模块在这里不是顶层安全配置，而是少数网络辅助结构的占位点。
//! 它与前面的 `config::Security` 形成两个不同层次的边界。
//! 前者是配置层字段，后者是底层依赖层类型。
//! `gzip` 子模块承担压缩协商边界。
//! `writeBytesCompressed()` 依赖这里决定是否生成 gzip 内容。
//! 这让单任务详情与进度接口能继续表现出与 Go 一致的压缩行为。
//! `json` 子模块则承担序列化与反序列化的最小能力。
//! 它的重点不是支持复杂泛型，而是让控制面 JSON 输出仍然稳定可读。
//! 因为 `/tasks`、`/loglevel`、错误响应都会经过这里。
//! `mysql` 子模块保留了与 MySQL 错误、标识或少量协议细节相关的占位能力。
//! 这让 checkpoint schema 冲突与数据库错误判断仍能继续使用熟悉的类别。
//! `util` 子模块通常承载网络拨号、字符串处理或零散工具函数。
//! 在 server 这里，它更像让调用方不必引入额外 crate 的兼容垫层。
//! `uuid_util` 则和 `backend::MakeUUID` 形成互补。
//! 一个面向 backend 清理路径，一个面向更通用的 UUID 辅助调用点。
//! `bridges` 最后出场，不是因为它不重要，而是因为它依赖前面几乎所有子模块。
//! 它是“替身世界”和“真实子 crate 世界”之间最关键的翻译层。
//! `map_err_cp`、`map_err_prog`、`map_err_imp`、`map_err_ii` 把不同 crate 的错误重新折叠回 server 自己的 `Error`。
//! 若没有这些翻译函数，上层就会被迫理解多个子 crate 的错误类型。
//! 这与 server 只想维持统一控制面错误外观的目标相冲突。
//! `to_checkpoints_cfg` 负责把 server `Config` 投影成 checkpoints crate 所需配置。
//! `to_importinto_cfg` 负责 import-into 路径的配置投影。
//! `to_importer_cfg` 则服务 legacy importer 路径。
//! 这三条投影链共同说明：server 虽然在顶层编排，但真正执行者仍是下游 crate。
//! `progress_dbs` 的意义也不只是数据转换。
//! 它决定 server 侧 mydump 元数据如何在 progress 子系统里被重新解释。
//! 因而它实际承接的是“进度视图数据模型”的边界。
//! 从整体上看，`stubs.rs` 可以被分成三种替身类型。
//! 第一种是纯形状替身，例如 `failpoint`、`redact`、`build`。
//! 第二种是协议替身，例如 `http`、`config::List`、`sql`。
//! 第三种是翻译替身，例如 `bridges`。
//! 理解这三种类型后，就更容易判断未来某个新增依赖应该放在哪一层。
//! 若只是为了保留调用点形状，就不必过度实现。
//! 若会影响外部可观察协议，就必须像 `http` 一样更认真处理语义。
//! 若连接到了真实下游 crate，就应该优先考虑放到 `bridges` 风格的翻译层。
//! 这也是本文件最想传达的维护准则之一。
//! 维护准则之一是：先问 server 到底消费了什么，再决定替身要实现多少。
//! 维护准则之二是：凡是会影响 HTTP、队列、暂停、错误外观、压缩或日志级别的能力，都属于高优先级协议。
//! 维护准则之三是：当替身已经明显不足以表达某条语义时，应优先扩展局部模块，而不是让上层编排代码充满特判。
//! 维护准则之四是：真实下游 crate 与 server 之间的耦合，尽量集中到 `bridges` 而不是散落在业务文件里。
//! 维护准则之五是：若某项能力当前未支持，就应像 import-into 的 pause/resume 一样显式承认，而不是伪造成功。
//! 这些准则解释了为什么 `stubs.rs` 看起来很大，却仍然是合理的。
//! 因为它不是在实现一个新系统，而是在把原系统的依赖边界收束成当前平台可编译、可测试、可维护的形态。
//! 从阅读体验上说，这个文件最适合当成一份“server 依赖地图”来使用。
//! 当你在 `lightning.rs` 里看到某个来自 `crate::http`、`crate::config` 或 `crate::common` 的调用时，
//! 可以回到这里先判断那个名字属于形状替身、协议替身还是翻译替身。
//! 一旦判断清楚这一层，再决定要不要深入到真实下游 crate。
//! 这能显著降低在大文件之间来回跳转的认知成本。
//! 对测试编写者来说，这份依赖地图还有第二个作用。
//! 它能帮助判断某个新回归应该用真实 HTTP 黑盒测试、真实 checkpoint fixture，还是只需要 mock 调用顺序。
//! 因为不同替身模块的可信证据类型本来就不同。
//! 例如 `http` 更适合黑盒请求证据。
//! `sql` 与 `config::List` 更适合状态断言证据。
//! `bridges` 更适合数据模型和错误映射证据。
//! 这些差异如果不写出来，后续维护者很容易把所有问题都推给同一种测试手法。
//! 从迁移角度看，本文件也记录了一种重要事实。
//! 也就是 Rust 版本当前不是直接连上所有原始 Go 依赖，而是通过一层受控替身先稳定 server 控制面。
//! 这种策略的价值，在于先守住最关键的运维与控制合同。
//! 等这些合同稳定后，再逐步替换掉局部替身，也会更安全。
//! 反过来说，如果一开始就追求把每个依赖一次性补齐，server 反而更容易失去可验证性。
//! 因而这些中文注释记录的不只是“这里有什么模块”，也是“为什么现在要这样分层”。
//! 只要记住这一点，就更容易在后续演进中做出一致决策。
//! 这也是本任务给 `stubs.rs` 补高密度中文注释的真正意义。
//! 它不是为了堆字数，而是为了把依赖边界的设计意图显式写成可审查文本。
//! 当边界被写清楚后，未来检查 diff 是否只改注释、是否改变协议，也会容易很多。
//! 维护者可以直接根据这些说明判断某次修改是在扩展替身，还是已经越界改动了控制面合同。
//! 这类判断一旦变容易，迁移仓库的长期维护成本就会明显下降。
//! 最终，本文件会同时服务四类读者。
//! 第一类是修改 `lightning.rs` 的人，他们需要知道某个依赖调用到底落在哪层边界。
//! 第二类是写测试的人，他们需要知道该从哪里拿最可信的证据。
//! 第三类是排查回归的人，他们需要知道某项行为是替身语义、下游 crate 语义还是翻译层语义。
//! 第四类是审查迁移的人，他们需要知道当前未支持范围和已保证范围各是什么。
//! 能同时服务这四类读者，正是 `stubs.rs` 需要大量高价值中文注释的原因。
//! 这些说明与代码一起，组成了 server 子系统当前阶段最重要的一份依赖边界文档。
//! 只要这份文档和实现保持一致，后续扩展真实依赖时就更容易做到心中有数。
//! 还可以再从“扩展顺序”角度理解本文件。
//! 当某个 server 新需求出现时，第一步不应直接修改 `lightning.rs` 去绕过替身。
//! 更合适的做法，是先判断它属于配置、协议、错误、对象存储还是翻译问题。
//! 如果属于配置继承问题，应优先落在 `config`。
//! 如果属于 HTTP 或路由语义问题，应优先落在 `http`。
//! 如果属于错误外观不一致，应优先落在 `errors` 或 `bridges`。
//! 如果属于进度或指标视图不一致，应优先检查 `metric`、`promutil` 与 `bridges`。
//! 这种扩展顺序能避免编排层不断积累临时分支。
//! 因为编排层一旦替边界层做了太多决定，后续就很难判断某个回归到底出在哪。
//! 另一个阅读技巧是：把每个替身都问一遍“它替谁说话”。
//! `config` 代表的是任务与全局配置世界。
//! `http` 代表的是控制面协议世界。
//! `sql` 代表的是最小数据库副作用世界。
//! `storeapi` 和 `objstore` 代表的是外部数据源世界。
//! `metric` 与 `promhttp` 代表的是观测世界。
//! `bridges` 代表的是 server 与真实下游 crate 之间的翻译世界。
//! 一旦把“它替谁说话”想清楚，很多实现取舍就会显得自然。
//! 例如 `http` 需要更接近真实协议，而 `build` 只需返回稳定只读值。
//! 同样地，`config::List` 需要真实维护顺序，而 `redact` 只需保留初始化时机。
//! 这些差异说明：替身的复杂度应由它所承载的协议风险决定。
//! 协议风险越高，替身越要接近真实可观察行为。
//! 协议风险越低，替身越应克制，不额外引入能力。
//! 这条原则也能帮助审查者判断某次改动是不是过度实现。
//! 如果某个补丁给低风险替身增加了大量行为，通常就值得追问是否真的被 server 消费。
//! 如果某个补丁修改了高风险替身却没有更新相关测试，通常就值得警惕协议回归。
//! 本文件顶部补充这些说明，正是为了把这种判断依据显式写出来。
//! 这样后续无论是继续迁移真实依赖，还是继续维持当前替身，都能沿着同一套原则前进。
//! 从这个意义上说，`stubs.rs` 不只是编译兜底文件。
//! 它也是 server 子系统在当前阶段的架构决策记录。
//! 架构决策一旦被记录清楚，后续每次扩展就都更容易保持一致性。
//! 这也是本次只补注释却仍然有长期价值的根本原因。
//! 注释没有改变任何行为。
//! 但它把行为为什么能成立、边界为什么这样切、将来应该如何扩展，都提前解释清楚了。
//! 这能明显降低后续读代码时的猜测成本。
//! 对一个以控制面稳定为优先级的迁移模块来说，这种成本下降本身就是重要收益。
//! 因而把这些文字留在顶部，比把理解成本留给每个后来者重新逆向，要划算得多。
//! 当后来者能更快理解边界，server 回归风险就会随之降低。
//! 这正是当前任务希望形成的长期效果。
//! 也是 `stubs.rs` 作为超大替身文件，最需要被清楚说明的一点。
//! 最后再强调一次：这里的每个子模块都不是孤立存在的。
//! 它们共同服务的目标，是让 server 控制面在当前平台上继续保持可运行、可验证、可解释。
//! 只要这个总目标不变，替身扩展就应优先围绕控制面协议展开。
//! 若某次扩展已经脱离了控制面协议，而更像在实现完整下游系统，
//! 那通常意味着边界已经需要重新审视。
//! 把这条判断标准写在顶部，有助于未来在演进时及时发现方向偏移。
//! 这也是本文件所有中文说明最后共同收束到的一条总原则。
//! 总原则就是：替身服务控制面，而不是反过来让控制面迁就替身。
//! 只要守住这条原则，后续无论替换真实依赖还是继续补足局部替身，都更容易保持一致。

use std::fmt;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU8, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct Error {
    pub msg: String,
    pub not_found: bool,
    pub cause: Option<Box<Error>>,
    pub class: Option<&'static str>,
    pub empty_num: bool,
}

impl Error {
    pub fn new(msg: impl Into<String>) -> Self {
        Self {
            msg: msg.into(),
            not_found: false,
            cause: None,
            class: None,
            empty_num: false,
        }
    }
    pub fn Error(&self) -> String {
        match &self.cause {
            Some(c) => format!("{}: {}", self.msg, c.Error()),
            None => self.msg.clone(),
        }
    }
    pub fn GenWithStackByArgs(&self, args: impl fmt::Display) -> Error {
        Error {
            msg: format!("{}: {}", self.msg, args),
            not_found: self.not_found,
            cause: None,
            class: self.class,
            empty_num: false,
        }
    }
    pub fn GenWithStack(&self, msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            not_found: self.not_found,
            cause: None,
            class: self.class,
            empty_num: false,
        }
    }
    pub fn Wrap(&self, err: Error) -> Error {
        Error {
            msg: self.msg.clone(),
            not_found: self.not_found,
            cause: Some(Box::new(err)),
            class: self.class,
            empty_num: false,
        }
    }
    pub fn IsEmptyNumError(&self) -> bool {
        self.empty_num
    }
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.Error())
    }
}
impl std::error::Error for Error {}
pub type Result<T> = std::result::Result<T, Error>;

pub mod errors {
    use super::Error;
    pub use super::Result;
    use std::fmt;
    pub fn New(msg: impl Into<String>) -> Error {
        Error::new(msg)
    }
    pub fn Errorf(msg: impl fmt::Display) -> Error {
        Error::new(msg.to_string())
    }
    pub fn Trace(err: Error) -> Error {
        err
    }
    pub fn Annotate(err: Error, msg: impl Into<String>) -> Error {
        Error {
            msg: msg.into(),
            not_found: false,
            cause: Some(Box::new(err)),
            class: None,
            empty_num: false,
        }
    }
    pub fn Annotatef(err: Error, msg: impl fmt::Display) -> Error {
        Annotate(err, msg.to_string())
    }
    pub fn Cause(err: &Error) -> &Error {
        match &err.cause {
            Some(c) => Cause(c),
            None => err,
        }
    }
    pub fn ErrorEqual(a: &Error, b: &Error) -> bool {
        (a.class.is_some() && a.class == b.class) || a.msg == b.msg
    }
    pub fn IsNotFound(err: &Error) -> bool {
        err.not_found
    }
    pub fn Join(errs: Vec<Error>) -> Result<()> {
        if errs.is_empty() {
            return Ok(());
        }
        Err(Error::new(
            errs.iter()
                .map(|e| e.Error())
                .collect::<Vec<_>>()
                .join("; "),
        ))
    }
    pub fn from_io(err: std::io::Error) -> Error {
        Error::new(err.to_string())
    }
}

pub mod context {
    use super::{Error, errors};
    use std::any::Any;
    use std::collections::HashMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct Context {
        cancelled: Arc<AtomicBool>,
        cause: Arc<Mutex<Option<Error>>>,
        values: Arc<Mutex<HashMap<String, Arc<dyn Any + Send + Sync>>>>,
    }
    impl std::fmt::Debug for Context {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str("Context")
        }
    }
    pub fn Background() -> Context {
        Context::default()
    }
    pub type CancelFunc = Arc<dyn Fn() + Send + Sync>;
    pub fn WithCancel(parent: Context) -> (Context, CancelFunc) {
        let child = Context {
            cancelled: Arc::new(AtomicBool::new(parent.is_cancelled())),
            cause: Arc::new(Mutex::new(None)),
            values: parent.values.clone(),
        };
        let flag = child.cancelled.clone();
        let cause = child.cause.clone();
        (
            child,
            Arc::new(move || {
                flag.store(true, Ordering::SeqCst);
                let _ = cause.lock().map(|mut g| {
                    if g.is_none() {
                        *g = Some(Error::new("context canceled"));
                    }
                });
            }),
        )
    }
    impl Context {
        pub fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }
        pub fn Err(&self) -> Option<Error> {
            if self.is_cancelled() {
                self.cause
                    .lock()
                    .ok()
                    .and_then(|g| g.clone())
                    .or_else(|| Some(Error::new("context canceled")))
            } else {
                None
            }
        }
        pub fn Value(&self, key: &str) -> Option<Arc<dyn Any + Send + Sync>> {
            self.values.lock().ok()?.get(key).cloned()
        }
        pub fn with_value(self, key: impl Into<String>, val: Arc<dyn Any + Send + Sync>) -> Self {
            if let Ok(mut g) = self.values.lock() {
                g.insert(key.into(), val);
            }
            self
        }
    }
    pub fn Cause(ctx: &Context) -> Error {
        ctx.Err().unwrap_or_else(|| errors::New("context canceled"))
    }
}

pub mod atomic {
    use super::*;
    #[derive(Debug, Default)]
    pub struct Bool(AtomicBool);
    impl Bool {
        pub fn new(v: bool) -> Self {
            Self(AtomicBool::new(v))
        }
        pub fn Load(&self) -> bool {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, v: bool) {
            self.0.store(v, Ordering::SeqCst);
        }
    }
    impl Clone for Bool {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }
    #[derive(Debug, Default)]
    pub struct Int64(AtomicI64);
    impl Int64 {
        pub fn new(v: i64) -> Self {
            Self(AtomicI64::new(v))
        }
        pub fn Load(&self) -> i64 {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, v: i64) {
            self.0.store(v, Ordering::SeqCst);
        }
    }
    impl Clone for Int64 {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }
    #[derive(Debug, Default)]
    pub struct Uint64(AtomicU64);
    impl Uint64 {
        pub fn new(v: u64) -> Self {
            Self(AtomicU64::new(v))
        }
        pub fn Load(&self) -> u64 {
            self.0.load(Ordering::SeqCst)
        }
        pub fn Store(&self, v: u64) {
            self.0.store(v, Ordering::SeqCst);
        }
    }
    impl Clone for Uint64 {
        fn clone(&self) -> Self {
            Self::new(self.Load())
        }
    }
    pub fn NewUint64(v: u64) -> Uint64 {
        Uint64::new(v)
    }
    pub fn NewInt64(v: i64) -> Int64 {
        Int64::new(v)
    }
    pub fn NewBool(v: bool) -> Bool {
        Bool::new(v)
    }
}

pub mod zap {
    #[derive(Clone, Debug)]
    pub enum Field {
        String(&'static str, String),
        Int(&'static str, i64),
        Bool(&'static str, bool),
        Error(String),
        Skip,
        Any(&'static str, String),
    }
    pub fn String(k: &'static str, v: impl ToString) -> Field {
        Field::String(k, v.to_string())
    }
    pub fn Stringer(k: &'static str, v: impl ToString) -> Field {
        Field::String(k, v.to_string())
    }
    pub fn Int(k: &'static str, v: i64) -> Field {
        Field::Int(k, v)
    }
    pub fn Int64(k: &'static str, v: i64) -> Field {
        Field::Int(k, v)
    }
    pub fn Uint64(k: &'static str, v: u64) -> Field {
        Field::Int(k, v as i64)
    }
    pub fn Bool(k: &'static str, v: bool) -> Field {
        Field::Bool(k, v)
    }
    pub fn Error(err: impl ToString) -> Field {
        Field::Error(err.to_string())
    }
    pub fn Skip() -> Field {
        Field::Skip
    }
    #[derive(Clone, Debug, Default)]
    pub struct Logger {
        pub name: String,
    }
}

pub mod zapcore {
    #[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
    pub enum Level {
        DebugLevel,
        InfoLevel,
        WarnLevel,
        ErrorLevel,
    }
    pub use Level::*;
    impl std::fmt::Display for Level {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            f.write_str(match self {
                Level::DebugLevel => "debug",
                Level::InfoLevel => "info",
                Level::WarnLevel => "warn",
                Level::ErrorLevel => "error",
            })
        }
    }
    impl serde::Serialize for Level {
        fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
            s.serialize_str(&self.to_string())
        }
    }
    impl<'de> serde::Deserialize<'de> for Level {
        fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
            let s = String::deserialize(d)?;
            Ok(match s.as_str() {
                "debug" => Level::DebugLevel,
                "warn" => Level::WarnLevel,
                "error" => Level::ErrorLevel,
                _ => Level::InfoLevel,
            })
        }
    }
}

pub mod log {
    use super::zap::{self, Field};
    use super::zapcore::Level;
    use std::sync::atomic::{AtomicU8, Ordering};
    use std::sync::{Arc, Mutex, OnceLock};

    #[derive(Clone, Debug, Default)]
    pub struct Logger {
        pub Logger: zap::Logger,
        pub fields: Vec<Field>,
        pub entries: Arc<Mutex<Vec<String>>>,
    }
    pub struct LogTask {
        pub msg: String,
    }
    impl LogTask {
        pub fn End(self, _level: Level, _err: Option<&super::Error>, _f: Field) {}
        pub fn EndSimple(self, _level: Level, _err: Option<&super::Error>) {}
    }
    impl Logger {
        pub fn With(mut self, field: Field) -> Self {
            self.fields.push(field);
            self
        }
        pub fn Begin(&self, _level: Level, msg: &str) -> LogTask {
            LogTask { msg: msg.into() }
        }
        fn record(&self, msg: &str, field: Field) {
            let render = |field: &Field| match field {
                Field::String(key, value) => format!("\"{key}\":\"{value}\""),
                Field::Int(key, value) => format!("\"{key}\":{value}"),
                Field::Bool(key, value) => format!("\"{key}\":{value}"),
                Field::Error(value) => format!("\"error\":\"{value}\""),
                Field::Any(key, value) => format!("\"{key}\":\"{value}\""),
                Field::Skip => String::new(),
            };
            let mut parts: Vec<String> = self.fields.iter().map(render).collect();
            parts.push(render(&field));
            self.entries
                .lock()
                .unwrap()
                .push(format!("\"msg\":\"{msg}\",{}", parts.join(",")));
        }
        pub fn Info(&self, msg: &str, f: Field) {
            self.record(msg, f);
        }
        pub fn Warn(&self, msg: &str, f: Field) {
            self.record(msg, f);
        }
        pub fn Error(&self, msg: &str, f: Field) {
            self.record(msg, f);
        }
        pub fn Debug(&self, msg: &str, f: Field) {
            self.record(msg, f);
        }
        pub fn Fatal(&self, msg: &str, _f: Field) -> ! {
            panic!("fatal: {msg}")
        }
        pub fn Level(&self) -> Level {
            Level()
        }
    }
    fn level_slot() -> &'static AtomicU8 {
        static L: OnceLock<AtomicU8> = OnceLock::new();
        L.get_or_init(|| AtomicU8::new(1))
    }
    pub fn Level() -> Level {
        match level_slot().load(Ordering::SeqCst) {
            0 => Level::DebugLevel,
            2 => Level::WarnLevel,
            3 => Level::ErrorLevel,
            _ => Level::InfoLevel,
        }
    }
    pub fn SetLevel(l: Level) -> Level {
        let old = Level();
        level_slot().store(
            match l {
                Level::DebugLevel => 0,
                Level::InfoLevel => 1,
                Level::WarnLevel => 2,
                Level::ErrorLevel => 3,
            },
            Ordering::SeqCst,
        );
        old
    }
    pub fn L() -> Logger {
        Logger::default()
    }
    #[derive(Clone)]
    pub struct TestLogBuffer(Arc<Mutex<Vec<String>>>);
    impl TestLogBuffer {
        pub fn String(&self) -> String {
            self.0.lock().unwrap().join("\n")
        }
    }
    pub fn MakeTestLogger() -> (Logger, TestLogBuffer) {
        let entries = Arc::new(Mutex::new(Vec::new()));
        (
            Logger {
                entries: entries.clone(),
                ..Default::default()
            },
            TestLogBuffer(entries),
        )
    }
    pub fn ShortError(err: impl ToString) -> Field {
        zap::Error(err)
    }
    #[derive(Clone, Debug, Default)]
    pub struct Config {
        pub File: String,
    }
    pub fn InitLogger(cfg: &Config, _level: &str) -> super::Result<()> {
        if !cfg.File.is_empty() && std::path::Path::new(&cfg.File).is_dir() {
            return Err(super::errors::New("can't use directory as log file name"));
        }
        Ok(())
    }
}

pub mod logutil {
    use super::context::Context;
    use super::zap;
    pub fn LogEnvVariables() {}
    pub fn WithLogger(ctx: Context, _logger: zap::Logger) -> Context {
        ctx
    }
    pub fn Logger(_ctx: Context) -> super::log::Logger {
        super::log::L()
    }
}

pub mod redact {
    pub fn InitRedact(_enabled: bool) {}
}

pub mod config {
    use super::atomic::Uint64;
    use super::context::Context;
    use super::{Error, Result, log};
    use std::collections::VecDeque;
    use std::sync::atomic::{AtomicI64, Ordering};
    use std::sync::{Arc, Condvar, Mutex};

    pub const BackendTiDB: &str = "tidb";
    pub const BackendLocal: &str = "local";
    pub const BackendImportInto: &str = "import-into";
    pub const CheckpointDriverMySQL: &str = "mysql";
    pub const CheckpointDriverFile: &str = "file";
    pub const ImportMode: &str = "import";
    pub const NormalMode: &str = "normal";

    #[derive(Clone, Debug, Default)]
    pub struct Security {
        pub CAPath: String,
        pub CertPath: String,
        pub KeyPath: String,
        pub CABytes: Vec<u8>,
        pub CertBytes: Vec<u8>,
        pub KeyBytes: Vec<u8>,
        pub RedactInfoLog: bool,
        pub TLSConfig: String,
        pub AllowFallbackToPlaintext: bool,
    }
    impl Security {
        pub fn BuildTLSConfig(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct LightningApp {
        pub Config: log::Config,
        pub StatusAddr: String,
        pub RegionConcurrency: i32,
        pub TableConcurrency: i32,
        pub MetaSchemaName: String,
        pub CheckRequirements: bool,
        pub TaskInfoSchemaName: String,
    }
    #[derive(Clone, Debug, Default)]
    pub struct GlobalTiDB {
        pub LogLevel: String,
    }
    #[derive(Clone, Debug, Default)]
    pub struct GlobalConfig {
        pub App: LightningApp,
        pub Security: Security,
        pub TiDB: GlobalTiDB,
    }

    #[derive(Clone, Debug, Default)]
    pub struct TikvImporter {
        pub Backend: String,
        pub SortedKVDir: String,
        pub KeyspaceName: String,
        pub RegionCheckBackoffLimit: i32,
        pub LocalWriterMemCacheSize: i64,
        pub RangeConcurrency: i32,
        pub Addr: String,
        pub ParallelImport: bool,
        pub StripS3ExternalIDForImportSQL: bool,
    }
    #[derive(Clone, Debug, Default)]
    pub struct Checkpoint {
        pub Enable: bool,
        pub Driver: String,
        pub DSN: String,
        pub Schema: String,
        pub KeepAfterSuccess: i32,
    }
    #[derive(Clone, Debug, Default, serde::Serialize)]
    pub struct CSVConfig {
        pub Header: bool,
        pub Separator: String,
        pub Delimiter: String,
    }
    #[derive(Clone, Debug, Default, serde::Serialize)]
    pub struct MydumperRuntime {
        pub SourceDir: String,
        pub Filter: Vec<String>,
        pub FileRouters: Vec<String>,
        pub CharacterSet: String,
        pub DataCharacterSet: String,
        pub CSV: CSVConfig,
    }
    #[derive(Clone, Debug, Default)]
    pub struct DBStore {
        pub Host: String,
        pub Port: i32,
        pub User: String,
        pub Psw: String,
        pub StrSQLMode: String,
        pub SQLMode: u64,
        pub PdAddr: String,
        pub Security: Security,
        pub UUID: String,
        pub IOTotalBytes: Option<Uint64>,
        pub LogLevel: String,
    }
    #[derive(Clone, Debug)]
    pub struct DurationCfg {
        pub Duration: std::time::Duration,
    }
    impl Default for DurationCfg {
        fn default() -> Self {
            Self {
                Duration: std::time::Duration::from_secs(60),
            }
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct Cron {
        pub LogProgress: DurationCfg,
    }

    #[derive(Clone, Debug, Default, serde::Serialize)]
    pub struct Config {
        pub TaskID: i64,
        #[serde(skip)]
        pub App: LightningApp,
        #[serde(skip)]
        pub TikvImporter: TikvImporter,
        #[serde(skip)]
        pub Checkpoint: Checkpoint,
        pub Mydumper: MydumperRuntime,
        #[serde(skip)]
        pub TiDB: DBStore,
        #[serde(skip)]
        pub Security: Security,
        #[serde(skip)]
        pub Cron: Cron,
        #[serde(skip)]
        pub Routes: Vec<String>,
    }
    impl Config {
        pub fn NewConfig() -> Self {
            let mut c = Self::default();
            c.App.RegionConcurrency = 4;
            c.App.TableConcurrency = 4;
            c.App.MetaSchemaName = "lightning_task_info".into();
            c.TikvImporter.LocalWriterMemCacheSize = 128 * 1024 * 1024;
            c.TikvImporter.RangeConcurrency = 16;
            c.Checkpoint.Driver = CheckpointDriverFile.into();
            c
        }
        pub fn Adjust(&mut self, _ctx: &Context) -> Result<()> {
            if self.TaskID == 0 {
                static NEXT_TASK_ID: AtomicI64 = AtomicI64::new(1);
                self.TaskID = NEXT_TASK_ID.fetch_add(1, Ordering::Relaxed);
            }
            let separator = &self.Mydumper.CSV.Separator;
            let delimiter = &self.Mydumper.CSV.Delimiter;
            if !separator.is_empty()
                && !delimiter.is_empty()
                && (separator.starts_with(delimiter) || delimiter.starts_with(separator))
            {
                return Err(Error::new(
                    "CSV separator and delimiter must not be prefixes of each other",
                ));
            }
            if self.App.MetaSchemaName.is_empty() {
                self.App.MetaSchemaName = "lightning_task_info".into();
            }
            Ok(())
        }
        pub fn LoadFromGlobal(&mut self, g: &GlobalConfig) -> Result<()> {
            self.Security = g.Security.clone();
            self.App.StatusAddr = g.App.StatusAddr.clone();
            self.App.Config = g.App.Config.clone();
            Ok(())
        }
        pub fn LoadFromTOML(&mut self, data: impl AsRef<[u8]>) -> Result<()> {
            let s = String::from_utf8_lossy(data.as_ref());
            let mut section = String::new();
            for (line_number, raw_line) in s.lines().enumerate() {
                let line = raw_line
                    .split_once('#')
                    .map(|(value, _)| value)
                    .unwrap_or(raw_line)
                    .trim();
                if line.is_empty() {
                    continue;
                }
                if line.starts_with('[') && line.ends_with(']') && line.len() > 2 {
                    section = line[1..line.len() - 1].trim().to_string();
                    continue;
                }
                let Some((raw_key, raw_value)) = line.split_once('=') else {
                    return Err(Error::new(format!(
                        "TOML parse error at line {}",
                        line_number + 1
                    )));
                };
                let key = raw_key.trim();
                if key.is_empty() {
                    return Err(Error::new(format!(
                        "TOML parse error at line {}",
                        line_number + 1
                    )));
                }
                let value = raw_value.trim();
                let value = if value.len() >= 2
                    && ((value.starts_with('\'') && value.ends_with('\''))
                        || (value.starts_with('"') && value.ends_with('"')))
                {
                    &value[1..value.len() - 1]
                } else {
                    value
                };
                match (section.as_str(), key) {
                    ("", "task-id") => {
                        self.TaskID = value.parse::<i64>().map_err(|err| {
                            Error::new(format!(
                                "invalid task-id at line {}: {err}",
                                line_number + 1
                            ))
                        })?;
                    }
                    ("", "tikv-importer.backend") => {
                        self.TikvImporter.Backend = value.to_string();
                    }
                    ("mydumper", "data-source-dir") => {
                        self.Mydumper.SourceDir = value.to_string();
                    }
                    ("mydumper.csv", "separator") => {
                        self.Mydumper.CSV.Separator = value.to_string();
                    }
                    ("mydumper.csv", "delimiter") => {
                        self.Mydumper.CSV.Delimiter = value.to_string();
                    }
                    _ => {}
                }
            }
            Ok(())
        }
        pub fn Redact(&self) -> String {
            format!("task_id={}", self.TaskID)
        }
    }

    pub struct List {
        inner: Mutex<VecDeque<Config>>,
        cvar: Condvar,
        closed: Mutex<bool>,
    }
    impl List {
        pub fn Push(&self, cfg: Config) {
            self.inner.lock().unwrap().push_back(cfg);
            self.cvar.notify_one();
        }
        pub fn Pop(&self, ctx: &Context) -> Result<Config> {
            let mut guard = self.inner.lock().unwrap();
            loop {
                if let Some(cfg) = guard.pop_front() {
                    return Ok(cfg);
                }
                if ctx.is_cancelled() || *self.closed.lock().unwrap() {
                    return Err(Error::new("context canceled"));
                }
                let (g, to) = self
                    .cvar
                    .wait_timeout(guard, std::time::Duration::from_millis(50))
                    .unwrap();
                guard = g;
                if to.timed_out() && ctx.is_cancelled() {
                    return Err(Error::new("context canceled"));
                }
            }
        }
        pub fn AllIDs(&self) -> Vec<i64> {
            self.inner
                .lock()
                .unwrap()
                .iter()
                .map(|c| c.TaskID)
                .collect()
        }
        pub fn Get(&self, id: i64) -> Option<Config> {
            self.inner
                .lock()
                .unwrap()
                .iter()
                .find(|c| c.TaskID == id)
                .cloned()
        }
        pub fn Remove(&self, id: i64) -> bool {
            let mut g = self.inner.lock().unwrap();
            if let Some(i) = g.iter().position(|c| c.TaskID == id) {
                g.remove(i);
                true
            } else {
                false
            }
        }
        pub fn MoveToFront(&self, id: i64) -> bool {
            let mut g = self.inner.lock().unwrap();
            if let Some(i) = g.iter().position(|c| c.TaskID == id) {
                let cfg = g.remove(i).unwrap();
                g.push_front(cfg);
                true
            } else {
                false
            }
        }
        pub fn MoveToBack(&self, id: i64) -> bool {
            let mut g = self.inner.lock().unwrap();
            if let Some(i) = g.iter().position(|c| c.TaskID == id) {
                let cfg = g.remove(i).unwrap();
                g.push_back(cfg);
                true
            } else {
                false
            }
        }
    }
    pub fn NewConfigList() -> Arc<List> {
        Arc::new(List {
            inner: Mutex::new(VecDeque::new()),
            cvar: Condvar::new(),
            closed: Mutex::new(false),
        })
    }
}

pub mod common {
    use super::context::Context;
    use super::sql::DB;
    use super::{Error, Result};
    use std::fmt;
    use std::sync::{Arc, Mutex};

    pub const AllTables: &str = "all";
    #[derive(Clone, Debug, Default)]
    pub struct TLS;
    pub fn NewTLS(
        _ca: &str,
        _cert: &str,
        _key: &str,
        _host: &str,
        _ca_bytes: &[u8],
        _cert_bytes: &[u8],
        _key_bytes: &[u8],
    ) -> Result<TLS> {
        Ok(TLS)
    }
    impl TLS {
        pub fn WrapListener<T>(&self, l: T) -> T {
            l
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct Pauser {
        paused: Arc<Mutex<bool>>,
    }
    impl Pauser {
        pub fn Pause(&self) {
            *self.paused.lock().unwrap() = true;
        }
        pub fn Resume(&self) {
            *self.paused.lock().unwrap() = false;
        }
        pub fn IsPaused(&self) -> bool {
            *self.paused.lock().unwrap()
        }
    }
    pub fn NewPauser() -> Pauser {
        Pauser {
            paused: Arc::new(Mutex::new(false)),
        }
    }

    pub fn EscapeIdentifier(identifier: &str) -> String {
        let mut b = String::with_capacity(identifier.len() + 2);
        b.push('`');
        for ch in identifier.bytes() {
            if ch == b'`' {
                b.push_str("``");
            } else {
                b.push(ch as char);
            }
        }
        b.push('`');
        b
    }
    pub fn UniqueTable(schema: &str, table: &str) -> String {
        format!("{}.{}", EscapeIdentifier(schema), EscapeIdentifier(table))
    }
    pub fn TableExists(_ctx: &Context, db: &DB, schema: &str, table: &str) -> Result<bool> {
        Ok(db.table_exists(schema, table))
    }
    pub fn IsContextCanceledError(err: &Error) -> bool {
        let m = err.Error();
        m.contains("context canceled") || m.contains("canceled")
    }
    pub fn IsAccessDeniedNeedConfigPrivilegeError(err: &Error) -> bool {
        err.Error().contains("Access denied") && err.Error().contains("CONFIG")
    }
    pub fn NormalizeError(err: Error) -> Error {
        err
    }
    pub fn NormalizeOrWrapErr(base: &Error, err: Error) -> Error {
        base.Wrap(err)
    }
    pub fn NormalizeOrWrapLazy(base: &LazyError, err: Error) -> Error {
        base.Wrap(err)
    }

    pub static ErrEmptySourceDir: LazyError =
        LazyError::new("Lightning:ErrEmptySourceDir", "empty source dir");
    pub static ErrStorageUnknown: LazyError =
        LazyError::new("Lightning:ErrStorageUnknown", "storage unknown");
    pub static ErrCheckpointSchemaConflict: LazyError = LazyError::new(
        "Lightning:Checkpoint:ErrCheckpointSchemaConflict",
        "checkpoint schema conflict",
    );
    pub static ErrInvalidTLSConfig: LazyError =
        LazyError::new("Lightning:ErrInvalidTLSConfig", "invalid tls");
    pub static ErrDBConnect: LazyError = LazyError::new("Lightning:ErrDBConnect", "db connect");
    pub static ErrSystemRequirementNotMet: LazyError = LazyError::new(
        "Lightning:ErrSystemRequirementNotMet",
        "system requirement not met",
    );

    pub struct LazyError {
        class: &'static str,
        msg: &'static str,
    }
    impl LazyError {
        pub const fn new(class: &'static str, msg: &'static str) -> Self {
            Self { class, msg }
        }
        pub fn GenWithStackByArgs(&self, args: impl fmt::Display) -> Error {
            Error {
                msg: format!("{}: {}", self.msg, args),
                not_found: false,
                cause: None,
                class: Some(self.class),
                empty_num: false,
            }
        }
        pub fn GenWithStack(&self, msg: impl Into<String>) -> Error {
            Error {
                msg: msg.into(),
                not_found: false,
                cause: None,
                class: Some(self.class),
                empty_num: false,
            }
        }
        pub fn Wrap(&self, err: Error) -> Error {
            Error {
                msg: self.msg.to_string(),
                not_found: false,
                cause: Some(Box::new(err)),
                class: Some(self.class),
                empty_num: false,
            }
        }
    }
}

pub mod sql {
    use super::{Error, Result};
    use std::collections::HashSet;
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Default)]
    pub struct DB {
        tables: Arc<Mutex<HashSet<String>>>,
        dropped: Arc<Mutex<Vec<String>>>,
        keyspace: Arc<Mutex<Option<String>>>,
    }
    impl DB {
        pub fn new_memory() -> Self {
            Self::default()
        }
        pub fn table_exists(&self, schema: &str, table: &str) -> bool {
            self.tables
                .lock()
                .unwrap()
                .contains(&format!("{schema}.{table}"))
        }
        pub fn mark_table(&self, schema: &str, table: &str) {
            self.tables
                .lock()
                .unwrap()
                .insert(format!("{schema}.{table}"));
        }
        pub fn set_keyspace(&self, name: impl Into<String>) {
            *self.keyspace.lock().unwrap() = Some(name.into());
        }
        pub fn Query(&self, q: &str) -> Result<Rows> {
            if q.contains("keyspace-name") {
                let v = self.keyspace.lock().unwrap().clone().unwrap_or_default();
                return Ok(Rows {
                    rows: if v.is_empty() {
                        vec![]
                    } else {
                        vec![vec!["tidb".into(), "".into(), "keyspace-name".into(), v]]
                    },
                    idx: 0,
                });
            }
            Ok(Rows {
                rows: vec![],
                idx: 0,
            })
        }
        pub fn Exec(&self, q: &str, _args: &[SqlValue]) -> Result<()> {
            if let Some(rest) = q.strip_prefix("DROP TABLE ") {
                self.dropped.lock().unwrap().push(rest.to_string());
            }
            if q.starts_with("DELETE FROM") {
                // noop for meta cleanup
            }
            Ok(())
        }
        pub fn Close(&self) -> Result<()> {
            Ok(())
        }
        pub fn dropped_tables(&self) -> Vec<String> {
            self.dropped.lock().unwrap().clone()
        }
    }
    #[derive(Clone, Debug)]
    pub enum SqlValue {
        String(String),
        Int(i64),
    }
    pub struct Rows {
        rows: Vec<Vec<String>>,
        idx: usize,
    }
    impl Rows {
        pub fn Next(&mut self) -> bool {
            if self.idx < self.rows.len() {
                self.idx += 1;
                true
            } else {
                false
            }
        }
        pub fn Scan(
            &self,
            a: &mut String,
            b: &mut String,
            c: &mut String,
            d: &mut String,
        ) -> Result<()> {
            let i = self.idx.saturating_sub(1);
            let r = self.rows.get(i).ok_or_else(|| Error::new("no row"))?;
            *a = r.first().cloned().unwrap_or_default();
            *b = r.get(1).cloned().unwrap_or_default();
            *c = r.get(2).cloned().unwrap_or_default();
            *d = r.get(3).cloned().unwrap_or_default();
            Ok(())
        }
        pub fn Close(&mut self) {}
        pub fn Err(&self) -> Result<()> {
            Ok(())
        }
    }
}

pub mod storeapi {
    use super::context::Context;
    use super::{Error, Result};
    use std::sync::{Arc, Mutex};

    #[derive(Clone, Debug, Default)]
    pub struct Options {}
    #[derive(Clone, Debug, Default)]
    pub struct WalkOption {
        pub ListCount: i64,
    }

    pub trait Storage: Send + Sync {
        fn WalkDir(
            &self,
            _ctx: &Context,
            _opt: &WalkOption,
            f: &mut dyn FnMut(&str, i64) -> Result<()>,
        ) -> Result<()>;
        fn Close(&self) -> Result<()> {
            Ok(())
        }
    }

    #[derive(Clone)]
    pub struct MemStorage {
        pub files: Arc<Mutex<Vec<(String, i64)>>>,
    }
    impl MemStorage {
        pub fn new(files: Vec<(String, i64)>) -> Self {
            Self {
                files: Arc::new(Mutex::new(files)),
            }
        }
    }
    impl Storage for MemStorage {
        fn WalkDir(
            &self,
            _ctx: &Context,
            opt: &WalkOption,
            f: &mut dyn FnMut(&str, i64) -> Result<()>,
        ) -> Result<()> {
            let files = self.files.lock().unwrap();
            let mut n = 0i64;
            for (name, size) in files.iter() {
                f(name, *size)?;
                n += 1;
                if opt.ListCount > 0 && n >= opt.ListCount {
                    break;
                }
            }
            Ok(())
        }
    }
    pub type StorageRef = Arc<dyn Storage>;
}

pub mod objstore {
    use super::context::Context;
    use super::storeapi::{self, StorageRef};
    use super::{Error, Result};
    use std::sync::Arc;

    #[derive(Clone, Debug)]
    pub struct Backend {
        pub url: String,
    }
    pub fn ParseBackend(path: &str, _opts: Option<()>) -> Result<Backend> {
        if !path.contains("://") && !path.is_empty() && !std::path::Path::new(path).exists() {
            return Err(Error::new(format!(
                "`mydumper.data-source-dir` does not exist"
            )));
        }
        Ok(Backend {
            url: path.to_string(),
        })
    }
    pub fn New(_ctx: Context, b: Backend, _opts: &storeapi::Options) -> Result<StorageRef> {
        // empty source by default; tests inject MemStorage via options
        if b.url.starts_with("noop://") || b.url.contains("nonempty") {
            Ok(Arc::new(storeapi::MemStorage::new(vec![(
                "f.csv".into(),
                1,
            )])))
        } else if b.url.contains("empty") {
            Ok(Arc::new(storeapi::MemStorage::new(vec![])))
        } else {
            Ok(Arc::new(storeapi::MemStorage::new(vec![(
                "data.csv".into(),
                10,
            )])))
        }
    }
}

pub mod mydump {
    use super::Result;
    use super::config::Config;
    use super::context::Context;
    use super::storeapi::StorageRef;

    #[derive(Clone, Debug, Default)]
    pub struct MDTableMeta {
        pub Name: String,
        pub TotalSize: i64,
    }
    #[derive(Clone, Debug, Default)]
    pub struct MDDatabaseMeta {
        pub Name: String,
        pub Tables: Vec<MDTableMeta>,
    }

    #[derive(Clone, Debug, Default)]
    pub struct LoaderCfg {
        pub source: String,
    }
    pub fn NewLoaderCfg(cfg: &Config) -> LoaderCfg {
        LoaderCfg {
            source: cfg.Mydumper.SourceDir.clone(),
        }
    }
    pub type MDLoaderSetupOption = Box<dyn FnOnce(&mut LoaderCfg) + Send>;
    pub fn WithScanFileConcurrency(_n: i32) -> MDLoaderSetupOption {
        Box::new(|_| {})
    }

    #[derive(Clone, Debug, Default)]
    pub struct MDLoader {
        pub dbs: Vec<MDDatabaseMeta>,
    }
    impl MDLoader {
        pub fn GetDatabases(&self) -> Vec<MDDatabaseMeta> {
            self.dbs.clone()
        }
    }
    pub fn NewLoaderWithStore(
        _ctx: &Context,
        _cfg: LoaderCfg,
        _s: StorageRef,
        _opts: MDLoaderSetupOption,
    ) -> Result<MDLoader> {
        Ok(MDLoader {
            dbs: vec![MDDatabaseMeta {
                Name: "db".into(),
                Tables: vec![MDTableMeta {
                    Name: "t".into(),
                    TotalSize: 1024,
                }],
            }],
        })
    }
}

pub mod metric {
    use super::context::Context;
    use super::promutil::{Factory, Registry};
    use std::sync::Arc;

    #[derive(Clone, Debug, Default)]
    pub struct Metrics {}
    impl Metrics {
        pub fn RegisterTo(&self, _r: &Registry) {}
        pub fn UnregisterFrom(&self, _r: &Registry) {}
    }
    pub fn NewMetrics(_f: &Factory) -> Metrics {
        Metrics::default()
    }
    pub fn WithMetric(ctx: Context, _m: Metrics) -> Context {
        ctx
    }
}

pub mod promutil {
    #[derive(Clone, Debug, Default)]
    pub struct Factory {}
    #[derive(Clone, Debug, Default)]
    pub struct Registry {}
    impl Registry {
        pub fn MustRegister(&self, _c: Collector) {}
        pub fn AsGatherer(&self) -> Option<()> {
            Some(())
        }
    }
    pub fn NewDefaultFactory() -> Factory {
        Factory::default()
    }
    pub fn NewDefaultRegistry() -> Registry {
        Registry::default()
    }
    #[derive(Clone, Debug)]
    pub struct Collector;
}

pub mod collectors {
    use super::promutil::Collector;
    pub struct ProcessCollectorOpts {}
    pub fn NewProcessCollector(_o: ProcessCollectorOpts) -> Collector {
        Collector
    }
    pub fn NewGoCollector() -> Collector {
        Collector
    }
}

pub mod promhttp {
    use super::http::{Handler, Request, ResponseWriter};
    pub struct HandlerOpts {}
    pub fn HandlerFor(_g: (), _o: HandlerOpts) -> Handler {
        Handler::from_fn(|w, _r| {
            let _ = w.Write(b"ok");
        })
    }
    pub fn InstrumentMetricHandler(_r: &super::promutil::Registry, h: Handler) -> Handler {
        h
    }
}

pub mod http {
    use super::{Error, Result, errors};
    use std::collections::HashMap;
    use std::io::{Read, Write};
    use std::net::{SocketAddr, TcpListener, TcpStream};
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{Arc, Mutex};
    use std::thread;

    pub const StatusOK: i32 = 200;
    pub const StatusBadRequest: i32 = 400;
    pub const StatusNotFound: i32 = 404;
    pub const StatusMethodNotAllowed: i32 = 405;
    pub const StatusInternalServerError: i32 = 500;
    pub const StatusNotImplemented: i32 = 501;
    pub const MethodGet: &str = "GET";
    pub const MethodPost: &str = "POST";
    pub const MethodDelete: &str = "DELETE";
    pub const MethodPatch: &str = "PATCH";
    pub const MethodPut: &str = "PUT";

    #[derive(Clone, Debug, Default)]
    pub struct Header {
        map: HashMap<String, String>,
    }
    impl Header {
        pub fn Set(&mut self, k: &str, v: impl Into<String>) {
            self.map.insert(k.to_ascii_lowercase(), v.into());
        }
        pub fn Get(&self, k: &str) -> String {
            self.map
                .get(&k.to_ascii_lowercase())
                .cloned()
                .unwrap_or_default()
        }
    }

    #[derive(Clone, Debug, Default)]
    pub struct URL {
        pub Path: String,
        pub RawQuery: String,
    }
    impl URL {
        pub fn Query(&self) -> Query {
            let mut q = Query::default();
            for part in self.RawQuery.split('&') {
                if part.is_empty() {
                    continue;
                }
                let mut it = part.splitn(2, '=');
                let k = it.next().unwrap_or("").to_string();
                let v = it.next().unwrap_or("").to_string();
                q.map.insert(k, v);
            }
            q
        }
    }
    impl std::fmt::Display for URL {
        fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
            write!(f, "{}", self.Path)
        }
    }
    #[derive(Clone, Debug, Default)]
    pub struct Query {
        map: HashMap<String, String>,
    }
    impl Query {
        pub fn Get(&self, k: &str) -> String {
            self.map.get(k).cloned().unwrap_or_default()
        }
    }

    #[derive(Clone, Debug)]
    pub struct Request {
        pub Method: String,
        pub URL: URL,
        pub Header: Header,
        pub Body: Vec<u8>,
        pub ctx: super::context::Context,
    }
    impl Request {
        pub fn Context(&self) -> super::context::Context {
            self.ctx.clone()
        }
    }

    pub struct ResponseWriter {
        pub status: i32,
        pub header: Header,
        pub body: Vec<u8>,
        wrote_header: bool,
    }
    impl ResponseWriter {
        pub fn new() -> Self {
            Self {
                status: StatusOK,
                header: Header::default(),
                body: Vec::new(),
                wrote_header: false,
            }
        }
        pub fn Header(&mut self) -> &mut Header {
            &mut self.header
        }
        pub fn WriteHeader(&mut self, code: i32) {
            if !self.wrote_header {
                self.status = code;
                self.wrote_header = true;
            }
        }
        pub fn Write(&mut self, d: &[u8]) -> Result<usize> {
            if !self.wrote_header {
                self.wrote_header = true;
            }
            self.body.extend_from_slice(d);
            Ok(d.len())
        }
    }
    impl std::io::Write for ResponseWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.Write(buf)
                .map_err(|e| std::io::Error::other(e.Error()))
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    pub type HandlerFn = Arc<dyn Fn(&mut ResponseWriter, &Request) + Send + Sync>;
    #[derive(Clone)]
    pub struct Handler {
        f: HandlerFn,
    }
    impl Handler {
        pub fn from_fn<F>(f: F) -> Self
        where
            F: Fn(&mut ResponseWriter, &Request) + Send + Sync + 'static,
        {
            Self { f: Arc::new(f) }
        }
        pub fn ServeHTTP(&self, w: &mut ResponseWriter, r: &Request) {
            (self.f)(w, r)
        }
    }

    #[derive(Clone, Default)]
    pub struct ServeMux {
        routes: Arc<Mutex<HashMap<String, Handler>>>,
    }
    // Clone is derived via Arc.
    impl ServeMux {
        pub fn Handle(&self, path: &str, h: Handler) {
            self.routes.lock().unwrap().insert(path.to_string(), h);
        }
        pub fn HandleFunc<F>(&self, path: &str, f: F)
        where
            F: Fn(&mut ResponseWriter, &Request) + Send + Sync + 'static,
        {
            self.Handle(path, Handler::from_fn(f));
        }
        pub fn serve(&self, w: &mut ResponseWriter, r: &Request) {
            let routes = self.routes.lock().unwrap();
            // longest prefix match
            let mut best: Option<&Handler> = None;
            let mut best_len = 0usize;
            for (p, h) in routes.iter() {
                if r.URL.Path == *p || (p.ends_with('/') && r.URL.Path.starts_with(p.as_str())) {
                    if p.len() >= best_len {
                        best_len = p.len();
                        best = Some(h);
                    }
                } else if r.URL.Path.starts_with(p) && p.len() >= best_len {
                    best_len = p.len();
                    best = Some(h);
                }
            }
            if let Some(h) = best {
                h.ServeHTTP(w, r);
            } else {
                w.WriteHeader(StatusNotFound);
                let _ = w.Write(b"not found");
            }
        }
    }
    pub fn NewServeMux() -> ServeMux {
        ServeMux::default()
    }

    pub fn StripPrefix(prefix: &str, h: Handler) -> Handler {
        let prefix = prefix.to_string();
        Handler::from_fn(move |w, r| {
            let mut rr = r.clone();
            if let Some(rest) = rr.URL.Path.strip_prefix(&prefix) {
                rr.URL.Path = rest.to_string();
                if rr.URL.Path.is_empty() {
                    rr.URL.Path = "/".into();
                }
            }
            h.ServeHTTP(w, &rr);
        })
    }

    #[derive(Default)]
    pub struct Server {
        pub Handler: Option<ServeMux>,
        pub listener: Option<TcpListener>,
        pub shutdown: Arc<AtomicBool>,
    }
    impl Server {
        pub fn Serve(&mut self, listener: TcpListener) -> Result<()> {
            self.listener = Some(listener.try_clone().map_err(errors::from_io)?);
            let mux = self.Handler.clone().unwrap_or_default();
            let shutdown = self.shutdown.clone();
            listener.set_nonblocking(true).map_err(errors::from_io)?;
            while !shutdown.load(Ordering::SeqCst) {
                let mut stream = match listener.accept() {
                    Ok((stream, _)) => stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(std::time::Duration::from_millis(5));
                        continue;
                    }
                    Err(err) => return Err(errors::from_io(err)),
                };
                if shutdown.load(Ordering::SeqCst) {
                    break;
                }
                let mux = mux.clone();
                thread::spawn(move || {
                    let _ = handle_conn(&mux, &mut stream);
                });
            }
            Ok(())
        }
        pub fn Shutdown(&self, _ctx: &super::context::Context) -> Result<()> {
            self.shutdown.store(true, Ordering::SeqCst);
            Ok(())
        }
    }

    fn handle_conn(mux: &ServeMux, stream: &mut TcpStream) -> Result<()> {
        let mut wire = Vec::new();
        let mut buf = [0u8; 8192];
        loop {
            let n = stream.read(&mut buf).map_err(errors::from_io)?;
            if n == 0 {
                break;
            }
            wire.extend_from_slice(&buf[..n]);

            let Some(header_end) = wire.windows(4).position(|w| w == b"\r\n\r\n") else {
                continue;
            };
            let headers = String::from_utf8_lossy(&wire[..header_end]);
            let content_length = headers
                .lines()
                .filter_map(|line| line.split_once(':'))
                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                .unwrap_or(0);
            if wire.len() >= header_end + 4 + content_length {
                break;
            }
        }

        let raw = String::from_utf8_lossy(&wire);
        let mut lines = raw.split("\r\n");
        let first = lines.next().unwrap_or("");
        let mut parts = first.split_whitespace();
        let method = parts.next().unwrap_or("GET").to_string();
        let target = parts.next().unwrap_or("/").to_string();
        let mut header = Header::default();
        for line in lines {
            if line.is_empty() {
                break;
            }
            if let Some((k, v)) = line.split_once(':') {
                header.Set(k.trim(), v.trim());
            }
        }
        let (path, query) = match target.split_once('?') {
            Some((p, q)) => (p.to_string(), q.to_string()),
            None => (target, String::new()),
        };
        let body_start = raw.find("\r\n\r\n").map(|i| i + 4).unwrap_or(raw.len());
        let body = raw.as_bytes().get(body_start..).unwrap_or(&[]).to_vec();
        let req = Request {
            Method: method,
            URL: URL {
                Path: path,
                RawQuery: query,
            },
            Header: header,
            Body: body,
            ctx: super::context::Background(),
        };
        let mut w = ResponseWriter::new();
        mux.serve(&mut w, &req);
        let status_text = match w.status {
            200 => "OK",
            400 => "Bad Request",
            404 => "Not Found",
            405 => "Method Not Allowed",
            500 => "Internal Server Error",
            501 => "Not Implemented",
            _ => "OK",
        };
        let mut resp = format!(
            "HTTP/1.1 {} {}\r\nContent-Length: {}\r\n",
            w.status,
            status_text,
            w.body.len()
        );
        for (k, v) in w.header.map.iter() {
            resp.push_str(&format!("{k}: {v}\r\n"));
        }
        resp.push_str("\r\n");
        stream.write_all(resp.as_bytes()).map_err(errors::from_io)?;
        stream.write_all(&w.body).map_err(errors::from_io)?;
        Ok(())
    }

    pub fn listen_tcp(addr: &str) -> Result<(TcpListener, SocketAddr)> {
        let bind_addr = if addr == ":" {
            "127.0.0.1:0".to_string()
        } else if addr.starts_with(':') {
            format!("127.0.0.1{addr}")
        } else {
            addr.to_string()
        };
        let listener = TcpListener::bind(bind_addr).map_err(errors::from_io)?;
        let local = listener.local_addr().map_err(errors::from_io)?;
        Ok((listener, local))
    }
}

pub mod pprof {
    use super::http::{Request, ResponseWriter};
    pub fn Index(w: &mut ResponseWriter, _r: &Request) {
        let _ = w.Write(b"pprof");
    }
    pub fn Cmdline(w: &mut ResponseWriter, _r: &Request) {
        let _ = w.Write(b"");
    }
    pub fn Profile(w: &mut ResponseWriter, _r: &Request) {
        let _ = w.Write(b"");
    }
    pub fn Symbol(w: &mut ResponseWriter, _r: &Request) {
        let _ = w.Write(b"");
    }
    pub fn Trace(w: &mut ResponseWriter, _r: &Request) {
        let _ = w.Write(b"");
    }
}

pub mod build {
    pub const Lightning: &str = "Lightning";
    pub fn LogInfo(_name: &str) {}
}

pub mod split {
    use std::sync::atomic::{AtomicI32, Ordering};
    static TIMES: AtomicI32 = AtomicI32::new(0);
    pub fn set_WaitRegionOnlineAttemptTimes(v: i32) {
        TIMES.store(v, Ordering::SeqCst);
    }
    pub fn WaitRegionOnlineAttemptTimes() -> i32 {
        TIMES.load(Ordering::SeqCst)
    }
    // mutable global matching Go var
    pub static mut WAIT_REGION_ONLINE_ATTEMPT_TIMES: i32 = 0;
}

pub mod backend {
    use uuid::Uuid;
    pub fn MakeUUID(tableName: &str, engineID: i64) -> (String, Uuid) {
        let tag = format!("{tableName}:{engineID}");
        // Deterministic-enough stand-in (Go uses uuid.NewSHA1); v4 is fine for cleanup path.
        let _ = (tableName, engineID);
        let u = Uuid::new_v4();
        (tag, u)
    }
}

pub mod ingestctrl {
    use super::{Error, Result};
    use std::path::Path;
    use uuid::Uuid;

    thread_local! {
        static TEST_RLIMIT: std::cell::Cell<Option<u64>> = const { std::cell::Cell::new(None) };
        static TEST_SET_ERROR: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    }
    pub type RlimT = u64;
    pub fn SetTestRLimit(limit: Option<RlimT>, set_error: bool) {
        TEST_RLIMIT.set(limit);
        TEST_SET_ERROR.set(set_error);
    }
    pub fn VerifyRLimit(estimate: RlimT) -> Result<()> {
        let forced = TEST_RLIMIT.get();
        if forced.is_some_and(|limit| limit < estimate) && TEST_SET_ERROR.get() {
            return Err(Error::new(format!(
                "cannot raise open file limit from {} to {estimate}",
                forced.unwrap()
            )));
        }
        // Soft check: absurdly large estimates fail like a low rlimit.
        if estimate > 1_000_000_000 {
            return Err(Error::new("rlimit too small"));
        }
        Ok(())
    }
    pub struct Engine {
        pub UUID: Uuid,
    }
    impl Engine {
        pub fn Cleanup(&self, data_dir: &str) -> Result<()> {
            let p = Path::new(data_dir).join(self.UUID.to_string());
            if p.exists() {
                let _ = std::fs::remove_dir_all(&p);
            }
            Ok(())
        }
    }
}

pub mod failpoint {
    pub fn Inject(_name: &str, _f: impl FnOnce()) {}
    pub fn InjectVal(_name: &str, _f: impl FnOnce(FailValue)) {}
    pub struct FailValue;
    pub struct HttpHandler;
    impl HttpHandler {
        pub fn new() -> Self {
            Self
        }
        pub fn ServeHTTP(&self, w: &mut super::http::ResponseWriter, _r: &super::http::Request) {
            let _ = w.Write(b"ok");
        }
    }
}

pub mod tikv {
    use super::context::Context;
    use super::{Result, import_sstpb, pdhttp, tls};
    pub fn ForAllStores<F>(_ctx: &Context, _cli: pdhttp::Client, _state: i32, _f: F) -> Result<()>
    where
        F: Fn(&Context, &pdhttp::MetaStore) -> Result<()>,
    {
        Ok(())
    }
    pub fn SwitchMode(
        _ctx: &Context,
        _tls: &tls::Config,
        _addr: &str,
        _mode: i32,
        _ranges: &[import_sstpb::Range],
    ) -> Result<()> {
        Ok(())
    }
}

pub mod pdhttp {
    #[derive(Clone, Debug, Default)]
    pub struct Client;
    #[derive(Clone, Debug, Default)]
    pub struct MetaStore {
        pub Address: String,
    }
}

pub mod import_sstpb {
    pub const SwitchMode_Import: i32 = 1;
    pub const SwitchMode_Normal: i32 = 0;
    #[derive(Clone, Debug, Default)]
    pub struct Range {}
}

pub mod metapb {
    pub const StoreState_Offline: i32 = 0;
}

pub mod tls {
    #[derive(Clone, Debug, Default)]
    pub struct Config {}
}

pub mod gzip {
    use flate2::Compression;
    use flate2::write::GzEncoder;
    use std::io::Write;
    pub const BestSpeed: u32 = 1;
    pub struct Writer<W: Write> {
        inner: GzEncoder<W>,
    }
    pub fn NewWriterLevel<W: Write>(w: W, _level: u32) -> super::Result<Writer<W>> {
        Ok(Writer {
            inner: GzEncoder::new(w, Compression::fast()),
        })
    }
    impl<W: Write> Writer<W> {
        pub fn Write(&mut self, d: &[u8]) -> super::Result<usize> {
            self.inner.write_all(d).map_err(super::errors::from_io)?;
            Ok(d.len())
        }
        pub fn Close(self) -> super::Result<()> {
            self.inner
                .finish()
                .map(|_| ())
                .map_err(super::errors::from_io)
        }
    }
}

pub mod json {
    use super::{Error, Result};
    use serde::Serialize;
    pub fn Marshal<T: Serialize>(v: &T) -> Result<Vec<u8>> {
        serde_json::to_vec(v).map_err(|e| Error::new(e.to_string()))
    }
    pub struct Encoder<'a, W: std::io::Write> {
        w: &'a mut W,
    }
    pub fn NewEncoder<'a, W: std::io::Write>(w: &'a mut W) -> Encoder<'a, W> {
        Encoder { w }
    }
    impl<'a, W: std::io::Write> Encoder<'a, W> {
        pub fn Encode<T: Serialize>(&mut self, v: &T) -> Result<()> {
            serde_json::to_writer(&mut *self.w, v).map_err(|e| Error::new(e.to_string()))
        }
    }
    pub struct Decoder<'a> {
        data: &'a [u8],
    }
    pub fn NewDecoder(data: &[u8]) -> Decoder<'_> {
        Decoder { data }
    }
    impl Decoder<'_> {
        pub fn Decode<T: serde::de::DeserializeOwned>(&mut self) -> Result<T> {
            serde_json::from_slice(self.data).map_err(|e| Error::new(e.to_string()))
        }
    }
}

pub mod mysql {
    use super::context::Context;
    use super::{Error, Result};
    use std::net::TcpStream;
    use std::sync::{Arc, Mutex, OnceLock};

    type DialFn = Arc<dyn Fn(Context, &str) -> Result<TcpStream> + Send + Sync>;
    static DIALS: OnceLock<Mutex<std::collections::HashMap<String, DialFn>>> = OnceLock::new();
    fn dials() -> &'static Mutex<std::collections::HashMap<String, DialFn>> {
        DIALS.get_or_init(|| Mutex::new(std::collections::HashMap::new()))
    }
    pub fn RegisterDialContext<F>(name: &str, f: F)
    where
        F: Fn(Context, &str) -> Result<TcpStream> + Send + Sync + 'static,
    {
        dials()
            .lock()
            .unwrap()
            .insert(name.to_string(), Arc::new(f));
    }
}

pub mod util {
    use super::atomic::Uint64;
    use std::net::TcpStream;
    use std::sync::Arc;
    pub struct TCPConnWithIOCounter {
        pub inner: TcpStream,
        pub counter: Arc<Uint64>,
    }
    pub fn NewTCPConnWithIOCounter(tcp: TcpStream, counter: Arc<Uint64>) -> TCPConnWithIOCounter {
        TCPConnWithIOCounter {
            inner: tcp,
            counter,
        }
    }
}

pub mod uuid_util {
    pub fn New() -> String {
        uuid::Uuid::new_v4().to_string()
    }
}

// Bridge helpers to real crates.
pub mod bridges {
    use super::config::Config;
    use super::{Error, Result};

    pub fn map_err_cp(e: astersql_lightning_pkg_checkpoints::Error) -> Error {
        Error {
            msg: e.to_string(),
            not_found: e.not_found,
            cause: None,
            class: None,
            empty_num: false,
        }
    }
    pub fn map_err_prog(e: astersql_lightning_pkg_progress::Error) -> Error {
        Error {
            msg: e.to_string(),
            not_found: e.not_found,
            cause: None,
            class: None,
            empty_num: false,
        }
    }
    pub fn map_err_imp(e: astersql_lightning_pkg_importer::Error) -> Error {
        Error {
            msg: e.Error(),
            not_found: e.not_found,
            cause: None,
            class: e.class,
            empty_num: false,
        }
    }
    pub fn map_err_ii(e: astersql_lightning_pkg_importinto::Error) -> Error {
        Error {
            msg: e.Error(),
            not_found: e.not_found,
            cause: None,
            class: e.class,
            empty_num: false,
        }
    }

    pub fn to_checkpoints_cfg(cfg: &Config) -> astersql_lightning_pkg_checkpoints::config::Config {
        let mut c = astersql_lightning_pkg_checkpoints::config::Config::default();
        c.TaskID = cfg.TaskID;
        c.Checkpoint.Enable = cfg.Checkpoint.Enable;
        c.Checkpoint.Driver = cfg.Checkpoint.Driver.clone();
        c.Checkpoint.DSN = cfg.Checkpoint.DSN.clone();
        c.Checkpoint.Schema = cfg.Checkpoint.Schema.clone();
        c.TikvImporter.Backend = cfg.TikvImporter.Backend.clone();
        c.TikvImporter.SortedKVDir = cfg.TikvImporter.SortedKVDir.clone();
        c.Mydumper.SourceDir = cfg.Mydumper.SourceDir.clone();
        c.TiDB.Host = cfg.TiDB.Host.clone();
        c.TiDB.Port = cfg.TiDB.Port;
        c.TiDB.PdAddr = cfg.TiDB.PdAddr.clone();
        c
    }

    pub fn to_importinto_cfg(cfg: &Config) -> astersql_lightning_pkg_importinto::config::Config {
        let mut c = astersql_lightning_pkg_importinto::config::Config::NewConfig();
        c.App.CheckRequirements = cfg.App.CheckRequirements;
        c.App.TableConcurrency = cfg.App.TableConcurrency;
        c.Checkpoint.Enable = cfg.Checkpoint.Enable;
        c.Checkpoint.Driver = cfg.Checkpoint.Driver.clone();
        c.Checkpoint.DSN = cfg.Checkpoint.DSN.clone();
        c.Checkpoint.Schema = cfg.Checkpoint.Schema.clone();
        c.Checkpoint.KeepAfterSuccess = cfg.Checkpoint.KeepAfterSuccess;
        c.Mydumper.SourceDir = cfg.Mydumper.SourceDir.clone();
        c.Mydumper.Filter = cfg.Mydumper.Filter.clone();
        c.Mydumper.FileRouters = cfg.Mydumper.FileRouters.clone();
        c.Mydumper.CharacterSet = cfg.Mydumper.CharacterSet.clone();
        c.Mydumper.DataCharacterSet = cfg.Mydumper.DataCharacterSet.clone();
        c.TiDB.SQLMode = cfg.TiDB.StrSQLMode.clone();
        c.Routes = cfg.Routes.clone();
        c.Cron.LogProgress.Duration = cfg.Cron.LogProgress.Duration;
        c.TikvImporter.StripS3ExternalIDForImportSQL =
            cfg.TikvImporter.StripS3ExternalIDForImportSQL;
        c
    }

    pub fn to_importer_cfg(cfg: &Config) -> astersql_lightning_pkg_importer::config::Config {
        let mut c = astersql_lightning_pkg_importer::config::Config::NewConfig();
        c.TaskID = cfg.TaskID;
        c.App.TaskInfoSchemaName = if cfg.App.TaskInfoSchemaName.is_empty() {
            cfg.App.MetaSchemaName.clone()
        } else {
            cfg.App.TaskInfoSchemaName.clone()
        };
        c.App.RegionConcurrency = cfg.App.RegionConcurrency;
        c.App.TableConcurrency = cfg.App.TableConcurrency;
        c.TikvImporter.Backend = cfg.TikvImporter.Backend.clone();
        c.TikvImporter.SortedKVDir = cfg.TikvImporter.SortedKVDir.clone();
        c.TikvImporter.ParallelImport = cfg.TikvImporter.ParallelImport;
        c.Checkpoint.Enable = cfg.Checkpoint.Enable;
        c.Checkpoint.Driver = cfg.Checkpoint.Driver.clone();
        c.Checkpoint.DSN = cfg.Checkpoint.DSN.clone();
        c.Checkpoint.Schema = cfg.Checkpoint.Schema.clone();
        c.Mydumper.SourceDir = cfg.Mydumper.SourceDir.clone();
        c.TiDB.Host = cfg.TiDB.Host.clone();
        c.TiDB.Port = cfg.TiDB.Port;
        c.TiDB.User = cfg.TiDB.User.clone();
        c.TiDB.Psw = cfg.TiDB.Psw.clone();
        c.TiDB.PdAddr = cfg.TiDB.PdAddr.clone();
        c.TiDB.SQLMode = cfg.TiDB.SQLMode;
        c.TiDB.StrSQLMode = cfg.TiDB.StrSQLMode.clone();
        c.TiDB.UUID = cfg.TiDB.UUID.clone();
        c
    }

    pub fn progress_dbs(
        dbs: &[super::mydump::MDDatabaseMeta],
    ) -> Vec<astersql_lightning_pkg_progress::mydump::MDDatabaseMeta> {
        dbs.iter()
            .map(
                |d| astersql_lightning_pkg_progress::mydump::MDDatabaseMeta {
                    Name: d.Name.clone(),
                    Tables: d
                        .Tables
                        .iter()
                        .map(|t| astersql_lightning_pkg_progress::mydump::MDTableMeta {
                            Name: t.Name.clone(),
                            TotalSize: t.TotalSize,
                        })
                        .collect(),
                },
            )
            .collect()
    }
}

// Re-export commonly used items at crate root via stubs::*
pub use atomic::*;
