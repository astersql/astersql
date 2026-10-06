// Copyright 2025 PingCAP, Inc.
// Copyright 2026 AsterSQL.
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

// MySQL 兼容系统变量名常量，以及 SET NAMES/CHARSET 相关分组。
//
// 系统变量（system variable）是会话/全局可配置项，如字符集、超时、事务隔离级别等。
// 本文件仅存放变量名字面量，供 SET/SHOW 与 SysVar 注册表引用；顺序对齐 Go `sysvar.go`。

#![allow(dead_code, non_snake_case, non_upper_case_globals, unused_variables)]

// SetNamesVariables is the system variable names related to set names statements.
/// `SET NAMES` 语句会一并修改的系统变量名列表。
///
/// 对应 client/connection/results 三套字符集。
pub const SetNamesVariables: [&str; 3] = [
    CharacterSetClient,
    CharacterSetConnection,
    CharacterSetResults,
];

// SetCharsetVariables is the system variable names related to set charset statements.
/// `SET CHARACTER SET` / `SET CHARSET` 相关的系统变量名列表。
pub const SetCharsetVariables: [&str; 2] = [CharacterSetClient, CharacterSetResults];

// Go const block：以下常量按原声明顺序逐项迁移。
// MaskPwd is the mask of password for LDAP variables.
/// LDAP 等场景打印密码时使用的掩码字符串。
pub const MaskPwd: &str = "******";

// PessimisticTxnMode is the name for tidb_txn_mode system variable.
/// `tidb_txn_mode` 的悲观事务（pessimistic）取值。
///
/// 悲观事务在写冲突时加锁等待，而非乐观重试。
pub const PessimisticTxnMode: &str = "pessimistic";
// OptimisticTxnMode is the name for tidb_txn_mode system variable.
/// `tidb_txn_mode` 的乐观事务（optimistic）取值。
pub const OptimisticTxnMode: &str = "optimistic";

// Go const block：以下常量按原声明顺序逐项迁移。
// CharacterSetConnection is the name for character_set_connection system variable.
// 以下为 MySQL/TiDB 系统变量名字面量；英文注释保留 Go 原意，此处用分区说明概括。
// 字符集与排序规则（collation）相关：
pub const CharacterSetConnection: &str = "character_set_connection";
// CollationConnection is the name for collation_connection system variable.
pub const CollationConnection: &str = "collation_connection";
// CharsetDatabase is the name for character_set_database system variable.
pub const CharsetDatabase: &str = "character_set_database";
// CollationDatabase is the name for collation_database system variable.
pub const CollationDatabase: &str = "collation_database";
// CharacterSetFilesystem is the name for character_set_filesystem system variable.
pub const CharacterSetFilesystem: &str = "character_set_filesystem";
// CharacterSetClient is the name for character_set_client system variable.
pub const CharacterSetClient: &str = "character_set_client";
// CharacterSetSystem is the name for character_set_system system variable.
pub const CharacterSetSystem: &str = "character_set_system";
// GeneralLog is the name for 'general_log' system variable.
// 通用日志、连接与会话行为相关：
pub const GeneralLog: &str = "general_log";
// AvoidTemporalUpgrade is the name for 'avoid_temporal_upgrade' system variable.
pub const AvoidTemporalUpgrade: &str = "avoid_temporal_upgrade";
// MaxPreparedStmtCount is the name for 'max_prepared_stmt_count' system variable.
pub const MaxPreparedStmtCount: &str = "max_prepared_stmt_count";
// BigTables is the name for 'big_tables' system variable.
pub const BigTables: &str = "big_tables";
// CheckProxyUsers is the name for 'check_proxy_users' system variable.
pub const CheckProxyUsers: &str = "check_proxy_users";
// CoreFile is the name for 'core_file' system variable.
pub const CoreFile: &str = "core_file";
// DefaultWeekFormat is the name for 'default_week_format' system variable.
pub const DefaultWeekFormat: &str = "default_week_format";
// GroupConcatMaxLen is the name for 'group_concat_max_len' system variable.
pub const GroupConcatMaxLen: &str = "group_concat_max_len";
// DelayKeyWrite is the name for 'delay_key_write' system variable.
pub const DelayKeyWrite: &str = "delay_key_write";
// EndMarkersInJSON is the name for 'end_markers_in_json' system variable.
pub const EndMarkersInJSON: &str = "end_markers_in_json";
// Hostname is the name for 'hostname' system variable.
pub const Hostname: &str = "hostname";
// InnodbCommitConcurrency is the name for 'innodb_commit_concurrency' system variable.
// InnoDB/MySQL 兼容占位变量（TiDB 中多为 noop 或有限支持）：
pub const InnodbCommitConcurrency: &str = "innodb_commit_concurrency";
// InnodbFastShutdown is the name for 'innodb_fast_shutdown' system variable.
pub const InnodbFastShutdown: &str = "innodb_fast_shutdown";
// InnodbLockWaitTimeout is the name for 'innodb_lock_wait_timeout' system variable.
pub const InnodbLockWaitTimeout: &str = "innodb_lock_wait_timeout";
// MaxSortLength is the name for 'max_sort_length' system variable.
pub const MaxSortLength: &str = "max_sort_length";
// MaxSpRecursionDepth is the name for 'max_sp_recursion_depth' system variable.
pub const MaxSpRecursionDepth: &str = "max_sp_recursion_depth";
// MaxUserConnections is the name for 'max_user_connections' system variable.
pub const MaxUserConnections: &str = "max_user_connections";
// OfflineMode is the name for 'offline_mode' system variable.
pub const OfflineMode: &str = "offline_mode";
// InteractiveTimeout is the name for 'interactive_timeout' system variable.
pub const InteractiveTimeout: &str = "interactive_timeout";
// FlushTime is the name for 'flush_time' system variable.
pub const FlushTime: &str = "flush_time";
// PseudoSlaveMode is the name for 'pseudo_slave_mode' system variable.
pub const PseudoSlaveMode: &str = "pseudo_slave_mode";
// LowPriorityUpdates is the name for 'low_priority_updates' system variable.
pub const LowPriorityUpdates: &str = "low_priority_updates";
// LowerCaseTableNames is the name for 'lower_case_table_names' system variable.
pub const LowerCaseTableNames: &str = "lower_case_table_names";
// SessionTrackGtids is the name for 'session_track_gtids' system variable.
pub const SessionTrackGtids: &str = "session_track_gtids";
// OldPasswords is the name for 'old_passwords' system variable.
pub const OldPasswords: &str = "old_passwords";
// MaxConnections is the name for 'max_connections' system variable.
pub const MaxConnections: &str = "max_connections";
// SkipNameResolve is the name for 'skip_name_resolve' system variable.
pub const SkipNameResolve: &str = "skip_name_resolve";
// ForeignKeyChecks is the name for 'foreign_key_checks' system variable.
pub const ForeignKeyChecks: &str = "foreign_key_checks";
// SQLSafeUpdates is the name for 'sql_safe_updates' system variable.
pub const SQLSafeUpdates: &str = "sql_safe_updates";
// WarningCount is the name for 'warning_count' system variable.
pub const WarningCount: &str = "warning_count";
// ErrorCount is the name for 'error_count' system variable.
pub const ErrorCount: &str = "error_count";
// DefaultPasswordLifetime is the name for 'default_password_lifetime' system variable.
pub const DefaultPasswordLifetime: &str = "default_password_lifetime";
// DisconnectOnExpiredPassword is the name for 'disconnect_on_expired_password' system variable.
pub const DisconnectOnExpiredPassword: &str = "disconnect_on_expired_password";
// SQLSelectLimit is the name for 'sql_select_limit' system variable.
pub const SQLSelectLimit: &str = "sql_select_limit";
// MaxConnectErrors is the name for 'max_connect_errors' system variable.
pub const MaxConnectErrors: &str = "max_connect_errors";
// TableDefinitionCache is the name for 'table_definition_cache' system variable.
pub const TableDefinitionCache: &str = "table_definition_cache";
// Timestamp is the name for 'timestamp' system variable.
pub const Timestamp: &str = "timestamp";
// ConnectTimeout is the name for 'connect_timeout' system variable.
pub const ConnectTimeout: &str = "connect_timeout";
// SyncBinlog is the name for 'sync_binlog' system variable.
pub const SyncBinlog: &str = "sync_binlog";
// BlockEncryptionMode is the name for 'block_encryption_mode' system variable.
pub const BlockEncryptionMode: &str = "block_encryption_mode";
// WaitTimeout is the name for 'wait_timeout' system variable.
pub const WaitTimeout: &str = "wait_timeout";
// Version is the name of 'version' system variable.
pub const Version: &str = "version";
// VersionComment is the name of 'version_comment' system variable.
pub const VersionComment: &str = "version_comment";
// PluginDir is the name of 'plugin_dir' system variable.
pub const PluginDir: &str = "plugin_dir";
// PluginLoad is the name of 'plugin_load' system variable.
pub const PluginLoad: &str = "plugin_load";
// PluginAuditLogBufferSize is the name of 'plugin_audit_log_buffer_size' system variable.
pub const PluginAuditLogBufferSize: &str = "plugin_audit_log_buffer_size";
// PluginAuditLogFlushInterval is the name of 'plugin_audit_log_flush_interval' system variable.
pub const PluginAuditLogFlushInterval: &str = "plugin_audit_log_flush_interval";
// TiDBEnableDDL indicates whether the tidb-server campaigns the DDL owner,
// TiDB 服务端竞选 DDL/Stats Owner 开关：
pub const TiDBEnableDDL: &str = "tidb_enable_ddl";
// TiDBEnableStatsOwner indicates whether the tidb-server campaigns the Stats owner,
pub const TiDBEnableStatsOwner: &str = "tidb_enable_stats_owner";
// Port is the name for 'port' system variable.
pub const Port: &str = "port";
// DataDir is the name for 'datadir' system variable.
pub const DataDir: &str = "datadir";
// Profiling is the name for 'Profiling' system variable.
pub const Profiling: &str = "profiling";
// Socket is the name for 'socket' system variable.
pub const Socket: &str = "socket";
// BinlogOrderCommits is the name for 'binlog_order_commits' system variable.
pub const BinlogOrderCommits: &str = "binlog_order_commits";
// MasterVerifyChecksum is the name for 'master_verify_checksum' system variable.
pub const MasterVerifyChecksum: &str = "master_verify_checksum";
// SuperReadOnly is the name for 'super_read_only' system variable.
pub const SuperReadOnly: &str = "super_read_only";
// SQLNotes is the name for 'sql_notes' system variable.
pub const SQLNotes: &str = "sql_notes";
// SlaveCompressedProtocol is the name for 'slave_compressed_protocol' system variable.
pub const SlaveCompressedProtocol: &str = "slave_compressed_protocol";
// BinlogRowQueryLogEvents is the name for 'binlog_rows_query_log_events' system variable.
pub const BinlogRowQueryLogEvents: &str = "binlog_rows_query_log_events";
// LogSlowSlaveStatements is the name for 'log_slow_slave_statements' system variable.
pub const LogSlowSlaveStatements: &str = "log_slow_slave_statements";
// LogSlowAdminStatements is the name for 'log_slow_admin_statements' system variable.
pub const LogSlowAdminStatements: &str = "log_slow_admin_statements";
// LogQueriesNotUsingIndexes is the name for 'log_queries_not_using_indexes' system variable.
pub const LogQueriesNotUsingIndexes: &str = "log_queries_not_using_indexes";
// SQLAutoIsNull is the name for 'sql_auto_is_null' system variable.
pub const SQLAutoIsNull: &str = "sql_auto_is_null";
// RelayLogPurge is the name for 'relay_log_purge' system variable.
pub const RelayLogPurge: &str = "relay_log_purge";
// AutomaticSpPrivileges is the name for 'automatic_sp_privileges' system variable.
pub const AutomaticSpPrivileges: &str = "automatic_sp_privileges";
// SQLQuoteShowCreate is the name for 'sql_quote_show_create' system variable.
pub const SQLQuoteShowCreate: &str = "sql_quote_show_create";
// SlowQueryLog is the name for 'slow_query_log' system variable.
pub const SlowQueryLog: &str = "slow_query_log";
// BinlogDirectNonTransactionalUpdates is the name for 'binlog_direct_non_transactional_updates' system variable.
pub const BinlogDirectNonTransactionalUpdates: &str = "binlog_direct_non_transactional_updates";
// SQLBigSelects is the name for 'sql_big_selects' system variable.
pub const SQLBigSelects: &str = "sql_big_selects";
// LogBinTrustFunctionCreators is the name for 'log_bin_trust_function_creators' system variable.
pub const LogBinTrustFunctionCreators: &str = "log_bin_trust_function_creators";
// OldAlterTable is the name for 'old_alter_table' system variable.
pub const OldAlterTable: &str = "old_alter_table";
// EnforceGtidConsistency is the name for 'enforce_gtid_consistency' system variable.
pub const EnforceGtidConsistency: &str = "enforce_gtid_consistency";
// SecureAuth is the name for 'secure_auth' system variable.
pub const SecureAuth: &str = "secure_auth";
// UniqueChecks is the name for 'unique_checks' system variable.
pub const UniqueChecks: &str = "unique_checks";
// SQLWarnings is the name for 'sql_warnings' system variable.
pub const SQLWarnings: &str = "sql_warnings";
// AutoCommit is the name for 'autocommit' system variable.
pub const AutoCommit: &str = "autocommit";
// KeepFilesOnCreate is the name for 'keep_files_on_create' system variable.
pub const KeepFilesOnCreate: &str = "keep_files_on_create";
// ShowOldTemporals is the name for 'show_old_temporals' system variable.
pub const ShowOldTemporals: &str = "show_old_temporals";
// LocalInFile is the name for 'local_infile' system variable.
pub const LocalInFile: &str = "local_infile";
// PerformanceSchema is the name for 'performance_schema' system variable.
pub const PerformanceSchema: &str = "performance_schema";
// PerformanceSchemaSessionConnectAttrsSize is the name for 'performance_schema_session_connect_attrs_size' system variable.
pub const PerformanceSchemaSessionConnectAttrsSize: &str =
    "performance_schema_session_connect_attrs_size";
// Flush is the name for 'flush' system variable.
pub const Flush: &str = "flush";
// SlaveAllowBatching is the name for 'slave_allow_batching' system variable.
pub const SlaveAllowBatching: &str = "slave_allow_batching";
// MyISAMUseMmap is the name for 'myisam_use_mmap' system variable.
pub const MyISAMUseMmap: &str = "myisam_use_mmap";
// InnodbFilePerTable is the name for 'innodb_file_per_table' system variable.
pub const InnodbFilePerTable: &str = "innodb_file_per_table";
// InnodbLogCompressedPages is the name for 'innodb_log_compressed_pages' system variable.
pub const InnodbLogCompressedPages: &str = "innodb_log_compressed_pages";
// InnodbPrintAllDeadlocks is the name for 'innodb_print_all_deadlocks' system variable.
pub const InnodbPrintAllDeadlocks: &str = "innodb_print_all_deadlocks";
// InnodbStrictMode is the name for 'innodb_strict_mode' system variable.
pub const InnodbStrictMode: &str = "innodb_strict_mode";
// InnodbCmpPerIndexEnabled is the name for 'innodb_cmp_per_index_enabled' system variable.
pub const InnodbCmpPerIndexEnabled: &str = "innodb_cmp_per_index_enabled";
// InnodbBufferPoolDumpAtShutdown is the name for 'innodb_buffer_pool_dump_at_shutdown' system variable.
pub const InnodbBufferPoolDumpAtShutdown: &str = "innodb_buffer_pool_dump_at_shutdown";
// InnodbAdaptiveHashIndex is the name for 'innodb_adaptive_hash_index' system variable.
pub const InnodbAdaptiveHashIndex: &str = "innodb_adaptive_hash_index";
// InnodbFtEnableStopword is the name for 'innodb_ft_enable_stopword' system variable.
// #nosec G101
pub const InnodbFtEnableStopword: &str = "innodb_ft_enable_stopword";
// InnodbOptimizeFullTextOnly is the name for 'innodb_optimize_fulltext_only' system variable.
pub const InnodbOptimizeFullTextOnly: &str = "innodb_optimize_fulltext_only";
// InnodbStatusOutputLocks is the name for 'innodb_status_output_locks' system variable.
pub const InnodbStatusOutputLocks: &str = "innodb_status_output_locks";
// InnodbBufferPoolDumpNow is the name for 'innodb_buffer_pool_dump_now' system variable.
pub const InnodbBufferPoolDumpNow: &str = "innodb_buffer_pool_dump_now";
// InnodbBufferPoolLoadNow is the name for 'innodb_buffer_pool_load_now' system variable.
pub const InnodbBufferPoolLoadNow: &str = "innodb_buffer_pool_load_now";
// InnodbStatsOnMetadata is the name for 'innodb_stats_on_metadata' system variable.
pub const InnodbStatsOnMetadata: &str = "innodb_stats_on_metadata";
// InnodbDisableSortFileCache is the name for 'innodb_disable_sort_file_cache' system variable.
pub const InnodbDisableSortFileCache: &str = "innodb_disable_sort_file_cache";
// InnodbStatsAutoRecalc is the name for 'innodb_stats_auto_recalc' system variable.
pub const InnodbStatsAutoRecalc: &str = "innodb_stats_auto_recalc";
// InnodbBufferPoolLoadAbort is the name for 'innodb_buffer_pool_load_abort' system variable.
pub const InnodbBufferPoolLoadAbort: &str = "innodb_buffer_pool_load_abort";
// InnodbStatsPersistent is the name for 'innodb_stats_persistent' system variable.
pub const InnodbStatsPersistent: &str = "innodb_stats_persistent";
// InnodbRandomReadAhead is the name for 'innodb_random_read_ahead' system variable.
pub const InnodbRandomReadAhead: &str = "innodb_random_read_ahead";
// InnodbAdaptiveFlushing is the name for 'innodb_adaptive_flushing' system variable.
pub const InnodbAdaptiveFlushing: &str = "innodb_adaptive_flushing";
// InnodbTableLocks is the name for 'innodb_table_locks' system variable.
pub const InnodbTableLocks: &str = "innodb_table_locks";
// InnodbStatusOutput is the name for 'innodb_status_output' system variable.
pub const InnodbStatusOutput: &str = "innodb_status_output";
// NetBufferLength is the name for 'net_buffer_length' system variable.
pub const NetBufferLength: &str = "net_buffer_length";
// TxReadOnly is the name of 'tx_read_only' system variable.
// 事务只读、服务端字符集与自增偏移等：
pub const TxReadOnly: &str = "tx_read_only";
// TransactionReadOnly is the name of 'transaction_read_only' system variable.
pub const TransactionReadOnly: &str = "transaction_read_only";
// CharacterSetServer is the name of 'character_set_server' system variable.
pub const CharacterSetServer: &str = "character_set_server";
// AutoIncrementIncrement is the name of 'auto_increment_increment' system variable.
pub const AutoIncrementIncrement: &str = "auto_increment_increment";
// AutoIncrementOffset is the name of 'auto_increment_offset' system variable.
pub const AutoIncrementOffset: &str = "auto_increment_offset";
// InitConnect is the name of 'init_connect' system variable.
pub const InitConnect: &str = "init_connect";
// CollationServer is the name of 'collation_server' variable.
pub const CollationServer: &str = "collation_server";
// DefaultCollationForUTF8MB4 is the name of 'default_collation_for_utf8mb4' variable.
pub const DefaultCollationForUTF8MB4: &str = "default_collation_for_utf8mb4";
// NetWriteTimeout is the name of 'net_write_timeout' variable.
pub const NetWriteTimeout: &str = "net_write_timeout";
// ThreadPoolSize is the name of 'thread_pool_size' variable.
pub const ThreadPoolSize: &str = "thread_pool_size";
// WindowingUseHighPrecision is the name of 'windowing_use_high_precision' system variable.
pub const WindowingUseHighPrecision: &str = "windowing_use_high_precision";
// OptimizerSwitch is the name of 'optimizer_switch' system variable.
pub const OptimizerSwitch: &str = "optimizer_switch";
// SystemTimeZone is the name of 'system_time_zone' system variable.
pub const SystemTimeZone: &str = "system_time_zone";
// CTEMaxRecursionDepth is the name of 'cte_max_recursion_depth' system variable.
pub const CTEMaxRecursionDepth: &str = "cte_max_recursion_depth";
// SQLModeVar is the name of the 'sql_mode' system variable.
// SQL 模式、结果字符集、网络与隔离级别等核心会话变量：
pub const SQLModeVar: &str = "sql_mode";
// CharacterSetResults is the name of the 'character_set_results' system variable.
pub const CharacterSetResults: &str = "character_set_results";
// MaxAllowedPacket is the name of the 'max_allowed_packet' system variable.
pub const MaxAllowedPacket: &str = "max_allowed_packet";
// TimeZone is the name of the 'time_zone' system variable.
pub const TimeZone: &str = "time_zone";
// TxnIsolation is the name of the 'tx_isolation' system variable.
pub const TxnIsolation: &str = "tx_isolation";
// TransactionIsolation is the name of the 'transaction_isolation' system variable.
pub const TransactionIsolation: &str = "transaction_isolation";
// TxnIsolationOneShot is the name of the 'tx_isolation_one_shot' system variable.
pub const TxnIsolationOneShot: &str = "tx_isolation_one_shot";
// MaxExecutionTime is the name of the 'max_execution_time' system variable.
pub const MaxExecutionTime: &str = "max_execution_time";
// TiDBDMLMaxExecutionTime is the maximum execution time for transactional DML and COMMIT.
pub const TiDBDMLMaxExecutionTime: &str = "tidb_dml_max_execution_time";
// TiDBMaxKeysRead is the name of the 'tidb_max_keys_read' system variable.
pub const TiDBMaxKeysRead: &str = "tidb_max_keys_read";
// TiKVClientReadTimeout is the name of the 'tikv_client_read_timeout' system variable.
pub const TiKVClientReadTimeout: &str = "tikv_client_read_timeout";
// TiDBLoadBindingTimeout is the name of the 'tidb_load_binding_timeout' system variable.
pub const TiDBLoadBindingTimeout: &str = "tidb_load_binding_timeout";
// TiDBEnableBindingUsage is the name of the 'tidb_enable_binding_usage' system variable.
pub const TiDBEnableBindingUsage: &str = "tidb_enable_binding_usage";
// ReadOnly is the name of the 'read_only' system variable.
pub const ReadOnly: &str = "read_only";
// DefaultAuthPlugin is the name of 'default_authentication_plugin' system variable.
pub const DefaultAuthPlugin: &str = "default_authentication_plugin";
// LastInsertID is the name of 'last_insert_id' system variable.
pub const LastInsertID: &str = "last_insert_id";
// Identity is the name of 'identity' system variable.
pub const Identity: &str = "identity";
// TiDBAllowFunctionForExpressionIndex is the name of `TiDBAllowFunctionForExpressionIndex` system variable.
pub const TiDBAllowFunctionForExpressionIndex: &str = "tidb_allow_function_for_expression_index";
// RandSeed1 is the name of 'rand_seed1' system variable.
pub const RandSeed1: &str = "rand_seed1";
// RandSeed2 is the name of 'rand_seed2' system variable.
pub const RandSeed2: &str = "rand_seed2";
// SQLRequirePrimaryKey is the name of `sql_require_primary_key` system variable.
pub const SQLRequirePrimaryKey: &str = "sql_require_primary_key";
// ValidatePasswordEnable turns on/off the validation of password.
// 密码策略校验（validate_password）插件相关变量：
pub const ValidatePasswordEnable: &str = "validate_password.enable";
// ValidatePasswordPolicy specifies the password policy enforced by validate_password.
pub const ValidatePasswordPolicy: &str = "validate_password.policy";
// ValidatePasswordCheckUserName controls whether validate_password compares passwords to the user name part of
// the effective user account for the current session
pub const ValidatePasswordCheckUserName: &str = "validate_password.check_user_name";
// ValidatePasswordLength specified the minimum number of characters that validate_password requires passwords to have
pub const ValidatePasswordLength: &str = "validate_password.length";
// ValidatePasswordMixedCaseCount specified the minimum number of lowercase and uppercase characters that validate_password requires
pub const ValidatePasswordMixedCaseCount: &str = "validate_password.mixed_case_count";
// ValidatePasswordNumberCount specified the minimum number of numeric (digit) characters that validate_password requires
pub const ValidatePasswordNumberCount: &str = "validate_password.number_count";
// ValidatePasswordSpecialCharCount specified the minimum number of nonalphanumeric characters that validate_password requires
pub const ValidatePasswordSpecialCharCount: &str = "validate_password.special_char_count";
// ValidatePasswordDictionary specified the dictionary that validate_password uses for checking passwords. Each word is separated by semicolon (;).
pub const ValidatePasswordDictionary: &str = "validate_password.dictionary";
