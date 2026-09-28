// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc.
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

// Lightning / Import 统一错误类型与归一化。
//
// 定义带 RFC 风格 ID 的 `CommonError`、日志脱敏开关、以及将 BR/内部错误
// 映射为 Lightning/Import 错误码的 `NormalizeError` / `NormalizeOrWrapErr`。
// 便于上层按错误 ID 做重试、分类与用户可读提示。

use std::fmt;
use std::sync::LazyLock;
use std::sync::atomic::{AtomicU8, Ordering};

/// Matches `errors.RedactLogDisable` / `Enable` / `Marker` in pingcap/errors.
/// 日志脱敏：关闭 / 用 `?` 替换 / 用标记符包裹敏感片段。
pub const RedactLogDisable: u8 = 0;
pub const RedactLogEnable: u8 = 1;
pub const RedactLogMarker: u8 = 2;

/// 全局日志脱敏模式，影响如 `ErrCastValue` 中用户输入值的展示。
pub static REDACT_LOG_ENABLED: AtomicU8 = AtomicU8::new(RedactLogDisable);

/// Lightning 公共错误：可选 RFC ID、消息、种类、嵌套原因与简易栈信息。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommonError {
    pub ID: String,
    pub Message: String,
    pub Kind: String,
    pub Code: Option<u16>,
    pub StatusCode: Option<u16>,
    pub RpcCode: Option<String>,
    pub Causes: Vec<CommonError>,
    pub Stack: Vec<String>,
}

impl CommonError {
    /// 构造无 RFC ID 的普通错误。
    pub fn new(kind: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ID: String::new(),
            Message: message.into(),
            Kind: kind.into(),
            Code: None,
            StatusCode: None,
            RpcCode: None,
            Causes: vec![],
            Stack: vec![],
        }
    }

    /// 构造带 RFC 风格错误 ID 的错误（Kind 固定为 `"rfc"`）。
    pub fn rfc(id: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            ID: id.into(),
            Message: message.into(),
            Kind: "rfc".to_owned(),
            Code: None,
            StatusCode: None,
            RpcCode: None,
            Causes: vec![],
            Stack: vec![],
        }
    }

    /// 设置错误 ID 后返回自身（链式调用）。
    pub fn with_id(mut self, id: impl Into<String>) -> Self {
        self.ID = id.into();
        self
    }

    /// 将 `cause` 追加为嵌套原因。
    pub fn wrap(mut self, cause: CommonError) -> Self {
        self.Causes.push(cause);
        self
    }

    /// 在外层包裹注解消息，对应 Go `errors.Annotate`。
    pub fn annotate(self, message: impl Into<String>) -> Self {
        let message = message.into();
        let cause_display = self.to_string();
        // Match pingcap/errors.Annotate: the wrapper itself is not an *errors.Error;
        // Find walks into Causes to locate the RFC error.
        // 外层 annotated 本身无 ID；归一化时沿 Causes 查找真正的 RFC 错误。
        let mut annotated = CommonError::new("annotated", format!("{message}: {cause_display}"));
        annotated.Stack = if self.Stack.is_empty() {
            vec!["annotate".to_owned()]
        } else {
            self.Stack.clone()
        };
        annotated.Causes = vec![self];
        annotated
    }

    /// 覆盖消息并在栈为空时补一条占位栈帧。
    pub fn gen_with_stack(self, message: impl Into<String>) -> Self {
        let mut error = self;
        error.Message = message.into();
        if error.Stack.is_empty() {
            error.Stack.push("gen_with_stack".to_owned());
        }
        error
    }

    /// 按 `%s` 占位符填入参数；`ErrCastValue` 第三个参数（用户值）会按脱敏模式处理。
    pub fn gen_with_stack_by_args(self, args: &[&str]) -> Self {
        let mut message = self.Message.clone();
        for (index, arg) in args.iter().enumerate() {
            let rendered = if self.ID == "Import:ErrCastValue" && index == 2 {
                redact_arg(arg)
            } else {
                (*arg).to_owned()
            };
            if let Some((offset, placeholder)) = ["%s", "%d", "%x"]
                .into_iter()
                .filter_map(|placeholder| {
                    message
                        .find(placeholder)
                        .map(|offset| (offset, placeholder))
                })
                .min_by_key(|(offset, _)| *offset)
            {
                message.replace_range(offset..offset + placeholder.len(), &rendered);
            }
        }
        let mut error = self;
        error.Message = message;
        if error.Stack.is_empty() {
            error.Stack.push("gen_with_stack_by_args".to_owned());
        }
        error
    }

    /// 返回简易栈跟踪片段。
    pub fn stack_trace(&self) -> &[String] {
        &self.Stack
    }
}

/// 按全局脱敏模式渲染单个参数。
fn redact_arg(arg: &str) -> String {
    match REDACT_LOG_ENABLED.load(Ordering::SeqCst) {
        RedactLogEnable => "?".to_owned(),
        RedactLogMarker => format!("‹{arg}›"),
        _ => arg.to_owned(),
    }
}

impl fmt::Display for CommonError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.ID.is_empty() {
            f.write_str(&self.Message)
        } else {
            write!(f, "[{}]{}", self.ID, self.Message)
        }
    }
}

impl std::error::Error for CommonError {}

/// 带显式栈信息的错误包装，对应 Go `errors.WithStack` 一类形态。
pub struct withStack {
    pub error: CommonError,
    pub stack: Vec<String>,
}

impl withStack {
    /// 返回被包装的错误。
    pub fn Cause(&self) -> &CommonError {
        &self.error
    }

    /// 同 `Cause`，便于与 Go `Unwrap` 语义对照。
    pub fn Unwrap(&self) -> &CommonError {
        &self.error
    }

    /// 格式化：verbose 时追加栈行。
    pub fn Format(&self, verbose: bool) -> String {
        if verbose {
            format!("{}\n{}", self.error, self.stack.join("\n"))
        } else {
            self.error.to_string()
        }
    }
}

/// 判断 `error`（含 Causes 链）是否匹配期望的 RFC 错误（按 ID）。
pub fn Is(error: &CommonError, expect: &CommonError) -> bool {
    if !expect.ID.is_empty() && error.ID == expect.ID {
        return true;
    }
    error.Causes.iter().any(|cause| Is(cause, expect))
}

/// 在错误树中查找带非空 ID 的 RFC 错误。
fn find_rfc_error(error: &CommonError) -> Option<&CommonError> {
    if !error.ID.is_empty() {
        return Some(error);
    }
    error.Causes.iter().find_map(find_rfc_error)
}

/// 将 BR 侧错误 ID 映射为 Lightning 对应 ID；未知则归为 `ErrUnknown`。
fn map_br_error_id(id: &str) -> &'static str {
    match id {
        "BR:KV:ErrStorageUnknown" | "BR:Storage:ErrStorageUnknown" => {
            "Lightning:Storage:ErrStorageUnknown"
        }
        "BR:KV:ErrStorageInvalidConfig" | "BR:Storage:ErrStorageInvalidConfig" => {
            "Lightning:Storage:ErrInvalidStorageConfig"
        }
        "BR:KV:ErrStorageInvalidPermission" | "BR:Storage:ErrStorageInvalidPermission" => {
            "Lightning:Storage:ErrInvalidPermission"
        }
        "BR:PD:ErrPDUpdateFailed" => "Lightning:PD:ErrUpdatePD",
        "BR:Common:ErrVersionMismatch" => "Lightning:Common:ErrVersionMismatch",
        _ => "Lightning:Common:ErrUnknown",
    }
}

/// Converts an arbitrary error to a Lightning/Import RFC error.
/// 将任意错误归一为 Lightning/Import RFC 错误；已是 Lightning/Import 则保留，
/// BR 错误做 ID 映射，否则包装为 `ErrUnknown`。
pub fn NormalizeError(error: Option<CommonError>) -> Option<CommonError> {
    let error = error?;
    if crate::IsContextCanceledError(Some(&error)) {
        return Some(error);
    }

    let original_stack = error.Stack.clone();
    let maybe_add_stack = |mut normalized: CommonError| {
        if !original_stack.is_empty() && normalized.Stack.is_empty() {
            normalized.Stack = original_stack.clone();
        }
        normalized
    };

    if let Some(found) = find_rfc_error(&error) {
        let mut normalized_err = found.clone();
        let err_msg = error.to_string();
        let n_err_msg = normalized_err.to_string();
        // Workaround for https://github.com/pingcap/tidb/issues/32133.
        // annotate 产生 “前缀: RFC消息” 时，把前缀收成 Message，保留内层 cause。
        if err_msg != n_err_msg && err_msg.ends_with(&format!(": {n_err_msg}")) {
            let prefix_len = err_msg.len() - n_err_msg.len() - 2;
            let prefix = err_msg[..prefix_len].to_owned();
            let cause = normalized_err.Causes.first().cloned();
            normalized_err.Message = prefix;
            normalized_err.Causes.clear();
            if let Some(cause) = cause {
                normalized_err.Causes.push(cause);
            }
        }

        if normalized_err.ID.starts_with("Lightning:") || normalized_err.ID.starts_with("Import:") {
            let mut kept = normalized_err;
            if kept.Stack.is_empty() {
                kept.Stack = original_stack;
            }
            return Some(kept);
        }

        let err_id = map_br_error_id(&normalized_err.ID);
        let cause = normalized_err.Causes.first().cloned();
        let mut mapped = CommonError::rfc(err_id, normalized_err.Message);
        if let Some(cause) = cause {
            mapped.Causes.push(cause);
        }
        return Some(maybe_add_stack(mapped));
    }

    Some(maybe_add_stack(ErrUnknown.clone().wrap(error)))
}

/// 归一化失败（落成 Unknown）时用 `rfc_error` 包装原错误，否则返回已归一化结果。
pub fn NormalizeOrWrapErr(
    rfc_error: &CommonError,
    error: Option<CommonError>,
    args: &[&str],
) -> Option<CommonError> {
    let error = error?;
    if crate::IsContextCanceledError(Some(&error)) {
        return Some(error);
    }
    let normalized = NormalizeError(Some(error.clone()))?;
    if Is(&normalized, &ErrUnknown) {
        Some(rfc_error.clone().wrap(error).gen_with_stack_by_args(args))
    } else {
        Some(normalized)
    }
}

/// 构造“发现重复键”的 RFC 错误，消息中带键值十六进制调试形式。
pub fn ErrFoundDuplicateKeys(key: &[u8], value: &[u8]) -> CommonError {
    let key = key
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    let value = value
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    CommonError::rfc(
        "Lightning:Restore:ErrFoundDuplicateKey",
        format!("found duplicate key '{key}', value '{value}'"),
    )
}

/// 定义懒加载的静态 RFC 错误常量。
macro_rules! define_error {
    ($name:ident, $id:expr, $msg:expr) => {
        pub static $name: LazyLock<CommonError> = LazyLock::new(|| CommonError::rfc($id, $msg));
    };
}

define_error!(ErrUnknown, "Lightning:Common:ErrUnknown", "unknown error");
define_error!(
    ErrReadConfigFile,
    "Lightning:Config:ErrReadConfigFile",
    "cannot read config file '%s'"
);
define_error!(
    ErrParseConfigFile,
    "Lightning:Config:ErrParseConfigFile",
    "cannot parse config file '%s'"
);
define_error!(
    ErrInvalidArgument,
    "Lightning:Common:ErrInvalidArgument",
    "invalid argument"
);
define_error!(
    ErrVersionMismatch,
    "Lightning:Common:ErrVersionMismatch",
    "version mismatch"
);
define_error!(
    ErrInvalidConfig,
    "Lightning:Config:ErrInvalidConfig",
    "invalid config"
);
define_error!(
    ErrInvalidTLSConfig,
    "Lightning:Config:ErrInvalidTLSConfig",
    "invalid tls config"
);
define_error!(
    ErrInvalidSortedKVDir,
    "Lightning:Config:ErrInvalidSortedKVDir",
    "invalid sorted-kv-dir '%s' for local backend, please change the config or delete the path"
);
define_error!(
    ErrStorageUnknown,
    "Lightning:Storage:ErrStorageUnknown",
    "unknown storage error"
);
define_error!(
    ErrInvalidPermission,
    "Lightning:Storage:ErrInvalidPermission",
    "invalid permission"
);
define_error!(
    ErrInvalidStorageConfig,
    "Lightning:Storage:ErrInvalidStorageConfig",
    "invalid data-source-dir"
);
define_error!(
    ErrEmptySourceDir,
    "Lightning:Storage:ErrEmptySourceDir",
    "data-source-dir '%s' doesn't exist or contains no files"
);
define_error!(
    ErrTableRoute,
    "Lightning:Loader:ErrTableRoute",
    "table route error"
);
define_error!(
    ErrInvalidSchemaFile,
    "Lightning:Loader:ErrInvalidSchemaFile",
    "invalid schema file"
);
define_error!(
    ErrTooManySourceFiles,
    "Lightning:Loader:ErrTooManySourceFiles",
    "too many source files"
);
define_error!(
    ErrSystemRequirementNotMet,
    "Lightning:PreCheck:ErrSystemRequirementNotMet",
    "system requirement not met"
);
define_error!(
    ErrCheckpointSchemaConflict,
    "Lightning:PreCheck:ErrCheckpointSchemaConflict",
    "checkpoint schema conflict"
);
define_error!(
    ErrPreCheckFailed,
    "Lightning:PreCheck:ErrPreCheckFailed",
    "tidb-lightning pre-check failed: %s"
);
define_error!(
    ErrCheckClusterRegion,
    "Lightning:PreCheck:ErrCheckClusterRegion",
    "check tikv cluster region error"
);
define_error!(
    ErrCheckLocalResource,
    "Lightning:PreCheck:ErrCheckLocalResource",
    "check local storage resource error"
);
define_error!(
    ErrCheckTableEmpty,
    "Lightning:PreCheck:ErrCheckTableEmpty",
    "check table empty error"
);
define_error!(
    ErrCheckCSVHeader,
    "Lightning:PreCheck:ErrCheckCSVHeader",
    "check csv header error"
);
define_error!(
    ErrCheckDataSource,
    "Lightning:PreCheck:ErrCheckDataSource",
    "check data source error"
);
define_error!(
    ErrCheckCDCPiTR,
    "Lightning:PreCheck:ErrCheckCDCPiTR",
    "check TiCDC/PiTR task error"
);
define_error!(
    ErrCheckPDTiDBFromSameCluster,
    "Lightning:PreCheck:ErrCheckPDTiDBSameCluster",
    "check PD and TiDB in the same cluster error"
);
define_error!(
    ErrOpenCheckpoint,
    "Lightning:Checkpoint:ErrOpenCheckpoint",
    "open checkpoint error"
);
define_error!(
    ErrReadCheckpoint,
    "Lightning:Checkpoint:ErrReadCheckpoint",
    "read checkpoint error"
);
define_error!(
    ErrUpdateCheckpoint,
    "Lightning:Checkpoint:ErrUpdateCheckpoint",
    "update checkpoint error"
);
define_error!(
    ErrUnknownCheckpointDriver,
    "Lightning:Checkpoint:ErrUnknownCheckpointDriver",
    "unknown checkpoint driver '%s'"
);
define_error!(
    ErrInvalidCheckpoint,
    "Lightning:Checkpoint:ErrInvalidCheckpoint",
    "invalid checkpoint"
);
define_error!(
    ErrCheckpointNotFound,
    "Lightning:Checkpoint:ErrCheckpointNotFound",
    "checkpoint not found"
);
define_error!(
    ErrCheckpointTableNotFound,
    "Lightning:Checkpoint:ErrCheckpointTableNotFound",
    "checkpoint for table %s not found"
);
define_error!(
    ErrInitCheckpoint,
    "Lightning:Checkpoint:ErrInitCheckpoint",
    "init checkpoint error"
);
define_error!(
    ErrCleanCheckpoint,
    "Lightning:Checkpoint:ErrCleanCheckpoint",
    "clean checkpoint error"
);
define_error!(
    ErrMetaMgrUnknown,
    "Lightning:MetaMgr:ErrMetaMgrUnknown",
    "unknown error occur on meta manager"
);
define_error!(
    ErrDBConnect,
    "Lightning:DB:ErrDBConnect",
    "failed to connect database"
);
define_error!(
    ErrInitErrManager,
    "Lightning:DB:ErrInitErrManager",
    "init error manager error"
);
define_error!(
    ErrInitMetaManager,
    "Lightning:DB:ErrInitMetaManager",
    "init meta manager error"
);
define_error!(ErrUpdatePD, "Lightning:PD:ErrUpdatePD", "update pd error");
define_error!(
    ErrCreatePDClient,
    "Lightning:PD:ErrCreatePDClient",
    "create pd client error"
);
define_error!(
    ErrCreateKVClient,
    "Lightning:KV:ErrCreateKVClient",
    "create kv client error"
);
define_error!(ErrPauseGC, "Lightning:PD:ErrPauseGC", "pause gc error");
define_error!(
    ErrCheckKVVersion,
    "Lightning:KV:ErrCheckKVVersion",
    "check tikv version error"
);
define_error!(
    ErrCheckMultiIngest,
    "Lightning:KV:ErrCheckMultiIngest",
    "check multi-ingest support error"
);
define_error!(
    ErrUnknownBackend,
    "Lightning:Restore:ErrUnknownBackend",
    "unknown backend %s"
);
define_error!(
    ErrCheckLocalFile,
    "Lightning:Restore:ErrCheckLocalFile",
    "cannot find local file for table: %s engineDir: %s"
);
define_error!(
    ErrOpenDuplicateDB,
    "Lightning:Restore:ErrOpenDuplicateDB",
    "open duplicate db error"
);
define_error!(
    ErrSchemaNotExists,
    "Lightning:Restore:ErrSchemaNotExists",
    "table `%s`.`%s` schema not found"
);
define_error!(
    ErrInvalidSchemaStmt,
    "Lightning:Restore:ErrInvalidSchemaStmt",
    "invalid schema statement: '%s'"
);
define_error!(
    ErrCreateSchema,
    "Lightning:Restore:ErrCreateSchema",
    "create schema failed, table: %s, stmt: %s"
);
define_error!(
    ErrUnknownColumns,
    "Lightning:Restore:ErrUnknownColumns",
    "unknown columns in header (%s) for table %s"
);
define_error!(
    ErrChecksumMismatch,
    "Lighting:Restore:ErrChecksumMismatch",
    "checksum mismatched remote vs local => (checksum: %d vs %d) (total_kvs: %d vs %d) (total_bytes:%d vs %d)"
);
define_error!(
    ErrRestoreTable,
    "Lightning:Restore:ErrRestoreTable",
    "restore table %s failed"
);
define_error!(
    ErrEncodeKV,
    "Lightning:Restore:ErrEncodeKV",
    "encode kv error in file %s at offset %d"
);
define_error!(
    ErrCastValue,
    "Import:ErrCastValue",
    "Value conversion failed for column '%s'. Expected type: %s, received value: %s. Reason: %s."
);
define_error!(
    ErrAllocTableRowIDs,
    "Lightning:Restore:ErrAllocTableRowIDs",
    "allocate table row id error"
);
define_error!(
    ErrInvalidMetaStatus,
    "Lightning:Restore:ErrInvalidMetaStatus",
    "invalid meta status: '%s'"
);
define_error!(
    ErrTableIsChecksuming,
    "Lightning:Restore:ErrTableIsChecksuming",
    "table '%s' is checksuming"
);
define_error!(
    ErrResolveDuplicateRows,
    "Lightning:Restore:ErrResolveDuplicateRows",
    "resolve duplicate rows error on table '%s'"
);
define_error!(
    ErrAddIndexFailed,
    "Lightning:Restore:ErrAddIndexFailed",
    "add index on table %s failed"
);
define_error!(
    ErrDropIndexFailed,
    "Lightning:Restore:ErrDropIndexFailed",
    "drop index %s on table %s failed"
);
define_error!(
    ErrFoundDataConflictRecords,
    "Lightning:Restore:ErrFoundDataConflictRecords",
    "found data conflict records in table %s, primary key is '%s', row data is '%s'"
);
define_error!(
    ErrFoundIndexConflictRecords,
    "Lightning:Restore:ErrFoundIndexConflictRecords",
    "found index conflict records in table %s, index name is '%s', unique key is '%s', primary key is '%s'"
);

/// BR error IDs used by NormalizeError mapping tests.
/// 测试用 BR 错误样例，供 NormalizeError 的 ID 映射断言使用。
pub static BR_ErrStorageUnknown: LazyLock<CommonError> =
    LazyLock::new(|| CommonError::rfc("BR:Storage:ErrStorageUnknown", "unknown storage error"));
pub static BR_ErrStorageInvalidConfig: LazyLock<CommonError> = LazyLock::new(|| {
    CommonError::rfc(
        "BR:Storage:ErrStorageInvalidConfig",
        "invalid data-source-dir",
    )
});
pub static BR_ErrStorageInvalidPermission: LazyLock<CommonError> = LazyLock::new(|| {
    CommonError::rfc(
        "BR:Storage:ErrStorageInvalidPermission",
        "invalid permission",
    )
});
pub static BR_ErrPDUpdateFailed: LazyLock<CommonError> =
    LazyLock::new(|| CommonError::rfc("BR:PD:ErrPDUpdateFailed", "update pd error"));
