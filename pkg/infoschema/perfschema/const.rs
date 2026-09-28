// Copyright 2026 AsterSQL.
// Copyright 2016 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// PERFORMANCE_SCHEMA 静态建表 SQL 常量。
//
// 每张表对应一段与 MySQL performance_schema 对齐的 CREATE TABLE 文本，
// 由 init 模块轻量解析后注册为虚拟表。DDL 字符串与 Go const 保持逐字节一致。
// Performance Schema：运行时性能统计虚拟库。

#![allow(dead_code, non_snake_case, non_upper_case_globals)]

use std::sync::LazyLock;

use crate::tables::*;

// perfSchemaTables 对应 Go 的完整建表 SQL 清单，顺序保持不变，供 schema 初始化逐项处理。
/// 初始化时按序解析的全部 PERFORMANCE_SCHEMA 建表 SQL。
pub static perfSchemaTables: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        tableGlobalStatus.as_str(),
        tableGlobalVariables.as_str(),
        tableSessionAccountConnectAttrs.as_str(),
        tableSessionConnectAttrs.as_str(),
        tableSessionStatus.as_str(),
        tableSetupActors.as_str(),
        tableSetupObjects.as_str(),
        tableSetupInstruments.as_str(),
        tableSetupConsumers.as_str(),
        tableStmtsCurrent.as_str(),
        tableStmtsHistory.as_str(),
        tableStmtsHistoryLong.as_str(),
        tablePreparedStmtsInstances.as_str(),
        tableTransCurrent.as_str(),
        tableTransHistory.as_str(),
        tableTransHistoryLong.as_str(),
        tableSessionVariables.as_str(),
        tableStagesCurrent.as_str(),
        tableStagesHistory.as_str(),
        tableStagesHistoryLong.as_str(),
        tableEventsStatementsSummaryByDigest.as_str(),
        tableTiDBProfileCPU.as_str(),
        tableTiDBProfileMemory.as_str(),
        tableTiDBProfileMutex.as_str(),
        tableTiDBProfileAllocs.as_str(),
        tableTiDBProfileBlock.as_str(),
        tableTiDBProfileGoroutines.as_str(),
        tableTiKVProfileCPU.as_str(),
        tablePDProfileCPU.as_str(),
        tablePDProfileMemory.as_str(),
        tablePDProfileMutex.as_str(),
        tablePDProfileAllocs.as_str(),
        tablePDProfileBlock.as_str(),
        tablePDProfileGoroutines.as_str(),
        tableStatusByConnection.as_str(),
        tableCondInstances.as_str(),
        tableEventsWaitsCurrent.as_str(),
        tableEventsWaitsHistory.as_str(),
        tableEventsWaitsHistoryLong.as_str(),
        tableAccounts.as_str(),
        tableHosts.as_str(),
        tableUsers.as_str(),
        tableBinaryLogTransactionCompressionStats.as_str(),
        tableEventsTransactionsSummaryByUserByEventName.as_str(),
    ]
});

// 以下每个 LazyLock<String> 对应一个 Go const DDL；各列片段和空白原样保留，不在迁移中改写 SQL。
// 状态、变量和 setup 表保留 MySQL performance_schema 的列宽、空值与默认值约束。
// tableGlobalStatus contains the column name definitions for table global_status, same as MySQL.
/// global_status 表 DDL：全局状态变量名与值。
pub static tableGlobalStatus: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE performance_schema.",
        tableNameGlobalStatus,
        " (",
        "VARIABLE_NAME VARCHAR(64) not null,",
        "VARIABLE_VALUE VARCHAR(1024));",
    ]
    .concat()
});

// tableSessionStatus contains the column name definitions for table session_status, same as MySQL.
/// session_status 表 DDL：会话状态变量名与值。
pub static tableSessionStatus: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE performance_schema.",
        tableNameSessionStatus,
        " (",
        "VARIABLE_NAME VARCHAR(64) not null,",
        "VARIABLE_VALUE VARCHAR(1024));",
    ]
    .concat()
});

// tableGlobalVariables contains the column name definitions for table global_variables, same as MySQL.
/// global_variables 表 DDL：全局系统变量。
pub static tableGlobalVariables: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE performance_schema.",
        tableNameGlobalVariables,
        " (",
        "VARIABLE_NAME varchar(64) NOT NULL,",
        "VARIABLE_VALUE varchar(1024) DEFAULT NULL);",
    ]
    .concat()
});

// tableSetupActors contains the column name definitions for table setup_actors, same as MySQL.
/// setup_actors 表 DDL：按 HOST/USER/ROLE 配置插桩启用。
pub static tableSetupActors: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameSetupActors,
        " (",
        "HOST			CHAR(60) NOT NULL  DEFAULT '%',",
        "USER			CHAR(32) NOT NULL  DEFAULT '%',",
        "ROLE			CHAR(16) NOT NULL  DEFAULT '%',",
        "ENABLED		ENUM('YES','NO') NOT NULL  DEFAULT 'YES',",
        "HISTORY		ENUM('YES','NO') NOT NULL  DEFAULT 'YES');",
    ]
    .concat()
});

// tableSetupObjects contains the column name definitions for table setup_objects, same as MySQL.
/// setup_objects 表 DDL：按对象类型/库/名配置插桩。
pub static tableSetupObjects: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameSetupObjects,
        " (",
        "OBJECT_TYPE		ENUM('EVENT','FUNCTION','TABLE') NOT NULL  DEFAULT 'TABLE',",
        "OBJECT_SCHEMA		VARCHAR(64)  DEFAULT '%',",
        "OBJECT_NAME		VARCHAR(64) NOT NULL  DEFAULT '%',",
        "ENABLED		ENUM('YES','NO') NOT NULL  DEFAULT 'YES',",
        "TIMED			ENUM('YES','NO') NOT NULL  DEFAULT 'YES');",
    ]
    .concat()
});

// tableSetupInstruments contains the column name definitions for table setup_instruments, same as MySQL.
/// setup_instruments 表 DDL：仪器名称与启用/计时开关。
pub static tableSetupInstruments: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameSetupInstruments,
        " (",
        "NAME			VARCHAR(128) NOT NULL,",
        "ENABLED		ENUM('YES','NO') NOT NULL,",
        "TIMED			ENUM('YES','NO') NOT NULL);",
    ]
    .concat()
});

// tableSetupConsumers contains the column name definitions for table setup_consumers, same as MySQL.
/// setup_consumers 表 DDL：消费者名称与启用开关。
pub static tableSetupConsumers: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameSetupConsumers,
        " (",
        "NAME			VARCHAR(64) NOT NULL,",
        "ENABLED			ENUM('YES','NO') NOT NULL);",
    ]
    .concat()
});

// statement 当前、历史、长历史与预处理表按 Go 原列顺序完整展开，包括计时、行数和索引使用统计。
// tableStmtsCurrent contains the column name definitions for table events_statements_current, same as MySQL.
/// events_statements_current 表 DDL：当前语句事件。
pub static tableStmtsCurrent: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStatementsCurrent,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "LOCK_TIME		BIGINT(20) UNSIGNED NOT NULL,",
        "SQL_TEXT		LONGTEXT,",
        "DIGEST			VARCHAR(32),",
        "DIGEST_TEXT		LONGTEXT,",
        "CURRENT_SCHEMA	VARCHAR(64),",
        "OBJECT_TYPE		VARCHAR(64),",
        "OBJECT_SCHEMA	VARCHAR(64),",
        "OBJECT_NAME		VARCHAR(64),",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "MYSQL_ERRNO		INT(11),",
        "RETURNED_SQLSTATE	VARCHAR(5),",
        "MESSAGE_TEXT	VARCHAR(128),",
        "ERRORS			BIGINT(20) UNSIGNED NOT NULL,",
        "WARNINGS		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_AFFECTED	BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_SENT		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_EXAMINED	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_DISK_TABLES	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_TABLES		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_JOIN		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_RANGE_JOIN	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE_CHECK		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_MERGE_PASSES		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_RANGE		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_ROWS		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "NO_INDEX_USED	BIGINT(20) UNSIGNED NOT NULL,",
        "NO_GOOD_INDEX_USED		BIGINT(20) UNSIGNED NOT NULL,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'),",
        "NESTING_EVENT_LEVEL		INT(11));",
    ]
    .concat()
});

// tableStmtsHistory contains the column name definitions for table events_statements_history, same as MySQL.
/// events_statements_history 表 DDL：语句事件短历史。
pub static tableStmtsHistory: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStatementsHistory,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID		BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "LOCK_TIME		BIGINT(20) UNSIGNED NOT NULL,",
        "SQL_TEXT		LONGTEXT,",
        "DIGEST			VARCHAR(32),",
        "DIGEST_TEXT		LONGTEXT,",
        "CURRENT_SCHEMA 	VARCHAR(64),",
        "OBJECT_TYPE		VARCHAR(64),",
        "OBJECT_SCHEMA	VARCHAR(64),",
        "OBJECT_NAME		VARCHAR(64),",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "MYSQL_ERRNO		INT(11),",
        "RETURNED_SQLSTATE		VARCHAR(5),",
        "MESSAGE_TEXT	VARCHAR(128),",
        "ERRORS			BIGINT(20) UNSIGNED NOT NULL,",
        "WARNINGS		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_AFFECTED	BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_SENT		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_EXAMINED	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_DISK_TABLES	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_TABLES		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_JOIN		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_RANGE_JOIN	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE_CHECK		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_MERGE_PASSES		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_RANGE		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_ROWS		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "NO_INDEX_USED	BIGINT(20) UNSIGNED NOT NULL,",
        "NO_GOOD_INDEX_USED		BIGINT(20) UNSIGNED NOT NULL,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'),",
        "NESTING_EVENT_LEVEL		INT(11));",
    ]
    .concat()
});

// tableStmtsHistoryLong contains the column name definitions for table events_statements_history_long, same as MySQL.
/// events_statements_history_long 表 DDL：语句事件长历史。
pub static tableStmtsHistoryLong: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStatementsHistoryLong,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "LOCK_TIME		BIGINT(20) UNSIGNED NOT NULL,",
        "SQL_TEXT		LONGTEXT,",
        "DIGEST			VARCHAR(32),",
        "DIGEST_TEXT		LONGTEXT,",
        "CURRENT_SCHEMA	VARCHAR(64),",
        "OBJECT_TYPE		VARCHAR(64),",
        "OBJECT_SCHEMA	VARCHAR(64),",
        "OBJECT_NAME		VARCHAR(64),",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "MYSQL_ERRNO		INT(11),",
        "RETURNED_SQLSTATE		VARCHAR(5),",
        "MESSAGE_TEXT	VARCHAR(128),",
        "ERRORS			BIGINT(20) UNSIGNED NOT NULL,",
        "WARNINGS		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_AFFECTED	BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_SENT		BIGINT(20) UNSIGNED NOT NULL,",
        "ROWS_EXAMINED	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_DISK_TABLES	BIGINT(20) UNSIGNED NOT NULL,",
        "CREATED_TMP_TABLES		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_JOIN		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_FULL_RANGE_JOIN	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE	BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_RANGE_CHECK		BIGINT(20) UNSIGNED NOT NULL,",
        "SELECT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_MERGE_PASSES		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_RANGE		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_ROWS		BIGINT(20) UNSIGNED NOT NULL,",
        "SORT_SCAN		BIGINT(20) UNSIGNED NOT NULL,",
        "NO_INDEX_USED	BIGINT(20) UNSIGNED NOT NULL,",
        "NO_GOOD_INDEX_USED		BIGINT(20) UNSIGNED NOT NULL,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'),",
        "NESTING_EVENT_LEVEL		INT(11));",
    ]
    .concat()
});

// tablePreparedStmtsInstances contains the column name definitions for table prepared_statements_instances, same as MySQL.
/// prepared_statements_instances 表 DDL：预处理语句实例。
pub static tablePreparedStmtsInstances: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNamePreparedStatementsInstances,
        " (",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED NOT NULL,",
        "STATEMENT_ID	BIGINT(20) UNSIGNED NOT NULL,",
        "STATEMENT_NAME	VARCHAR(64),",
        "SQL_TEXT		LONGTEXT NOT NULL,",
        "OWNER_THREAD_ID	BIGINT(20) UNSIGNED NOT NULL,",
        "OWNER_EVENT_ID	BIGINT(20) UNSIGNED NOT NULL,",
        "OWNER_OBJECT_TYPE		ENUM('EVENT','FUNCTION','TABLE'),",
        "OWNER_OBJECT_SCHEMA		VARCHAR(64),",
        "OWNER_OBJECT_NAME		VARCHAR(64),",
        "TIMER_PREPARE	BIGINT(20) UNSIGNED NOT NULL,",
        "COUNT_REPREPARE	BIGINT(20) UNSIGNED NOT NULL,",
        "COUNT_EXECUTE	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_TIMER_EXECUTE		BIGINT(20) UNSIGNED NOT NULL,",
        "MIN_TIMER_EXECUTE		BIGINT(20) UNSIGNED NOT NULL,",
        "AVG_TIMER_EXECUTE		BIGINT(20) UNSIGNED NOT NULL,",
        "MAX_TIMER_EXECUTE		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_LOCK_TIME	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_ERRORS		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_WARNINGS	BIGINT(20) UNSIGNED NOT NULL,",
        "		SUM_ROWS_AFFECTED		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_ROWS_SENT	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_ROWS_EXAMINED		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_CREATED_TMP_DISK_TABLES	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_CREATED_TMP_TABLES	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SELECT_FULL_JOIN	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SELECT_FULL_RANGE_JOIN	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SELECT_RANGE		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SELECT_RANGE_CHECK	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SELECT_SCAN	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SORT_MERGE_PASSES	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SORT_RANGE	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SORT_ROWS	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_SORT_SCAN	BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_NO_INDEX_USED		BIGINT(20) UNSIGNED NOT NULL,",
        "SUM_NO_GOOD_INDEX_USED	BIGINT(20) UNSIGNED NOT NULL);",
    ]
    .concat()
});

// transaction 和 stage 的 current/history/history_long 三组 DDL 保持相同字段骨架及各自表名。
// tableTransCurrent contains the column name definitions for table events_transactions_current, same as MySQL.
/// events_transactions_current 表 DDL：当前事务事件。
pub static tableTransCurrent: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsTransactionsCurrent,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "STATE			ENUM('ACTIVE','COMMITTED','ROLLED BACK'),",
        "TRX_ID			BIGINT(20) UNSIGNED,",
        "GTID			VARCHAR(64),",
        "XID_FORMAT_ID	INT(11),",
        "XID_GTRID		VARCHAR(130),",
        "XID_BQUAL		VARCHAR(130),",
        "XA_STATE		VARCHAR(64),",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "ACCESS_MODE		ENUM('READ ONLY','READ WRITE'),",
        "ISOLATION_LEVEL	VARCHAR(64),",
        "AUTOCOMMIT		ENUM('YES','NO') NOT NULL,",
        "NUMBER_OF_SAVEPOINTS	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_ROLLBACK_TO_SAVEPOINT	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_RELEASE_SAVEPOINT		BIGINT(20) UNSIGNED,",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// tableTransHistory contains the column name definitions for table events_transactions_history, same as MySQL.
/// events_transactions_history 表 DDL：事务事件短历史。
pub static tableTransHistory: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsTransactionsHistory,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "STATE			ENUM('ACTIVE','COMMITTED','ROLLED BACK'),",
        "TRX_ID			BIGINT(20) UNSIGNED,",
        "GTID			VARCHAR(64),",
        "XID_FORMAT_ID	INT(11),",
        "XID_GTRID		VARCHAR(130),",
        "XID_BQUAL		VARCHAR(130),",
        "XA_STATE		VARCHAR(64),",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "ACCESS_MODE		ENUM('READ ONLY','READ WRITE'),",
        "ISOLATION_LEVEL	VARCHAR(64),",
        "AUTOCOMMIT		ENUM('YES','NO') NOT NULL,",
        "NUMBER_OF_SAVEPOINTS	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_ROLLBACK_TO_SAVEPOINT	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_RELEASE_SAVEPOINT		BIGINT(20) UNSIGNED,",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// tableTransHistoryLong contains the column name definitions for table events_transactions_history_long, same as MySQL.
/// events_transactions_history_long 表 DDL：事务事件长历史。
pub static tableTransHistoryLong: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsTransactionsHistoryLong,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "STATE			ENUM('ACTIVE','COMMITTED','ROLLED BACK'),",
        "TRX_ID			BIGINT(20) UNSIGNED,",
        "GTID			VARCHAR(64),",
        "XID_FORMAT_ID	INT(11),",
        "XID_GTRID		VARCHAR(130),",
        "XID_BQUAL		VARCHAR(130),",
        "XA_STATE		VARCHAR(64),",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "ACCESS_MODE		ENUM('READ ONLY','READ WRITE'),",
        "ISOLATION_LEVEL	VARCHAR(64),",
        "AUTOCOMMIT		ENUM('YES','NO') NOT NULL,",
        "NUMBER_OF_SAVEPOINTS	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_ROLLBACK_TO_SAVEPOINT	BIGINT(20) UNSIGNED,",
        "NUMBER_OF_RELEASE_SAVEPOINT		BIGINT(20) UNSIGNED,",
        "OBJECT_INSTANCE_BEGIN	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// tableStagesCurrent contains the column name definitions for table events_stages_current, same as MySQL.
/// events_stages_current 表 DDL：当前阶段事件。
pub static tableStagesCurrent: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStagesCurrent,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "WORK_COMPLETED	BIGINT(20) UNSIGNED,",
        "WORK_ESTIMATED	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// tableStagesHistory contains the column name definitions for table events_stages_history, same as MySQL.
/// events_stages_history 表 DDL：阶段事件短历史。
pub static tableStagesHistory: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStagesHistory,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "WORK_COMPLETED	BIGINT(20) UNSIGNED,",
        "WORK_ESTIMATED	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// tableStagesHistoryLong contains the column name definitions for table events_stages_history_long, same as MySQL.
/// events_stages_history_long 表 DDL：阶段事件长历史。
pub static tableStagesHistoryLong: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStagesHistoryLong,
        " (",
        "THREAD_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "EVENT_ID		BIGINT(20) UNSIGNED NOT NULL,",
        "END_EVENT_ID	BIGINT(20) UNSIGNED,",
        "EVENT_NAME		VARCHAR(128) NOT NULL,",
        "SOURCE			VARCHAR(64),",
        "TIMER_START		BIGINT(20) UNSIGNED,",
        "TIMER_END		BIGINT(20) UNSIGNED,",
        "TIMER_WAIT		BIGINT(20) UNSIGNED,",
        "WORK_COMPLETED	BIGINT(20) UNSIGNED,",
        "WORK_ESTIMATED	BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_ID		BIGINT(20) UNSIGNED,",
        "NESTING_EVENT_TYPE		ENUM('TRANSACTION','STATEMENT','STAGE'));",
    ]
    .concat()
});

// digest 汇总表保留计时分位、样本查询和唯一键片段，未尝试修正源 SQL 的大小写或零时间默认值。
// tableEventsStatementsSummaryByDigest contains the column name definitions for table
// events_statements_summary_by_digest, same as MySQL.
/// events_statements_summary_by_digest 表 DDL：按 digest 聚合的语句摘要。
pub static tableEventsStatementsSummaryByDigest: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE if not exists performance_schema.",
        tableNameEventsStatementsSummaryByDigest,
        " (",
        "SCHEMA_NAME varchar(64) DEFAULT NULL,",
        "DIGEST varchar(64) DEFAULT NULL,",
        "DIGEST_TEXT longtext,",
        "COUNT_STAR bigint unsigned NOT NULL,",
        "SUM_TIMER_WAIT bigint unsigned NOT NULL,",
        "MIN_TIMER_WAIT bigint unsigned NOT NULL,",
        "AVG_TIMER_WAIT bigint unsigned NOT NULL,",
        "MAX_TIMER_WAIT bigint unsigned NOT NULL,",
        "SUM_LOCK_TIME bigint unsigned NOT NULL,",
        "SUM_ERRORS bigint unsigned NOT NULL,",
        "SUM_WARNINGS bigint unsigned NOT NULL,",
        "SUM_ROWS_AFFECTED bigint unsigned NOT NULL,",
        "SUM_ROWS_SENT bigint unsigned NOT NULL,",
        "SUM_ROWS_EXAMINED bigint unsigned NOT NULL,",
        "SUM_CREATED_TMP_DISK_TABLES bigint unsigned NOT NULL,",
        "SUM_CREATED_TMP_TABLES bigint unsigned NOT NULL,",
        "SUM_SELECT_FULL_JOIN bigint unsigned NOT NULL,",
        "SUM_SELECT_FULL_RANGE_JOIN bigint unsigned NOT NULL,",
        "SUM_SELECT_RANGE bigint unsigned NOT NULL,",
        "SUM_SELECT_RANGE_CHECK bigint unsigned NOT NULL,",
        "SUM_SELECT_SCAN bigint unsigned NOT NULL,",
        "SUM_SORT_MERGE_PASSES bigint unsigned NOT NULL,",
        "SUM_SORT_RANGE bigint unsigned NOT NULL,",
        "SUM_SORT_ROWS bigint unsigned NOT NULL,",
        "SUM_SORT_SCAN bigint unsigned NOT NULL,",
        "SUM_NO_INDEX_USED bigint unsigned NOT NULL,",
        "SUM_NO_GOOD_INDEX_USED bigint unsigned NOT NULL,",
        "FIRST_SEEN timestamp(6) NOT NULL DEFAULT '0000-00-00 00:00:00.000000',",
        "LAST_SEEN timestamp(6) NOT NULL DEFAULT '0000-00-00 00:00:00.000000',",
        "PLAN_IN_CACHE bool NOT NULL,",
        "PLAN_CACHE_HITS bigint unsigned NOT NULL,",
        "PLAN_IN_BINDING bool NOT NULL,",
        "QUANTILE_95 bigint unsigned NOT NULL,",
        "QUANTILE_99 bigint unsigned NOT NULL,",
        "QUANTILE_999 bigint unsigned NOT NULL,",
        "QUERY_SAMPLE_TEXT longtext,",
        "QUERY_SAMPLE_SEEN timestamp(6) NOT NULL DEFAULT '0000-00-00 00:00:00.000000',",
        "QUERY_SAMPLE_TIMER_WAIT bigint unsigned NOT NULL,",
        "UNIQUE KEY `SCHEMA_NAME` (`SCHEMA_NAME`,`DIGEST`));",
    ]
    .concat()
});

// profile 系列表分别覆盖 TiDB、TiKV 和 PD，节点地址列只在源 DDL 包含时保留。
// tableTiDBProfileCPU contains the columns name definitions for table tidb_profile_cpu
/// tidb_profile_cpu 表 DDL：TiDB CPU pprof 剖析结果。
pub static tableTiDBProfileCPU: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileCPU,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiDBProfileMemory contains the columns name definitions for table tidb_profile_memory
/// tidb_profile_memory 表 DDL：TiDB 堆内存剖析。
pub static tableTiDBProfileMemory: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileMemory,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiDBProfileMutex contains the columns name definitions for table tidb_profile_mutex
/// tidb_profile_mutex 表 DDL：TiDB 互斥锁剖析。
pub static tableTiDBProfileMutex: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileMutex,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiDBProfileAllocs contains the columns name definitions for table tidb_profile_allocs
/// tidb_profile_allocs 表 DDL：TiDB 内存分配剖析。
pub static tableTiDBProfileAllocs: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileAllocs,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiDBProfileBlock contains the columns name definitions for table tidb_profile_block
/// tidb_profile_block 表 DDL：TiDB 阻塞剖析。
pub static tableTiDBProfileBlock: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileBlock,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiDBProfileGoroutines contains the columns name definitions for table tidb_profile_goroutines
/// tidb_profile_goroutines 表 DDL：TiDB goroutine 列表。
pub static tableTiDBProfileGoroutines: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiDBProfileGoroutines,
        " (",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "ID INT(8) NOT NULL,",
        "STATE VARCHAR(16) NOT NULL,",
        "LOCATION VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tableTiKVProfileCPU contains the columns name definitions for table tikv_profile_cpu
/// tikv_profile_cpu 表 DDL：TiKV CPU 远端剖析。
pub static tableTiKVProfileCPU: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameTiKVProfileCPU,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileCPU contains the columns name definitions for table pd_profile_cpu
/// pd_profile_cpu 表 DDL：PD CPU 远端剖析。
pub static tablePDProfileCPU: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileCPU,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileMemory contains the columns name definitions for table pd_profile_cpu_memory
/// pd_profile_memory 表 DDL：PD 堆内存远端剖析。
pub static tablePDProfileMemory: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileMemory,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileMutex contains the columns name definitions for table pd_profile_mutex
/// pd_profile_mutex 表 DDL：PD 互斥锁远端剖析。
pub static tablePDProfileMutex: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileMutex,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileAllocs contains the columns name definitions for table pd_profile_allocs
/// pd_profile_allocs 表 DDL：PD 分配远端剖析。
pub static tablePDProfileAllocs: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileAllocs,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileBlock contains the columns name definitions for table pd_profile_block
/// pd_profile_block 表 DDL：PD 阻塞远端剖析。
pub static tablePDProfileBlock: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileBlock,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "PERCENT_ABS VARCHAR(8) NOT NULL,",
        "PERCENT_REL VARCHAR(8) NOT NULL,",
        "ROOT_CHILD INT(8) NOT NULL,",
        "DEPTH INT(8) NOT NULL,",
        "FILE VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// tablePDProfileGoroutines contains the columns name definitions for table pd_profile_goroutines
/// pd_profile_goroutines 表 DDL：PD goroutine 远端剖析。
pub static tablePDProfileGoroutines: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNamePDProfileGoroutines,
        " (",
        "ADDRESS VARCHAR(64) NOT NULL,",
        "FUNCTION VARCHAR(512) NOT NULL,",
        "ID INT(8) NOT NULL,",
        "STATE VARCHAR(16) NOT NULL,",
        "LOCATION VARCHAR(512) NOT NULL);",
    ]
    .concat()
});

// session 变量、连接属性和按连接状态表保留原始排序与主键定义。
// tableSessionVariables contains the
/// session_variables 表 DDL：当前会话变量。
pub static tableSessionVariables: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameSessionVariables,
        " (",
        "VARIABLE_NAME VARCHAR(64) NOT NULL,",
        "VARIABLE_VALUE VARCHAR(1024) NOT NULL);",
    ]
    .concat()
});

// tableSessionConnectAttrs contains the column name definitions for the table session_connect_attrs
/// session_connect_attrs 表 DDL：会话连接属性。
pub static tableSessionConnectAttrs: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameSessionConnectAttrs,
        " (",
        "PROCESSLIST_ID bigint unsigned NOT NULL,",
        "ATTR_NAME varchar(32) COLLATE utf8mb4_bin NOT NULL,",
        "ATTR_VALUE varchar(1024) COLLATE utf8mb4_bin DEFAULT NULL,",
        "ORDINAL_POSITION int DEFAULT NULL);",
    ]
    .concat()
});

// tableSessionAccountConnectAttrs contains the column name definitions for the table session_connect_attrs
/// session_account_connect_attrs 表 DDL：按账户过滤的连接属性。
pub static tableSessionAccountConnectAttrs: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameSessionAccountConnectAttrs,
        " (",
        "PROCESSLIST_ID bigint unsigned NOT NULL,",
        "ATTR_NAME varchar(32) COLLATE utf8mb4_bin NOT NULL,",
        "ATTR_VALUE varchar(1024) COLLATE utf8mb4_bin DEFAULT NULL,",
        "ORDINAL_POSITION int DEFAULT NULL);",
    ]
    .concat()
});

// tableStatusByConnection contains the column name definitions for the table status_by_connection
/// status_by_connection 表 DDL：按连接维度的状态。
pub static tableStatusByConnection: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE IF NOT EXISTS ",
        tableNameStatusByConnection,
        " (",
        "CONNECTION_ID bigint unsigned NOT NULL,",
        "VARIABLE_NAME varchar(64) NOT NULL,",
        "VARIABLE_VALUE varchar(1024) DEFAULT NULL,",
        "PRIMARY KEY (CONNECTION_ID,VARIABLE_NAME));",
    ]
    .concat()
});

/// cond_instances 表 DDL：条件变量同步对象实例。
pub static tableCondInstances: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE performance_schema.",
        tableNameCondInstances,
        " (",
        "NAME VARCHAR(128) NOT NULL,",
        "OBJECT_INSTANCE_BEGIN BIGINT UNSIGNED NOT NULL);",
    ]
    .concat()
});

fn events_waits_table(name: &str) -> String {
    format!(
        "CREATE TABLE performance_schema.{name} (\
         THREAD_ID BIGINT UNSIGNED NOT NULL,\
         EVENT_ID BIGINT UNSIGNED NOT NULL,\
         END_EVENT_ID BIGINT UNSIGNED,\
         EVENT_NAME VARCHAR(128) NOT NULL,\
         SOURCE VARCHAR(64),\
         TIMER_START BIGINT UNSIGNED,\
         TIMER_END BIGINT UNSIGNED,\
         TIMER_WAIT BIGINT UNSIGNED,\
         SPINS INT UNSIGNED,\
         OBJECT_SCHEMA VARCHAR(64),\
         OBJECT_NAME VARCHAR(512),\
         INDEX_NAME VARCHAR(64),\
         OBJECT_TYPE VARCHAR(64),\
         OBJECT_INSTANCE_BEGIN BIGINT UNSIGNED NOT NULL,\
         NESTING_EVENT_ID BIGINT UNSIGNED,\
         NESTING_EVENT_TYPE VARCHAR(64),\
         OPERATION VARCHAR(32) NOT NULL,\
         NUMBER_OF_BYTES BIGINT,\
         FLAGS INT UNSIGNED);"
    )
}

/// events_waits_current 表 DDL：线程当前等待事件。
pub static tableEventsWaitsCurrent: LazyLock<String> =
    LazyLock::new(|| events_waits_table(tableNameEventsWaitsCurrent));
/// events_waits_history 表 DDL：线程最近等待事件。
pub static tableEventsWaitsHistory: LazyLock<String> =
    LazyLock::new(|| events_waits_table(tableNameEventsWaitsHistory));
/// events_waits_history_long 表 DDL：全局最近等待事件。
pub static tableEventsWaitsHistoryLong: LazyLock<String> =
    LazyLock::new(|| events_waits_table(tableNameEventsWaitsHistoryLong));

fn connection_summary_table(name: &str, identity_columns: &str) -> String {
    format!(
        "CREATE TABLE performance_schema.{name} (\
         {identity_columns}\
         CURRENT_CONNECTIONS BIGINT NOT NULL,\
         TOTAL_CONNECTIONS BIGINT NOT NULL,\
         MAX_SESSION_CONTROLLED_MEMORY BIGINT UNSIGNED NOT NULL,\
         MAX_SESSION_TOTAL_MEMORY BIGINT UNSIGNED NOT NULL);"
    )
}

/// accounts 表 DDL：按用户和主机汇总连接。
pub static tableAccounts: LazyLock<String> =
    LazyLock::new(|| connection_summary_table(tableNameAccounts, "USER CHAR(32), HOST CHAR(255),"));
/// hosts 表 DDL：按主机汇总连接。
pub static tableHosts: LazyLock<String> =
    LazyLock::new(|| connection_summary_table(tableNameHosts, "HOST CHAR(255),"));
/// users 表 DDL：按用户汇总连接。
pub static tableUsers: LazyLock<String> =
    LazyLock::new(|| connection_summary_table(tableNameUsers, "USER CHAR(32),"));

/// binary_log_transaction_compression_stats 表 DDL：二进制日志事务压缩统计。
pub static tableBinaryLogTransactionCompressionStats: LazyLock<String> = LazyLock::new(|| {
    [
        "CREATE TABLE performance_schema.",
        tableNameBinaryLogTransactionCompressionStats,
        " (",
        "LOG_TYPE ENUM('BINARY','RELAY') NOT NULL,",
        "COMPRESSION_TYPE VARCHAR(64) NOT NULL,",
        "TRANSACTION_COUNTER BIGINT UNSIGNED NOT NULL,",
        "COMPRESSED_BYTES_COUNTER BIGINT UNSIGNED NOT NULL,",
        "UNCOMPRESSED_BYTES_COUNTER BIGINT UNSIGNED NOT NULL,",
        "COMPRESSION_PERCENTAGE SMALLINT SIGNED NOT NULL);",
    ]
    .concat()
});

/// events_transactions_summary_by_user_by_event_name 表 DDL：按用户和事件名汇总事务。
pub static tableEventsTransactionsSummaryByUserByEventName: LazyLock<String> =
    LazyLock::new(|| {
        [
            "CREATE TABLE performance_schema.",
            tableNameEventsTransactionsSummaryByUserByEventName,
            " (",
            "USER CHAR(32) COLLATE utf8mb4_bin DEFAULT NULL,",
            "EVENT_NAME VARCHAR(128) NOT NULL,",
            "COUNT_STAR BIGINT UNSIGNED NOT NULL,",
            "SUM_TIMER_WAIT BIGINT UNSIGNED NOT NULL,",
            "MIN_TIMER_WAIT BIGINT UNSIGNED NOT NULL,",
            "AVG_TIMER_WAIT BIGINT UNSIGNED NOT NULL,",
            "MAX_TIMER_WAIT BIGINT UNSIGNED NOT NULL,",
            "COUNT_READ_WRITE BIGINT UNSIGNED NOT NULL,",
            "SUM_TIMER_READ_WRITE BIGINT UNSIGNED NOT NULL,",
            "MIN_TIMER_READ_WRITE BIGINT UNSIGNED NOT NULL,",
            "AVG_TIMER_READ_WRITE BIGINT UNSIGNED NOT NULL,",
            "MAX_TIMER_READ_WRITE BIGINT UNSIGNED NOT NULL,",
            "COUNT_READ_ONLY BIGINT UNSIGNED NOT NULL,",
            "SUM_TIMER_READ_ONLY BIGINT UNSIGNED NOT NULL,",
            "MIN_TIMER_READ_ONLY BIGINT UNSIGNED NOT NULL,",
            "AVG_TIMER_READ_ONLY BIGINT UNSIGNED NOT NULL,",
            "MAX_TIMER_READ_ONLY BIGINT UNSIGNED NOT NULL,",
            "UNIQUE KEY (USER, EVENT_NAME) USING HASH);",
        ]
        .concat()
    });
