# 任务 6：关系名称与regclass往返

批次：【批次 6】；依赖批次：批次 5（共享文件串行前置；语义依赖另见下文）。

状态：未开始

目的：实现并验证关系名称与regclass往返的真实元数据契约。

来源任务：`.plans/2026-10-24-postgresql18-后续整体实施/13-目录对象映射与标准模式.md`；`docs/postgresql-18-core-compatibility-roadmap.md` 第6章“关系身份与稳定标识”（同名五节正文）。

预计会话范围：只处理这一同源行为组及必要目录接线，在一个上下文内完成测试与交接；上游接口须先满足，不包含底层子系统建设。每个批次仅本任务，线性顺序为1至最终任务，无并行执行任务。

## 文件

- 修改：`pkg/server/pg_catalog.rs`（仅本任务入口）。
- 修改：`pkg/server/pg_oid.rs`（仅必要局部接线）。
- 测试：`pkg/server/pg_catalog_test.rs`，新增下述函数；该独立模块已由 `pkg/server/lib.rs` 注册，无需另建测试框架。

- 新增：`pkg/server/testdata/pg13_006_libpq.py`，仅承载本行为的独立libpq差分，不在本轮创建。
- 测试：`pkg/server/pg_client_integration_test.rs`，新增 `pg13_006_differential`；已有lib.rs模块注册。

## 上下文

现有入口：`pg_oid::resolve_with_path / display / catalog_indexes`。静态基线为de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3；当前实现与路线图历史状态不等同。优先rustcodegraph核对最新实现及调用链，再决定增量。

## 接口依赖与范围

大计划2/11提供持久索引名及有效path，8提供to_regclass正式函数；只补目录局部接线。

上游已完成文件可能已删除，须按源码、完整SHA和有效行为证据确认，不以文件缺失推定未实现。接口未交付则仅在本任务记录具体缺口；本计划不授权补建或迁移其数据模型。

## 测试计划

- 行为：REL-NAME/CAST/UNKNOWN：引号大小写、path遮蔽、限定名往返同OID；跨表重复局部索引名不能任选，持久PG名称来自上游。未知数字OID保留且不造行，缺名cast失败、to_regclass缺名NULL，NULL传播；坏引号/越界错误后恢复。
- 失败验证测试：待新增 `pkg/server/pg_catalog_test.rs::pg13_006_contract`，复用catalog_projection_variants、pg_introspection_columns_live、pg_introspection_constraints_live的真实连接和共享模型模式，仅取本任务相关部分。
- crate：`astersql-server`；目标：`--lib`；features：默认，不添加feature；测试过滤为上述完整路径。
- 失败验证命令：`cargo test --locked -p astersql-server --lib pg_catalog_test::pg13_006_contract -- --exact --nocapture`。
- 预期失败原因：本任务目标字段/真实来源/关联或错误门槛未满足时，已编译测试在行为断言处失败；无目录路由时须捕获实际拒绝并与成功目标对照。依赖编译失败、零测试、环境缺失都不是有效红灯。若当前行为已经正确，仅补覆盖并记录当前通过证据。
- 通过验证命令：`cargo test --locked -p astersql-server --lib pg_catalog_test::pg13_006_contract -- --exact --nocapture`，保持相同输入与断言；至少1个实际测试运行，并在输出中记录本任务各场景数。
- 模拟策略：真实共享内核及TCP优先；只对非法模型/越界边界注入元数据，明确与真实DDL场景区分，不模拟最终目录行。

## 共同行为与交付边界

从仓库根目录执行。先读根AGENTS.md、pkg/server/AGENTS.md、PLANS.md、docs/agents/testing-flow.md及本任务路线图同名主题五处正文。当前server、infoschema、meta/model无doc.go；实施时若新增则须读取。涉及DDL行为先读docs/agents/ddl/README.md；只消费已提交接口，不在目录任务建设上游子系统。

使用真实CreateAnalyzeSession共享domain、PgService及TCP Q/Parse/Bind/Describe/Execute/Sync；测试存储不是RealTiKV。每项覆盖正常、边界、NULL、零行完整描述、非法输入、错误阶段/SQLSTATE、恢复与清理。同输入比较字段值、类型OID/长度/typmod/来源、命令标签及I/T/E，不把未知字段视为相等。元数据损坏/越界可在明确模型边界注入，不能mock provider结果。读取目录无写入或序列分配副作用。涉及对象变更，分别验证同库第二连接新Execute与跨库隔离；不要求旧portal续取切换快照。

字段无来源只允许按契约明确拒绝，不允许伪NULL/空集。必需正向能力缺失属于依赖阻塞，拒绝测试通过不能将整项标完成。仅无关基础设施阻碍回归时允许“已完成，待回归”，保留文件及未验证清单。新功能/范围变化先请求确认，不扩大范围修复依赖。

WIP使用下列聚焦检查；Ready交付Rust修改先运行 `cargo fmt --all`，再运行本任务测试、相邻回归及 `make lint`、`git diff --check`。这些检查覆盖当前变更才可复用。纯Rust计划无Go/Bazel触发；实施若新增/移动Go源、改imports、顶层Go测试、依赖或Bazel元数据，必须按根完整清单先 `make bazel_prepare`；Go failpoints按testing-flow启用且最终关闭。禁止默认运行make bazel_lint_changed；不跑无关昂贵广扫。Go既有回归失败独立记录，可不阻塞；新回归不得忽略。

保留PingCAP版权；修复可用Rust文件顶部加/保留 `// Copyright 2026 AsterSQL.`。Rust测试留独立测试文件。外部Rust依赖只许独立上游提交/tag并统一引用，禁止vendor、third_party、本地patch；本任务不授权依赖移植。不要将全仓cargo fmt产生的无关变更混入提交，也不覆盖其他任务修改。

## Cargo 共享槽位规则

1. 同一仓库的所有计划和任务共用仓库根目录下的 `target/rust-slot-1` 至 `target/rust-slot-10`，不得为任务、会话或计划另建构建目录，也不得按任务编号取模固定分配。
2. 每次构建/验证阶段开始前，创建仓库根目录下的 `target/rust-slot-locks`；依次用原子命令 `mkdir target/rust-slot-locks/slot-N.lock` 尝试领取槽位，`N` 为 1–10。仅 `mkdir` 成功者获得该槽位；这些相对路径命令从仓库根目录执行。
3. 领取成功后，在自己的锁目录中记录计划、任务、会话标识和持有进程 PID。同一槽位同时只允许一个持有者；已占用时尝试下一个，仅全部 10 个槽位都占用时等待并重试。
4. 将 `CARGO_TARGET_DIR` 设置为领取槽位的绝对路径，例如领取槽位 3 后，从仓库根目录执行 `export CARGO_TARGET_DIR="$PWD/target/rust-slot-3"`。示例编号不代表固定分配。
5. 所有 Cargo 构建、检查、测试以及调用 Cargo 的脚本和子进程均继承该设置，不得通过 `--target-dir` 或其他配置绕过槽位；生成的二进制也从该槽位定位。
6. 该阶段的 Cargo、脚本以及使用槽位产物的子进程全部结束后，删除自己写入的占用记录，并用 `rmdir` 释放自己的锁目录；保留槽位编译缓存供后续任务复用。
7. 正常结束、失败和中断均须清理自己的锁；在后台进程或其他使用槽位产物的进程仍运行时不得提前释放。规划清理步骤时覆盖这三种退出路径。
8. 遗留锁须先核实持有会话及相关进程已结束；无法确认则保持占用，不得删除其他活跃任务的锁、进程或缓存。
9. 生成或更新计划时，统一移除与这些规则冲突的单一 Rust 共享构建空间、逐任务独立目录和 Cargo 缓存全局独占窗口要求；仍须遵守源码文件占用、真实业务依赖和其他共享验证资源的互斥，不能仅因缓存分槽就把任务判定为可并行。
10. 最终验证记录包含实际槽位、绝对路径 `CARGO_TARGET_DIR`、确切命令、退出码和有效测试数量；Rust 代码修改后先运行 `cargo fmt --all`，再运行相应验证。编译成功或零测试不能替代行为验收证据。

## 步骤与里程碑

1. 核对依赖，逐字段登记来源、关联键、目标类型和适用路线图验收ID；缺源拒绝与未完成正向目标分开。
2. 在独立测试新增 `pg13_006_contract`，用确切失败命令取得行为证据；只在证明该故事后进入修改。
3. 在 `pg_oid::resolve_with_path / display / catalog_indexes` 处实施最小映射/局部接线，同时更新相邻旧预期，避免保留与PG目标冲突的测试断言。
4. 先格式化，再按通过命令验证；运行 `cargo test --locked -p astersql-server --lib pg_catalog_query_test::pg_introspection_oid_live_casts -- --exact --nocapture` 作为本任务已有邻接回归，不运行全workspace测试。
5. 完成Ready检查并自审；记录本任务真实覆盖、失败恢复及资源清理。重跑使用独立夹具名，清理仅删除本任务所建对象，异常/中断同样关闭连接与listener。

## 验证

运行本任务失败/通过命令、`cargo fmt --all`、上述邻接回归、`make lint`、`git diff --check`。预期有效测试数非零、断言全部通过、lint及diff退出0；记录原始失败与通过输出、字段描述、共享模型ID、对象/连接清理结果。还须完成下节本行为的真实客户端差分；本任务不宣称完整客户端UI兼容。参考若仅18.4，18.6必须另列未验证，不能用替换版本消除缺证。

## 本行为真实客户端差分

本任务同时新增 `pg13_006_differential` 与 `pkg/server/testdata/pg13_006_libpq.py`，只验证本任务字段/行为，不重跑其他族。复用已有 `pg_core_libpq_integer_differential` 的Server双listener、子进程清理和MySQL健康探针模式；独立Python/libpq调用参考 `pg06_integer_literal_libpq.py`，不得直接import会在模块顶层连接数据库的 `pg_core_libpq.py`。不新建通用测试平台。

新脚本接口定义为命令行参数 `TCP端口 数据库名`，Rust测试传入自己临时Server的实际端口与独占库；通过环境读取 `PG18_REFERENCE_DSN` 和 `PG_LIBPQ_LIBRARY`，凭证不得写入参数、日志或证据。测试将证据目录作为新增环境变量 `PG13_EVIDENCE_DIR` 传入，取仓库target下独占运行子目录；这是证据目录而非额外Cargo构建目录。参考端只操作本次建立的可丢弃对象，不删除用户服务/数据库。

失败及通过命令均为 `cargo test --locked -p astersql-server --lib pg_client_integration_test::pg13_006_differential -- --exact --nocapture`；与单元用例同一目标，修复前记录实际差异、修复后保持断言。缺libpq/参考构建/认证接口不得skip成功；环境缺失不算行为红灯。分别记录PostgreSQL完整version/server_version_num、libpq版本、两端库/角色/path、SQL及参数、text/binary格式、完整Describe、值/NULL、错误阶段及恢复。使用固定18.6参考完成目标证据，18.4仅作为单列历史/补充运行，不临时降低目标。用户对象OID只核端内关联/稳定性，不要求跨端数值相等。

当前root测试模式不能证明低权角色；适用权限用例必须依赖15交付的真实认证连接、14有效角色事实，不能只SET字符串模拟。缺认证则该正向验收保持阻塞。产品明确未开放分支保存0A000及无副作用，参考端另记原始结果，不强求它也拒绝。MySQL同Server在前/中/后读取同一真实对象验证健康及原生目录语义未变；不以绕过健康探针替代两端行为比较。

完成证据至少包含本任务全部适用验收场景的双端观察、有效场景数>0、退出码，以及finally关闭libpq result/连接、子进程/Server/listener和测试对象的清理结果。原始错误先脱敏；未观测字段不得标相同。

## 完成

取得本任务正向目标与拒绝/恢复/清理证据后才标“已完成”。报告文件、关键符号、Ready选择理由、确切命令/退出码/有效测试数、槽位/CARGO_TARGET_DIR及未验证项；风险分别说明身份/快照正确性、PG/MySQL兼容性及性能（未测不得声称无影响）。保存本次提交完整SHA和证据路径，不将历史成功挪作当前证据。

有实际代码、测试、配置或说明修改时使用 `$git-commit` 只提交本任务变更并排除无关内容；只有编号文件状态更新/删除则不调用、不单独提交。完成并在最终答复交接证据后删除本编号文件；“已阻塞”或“已完成，待回归”保留文件。不得删除原大计划、覆盖表或修改任意plan.md。

## 进度（Progress）

- [ ] 核对依赖与来源契约。
- [ ] 新增行为测试并记录失败或已有正确行为证据。
- [ ] 完成范围内实现及Ready验证。
- [ ] 交接证据并按实际变更规则提交。

## 发现（Surprises & Discoveries）

初始仅有静态阅读，未运行本任务测试。实施中在本节记录新增发现及路径/输出证据。

## 决策记录（Decision Log）

2026-10-29：复用共享元数据、有界provider和独立测试；必需上游事实缺失时阻塞，不递归补建。

## 结果与回顾（Outcomes & Retrospective）

当前为未开始。实施后记录目标与实际差异、成功/失败场景数、剩余缺证及清理结果。
