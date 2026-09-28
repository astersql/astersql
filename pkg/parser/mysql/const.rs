// Copyright 2026 AsterSQL.
// Copyright 2017 PingCAP, Inc.
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//     http://www.apache.org/licenses/LICENSE-2.0
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express or implied.
// See the License for the specific language governing permissions and
// limitations under the License.

// MySQL/TiDB 协议与会话相关常量，以及 SQL Mode、版本字符串工具。
//
// 涵盖包头字节、服务器状态位、COM_* 命令、客户端 capability、
// 认证插件名、系统表名、类型显示宽度，以及 `SQLMode` 位运算与解析。

// 进行字符串/位标志转换和内存查表。
// 版本解析使用 Rust semver crate；优先级恢复使用标准库字符串边界。
use std::collections::HashSet;

use semver::Version;

use super::errcode::*;
use super::error::{NewErr, NewErrf, SQLError};
use super::r#type::*;

// newInvalidModeErr 对应未知 sql_mode 的标准 MySQL 错误构造。
/// 构造未知 sql_mode 的标准错误。
fn newInvalidModeErr(mode: &str) -> SQLError {
    NewErr(ErrWrongValueForVar, vec!["sql_mode".into(), mode.into()])
}

/// 构造版本相关变量错误。
fn newVersionErr(message: String) -> SQLError {
    NewErrf(ErrWrongValueForVar, "%s", &[], vec![message.into()])
}

/// 对外宣称的 MySQL 兼容版本号。
const mysqlCompatibilityVersion: &str = "8.0.11";
// VersionSeparator 会参与 PD 中 ServerVersion 的拼接和反向解析，不可修改。
/// ServerVersion 中 MySQL 与 TiDB 版本的分隔符。
pub const VersionSeparator: &str = "-TiDB-";
/// TiDBX/CLOUD 发布版本前缀。
const tidbXReleaseVersionPrefix: &str = "CLOUD.";
/// 经典 TiDB 构建占位版本。
const legacyTiDBReleaseVersionPlaceholder: &str = "v8.4.0-this-is-a-placeholder";
/// TiDBX 构建占位版本。
const tidbXPlaceholderReleaseVersion: &str = "v26.3.0-this-is-a-placeholder";
/// TiDBX 版本年份下限。
pub const TiDBXVerMinYear: u64 = 2025;
/// TiDBX 版本年份上限。
pub const TiDBXVerMaxYear: u64 = 2099;

// TiDBReleaseVersion 在 Go 中由构建参数覆盖；保留默认值和派生 ServerVersion 函数形状。
/// 可由构建注入的 TiDB 发布版本。
pub static mut TiDBReleaseVersion: &str = legacyTiDBReleaseVersionPlaceholder;
/// 拼接 MySQL 兼容版本与 TiDB 发布版本。
pub fn ServerVersion() -> String {
    format!(
        "{}{}{}",
        mysqlCompatibilityVersion,
        VersionSeparator,
        unsafe { TiDBReleaseVersion }
    )
}

// IDE 启动 next-gen 且未注入版本时，把 classic 占位版本改写为 CLOUD 版本占位值。
/// next-gen 下将经典占位版本改写为 CLOUD 占位。
pub fn NormalizeTiDBReleaseVersionForNextGen(release: &str) -> String {
    if release == legacyTiDBReleaseVersionPlaceholder {
        tidbXPlaceholderReleaseVersion.into()
    } else {
        release.into()
    }
}

// BuildTiDBXReleaseVersion 校验 vYY.MM.PATCH[-pre]，并输出 CLOUD.YYYYMM.PATCH[-pre]。
/// 将 `vYY.MM.PATCH` 转为 `CLOUD.YYYYMM.PATCH`。
pub fn BuildTiDBXReleaseVersion(release: &str) -> Result<String, SQLError> {
    let raw = release.strip_prefix('v').ok_or_else(|| {
        newVersionErr(format!(
            "invalid TiDB release version {:?}, should start with 'v'",
            release
        ))
    })?;
    let version = Version::parse(raw).map_err(|_| invalidSemanticVersion(release))?;
    let year = 2000 + version.major;
    if !(TiDBXVerMinYear..=TiDBXVerMaxYear).contains(&year) || !(1..=12).contains(&version.minor) {
        return Err(newVersionErr(format!(
            "invalid TiDB release version {:?}, the semantic version part should be in [2-digit-year].[month].[fix-version]-[xxx] format",
            release
        )));
    }
    let pre = if version.pre.is_empty() {
        String::new()
    } else {
        format!("-{}", version.pre)
    };
    Ok(format!(
        "{}{}{:02}.{}{}",
        tidbXReleaseVersionPrefix, year, version.minor, version.patch, pre
    ))
}

/// 语义化版本解析失败时的错误。
fn invalidSemanticVersion(release: &str) -> SQLError {
    newVersionErr(format!(
        "invalid TiDB release version {:?}, expect a semantic version",
        release
    ))
}

/// 构建带 TiDBX 发布版本的完整 ServerVersion。
pub fn BuildTiDBXServerVersion(release: &str) -> Result<String, SQLError> {
    Ok(format!(
        "{}{}{}",
        mysqlCompatibilityVersion,
        VersionSeparator,
        BuildTiDBXReleaseVersion(release)?
    ))
}

// MySQL packet header bytes。
/// OK 包头字节。
pub const OKHeader: u8 = 0x00;
/// Error 包头字节。
pub const ErrHeader: u8 = 0xff;
/// EOF 包头字节。
pub const EOFHeader: u8 = 0xfe;
/// LOCAL INFILE 请求包头。
pub const LocalInFileHeader: u8 = 0xfb;
/// 认证方式切换请求（与 EOF 同值）。
pub const AuthSwitchRequest: u8 = 0xfe;

// server status 位与协议定义一致。
/// 服务器状态：事务中。
pub const ServerStatusInTrans: u16 = 0x0001;
pub const ServerStatusAutocommit: u16 = 0x0002;
pub const ServerMoreResultsExists: u16 = 0x0008;
pub const ServerStatusNoGoodIndexUsed: u16 = 0x0010;
pub const ServerStatusNoIndexUsed: u16 = 0x0020;
pub const ServerStatusCursorExists: u16 = 0x0040;
pub const ServerStatusLastRowSend: u16 = 0x0080;
pub const ServerStatusDBDropped: u16 = 0x0100;
pub const ServerStatusNoBackslashEscaped: u16 = 0x0200;
pub const ServerStatusMetadataChanged: u16 = 0x0400;
pub const ServerStatusWasSlow: u16 = 0x0800;
pub const ServerPSOutParams: u16 = 0x1000;
/// 状态字是否含 Cursor Exists。
pub fn HasCursorExistsFlag(status: u16) -> bool {
    status & ServerStatusCursorExists > 0
}

// 标识符和 packet 长度限制。
/// 单包最大载荷（24 位长度上限）。
pub const MaxPayloadLen: usize = (1 << 24) - 1;
pub const MaxTableNameLength: usize = 64;
pub const MaxDatabaseNameLength: usize = 64;
pub const MaxColumnNameLength: usize = 64;
pub const MaxKeyParts: usize = 16;
pub const MaxIndexIdentifierLen: usize = 64;
pub const MaxForeignKeyIdentifierLen: usize = 64;
pub const MaxConstraintIdentifierLen: usize = 64;
pub const MaxViewIdentifierLen: usize = 64;
pub const MaxAliasIdentifierLen: usize = 256;
pub const MaxUserDefinedVariableLen: usize = 64;
pub const ErrTextLength: usize = 80;

// Command 字节值从 COM_SLEEP=0 连续递增到 COM_END=32。
/// COM_SLEEP 命令字节。
pub const ComSleep: u8 = 0;
pub const ComQuit: u8 = 1;
pub const ComInitDB: u8 = 2;
pub const ComQuery: u8 = 3;
pub const ComFieldList: u8 = 4;
pub const ComCreateDB: u8 = 5;
pub const ComDropDB: u8 = 6;
pub const ComRefresh: u8 = 7;
pub const ComShutdown: u8 = 8;
pub const ComStatistics: u8 = 9;
pub const ComProcessInfo: u8 = 10;
pub const ComConnect: u8 = 11;
pub const ComProcessKill: u8 = 12;
pub const ComDebug: u8 = 13;
pub const ComPing: u8 = 14;
pub const ComTime: u8 = 15;
pub const ComDelayedInsert: u8 = 16;
pub const ComChangeUser: u8 = 17;
pub const ComBinlogDump: u8 = 18;
pub const ComTableDump: u8 = 19;
pub const ComConnectOut: u8 = 20;
pub const ComRegisterSlave: u8 = 21;
pub const ComStmtPrepare: u8 = 22;
pub const ComStmtExecute: u8 = 23;
pub const ComStmtSendLongData: u8 = 24;
pub const ComStmtClose: u8 = 25;
pub const ComStmtReset: u8 = 26;
pub const ComSetOption: u8 = 27;
pub const ComStmtFetch: u8 = 28;
pub const ComDaemon: u8 = 29;
pub const ComBinlogDumpGtid: u8 = 30;
pub const ComResetConnection: u8 = 31;
pub const ComEnd: u8 = 32;

// 客户端 capability 从 bit 0 到 bit 26；Go 注释中的 27..31 未声明位继续保持未定义。
/// 客户端 capability：长密码。
pub const ClientLongPassword: u32 = 1 << 0;
pub const ClientFoundRows: u32 = 1 << 1;
pub const ClientLongFlag: u32 = 1 << 2;
pub const ClientConnectWithDB: u32 = 1 << 3;
pub const ClientNoSchema: u32 = 1 << 4;
pub const ClientCompress: u32 = 1 << 5;
pub const ClientODBC: u32 = 1 << 6;
pub const ClientLocalFiles: u32 = 1 << 7;
pub const ClientIgnoreSpace: u32 = 1 << 8;
pub const ClientProtocol41: u32 = 1 << 9;
pub const ClientInteractive: u32 = 1 << 10;
pub const ClientSSL: u32 = 1 << 11;
pub const ClientIgnoreSigpipe: u32 = 1 << 12;
pub const ClientTransactions: u32 = 1 << 13;
pub const ClientReserved: u32 = 1 << 14;
pub const ClientSecureConnection: u32 = 1 << 15;
pub const ClientMultiStatements: u32 = 1 << 16;
pub const ClientMultiResults: u32 = 1 << 17;
pub const ClientPSMultiResults: u32 = 1 << 18;
pub const ClientPluginAuth: u32 = 1 << 19;
pub const ClientConnectAtts: u32 = 1 << 20;
pub const ClientPluginAuthLenencClientData: u32 = 1 << 21;
pub const ClientHandleExpiredPasswords: u32 = 1 << 22;
pub const ClientSessionTrack: u32 = 1 << 23;
pub const ClientDeprecateEOF: u32 = 1 << 24;
pub const ClientOptionalResultsetMetadata: u32 = 1 << 25;
pub const ClientZstdCompressionAlgorithm: u32 = 1 << 26;

/// 类型缓存禁用标记。
pub const TypeNoCache: u8 = 0xff;
// 认证插件名称只作协议字符串，不在这里中加载插件或执行认证。
/// mysql_native_password 插件名。
pub const AuthNativePassword: &str = "mysql_native_password";
pub const AuthCachingSha2Password: &str = "caching_sha2_password";
pub const AuthTiDBSM3Password: &str = "tidb_sm3_password";
pub const AuthMySQLClearPassword: &str = "mysql_clear_password";
pub const AuthSocket: &str = "auth_socket";
pub const AuthTiDBSessionToken: &str = "tidb_session_token";
pub const AuthTiDBAuthToken: &str = "tidb_auth_token";
pub const AuthLDAPSimple: &str = "authentication_ldap_simple";
pub const AuthLDAPSASL: &str = "authentication_ldap_sasl";

// MySQL 系统 schema/table 名称。
/// 系统库名 `mysql`。
pub const SystemDB: &str = "mysql";
pub const SysDB: &str = "sys";
pub const GlobalPrivTable: &str = "global_priv";
pub const UserTable: &str = "User";
pub const DBTable: &str = "DB";
pub const TablePrivTable: &str = "Tables_priv";
pub const ColumnPrivTable: &str = "Columns_priv";
pub const GlobalVariablesTable: &str = "GLOBAL_VARIABLES";
pub const GlobalStatusTable: &str = "GLOBAL_STATUS";
pub const TiDBTable: &str = "tidb";
pub const RoleEdgeTable: &str = "role_edges";
pub const DefaultRoleTable: &str = "default_roles";
pub const PasswordHistoryTable: &str = "password_history";
pub const WorkloadSchema: &str = "workload_schema";

// MySQL 类型显示宽度、物理长度和 hash 长度上限。
/// 非固定小数位数标记值。
pub const NotFixedDec: usize = 31;
pub const MaxIntWidth: usize = 20;
pub const MaxRealWidth: usize = 23;
pub const MaxFloatingTypeScale: usize = 30;
pub const MaxFloatingTypeWidth: usize = 255;
pub const MaxDecimalScale: usize = 30;
pub const MaxDecimalWidth: usize = 65;
pub const MaxDateWidth: usize = 10;
pub const MaxDatetimeWidthNoFsp: usize = 19;
pub const MaxDatetimeWidthWithFsp: usize = 26;
pub const MaxDatetimeFullWidth: usize = 29;
pub const MaxDurationWidthNoFsp: usize = 10;
pub const MaxDurationWidthWithFsp: usize = 17;
pub const MaxBlobWidth: u64 = 16_777_216;
pub const MaxLongBlobWidth: u64 = 4_294_967_295;
pub const MaxBitDisplayWidth: usize = 64;
pub const MaxFloatPrecisionLength: usize = 24;
pub const MaxDoublePrecisionLength: usize = 53;
pub const MaxFieldCharLength: usize = 255;
pub const MaxFieldVarCharLength: usize = 65_535;
pub const MaxTypeSetMembers: usize = 64;
pub const PWDHashLen: usize = 40;
pub const SHAPWDHashLen: usize = 70;
pub const SM3PWDHashLen: usize = 70;

// Command2Str 用于 processlist/诊断展示，不发送命令。
/// COM_* 到可读名称（诊断用）。
pub static Command2Str: &[(u8, &str)] = &[
    (ComSleep, "Sleep"),
    (ComQuit, "Quit"),
    (ComInitDB, "Init DB"),
    (ComQuery, "Query"),
    (ComFieldList, "Field List"),
    (ComCreateDB, "Create DB"),
    (ComDropDB, "Drop DB"),
    (ComRefresh, "Refresh"),
    (ComShutdown, "Shutdown"),
    (ComStatistics, "Statistics"),
    (ComProcessInfo, "Processlist"),
    (ComConnect, "Connect"),
    (ComProcessKill, "Kill"),
    (ComDebug, "Debug"),
    (ComPing, "Ping"),
    (ComTime, "Time"),
    (ComDelayedInsert, "Delayed Insert"),
    (ComChangeUser, "Change User"),
    (ComBinlogDump, "Binlog Dump"),
    (ComTableDump, "Table Dump"),
    (ComConnectOut, "Connect out"),
    (ComRegisterSlave, "Register Slave"),
    (ComStmtPrepare, "Prepare"),
    (ComStmtExecute, "Execute"),
    (ComStmtSendLongData, "Long Data"),
    (ComStmtClose, "Close stmt"),
    (ComStmtReset, "Reset stmt"),
    (ComSetOption, "Set option"),
    (ComStmtFetch, "Fetch"),
    (ComDaemon, "Daemon"),
    (ComBinlogDumpGtid, "Binlog Dump"),
    (ComResetConnection, "Reset connect"),
];

/// 默认 sql_mode 字符串。
pub const DefaultSQLMode: &str = "ONLY_FULL_GROUP_BY,STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_AUTO_CREATE_USER,NO_ENGINE_SUBSTITUTION";
// 默认物理长度表保留 Go 中支持的固定长度类型。
/// 各 MySQL 类型默认物理长度。
pub static DefaultLengthOfMysqlTypes: &[(u8, usize)] = &[
    (TypeYear, 1),
    (TypeDate, 3),
    (TypeDuration, 3),
    (TypeDatetime, 8),
    (TypeTimestamp, 4),
    (TypeTiny, 1),
    (TypeShort, 2),
    (TypeInt24, 3),
    (TypeLong, 4),
    (TypeLonglong, 8),
    (TypeFloat, 4),
    (TypeDouble, 8),
    (TypeEnum, 2),
    (TypeString, 1),
    (TypeSet, 8),
];

/// 时间小数精度到存储长度。
pub static DefaultLengthOfTimeFraction: &[(usize, usize)] =
    &[(0, 0), (1, 1), (2, 1), (3, 2), (4, 2), (5, 3), (6, 3)];

// DefaultAuthPlugins 的顺序决定默认支持列表的展示顺序。
/// 默认支持的认证插件列表（有序）。
pub static DefaultAuthPlugins: &[&str] = &[
    AuthNativePassword,
    AuthCachingSha2Password,
    AuthTiDBSM3Password,
    AuthLDAPSASL,
    AuthLDAPSimple,
    AuthSocket,
    AuthTiDBSessionToken,
    AuthTiDBAuthToken,
    AuthMySQLClearPassword,
];

// SQLMode 是 MySQL sql_mode 的 64 位位集合。
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
/// sql_mode 位集合（64 位）。
pub struct SQLMode(pub i64);
impl SQLMode {
    /// 是否包含指定 mode 位。
    fn has(self, flag: SQLMode) -> bool {
        self.0 & flag.0 == flag.0
    }
    /// 是否启用 NO_ZERO_DATE。
    pub fn HasNoZeroDateMode(self) -> bool {
        self.has(ModeNoZeroDate)
    }
    /// 是否启用 NO_ZERO_IN_DATE。
    pub fn HasNoZeroInDateMode(self) -> bool {
        self.has(ModeNoZeroInDate)
    }
    /// 是否启用 ERROR_FOR_DIVISION_BY_ZERO。
    pub fn HasErrorForDivisionByZeroMode(self) -> bool {
        self.has(ModeErrorForDivisionByZero)
    }
    /// 是否启用 ONLY_FULL_GROUP_BY。
    pub fn HasOnlyFullGroupBy(self) -> bool {
        self.has(ModeOnlyFullGroupBy)
    }
    /// 是否处于严格模式（TRANS 或 ALL TABLES）。
    pub fn HasStrictMode(self) -> bool {
        self.has(ModeStrictTransTables) || self.has(ModeStrictAllTables)
    }
    /// 是否将 `||` 视为字符串拼接。
    pub fn HasPipesAsConcatMode(self) -> bool {
        self.has(ModePipesAsConcat)
    }
    /// 是否启用 NO_UNSIGNED_SUBTRACTION。
    pub fn HasNoUnsignedSubtractionMode(self) -> bool {
        self.has(ModeNoUnsignedSubtraction)
    }
    /// 是否提高 NOT 运算符优先级。
    pub fn HasHighNotPrecedenceMode(self) -> bool {
        self.has(ModeHighNotPrecedence)
    }
    /// 是否启用 ANSI_QUOTES（双引号作标识符）。
    pub fn HasANSIQuotesMode(self) -> bool {
        self.has(ModeANSIQuotes)
    }
    /// 是否将 REAL 视为 FLOAT。
    pub fn HasRealAsFloatMode(self) -> bool {
        self.has(ModeRealAsFloat)
    }
    /// 是否将 CHAR 填充到声明长度。
    pub fn HasPadCharToFullLengthMode(self) -> bool {
        self.has(ModePadCharToFullLength)
    }
    /// 是否禁用反斜杠转义。
    pub fn HasNoBackslashEscapesMode(self) -> bool {
        self.has(ModeNoBackslashEscapes)
    }
    /// 是否忽略函数名与 `(` 间空白。
    pub fn HasIgnoreSpaceMode(self) -> bool {
        self.has(ModeIgnoreSpace)
    }
    /// 是否禁止 GRANT 隐式建用户。
    pub fn HasNoAutoCreateUserMode(self) -> bool {
        self.has(ModeNoAutoCreateUser)
    }
    /// 是否允许非法日期值。
    pub fn HasAllowInvalidDatesMode(self) -> bool {
        self.has(ModeAllowInvalidDates)
    }
}
/// 清除指定 mode 位。
pub fn DelSQLMode(ori: SQLMode, del: SQLMode) -> SQLMode {
    SQLMode(ori.0 & !del.0)
}
/// 置位指定 mode 位。
pub fn SetSQLMode(ori: SQLMode, add: SQLMode) -> SQLMode {
    SQLMode(ori.0 | add.0)
}

// sql_mode 位值按 Go iota 的 0..32 顺序展开。
/// REAL_AS_FLOAT 位。
pub const ModeRealAsFloat: SQLMode = SQLMode(1 << 0);
pub const ModePipesAsConcat: SQLMode = SQLMode(1 << 1);
pub const ModeANSIQuotes: SQLMode = SQLMode(1 << 2);
pub const ModeIgnoreSpace: SQLMode = SQLMode(1 << 3);
pub const ModeNotUsed: SQLMode = SQLMode(1 << 4);
pub const ModeOnlyFullGroupBy: SQLMode = SQLMode(1 << 5);
pub const ModeNoUnsignedSubtraction: SQLMode = SQLMode(1 << 6);
pub const ModeNoDirInCreate: SQLMode = SQLMode(1 << 7);
pub const ModePostgreSQL: SQLMode = SQLMode(1 << 8);
pub const ModeOracle: SQLMode = SQLMode(1 << 9);
pub const ModeMsSQL: SQLMode = SQLMode(1 << 10);
pub const ModeDb2: SQLMode = SQLMode(1 << 11);
pub const ModeMaxdb: SQLMode = SQLMode(1 << 12);
pub const ModeNoKeyOptions: SQLMode = SQLMode(1 << 13);
pub const ModeNoTableOptions: SQLMode = SQLMode(1 << 14);
pub const ModeNoFieldOptions: SQLMode = SQLMode(1 << 15);
pub const ModeMySQL323: SQLMode = SQLMode(1 << 16);
pub const ModeMySQL40: SQLMode = SQLMode(1 << 17);
pub const ModeANSI: SQLMode = SQLMode(1 << 18);
pub const ModeNoAutoValueOnZero: SQLMode = SQLMode(1 << 19);
pub const ModeNoBackslashEscapes: SQLMode = SQLMode(1 << 20);
pub const ModeStrictTransTables: SQLMode = SQLMode(1 << 21);
pub const ModeStrictAllTables: SQLMode = SQLMode(1 << 22);
pub const ModeNoZeroInDate: SQLMode = SQLMode(1 << 23);
pub const ModeNoZeroDate: SQLMode = SQLMode(1 << 24);
pub const ModeInvalidDates: SQLMode = SQLMode(1 << 25);
pub const ModeErrorForDivisionByZero: SQLMode = SQLMode(1 << 26);
pub const ModeTraditional: SQLMode = SQLMode(1 << 27);
pub const ModeNoAutoCreateUser: SQLMode = SQLMode(1 << 28);
pub const ModeHighNotPrecedence: SQLMode = SQLMode(1 << 29);
pub const ModeNoEngineSubstitution: SQLMode = SQLMode(1 << 30);
pub const ModePadCharToFullLength: SQLMode = SQLMode(1 << 31);
pub const ModeAllowInvalidDates: SQLMode = SQLMode(1 << 32);
pub const ModeNone: SQLMode = SQLMode(0);

// Str2SQLMode 保留所有单项模式名称。
/// 模式名到位值映射。
pub static Str2SQLMode: &[(&str, SQLMode)] = &[
    ("REAL_AS_FLOAT", ModeRealAsFloat),
    ("PIPES_AS_CONCAT", ModePipesAsConcat),
    ("ANSI_QUOTES", ModeANSIQuotes),
    ("IGNORE_SPACE", ModeIgnoreSpace),
    ("NOT_USED", ModeNotUsed),
    ("ONLY_FULL_GROUP_BY", ModeOnlyFullGroupBy),
    ("NO_UNSIGNED_SUBTRACTION", ModeNoUnsignedSubtraction),
    ("NO_DIR_IN_CREATE", ModeNoDirInCreate),
    ("POSTGRESQL", ModePostgreSQL),
    ("ORACLE", ModeOracle),
    ("MSSQL", ModeMsSQL),
    ("DB2", ModeDb2),
    ("MAXDB", ModeMaxdb),
    ("NO_KEY_OPTIONS", ModeNoKeyOptions),
    ("NO_TABLE_OPTIONS", ModeNoTableOptions),
    ("NO_FIELD_OPTIONS", ModeNoFieldOptions),
    ("MYSQL323", ModeMySQL323),
    ("MYSQL40", ModeMySQL40),
    ("ANSI", ModeANSI),
    ("NO_AUTO_VALUE_ON_ZERO", ModeNoAutoValueOnZero),
    ("NO_BACKSLASH_ESCAPES", ModeNoBackslashEscapes),
    ("STRICT_TRANS_TABLES", ModeStrictTransTables),
    ("STRICT_ALL_TABLES", ModeStrictAllTables),
    ("NO_ZERO_IN_DATE", ModeNoZeroInDate),
    ("NO_ZERO_DATE", ModeNoZeroDate),
    ("INVALID_DATES", ModeInvalidDates),
    ("ERROR_FOR_DIVISION_BY_ZERO", ModeErrorForDivisionByZero),
    ("TRADITIONAL", ModeTraditional),
    ("NO_AUTO_CREATE_USER", ModeNoAutoCreateUser),
    ("HIGH_NOT_PRECEDENCE", ModeHighNotPrecedence),
    ("NO_ENGINE_SUBSTITUTION", ModeNoEngineSubstitution),
    ("PAD_CHAR_TO_FULL_LENGTH", ModePadCharToFullLength),
    ("ALLOW_INVALID_DATES", ModeAllowInvalidDates),
];

// CombinationSQLMode 是 MySQL 组合模式的展开表。
/// 组合模式（如 ANSI）展开为子模式列表。
pub static CombinationSQLMode: &[(&str, &[&str])] = &[
    (
        "ANSI",
        &[
            "REAL_AS_FLOAT",
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "ONLY_FULL_GROUP_BY",
        ],
    ),
    (
        "DB2",
        &[
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "NO_KEY_OPTIONS",
            "NO_TABLE_OPTIONS",
            "NO_FIELD_OPTIONS",
        ],
    ),
    (
        "MAXDB",
        &[
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "NO_KEY_OPTIONS",
            "NO_TABLE_OPTIONS",
            "NO_FIELD_OPTIONS",
            "NO_AUTO_CREATE_USER",
        ],
    ),
    (
        "MSSQL",
        &[
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "NO_KEY_OPTIONS",
            "NO_TABLE_OPTIONS",
            "NO_FIELD_OPTIONS",
        ],
    ),
    ("MYSQL323", &["MYSQL323", "HIGH_NOT_PRECEDENCE"]),
    ("MYSQL40", &["MYSQL40", "HIGH_NOT_PRECEDENCE"]),
    (
        "ORACLE",
        &[
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "NO_KEY_OPTIONS",
            "NO_TABLE_OPTIONS",
            "NO_FIELD_OPTIONS",
            "NO_AUTO_CREATE_USER",
        ],
    ),
    (
        "POSTGRESQL",
        &[
            "PIPES_AS_CONCAT",
            "ANSI_QUOTES",
            "IGNORE_SPACE",
            "NO_KEY_OPTIONS",
            "NO_TABLE_OPTIONS",
            "NO_FIELD_OPTIONS",
        ],
    ),
    (
        "TRADITIONAL",
        &[
            "STRICT_TRANS_TABLES",
            "STRICT_ALL_TABLES",
            "NO_ZERO_IN_DATE",
            "NO_ZERO_DATE",
            "ERROR_FOR_DIVISION_BY_ZERO",
            "NO_AUTO_CREATE_USER",
            "NO_ENGINE_SUBSTITUTION",
        ],
    ),
];

// FormatSQLModeStr 大写并去尾空格，按首次出现顺序展开组合模式并去重。
/// 规范化并展开组合 sql_mode 字符串。
pub fn FormatSQLModeStr(input: &str) -> String {
    let normalized = input.trim_end_matches(' ').to_ascii_uppercase();
    let mut result = Vec::new();
    let mut seen = HashSet::new();
    for part in normalized.split(',').filter(|part| !part.is_empty()) {
        if let Some((_, modes)) = CombinationSQLMode.iter().find(|(name, _)| *name == part) {
            for mode in *modes {
                if seen.insert(*mode) {
                    result.push(*mode);
                }
            }
        }
        if seen.insert(part) {
            result.push(part);
        }
    }
    result.join(",")
}

// GetSQLMode 把已经 Format 的逗号列表折叠为位集合；未知非空项立即返回 MySQL 错误。
/// 解析 sql_mode 字符串为位集合。
pub fn GetSQLMode(input: &str) -> Result<SQLMode, SQLError> {
    let mut mode = ModeNone;
    for name in input.split(',') {
        if let Some((_, value)) = Str2SQLMode.iter().find(|(key, _)| *key == name) {
            mode = SetSQLMode(mode, *value);
        } else if !name.is_empty() {
            return Err(newInvalidModeErr(name));
        }
    }
    Ok(mode)
}

// PriorityEnum 与 Go 的 int 别名一致；独立常量保留 mysql.NoPriority 等包级符号。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// INSERT/语句优先级枚举。
pub struct PriorityEnum(pub i32);
/// 无优先级。
pub const NoPriority: PriorityEnum = PriorityEnum(0);
/// LOW_PRIORITY。
pub const LowPriority: PriorityEnum = PriorityEnum(1);
/// HIGH_PRIORITY。
pub const HighPriority: PriorityEnum = PriorityEnum(2);
/// DELAYED。
pub const DelayedPriority: PriorityEnum = PriorityEnum(3);
/// 优先级到关键字字符串。
pub static Priority2Str: &[(PriorityEnum, &str)] = &[
    (NoPriority, "NO_PRIORITY"),
    (LowPriority, "LOW_PRIORITY"),
    (HighPriority, "HIGH_PRIORITY"),
    (DelayedPriority, "DELAYED"),
];
/// 关键字字符串到优先级。
pub fn Str2Priority(value: &str) -> PriorityEnum {
    match value.to_ascii_uppercase().as_str() {
        "HIGH_PRIORITY" => HighPriority,
        "LOW_PRIORITY" => LowPriority,
        "DELAYED" => DelayedPriority,
        _ => NoPriority,
    }
}
/// `PriorityEnum` 与字符串互转辅助。
impl PriorityEnum {
    // Restore 返回原本应写入 restore writer 的关键字，不引入完整 format::RestoreCtx。
    pub fn Restore(&self) -> Result<&'static str, SQLError> {
        match *self {
            NoPriority => Ok(""),
            LowPriority => Ok("LOW_PRIORITY"),
            HighPriority => Ok("HIGH_PRIORITY"),
            DelayedPriority => Ok("DELAYED"),
            _ => Err(NewErrf(
                ErrWrongValueForVar,
                "%s",
                &[],
                vec![format!("undefined PriorityEnum Type[{}]", self.0).into()],
            )),
        }
    }
}

/// 主键约束默认名。
pub const PrimaryKeyName: &str = "PRIMARY";
/// 默认 DECIMAL 字符串表示（与 Go 一致）。
pub const DefaultDecimal: &str =
    "99999999999999999999999999999999999999999999999999999999999999999";
/// 分区数量上限。
pub const PartitionCountLimit: usize = 8192;
// MySQL enum_cursor_type 位。
/// 只读游标类型位。
pub const CursorTypeReadOnly: u32 = 1 << 0;
pub const CursorTypeForUpdate: u32 = 1 << 1;
pub const CursorTypeScrollable: u32 = 1 << 2;
/// zlib 默认压缩级别。
pub const ZlibCompressDefaultLevel: i32 = 6;
/// 无压缩。
pub const CompressionNone: i32 = 0;
/// zlib 压缩。
pub const CompressionZlib: i32 = 1;
/// zstd 压缩。
pub const CompressionZstd: i32 = 2;
