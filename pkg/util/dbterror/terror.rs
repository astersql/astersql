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

// `dbterror` 对 `parser/terror` 错误类的轻量包装。
//
// 提供各子系统（DDL、Optimizer、KV 等）的 `ErrClass` 包级常量，以及用标准 MySQL
// 消息模板创建 `terror::Error` 的 `NewStd` / `NewStdErr` 入口。

// 本文件由 pkg/util/dbterror/terror.go 迁移而来，保留 dbterror 对 parser/terror 错误类的轻量包装。

use astersql_errno::errname;
use astersql_parser_terror as terror;

// ErrClass represents a class of errors.
// ErrClass 对应 Go 的 `type ErrClass struct{ terror.ErrClass }`。
// Rust 没有匿名嵌入字段，这里用 inner 字段保留“包装底层 terror.ErrClass”的语义。
/// 包装底层 `terror::ErrClass` 的错误类；`inner` 对应 Go 匿名嵌入字段。
pub struct ErrClass {
    pub inner: terror::ErrClass,
}

/// 将 errno 常量或 parser terror 错误码转换为完整宽度的错误码。
pub trait IntoErrCode {
    fn into_err_code(self) -> terror::ErrCode;
}

impl IntoErrCode for u16 {
    fn into_err_code(self) -> terror::ErrCode {
        terror::ErrCode(self as isize)
    }
}

impl IntoErrCode for terror::ErrCode {
    fn into_err_code(self) -> terror::ErrCode {
        self
    }
}

// Error classes.
// 这些包级值按 Go var 块顺序映射到底层 terror.Class*；当前只是错误分类表，不会产生外部副作用。
/// 自增 ID（Autoid）子系统错误类。
pub static ClassAutoid: ErrClass = ErrClass {
    inner: terror::ClassAutoid,
};
/// DDL（数据定义语言）子系统错误类。
pub static ClassDDL: ErrClass = ErrClass {
    inner: terror::ClassDDL,
};
/// Domain（域/元数据管理）子系统错误类。
pub static ClassDomain: ErrClass = ErrClass {
    inner: terror::ClassDomain,
};
/// 执行器子系统错误类。
pub static ClassExecutor: ErrClass = ErrClass {
    inner: terror::ClassExecutor,
};
/// 表达式求值子系统错误类。
pub static ClassExpression: ErrClass = ErrClass {
    inner: terror::ClassExpression,
};
/// 管理命令子系统错误类。
pub static ClassAdmin: ErrClass = ErrClass {
    inner: terror::ClassAdmin,
};
/// KV 存储访问子系统错误类。
pub static ClassKV: ErrClass = ErrClass {
    inner: terror::ClassKV,
};
/// 元数据子系统错误类。
pub static ClassMeta: ErrClass = ErrClass {
    inner: terror::ClassMeta,
};
/// 优化器/规划器子系统错误类。
pub static ClassOptimizer: ErrClass = ErrClass {
    inner: terror::ClassOptimizer,
};
/// 权限子系统错误类。
pub static ClassPrivilege: ErrClass = ErrClass {
    inner: terror::ClassPrivilege,
};
/// Schema（库表结构）子系统错误类。
pub static ClassSchema: ErrClass = ErrClass {
    inner: terror::ClassSchema,
};
/// Server 子系统错误类。
pub static ClassServer: ErrClass = ErrClass {
    inner: terror::ClassServer,
};
/// 结构校验子系统错误类。
pub static ClassStructure: ErrClass = ErrClass {
    inner: terror::ClassStructure,
};
/// 系统变量子系统错误类。
pub static ClassVariable: ErrClass = ErrClass {
    inner: terror::ClassVariable,
};
/// 表达式求值扩展（XEval）子系统错误类。
pub static ClassXEval: ErrClass = ErrClass {
    inner: terror::ClassXEval,
};
/// 表操作子系统错误类。
pub static ClassTable: ErrClass = ErrClass {
    inner: terror::ClassTable,
};
/// 类型系统子系统错误类。
pub static ClassTypes: ErrClass = ErrClass {
    inner: terror::ClassTypes,
};
/// JSON 类型子系统错误类。
pub static ClassJSON: ErrClass = ErrClass {
    inner: terror::ClassJSON,
};
/// TiKV 客户端子系统错误类。
pub static ClassTiKV: ErrClass = ErrClass {
    inner: terror::ClassTiKV,
};
/// Session 会话子系统错误类。
pub static ClassSession: ErrClass = ErrClass {
    inner: terror::ClassSession,
};
/// 插件子系统错误类。
pub static ClassPlugin: ErrClass = ErrClass {
    inner: terror::ClassPlugin,
};
/// 工具包子系统错误类。
pub static ClassUtil: ErrClass = ErrClass {
    inner: terror::ClassUtil,
};

impl ErrClass {
    // NewStd calls New using the standard message for the error code
    // Attention:
    // this method is not goroutine-safe and
    // usually be used in global variable initializer
    // NewStd 对应 Go 方法：用 errno.MySQLErrName 中的标准消息创建 terror.Error。
    // Go 注释强调它通常用于全局变量初始化且非 goroutine-safe；保留这个并发注意点。
    pub fn NewStd(&self, code: impl IntoErrCode) -> Box<terror::Error> {
        let code = code.into_err_code();
        let mysql_code = code.0 as u16;
        self.NewStdErr(code, &errname::MySQLErrName[&mysql_code])
    }

    /// NewStdErr mirrors the promoted Go method while accepting errno constants and ErrCode.
    /// 用指定错误码与消息模板创建错误，并保留 `terror::ErrCode` 的完整宽度。
    pub fn NewStdErr(
        &self,
        code: impl IntoErrCode,
        message: &terror::parser::mysql::errname::ErrMessage,
    ) -> Box<terror::Error> {
        self.inner.NewStdErr(code.into_err_code(), message)
    }
}
