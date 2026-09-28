// Copyright 2026 AsterSQL.
// Copyright 2015 PingCAP, Inc.
//
// Licensed under the Apache License, Version 2.0 (the "License");
// you may not use this file except in compliance with the License.
// You may obtain a copy of the License at
//
//     http://www.apache.org/licenses/LICENSE-2.0
//
// Unless required by applicable law or agreed to in writing, software
// distributed under the License is distributed on an "AS IS" BASIS,
// See the License for the specific language governing permissions and
// limitations under the License.

// terror：按 ErrClass / ErrCode 分类的错误框架。
//
// 对应 Go `terror` 包：注册错误类别与码，生成带 RFC 前缀的规范化错误，
// 并转换为 MySQL 协议 SQLError。ErrClass 表示子系统（如 parser、kv），
// ErrCode 为类内错误编号；RFCCode 形如 `parser:1062`。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{LazyLock, RwLock};

// 正式包与兼容 task 都通过 crate 根别名复用共享 errors/mysql 基座。
use crate::errors;
use crate::parser::mysql;

/// 与共享 errors 基座的 Error 类型别名。
pub type Error = errors::Error;

// ErrCode 对应 Go int 错误码；同一个数值可以在不同 ErrClass 中复用。
/// 错误码包装类型；同一数值可在不同 ErrClass 中复用。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ErrCode(pub isize);

/// 未知错误码。
pub const CodeUnknown: ErrCode = ErrCode(-1);
/// 执行结果为空的临界错误码。
pub const CodeExecResultIsEmpty: ErrCode = ErrCode(3);
/// 缺少连接 ID。
pub const CodeMissConnectionID: ErrCode = ErrCode(1);
/// 执行结果不确定。
pub const CodeResultUndetermined: ErrCode = ErrCode(2);

// ErrClass 对应 Go 的错误类别编号。
/// 错误类别编号，标识产生错误的子系统。
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct ErrClass(pub isize);

// 内置错误类在注册表首次访问时一次性完整填充，访问单个常量不会改变注册顺序或内容。
static errClass2Desc: LazyLock<RwLock<HashMap<ErrClass, String>>> = LazyLock::new(|| {
    RwLock::new(
        ERROR_CLASSES
            .iter()
            .map(|(class, description)| (*class, (*description).to_owned()))
            .collect(),
    )
});

// Code2ErrClassMap 对应包含 sync.Map 的 Go 包装类型；这里用 RwLock 保留并发 Get/Put 语义。
/// RFC 类名前缀到 ErrClass 的并发映射。
pub struct Code2ErrClassMap {
    data: RwLock<HashMap<String, ErrClass>>,
}

/// 构造空的 Code2ErrClassMap。
pub fn newCode2ErrClassMap() -> Code2ErrClassMap {
    Code2ErrClassMap {
        data: RwLock::new(HashMap::new()),
    }
}

impl Code2ErrClassMap {
    // Get 在键不存在时返回 (-1, false)，与 Go 的类型断言分支一致。
    /// 按 RFC 前缀查找 ErrClass；未命中返回 (ErrClass(-1), false)。
    pub fn Get(&self, key: &str) -> (ErrClass, bool) {
        match self
            .data
            .read()
            .expect("rfc class map poisoned")
            .get(key)
            .copied()
        {
            Some(class) => (class, true),
            None => (ErrClass(-1), false),
        }
    }

    // Put 原子地替换同名 RFC 类前缀；锁离开作用域时自动释放，无需 defer 收尾。
    /// 写入或覆盖 RFC 前缀到 ErrClass 的映射。
    pub fn Put(&self, key: &str, err: ErrClass) {
        self.data
            .write()
            .expect("rfc class map poisoned")
            .insert(key.to_owned(), err);
    }
}

static rfcCode2errClass: LazyLock<Code2ErrClassMap> = LazyLock::new(|| {
    let classes = newCode2ErrClassMap();
    // Go 包变量初始化会立即构造 ErrCritical / ErrResultUndetermined，
    // 因而 "global" 反向映射在任何公开调用前已经存在。
    classes.Put("global", ClassGlobal);
    classes
});
static registerFinish: AtomicU32 = AtomicU32::new(0);

// RegisterFinish 冻结后续错误注册；Release/Acquire 保证其它线程观察到初始化阶段的注册结果。
/// 冻结错误码注册；之后再 New/NewStd 会 panic。
pub fn RegisterFinish() {
    registerFinish.store(1, Ordering::Release);
}

/// 是否已调用 RegisterFinish。
fn frozen() -> bool {
    registerFinish.load(Ordering::Acquire) != 0
}

// RegisterErrorClass 对应 Go 的全局类注册：重复编号立即 panic，不静默覆盖描述。
/// 注册新的错误类别；重复 classCode 立即 panic。
pub fn RegisterErrorClass(classCode: isize, desc: &str) -> ErrClass {
    let class = ErrClass(classCode);
    let mut descriptions = errClass2Desc
        .write()
        .expect("error-class registry poisoned");
    if descriptions.contains_key(&class) {
        drop(descriptions);
        panic!("duplicate register ClassCode {} - {}", classCode, desc);
    }
    descriptions.insert(class, desc.to_owned());
    class
}

/// 自增 ID（autoid）子系统错误类。
pub const ClassAutoid: ErrClass = ErrClass(1);
/// DDL 子系统错误类。
pub const ClassDDL: ErrClass = ErrClass(2);
/// Domain（域/租户运行时）错误类。
pub const ClassDomain: ErrClass = ErrClass(3);
/// 表达式求值器错误类。
pub const ClassEvaluator: ErrClass = ErrClass(4);
/// 执行器错误类。
pub const ClassExecutor: ErrClass = ErrClass(5);
/// 表达式构建错误类。
pub const ClassExpression: ErrClass = ErrClass(6);
/// 管理命令错误类。
pub const ClassAdmin: ErrClass = ErrClass(7);
/// KV 存储层错误类。
pub const ClassKV: ErrClass = ErrClass(8);
/// Meta（元数据）错误类。
pub const ClassMeta: ErrClass = ErrClass(9);
/// 优化器/规划器错误类。
pub const ClassOptimizer: ErrClass = ErrClass(10);
/// SQL 解析器错误类。
pub const ClassParser: ErrClass = ErrClass(11);
/// Performance Schema 错误类。
pub const ClassPerfSchema: ErrClass = ErrClass(12);
/// 权限子系统错误类。
pub const ClassPrivilege: ErrClass = ErrClass(13);
/// Schema（库表结构）错误类。
pub const ClassSchema: ErrClass = ErrClass(14);
/// Server（服务端）错误类。
pub const ClassServer: ErrClass = ErrClass(15);
/// 内部结构错误类。
pub const ClassStructure: ErrClass = ErrClass(16);
/// 会话变量错误类。
pub const ClassVariable: ErrClass = ErrClass(17);
/// X 协议求值错误类。
pub const ClassXEval: ErrClass = ErrClass(18);
/// 表操作错误类。
pub const ClassTable: ErrClass = ErrClass(19);
/// 类型系统错误类。
pub const ClassTypes: ErrClass = ErrClass(20);
/// 全局通用错误类。
pub const ClassGlobal: ErrClass = ErrClass(21);
/// Mock TiKV 测试错误类。
pub const ClassMockTikv: ErrClass = ErrClass(22);
/// JSON 相关错误类。
pub const ClassJSON: ErrClass = ErrClass(23);
/// TiKV 客户端错误类。
pub const ClassTiKV: ErrClass = ErrClass(24);
/// Session（会话）错误类。
pub const ClassSession: ErrClass = ErrClass(25);
/// 插件错误类。
pub const ClassPlugin: ErrClass = ErrClass(26);
/// 工具库错误类。
pub const ClassUtil: ErrClass = ErrClass(27);

static ERROR_CLASSES: [(ErrClass, &str); 27] = [
    (ClassAutoid, "autoid"),
    (ClassDDL, "ddl"),
    (ClassDomain, "domain"),
    (ClassEvaluator, "evaluator"),
    (ClassExecutor, "executor"),
    (ClassExpression, "expression"),
    (ClassAdmin, "admin"),
    (ClassKV, "kv"),
    (ClassMeta, "meta"),
    (ClassOptimizer, "planner"),
    (ClassParser, "parser"),
    (ClassPerfSchema, "perfschema"),
    (ClassPrivilege, "privilege"),
    (ClassSchema, "schema"),
    (ClassServer, "server"),
    (ClassStructure, "structure"),
    (ClassVariable, "variable"),
    (ClassXEval, "xeval"),
    (ClassTable, "table"),
    (ClassTypes, "types"),
    (ClassGlobal, "global"),
    (ClassMockTikv, "mocktikv"),
    (ClassJSON, "json"),
    (ClassTiKV, "tikv"),
    (ClassSession, "session"),
    (ClassPlugin, "plugin"),
    (ClassUtil, "util"),
];

// ErrClassToMySQLCodes 保存每一错误类已注册的 ErrCode 集合；空元组对应 Go struct{} 零开销集合值。
/// 各类别已注册 ErrCode 的全局集合（值为空元组，仅作集合成员标记）。
pub static ErrClassToMySQLCodes: LazyLock<RwLock<HashMap<ErrClass, HashMap<ErrCode, ()>>>> =
    LazyLock::new(|| {
        RwLock::new(HashMap::from([(
            ClassGlobal,
            HashMap::from([(CodeExecResultIsEmpty, ()), (CodeResultUndetermined, ())]),
        )]))
    });

impl ErrClass {
    // String 对应 fmt.Stringer；未知类别退回十进制编号。
    /// 返回类别描述字符串；未注册时回退为数字编号。
    pub fn String(self) -> String {
        errClass2Desc
            .read()
            .expect("error-class registry poisoned")
            .get(&self)
            .cloned()
            .unwrap_or_else(|| self.0.to_string())
    }

    // Go 的构造路径直接索引 errClass2Desc；未注册类得到空字符串，不能复用 String 的数字回退。
    /// 仅返回已注册描述；未注册返回空串（与 Go 索引行为一致）。
    fn registeredDescription(self) -> String {
        errClass2Desc
            .read()
            .expect("error-class registry poisoned")
            .get(&self)
            .cloned()
            .unwrap_or_default()
    }

    // EqualClass 先取 errors.Cause，再从 RFCCode 的冒号前缀反查类别。
    /// 判断错误根因是否属于本 ErrClass。
    pub fn EqualClass(self, err: Option<&errors::SharedError>) -> bool {
        let Some(cause) = errors::Cause(err) else {
            return false;
        };
        let Some(terror) = cause.downcast_ref::<Error>() else {
            return false;
        };
        let rfc = terror.RFCCode().to_string();
        let Some((prefix, _)) = rfc.split_once(':') else {
            return false;
        };
        if prefix.is_empty() {
            return false;
        }
        let (class, found) = rfcCode2errClass.Get(prefix);
        found && class == self
    }

    /// EqualClass 的否定形式。
    pub fn NotEqualClass(self, err: Option<&errors::SharedError>) -> bool {
        !self.EqualClass(err)
    }

    // initError 只允许在冻结前调用，同时登记类到错误码集合以及 RFC 前缀到类的反向映射。
    /// 登记错误码并返回 RFC 文本；冻结后调用会 panic。
    fn initError(self, code: ErrCode) -> String {
        if frozen() {
            // Go 会先 debug.PrintStack 再 panic；Backtrace 保留同一诊断目的。
            eprintln!("{:?}", std::backtrace::Backtrace::force_capture());
            panic!("register error after initialized is prohibited");
        }

        ErrClassToMySQLCodes
            .write()
            .expect("error-code registry poisoned")
            .entry(self)
            .or_default()
            .insert(code, ());

        let class = self.registeredDescription();
        let rfc = format!("{}:{}", class, code.0);
        rfcCode2errClass.Put(&class, self);
        rfc
    }

    // New 保留已弃用的 Go 构造入口：先注册，再附加 MySQL code 和 RFC code。
    /// 已弃用：注册错误码并构造带消息的 Error。
    #[deprecated(note = "use NewStd or NewStdErr instead")]
    pub fn New(self, code: ErrCode, message: &str) -> Box<Error> {
        let rfc = self.initError(code);
        Box::new(errors::Normalize(
            message,
            &[
                errors::MySQLErrorCode(code.0 as i32),
                errors::RFCCodeText(rfc),
            ],
        ))
    }

    // NewStdErr 使用标准消息及脱敏参数位置，构造前仍会登记错误码。
    /// 用标准 ErrMessage（含脱敏参数位置）构造并注册错误。
    pub fn NewStdErr(self, code: ErrCode, message: &mysql::errname::ErrMessage) -> Box<Error> {
        let rfc = self.initError(code);
        Box::new(errors::Normalize(
            &message.Raw,
            &[
                errors::RedactArgs(&message.RedactArgPos),
                errors::MySQLErrorCode(code.0 as i32),
                errors::RFCCodeText(rfc),
            ],
        ))
    }

    // NewStd 从 mysql 标准错误消息表取模板，再委托 NewStdErr。
    /// 按 MySQL 标准错误码查消息表后构造错误。
    pub fn NewStd(self, code: ErrCode) -> Box<Error> {
        let messages = mysql::errname::MySQLErrName();
        self.NewStdErr(
            code,
            messages
                .get(&(code.0 as u16))
                .expect("standard MySQL error code must have a message"),
        )
    }

    // Synthesize 不写 ErrClassToMySQLCodes，因此可在初始化完成后并发构造外部系统错误。
    /// 合成错误：不注册码表，可在冻结后使用。
    pub fn Synthesize(self, code: ErrCode, message: &str) -> Box<Error> {
        let class = self.registeredDescription();
        Box::new(errors::Normalize(
            message,
            &[
                errors::MySQLErrorCode(code.0 as i32),
                errors::RFCCodeText(format!("{}:{}", class, code.0)),
            ],
        ))
    }
}

// 默认 MySQL 错误码对应 Go init 中赋入的 mysql.ErrUnknown。
static defaultMySQLErrorCode: u16 = mysql::errcode::ErrUnknown;

// getMySQLErrorCode 先解析 RFC 类，再确认该 code 确实在类集合中注册；异常路径均回退默认码并记录日志。
/// 从 terror Error 解析已注册的 MySQL 错误码；未知则回退 ErrUnknown。
fn getMySQLErrorCode(error: &Error) -> u16 {
    let rfc = error.RFCCode();
    let class =
        if let Some((prefix, _)) = rfc.split_once(':').filter(|(prefix, _)| !prefix.is_empty()) {
            let (class, found) = rfcCode2errClass.Get(prefix);
            if !found {
                log::warn!("Unknown error class: {prefix}");
                return defaultMySQLErrorCode;
            }
            class
        } else {
            ErrClass(0)
        };

    let classes = ErrClassToMySQLCodes
        .read()
        .expect("error-code registry poisoned");
    let Some(codes) = classes.get(&class) else {
        log::warn!("Unknown error class: {}", class.0);
        return defaultMySQLErrorCode;
    };
    let code = ErrCode(error.Code() as isize);
    if !codes.contains_key(&code) {
        log::debug!("Unknown error code: class={}, code={}", class.0, code.0);
        return defaultMySQLErrorCode;
    }
    code.0 as u16
}

// ToSQLError 保留 Go NewErrf 的格式化形状，把 terror Error 转换为 MySQL 协议错误。
/// 将 terror Error 转为 MySQL 协议层 SQLError。
pub fn ToSQLError(error: &Error) -> mysql::error::SQLError {
    mysql::error::NewErrf(
        getMySQLErrorCode(error),
        "%s",
        &[],
        vec![error.GetMsg().into()],
    )
}

// 两个全局错误对应 Go var 初始化；LazyLock 避免 Rust 静态初始化期间调用非 const 构造函数。
/// 全局临界错误模板（执行结果为空）。
pub static ErrCritical: LazyLock<Box<Error>> = LazyLock::new(|| {
    ClassGlobal.NewStdErr(
        CodeExecResultIsEmpty,
        &mysql::errname::Message("critical error %v", &[]),
    )
});
/// 全局“执行结果不确定”错误模板。
pub static ErrResultUndetermined: LazyLock<Box<Error>> = LazyLock::new(|| {
    ClassGlobal.NewStdErr(
        CodeResultUndetermined,
        &mysql::errname::Message("execution result undetermined", &[]),
    )
});

// ErrorEqual 委托给共享 errors 基座，以统一根 cause、Normalize ID 和普通错误文本比较规则。
/// 比较两个错误是否相等（根因 / Normalize ID / 文本）。
pub fn ErrorEqual(err1: Option<&errors::SharedError>, err2: Option<&errors::SharedError>) -> bool {
    errors::ErrorEqual(err1, err2)
}

/// ErrorEqual 的否定形式。
pub fn ErrorNotEqual(
    err1: Option<&errors::SharedError>,
    err2: Option<&errors::SharedError>,
) -> bool {
    !ErrorEqual(err1, err2)
}

// MustNil 对齐 Go log.Fatal：先顺序执行清理回调、记录并刷新日志，再以状态 1 终止进程。
/// 若 error 非空则先执行清理回调，再以退出码 1 终止进程。
pub fn MustNil(error: Option<&errors::DynError>, closeFuns: &mut [Box<dyn FnMut()>]) {
    if let Some(error) = error {
        for close in closeFuns {
            close();
        }
        log::error!("unexpected error: {error}");
        log::logger().flush();
        std::process::exit(1);
    }
}

// Call 执行一次可能失败的函数；错误只记录，不向调用者继续传播。
/// 调用可能失败的函数，错误仅写日志不传播。
pub fn Call<F, E>(function: F)
where
    F: FnOnce() -> Result<(), E>,
    E: std::fmt::Display,
{
    if let Err(error) = function() {
        log::error!("function call errored: {error}");
    }
}

// Log 仅在 Some(error) 时写日志，对应 Go 的 nil 检查。
/// 若存在错误则记录 error 级别日志。
pub fn Log(error: Option<&errors::DynError>) {
    if let Some(error) = error {
        log::error!("encountered error: {error}");
    }
}

// GetErrClass 从 RFC code 冒号前缀反查类别；无法识别时返回 -1。
/// 从 Error 的 RFCCode 反查 ErrClass；失败返回 ErrClass(-1)。
pub fn GetErrClass(error: &Error) -> ErrClass {
    let rfc = error.RFCCode().to_string();
    if let Some((prefix, _)) = rfc.split_once(':').filter(|(prefix, _)| !prefix.is_empty()) {
        let (class, found) = rfcCode2errClass.Get(prefix);
        if found {
            return class;
        }
    }
    ErrClass(-1)
}
