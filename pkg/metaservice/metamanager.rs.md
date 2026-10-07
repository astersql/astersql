# `pkg/metaservice/metamanager.rs`

## 文件定位

[`metamanager.rs`](./metamanager.rs) 属于 `astersql-metaservice` crate。该 crate 由 [`lib.rs`](./lib.rs) 声明并重新导出 `metamanager`，其 manifest 是 [`Cargo.toml`](./Cargo.toml)。本文件位于 PD 成员发现与上层 etcd/Meta Service 建连之间：它把 keyspace 配置和 PD 地址归一化为 `Info { pd_addrs, group }`，决定后续访问全局 PD 地址还是 keyspace 专属 Meta Service group 地址。

真实运行时接线位于 [`dial.rs`](./dial.rs) 的 `ResolveEtcdDialInfo`：该函数取得或接收 PD endpoints，将 `DialKeyspaceMeta` 转为本文件的 `KeyspaceMeta`，调用 `get_info`，再把 `Info::group_addrs()` 作为 etcd endpoint。`lib.rs` 对本模块做通配再导出，所以其他 crate 可通过 `astersql_metaservice::*` 使用这些类型和函数。`Cargo.toml` 表明本 crate 是工作区 library，依赖 `log`、`thiserror`、`etcd-client`、`tokio`、`tikv-client` 与 `astersql-util`；其中本文件直接使用标准库 `HashMap`、`log` 和 `thiserror`，PD/etcd 相关错误与接口则由同 crate 其他模块共享。

## 核心职责

1. 定义 Meta Service 分组配置协议：`GROUP_ID_KEY`、`GROUP_ADDRS_KEY`、GC 管理键和值，以及默认 `GLOBAL_GROUP_ID = "0"`。
2. 校验专属 group ID。`validate_group_id` 要求至少一个 ASCII 字母，且所有字符只能是 ASCII 字母、数字、`-` 或 `_`；默认全局 ID `"0"` 不走该校验。
3. 将 `KeyspaceMeta.config` 解析为 `Group`。存在 group ID 时必须同时满足 keyspace-level GC 和非空地址列表；没有 group ID 时回退到调用方提供的 PD 地址。
4. 将原始 PD 地址和解析后的 `Group` 组合为 `Info`，并提供只读 group 地址视图。
5. 提供从 `PdClient` 拉取 PD 地址后再解析信息的组合入口，以及与 Go 命名兼容的常量和函数别名。
6. 集中声明 `MetaServiceError`。部分变体由本文件产生，部分供相邻的 PD、URL、etcd 和拨号实现复用，因此错误枚举的范围大于本文件自身的执行路径。

本文件不负责建立 etcd 连接、加载 keyspace 元数据、重试 PD 请求或管理后台 runtime；这些职责在 [`dial.rs`](./dial.rs) 和 [`etcd.rs`](./etcd.rs) 中。

## 主要符号

- `GLOBAL_GROUP_ID`、`GROUP_ID_KEY`、`GROUP_ADDRS_KEY`：默认 group 与 keyspace 配置键。`GC_MANAGEMENT_TYPE_KEY`、`GC_MANAGEMENT_TYPE_KEYSPACE_LEVEL`、`GC_MANAGEMENT_TYPE_UNIFIED` 描述 GC 模式，其中专属 group 只接受 `keyspace_level`。
- `KeyspaceMeta { name, config }`：本文件使用的轻量 keyspace 输入。`name` 仅用于错误上下文，`config` 承载 group 与 GC 配置。
- `Group { group_id, addrs }`：路由结果；无专属配置时为 `group_id == "0"` 且 `addrs` 是 PD 地址副本。
- `Info { pd_addrs, group }`：同时保留原始 PD 路由和最终 Meta Service group 路由。`Info::group_addrs` 返回借用切片，不复制数据；`Info::GroupAddrs` 是 Go 风格兼容别名。
- `MetaServiceError`：`NilKeyspaceMeta`、`InvalidGroupId`、`KeyspaceLevelGcRequired`、`GroupNotMatch` 是本文件解析分支的直接错误；`PdClientNotFound`、`NoUsablePdUrl`、`ServiceUrl`、`InvalidUrlPrefix`、`InvalidUrlFormat`、`PdMemberUrl`、`MissingKeyspaceMeta`、`Etcd`、`Pd`、`Cancelled` 供 crate 内 PD/URL/etcd/拨号路径共用。
- `ServiceClient`：抽象“裸 PD 地址”和“带服务 scheme 的 URL”。`get_pd_service_urls` 默认转发到 `get_pd_http_addrs`。本文件的分组主流程不直接调用该 trait；它是 crate 对外服务发现契约的一部分。
- `validate_group_id`：公开的纯校验函数；`is_keyspace_level_gc` 是私有配置判断。
- `get_group`：核心纯解析入口。与 `get_info(None, ...)` 不同，直接传入 `None` 会返回 `NilKeyspaceMeta`。
- `get_info`：面向一般调用者的组装入口；`None` 明确表示非 keyspace 场景并回退到全局 group。
- `fetch_info`：调用同 crate 的 `get_pd_addrs(ctx, pd_client, false)`，随后进入 `get_info`。
- `get_info_and_group_addrs`：在 `fetch_info` 成功后复制一份 group 地址，与 `Info` 一起返回。
- `GlobalGroupID`、`GroupIDKey`、`GroupAddrsKey` 及 `GetGroup`、`GetInfo`、`FetchInfo`、`GetInfoAndGroupAddrs`：保持 Go 命名的兼容门面，均直接引用常量或转发到 snake_case 实现，没有第二套业务逻辑。

## 执行流程

`get_group` 的决策顺序如下：

1. 将 `Option<&KeyspaceMeta>` 解包；`None` 立即返回 `MetaServiceError::NilKeyspaceMeta`。
2. 查询 `GROUP_ID_KEY`。若不存在，返回全局 `Group`，ID 为 `GLOBAL_GROUP_ID`，地址为 `pd_addrs.to_vec()`；此路径允许 PD 地址为空，本文件不另做可用性校验。
3. 若 group ID 存在，先调用 `validate_group_id`。空串、纯数字、含空格/点号，或只含数字和分隔符的值会失败。
4. 调用 `is_keyspace_level_gc`。只有配置值精确等于 `"keyspace_level"` 才通过；键缺失、`"unified"` 或其他值均返回含 keyspace 名称和 group ID 的 `KeyspaceLevelGcRequired`。
5. 读取 `GROUP_ADDRS_KEY`；键缺失返回 `GroupNotMatch`。地址字符串按逗号拆分，每段 `trim`，空段被过滤，非空段保持原顺序且不去重、不验证 URL 格式。过滤后为空同样返回 `GroupNotMatch`。
6. 构造专属 `Group`，记录 info 日志并返回。

`get_info` 在输入为 `Some(meta)` 时复用 `get_group`；输入为 `None` 时自己构造全局 `Group`，因此它不会触发 `NilKeyspaceMeta`。无论哪条成功路径，`Info.pd_addrs` 都保存 PD 地址副本，而 `Info.group.addrs` 可能是专属地址或另一份 PD 地址副本。

`fetch_info` 先以 `with_http_prefix = false` 的语义调用 `get_pd_addrs` 获得可拨号的裸 PD 地址，再调用 `get_info`。`get_info_and_group_addrs` 继续调用 `Info::group_addrs().to_vec()`，让调用者同时得到完整信息和拥有所有权的 endpoint 列表。运行时拨号主链则是 `dial.rs::ResolveEtcdDialInfo → get_info → get_group（仅 Some） → Info::group_addrs`。

## 数据与状态

本文件没有全局可变状态。所有业务结果都由输入即时计算：

- `KeyspaceMeta.config` 是拥有所有权的 `HashMap<String, String>`；解析只借用它，不修改配置。
- `Group`、`Info` 和 `KeyspaceMeta` 均实现 `Clone`，跨边界时通过克隆字符串和向量获得独立所有权。
- `get_group` 与 `get_info` 会复制传入的 `pd_addrs`；专属地址则从配置字符串新建 `String`。因此返回值不借用调用者数据，代价与地址数量和字符串长度线性相关。
- `Info::group_addrs` 返回 `&[String]`，生命周期绑定到 `Info`；只有 `get_info_and_group_addrs` 和拨号接线显式复制为 `Vec<String>`。
- 地址解析保留顺序和重复项。它只做逗号切分、首尾空白清理和空项过滤，不做 DNS、scheme、端口、去重或连通性检查。
- 日志包含完整 `Group`/`Info` 的 `Debug` 输出，因而会包含地址和 group ID；新增敏感配置字段时不应无审查地加入这些结构。

## 依赖与调用关系

上游与下游的直接关系为：

- `pkg/metaservice/lib.rs` 声明并重新导出本模块，根工作区又将该 crate 作为 `facade_metaservice` 暴露。
- `pkg/metaservice/dial.rs::ResolveEtcdDialInfo` 是已确认的生产 Rust 直接调用者：它在必要时通过 PD 获取 endpoints，把拨号元数据转换成 `KeyspaceMeta`，调用 `get_info`，再选择 `group_addrs` 建立带 namespace 的 etcd 客户端。
- 本文件的 `fetch_info` 下调同 crate 的 `get_pd_addrs` 和 `PdClient`。`get_pd_addrs` 的成员发现、URL 处理、重试与取消语义不在本文件实现。
- `get_info` 下调 `get_group`；`get_group` 下调 `validate_group_id` 与私有 `is_keyspace_level_gc`；`get_info_and_group_addrs` 下调 `fetch_info` 和 `Info::group_addrs`。
- `pkg/metaservice/Cargo.toml` 定义 crate 边界；工作区中 `pkg/store/driver`、`pkg/session`、`cmd/tidb-server`、BR、Lightning 与 executor importer 等 manifest 依赖该 crate。对本文件路由结果的生产消费主要通过 `dial.rs` 封装，而不是各上层模块重复解析配置。
- `pkg/metaservice/metamanager_test.rs` 是与源文件分离的直接单元测试；`pkg/metaservice/migration_aster_unit_test.rs` 进一步用 mock `PdClient` 覆盖 `get_info_and_group_addrs` 的组合调用。BR、Lightning 和 executor 的 Meta Service group 测试通过更高层拨号接口验证专属 endpoint 的实际接线。

RustCodeGraph 的文件图还显示 `metamanager.rs` 被 `dial.rs` 和若干跨模块测试引用；精确调用图查询因同名扩散/查询超时未给出完整结果，因此直接引用集合又用受限 `rg --glob '*.rs'` 核对，未把模糊图结果当成确定生产调用边。

## 错误处理与边界

- `get_group(None, ...)` 与 `get_info(None, ...)` 语义不同：前者是调用错误，后者是受支持的非 keyspace/全局回退。扩展或重构时必须保留这一差异。
- 专属 group 的前置条件按固定次序校验：ID 合法性先于 GC 模式，GC 模式先于地址存在性。因此同时含多个错误的配置只返回最先遇到的错误。
- group ID 采用 ASCII 字节规则；非 ASCII 字母不会被视为合法字母。全局 ID `"0"` 是保留回退值，但若把 `"0"` 显式写入 `GROUP_ID_KEY`，会因纯数字而被拒绝。
- GC 配置做精确字符串比较，没有大小写归一化或默认值；显式专属 group 缺失 GC 键也会失败。
- 专属地址只验证“清理后至少一项”，不会验证 `host:port`。实际 URL/endpoint 合法性由后续拨号层处理，因而此处成功不等于连接必然成功。
- `fetch_info` 使用 `?` 原样传播 `get_pd_addrs` 与 `get_info` 的错误；`get_info_and_group_addrs` 也不包装错误。调用者可按 `MetaServiceError` 变体判断根因。
- `MetaServiceError::Etcd` 使用 `#[from]` 转换 `etcd_client::Error`；`PdMemberUrl` 保留原 URL 与嵌套来源。它们是共享错误面，不能据此推断本文件的纯解析函数会执行 etcd 操作。
- `pd_addrs` 为空时全局分组仍可成功构造。这是当前代码事实；需要更强不变量时，应在明确的网络边界校验并补充兼容性测试，而不是悄然改变纯解析函数。

## 并发与资源生命周期

本文件不创建线程、Tokio task、channel、锁、事务或长生命周期客户端。`get_group`、`get_info`、`validate_group_id` 和地址访问器都是同步、确定性的内存操作，除日志外没有副作用；给定相同输入会得到相同结果。

网络和取消生命周期仅通过参数跨越本文件：`fetch_info` 借用 `&Context` 与 `&dyn PdClient`，在同步调用 `get_pd_addrs` 完成后不保留它们；`get_info_and_group_addrs` 同理。`Context` 的取消检查、PD 请求重试、Tokio runtime 和 etcd session 的所有权位于相邻实现。返回的 `Info` 和地址向量均拥有数据，可在借用结束后继续使用。

若未来给本文件增加缓存或并发刷新，需要明确 keyspace 配置更新的一致性、PD 地址失效策略和关闭语义；当前无状态设计不存在锁顺序或后台清理问题。

## 与 Go 版本的对应关系

直接对照文件是 [`metamanager.go`](./metamanager.go)，独立测试是 [`metamanager_test.go`](./metamanager_test.go)。主要语义保持一致：

- `GlobalGroupID`、`GroupIDKey`、`GroupAddrsKey` 值一致；Rust 额外公开 snake_case/全大写常量，并保留 Go 风格别名。
- Go `keyspacepb.KeyspaceMeta` 在 Rust 中被缩减为本模块所需的 `KeyspaceMeta { name, config }`；`dial.rs` 在拨号边界完成转换。
- Go 正则 `^[A-Za-z0-9_-]*[A-Za-z][A-Za-z0-9_-]*$` 与 Rust 的“全部字符在允许集合内且至少一个 ASCII 字母”语义等价。
- 两端都要求专属 group 使用 keyspace-level GC，都会 trim 逗号分隔地址、忽略空段，并在缺失或全空地址时报 group/address 不匹配。
- 两端 `GetInfo(nil/None, pdAddrs)` 都回退全局 group；直接 `GetGroup(nil/None, ...)` 都报错。
- Go 通过指针返回 `*Info`/`*Group` 并让切片可能复用输入底层数组；Rust 返回拥有所有权的值并克隆输入地址，消除了借用/别名关系，但增加复制成本。
- Go 的 `ServiceClient` 声明 `GetPDAddrs` 与 `GetPDServiceURLs`；Rust trait 同时保留 `get_pd_service_urls` 默认方法和兼容的 `get_pd_http_addrs`，这是相邻 URL 发现实现的接口适配差异。
- Rust 将 Go 包级 sentinel/annotated errors 建模为结构化 `MetaServiceError` 变体；`KeyspaceLevelGcRequired` 直接携带 `name` 与 `group_id`，错误字符串仍保留 Go 测试要求的关键语义。
- Rust 比 Go 文件多提供 `fetch_info` 和 `get_info_and_group_addrs` 的 snake_case 实现及 Go 风格门面；Go 文件中也已有同名组合 helper，因此功能意图一致。

[`metamanager_test.rs`](./metamanager_test.rs) 与 Go 测试覆盖同一组核心表：nil/None、合法 group、统一 GC 拒绝、空白地址过滤、缺失/全空地址、全局回退、非法 ID 和 `GetInfo` 两条路径。`migration_aster_unit_test.rs` 还验证了 mock PD 成员发现后 `Info.pd_addrs` 与专属 `group_addrs` 的分离。

## 扩展指南

- 新增或改变 keyspace 配置键：先修改常量和 `get_group` 的解析顺序，再同步 [`metamanager_test.rs`](./metamanager_test.rs)、[`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 与 Go 对照实现/测试。必须明确缺失值、空值、未知值以及旧集群配置的兼容行为。
- 调整 group ID 规则：修改 `validate_group_id`，保持与 Go 正则等价，并为合法边界（大小写、`-`、`_`、字母数字混合）和非法边界（空、纯数字、非 ASCII、其他标点）补独立测试。
- 增强地址校验：最可能接入 `get_group` 的地址解析段，但需评估当前允许 Unix scheme、IPv6 或后续层才解析 scheme 的调用链。不要把 `dial.rs`/`etcd.rs` 的 URL 规则不加区分地复制进来。
- 改变 GC 约束：修改 `is_keyspace_level_gc` 与 `get_group` 的专属分支，并同步 Go 行为；该约束防止专属 Meta Service group 与统一 GC 发生语义冲突，属于兼容性不变量。
- 新增返回字段：修改 `Info`/`Group`、`get_info`/`get_group` 构造点、日志输出和 `dial.rs::ResolveEtcdDialInfo` 消费点；关注 clone 成本及敏感信息日志风险。
- 改变 PD 获取流程：入口是 `fetch_info`，真正的成员发现应继续放在 `get_pd_addrs`/`PdClient` 实现。要用 mock `PdClient` 测试取消、错误传播和地址选择，不应在本文件内嵌网络测试。
- 兼容门面：snake_case 实现是唯一业务逻辑来源；Go 风格导出函数应保持薄转发，避免两套实现漂移。
- 测试必须继续放在独立文件，主要同步点是 `pkg/metaservice/metamanager_test.rs`；跨模块拨号行为可在现有 BR、Lightning、executor 的 `meta_service_group_test.rs` 中扩展。性能风险主要来自地址/配置的重复克隆与过大的日志载荷，而不是算法复杂度。

## 验证依据

- RustCodeGraph `status`：索引包含 11,467 个文件，目标 `pkg/metaservice/metamanager.rs`、Go 对照和测试均在索引中。
- RustCodeGraph `files --filter pkg/metaservice`：确认 crate 内 `lib.rs`、`dial.rs`、`etcd.rs`、本文件及独立 Rust/Go 测试的邻接结构。
- RustCodeGraph `node --file pkg/metaservice/metamanager.rs --offset 1 --limit 500`：读取目标文件完整 287 行，核对常量、错误枚举、三个结构、trait、纯解析函数、组合入口、兼容别名和独立测试模块声明。
- RustCodeGraph `node` 读取 `pkg/metaservice/lib.rs`、`pkg/metaservice/metamanager.go`、`pkg/metaservice/metamanager_test.rs`、`pkg/metaservice/metamanager_test.go`：核对模块再导出、Go 语义和测试边界。
- RustCodeGraph `explore "pkg/metaservice/metamanager.rs MetaManager StateSyncer SchemaInfoReader"`：获得 `get_info → get_group`、`fetch_info → get_pd_addrs`、`get_info_and_group_addrs → fetch_info/group_addrs` 及 `dial.rs::ResolveEtcdDialInfo` 等候选调用证据。因索引存在大量同名符号，后续用文件限定查询和受限文本搜索复核。
- `query get_group --kind function --json`：定位精确节点 `metamanager.rs::get_group`（第 139 行）及其直接/迁移测试。`callers/callees --file` 在本地索引查询中未及时返回，未将其缺失结果用于推断。
- `rg --glob '*.rs'` 的限定直接引用检查与局部源码读取：确认生产直接调用点 `pkg/metaservice/dial.rs:299` 和 `Info::group_addrs` 消费点 `dial.rs:311`；确认其他命中主要是本文件内部转发和独立测试。
- [`Cargo.toml`](./Cargo.toml) 与工作区 Cargo manifest 搜索：确认 crate 名、library 入口、直接依赖、porting 元数据，以及 store/session/server/BR/Lightning/executor 等 crate 的依赖边界。
- [`migration_aster_unit_test.rs`](./migration_aster_unit_test.rs) 的 `migration_get_group_matches_go_validation_and_fallbacks`、`migration_get_group_rejects_invalid_or_incomplete_configuration`、`migration_get_info_and_combined_lookup_match_go`：确认纯解析不变量和 mock PD 组合链。
- 人工复核：文档区分了本文件当前实现与相邻模块职责；未宣称本文件执行网络、重试或资源管理；没有把测试嵌入生产源文件；所有主要行为都可回溯到上述真实符号或对照测试。
