# PostgreSQL18专属函数与运算符实施计划

目标：在既有共享执行链交付批次8选定函数与运算符的普通SQL行为、独立测试与真实双方言证据。

范围：时间截断/提取、选定to_char、int4数组函数与int8追加提升、int4range/numrange内建函数、五个系统函数、ILIKE及四类运算符。每个编号任务负责一个行为闭环。

范围外：重做首批、完整类型/目录/权限/locale/取消系统、下推、数组下标/切片新语法、multirange、扩展、聚合/窗口/SRF。本轮只生成计划。

假设：类型载体和有界工厂已部分存在；接口须按具体消费者取证。删除历史任务不证明完成；缺失上游不由本批递归修复。

## 设计决策

- 选择共享AST→方言签名→正式工厂→共享会话→协议元数据；拒绝SQL替换与第二套PG求值器。
- 内建range依赖独立规范表示；系统函数消费真实事实，禁止常量或目录primitive冒充普通SQL。

## 架构说明

- expression拥有求值，session传递连接上下文与分析身份，server只负责协议和事实桥接。
- 类型、时区、权限、名称与缓存隔离必须在实际消费者保持，参数值不决定重载或OID。

## 开发策略

- 新增独立失败测试后最小实现，同命令复验；已有正确行为只补覆盖，不制造红灯。
- WIP聚焦、Ready交付；每个编号文件是活动ExecPlan，维护进度/发现/决策/结果。
- 本文件生成后只读，不记录状态、阻塞或进度。源码与测试均未在拆分阶段修改或运行。

## 来源与当前事实

当前静态基线 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`（2026-10-29）。首批基线 `c84583fd558f6232fb400e19cfeb18aa0fe99a5c` 保留整数转换/身份/描述；`docs/postgresql-protocol-first-phase.md` 的94场景仅是18.4历史证据，本轮未运行。后续 `2287690688032e463362f123aed54156a4c8c28f` 已接 date_trunc/有界注册，`216ad4288219524d21dd4887091ce99f14e8538e` 已修MySQL专属客户端契约；不得重做IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION。`f3237799861b33b1ef2278aef2dfd277608cd957` 保留扩展数组元素实现；类型证据文档存在18.6记录，但不等于本函数族当前通过。

原蓝图`.plans/2026-10-22-postgresql18-核心兼容蓝图/4-方言函数与运算符契约.md`与首批`.plans/2026-10-24-postgresql18-首批实施/plan.md`已由`ed817facfc77593cbdecbca74b4e5a1509f018f3`清理，本次只读`a789e9b50e37d98dc7611c360c833a1f060c1ddf`版本，不恢复文件。当前大计划2–7也已清理；以Git历史及保留代码核对适用接口，不能按编号删除认定通过。

源码核对：`pkg/expression/builtin.rs`的`resolve_pg_signature`/`PgDateTrunc`已支持text+timestamp；`pkg/types/pg_temporal.rs::PgTimestamp::truncate`已有日历单位；`pkg/session/runtime/query.rs`有对应正式工厂调用；`pkg/server/testdata/pg_function_23_libpq.py`含day、非法单位、签名与参数场景。`pkg/types/pg_array.rs`已有shape/lower/元素NULL与多族载体；`pkg/session/runtime/pg_array.rs`已有普通SQL数组消费，不能当成目录数组。`pg_session.rs::effective_explicit_path`已有存在性筛选，不能推导USAGE过滤。`pg_result.rs::CatalogColumnType::Name`仅证明目录name映射。`builtin_ilike_test.rs`已有正式工厂测试，不证明PG locale全链。

本计划冻结函数清单为：date_trunc四个重载；EXTRACT及date_part各三种源；to_char八种源；array_length/cardinality/array_append的int4代表与int8提升；int4range/numrange两/三参、lower/upper/isempty；current_database/current_schema/pg_typeof/pg_backend_pid/version；ILIKE/NOT ILIKE；int4/numeric二元/、int4前缀-、text||、int4 IS [NOT] DISTINCT FROM。下标仅是数组形状上游交接，不新增下标任务。

## 依赖与执行组织

首批身份和整数基线、大计划2解析与名称、大计划3/4类型和时态、大计划5数组与组合载体、大计划6转换/比较资格、大计划7正式工厂/隔离是局部依赖。补充事实来自会话路径、目录授权、连接backend及版本所有者；不要求这些大计划整体先完成。范围表示、regtype普通SQL身份、USAGE事实和locale上下文要先有可消费接口及当前证据。

线性顺序为1→2→…→38；实施批次1–38各一个任务，没有获准并行批次。原因是共同修改builtin.rs、session query/planning、server测试注册及真实参考环境。编号内列真正语义依赖；前一批仅作为共享文件/资源互斥门槛，不把无关上游缺口变成所有任务的语义依赖。某任务阻塞时可跳至语义依赖已满足的下一批，保证上一持有者已释放文件和资源；不得并发修改共享文件。未来若重新分批，先在对应编号文件和prompt记录文件/参考数据库/locale/端口冲突审计，所有plan.md仍只读。

## 来源主题映射

| 路线图第4章主题与验收族 | 编号任务 |
| --- | --- |
| 时间截断与提取 FUNC-TEMP DAY/ZONE/DST/INTERVAL/NULL-INVALID | 1–10 |
| 格式化函数 FUNC-FMT TIME/NUM/TEMPLATE/NULL/CONTEXT | 11–18 |
| 数组函数 FUNC-ARR EMPTY/NULL-ARRAY/NULL-ELEMENT/MULTIDIM/LOWER | 19–22 |
| 范围函数 FUNC-RNG EMPTY-NULL/UNBOUNDED/BOUNDS/CANON | 23–27 |
| 必需系统函数 FUNC-SYS SESSION/VISIBILITY/TYPEOF/IDENTITY/ISOLATION | 28–32 |
| 大计划8的ILIKE方向及第2章模式匹配契约 | 33 |
| 运算符 OP-CALL DIV/ZERO/CONCAT/NULLSAFE及一元负号 | 34–38 |

每组同时继承该主题“模块、目标、范围外、验收、后续细分”正文。所有计数、参考版本、清理结果由未来真实执行记录，本轮行为有效运行数为0。
