// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

//! Lightning server lifecycle — ported from `lightning.go`.
//!
//! 中文说明总览：
//! 本文件是 Lightning server 子系统的真正主入口，负责把静态配置变成可运行的控制面和任务生命周期。
//! 它既连接 HTTP API，又连接 importer 与 import-into 两个导入后端，因此本质上是编排层而不是算法层。
//! 保持 Go 语义一致的重点有三类：错误时机、HTTP 状态码、任务与资源释放顺序。
//! 下面的逐段说明按“全局状态 -> 生命周期 -> HTTP 接口 -> 校验函数 -> 后端适配器”展开。
//! `DELIVER_PAUSER_GLOBAL` 保存进程级暂停器。
//! HTTP `/pause` 与 legacy importer 共享这一状态，避免当前没有 importer 时丢失暂停意图。
//! `deliver_pauser()` 统一返回全局暂停器，减少多处初始化静态值的样板代码。
//! `DeliverPauser` 对外暴露 Pause、Resume、IsPaused 三个动作。
//! 这样路由层只接触稳定接口，而不直接感知 OnceLock。
//! `LightningStatus` 只记录已完成与总文件大小。
//! 这里故意不承载表级细节，因为表级进度由 progress 子系统单独维护。
//! `HttpState` 保存 HTTP handler 需要观测的共享状态。
//! 队列、当前任务、取消函数和 importer 被打包进 `Arc<Mutex<_>>`，便于多个路由闭包复用。
//! `Lightning` 是 server 子系统的总协调者。
//! 它同时持有全局配置、运行时状态、HTTP server、指标注册器和当前任务快照。
//! 这是 Go `Lightning` 结构体的直接对应物。
//! `initEnv` 负责初始化日志环境。
//! 只有配置了日志文件时才真正调用 logger 初始化，避免默认空配置触发副作用。
//! `New` 负责构造 Lightning 实例并完成 TLS、context、registry 等基础设施初始化。
//! 如果日志或 TLS 初始化失败，它会立即中止启动，以保持 Go 那种“启动失败尽早暴露”的风格。
//! `httpHandleWrapper` 为所有 HTTP handler 包一层统一访问日志。
//! method、url 与最终状态码都在这里记录，从而让业务 handler 专注于协议逻辑。
//! `GoServe` 是外部启动状态服务的入口。
//! 它同时处理常规 `StatusAddr` 启动和 Unix `SIGUSR1` 拉起状态服务这两条路径。
//! 当 `StatusAddr` 为空时，它必须安静返回，不应擅自监听端口。
//! `goServe` 才是真正搭建 mux、metrics、pprof 与业务路由的地方。
//! 它会把当前运行时快照写入 `HttpState`，以便路由闭包拥有一致视图。
//! `/tasks`、`/progress/task`、`/progress/table`、`/pause`、`/resume`、`/loglevel` 都在这里挂载。
//! 监听成功后还要把实际地址写回 `serverAddr`，供日志与测试读取。
//! `RunServer` 进入 server mode 的任务循环。
//! 它从队列弹出任务后复用 `run()` 完成单次导入，而不是实现第二套运行逻辑。
//! 这样 HTTP server 模式和单次任务模式才能共享同一份核心流程。
//! 错误时调用 `DeliverPauser::Pause()`，是为了与 Go 一样在失败后冻结进度展示。
//! `RunOnceWithOptions` 是单次任务入口。
//! 它负责应用闭包注入、补齐 TaskID、设置自定义 DB 或存储，并最终转入 `run()`。
//! 对外暴露 Option 形态，是为了让测试和上层 API 都能按需覆盖默认资源。
//! `run` 串起日志、metrics、数据源加载、DB 初始化、importer 选择和最终清理。
//! 它是本文件最需要保持 Go 失败时机一致的函数。
//! 一旦这里的顺序改变，调用方看到的错误、HTTP 状态和资源副作用都会发生偏移。
//! `run` 开始时会记录配置并同步 region online backoff。
//! 随后它构造 metrics，并把 logger 与 metrics 注入任务级 context。
//! 再之后它设置 `cancel`、`curTask` 并广播任务开始事件。
//! 只有在这些共享状态准备完毕后，后续错误才会对外表现成“确实有任务在执行中”。
//! `run` 中 `BuildTLSConfig()` 的位置很重要。
//! 这是一个典型的前置校验：必须在真正加载数据与创建 importer 之前就失败。
//! 数据源初始化只在非 import-into backend 下执行。
//! 因为 import-into 的数据准备模型与 legacy 不同，不能共享同一套 MDLoader 流程。
//! `initDBAndKeyspace` 返回 DB 和 keyspace 名称后，`ControllerParamLocal` 才能完整构造。
//! `newImporter()` 返回的对象会被包进 `Arc<Mutex<_>>` 并挂到 `self.importer`。
//! 这样 pause、resume、delete current task 等 HTTP 动作才能安全拿到运行中的 importer。
//! importer 运行结束后，`run` 会无论成功失败都清空 `cancel` 与 `importer`。
//! 这条清理动作直接决定 `/tasks` 列表里是否还会把任务误判成 current。
//! 最终它还会广播任务结束并注销 metrics，确保每次任务运行都是独立的生命周期。
//! `initDataSource` 的职责是解析数据源、创建 storage、探测空目录并构造 MDLoader。
//! 它会先通过 `WalkDir` 做最小探测，区分“空目录”和“访问出错”这两类情况。
//! 探测成功后再真正创建 loader，并执行系统要求检查与 checkpoint schema 冲突检查。
//! 这保证昂贵的导入动作不会在明显前置条件失败时继续推进。
//! `Stop` 的职责是停止当前任务并关闭 HTTP server。
//! 如果当前存在 cancel 句柄，还要把 `taskCanceled` 标成真，以便测试和调用方区分主动取消。
//! `TaskCanceled` 只在锁保护下读取该标志，避免并发观察到中间态。
//! `Status` 和 `Metrics` 则是两个简单的只读视图接口。
//! `getKeyspaceName` 的职责是从 DB 里探测 TiDB keyspace 配置。
//! 只有 local backend 且未显式配置 keyspace 时才需要查询它。
//! `initDBAndKeyspace` 的职责是统一准备 DB 连接与 keyspace 名称。
//! 显式配置优先，自动探测只作为兜底，而且权限不足属于可记录日志但可继续的情况。
//! `writeJSONError` 统一构造 JSON 错误响应。
//! 这样所有 HTTP handler 都能输出同一格式，减少前端和脚本端的解析分支。
//! `parseTaskID` 负责把剥离前缀后的路径解析成 task id 与 patch verb。
//! 空字符串需要带上 `empty_num` 标记，这是 GET `/tasks` 与 GET `/tasks/<id>` 分流的关键。
//! `handle_task_http` 是 `/tasks` 的总路由入口。
//! 它根据方法和值语义分派到列表、详情、创建、删除或调整顺序逻辑。
//! 同时它也负责在方法非法时设置 `Allow` 头。
//! `handle_get_task` 的职责是返回当前任务和排队任务的快照。
//! `handle_get_one_task` 的职责是返回单个任务配置，并在可能时走 gzip 压缩输出。
//! `handle_post_task` 的职责是把 TOML 请求体恢复成 task config 并压入队列。
//! `handle_delete_one_task` 的职责是区分“取消当前任务”和“删除排队任务”两种情况。
//! `handle_patch_one_task` 的职责是调整队列中的前后顺序，只允许 `front` 和 `back` 两个动作。
//! `handle_pause_http` 与 `handle_resume_http` 的职责是桥接 HTTP 控制面与 importer 暂停语义。
//! 有 importer 时优先调用 importer；没有 importer 时则只修改全局 `DeliverPauser` 状态。
//! 这样队列空闲时也能记录暂停意图，保持与 Go 的控制面心智一致。
//! `writeBytesCompressed` 的职责是处理 gzip 协商。
//! GET 单任务与进度接口都通过它输出，避免每个 handler 自己实现压缩逻辑。
//! `handleProgressTask` 与 `handleProgressTable` 只负责 HTTP 协议包装。
//! 真正的进度序列化由 progress 子系统完成，这里不重复实现状态聚合。
//! `handleLogLevel` 提供读取与修改日志级别的在线接口。
//! 其意义不只是调试便利，更在于保持与 Go 相同的运维控制面。
//! `checkSystemRequirement` 只在 local backend 下估算打开文件数。
//! 这条公式与 Go 对齐非常重要，因为它直接影响大规模导入时的资源保护行为。
//! `checkSchemaConflict` 的职责是检查 checkpoint schema 是否与导入数据重名。
//! 只有 MySQL checkpoint driver 开启时才执行该保护，并且错误类名也要保持对齐。
//! `SwitchMode` 把字符串模式翻译成 proto 枚举，再对所有 store 发起切换动作。
//! 非法模式必须立即失败，避免把错误请求静默降级成默认行为。
//! `LightningImporter` trait 抽象 legacy 与 import-into 两种导入后端。
//! server 层只关心 Run、Pause、Resume、Close 四类动作，不关心具体实现细节。
//! `ControllerParamLocal` 是 server 运行时到真正导入器之间的参数桥。
//! 它把 DBMeta、状态、外部存储、checkpoint 和 keyspace 信息集中到一处。
//! `LegacyImporter` 的职责是把 legacy importer::Controller 包装成 `LightningImporter`。
//! 因为原 controller 不是 `Send`，所以它要在当前线程里完成运行与关闭。
//! 这解释了为什么 server 层需要单独的适配器，而不能直接持有下游 controller。
//! `LegacyImporter::Pause` 与 `Resume` 会同时驱动本地 pauser 和全局 `DeliverPauser`。
//! 这样 HTTP 查询到的暂停状态与导入线程观察到的状态不会分裂。
//! `ImportIntoImporter` 的职责是包装 import-into backend 的运行流程。
//! 它通过 `ProgressUpdater` 把 import-into 进度回写到 `LightningStatus`。
//! 同时它会明确记录“当前 backend 暂不支持 pause/resume”，而不是伪装成已支持。
//! `newImporter` 是后端选择的守门点。
//! 在真正返回 importer 之前，它会先做 checkpoint driver 或 DSN 的构造性探测。
//! 这样未知 driver 和坏 checkpoint 存储能在预检查阶段就按 Go 的时机失败。
//! 整体不变量是：启动阶段先准备 TLS、logger、context 和 registry，再决定是否监听状态端口。
//! 整体不变量是：任务开始时必须设置 `cancel`、`curTask` 与进度广播；任务结束后必须清空这些共享状态。
//! 整体不变量是：HTTP handler 只读取或调整共享状态，不直接决定 importer 的实现细节。
//! 整体不变量是：legacy 与 import-into 的能力差异要通过适配层吸收，而不是泄漏到路由层。
//! 整体不变量是：注释强调“为什么这样分层”，以便未来改动时仍能守住 Go 的行为边界。
//! 下面继续按实现顺序补充更细的阅读索引，帮助维护者把一千多行编排代码拆成若干稳定区段。
//! 第一段区段可以概括为“进程级静态状态”。
//! `DELIVER_PAUSER_GLOBAL`、默认 logger 与默认 registry 都属于进程级一次性资源。
//! 这些资源要跨多次任务执行复用，否则 server mode 很容易出现跨任务状态漂移。
//! 其中暂停器最特殊，因为它既要被 HTTP 请求修改，也要被 legacy importer 线程读取。
//! 所以它被放到全局静态位置，而不是挂在某个短生命周期 task 上。
//! 第二段区段是“实例级共享状态”。
//! `Lightning` 结构体保存当前任务、取消函数、metrics 和 HTTP server。
//! 这些字段代表一次 server 实例生命周期内需要集中管理的可变资源。
//! 它们与进程级静态值的区别在于：实例可关闭重建，但进程级控制意图仍可能存在。
//! `HttpState` 只是这些共享状态的路由观察视图，而不是另一份真正的数据源。
//! 这意味着每当 `goServe()` 重新装配 mux 时，都要把最新快照写回 `http_state`。
//! 否则 handler 读到的就会是旧 task、旧 cancel 或旧 importer。
//! `httpHandleWrapper` 除了写访问日志，还有一个隐含价值。
//! 它把状态码采集放到统一出口，避免某个 handler 早退时遗漏日志结束事件。
//! 对线上排障来说，这种统一包装比在每个路由里散落日志更可靠。
//! `GoServe` 的 Unix `SIGUSR1` 分支看起来像特殊路径，实际上是在保留 Go 的运维习惯。
//! 某些部署模式不会提前配置 `StatusAddr`，但仍希望在收到信号后临时打开状态端口。
//! 因此该分支必须既支持“第一次启动”，也支持“已经启动则只提示地址”。
//! 这也是它额外保存 `status_for_sig` 与 `addr_slot` 的原因。
//! `GoServe` 外层先取一次 `serverLock` 再读地址，不是多余同步。
//! 这样能避免信号分支与常规启动分支在地址判定上发生竞态。
//! `goServe()` 里的 mux 装配顺序也值得明确。
//! 它先挂 metrics、pprof 等通用端点，再挂 `/tasks` 和进度相关业务端点。
//! 这样即使任务还未开始，状态服务仍可提供最小观测能力。
//! 对应地，HTTP 服务能成功监听，并不代表 importer 已准备完成。
//! 这是控制面与执行面之间的重要分层边界。
//! `serverAddr` 会在监听成功后回填。
//! 这个回填不仅供日志打印，也供测试黑盒检查“到底监听到了哪个随机端口”。
//! 如果去掉这个回填，很多串行 HTTP 测试就只能依赖脆弱的 stderr 文本解析。
//! `RunServer` 的阅读关键在于，它并不拥有自己的一套执行器状态机。
//! 它只是从 `taskCfgs` 队列里不停取出任务，然后复用 `run()`。
//! 这保证 server mode 与单次执行模式共享完全相同的前置校验和清理顺序。
//! 也正因为如此，大多数真正决定行为的逻辑都集中在 `run()` 而不是 `RunServer`。
//! `RunServer` 更像是一个调度壳。
//! 它决定什么时候取下一个任务，以及错误后是否进入暂停态。
//! 这里错误后调用 `DeliverPauser::Pause()`，本质上是把失败转成一个跨请求可见的控制信号。
//! 这样外部观察者即使没有立刻看到具体错误，也能先从暂停状态推断系统需要人工介入。
//! `RunOnceWithOptions` 则承担完全不同的入口职责。
//! 它服务于单任务 CLI 或测试场景，因此允许通过闭包覆盖 DB、存储、logger 等依赖。
//! 这种 Option 入口的价值，不在于模式本身，而在于它让单次执行共享 `run()` 的主流程。
//! 如果没有这层入口，测试就只能复制另一套构造路径，很容易与 Go 行为偏离。
//! `run()` 可以被拆成七个连续阶段来理解。
//! 第一阶段是记录配置、准备 logger、同步少量全局开关。
//! 第二阶段是创建任务级 metrics 并把它们注入 context。
//! 第三阶段是安装 `cancel`、`curTask`、`importer` 等共享状态。
//! 第四阶段是执行 TLS、数据源、DB、keyspace 等前置校验。
//! 第五阶段是创建具体 importer 并执行 `Run`。
//! 第六阶段是根据成功或失败结果更新日志与对外状态。
//! 第七阶段是无条件做清理，包括注销 metrics 和清空共享字段。
//! 这七个阶段的顺序不能随意交换。
//! 例如如果把 `cancel` 设置挪到 importer 创建之后，HTTP 删除当前任务时就可能观察不到取消句柄。
//! 反之如果把 `importer` 提前暴露到共享状态，又会让 pause/resume 过早拿到未初始化完成的对象。
//! 因而 `run()` 的主要难点不是算法，而是对外可观察时机的稳定性。
//! `BuildTLSConfig()` 放在数据源加载之前，也是这种时机稳定性的一个例子。
//! TLS 构造失败属于明显前置错误，应该尽早暴露。
//! 如果放到更后面，调用方就会看到数据源探测等多余副作用。
//! `initDataSource()` 可以再拆成四个小责任。
//! 第一个责任是把配置里的 backend 地址翻译成可访问的 storage。
//! 第二个责任是通过最小遍历判断目录是空、可达还是访问失败。
//! 第三个责任是基于 storage 构造 `MDLoader` 并拉出数据库元信息。
//! 第四个责任是利用这些元信息执行系统要求检查与 checkpoint schema 冲突检查。
//! 这四步组合起来，形成 importer 之前最重要的“现实世界前置面”。
//! 如果其中任何一步失败，后续 importer 根本不该被构造。
//! `Stop` 的实现也不是简单关闭 HTTP server。
//! 它首先承担“当前任务是否被主动取消”这一额外状态的记录。
//! 这能让测试和上层调用方区分主动停止与任务自然结束。
//! 对长期维护来说，这个差别会直接影响错误归因。
//! `TaskCanceled` 则有意做成只读查询，而不是让调用者直接摸 `taskCanceled` 字段。
//! 这样可以把并发读取统一约束在锁保护下。
//! `Status` 与 `Metrics` 虽然只是 getter，但也是对外合同的一部分。
//! 它们告诉调用方：状态与指标可以被观察，但不允许从外部直接改写。
//! `getKeyspaceName` 看似只是个小查询函数，实则承担 local backend 的兼容兜底。
//! 因为配置未显式声明 keyspace 时，server 仍需要尽量与实际 TiDB 环境对齐。
//! 这里对权限不足的处理尤其微妙。
//! 权限不足会记日志，但在某些路径下不直接阻塞任务。
//! 这种“可解释但可继续”的处理方式，正是 Go 控制面对运维容错的延续。
//! `initDBAndKeyspace` 把 DB 构造与 keyspace 判定收束到一起。
//! 它的意义不是省代码，而是确保所有调用路径都遵守同一套优先级。
//! 这套优先级就是：显式配置优先，自动探测次之，权限不足可退化记录。
//! `writeJSONError` 的价值也不止于复用响应模板。
//! 它统一了错误前缀、消息拼接和内容类型，从而让 CLI、脚本与测试都能稳定解码。
//! 一旦不同 handler 自己拼错误 JSON，外围系统就会出现大量条件分支。
//! `parseTaskID` 是整个 `/tasks` 家族里最容易被低估的辅助函数。
//! 它不仅要解析数字 task id，还要保留 patch 动作尾缀。
//! 更关键的是，它要用 `empty_num` 把“没有 id”与“id 非法”区分开。
//! 没有这个区分，`GET /tasks` 与 `GET /tasks/<id>` 的路由分流就会混乱。
//! `handle_task_http` 因而承担真正的总路由职责。
//! 它根据 HTTP method、解析结果以及尾缀 verb 决定进入哪个具体子处理器。
//! 这个总路由还负责在非法 method 时写 `Allow` 头，维持规范的 HTTP 语义。
//! `handle_get_task` 返回的是任务快照而不是实时可变引用。
//! 这样外部看到的是一个一致的瞬时视图，而不是读到一半结构又被后台线程改掉。
//! `handle_get_one_task` 与 `writeBytesCompressed` 配合，代表另一类控制面约束。
//! 也就是“即便只读接口，也要保持与 Go 一样的压缩协商行为”。
//! 这类行为若缺失，人工 curl 或自动脚本往往会最先发现兼容性问题。
//! `handle_post_task` 是把 TOML 文本变成排队任务的唯一入口。
//! 它依赖 `LoadFromGlobal` 再 `LoadFromTOML` 的顺序恢复配置。
//! 该顺序既体现全局默认值继承，也决定了哪些字段允许被单任务覆盖。
//! 所以这里最值得防守的不是语法解析，而是恢复顺序和错误边界。
//! `handle_delete_one_task` 要同时支持取消当前任务与删除排队任务。
//! 这两个动作在实现上完全不同，但对外都表现为删除。
//! 因而它必须先判断目标是不是当前任务，再决定走 cancel 还是队列删除。
//! `handle_patch_one_task` 则只允许 `front` 与 `back` 两种 verb。
//! 这不是实现偷懒，而是为了保持控制面只暴露最小稳定重排语义。
//! 一旦开放更多中间排序动作，调用方合同就会迅速复杂化。
//! `handle_pause_http` 与 `handle_resume_http` 是最能体现适配层价值的路由。
//! 当 importer 存在且支持暂停时，它们会桥接到具体 importer。
//! 当 importer 不存在时，它们只修改全局暂停器，保存“未来任务应暂停”的意图。
//! 这说明暂停语义并不只属于某个 importer，而属于整个 server 控制面。
//! `handleProgressTask` 与 `handleProgressTable` 的设计也体现了明确分工。
//! 它们负责 HTTP 封装和压缩协商，但不负责自己计算进度。
//! 这样一来，进度聚合逻辑就能继续集中在 progress 子系统内部。
//! `handleLogLevel` 则是典型的在线运维控制面接口。
//! 它既要支持读取当前级别，也要支持修改级别并回写 JSON。
//! 保持这套接口存在，能让 Rust 版本继续被现有运维脚本直接控制。
//! `checkSystemRequirement` 的重点不是检查很多东西，而是检查对的东西。
//! 对 local backend 而言，最关键的系统前提就是打开文件数是否足够。
//! 因为导入时会同时持有大量数据文件、engine 文件与网络句柄。
//! 所以这里保留 Go 同款估算公式，比堆更多检查项更重要。
//! `checkSchemaConflict` 看似独立，其实紧贴 checkpoint 协议。
//! 当 checkpoint 使用 MySQL driver 时，某些保留 schema 名称会与导入数据冲突。
//! 如果不在数据加载后立刻挡住这个冲突，后续失败信息就会更晚也更难解释。
//! `SwitchMode` 的职责则是把运维层字符串模式翻译成 TiKV 侧真实枚举。
//! 这个翻译必须显式失败，不能把未知字符串静默映射到默认模式。
//! 因为模式切换属于高风险运维动作，任何静默降级都可能扩大故障面。
//! `LightningImporter` trait 只暴露 Run、Pause、Resume、Close 四类动作。
//! 这说明 server 层只愿意依赖最小控制协议，而不愿意感知底层 importer 的全部细节。
//! 该最小协议越稳定，legacy 与 import-into 之间的替换成本就越低。
//! `ControllerParamLocal` 是从 server 世界通往真实导入器世界的参数桥。
//! 它把 DBMeta、checkpoint、外部存储、keyspace 和进度状态集中在一处。
//! 集中参数的好处不是形式整洁，而是能明确 adapter 究竟从上层消费了哪些语义。
//! `LegacyImporter` 的包装重点是线程模型。
//! 原 controller 不是 `Send`，因此 server 层不能把它随意移动到别的线程。
//! 适配器存在的目的，就是让这种线程限制对上层表现成一个普通 importer。
//! `LegacyImporter::Pause` 与 `Resume` 同时修改本地 pauser 与全局暂停器，也是一条关键对齐点。
//! 如果只改其中一个，HTTP 观察到的状态与实际导入线程观察到的状态就会分裂。
//! `ImportIntoImporter` 的包装重点则完全不同。
//! 它更关注如何把 import-into 的进度回写到 `LightningStatus`。
//! 同时它会明确承认“当前 backend 暂不支持 pause/resume”。
//! 这种显式承认能力边界，比伪造一个无效实现更符合当前迁移阶段的真实语义。
//! `newImporter` 于是成为两类后端的总守门点。
//! 它不仅选择实现，还在选择前先做 checkpoint driver 与 DSN 的构造性探测。
//! 这样未知 driver、坏存储或不支持的 backend 都能在统一位置失败。
//! 从维护角度看，本文件最容易出错的地方通常不是某个 if 分支，而是共享状态时机。
//! 因为控制面代码的回归往往表现为“某时刻外部看到了不该看到的状态”。
//! 顶部这些补充注释就是在帮助维护者先建立时机模型，再进入具体实现。
//! 只要时机模型不被破坏，很多重构仍可以在不改变 Go 行为的前提下进行。
//! 反过来说，只要时机模型被破坏，即便单个函数看起来更简洁，也可能造成协议回归。
//! 因而本文件的中文注释重点始终放在生命周期和可观察边界上。
//! 这也是为什么这里的注释密度会显著高于普通业务函数。
//! 它承担的是控制面编排文档的角色，而不仅是局部代码说明。
//! 对阅读者来说，先把这些层次装进脑中，再看具体实现，会更容易分辨哪里是合同、哪里只是写法。
//! 这正是当前任务希望为后续维护者留下的长期价值。

use crate::atomic;
use crate::bridges;
use crate::build;
use crate::collectors;
use crate::common;
use crate::config;
use crate::context;
use crate::errors;
use crate::failpoint;
use crate::gzip;
use crate::handleSigUsr1;
use crate::http;
use crate::import_sstpb;
use crate::ingestctrl;
use crate::json;
use crate::log;
use crate::logutil;
use crate::metapb;
use crate::metric;
use crate::mydump;
use crate::mysql;
use crate::objstore;
use crate::options;
use crate::pdhttp;
use crate::pprof;
use crate::promhttp;
use crate::promutil;
use crate::redact;
use crate::split;
use crate::sql;
use crate::storeapi;
use crate::tikv;
use crate::tls;
use crate::util;
use crate::zap;
use crate::zapcore;
use crate::{Error, Result, RunOption};
use std::io::{self, Write};
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};
use std::thread;

static DELIVER_PAUSER_GLOBAL: std::sync::OnceLock<common::Pauser> = std::sync::OnceLock::new();
fn deliver_pauser() -> &'static common::Pauser {
    DELIVER_PAUSER_GLOBAL.get_or_init(common::NewPauser)
}

/// Go `importer.DeliverPauser`.
pub struct DeliverPauser;
impl DeliverPauser {
    pub fn Pause() {
        deliver_pauser().Pause();
    }
    pub fn Resume() {
        deliver_pauser().Resume();
    }
    pub fn IsPaused() -> bool {
        deliver_pauser().IsPaused()
    }
}

/// LightningStatus tracks import file size progress.
#[derive(Clone, Default)]
pub struct LightningStatus {
    pub FinishedFileSize: atomic::Int64,
    pub TotalFileSize: atomic::Int64,
}

struct HttpState {
    taskCfgs: Option<Arc<config::List>>,
    curTask: Option<config::Config>,
    cancel: Option<context::CancelFunc>,
    importer: Option<Arc<Mutex<Box<dyn LightningImporter>>>>,
    globalCfg: config::GlobalConfig,
    ctx: context::Context,
}

/// Lightning is the main struct of the lightning package.
pub struct Lightning {
    pub globalCfg: config::GlobalConfig,
    pub globalTLS: common::TLS,
    pub taskCfgs: Option<Arc<config::List>>,
    pub ctx: context::Context,
    pub shutdown: context::CancelFunc,
    pub server: http::Server,
    pub serverAddr: Option<SocketAddr>,
    pub serverLock: Mutex<()>,
    pub status: Arc<LightningStatus>,
    pub promFactory: promutil::Factory,
    pub promRegistry: promutil::Registry,
    pub metrics: Option<metric::Metrics>,
    pub cancelLock: Mutex<()>,
    pub curTask: Option<config::Config>,
    pub cancel: Option<context::CancelFunc>,
    pub importer: Option<Arc<Mutex<Box<dyn LightningImporter>>>>,
    pub taskCanceled: bool,
    http_state: Option<Arc<Mutex<HttpState>>>,
}

pub fn initEnv(cfg: &config::GlobalConfig) -> Result<()> {
    if cfg.App.Config.File.is_empty() {
        return Ok(());
    }
    log::InitLogger(&cfg.App.Config, &cfg.TiDB.LogLevel)
}

/// New creates a new Lightning instance.
pub fn New(globalCfg: config::GlobalConfig) -> Box<Lightning> {
    if let Err(err) = initEnv(&globalCfg) {
        println!("Failed to initialize environment: {err}");
        std::process::exit(1);
    }
    let tls = common::NewTLS(
        &globalCfg.Security.CAPath,
        &globalCfg.Security.CertPath,
        &globalCfg.Security.KeyPath,
        &globalCfg.App.StatusAddr,
        &globalCfg.Security.CABytes,
        &globalCfg.Security.CertBytes,
        &globalCfg.Security.KeyBytes,
    )
    .unwrap_or_else(|err| {
        log::L().Fatal("failed to load TLS certificates", zap::Error(err));
    });
    redact::InitRedact(globalCfg.Security.RedactInfoLog);
    let (ctx, shutdown) = context::WithCancel(context::Background());
    Box::new(Lightning {
        globalCfg,
        globalTLS: tls,
        taskCfgs: None,
        ctx,
        shutdown,
        server: http::Server::default(),
        serverAddr: None,
        serverLock: Mutex::new(()),
        status: Arc::new(LightningStatus::default()),
        promFactory: promutil::NewDefaultFactory(),
        promRegistry: promutil::NewDefaultRegistry(),
        metrics: None,
        cancelLock: Mutex::new(()),
        curTask: None,
        cancel: None,
        importer: None,
        taskCanceled: false,
        http_state: None,
    })
}

fn httpHandleWrapper<F>(h: F) -> http::Handler
where
    F: Fn(&mut http::ResponseWriter, &http::Request) + Send + Sync + 'static,
{
    http::Handler::from_fn(move |w, r| {
        let logger = log::L()
            .With(zap::String("method", &r.Method))
            .With(zap::Stringer("url", &r.URL))
            .Begin(zapcore::InfoLevel, "process http request");
        h(w, r);
        let status = w.status;
        logger.End(zapcore::InfoLevel, None, zap::Int("status", status as i64));
    })
}

impl Lightning {
    fn refreshHttpState(&self) {
        if let Some(state) = &self.http_state {
            let mut state = state.lock().unwrap();
            state.taskCfgs = self.taskCfgs.clone();
            state.curTask = self.curTask.clone();
            state.cancel = self.cancel.clone();
            state.importer = self.importer.clone();
        }
    }

    pub(crate) fn enableServerMode(&mut self) {
        self.taskCfgs = Some(config::NewConfigList());
        self.refreshHttpState();
    }

    /// GoServe starts the HTTP server in a goroutine.
    pub fn GoServe(&mut self) -> Result<()> {
        let status_for_sig = Arc::new(Mutex::new(self.globalCfg.App.StatusAddr.clone()));
        let addr_slot = Arc::new(Mutex::new(self.serverAddr));
        {
            let status_for_sig = status_for_sig.clone();
            let addr_slot = addr_slot.clone();
            handleSigUsr1(move || {
                let should_start = {
                    let mut status = status_for_sig.lock().unwrap();
                    let empty = status.is_empty();
                    if empty {
                        *status = ":".to_string();
                    }
                    empty
                };
                if should_start {
                    match http::listen_tcp(":") {
                        Ok((listener, addr)) => {
                            *addr_slot.lock().unwrap() = Some(addr);
                            eprintln!("started HTTP server on {addr}");
                            let mut server = http::Server::default();
                            server.Handler = Some(http::NewServeMux());
                            thread::spawn(move || {
                                let _ = server.Serve(listener);
                            });
                        }
                        Err(err) => {
                            log::L().Warn("failed to start HTTP server", log::ShortError(err));
                        }
                    }
                } else {
                    let addr = addr_slot.lock().unwrap().clone();
                    log::L().Info(
                        "already started HTTP server",
                        zap::Stringer("address", format!("{addr:?}")),
                    );
                }
            });
        }

        let statusAddr = {
            let _g = self.serverLock.lock().unwrap();
            self.globalCfg.App.StatusAddr.clone()
        };
        if statusAddr.is_empty() {
            return Ok(());
        }
        self.goServe(&statusAddr, &mut io::sink())
    }

    pub fn goServe(&mut self, statusAddr: &str, realAddrWriter: &mut dyn Write) -> Result<()> {
        let mux = http::NewServeMux();
        let registry = self.promRegistry.clone();
        registry.MustRegister(collectors::NewProcessCollector(
            collectors::ProcessCollectorOpts {},
        ));
        registry.MustRegister(collectors::NewGoCollector());
        if registry.AsGatherer().is_some() {
            let handler = promhttp::InstrumentMetricHandler(
                &registry,
                promhttp::HandlerFor((), promhttp::HandlerOpts {}),
            );
            mux.Handle("/metrics", handler);
        }
        mux.HandleFunc("/debug/pprof/", pprof::Index);
        mux.HandleFunc("/debug/pprof/cmdline", pprof::Cmdline);
        mux.HandleFunc("/debug/pprof/profile", pprof::Profile);
        mux.HandleFunc("/debug/pprof/symbol", pprof::Symbol);
        mux.HandleFunc("/debug/pprof/trace", pprof::Trace);

        let state = Arc::new(Mutex::new(HttpState {
            taskCfgs: self.taskCfgs.clone(),
            curTask: self.curTask.clone(),
            cancel: self.cancel.clone(),
            importer: self.importer.clone(),
            globalCfg: self.globalCfg.clone(),
            ctx: self.ctx.clone(),
        }));
        self.http_state = Some(state.clone());

        let st = state.clone();
        let handle_tasks = http::StripPrefix(
            "/tasks",
            http::Handler::from_fn(move |w, r| handle_task_http(&st, w, r)),
        );
        let ht = handle_tasks.clone();
        mux.Handle("/tasks", httpHandleWrapper(move |w, r| ht.ServeHTTP(w, r)));
        let st2 = state.clone();
        let handle_tasks2 = http::StripPrefix(
            "/tasks",
            http::Handler::from_fn(move |w, r| handle_task_http(&st2, w, r)),
        );
        mux.Handle(
            "/tasks/",
            httpHandleWrapper(move |w, r| handle_tasks2.ServeHTTP(w, r)),
        );
        mux.Handle("/progress/task", httpHandleWrapper(handleProgressTask));
        mux.Handle("/progress/table", httpHandleWrapper(handleProgressTable));
        let st3 = state.clone();
        mux.Handle(
            "/pause",
            httpHandleWrapper(move |w, r| handle_pause_http(&st3, w, r)),
        );
        let st4 = state.clone();
        mux.Handle(
            "/resume",
            httpHandleWrapper(move |w, r| handle_resume_http(&st4, w, r)),
        );
        mux.Handle("/loglevel", httpHandleWrapper(handleLogLevel));

        let (listener, addr) = http::listen_tcp(statusAddr)?;
        self.serverAddr = Some(addr);
        log::L().Info(
            "starting HTTP server",
            zap::Stringer("address", format!("{addr}")),
        );
        let _ = writeln!(realAddrWriter, "started HTTP server on {addr}");
        self.server.Handler = Some(mux.clone());
        let listener = self.globalTLS.WrapListener(listener);
        let mut server = http::Server {
            Handler: Some(mux),
            shutdown: self.server.shutdown.clone(),
            ..Default::default()
        };
        thread::spawn(move || {
            let err = server.Serve(listener);
            log::L().Info("stopped HTTP server", log::ShortError(format!("{err:?}")));
        });
        Ok(())
    }

    /// RunServer starts HTTP server mode task loop.
    pub fn RunServer(&mut self) -> Result<()> {
        self.enableServerMode();
        log::L().Info(
            "Lightning server is running, post to /tasks to start an import task",
            zap::Stringer("address", format!("{:?}", self.serverAddr)),
        );
        loop {
            let task = self.taskCfgs.as_ref().unwrap().Pop(&self.ctx)?;
            let mut o = options {
                promFactory: Some(self.promFactory.clone()),
                promRegistry: Some(self.promRegistry.clone()),
                logger: log::L(),
                ..Default::default()
            };
            if let Err(err) = self.run(context::Background(), task, &mut o) {
                if !common::IsContextCanceledError(&err) {
                    DeliverPauser::Pause();
                    log::L().Error("tidb lightning encountered error", zap::Error(err));
                }
            }
        }
    }

    /// RunOnceWithOptions runs a single import task.
    pub fn RunOnceWithOptions(
        &mut self,
        taskCtx: context::Context,
        mut taskCfg: config::Config,
        opts: Vec<RunOption>,
    ) -> Result<()> {
        let mut o = options {
            promFactory: Some(self.promFactory.clone()),
            promRegistry: Some(self.promRegistry.clone()),
            logger: log::L(),
            ..Default::default()
        };
        for opt in opts {
            opt(&mut o);
        }
        failpoint::Inject("setExtStorage", || {});
        failpoint::Inject("setCheckpointName", || {});
        if o.dumpFileStorage.is_some() {
            taskCfg.Mydumper.SourceDir = "noop://".to_string();
        }
        taskCfg.Adjust(&taskCtx)?;
        let r = (uuid::Uuid::new_v4().as_u128() & ((1u128 << 63) - 1)) as i64;
        taskCfg.TaskID = if r == 0 { 1 } else { r };

        if let Some(counter) = taskCfg.TiDB.IOTotalBytes.clone() {
            o.logger.Info("found IO total bytes counter", zap::Skip());
            let uuid = taskCfg.TiDB.UUID.clone();
            let counter = Arc::new(counter);
            mysql::RegisterDialContext(&uuid, move |_ctx, addr| {
                use std::net::TcpStream;
                let stream = TcpStream::connect(addr).map_err(errors::from_io)?;
                let _ = util::NewTCPConnWithIOCounter(
                    stream.try_clone().map_err(errors::from_io)?,
                    counter.clone(),
                );
                Ok(stream)
            });
        }
        self.run(taskCtx, taskCfg, &mut o)
    }

    pub fn run(
        &mut self,
        taskCtx: context::Context,
        taskCfg: config::Config,
        o: &mut options,
    ) -> Result<()> {
        build::LogInfo(build::Lightning);
        o.logger.Info("cfg", zap::String("cfg", taskCfg.Redact()));
        logutil::LogEnvVariables();

        unsafe {
            if split::WAIT_REGION_ONLINE_ATTEMPT_TIMES
                != taskCfg.TikvImporter.RegionCheckBackoffLimit
            {
                split::WAIT_REGION_ONLINE_ATTEMPT_TIMES =
                    taskCfg.TikvImporter.RegionCheckBackoffLimit;
            }
        }

        let factory = o
            .promFactory
            .clone()
            .unwrap_or_else(promutil::NewDefaultFactory);
        let registry = o
            .promRegistry
            .clone()
            .unwrap_or_else(promutil::NewDefaultRegistry);
        let metrics = metric::NewMetrics(&factory);
        metrics.RegisterTo(&registry);
        self.metrics = Some(metrics.clone());

        let mut ctx = metric::WithMetric(taskCtx, metrics.clone());
        ctx = logutil::WithLogger(ctx, o.logger.Logger.clone());
        let (ctx, cancel) = context::WithCancel(ctx);
        {
            let _g = self.cancelLock.lock().unwrap();
            self.cancel = Some(cancel.clone());
            self.curTask = Some(taskCfg.clone());
        }
        self.refreshHttpState();
        astersql_lightning_pkg_progress::BroadcastStartTask();

        let run_result = (|| -> Result<()> {
            failpoint::Inject("SkipRunTask", || {});
            taskCfg
                .TiDB
                .Security
                .BuildTLSConfig()
                .map_err(|e| common::ErrInvalidTLSConfig.Wrap(e))?;

            let mut dbMetas: Vec<mydump::MDDatabaseMeta> = Vec::new();
            let mut s = o.dumpFileStorage.clone();
            if taskCfg.TikvImporter.Backend != config::BackendImportInto {
                let (mdl, store) = self.initDataSource(&ctx, &taskCfg, o)?;
                s = Some(store);
                dbMetas = mdl.GetDatabases();
                let prog_dbs = bridges::progress_dbs(&dbMetas);
                astersql_lightning_pkg_progress::BroadcastInitProgress(&prog_dbs);
            }

            let (db, keyspaceName) = initDBAndKeyspace(&ctx, &taskCfg, o)?;
            let param = ControllerParamLocal {
                DBMetas: dbMetas,
                Status: self.status.clone(),
                DumpFileStorage: s,
                OwnExtStorage: o.dumpFileStorage.is_none(),
                DB: db,
                CheckpointStorage: o.checkpointStorage.clone(),
                CheckpointName: o.checkpointName.clone(),
                DupIndicator: o.dupIndicator.clone(),
                KeyspaceName: keyspaceName,
            };

            let procedure = newImporter(&ctx, &taskCfg, &param).map_err(|err| {
                o.logger.Error("restore failed", log::ShortError(&err));
                errors::Trace(err)
            })?;
            {
                let _g = self.cancelLock.lock().unwrap();
                self.importer = Some(Arc::new(Mutex::new(procedure)));
            }
            self.refreshHttpState();
            let result = {
                let imp = self.importer.as_ref().unwrap().clone();
                let mut g = imp.lock().unwrap();
                g.Run(&ctx).map_err(errors::Trace)
            };
            {
                let imp = self.importer.as_ref().unwrap().clone();
                let mut g = imp.lock().unwrap();
                g.Close();
            }
            result
        })();

        cancel();
        {
            let _g = self.cancelLock.lock().unwrap();
            self.cancel = None;
            self.importer = None;
        }
        self.refreshHttpState();
        let prog_err = run_result
            .as_ref()
            .err()
            .map(|e| astersql_lightning_pkg_progress::errors::New(e.Error()));
        astersql_lightning_pkg_progress::BroadcastEndTask(prog_err.as_ref());
        metrics.UnregisterFrom(&registry);
        run_result
    }

    pub fn initDataSource(
        &self,
        ctx: &context::Context,
        taskCfg: &config::Config,
        o: &options,
    ) -> Result<(mydump::MDLoader, storeapi::StorageRef)> {
        let mut s = o.dumpFileStorage.clone();
        if s.is_none() {
            let u = objstore::ParseBackend(&taskCfg.Mydumper.SourceDir, None)
                .map_err(common::NormalizeError)?;
            s = Some(
                objstore::New(ctx.clone(), u, &storeapi::Options {})
                    .map_err(common::NormalizeError)?,
            );
        }
        let store = s.unwrap();
        let expectedErr = errors::New("Stop Iter");
        let walkErr = match store.WalkDir(
            ctx,
            &storeapi::WalkOption { ListCount: 1 },
            &mut |_name, _size| Err(expectedErr.clone()),
        ) {
            Ok(()) => None,
            Err(e) => Some(e),
        };
        if !matches!(&walkErr, Some(e) if errors::ErrorEqual(e, &expectedErr)) {
            if walkErr.is_none() {
                return Err(
                    common::ErrEmptySourceDir.GenWithStackByArgs(&taskCfg.Mydumper.SourceDir)
                );
            }
            return Err(common::NormalizeOrWrapLazy(
                &common::ErrStorageUnknown,
                walkErr.unwrap(),
            ));
        }

        let loadTask = o.logger.Begin(zapcore::InfoLevel, "load data source");
        let region_conc = self
            .curTask
            .as_ref()
            .map(|c| c.App.RegionConcurrency)
            .unwrap_or(taskCfg.App.RegionConcurrency);
        let mdl = mydump::NewLoaderWithStore(
            ctx,
            mydump::NewLoaderCfg(taskCfg),
            store.clone(),
            mydump::WithScanFileConcurrency(region_conc * 2),
        )
        .map_err(errors::Trace)?;
        loadTask.EndSimple(zapcore::ErrorLevel, None);

        checkSystemRequirement(taskCfg, &mdl.GetDatabases()).map_err(|err| {
            o.logger
                .Error("check system requirements failed", zap::Error(&err));
            common::ErrSystemRequirementNotMet
                .Wrap(err)
                .GenWithStackByArgs("")
        })?;
        checkSchemaConflict(taskCfg, &mdl.GetDatabases()).map_err(|err| {
            o.logger.Error(
                "checkpoint schema conflicts with data files",
                zap::Error(&err),
            );
            errors::Trace(err)
        })?;
        Ok((mdl, store))
    }

    /// Stop stops the lightning server.
    pub fn Stop(&mut self) {
        {
            let _g = self.cancelLock.lock().unwrap();
            if let Some(cancel) = self.cancel.take() {
                self.taskCanceled = true;
                cancel();
            }
        }
        self.refreshHttpState();
        if let Err(err) = self.server.Shutdown(&self.ctx) {
            log::L().Warn("failed to shutdown HTTP server", log::ShortError(err));
        }
        (self.shutdown)();
    }

    pub fn TaskCanceled(&self) -> bool {
        let _g = self.cancelLock.lock().unwrap();
        self.taskCanceled
    }

    pub fn Status(&self) -> (i64, i64) {
        (
            self.status.FinishedFileSize.Load(),
            self.status.TotalFileSize.Load(),
        )
    }

    pub fn Metrics(&self) -> Option<metric::Metrics> {
        self.metrics.clone()
    }
}

pub fn getKeyspaceName(db: Option<&sql::DB>) -> Result<String> {
    let Some(db) = db else {
        return Ok(String::new());
    };
    let mut rows = db.Query("show config where Type = 'tidb' and name = 'keyspace-name'")?;
    let mut value = String::new();
    if rows.Next() {
        let mut _type = String::new();
        let mut _instance = String::new();
        let mut _name = String::new();
        rows.Scan(&mut _type, &mut _instance, &mut _name, &mut value)?;
    }
    rows.Close();
    rows.Err()?;
    Ok(value)
}

pub fn initDBAndKeyspace(
    ctx: &context::Context,
    taskCfg: &config::Config,
    o: &options,
) -> Result<(Option<sql::DB>, String)> {
    let mut db = o.db.clone();
    if db.is_none() {
        db = Some(
            crate::DBFromConfigLocal(ctx, &taskCfg.TiDB)
                .map_err(|e| common::ErrDBConnect.Wrap(e))?,
        );
    }
    let mut keyspaceName = String::new();
    if taskCfg.TikvImporter.Backend == config::BackendLocal {
        keyspaceName = taskCfg.TikvImporter.KeyspaceName.clone();
        if keyspaceName.is_empty() {
            match getKeyspaceName(db.as_ref()) {
                Ok(name) => keyspaceName = name,
                Err(err) if common::IsAccessDeniedNeedConfigPrivilegeError(&err) => {
                    o.logger.Info(
                        "keyspace is unspecified and target user has no config privilege, assuming dedicated cluster",
                        zap::Skip(),
                    );
                }
                Err(err) => {
                    o.logger.Warn(
                        "unable to get keyspace name, lightning will use empty keyspace name",
                        zap::Error(err),
                    );
                }
            }
        }
        o.logger.Info(
            "acquired keyspace name",
            zap::String("keyspaceName", &keyspaceName),
        );
    }
    Ok((db, keyspaceName))
}

pub fn writeJSONError(w: &mut http::ResponseWriter, code: i32, prefix: &str, err: Option<Error>) {
    #[derive(serde::Serialize)]
    struct errorResponse {
        error: String,
    }
    w.WriteHeader(code);
    let mut msg = prefix.to_string();
    if let Some(err) = err {
        msg.push_str(": ");
        msg.push_str(&err.Error());
    }
    let _ = json::NewEncoder(w).Encode(&errorResponse { error: msg });
}

/// parseTaskID parses `/<id>` or `/<id>/<verb>` from a stripped path.
pub fn parseTaskID(req: &http::Request) -> Result<(i64, String)> {
    // Go's strings.TrimPrefix removes at most one slash. Keeping an additional
    // leading slash makes the numeric component empty instead of accepting a
    // malformed path such as `//42` as task 42.
    let path = req.URL.Path.strip_prefix('/').unwrap_or(&req.URL.Path);
    let (taskIDString, verb) = match path.split_once('/') {
        Some((id, v)) => (id, v.to_string()),
        None => (path, String::new()),
    };
    if taskIDString.is_empty() {
        return Err(Error {
            msg: "invalid syntax".into(),
            not_found: false,
            cause: None,
            class: None,
            empty_num: true,
        });
    }
    let taskID = taskIDString.parse::<i64>().map_err(|e| Error {
        msg: e.to_string(),
        not_found: false,
        cause: None,
        class: None,
        empty_num: false,
    })?;
    Ok((taskID, verb))
}

fn handle_task_http(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    w.Header().Set("Content-Type", "application/json");
    match req.Method.as_str() {
        http::MethodGet => match parseTaskID(req) {
            Err(err) if err.IsEmptyNumError() => handle_get_task(state, w),
            Ok((taskID, _)) => handle_get_one_task(state, w, req, taskID),
            Err(err) => writeJSONError(w, http::StatusBadRequest, "invalid task ID", Some(err)),
        },
        http::MethodPost => handle_post_task(state, w, req),
        http::MethodDelete => handle_delete_one_task(state, w, req),
        http::MethodPatch => handle_patch_one_task(state, w, req),
        _ => {
            w.Header().Set("Allow", "GET, POST, DELETE, PATCH");
            writeJSONError(
                w,
                http::StatusMethodNotAllowed,
                "only GET, POST, DELETE and PATCH are allowed",
                None,
            );
        }
    }
}

fn handle_get_task(state: &Arc<Mutex<HttpState>>, w: &mut http::ResponseWriter) {
    #[derive(serde::Serialize)]
    struct response {
        current: Option<i64>,
        queue: Vec<i64>,
    }
    let g = state.lock().unwrap();
    let queued = g.taskCfgs.as_ref().map(|q| q.AllIDs()).unwrap_or_default();
    let current = if g.cancel.is_some() {
        g.curTask.as_ref().map(|t| t.TaskID)
    } else {
        None
    };
    w.WriteHeader(http::StatusOK);
    let _ = json::NewEncoder(w).Encode(&response {
        current,
        queue: queued,
    });
}

fn handle_get_one_task(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
    taskID: i64,
) {
    let g = state.lock().unwrap();
    let mut task = g.curTask.as_ref().filter(|c| c.TaskID == taskID).cloned();
    if task.is_none() {
        task = g.taskCfgs.as_ref().and_then(|q| q.Get(taskID));
    }
    drop(g);
    let Some(task) = task else {
        writeJSONError(w, http::StatusNotFound, "task ID not found", None);
        return;
    };
    match json::Marshal(&task) {
        Ok(data) => writeBytesCompressed(w, req, data),
        Err(err) => writeJSONError(
            w,
            http::StatusInternalServerError,
            "unable to serialize task",
            Some(err),
        ),
    }
}

fn handle_post_task(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    w.Header().Set("Cache-Control", "no-store");
    let mut g = state.lock().unwrap();
    if g.taskCfgs.is_none() {
        writeJSONError(
            w,
            http::StatusNotImplemented,
            "server-mode not enabled",
            None,
        );
        return;
    }
    #[derive(serde::Serialize)]
    struct taskResponse {
        id: i64,
    }
    log::L().Info("received task config", zap::Skip());
    let mut cfg = config::Config::NewConfig();
    if let Err(err) = cfg.LoadFromGlobal(&g.globalCfg) {
        writeJSONError(
            w,
            http::StatusInternalServerError,
            "cannot restore from global config",
            Some(err),
        );
        return;
    }
    if let Err(err) = cfg.LoadFromTOML(&req.Body) {
        writeJSONError(
            w,
            http::StatusBadRequest,
            "cannot parse task (must be TOML)",
            Some(err),
        );
        return;
    }
    if let Err(err) = cfg.Adjust(&g.ctx) {
        writeJSONError(
            w,
            http::StatusBadRequest,
            "invalid task configuration",
            Some(err),
        );
        return;
    }
    let id = cfg.TaskID;
    g.taskCfgs.as_ref().unwrap().Push(cfg);
    w.WriteHeader(http::StatusOK);
    let _ = json::NewEncoder(w).Encode(&taskResponse { id });
}

fn handle_delete_one_task(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    w.Header().Set("Content-Type", "application/json");
    let (taskID, _) = match parseTaskID(req) {
        Ok(v) => v,
        Err(err) => {
            writeJSONError(w, http::StatusBadRequest, "invalid task ID", Some(err));
            return;
        }
    };
    let mut g = state.lock().unwrap();
    let mut cancel: Option<context::CancelFunc> = None;
    if g.cancel.is_some()
        && g.curTask
            .as_ref()
            .map(|t| t.TaskID == taskID)
            .unwrap_or(false)
    {
        cancel = g.cancel.take();
    }
    let cancelSuccess = if let Some(cancel) = cancel {
        cancel();
        true
    } else if let Some(q) = g.taskCfgs.as_ref() {
        q.Remove(taskID)
    } else {
        false
    };
    log::L().Info("canceled task", zap::Int64("taskID", taskID));
    if cancelSuccess {
        w.WriteHeader(http::StatusOK);
        let _ = w.Write(b"{}");
    } else {
        writeJSONError(w, http::StatusNotFound, "task ID not found", None);
    }
}

fn handle_patch_one_task(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    let g = state.lock().unwrap();
    if g.taskCfgs.is_none() {
        writeJSONError(
            w,
            http::StatusNotImplemented,
            "server-mode not enabled",
            None,
        );
        return;
    }
    let (taskID, verb) = match parseTaskID(req) {
        Ok(v) => v,
        Err(err) => {
            writeJSONError(w, http::StatusBadRequest, "invalid task ID", Some(err));
            return;
        }
    };
    let moveSuccess = match verb.as_str() {
        "front" => g.taskCfgs.as_ref().unwrap().MoveToFront(taskID),
        "back" => g.taskCfgs.as_ref().unwrap().MoveToBack(taskID),
        _ => {
            writeJSONError(w, http::StatusBadRequest, "unknown patch action", None);
            return;
        }
    };
    if moveSuccess {
        w.WriteHeader(http::StatusOK);
        let _ = w.Write(b"{}");
    } else {
        writeJSONError(w, http::StatusNotFound, "task ID not found", None);
    }
}

fn handle_pause_http(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    w.Header().Set("Content-Type", "application/json");
    match req.Method.as_str() {
        http::MethodGet => {
            w.WriteHeader(http::StatusOK);
            let body = format!(r#"{{"paused":{}}}"#, DeliverPauser::IsPaused());
            let _ = w.Write(body.as_bytes());
        }
        http::MethodPut => {
            w.WriteHeader(http::StatusOK);
            let imp = state.lock().unwrap().importer.clone();
            if let Some(imp) = imp {
                if let Err(err) = imp.lock().unwrap().Pause(&req.Context()) {
                    log::L().Error("failed to pause", zap::Error(&err));
                    writeJSONError(
                        w,
                        http::StatusInternalServerError,
                        "failed to pause",
                        Some(err),
                    );
                    return;
                }
                log::L().Info("progress paused", zap::Skip());
            } else {
                DeliverPauser::Pause();
                log::L().Info("progress paused", zap::Skip());
            }
            let _ = w.Write(b"{}");
        }
        _ => {
            w.Header().Set("Allow", "GET, PUT");
            writeJSONError(
                w,
                http::StatusMethodNotAllowed,
                "only GET and PUT are allowed",
                None,
            );
        }
    }
}

fn handle_resume_http(
    state: &Arc<Mutex<HttpState>>,
    w: &mut http::ResponseWriter,
    req: &http::Request,
) {
    w.Header().Set("Content-Type", "application/json");
    match req.Method.as_str() {
        http::MethodPut => {
            w.WriteHeader(http::StatusOK);
            let imp = state.lock().unwrap().importer.clone();
            if let Some(imp) = imp {
                if let Err(err) = imp.lock().unwrap().Resume(&req.Context()) {
                    log::L().Error("failed to resume", zap::Error(&err));
                    writeJSONError(
                        w,
                        http::StatusInternalServerError,
                        "failed to resume",
                        Some(err),
                    );
                    return;
                }
                log::L().Info("progress resumed", zap::Skip());
            } else {
                DeliverPauser::Resume();
                log::L().Info("progress resumed", zap::Skip());
            }
            let _ = w.Write(b"{}");
        }
        _ => {
            w.Header().Set("Allow", "PUT");
            writeJSONError(w, http::StatusMethodNotAllowed, "only PUT is allowed", None);
        }
    }
}

pub fn writeBytesCompressed(w: &mut http::ResponseWriter, req: &http::Request, b: Vec<u8>) {
    if !req.Header.Get("Accept-Encoding").contains("gzip") {
        let _ = w.Write(&b);
        return;
    }
    w.Header().Set("Content-Encoding", "gzip");
    w.WriteHeader(http::StatusOK);
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut gw = gzip::NewWriterLevel(&mut buf, gzip::BestSpeed).unwrap();
        let _ = gw.Write(&b);
        let _ = gw.Close();
    }
    let _ = w.Write(&buf);
}

pub fn handleProgressTask(w: &mut http::ResponseWriter, req: &http::Request) {
    w.Header().Set("Content-Type", "application/json");
    match astersql_lightning_pkg_progress::MarshalTaskProgress() {
        Ok(res) => writeBytesCompressed(w, req, res),
        Err(err) => {
            w.WriteHeader(http::StatusInternalServerError);
            let _ = json::NewEncoder(w).Encode(&err.to_string());
        }
    }
}

pub fn handleProgressTable(w: &mut http::ResponseWriter, req: &http::Request) {
    w.Header().Set("Content-Type", "application/json");
    let tableName = req.URL.Query().Get("t");
    match astersql_lightning_pkg_progress::MarshalTableCheckpoints(&tableName) {
        Ok(res) => writeBytesCompressed(w, req, res),
        Err(err) => {
            if err.not_found {
                w.WriteHeader(http::StatusNotFound);
            } else {
                w.WriteHeader(http::StatusInternalServerError);
            }
            let _ = json::NewEncoder(w).Encode(&err.to_string());
        }
    }
}

pub fn handleLogLevel(w: &mut http::ResponseWriter, req: &http::Request) {
    w.Header().Set("Content-Type", "application/json");
    #[derive(serde::Serialize, serde::Deserialize)]
    struct logLevel {
        level: zapcore::Level,
    }
    match req.Method.as_str() {
        http::MethodGet => {
            let body = logLevel {
                level: log::Level(),
            };
            w.WriteHeader(http::StatusOK);
            let _ = json::NewEncoder(w).Encode(&body);
        }
        http::MethodPut | http::MethodPost => {
            let mut body = logLevel {
                level: zapcore::InfoLevel,
            };
            body = match json::NewDecoder(&req.Body).Decode() {
                Ok(v) => v,
                Err(err) => {
                    writeJSONError(w, http::StatusBadRequest, "invalid log level", Some(err));
                    return;
                }
            };
            let oldLevel = log::SetLevel(zapcore::InfoLevel);
            log::L().Info(
                "changed log level. No effects if task has specified its logger",
                zap::Stringer("old", oldLevel),
            );
            log::SetLevel(body.level);
            w.WriteHeader(http::StatusOK);
            let _ = w.Write(b"{}");
        }
        _ => {
            w.Header().Set("Allow", "GET, PUT, POST");
            writeJSONError(
                w,
                http::StatusMethodNotAllowed,
                "only GET, PUT and POST are allowed",
                None,
            );
        }
    }
}

/// checkSystemRequirement matches Go open-files estimate for local backend.
pub fn checkSystemRequirement(
    cfg: &config::Config,
    dbsMeta: &[mydump::MDDatabaseMeta],
) -> Result<()> {
    if cfg.TikvImporter.Backend == config::BackendLocal {
        let mut tableTotalSizes: Vec<i64> = Vec::new();
        for dbs in dbsMeta {
            for tb in &dbs.Tables {
                tableTotalSizes.push(tb.TotalSize);
            }
        }
        tableTotalSizes.sort_by(|i, j| j.cmp(i));
        let mut topNTotalSize: i64 = 0;
        let n = tableTotalSizes
            .len()
            .min(cfg.App.TableConcurrency.max(0) as usize);
        for size in tableTotalSizes.iter().take(n) {
            topNTotalSize += *size;
        }
        let mem = cfg.TikvImporter.LocalWriterMemCacheSize.max(1);
        let maxDBFiles = topNTotalSize / mem * 2;
        let maxOpenDBFiles = maxDBFiles * (1 + cfg.TikvImporter.RangeConcurrency as i64);
        let estimateMaxFiles = cfg.App.RegionConcurrency as u64 + maxOpenDBFiles.max(0) as u64;
        ingestctrl::VerifyRLimit(estimateMaxFiles)?;
    }
    Ok(())
}

/// checkSchemaConflict matches Go checkpoint/schema conflict check.
pub fn checkSchemaConflict(cfg: &config::Config, dbsMeta: &[mydump::MDDatabaseMeta]) -> Result<()> {
    if cfg.Checkpoint.Enable && cfg.Checkpoint.Driver == config::CheckpointDriverMySQL {
        for db in dbsMeta {
            if db.Name == cfg.Checkpoint.Schema {
                for tb in &db.Tables {
                    if astersql_lightning_pkg_checkpoints::IsCheckpointTable(&tb.Name) {
                        return Err(common::ErrCheckpointSchemaConflict.GenWithStack(format!(
                            "checkpoint table `{}`.`{}` conflict with data files. Please change the `checkpoint.schema` config or set `checkpoint.driver` to \"file\" instead",
                            db.Name, tb.Name
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

/// SwitchMode switches the mode of the TiKV cluster.
pub fn SwitchMode(
    ctx: &context::Context,
    cli: pdhttp::Client,
    tls_cfg: &tls::Config,
    mode: &str,
    ranges: Vec<import_sstpb::Range>,
) -> Result<()> {
    let m = match mode {
        config::ImportMode => import_sstpb::SwitchMode_Import,
        config::NormalMode => import_sstpb::SwitchMode_Normal,
        _ => {
            return Err(errors::Errorf(format!(
                "invalid mode {mode}, must use {} or {}",
                config::ImportMode,
                config::NormalMode
            )));
        }
    };
    tikv::ForAllStores(ctx, cli, metapb::StoreState_Offline, |c, store| {
        tikv::SwitchMode(c, tls_cfg, &store.Address, m, &ranges)
    })
}

/// LightningImporter abstracts import backends.
pub trait LightningImporter: Send {
    fn Run(&mut self, ctx: &context::Context) -> Result<()>;
    fn Pause(&mut self, ctx: &context::Context) -> Result<()>;
    fn Resume(&mut self, ctx: &context::Context) -> Result<()>;
    fn Close(&mut self);
}

#[derive(Clone)]
pub struct ControllerParamLocal {
    pub DBMetas: Vec<mydump::MDDatabaseMeta>,
    pub Status: Arc<LightningStatus>,
    pub DumpFileStorage: Option<storeapi::StorageRef>,
    pub OwnExtStorage: bool,
    pub DB: Option<sql::DB>,
    pub CheckpointStorage: Option<storeapi::StorageRef>,
    pub CheckpointName: String,
    pub DupIndicator: Option<atomic::Bool>,
    pub KeyspaceName: String,
}

/// Legacy local/tidb importer adapter.
///
/// `importer::Controller` is not `Send` (checkpoint trait objects), so we invoke
/// `NewImportController` + `Run` + `Close` inside `Run` on the calling thread —
/// matching Go's single-task ownership without crossing threads.
struct LegacyImporter {
    cfg: config::Config,
    param: ControllerParamLocal,
    pauser: common::Pauser,
    closed: bool,
}

impl LightningImporter for LegacyImporter {
    fn Run(&mut self, _ctx: &context::Context) -> Result<()> {
        let imp_cfg = bridges::to_importer_cfg(&self.cfg);
        let imp_ctx = astersql_lightning_pkg_importer::context::Background();
        let cparam = astersql_lightning_pkg_importer::ControllerParam {
            DBMetas: self
                .param
                .DBMetas
                .iter()
                .map(
                    |d| astersql_lightning_pkg_importer::mydump::MDDatabaseMeta {
                        Name: d.Name.clone(),
                        Tables: d
                            .Tables
                            .iter()
                            .map(|t| astersql_lightning_pkg_importer::mydump::MDTableMeta {
                                Name: t.Name.clone(),
                                TotalSize: t.TotalSize,
                                ..Default::default()
                            })
                            .collect(),
                        ..Default::default()
                    },
                )
                .collect(),
            Status: Some(Arc::new(astersql_lightning_pkg_importer::LightningStatus {
                backend: self.cfg.TikvImporter.Backend.clone(),
                FinishedFileSize: astersql_lightning_pkg_importer::atomic::NewInt64(
                    self.param.Status.FinishedFileSize.Load(),
                ),
                TotalFileSize: astersql_lightning_pkg_importer::atomic::NewInt64(
                    self.param.Status.TotalFileSize.Load(),
                ),
            })),
            DumpFileStorage: astersql_lightning_pkg_importer::storeapi::Storage::default(),
            OwnExtStorage: self.param.OwnExtStorage,
            Pauser: Some(astersql_lightning_pkg_importer::common::NewPauser()),
            DB: Some(astersql_lightning_pkg_importer::sql::DB::new_memory()),
            CheckpointStorage: None,
            CheckpointName: self.param.CheckpointName.clone(),
            DupIndicator: None,
            KeyspaceName: self.param.KeyspaceName.clone(),
            ResourceGroupName: String::new(),
            TaskType: String::new(),
        };
        let mut controller =
            astersql_lightning_pkg_importer::NewImportController(imp_ctx.clone(), &imp_cfg, cparam)
                .map_err(bridges::map_err_imp)?;
        let result = controller.Run(imp_ctx).map_err(bridges::map_err_imp);
        controller.Close();
        result
    }
    fn Pause(&mut self, _ctx: &context::Context) -> Result<()> {
        self.pauser.Pause();
        DeliverPauser::Pause();
        Ok(())
    }
    fn Resume(&mut self, _ctx: &context::Context) -> Result<()> {
        self.pauser.Resume();
        DeliverPauser::Resume();
        Ok(())
    }
    fn Close(&mut self) {
        self.closed = true;
    }
}

struct ImportIntoImporter {
    cfg: config::Config,
    db: sql::DB,
    status: Arc<LightningStatus>,
    closed: bool,
}

impl LightningImporter for ImportIntoImporter {
    fn Run(&mut self, _ctx: &context::Context) -> Result<()> {
        let mut ii_cfg = bridges::to_importinto_cfg(&self.cfg);
        ii_cfg.App.CheckRequirements = false;
        let ii_ctx = astersql_lightning_pkg_importinto::context::Background();
        let ii_db = astersql_lightning_pkg_importinto::sql::DB::new_memory();
        let status = self.status.clone();
        struct Adapter {
            s: Arc<LightningStatus>,
        }
        impl astersql_lightning_pkg_importinto::ProgressUpdater for Adapter {
            fn UpdateTotalSize(&self, size: i64) {
                self.s.TotalFileSize.Store(size);
            }
            fn UpdateFinishedSize(&self, size: i64) {
                self.s.FinishedFileSize.Store(size);
            }
        }
        let opts = vec![astersql_lightning_pkg_importinto::WithProgressUpdater(
            Arc::new(Adapter { s: status }),
        )];
        let imp = astersql_lightning_pkg_importinto::NewImporter(&ii_ctx, ii_cfg, ii_db, opts)
            .map_err(bridges::map_err_ii)?;
        let result = imp.Run(&ii_ctx).map_err(bridges::map_err_ii);
        imp.Close();
        result
    }
    fn Pause(&mut self, _ctx: &context::Context) -> Result<()> {
        log::L().Info(
            "pause is not supported for 'import into' backend",
            zap::Skip(),
        );
        Ok(())
    }
    fn Resume(&mut self, _ctx: &context::Context) -> Result<()> {
        log::L().Info(
            "resume is not supported for 'import into' backend",
            zap::Skip(),
        );
        Ok(())
    }
    fn Close(&mut self) {
        self.closed = true;
        let _ = self.db.Close();
    }
}

/// newImporter creates a LightningImporter based on configuration.
pub fn newImporter(
    _ctx: &context::Context,
    cfg: &config::Config,
    param: &ControllerParamLocal,
) -> Result<Box<dyn LightningImporter>> {
    if matches!(
        cfg.TikvImporter.Backend.as_str(),
        config::BackendLocal | config::BackendTiDB
    ) && cfg.Checkpoint.Enable
    {
        if !matches!(
            cfg.Checkpoint.Driver.as_str(),
            config::CheckpointDriverMySQL | config::CheckpointDriverFile
        ) {
            return Err(errors::New(format!(
                "[Lightning:Checkpoint:ErrUnknownCheckpointDriver]unknown checkpoint driver '{}'",
                cfg.Checkpoint.Driver
            )));
        }
        // Go's NewImportController opens the checkpoint DB during
        // construction, so an unknown driver or unusable DSN must fail before
        // precheck/run begins.
        let cp_cfg = bridges::to_checkpoints_cfg(cfg);
        let mut checkpoint = astersql_lightning_pkg_checkpoints::OpenCheckpointsDB(
            astersql_lightning_pkg_checkpoints::context::Background(),
            &cp_cfg,
        )
        .map_err(bridges::map_err_cp)
        .map_err(errors::Trace)?;
        checkpoint
            .Close()
            .map_err(bridges::map_err_cp)
            .map_err(errors::Trace)?;
    }
    match cfg.TikvImporter.Backend.as_str() {
        config::BackendImportInto => Ok(Box::new(ImportIntoImporter {
            cfg: cfg.clone(),
            db: param.DB.clone().unwrap_or_else(sql::DB::new_memory),
            status: param.Status.clone(),
            closed: false,
        })),
        config::BackendLocal | config::BackendTiDB => {
            // Constructability check — same errors as Go NewImportController.
            let imp_cfg = bridges::to_importer_cfg(cfg);
            let imp_ctx = astersql_lightning_pkg_importer::context::Background();
            let cparam = astersql_lightning_pkg_importer::ControllerParam {
                DBMetas: vec![],
                Status: None,
                DumpFileStorage: astersql_lightning_pkg_importer::storeapi::Storage::default(),
                OwnExtStorage: param.OwnExtStorage,
                Pauser: None,
                DB: Some(astersql_lightning_pkg_importer::sql::DB::new_memory()),
                CheckpointStorage: None,
                CheckpointName: param.CheckpointName.clone(),
                DupIndicator: None,
                KeyspaceName: param.KeyspaceName.clone(),
                ResourceGroupName: String::new(),
                TaskType: String::new(),
            };
            let mut probe =
                astersql_lightning_pkg_importer::NewImportController(imp_ctx, &imp_cfg, cparam)
                    .map_err(bridges::map_err_imp)?;
            probe.Close();
            Ok(Box::new(LegacyImporter {
                cfg: cfg.clone(),
                param: ControllerParamLocal {
                    DBMetas: param.DBMetas.clone(),
                    Status: param.Status.clone(),
                    DumpFileStorage: param.DumpFileStorage.clone(),
                    OwnExtStorage: param.OwnExtStorage,
                    DB: param.DB.clone(),
                    CheckpointStorage: param.CheckpointStorage.clone(),
                    CheckpointName: param.CheckpointName.clone(),
                    DupIndicator: param.DupIndicator.clone(),
                    KeyspaceName: param.KeyspaceName.clone(),
                },
                pauser: deliver_pauser().clone(),
                closed: false,
            }))
        }
        other => Err(errors::Errorf(format!("unknown backend {other}"))),
    }
}
