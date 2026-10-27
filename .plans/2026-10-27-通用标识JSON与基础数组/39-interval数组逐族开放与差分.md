# 任务 39: interval数组逐族开放与差分

批次：【批次 39】 依赖批次 38；并继承此前已满足接口

状态：未开始

目的：仅开放interval数组

来源任务：上级5号大计划；路线图第3章 ARRAY全部行为继承本族标量契约。

预计会话范围：只实现“interval数组逐族开放与差分”这一行为闭环；先复用依赖已交付接口，再补局部接线和本任务测试。容器与元素、输入与输出、不同标量族均已分离；不承担上游完整子系统。

线性顺序：任务39；可并行批次：仅本任务。依赖批次 38；并继承此前已满足接口。未获得依赖完成SHA和接口证据不得开始依赖实现；编号文件删除本身不算依赖完成。

## 文件

- 修改：`pkg/parser/types/field_type.rs`。
- 修改：`pkg/session/runtime/pg_array.rs`。
- 修改：`pkg/server/pg_extended.rs`。
- 修改：`pkg/server/pg_result.rs`。
- 修改：`pkg/server/pg_extended_test.rs`。
- 修改：`pkg/server/pg_client_integration_test.rs`。
- 新建：`pkg/server/testdata/pg05_array_interval_libpq.py`。
- 新建：`docs/postgresql-pg05-array-interval-evidence.md`。

文件清单为本任务允许的局部落点；新建文件与测试均为待新增，不声称已存在。只改闭环必需内容。涉及已有生产文件改动时同步对应同目录测试及适用的`.rs.md`/文档清单约定；保留PingCAP许可，真正可用的Rust文件顶部加/保留 `// Copyright 2026 AsterSQL.`。

## 上下文

阅读 `AGENTS.md`、适用子目录说明及 `docs/agents/testing-flow.md`；server读取 `pkg/server/AGENTS.md`。先用 `skills/rustcodegraph/SKILL.md` 查新鲜度及消费者；图未覆盖文件再rg。目标crate `astersql-server`，默认features，不加开关。来源上级5号及路线图对应契约/验收表只读；当前HEAD见plan.md。

## 行为与局部设计

仅开放interval数组；元素/数组OID=1186/1187，须执行前核对固定参考目录。months/days/microseconds三分量、独立符号、fields/p与非有限组合；四方向保留shape/lower/typmod及两层NULL，非法任一元素拒绝整个值。

先核对依赖：日期时间块interval及IntervalStyle已有独立身份、两方向codec和当前证据。只在现有数组元素分派注册本族，调用已证标量codec，不复制标量实现；补此前缀TCP/真实libpq测试及脚本。若标量缺口，登记明确接口与证据阻塞，禁止递归移植该标量子系统。每族单独取证，不能从int4数组推导通过。

不支持合法路径明确0A000并登记缺口；非法输入按来源各阶段诊断。任何新增身份必须跨定型、typed NULL、参数与结果持续保留；不能以OID表、编译或目录专用codec替代真实支持。外部Rust依赖不得vendor/third_party/本地patch；若确需移植必须独立上游提交/tag、统一Git tag引用并清理临时副本，不递归扩展本任务。

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

- 行为：上文单一目标及列出的正常、边界、非法值、NULL、恢复和清理要求全部覆盖。
- 失败验证测试：在 `pkg/server/pg_extended_test.rs` 中新增以 `pg05_array_interval` 为前缀的测试函数（待新增）；既有模块注册沿对应lib.rs/runtime.rs，已注册文件内加函数即可。若跨多个crate补测试，保持同一前缀并逐crate运行同类命令，不把测试放进生产文件。
- 失败验证命令：`cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1`。
- 预期失败原因：当前入口缺少上文目标身份/保真/codec或错误分类，新断言在实际值、字节、元数据或SQLSTATE处失败；先保证测试注册且可编译，编译失败不能当行为红灯。已有行为若正确，只补覆盖并记录当前通过，无需无意义生产修改。
- 通过验证命令：先 `cargo fmt --all`，再 `cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1`；与失败验证使用相同输入和断言。
- 模拟策略：规范值测试使用真实解析器/codec；SQL/TCP测试用CreateAnalyzeSession及ConcreteSessionDriver真实共享执行消费者。仅出口非法内部值/缺身份用受控夹具，不mock成功语义或参考端。

## 步骤

1. 阅读来源对应契约和验收表，核对当前源码、已有独立测试与依赖提交；记录真实缺口，不沿历史状态重做。
2. 按上述测试计划补注册与断言，领取Cargo槽位，运行并保存预期行为失败；若实现已正确保存当前通过证据。
3. 实施上文最小修改，不扩大到范围外子系统或重写已可用接口。未知诊断按精确参考构建采集。
4. 修改Rust后先格式化，再同命令验证；补运行受本任务修改影响的同目录测试，确切命令记录于本文件。
5. 采用Ready适用检查并自审差异；清理自己的进程/锁，记录完成证据、SHA与剩余缺证。

## 验证

- WIP：`cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1`；预期至少1个有效行为测试，0失败，退出码0；逐项断言实际值/类型/字节/错误，不接受零测试。
- Ready：`cargo fmt --all -- --check`、`cargo test -p astersql-server --lib pg05_array_interval -- --nocapture --test-threads=1`、`make lint`。代码修改必须make lint，Rust修改先fmt再验证；跨crate实际改动必须追加对应crate此前缀验证并报告数量。
- Go/Bazel：按根完整触发清单决定 `make bazel_prepare`；只改Rust不自动触发。若增加/移动Go、import或新顶层Go测试等，先prepare并包含元数据。Go测试先按testing-flow判定failpoint并最终禁用。禁止默认 `make bazel_lint_changed`；不默认昂贵广扫。
- 所需证据：预期行为失败与通过输出、精确命令/退出码/有效测试数；原始输入输出、身份、NULL及适用SQLSTATE/阶段；实际槽位与绝对CARGO_TARGET_DIR；差异自审及清理结果。来源要求四方向的项不得只验其中一向；源码证据不冒充当前运行证据。

## 真实差分资源契约

复用 `pkg/server/pg_client_integration_test.rs` 的 `pg_temporal_tz_diff` 和 `testdata/pg_tz_invalid_libpq.py` 模式；新增上述Python脚本，由本任务的Rust测试注册并启动同Server双协议夹具。脚本内通过PG18_BIN定位参考工具，在本任务临时目录initdb，以空闲本地端口启动专属参考服务，读取version()/server_version_num、libpq版本、编码和配置并记录。固定18.6构建验收；仅18.4可用时单独记录补充证据，18.6保持缺证，不跳过后报告成功。未知诊断必须实际采集；不照抄临时0A000或猜未知版本码。

所有合法输入覆盖IT→OT、IT→OB、IB→OT、IB→OB（text/binary输入与输出四组合），包含默认/负下界、空集/首行NULL/全NULL/重绑定中适用项；记录原始载荷、参数OID、结果OID/typlen/typmod/格式、值及NULL；成功SQLSTATE写不适用。拒绝项记录阶段、SQLSTATE、无DataRow/无副作用及Sync后恢复，缺失字段不得默认相等。libpq未暴露的portal字段由已有TCP测试补证。输入文本不能泄露密码、DSN、认证载荷。

正常、失败、异常/中断通过try/finally及Rust guard关闭客户端、prepared/portal和本任务服务，等待子进程结束后清理自己的临时数据库目录；不操作用户服务/数据。若使用测试数据库，唯一命名并清理。MySQL在同Server用原协议回归对应原行为。此任务不依赖真实TiKV，不自动启动Playground；涉及持久化行为必须另读实存储runbook后单独定义验证，不能把内存夹具说成RealTiKV通过。

## 完成与交接

只有上文行为与适用Ready检查具备当前证据才能标记已完成；报告确切文件、相关符号、行为、命令、正确性/兼容性/性能风险及本机未验证项。上游接口缺失只在本任务记录具体阻塞，不修改plan.md或原始契约，不递归扩大范围。既有Go回归失败不阻塞本任务；其他无关环境问题若妨碍验收，记录实际证据后标记已完成，待回归并保留文件，不宣称验证通过。缺少本任务核心实现或必需接口则标记已阻塞。

状态仅限未开始、已完成、已阻塞、已完成，待回归。实际代码/测试/配置/文档变更完成后使用 `$git-commit` 只提交本任务变更，排除无关文件；仅本任务状态/删除无需提交。已完成时最终回复报告完整SHA和证据后删除本编号文件；待回归或阻塞保留；plan.md始终只读。

## 进度（Progress）

- [ ] 已核对依赖与当前入口。
- [ ] 已取得失败验证或已有正确实现的当前证据。
- [ ] 已完成局部行为、通过验证及适用Ready检查。
- [ ] 已完成差异审查、资源清理与提交交接。

## 发现（Surprises & Discoveries）

规划阶段仅只读核对，未执行本任务测试；实施时把实际发现与简短证据记录于此。

## 决策记录（Decision Log）

2026-10-27：选择现有共享内核与明确族身份，按当前任务窄边界实施；与前批共享文件，串行执行。来源范围不扩展，未知参考诊断只采证。

## 结果与回顾（Outcomes & Retrospective）

尚无实施结果；结束时在此记录实际SHA、命令、退出码、有效测试数、风险与未验证内容，不能引用历史证据冒充本次通过。
