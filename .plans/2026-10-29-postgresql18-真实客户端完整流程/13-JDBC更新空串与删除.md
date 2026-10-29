# 任务 13: JDBC更新空串与删除


批次：【批次 13】依赖批次12（共享资源串行门禁）。业务依赖任务：12。

状态：未开始

目的：JDBC更新空串与删除，以真实客户端和两端观测闭环验收。

来源任务：大计划19；路线图第8章jdbc流程及对应窄实施切口。

预计会话范围：仅一个行为场景及相邻独立测试，复用既有生产链与客户端夹具；环境或共享能力缺失记录依赖，不扩建子系统。完整流程任务只组合已交付步骤补连续性观测，不重做其实现。

线性顺序：1→2→3→4→5→6→7→8→9→10→11→12→13→14→15→16→17→18→19→20→21→22→23→24→25。可并行批次：无；每批一个任务，避免lib.rs、共享客户端文件、IDE和验证实例冲突。

## 文件


- 待新增或在本客户端前序任务创建后修改：`pkg/server/pg_client19_jdbc_test.rs`，独立测试`pg19_jdbc_update_delete`；`pkg/server/testdata/pg19_jdbc/Pg19Workflow.java`，只实现当前场景分支。
- 注册：`pkg/server/lib.rs`仅首次挂载`pg_client19_jdbc_test`；后续不重复挂载。
- 复用：`pkg/server/pg_client_integration_test.rs`的真实listener和清理辅助；必要时最小可见性接线。
- 局部生产候选：pkg/server/pg_extended.rs、pkg/server/pg_result.rs。只有行为失败定位到这里才修改，同目录相邻独立测试一并修复；不预先承诺有生产缺陷。
- 待新增或增补：`pkg/server/testdata/pg19_jdbc_workflow.md`，保存该客户端固定配置、运行命令、case_id、脱敏证据索引、提交SHA和限制；不是另建路线图。

## 上下文与验收场景


PreparedStatement按键更新id1为two、id2为空串，各影响1；删除id3影响1，零匹配影响0；真实回读与NULL、空串及元数据分别核对，释放每个ResultSet和Statement。

客户端固定pgJDBC42.7.13，记录唯一jar摘要、实际加载路径和DatabaseMetaData.getDriverVersion，以及JRE完整build。新增Java源码启动入口由Rust测试传--class-path与单一jar启动，不改现有目录JDBC_WORKFLOW；新Java按场景参数分支。固定preferQueryMode=extended、stringtype=unspecified，其余属性显式记录；12–16基线prepareThreshold=1、binaryTransfer=false，17单独运行格式矩阵，不能把基线结果推广到binary。

## 实施边界与环境


从仓库根目录执行。先读根AGENTS.md、pkg/server/AGENTS.md、PLANS.md、docs/agents/testing-flow.md；目标包若存在doc.go必须读取（本次pkg/server/doc.go不存在）。DDL相关先读docs/agents/ddl/README.md，不绕过DDL持久化/owner机制。用rustcodegraph核新鲜度和调用链，图未覆盖才rg。

默认features；crate为astersql-server，测试以--lib和--exact执行。复用pg_client_integration_test.rs的start_client_server、CloseServer及同进程MySQL探针；只对需要的测试辅助符号做pub(super)可见性调整，不能把生产分支改成夹具返回值。新测试文件在pkg/server/lib.rs用cfg(test)和path挂载，Rust测试不放生产文件。

新入口契约（均待实现，不冒称现存命令）：读取环境PG19_CONFIG指向的本轮私有绝对路径JSON，字段reference_dsn、aster_dsn、mysql_host、mysql_port、mysql_database、client_path、driver_path、runtime_path、evidence_dir、run_id、timeout_seconds、cleanup_timeout_seconds；连接凭据仅安全配置传入，不打印JSON/完整DSN。非本客户端字段可不适用。实际版本、摘要、来源、配置和所有期限在执行前冻结并写脱敏证据。PG19_BACKEND=local使用真实临时listener/测试存储；PG19_BACKEND=tikv必须连接配置中的已授权隔离RealTiKV服务，禁止悄悄回退local。两模式都必须访问真实PG18.6参考端，缺参考或真实客户端直接准备失败，不能skip。

实存储服务按docs/rust/rust-postgresql-tikv-runbook.md与testing-flow准备；启动自有playground必须后台运行、选无冲突端口/tag，并以EXIT/INT/TERM清理自有进程和数据，不停止现有用户服务。所有Cargo子进程继承槽位。local与tikv结果分开，local通过不证明分布式存储。执行前检查并记录PG18.6完整版本、构建身份；18.4只可另记诊断，不能替代目标。

固定UTF8、UTC、public/search_path、权限、隔离级别和自动提交；唯一run_id限定本轮表名，DDL在事务外。开发InsecureRootOnly显式列证；生产TLS/SCRAM场景必须消费大计划15/16已交付接口，缺失在本文件登记，不扩建认证。类型/参数依赖6、写入依赖10/11、事务依赖12、目录及权限依赖13/14/15、协议依赖16；只检查当前行为所需局部能力，不等待整个大计划。

## 证据与清理契约


复用路线图第1/8章证据字段及现有pg_core_evidence.py的适用能力，禁止另外建立通用平台。每步保留SQL次序、参数/OID/格式/NULL、真实值/列描述、SQLSTATE/阶段、I/T/E、命令标签/行数与退出码。客户端无法暴露字段用关联wire证据补齐，未知写未采集，不默认相等；两端动态对象OID按各自身份映射，不硬判相等。

finally在成功、错误、超时、中断均执行：活T/E先适用Sync及ROLLBACK，释放结果和statement，再删本轮对象、关闭连接；死连接由独立管理连接清理。保存原始失败与清理失败，禁止后续健康检查覆盖。冻结期限内核服务连接/事务/端口及临时产物前后值；关闭只属于本轮的资源。前、中、后核同Server的健康MySQL连接真实SELECT值及PING，不能只凭进程仍存活推断健康。

独立记录发现/执行/有效/失败/异常/超时/跳过和断言数；零测试、marker、编译成功及跳过不算通过。每条有效用例必须全部适用断言与清理通过，别名、重试和两端运行不重复计数；留存脱敏证据文件和SHA。

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

## 测试计划


行为：上述单一场景。待新增测试`pg_client19_jdbc_test::pg19_jdbc_update_delete`在客户端真实返回值之后断言，不以客户端启动成功代替行为。

失败验证命令（先实现并注册上述测试，再运行）：`PG19_BACKEND=local cargo test --locked -p astersql-server --lib pg_client19_jdbc_test::pg19_jdbc_update_delete -- --exact --nocapture --test-threads=1`。

预期失败原因：更新空串或删除行数/回读结果错误。这是待验证风险而非已确认缺陷；如果实现已经正确，保留当前通过证据，只增覆盖。编译失败、缺依赖、缺参考、缺UI不能冒充行为红灯。

通过验证命令：`PG19_BACKEND=local cargo test --locked -p astersql-server --lib pg_client19_jdbc_test::pg19_jdbc_update_delete -- --exact --nocapture --test-threads=1`，以及`PG19_BACKEND=tikv cargo test --locked -p astersql-server --lib pg_client19_jdbc_test::pg19_jdbc_update_delete -- --exact --nocapture --test-threads=1`。两次使用不同run_id/初态与证据路径，均保留PG18.6参考端。每条命令要求恰好执行1个Rust测试，客户端有效断言数大于0；统计每个实际配置，不把多个端点重复算用例。

模拟策略：不mock客户端、SQL、元数据或事务；local仅作为已明确的测试存储聚焦回归，tikv为真实后端最终观测。外部工具超时/信号按异常处理。DataGrip必须另有实际UI证据才有效。

## 步骤


1. 核对本任务版本/接口前置与现存实现，记录输入、预期及权限，准备唯一初态和清理trap。
2. 新增并注册上述场景测试与最小客户端调用，先运行失败验证；定位第一个不一致的实际字段。
3. 仅修复该场景所必需的局部生产接线并补相邻回归；若行为已正确，不制造改动。
4. Rust修改后先`cargo fmt --all`，运行同输入local与tikv通过验证，MySQL旁路和资源清理一起采证。
5. 在真实证据通过后才重构小范围重复代码，执行Ready适用检查；记录到本文件与客户端说明，不改plan.md。

## 验证


确切命令：`cargo fmt --all`；上述两条场景命令；`make lint`；`git diff --check`。Go/Bazel触发时先`make bazel_prepare`；无触发则记录依据。运行前PG19_CONFIG必须指向真实安全配置，不能用文字占位值。

完成证据：实际槽位/绝对CARGO_TARGET_DIR，所有命令、退出码、恰好1个测试及客户端有效数；红灯/绿灯的相同输入与首个差异；版本/build/对象映射；原始结果、wire关联、MySQL探针及finally资源回收；当前提交SHA与工作区差异。任何适用字段未采集都要限制结论。未测性能，不作性能承诺。

## 完成与交接


WIP运行本任务聚焦验证；Ready按根规则选择检查，代码变更必须make lint。当前.agents/skills/tidb-verify-profile缺失，不声称读过。Rust改动后先cargo fmt --all；保留PingCAP版权，可用Rust文件顶部增加或保留// Copyright 2026 AsterSQL.。外部Rust依赖只允许独立上游提交tag并统一Git tag引用，禁止本地patch/vendor。

Go源码/导入/新增Test函数、Go依赖或Bazel变化等按根完整触发清单先make bazel_prepare，并纳入生成元数据；不用make bazel_lint_changed。Go failpoint按testing-flow启用/清理。既有无关Go回归失败按仓库规则记录不阻塞本任务，不能忽略新增回归。

只允许当前行为所必需局部修复；跨模块既有缺口登记依赖，不递归补完整子系统。逐项记录生产修改的来源步骤或不可缺少接线依据。任务全部证据齐备才标已完成；无关环境原因不能运行验证可标已完成，待回归并保留本文件及缺证清单，不能宣布客户端通过。其他依赖阻塞标已阻塞，只更新本任务文件。

有实际代码/测试/配置/文档变更时用$git-commit检查并只提交本任务，排除无关变更；只有任务文件状态或删除无需提交。已完成后最终回复报告文件、Ready选择理由、正确性/兼容性/性能风险、确切命令/退出码/有效数、槽位和CARGO_TARGET_DIR、未验证项及证据位置，再删除本任务文件。plan.md始终只读。稳定证据留在客户端说明和提交中，后续不以任务文件缺失推定完成。

## 进度（Progress）


- [ ] 核对版本、局部依赖和独立初态。
- [ ] 新断言运行并记录真实红灯，或已有行为当前通过证据。
- [ ] 最小局部修复及同输入复验。
- [ ] Ready检查、清理、证据持久化及交接。

## 发现（Surprises & Discoveries）


本轮仅规划，尚无本任务运行证据。实施时在此记录实际发现及脱敏证据路径。

## 决策记录（Decision Log）


2026-10-29：选择单行为、真实客户端及局部修复；不以重连、mock、固定元数据或删减断言实现通过。新发现只记录本文件，不修改只读指南。

## 结果与回顾（Outcomes & Retrospective）


尚未实施；执行后填写实际覆盖、首个失败、剩余依赖、资源回收结果及可复用证据SHA。
