# PostgreSQL18 聚合窗口与集合返回子计划

目标：让普通聚合、窗口、有序集合与集合返回调用沿现有共享内核执行，并用真实结果、固定描述及客户端证据验收。

范围：承接大计划9及路线图第4章四类调用契约，覆盖 count/sum/avg、row_number/rank/lag/lead/last_value、单比例 percentile_disc/cont、整数 generate_series 和单数组 unnest；复杂帧和多投影边界独立取证。

范围外：完整函数库、第二套执行器、通用测试平台、分布式下推扩建、hypothetical-set、数组比例、时间/numeric序列及多数组/复合输出；未覆盖类型仍保留接口依赖，不将其拒绝当作成功支持。

假设：规划只读核对 HEAD `de8e3180cfe4dae852b5d9d074eb2d0ca3c16bd3`；RustCodeGraph已有11703个文件，status未报告新鲜度警告，但其结果不是编译或运行证明。复核时以当前源文件为准。

## 设计决策


选择扩展现有描述符和执行器并作PG局部接线；拒绝在server拼接SQL结果或另建聚合/窗口系统。整块一次实施上下文过大，故按行为拆分；共享文件和验证资源使本计划采用单任务串行批次。

## 架构说明


`AggFuncDesc`承载聚合签名，`TypedHashAgg`管理分组；`PhysicalWindowPlan`装配已有缓冲/流水线执行器。session负责语义到计划的接线，server仅编码结果。SRF表示一次调用返回零至多行，必须复用执行器行源与投影生命周期。

## 开发策略


对行为改变先取得运行期失败，再作最小实现并验证通过；已有正确行为只补覆盖。按`PLANS.md`维护每个编号任务的活动ExecPlan。本文件生成后只读，不承载状态；状态和新发现只写对应编号任务。执行顺序和提示在`prompt.md`。

## 基线与证据边界


路线图的四套定义、目标、验收、后续切口仍是需求来源：普通聚合1687/1981/2132/2318/2465，窗口1699/2007/2136/2334/2471，有序集合1711/2024/2140/2348/2477，SRF1722/2042/2144/2361/2481；行号仅定位，按标题复核。2132等段落的“不实现”描述旧文档阶段，不是本次实施目标的取消。

当前`AggFuncDesc`有DISTINCT/OrderByItems但没有FILTER字段；`TypedHashAgg::new`仍以已有聚合名单分派。窗口计划已有分区、排序、帧和流水线入口；session已有PG VALUES与类型身份接线，不应重做首批。此检查不证明全部PG调用已支持或不支持。目标包doc.go搜索未发现，server局部AGENTS仅额外约束HTTP API，本计划不涉及HTTP。

原任务引用的2026-10-22蓝图任务4、2026-10-24首批plan/prompt已不在工作区；不重建或执行已删除任务。以现存`docs/postgresql-18-core-compatibility-roadmap.md`、`docs/postgresql-protocol-first-phase.md`与父目录`承接与覆盖.md`承接。历史首批提交`c84583fd558f6232fb400e19cfeb18aa0fe99a5c`及18.4证据不是当前验证；18.6目标缺证必须单独报告。

## 接口责任与覆盖


2号负责语法/作用域，3号负责数值/比较，5号负责数组，6号负责推导/参数/描述，7号负责调用分类；本计划只补本行为不可缺少的局部接线。16/18号分别提供portal与取消资源接口。编号不代表这些工作流已经完成，执行前检查真实接口；缺失只在任务记录阻塞并指出责任，不递归修复完整子系统。21号最终回归消费本计划四族证据。

| 子任务 | 来源主题 | 行为依赖 |
| --- | --- | --- |
| [1 COUNT调用与空组](1-COUNT调用与空组.md) | AGG-CALL-EMPTY/NULL/GROUP | 外部局部接口 |
| [2 小整数SUM与返回身份](2-小整数SUM与返回身份.md) | AGG-CALL-NUMERIC/EMPTY/NULL | 1 |
| [3 大整数与NUMERIC求和](3-大整数与NUMERIC求和.md) | AGG-CALL-NUMERIC | 2 |
| [4 精确数AVG最终化](4-精确数AVG最终化.md) | AGG-CALL-NUMERIC/EMPTY/NULL/GROUP | 3 |
| [5 浮点SUM身份与非有限值](5-浮点SUM身份与非有限值.md) | AGG-CALL-NUMERIC | 3 |
| [6 浮点AVG最终化](6-浮点AVG最终化.md) | AGG-CALL-NUMERIC | 4、5 |
| [7 逐聚合FILTER门禁](7-逐聚合FILTER门禁.md) | AGG-CALL-GROUP/DISTINCT-FILTER | 4、6 |
| [8 DISTINCT定型相等语义](8-DISTINCT定型相等语义.md) | AGG-CALL-DISTINCT-FILTER | 7 |
| [9 聚合内排序与非法组合](9-聚合内排序与非法组合.md) | AGG-CALL-DISTINCT-FILTER | 8 |
| [10 聚合局部最终等价门禁](10-聚合局部最终等价门禁.md) | AGG-CALL局部最终与下推 | 9 |
| [11 普通聚合真实客户端差分](11-普通聚合真实客户端差分.md) | AGG-CALL全部五组及状态门禁 | 1、2、3、4、5、6、7、8、9、10 |
| [12 窗口排名分区与同序组](12-窗口排名分区与同序组.md) | WIN-CALL-PEER/NULL/EMPTY-SINGLE | 外部局部接口 |
| [13 LAST_VALUE默认帧与ROWS](13-LAST_VALUE默认帧与ROWS.md) | WIN-CALL-FRAME/EMPTY-SINGLE/NULL | 12 |
| [14 数值RANGE偏移帧](14-数值RANGE偏移帧.md) | WIN-CALL-FRAME | 13 |
| [15 LAG与LEAD逐行偏移](15-LAG与LEAD逐行偏移.md) | WIN-CALL-OFFSET/NULL/EMPTY-SINGLE | 13 |
| [16 GROUPS同序组帧](16-GROUPS同序组帧.md) | WIN-CALL-FRAME的GROUPS目标 | 14 |
| [17 窗口EXCLUDE排除规则](17-窗口EXCLUDE排除规则.md) | WIN-CALL-FRAME的EXCLUDE目标 | 16 |
| [18 窗口真实客户端差分](18-窗口真实客户端差分.md) | WIN-CALL全部五组 | 12、13、14、15、16、17 |
| [19 离散分位数调用与选值](19-离散分位数调用与选值.md) | ORDERED-MEDIAN/EMPTY/NULL/FRACTION的disc | 7、9 |
| [20 连续分位数插值](20-连续分位数插值.md) | ORDERED-MEDIAN/EMPTY/NULL/FRACTION的cont | 19 |
| [21 直接参数与有序集合诊断](21-直接参数与有序集合诊断.md) | ORDERED-FRACTION与调用形态门禁 | 20 |
| [22 有序集合真实客户端差分](22-有序集合真实客户端差分.md) | ORDERED全部四组 | 19、20、21 |
| [23 INT4序列FROM行源](23-INT4序列FROM行源.md) | SRF-DIRECTION/ZERO/NULL | 外部局部接口 |
| [24 INT8序列边界终止](24-INT8序列边界终止.md) | SRF整数重载与边界 | 23 |
| [25 单数组UNNEST行源](25-单数组UNNEST行源.md) | SRF-ARRAY | 23 |
| [26 关联LATERAL重启与外连接](26-关联LATERAL重启与外连接.md) | SRF-CORRELATED | 23、25 |
| [27 单SRF投影行扩展](27-单SRF投影行扩展.md) | SRF-DIRECTION/NULL/ARRAY/CORRELATED投影 | 24、25、26 |
| [28 多SRF协调与非法位置门禁](28-多SRF协调与非法位置门禁.md) | SRF多投影扩展边界 | 27 |
| [29 SRF暂停恢复与资源释放](29-SRF暂停恢复与资源释放.md) | SRF生命周期 | 28 |
| [30 集合返回真实客户端差分](30-集合返回真实客户端差分.md) | SRF全部五组及投影生命周期 | 23、24、25、26、27、28、29 |

## 统一证据契约


单元/SQL/协议/客户端证据分别报告，不能互相替代。原始错误字段包含SQLSTATE、severity、主消息、适用detail/hint/position以及发生阶段；未采字段标缺证。Q为简单查询，扩展协议按Parse、statement Describe、Bind、portal Describe、Execute、Sync逐步核对。结果记录有序列、行数、值、NULL、OID、长度、typmod；text/binary参数×结果四组合、NULL重绑、零行描述均需证据，Execute不重发描述。两PG连接与同Server MySQL交错，MySQL合法对照使用自身语法和类型，不发送PG语法冒充共享证据。

规划不启动数据库、不运行Cargo、不生成实现代码。后续每项先WIP聚焦，交付Ready按根规则选检查；verify-profile技能当前缺失，不虚构技能通过。真实参考固定报告版本、构建、设置和客户端版本；18.4不能代替18.6。仅使用任务自建listener/表/参考进程并finally清理，不删除用户资源。RealTiKV仅实际存储依赖触发，依运行手册后台启动、验证和清理。
