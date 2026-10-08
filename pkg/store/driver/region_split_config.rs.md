# `pkg/store/driver/region_split_config.rs`

源码：[region_split_config.rs](region_split_config.rs)。独立测试：[region_split_config_test.rs](region_split_config_test.rs)。

## 文件定位

本文件是 `astersql-store-driver` crate 内 TiKV 驱动的一个私有网络适配模块，负责经由 PD 找到可用的 TiKV 节点，再从 TiKV status HTTP API 读取 coprocessor Region 拆分阈值。它不直接由 crate 根 `lib.rs` 导出，而是由 `kv_adapter.rs` 通过 `#[path = "region_split_config.rs"] mod region_split_config;` 挂载；唯一生产入口 `get_region_split_config` 的可见性为 `pub(super)`，因此只向父模块开放（`kv_adapter.rs:36-37`、`region_split_config.rs:70-74`）。

在完整应用中，`TikvStore` 对 `astersql_kv::Storage::DDLRegionSplitConfig` 的实现取得 client runtime、PD 地址和 TLS 配置，随后在 Tokio runtime 上同步等待本文件的异步查询完成（`kv_adapter.rs:1988-2005`）。目前确认的上游消费点是全局排序 INGEST 规划：`modify_column_cloud_planner.rs:161-180` 先设置默认 Region 大小和键数，再读取该配置并分别取最大值，查询失败仅记录错误并保留默认值。

## 核心职责

- 将 PD 地址列表转换为 `/pd/api/v1/stores` 请求，并按顺序尝试地址，直到拿到结构有效的 `stores` 数组（`get_region_split_config`，`region_split_config.rs:88-110`）。
- 从 PD 返回的 store 元数据中过滤 tombstone、TiFlash 和缺少 status 地址的节点，依次请求 TiKV `/config`，返回第一个可成功解析的 `(region_split_size, region_split_keys)`（`region_split_config.rs:111-157`）。
- 构造带 10 秒超时的 HTTP 客户端；存在 TLS 配置时加载 CA、客户端证书和私钥，并使用 HTTPS（`region_split_config.rs:75-87`）。
- 让每次 HTTP 请求都能被 `kv::Context` 取消，避免仅依赖网络超时（`json`，`region_split_config.rs:47-68`）。
- 修正 TiKV 上报的本地/未指定 status IP：当 store 服务地址是非本地地址、status 地址却是 loopback 或 unspecified 时，保留 status 端口并替换为 store IP（`status_address`，`region_split_config.rs:34-45`）。

本文件只读取配置，不修改 PD、TiKV 或进程内配置，也不负责决定最终拆分策略；阈值合并和默认值回退属于上游规划器。

## 主要符号

- `resolve(address: &str) -> Option<SocketAddr>`：用 `ToSocketAddrs` 解析主机和端口，优先返回 IPv4，否则返回解析结果中的首项；解析失败返回 `None`（`region_split_config.rs:25-32`）。它可能触发 DNS/系统地址解析。
- `status_address(address: &str, status: &str) -> String`：同时解析 store 服务地址与 status 地址。仅在前者为非本地 IP、后者为 loopback/unspecified IP 时替换 status IP；任一地址解析失败或无需替换时原样返回 `status`（`region_split_config.rs:34-45`）。
- `json(client, context, url) -> Result<serde_json::Value, kv::errors::SharedError>`：发送 GET，请求成功状态码后反序列化 JSON；请求前检查取消状态，请求中用 `futures::select` 在 HTTP future 和取消 future 之间竞争，并统一用父模块 `adapter_error` 转换错误（`region_split_config.rs:47-68`）。
- `get_region_split_config(context, pd_addresses, tls) -> Result<(i64, i64), SharedError>`：本模块唯一生产入口。它负责客户端/TLS 初始化、PD 故障切换、store 筛选、TiKV 配置查询和字段解析（`region_split_config.rs:70-158`）。
- `tests`：仅在 `cfg(test)` 下通过独立文件 `region_split_config_test.rs` 挂载，遵守生产源码与测试源码分离（`region_split_config.rs:160-162`）。

本文件没有模块级常量、结构体、枚举、trait 或全局可变状态；10 秒超时当前直接写在客户端构造处。

## 执行流程

1. `TikvStore::DDLRegionSplitConfig` 从驱动取得 runtime、PD 地址和可选 TLS 配置，并调用 `get_region_split_config`（`kv_adapter.rs:1989-2004`）。
2. 入口创建 `reqwest::ClientBuilder`，设置 10 秒超时。若有 TLS 配置，同步读取 CA、证书和私钥，将证书与私钥 PEM 拼接成客户端 identity，并把默认 scheme 设为 `https`；否则使用 `http`（`region_split_config.rs:75-87`）。
3. 按 `pd_addresses` 顺序请求 store 列表。含 `://` 的 PD 地址保留自身 scheme 并去掉末尾 `/`；不含 scheme 的地址使用步骤 2 选定的 scheme。HTTP/JSON 错误或响应缺少数组型 `stores` 时记录最后一个错误并继续下一个 PD（`region_split_config.rs:88-109`）。
4. 若所有 PD 都失败，返回最后一个错误；空 PD 列表返回预置的 `no PD addresses configured`（`region_split_config.rs:88-110`）。
5. 顺序遍历 store。`state == 2`、`state_name == "Tombstone"`、标签含 `engine=tiflash` 或 `status_address` 为空的项被跳过（`region_split_config.rs:111-126`）。
6. 用 `status_address` 必要时把本地/未指定 status IP 替换为 store 服务 IP，然后请求 `{scheme}://{status}/config`。注意 TiKV config 请求使用由 TLS 是否存在决定的 scheme，而不是 PD 地址中显式携带的 scheme（`region_split_config.rs:127-130`）。
7. 从 JSON 的 `coprocessor` 对象读取 `region-split-size` 和 `region-split-keys`。大小必须是字符串或缺失/null（后者按空字符串交给解析器）；键数必须是可表示为 `i64` 的 JSON 整数或缺失/null（后者为 `0`）。大小由 `astersql_config_configtypes::ParseGoSize(size, false)` 解析为字节数（`region_split_config.rs:130-144`）。
8. 第一个成功解析的 TiKV 立即返回。单个 TiKV 请求或解析失败时打印节点级错误并尝试后续节点；全部失败后返回统一错误 `get region split size and keys failed`（`region_split_config.rs:147-157`）。
9. 上游规划器对成功值与默认值分别取最大值，`None` 或错误则保留默认值（`modify_column_cloud_planner.rs:161-180`）。

## 数据与状态

- 输入 `pd_addresses: &[String]` 是有序候选列表；顺序决定优先访问哪个 PD。入口不会修改该列表（`region_split_config.rs:70-74,90-109`）。
- 输入 `tls: Option<TlsConfig>` 按值传入。存在时读取 `ca_path`、`cert_path`、`key_path`，构建仅供本次函数调用使用的 `reqwest::Client`；私钥字节追加到证书 PEM 后交给 `Identity::from_pem`（`region_split_config.rs:76-87`）。
- PD 响应只保留克隆后的 `stores` JSON 数组；不建立长期缓存。`last_error` 只用于保存 PD 探测阶段最近一次失败（`region_split_config.rs:88-110`）。
- 输出二元组均为 `i64`：大小是字节数，键数保持 TiKV 配置中的有符号数值。独立测试明确验证 `"0X_1.8p+1MB"` 被解析为 `3_000_000`，并且 `-42` 原样返回，说明本层不对键数做正数校验（`region_split_config_test.rs:31-38,63-72`）。
- 函数没有静态缓存、锁、原子量或后台任务；每次查询都会重新创建客户端、读取 TLS 文件并访问 PD/TiKV。

## 依赖与调用关系

上游调用链为：

`modify_column_cloud_planner.rs` 的 INGEST 规划 → `dyn Storage::DDLRegionSplitConfig` → `TikvStore::DDLRegionSplitConfig` → `region_split_config::get_region_split_config` → `json` / `status_address` → PD 与 TiKV status HTTP API。

RustCodeGraph 对 `get_region_split_config` 的节点查询确认其生产调用者为 `kv_adapter.rs::DDLRegionSplitConfig`，内部调用边指向本文件的 `json` 和 `status_address`；`status_address` 再调用 `resolve`。`kv::Storage` trait 的默认实现返回 `Ok(None)`，因此非 PD/TiKV 存储不会进入本文件（`pkg/kv/kv.rs:771-780`）。

crate 边界由 `pkg/store/driver/Cargo.toml` 定义：包名为 `astersql-store-driver`，库入口为 `lib.rs`。与本文件直接相关的依赖包括 `astersql-kv`（Context 与共享错误）、`astersql-config-configtypes`（Go 风格大小解析）、`reqwest`（`json`、`rustls-tls`）、`futures`（取消竞争）和 `serde_json`（动态 JSON）。Tokio runtime 由父适配层持有并用于 `block_on`，不是本文件自行创建。

下游外部边界有两个：PD `GET /pd/api/v1/stores` 提供 store 元数据，TiKV `GET /config` 提供 coprocessor 配置。两者的字段访问都基于 `serde_json::Value`，没有在本文件定义强类型响应结构。

## 错误处理与边界

- TLS 文件读取、证书/identity 解析、客户端构建、HTTP 发送、非成功 HTTP 状态、JSON 解码和大小解析错误都通过 `adapter_error` 转换为 `kv::errors::SharedError`（`region_split_config.rs:47-68,75-87,129-144`；转换器见 `kv_adapter.rs:102-104`）。
- PD 阶段具备地址级故障切换，并保留最后一次错误；响应为合法 JSON 但 `stores` 不是数组时产生明确的 `invalid PD stores response`（`region_split_config.rs:88-110`）。
- store 阶段具备节点级故障切换，但最终只返回统一错误；单节点的具体失败只写入标准错误流，调用者拿不到错误集合（`region_split_config.rs:147-157`）。
- `region-split-size` 的非字符串非 null 值和 `region-split-keys` 的非整数值会显式报类型错误；缺失字段因 `serde_json::Value` 索引结果为 null，分别进入空字符串解析和 `0` 分支。空字符串最终是否可接受由 `ParseGoSize` 决定（`region_split_config.rs:130-144`）。
- tombstone 同时兼容数值状态 `2` 与字符串状态名 `Tombstone`；TiFlash 通过 labels 中的精确 `engine=tiflash` 判断（`region_split_config.rs:111-121`）。其他状态没有额外白名单过滤。
- 地址解析失败不会中止查询：`status_address` 保留原 status 字符串，后续请求再产生网络错误并允许尝试下一个 store（`region_split_config.rs:34-45,147-155`）。
- URL 由配置值拼接；本层不做额外路径编码或主机白名单校验。新增地址格式支持时应特别检查 IPv6、显式 scheme、尾斜杠和 TLS scheme 的组合。

## 并发与资源生命周期

`get_region_split_config` 是异步函数，但 PD 地址与 TiKV store 都是串行探测；“第一个成功结果”依赖输入/PD 返回顺序。没有并行 fan-out，因此不会同时向多个节点发送请求，也不需要汇聚竞态结果（`region_split_config.rs:90-109,111-156`）。

`json` 在发送请求前检查一次 `Context`，发送后把完整的“请求、状态校验、JSON body 读取”future 与 `context.cancelled()` 竞争。取消一旦胜出即返回 `context canceled`，未完成的请求 future 随 `select` 结果被丢弃；独立测试保持 HTTP peer 打开并验证取消能在 500ms 内打断未完成 body（`region_split_config.rs:52-67`；`region_split_config_test.rs:85-117`）。每次 PD/store 失败后入口还会再次检查取消，避免继续故障切换（`region_split_config.rs:106-108,149-152`）。

`reqwest::Client`、解析出的 store JSON 和 TLS 字节都局限于一次调用，函数返回后释放。10 秒是客户端级请求超时；取消和超时是两条独立退出路径。`resolve` 使用同步系统地址解析，TLS 文件也同步读取，这两段工作不受异步 `Context` 取消 future 直接打断；扩展到高频调用时需评估对 runtime 工作线程的阻塞影响。

父层在 `RwLock<ClientRuntime>` 的读锁仍被持有时执行 `runtime.block_on`（`kv_adapter.rs:1993-2004`）。本文件不接触该锁，但任何把查询改成长生命周期或加入重试退避的变更，都需要同时评估这段锁持有时间。

## 与 Go 版本的对应关系

仓库中没有同路径 `pkg/store/driver/region_split_config.go`。最接近且承担相同业务语义的 Go 实现是 `pkg/ingestor/ingestctrl/local.go:1991-2038`：`GetRegionSplitSizeKeys` 通过 PD 取得排除 tombstone 的 stores，跳过空 status 地址和 TiFlash，调用 `ServerInfo.ResolveLoopBackAddr` 修正地址，逐个读取 `/config`，解析 region split size/keys，并返回第一个成功结果；全部失败时返回同名统一错误。

对应关系与差异如下：

- Rust 复用了 Go 的“过滤后串行尝试、第一个成功、单节点失败继续、全部失败报错”控制流；Rust 独立测试的三次 `/config` 请求（HTTP 500、非法 size、成功）直接固定了该语义（`region_split_config_test.rs:10-83`）。Go 测试则让前两个节点失败、第三个成功，验证同一顺序回退语义（`local_check_test.go:86-117`）。
- Go 使用 `pd.Client.GetAllStores(ctx, opt.WithExcludeTombstone())`；Rust 因 `Storage` 适配边界直接通过 PD HTTP API 获取 JSON，并自行兼容 `state == 2` 和 `state_name == "Tombstone"`（`local.go:2016-2025`；`region_split_config.rs:90-121`）。
- Go 的 `common.TLS.WithHost(...).GetJSON` 同时处理请求与 context；Rust 显式构建 rustls client，并在 `json` 中用 future 竞争传播取消（`local.go:1993-2012`；`region_split_config.rs:47-87`）。
- Go 用 `units.FromHumanSize`，Rust 用 `ParseGoSize(..., false)`。Rust 测试固定了十进制 `MB` 和 Go 数值语法的解析结果；修改解析器或 flag 必须同步验证 Go 兼容性（`local.go:2007-2012`；`region_split_config_test.rs:31-38,63-72`）。
- Rust 的 `status_address` 测试覆盖 IPv4 loopback、IPv6 unspecified、远端 status 地址保持不变和解析失败保持原值（`region_split_config_test.rs:119-137`）；这对应 Go 的 `ServerInfo.ResolveLoopBackAddr` 意图，但 Rust 实现通过 `ToSocketAddrs` 解析并优先 IPv4，不能假设所有 DNS 多地址行为与 Go 完全相同。

## 扩展指南

- 新增或调整 PD/store 筛选规则：修改 `get_region_split_config` 的两个循环，并在独立的 `region_split_config_test.rs` 增加包含该状态/标签的 fixture；同时与 Go `GetRegionSplitSizeKeys` 的过滤语义对照，避免 Rust 独有漂移。
- 新增配置字段或改变类型规则：优先考虑为响应定义局部强类型结构，修改 `coprocessor` 字段解析处，并覆盖缺失、null、错误类型、边界整数和非法大小。不得把测试内嵌回生产文件。
- 改变地址修正规则：集中修改 `resolve`/`status_address`，同步覆盖 IPv4、IPv6、DNS、多解析结果、loopback、unspecified 和不可解析输入；还需核对 Go `ServerInfo.ResolveLoopBackAddr`。
- 改变超时、重试或并行策略：检查 `json` 的取消竞争、父层持有 runtime 读锁执行 `block_on` 的生命周期，以及“第一个成功节点”的确定性。并行化会改变节点优先级和请求负载，不应仅作为机械优化。
- 改变 TLS/URL 行为：同时验证无 scheme/显式 scheme 的 PD 地址、TiKV status 地址、CA 错误、证书/私钥错误和 mTLS identity。尤其要明确 PD 显式 scheme 与 TiKV scheme 是否应继续采用当前不同来源。
- 若需要向 crate 外暴露能力，应先经 `Storage` trait 或 `kv_adapter` 设计稳定接口；不要直接把本模块改成 `pub mod`，否则会泄漏当前基于动态 JSON 和 reqwest 的实现细节。
- 性能风险主要来自每次调用重建 client、同步读取 TLS 文件、同步地址解析和串行节点探测；兼容风险集中在 PD JSON 字段、TiKV config 字段、大小单位、负/零键数、地址与 TLS scheme。任何缓存都必须定义配置刷新和证书轮换时机。

## 验证依据

- RustCodeGraph 索引状态：项目已索引 11,467 个文件；目标 `region_split_config.rs` 被识别为 162 行、10 个符号的 Rust 文件。
- RustCodeGraph 源码/调用查询：`node --file pkg/store/driver/region_split_config.rs`；`query get_region_split_config --kind function`；`node region_split_config.rs::{get_region_split_config,json,status_address,resolve}`；`node kv_adapter.rs::DDLRegionSplitConfig`。查询确认 `DDLRegionSplitConfig → get_region_split_config → json/status_address → resolve`。
- 已核对生产源码：`pkg/store/driver/region_split_config.rs`、`pkg/store/driver/kv_adapter.rs:36-37,1988-2005`、`pkg/kv/kv.rs:771-780`、`pkg/session/runtime/modify_column_cloud_planner.rs:161-180`、`pkg/store/driver/lib.rs`。
- 已核对 crate 声明：`pkg/store/driver/Cargo.toml`，确认包/库入口及 `astersql-kv`、`astersql-config-configtypes`、`reqwest`、`futures`、`serde_json`、Tokio 等边界。
- 已核对独立 Rust 测试：`pkg/store/driver/region_split_config_test.rs`，覆盖 store 过滤与逐节点回退、HTTP 状态错误、非法/合法大小解析、负键数、请求路径、请求体读取期间取消、IPv4/IPv6 status 地址修正与解析失败回退。
- 已核对 Go 对照及测试：`pkg/ingestor/ingestctrl/local.go:1991-2038`、`pkg/ingestor/ingestctrl/local_check_test.go:86-117`；同时定位 `pkg/infoschema/tables.go:1991` 的 `ServerInfo.ResolveLoopBackAddr` 作为地址修正来源。
- 本任务为纯文档分析，按总计划与任务要求未运行 Cargo。完成前使用任务指定命令验证目标文件存在且恰有 11 个固定二级章节，并人工复核本文区分了源码事实、Go 对照和扩展风险。
