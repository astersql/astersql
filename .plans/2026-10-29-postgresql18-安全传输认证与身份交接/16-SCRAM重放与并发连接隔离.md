# 任务 16: SCRAM重放与并发连接隔离

批次：【批次 16】；依赖批次：12、15。

状态：未开始

目的：成功连接final重放到新连接失败，双连接交叉proof/nonce失败，独立正确连接仍成功。

来源任务：`.plans/2026-10-24-postgresql18-后续整体实施/15-安全传输认证与身份交接.md`，后续整体实施大计划15；路线图第7章TLS/SCRAM。

预计会话范围：只处理“SCRAM重放与并发连接隔离”这一验收目标及必要局部接线、相邻测试，可在一个聚焦会话内理解和交接。来源依赖缺失不在本任务补建。线性推荐顺序为1—22；每批仅一个任务，不与其他编号任务并行；外部计划同文件或共享验证资源占用时也须串行。

## 文件

- 候选生产修改：`pkg/server/pg_scram.rs` `pkg/server/pg_conn.rs` 。仅在断言证明需要时修改。
- 独立测试：`pkg/server/pg_conn_test.rs`；待新增测试函数或同前缀测试组 `pg15_scram_replay`，当前不存在，不宣称已运行。

## 上下文

crate为 `astersql-server`，manifest在 `pkg/server/Cargo.toml`，workspace在根Cargo.toml；默认features，autotests=false，测试由lib.rs显式挂载。现有rustls 0.23和base64可复用，生产入口为Server→PgService及ConcreteSessionDriver。

依赖门槛：依赖任务12、15；不增加跨节点重放缓存。

编号批次表示避免写入冲突的推荐顺序，不把无业务依赖的前序任务人为升级为前提；如无关前序阻塞，可在它停止写入后按本文件依赖列继续。生产凭证和授权硬门槛不得绕过。

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

- 行为：成功连接final重放到新连接失败，双连接交叉proof/nonce失败，独立正确连接仍成功。
- 失败验证测试：在 `pkg/server/pg_conn_test.rs` 新增 `pg15_scram_replay` 前缀测试；复用现有真实listener和共享内核夹具，不放在生产源文件内。
- 失败验证命令：`cargo test -p astersql-server --lib pg15_scram_replay -- --test-threads=1`。
- 预期失败原因：算法正确不代表nonce每连接新生成或交换状态未被共享。 若依赖完成后已满足此行为，保留当前通过证据，只补覆盖，不故意破坏生产代码制造红灯。
- 通过验证命令：`cargo test -p astersql-server --lib pg15_scram_replay -- --test-threads=1`，同输入同断言；至少1个有效测试且目标子场景全部执行，不接受ignored/过滤为0。
- 模拟策略：TLS/协议用真实临时TCP和真实TLS库；认证接线用真实共享凭证/会话；算法用独立固定向量。仅故障签名、时钟期限或底层IO异常允许在清晰边界注入，不能mock认证成功或SQL结果。

## 步骤

1. 阅读依赖证据与本任务相关源码及相邻测试，确认基线实际行为；先写独立失败测试并运行上述命令，记录运行期断言失败。
2. 以安全随机源生成服务端nonce；隔离每连接状态并在结束销毁。测试同用户与不同用户并发交换和重放，断言阶段及无SQL；记录nonce不同的布尔断言，不输出proof/signature。PLUS仍不开放，错证书绑定正向测试不认领。
3. `cargo fmt --all` 后运行同命令验证通过；再运行相邻开发认证隔离回归 `cargo test -p astersql-server --lib startup_auth_roundtrip -- --test-threads=1`，确保测试确实执行。
4. 完成Ready适用检查和差异自审；只在断言持续通过时做本任务内重构。测试的临时证书、库/角色、socket、子进程及线程在成功、失败、中断时都清理；不得提前释放槽位。

## 验证

执行 `cargo test -p astersql-server --lib pg15_scram_replay -- --test-threads=1`、`cargo test -p astersql-server --lib startup_auth_roundtrip -- --test-threads=1`、`make lint`、`git diff --check`。Rust代码修改后先 `cargo fmt --all`；这些命令从仓库根运行并继承领取槽位的绝对CARGO_TARGET_DIR。

预期为全部目标断言通过、失败数0、有效测试数大于0，且明确记录 `pg15_scram_replay` 的每个子场景。输出敏感错误前先脱敏。所需证据包括首次行为失败、最终通过、退出码、实际测试数、TLS/认证阶段及消息顺序、适用SQLSTATE、无执行副作用、资源前后值和健康连接恢复。不适用SQL结果的传输/算法测试明确记值/NULL/OID不适用，不虚构证据。

## 实施约束与交付检查

从仓库根目录执行。先读根 AGENTS.md、pkg/server/AGENTS.md、PLANS.md、docs/agents/testing-flow.md 和本任务引用的路线图契约。当前目标server/privilege包没有doc.go；执行时如新增则先读。代码导航先用skills/rustcodegraph/SKILL.md检查索引，图未覆盖再用rg。只修改列明目标及不可缺少的局部消费者，新增落点须先在本任务记录依据。不得因依赖缺失扩建子系统。

实施用WIP聚焦测试，交付用Ready适用检查；验证技能当前缺失，按根规则手工执行并报告。Rust修改后先运行 `cargo fmt --all`，检查并排除无关格式化差异，然后运行本任务命令及 `make lint`、`git diff --check`。测试命令使用默认features，不加自创feature或绕开workspace；先用相同过滤器的 `-- --list` 核实注册，但列举成功不算行为证据。代码修复保留PingCAP版权，并在真正可用后增加/保留顶部 `// Copyright 2026 AsterSQL.`；全新原创文件不伪造PingCAP版权。

新Rust文件在lib.rs注册，同目录独立 *_test.rs 放测试，不把测试函数写入生产源文件。如新增/删除Go源、改变Go导入/顶层Test、Go依赖或Bazel元数据等命中根完整触发清单，先 `make bazel_prepare` 再验证并纳入元数据；Go failpoint测试必须按testing-flow启用且最终禁用。禁止默认 `make bazel_lint_changed` 或昂贵广扫。既有Go回归失败可按根规则记录，不阻塞已经充分验证的Rust行为。

外部Rust依赖如需移植，必须独立上游提交并打tag、统一Git tag引用，禁止vendor/third_party/本地patch；清理临时副本。无法在本任务窄范围完成则登记依赖，不扩写。测试优先真实本地TCP和共享内核，不能用mock查询结果验收身份授权。需要实存储时先按 docs/rust/rust-postgresql-tikv-runbook.md 和testing-flow启动隔离后台Playground，验证后关闭自己的进程/数据；不删除用户服务。DDL行为变更不在范围，若确需变更则先登记依赖并阅读DDL说明，不自行扩建。

## 完成

同一输入取得运行期失败与通过证据；已有正确行为允许只增加覆盖并记录基线通过。编译失败、零测试、跳过或缺夹具成功返回均不算完成。保存脱敏命令、退出码、有效测试数、资源回收观察和完整提交SHA；不得保存口令/verifier/proof/signature/私钥/取消secret/含密DSN。18.4和18.6分别记录，未知字段不默认相等。

状态只用未开始、已完成、已阻塞、已完成，待回归。实际依赖缺失记已阻塞；只有无关环境原因妨碍剩余验证且实现证据充分时可记已完成，待回归并保留文件，不能用此状态绕过真实凭证或授权门槛。最终按Agent Output Contract报告文件、WIP/Ready及理由、正确性/兼容性/性能风险、确切命令和未验证范围。有实际代码/测试/配置/文档变更时使用 $git-commit 仅提交本任务；只有编号文件状态或删除则不单独提交。完成证据先进入最终回复，再删除已完成编号文件；阻塞和待回归文件保留。plan.md始终只读。

## Progress

- [ ] 核对依赖、生产入口与本任务范围。
- [ ] 记录失败验证或已有正确行为的当前证据。
- [ ] 完成最小实现及聚焦通过验证。
- [ ] 完成Ready适用检查、自审、提交和交接。

## Surprises & Discoveries

规划期只有静态源码证据，本任务未执行。后续仅在本节记录本任务的新发现和安全证据。

## Decision Log

2026-10-29：沿来源大计划15限定的单一行为拆分；复用共享内核，缺生产依赖则阻塞；不把开发认证升级为生产证据。实现选择和有依据的预算数值在此续记。

## Outcomes & Retrospective

尚未实施；完成时填写实际行为、证据、剩余限制和交付SHA，不修改整体plan.md。
