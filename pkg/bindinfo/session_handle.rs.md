# `pkg/bindinfo/session_handle.rs`

## 文件定位

本文件属于 `astersql-bindinfo` crate 的会话级 SQL Plan Binding 边界。`pkg/bindinfo/lib.rs` 以私有模块 `session_handle` 装配它，再通过 `pub use session_handle::*` 对外导出其公开符号；`pkg/bindinfo/Cargo.toml` 则表明该 crate 直接依赖 `astersql-parser`、`serde` 与 `serde_json`，其中本文件实际使用前者计算规范化 SQL 摘要，使用后两者保存和恢复会话状态。

它与全局绑定句柄不同：`NewSessionBindingHandle` 创建的是进程内、句柄私有的缓存，不访问绑定系统表，也没有后台刷新逻辑。当前 Rust 仓库中，生产代码尚未创建 `NewSessionBindingHandle` 或通过 `SessionBindInfoKeyType` 将其接入会话上下文；除本文件自身外，这些符号的 Rust 使用者仅见 `pkg/bindinfo/session_handle_test.rs`。因此这里已经具备可执行、已单测的会话绑定 API，但不能据此断言完整 Rust SQL 会话主链已经接线。RustCodeGraph 曾把 `pkg/server/driver_tidb.rs::DecodeSessionStates` 识别为同名调用者；源码复核显示该方法只恢复 prepared statements，使用的也不是本文件的 `SessionStates`，不是本模块的实际入口。

## 核心职责

- `SessionBindingHandle` 定义会话绑定的创建、按摘要删除、跨库匹配、枚举、会话状态编解码和关闭接口。
- `sessionBindingHandle` 组合 `Arc<dyn BindingCache>` 与外层 `RwLock<()>`：缓存负责摘要索引、绑定存储和跨库候选匹配，外层锁负责把一批句柄操作串成一致的临界区。
- `CreateSessionBinding` 在写入前调用 `prepareHints`，填充可匹配所需的 Hint、ID 和表名，再规范化数据库名、刷新创建/更新时间，并以 `DigestNormalized(OriginalSQL)` 为键覆盖缓存。
- `EncodeSessionStates`/`DecodeSessionStates` 在 JSON 字符串与 `Vec<Binding>` 之间转换，并兼容 v8.0.0 之前带外层 `OriginalSQL`、`Db`、`Bindings` 的旧格式。
- `sessionBindInfoKeyType`、`SessionBindInfoKeyType` 及两个字符串函数保留 Go 会话上下文键的接口形状。

上述职责可由 `SessionBindingHandle` trait、其 `impl`、`decodeOldStyleSessionStates` 和 `pkg/bindinfo/binding_cache.rs::BindingCache` 直接核验。

## 主要符号

- `SessionStates { Bindings: String }`：本文件自有的可序列化载体。字段在 JSON 外层命名为 `bindings`，空字符串通过 `skip_serializing_if` 省略；字段内容本身又是一段绑定数组 JSON。
- `SessionBindingHandle: Send + Sync`：公开对象安全 trait。方法签名使用 `&self`，调用者通常持有 `Arc<dyn SessionBindingHandle>`；`BindingValidator` 由会话侧提供 SQL/Hint 校验能力。
- `sessionBindingHandle`：默认实现，字段 `cache` 保存会话绑定，`operation_lock` 保护批量操作。类型本身被公开，但字段私有，正常构造入口是 `NewSessionBindingHandle`。
- `NewSessionBindingHandle() -> Arc<dyn SessionBindingHandle>`：用 `newBindingCache(i64::MAX)` 构造近似不受配额限制的会话缓存。
- `CreateSessionBinding`：准备 Hint，转小写 `Db`，把 `CreateTime` 与 `UpdateTime` 同时设为 `BindingTime::now()`，再按原 SQL 的规范化摘要写入或替换绑定。
- `DropSessionBinding`：在一个写临界区内逐一调用 `BindingCache::RemoveBinding`；不存在的摘要是幂等空操作。
- `MatchSessionBinding`：把 `current_db`、`noDBDigest`、`tableNames` 传给 `BindingCache::MatchingBinding`，返回 `(Option<Arc<Binding>>, bool)`。
- `GetAllSessionBindings`：返回缓存快照。默认缓存按 `Binding.SQLDigest` 对返回值排序，调用者得到的是共享只读 `Arc`，不是可变借用。
- `EncodeSessionStates`：克隆每条 `Binding` 后序列化；空缓存不改写目标 `SessionStates`。
- `DecodeSessionStates`：识别新旧格式、重新执行 `prepareHints`，然后按 `OriginalSQL` 摘要合并/覆盖到现有缓存；它不会先清空已有绑定。
- `decodeOldStyleSessionStates`：私有兼容函数，把每个旧记录的外层原 SQL 和数据库名复制到其所有内层绑定。
- `Close`：调用缓存的 `Close` 清空主表、插入队列、内存计数和摘要双向索引；对象仍可再次使用，不是不可逆的资源终止状态。
- `sessionBindInfoKeyType`、`SessionBindInfoKeyType`、`sessionBindInfoKeyType_String`、`String`：上下文键兼容符号，两个函数都固定返回 `"session_bindinfo"`。

## 执行流程

创建流程从 `CreateSessionBinding` 开始：先在持有外层写锁之前逐条调用 `prepareHints`。`prepareHints` 位于 `pkg/bindinfo/binding.rs`，会解析 `BindSQL`、恢复 Hint 文本、收集表名，并在需要时调用 `BindingValidator`；任何此阶段错误都会通过 `?` 立即返回，缓存尚未发生变化。全部准备成功后获取写锁，对每条绑定规范化 `Db`、写入同一个当前时间到创建/更新时间、计算 `DigestNormalized(OriginalSQL)`，最后调用 `SetBinding`。相同规范化原 SQL 得到相同键，后写入者覆盖前者。

匹配流程由 `MatchSessionBinding` 获取读锁并委托缓存。`pkg/bindinfo/binding_cache.rs::MatchingBinding` 先用 `noDBDigest` 的双向索引取出候选，再调用 `pkg/bindinfo/binding.rs::crossDBMatchBindings` 比较当前库和表名；只考虑启用状态的绑定，并优先通配符更少的候选。返回布尔值表示是否最终选中绑定，不能把 `Option::None` 当成一个有效命中。

导出流程由 `EncodeSessionStates` 通过 `GetAllSessionBindings` 取得快照，克隆成拥有所有权的 `Vec<Binding>`；空集合直接成功返回，否则将数组序列化进 `SessionStates.Bindings`。恢复流程先拒绝无工作量的空字符串和空数组；若首个 JSON 元素包含大小写敏感键 `Bindings`，走旧格式展开，否则直接反序列化为新格式 `Vec<Binding>`。所有恢复项先重新准备 Hint，随后在写锁内逐项计算摘要并写入缓存。

删除与关闭都持有外层写锁：前者只移除指定摘要，后者清空整个缓存。读取列表和匹配持有读锁，可以彼此并行，但会与这些批量写操作互斥。

## 数据与状态

主要持久状态位于 `BindingCache`。默认实现的主表是 `sqlDigest -> Arc<Binding>`，同时维护 `noDBDigest <-> sqlDigest` 双向索引、插入顺序与内存用量；会话句柄给它传入 `i64::MAX` 容量，因此正常会话数据不会因有限配额被主动淘汰。`SetBinding` 仍会计算绑定大小、更新索引，并通过 `noDBDigestFromBinding` 解析 `BindSQL`，所以它仍是一个可失败操作。

`Binding` 的派生状态有两类：`prepareHints` 写入 Hint、ID 与 `TableNames`；`CreateSessionBinding` 写入小写数据库名和 UTC 微秒时间戳。`BindingTime::now()` 使用系统时间相对 Unix epoch 的微秒数，不携带 Go 版本的会话时区表示。摘要键来自 `OriginalSQL`，跨库候选索引则从 `BindSQL` 和绑定表名派生，扩展时必须维持两者同步。

`SessionStates.Bindings` 是双层编码：外层 `SessionStates` 可序列化为对象，`bindings` 属性值却是字符串化的绑定数组。这是兼容 Go `sessionstates.SessionStates.Bindings` 的协议约束，不应未经迁移方案改成直接 JSON 数组。旧格式记录的外层 `OriginalSQL` 和 `Db` 会覆盖内层同名字段；新格式保留每条 `Binding` 自带的值。

## 依赖与调用关系

上游边界如下：`pkg/bindinfo/lib.rs` 将本模块公开再导出；`pkg/bindinfo/session_handle_test.rs` 是当前唯一确认的 Rust 构造与调用方，直接覆盖全部 trait 操作和上下文键字符串。仓库搜索未发现生产 Rust 代码调用 `NewSessionBindingHandle`、`CreateSessionBinding`、`MatchSessionBinding` 或 `SessionBindInfoKeyType`，所以 Rust SQL 执行器、会话上下文和迁移编排目前不属于已验证调用链。

下游关系为：

- `CreateSessionBinding`、`DecodeSessionStates` -> `pkg/bindinfo/binding.rs::prepareHints`；
- 创建与恢复 -> `astersql_parser::DigestNormalized` -> `BindingCache::SetBinding`；
- 删除 -> `BindingCache::RemoveBinding`；
- 匹配 -> `BindingCache::MatchingBinding` -> `crossDBMatchBindings`；
- 枚举与编码 -> `BindingCache::GetAllBindings`；
- 关闭 -> `BindingCache::Close`；
- 编解码错误 -> `serde_json` -> `pkg/bindinfo/lib.rs::From<serde_json::Error> for BindError`。

`pkg/bindinfo/Cargo.toml` 声明的 `astersql-util-hint` 与 `astersql-util-parser` 并非本文件直接 import，而是经 `binding.rs::prepareHints` 间接参与 Hint 解析。没有条件编译项位于本文件；独立单测由 `pkg/bindinfo/lib.rs` 中的 `#[cfg(test)] mod session_handle_test` 装配。

## 错误处理与边界

所有可恢复失败统一返回 crate 的 `Result<T> = Result<T, BindError>`。创建和恢复会传播 Hint 解析/校验、SQL 解析、摘要索引构建以及 JSON 编解码错误；删除、匹配、枚举和关闭的 trait 接口不暴露缓存锁错误。`prepareHints` 自身会捕获解析逻辑 panic 并转换成 `BindError`，但缓存内部锁使用 `expect("binding cache poisoned")`，若该锁中毒仍会 panic。

批量创建只保证“全部 Hint 准备完成后才开始写入”：测试 `create_is_atomic_and_replaces_same_normalized_sql_like_go` 验证第二条绑定在准备阶段失败时第一条不会落库。但进入写循环后，若某条 `SetBinding` 因 `noDBDigestFromBinding` 失败，之前条目已写入且不会回滚；恢复流程也有相同的逐项写入边界。因此不要把两个方法描述为具备事务性全有或全无语义。

空导出不会清空调用者已有的 `SessionStates.Bindings`，只是原样返回；典型调用应传入默认空载体。恢复空字符串或空数组是幂等成功。恢复不是 replace-all：既有摘要之外的缓存条目会继续保留。旧格式判定只检查数组第一个元素的 `Bindings` 键，混合格式、非数组、字段类型错误或非法 JSON 均按反序列化错误返回。

`DropSessionBinding` 接受调用者已经计算好的摘要，不会从 SQL 文本重新派生；传错摘要只表现为没有删除。`Close` 后没有 closed 标志，后续创建仍可重新填充缓存。上下文键类型只是 `i32` 别名而非 Rust newtype，不提供 Go 定义中“独立类型避免键碰撞”的同等级类型隔离。

## 并发与资源生命周期

`SessionBindingHandle: Send + Sync` 与返回的 `Arc<dyn SessionBindingHandle>` 允许跨线程共享。外层 `operation_lock` 的读锁覆盖匹配和枚举，写锁覆盖批量写、删除和关闭；它保证这些句柄级操作不会在一批修改中间观察缓存。Hint 准备与 JSON 反序列化在加写锁之前完成，缩短临界区并确保准备阶段错误不改变缓存。

缓存内部还有自己的 `RwLock<CacheState>` 以及摘要映射锁。锁顺序通常是“句柄外层锁 -> 缓存状态锁 -> 摘要映射锁”；`SetBinding` 在持有缓存状态写锁时更新摘要映射，扩展代码不应反向持锁后再调用句柄，以免引入锁序环。外层锁中毒时本文件选择 `PoisonError::into_inner` 继续工作；内层缓存锁中毒策略不同，会 panic，维护时应保留或有意识地统一这一差异。

绑定通过 `Arc<Binding>` 从缓存共享。删除或 `Close` 只移除缓存所有权，已经返回给调用者的 `Arc` 快照仍然有效。`EncodeSessionStates` 在读取快照后释放读锁，再克隆与序列化，因此不会长时间阻塞写入，但编码的是取得快照那一刻的集合。当前实现没有线程、异步任务、通道、文件句柄或显式析构逻辑；资源生命周期完全由 `Arc` 和缓存清理控制。

## 与 Go 版本的对应关系

直接对照文件是 `pkg/bindinfo/session_handle.go`。trait 方法总体对应 Go `SessionBindingHandle`，创建、删除、枚举、状态编解码、旧格式恢复与上下文键字符串都保留原意；`pkg/bindinfo/session_handle_test.go` 进一步证明 Go 主链支持创建/覆盖/删除 session binding、session 优先于 global binding、跨默认库匹配、SHOW 展示和 prepared statement 应用。

关键差异如下：

- Go `sessionBindingHandle` 直接持有 `map[string]*Binding`，匹配时遍历 map；Rust 复用 `BindingCache` 及 no-db 摘要索引，列表还会按 `SQLDigest` 排序。
- Go 在持有 `sync.RWMutex` 写锁期间执行 `prepareHints`；Rust 在加外层锁前完成整批准备，减少锁时长，同时维持“准备失败前不写缓存”的测试意图。
- Go 创建时间使用会话 statement context 的时区和毫秒精度 `types.Time`；Rust 使用无时区的 UTC 微秒整数 `BindingTime`。这是表示层差异，跨语言序列化兼容性必须由实际协议测试确认，不能仅凭字段同名推断。
- Go `DecodeSessionStates` 写 map 时没有显式加锁；Rust 在批量写入阶段持有外层写锁。
- Go `Close` 是空操作；Rust `Close` 会实际清空缓存。
- Go 的 `sessionBindInfoKeyType` 是独立定义类型；Rust 是 `i32` 类型别名，并额外提供两个自由函数模拟 `String()`。
- Go 已在 `pkg/session/session.go` 创建句柄、写入会话上下文并把它注册为状态处理器，执行器和绑定匹配路径会取出它；对应 Rust 生产接线当前未找到。Rust 单测证明局部实现，不等同于端到端功能已经迁移。

## 扩展指南

新增创建规则时优先修改 `CreateSessionBinding`，并同步检查 `prepareHints`、摘要键派生与 `BindingCache::SetBinding` 的不变量；若规则也适用于会话迁移恢复，应同时更新 `DecodeSessionStates`，避免“新建”和“恢复”产生不同状态。回归测试应放在独立的 `pkg/bindinfo/session_handle_test.rs`，不要内嵌进生产文件。

变更状态协议时应同时维护 `SessionStates` 的 serde 属性、`EncodeSessionStates`、新格式解码和 `decodeOldStyleSessionStates`，并保留空值、非法 JSON、旧格式和 round-trip 用例。协议是字符串包裹 JSON 数组，任何结构调整都应补充与 Go `pkg/bindinfo/session_handle.go` 及真实会话迁移载体的兼容测试。

调整匹配策略应落在 `BindingCache::MatchingBinding` 或 `binding.rs::crossDBMatchBindings`，而不是在句柄层复制逻辑；同时覆盖当前库、显式库名、通配库名、禁用状态和多个候选优先级。调整关闭语义时应明确是否禁止复用，并处理已外借 `Arc<Binding>` 的存活行为。

若要完成 Rust 主链接线，最可能的接入点是会话初始化、上下文值存储、SQL CREATE/DROP/SHOW BINDING 执行路径、优化前匹配以及统一 `SessionStatesHandler` 注册。实施前必须以对应 Rust 会话与执行器的真实接口为准；当前文件的 `SessionStates` 还未实现 `pkg/sessionctx/context.rs::SessionStatesHandler`，不可仅复制 Go 调用点。风险主要包括：摘要算法或 DB 大小写不一致导致无法命中，锁序变化导致死锁，状态格式变化破坏滚动升级，以及用有限缓存容量意外淘汰会话绑定。

## 验证依据

- RustCodeGraph 索引状态：项目索引包含 11,467 个文件、307,296 个节点和 1,848,419 条边；`files --filter pkg/bindinfo` 确认目标、模块入口、缓存、Go 对照及测试均已索引。
- RustCodeGraph 查询：`explore` 覆盖 `SessionBindingHandle`、`NewSessionBindingHandle`、创建/匹配/编解码和上下文键；`query` 定位 Rust `NewSessionBindingHandle`、trait 方法、`BindingTime`、`prepareHints`、`noDBDigestFromBinding` 与 `crossDBMatchBindings`；`node --file` 核验 `pkg/bindinfo/lib.rs`、`binding.rs`、`binding_cache.rs`、`session_handle.go`、`session_handle_test.go` 和疑似同名调用者 `pkg/server/driver_tidb.rs`。
- 已读生产与配置路径：`pkg/bindinfo/session_handle.rs`、`pkg/bindinfo/lib.rs`、`pkg/bindinfo/Cargo.toml`、`pkg/bindinfo/binding.rs`、`pkg/bindinfo/binding_cache.rs`、`pkg/sessionctx/context.rs`、`pkg/server/driver_tidb.rs`。
- 已读对照与测试路径：`pkg/bindinfo/session_handle.go`、`pkg/bindinfo/session_handle_test.rs`、`pkg/bindinfo/session_handle_test.go`；仓库搜索还核验了 Go 接线点 `pkg/session/session.go`、`pkg/executor/bind.go`、`pkg/executor/show.go`、`pkg/executor/adapter.go` 与 `pkg/bindinfo/binding.go`。
- Rust 独立单测的直接证据：`canonical_session_binding_round_trips_state_and_drops_by_derived_digest` 覆盖创建、匹配、新格式 round-trip、删除和关闭；`create_is_atomic_and_replaces_same_normalized_sql_like_go` 覆盖准备失败不落库及同摘要覆盖；`session_state_empty_invalid_and_old_style_paths_match_go` 覆盖空值、非法 JSON 和旧格式；`session_bind_info_key_string_matches_go` 覆盖键字符串。
- 本任务为纯文档分析，按计划不运行 Cargo。交付前使用任务规定命令确认本文档存在且恰有 11 个固定二级标题，并人工复核未把 Go 生产调用边误写为 Rust 已接线行为。
