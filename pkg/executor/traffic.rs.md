# [`pkg/executor/traffic.rs`](traffic.rs)

## 文件定位

`request`、`requestOne`、`getForm`、`formReader4Capture`、`formReader4Replay` 提供 HTTP 扇出、表单编码和存储路径转换的共享实现。

## 核心职责

- `TrafficCaptureExec::Next` 为请求补入 RFC3339 `start-time`，发现 TiProxy 地址，并为本地或远程输出生成逐实例表单后调用 `/api/traffic/capture`。
- `TrafficReplayExec::Next` 在 10 秒超时上下文中枚举远程输入的 `tiproxy-*` 一级目录，将目录与 TiProxy 实例配对，再调用 `/api/traffic/replay`；实例数和路径数不相等时分别报错或告警。
- `TrafficCancelExec::Next` 根据 capture/replay 动态权限决定取消全部任务，或仅发送 `type=capture`、`type=replay`。
- `TrafficShowExec::{Open, Next}` 汇总各 TiProxy 的任务，按权限过滤、排序、缓存，并按 chunk 容量分页输出八列结果。
- `request`、`requestOne`、`getForm`、`formReader4Capture`、`formReader4Replay` 提供 HTTP 扇出、表单编码和存储路径转换的共享实现。

## 主要符号

- `TrafficBackend`：生产依赖抽象，集中承载 InfoSync 节点发现、内部 HTTP、URL/对象存储、超时、权限、JSON 解码、日志、statement warning/error 和时间转换。所有执行逻辑都通过该 trait 注入环境能力。
- `TrafficChunk`：`SHOW TRAFFIC JOBS` 的输出适配接口，固定使用列 0、1 写时间或 NULL，列 2 至 7 写实例、类型、进度、状态、错误和参数串。
- `TrafficCaptureExec<B>`、`TrafficReplayExec<B>`、`TrafficCancelExec<B>`、`TrafficShowExec<B>`：四类语句执行器；前三者以 `Next` 完成副作用，Show 在 `Open` 拉取数据、在 `Next` 分页。
- `TrafficJob`：TiProxy show 响应的规范化任务模型；`instance` 由请求地址补入，其余字段用于过滤、排序和展示。
- `TrafficStorageHandle<S>`：外部存储及其所有权标志；`owned=false` 表示上下文注入的 mock/借用存储，执行器不得关闭。
- `TrafficRequestFailure<E>`：保存首个失败之前已经得到的 `responses` 和包装后的错误，便于测试和调用方观察部分成功。
- `getTiProxyAddrs`：把 `TiProxyNode { ip, status_port }` 转为地址列表；空集返回 `no tiproxy server found`。
- `request` / `requestOne`：按地址顺序同步请求；POST 使用 `application/x-www-form-urlencoded`，只有 HTTP 200 被视为成功。
- `getForm` / `queryEscape`：按 key 排序后编码表单；空格编码为 `+`，其他非 unreserved 字节编码为大写 `%XX`，对齐 Go `url.Values.Encode`/`url.QueryEscape` 的本任务覆盖范围。
- 常量 `capturePath`、`replayPath`、`cancelPath`、`showPath`、`sharedStorageTimeout`、`filePrefix` 定义 TiProxy API 和共享存储协议。

## 执行流程

1. Rust builder 在 `pkg/executor/builder.rs` 的 Traffic 分支中把 plan 的目录和选项整理为参数表，再通过依赖接口请求构造具体执行器；当前文件不负责解析 SQL 或实例化生产 backend。
2. Capture 在 `Next` 中写入当前时间，经 `getTiProxyAddrs` 获得节点。`formReader4Capture` 要求非空 `output`：本地路径为每个实例复用同一表单；远程 URL 则追加 `tiproxy-0`、`tiproxy-1` 等子路径，避免多个实例写同一目录。随后 `request` 逐个 POST。
3. Replay 同样写入当前时间和发现节点，然后创建 `sharedStorageTimeout`（10 秒）上下文。`formReader4Replay` 对本地输入直接复制表单；远程输入则打开存储、仅遍历 `tiproxy-` 前缀对象、提取去重后的一级目录，并为每个目录生成输入表单。无论读取成功与否，调用返回后都会结束超时上下文。
4. Replay 若目录数多于实例数，在任何 HTTP 请求前报错；若目录数少于实例数，则截断地址列表、追加 statement warning，并只让相同数量的实例回放。目录集合来自 `HashSet`，没有承诺目录到具体 TiProxy 地址的稳定映射顺序。
5. Cancel 查询 `(capture, replay)` 权限：只有一侧权限时限定任务类型，两侧都有或都没有时表单为空；本文件本身不拒绝“两侧都无权限”，权限拒绝是否在更上游完成取决于生产接线。
6. Show 的 `Open` 先执行 `open_base`，再向所有节点 GET show。每个 body 经 `decode_jobs` 解析，按动态权限丢弃不可见类型，补入来源地址，最后按 `start_time` 字符串降序、相同时间按 `instance` 升序排序。
7. Show 的 `Next` 取 `min(max_chunk_size, jobs.len() - cursor)`，重置 chunk 后逐行写出。运行中任务的空 `end_time` 写 NULL；capture 与 replay 分别格式化不同的参数串。

## 数据与状态

`TrafficCaptureExec::Args` 和 `TrafficReplayExec::Args` 会被 `Next` 原地加入或覆盖 `start-time`；调用后该状态保留。`TrafficShowExec` 在 `jobs` 中缓存一次 `Open` 的全量结果，并通过单调递增的 `cursor` 分页；本文件没有在 `Open` 中显式清零 `cursor`，调用方应遵循一次构造/打开后顺序消费的执行器生命周期。

请求结果使用 `HashMap<address, body>` 聚合；Show 的全局排序消除了响应 map 迭代顺序对输出的影响。远程 replay 目录先进入 `HashSet` 去重，因此表单顺序不稳定，但路径数检查和覆盖集合稳定。`TrafficJob.start_time` 以 RFC3339 字符串排序；在规范 RFC3339 输出下与 Go 版本一致，非规范但可解析的偏移格式是否保持时间先后未在测试中验证。

## 依赖与调用关系

上游模块接线为 `pkg/executor/lib.rs -> traffic`；计划构建侧证据是 `pkg/executor/builder.rs` 的 Traffic 参数收集及 `build_traffic_executor(plan, arguments)` 调用。Go 生产链在 `pkg/executor/builder.go::buildTraffic` 直接构造四类执行器，可用于理解预期位置，但不能替代 Rust 生产 backend 的缺失接线。

文件内部主要调用边为：`TrafficCaptureExec::Next -> getTiProxyAddrs -> formReader4Capture -> request -> requestOne`；`TrafficReplayExec::Next -> getTiProxyAddrs -> formReader4Replay -> request`；`TrafficCancelExec::Next -> hasTrafficPriv -> getForm -> request`；`TrafficShowExec::Open -> open_base/getTiProxyAddrs/request/decode_jobs/hasTrafficPriv`，随后 `TrafficShowExec::Next -> parseTime/TrafficChunk`。RustCodeGraph 的 `explore` 结果还确认 `getTiProxyAddrs` 被 Capture、Replay、Cancel 和 Show 使用，路径/表单辅助函数由相应执行器调用。

具体外部能力不以 crate 类型直接耦合，而由 `TrafficBackend` 方法提供；这包括 InfoSync、HTTP client、对象存储和 session statement context。因此 `Cargo.toml` 虽列出 domain/infosync、objstore、privilege、types、util 等 crate，本文件自身只直接依赖标准库，生产适配器才应承担这些 crate 的类型转换。

## 错误处理与边界

- 节点发现失败或节点为空立即失败；没有发出部分请求。
- `request` 顺序处理地址，遇到首个 transport 错误或非 200 状态立即停止，记录失败日志并返回此前的部分响应；后续地址不再请求。成功日志仅在全部地址完成后记录。
- `requestOne` 对非 200 把响应 body 同时保留为响应文本和错误消息；响应字节以有损 UTF-8 转换，非法字节不会导致单独的解码错误。
- Capture/Replay 都要求非空路径，并为 URL、backend、存储创建、遍历和 raw URL 解析增加阶段化错误上下文。远程 replay 没有任何 `tiproxy-*` 一级目录时明确失败。
- Replay 的超时上下文在 `formReader4Replay` 返回后、传播结果前结束；执行器拥有的存储在成功或闭包内错误后关闭，借用存储不关闭。若 `traffic_storage` 本身失败，则尚无句柄可关闭。
- Show 的 JSON 解码失败会记录地址和原始响应并中止整个 `Open`。时间解析失败则不使 `Next` 返回错误，而是追加 statement error、记录日志并输出 backend 的零时间。
- `TrafficShowExec::Next` 假定 `cursor <= jobs.len()`；正常生命周期满足该不变量，错误地令 cursor 越界会在减法/索引处失败，接口没有防御该内部状态破坏。

## 并发与资源生命周期

本文件没有线程、异步任务、锁或通道。HTTP 请求按地址串行执行，所以单个慢节点会阻塞后续节点，且失败可能形成“前部节点成功、后部节点未执行”的部分副作用；Capture、Replay、Cancel 没有补偿或回滚。

`TrafficReplayExec::Next` 为共享存储阶段创建并结束 10 秒超时上下文，但该超时不包围随后对 TiProxy 的 HTTP 请求。`TrafficStorageHandle::owned` 控制存储关闭责任：仅本执行器创建的句柄由 `close_storage` 回收，上下文注入句柄继续存活。Show 将远端快照持有到分页结束，其内存量与所有可见任务数成正比；每次 `Next` 只 materialize 一个最大 chunk。

## 与 Go 版本的对应关系

实现形态不同：Go 文件直接依赖 `infosync`、HTTP client、objstore、privilege、session context 和 chunk；Rust 将这些能力收拢到 `TrafficBackend`/`TrafficChunk`。Go `request` 只返回 `(部分响应, error)`，Rust 用 `TrafficRequestFailure` 显式保存部分响应。Go 使用 map 枚举远程目录，Rust 使用 `HashSet`，两者都不承诺目录分派顺序。

迁移状态必须保守描述：算法文件与独立 Rust 测试存在，模块也公开导出；但当前搜索没有发现生产 `TrafficBackend` 实现，Rust builder 仅通过依赖 trait 委托构造。

## 扩展指南

- 新增 TiProxy Traffic 操作时，应先在计划/AST 与 `pkg/executor/builder.rs` 的参数映射处接入，再在本文件增加执行器或复用 `request`；同时在独立的 `pkg/executor/traffic_test.rs` 增加路径、method、content type、部分失败和权限用例，不要把测试写入生产文件。
- 新增表单字段时复用 `getForm`，并对照 Go builder 的字段名；字段名是 TiProxy HTTP 协议，改变 `encrypt-method`、`readonly` 等拼写具有兼容风险。
- 修改远程路径策略时同步检查 Capture 的逐实例后缀、Replay 的一级目录提取和 query 保留语义；还需覆盖目录多于、少于、等于实例数及空目录。
- 若实现生产 `TrafficBackend`，应明确 HTTP 响应体关闭、超时取消、对象存储所有权、权限来源、warning/error 写入和 RFC3339 到 SQL 时间的转换，并为它增加独立集成测试。不要让测试 mock 的 `owned=false` 行为泄漏为生产资源不关闭。
- 并行化 `request` 会改变首错、部分响应、日志顺序和外部副作用范围，属于行为及性能语义变更；需要与 Go 版本共同设计，而不是局部优化。
- Show 增加字段时必须同步 `TrafficJob`、`decode_jobs` 适配器、`TrafficChunk` 列映射、SQL schema 和 Go/Rust 测试；调整排序时应明确 RFC3339 偏移格式和稳定性。
