// Copyright 2026 AsterSQL.
// Copyright 2023 PingCAP, Inc.
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

#![allow(dead_code, non_camel_case_types, non_snake_case)]
// SQL warning 收集与处理，对齐 MySQL `SHOW WARNINGS` 语义。
//
// 提供 Warning/Note/Error 级别常量、`SQLWarn` JSON 编解码、
// `WarnAppender`/`WarnHandler`/`WarnHandlerExt` 接口，以及互斥保护的
// `StaticWarnHandler`、全局忽略型 `IgnoreWarn` 与测试用回调追加器。

use crate::errors;
use std::sync::Mutex;

// WarnLevelError represents level "Error" for 'SHOW WARNINGS' syntax.
/// `SHOW WARNINGS` 结果中的 Error 级别字符串。
// WarnLevelError 对应 SHOW WARNINGS 返回行里的 Error 级别。
pub const WarnLevelError: &str = "Error";
// WarnLevelWarning represents level "Warning" for 'SHOW WARNINGS' syntax.
/// `SHOW WARNINGS` 结果中的 Warning 级别字符串。
// WarnLevelWarning 对应 SHOW WARNINGS 返回行里的 Warning 级别。
pub const WarnLevelWarning: &str = "Warning";
// WarnLevelNote represents level "Note" for 'SHOW WARNINGS' syntax.
/// `SHOW WARNINGS` 结果中的 Note 级别字符串。
// WarnLevelNote 对应 SHOW WARNINGS 返回行里的 Note 级别。
pub const WarnLevelNote: &str = "Note";

// SQLWarn relates a sql warning and it's level.
/// 一条 SQL warning：级别字符串 + 可选错误值。
// SQLWarn 保存一条 SQL warning 的级别和原始错误；Err 对应 Go 的 error 接口值。
#[derive(Clone, Debug)]
pub struct SQLWarn {
    pub Level: String,
    pub Err: Option<errors::SharedError>,
}

// jsonSQLWarn 是 SQLWarn 的 JSON 中间形态。
/// SQLWarn 的 JSON 中间形态：terror.Error 走 `err`，其它错误走 `msg`。
// Go 代码会把 terror.Error 单独放入 err 字段，其它错误只序列化为 msg 字符串。
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub struct jsonSQLWarn {
    #[serde(rename = "level")]
    pub Level: String,
    #[serde(rename = "err", skip_serializing_if = "Option::is_none")]
    pub SQLErr: Option<errors::Error>,
    #[serde(default, rename = "msg", skip_serializing_if = "String::is_empty")]
    pub Msg: String,
}

impl SQLWarn {
    // MarshalJSON implements the Marshaler.MarshalJSON interface.
    /// 序列化为 JSON；只暴露最内层 terror.Error，否则暴露错误消息。
    // MarshalJSON 保留 Go 的 json.Marshaler 语义：只暴露最内层 terror.Error，否则暴露错误消息。
    pub fn MarshalJSON(&self) -> Result<Vec<u8>, errors::SharedError> {
        let mut w = jsonSQLWarn {
            Level: self.Level.clone(),
            ..Default::default()
        };

        // Go 会在 errors.Cause(nil) 后调用 e.Error() 并 panic；None 保留同一非法输入行为。
        let err = self.Err.as_ref().expect("SQLWarn.MarshalJSON requires Err");
        // 对应 errors.Cause(warn.Err)，去掉外层包装后再判断是否为 *terror.Error。
        let e = errors::Cause(Some(err)).expect("present error has a cause");
        if let Some(sql_err) = e.downcast_ref::<errors::Error>() {
            // Omit outter errors because only the most inner error matters.
            // Go 只保留最内层 terror.Error，外层 Trace 包装不会进入 JSON。
            w.SQLErr = Some(sql_err.clone());
        } else {
            w.Msg = e.to_string();
        }

        // 对应 json.Marshal(w)；serde_json 只是中的明显 Rust 映射。
        serde_json::to_vec(&w).map_err(errors::SharedError::new)
    }

    // UnmarshalJSON implements the Unmarshaler.UnmarshalJSON interface.
    /// 从 JSON 反序列化；优先恢复 terror.Error，否则用 msg 新建错误。
    // UnmarshalJSON 保留 Go 的 json.Unmarshaler 语义：优先恢复 terror.Error，否则用 msg 新建普通错误。
    pub fn UnmarshalJSON(&mut self, data: &[u8]) -> Result<(), errors::SharedError> {
        // 对应 var w jsonSQLWarn 和 json.Unmarshal(data, &w)。
        let w: jsonSQLWarn = serde_json::from_slice(data).map_err(errors::SharedError::new)?;
        self.Level = w.Level;
        if let Some(sql_err) = w.SQLErr {
            // Go 里 *terror.Error 直接赋给 error 接口；用 errors::From 保留装箱意图。
            self.Err = Some(errors::SharedError::new(sql_err));
        } else {
            self.Err = Some(errors::New(w.Msg));
        }
        Ok(())
    }
}

// WarnAppender provides a function to add a warning.
/// 追加 warning/note 的最小能力接口（避免闭包额外分配）。
// Using interface rather than a simple function/closure can avoid memory allocation in some cases.
// See https://github.com/pingcap/tidb/issues/49277
// WarnAppender 对应 Go 接口，抽象“追加 warning/note”的最小能力。
pub trait WarnAppender {
    // AppendWarning appends a warning
    // AppendWarning 追加默认 Warning 级别的错误。
    fn AppendWarning(&self, err: errors::SharedError);
    // AppendNote appends a warning with level 'Note'.
    // AppendNote 追加 Note 级别的错误。
    fn AppendNote(&self, msg: errors::SharedError);
}

// WarnHandler provides a handler to append and get warnings.
/// 在 WarnAppender 上扩展计数、截断与复制能力。
// WarnHandler 扩展 WarnAppender，提供计数、截断和复制 warning 的能力。
pub trait WarnHandler: WarnAppender {
    // WarningCount gets warning count.
    // WarningCount 返回当前累计 warning 数。
    fn WarningCount(&self) -> usize;

    // TruncateWarnings truncates warnings begin from start and returns the truncated warnings.
    // Deprecated: This method is deprecated. Because it's unsafe to read the warnings returned by `GetWarnings`
    // after truncate and append the warnings.
    // Currently it's used in two cases and they all have better alternatives:
    // 1. Read warnings count, do some operation and truncate warnings to read new warnings. In this case, we
    //   can use a new temporary WarnHandler to do the operation and get the warnings without touching the
    //   global `WarnHandler` in the statement context.
    // 2. Read warnings count, do some operation and truncate warnings to see whether new warnings are appended.
    //   In this case, we can use a specially designed `WarnHandler` which doesn't actually record warnings, but
    //   just counts whether new warnings are appended.
    // It's understandable to use `TruncateWarnings` as it's not always easy to assign a new `WarnHandler` to the
    // context now.
    // TODO: Make it easier to assign a new `WarnHandler` to the context (of `table` and other packages) and remove
    // this method.
    // TruncateWarnings 从 start 开始截断 warning 并返回被截出的副本；Go 语义会修改内部切片。
    fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn>;

    // CopyWarnings copies warnings to another slice.
    // The input argument provides target warnings that copy to.
    // If the dist capacity is not enough, it will allocate a new slice.
    // CopyWarnings 把内部 warning 复制到目标 Vec；容量不足时由 Vec 重新分配来对应 Go 行为。
    fn CopyWarnings(&self, dst: Vec<SQLWarn>) -> Vec<SQLWarn>;
}

// WarnHandlerExt includes more methods for WarnHandler. It allows more detailed control over warnings.
/// WarnHandler 的细粒度扩展：批量追加、Error 级别、读写全量集合。
// TODO: it's a standalone interface, because it's not necessary for all WarnHandler to implement these methods.
// However, it's still needed for many executors, so we'll see whether it's good to merge it with `WarnAppender`
// and `WarnHandler` in the future.
// WarnHandlerExt 对应 Go 的扩展接口，给 executor 等调用点更细粒度地控制 warning 集合。
pub trait WarnHandlerExt: WarnHandler {
    // AppendWarnings appends multiple warnings
    // AppendWarnings 批量追加 warning，沿用 Go 中受 MaxUint16 限制的入口语义。
    fn AppendWarnings(&self, warns: Vec<SQLWarn>);
    // AppendNote appends a warning with level 'Note'.
    // AppendNote 在扩展接口中重复声明，保留 Go 源文件的接口形状。
    fn AppendNote(&self, warn: errors::SharedError);
    // AppendError appends a warning with level 'Error'.
    // AppendError 追加 Error 级别的 warning。
    fn AppendError(&self, warn: errors::SharedError);

    // GetWarnings gets all warnings. The slice is not copied, so it should not be modified.
    // GetWarnings 在 Go 中返回内部切片别名；返回 Vec 副本并用注释保留“不应修改”的契约。
    fn GetWarnings(&self) -> Vec<SQLWarn>;
    // SetWarnings resets all warnings in the handler directly. The handler may ignore the given warnings.
    // SetWarnings 直接替换内部 warning 集合，忽略型 handler 可以选择丢弃。
    fn SetWarnings(&self, warns: Vec<SQLWarn>);
    // NumErrorWarnings returns the number of warnings with level 'Error' and the total number of warnings.
    // NumErrorWarnings 返回 Error 级别数量和 warning 总数。
    fn NumErrorWarnings(&self) -> (u16, usize);
}

// Go 源文件的 `var _ WarnHandler = &StaticWarnHandler{}` 是编译期接口断言；
// Rust 用泛型约束函数表达同一意图，不在运行时产生动作。
#[allow(unused)]
fn _assert_StaticWarnHandler_implements_WarnHandler<T: WarnHandler>() {}

// StaticWarnHandler implements the WarnHandler interface.
/// 用 Mutex 保护的静态 warning 切片实现。
// StaticWarnHandler 用互斥锁保护 warning 切片，对应 Go 结构体里嵌入 sync.Mutex 的设计。
pub struct StaticWarnHandler {
    pub warnings: Mutex<Vec<SQLWarn>>,
}

// NewStaticWarnHandler creates a new StaticWarnHandler.
/// 按容量预分配构造 `StaticWarnHandler`；非正容量用空 Vec。
// NewStaticWarnHandler 按 Go 的 sliceCap 预分配 warning 容量；非正容量使用空 Vec。
pub fn NewStaticWarnHandler(sliceCap: isize) -> StaticWarnHandler {
    let warnings = if sliceCap > 0 {
        Vec::with_capacity(sliceCap as usize)
    } else {
        Vec::new()
    };
    StaticWarnHandler {
        warnings: Mutex::new(warnings),
    }
}

// NewStaticWarnHandlerWithHandler creates a new StaticWarnHandler with copying the warnings from the given WarnHandler.
/// 从已有 WarnHandler 复制 warning 构造新 handler；`None` 得到空 handler。
// NewStaticWarnHandlerWithHandler 从已有 WarnHandler 复制 warning；nil handler 映射为 None。
pub fn NewStaticWarnHandlerWithHandler(h: Option<&dyn WarnHandler>) -> StaticWarnHandler {
    if h.is_none() {
        return NewStaticWarnHandler(0);
    }

    let h = h.expect("checked above");
    let cnt = h.WarningCount();
    let newHandler = NewStaticWarnHandler(cnt as isize);
    if cnt > 0 {
        // Go 直接写 newHandler.warnings；Rust 需要先拿锁再替换内部 Vec。
        let copied = h.CopyWarnings(Vec::with_capacity(cnt));
        *newHandler
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned") = copied;
    }
    newHandler
}

impl StaticWarnHandler {
    // Reset resets the warnings of this handler.
    /// 清空 warning 但保留 Vec 容量（对应 Go `warnings[:0]`）。
    // Reset 清空已有 warning 但保留 Vec 容量，对应 Go 的 h.warnings[:0]。
    pub fn Reset(&self) {
        let mut warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        if !warnings.is_empty() {
            warnings.clear();
        }
    }

    // appendWarningWithLevel 按指定级别追加 warning；调用方已通过公开方法决定级别。
    fn appendWarningWithLevel(&self, level: &str, warn: errors::SharedError) {
        let mut warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        if warnings.len() < u16::MAX as usize {
            warnings.push(SQLWarn {
                Level: level.to_owned(),
                Err: Some(warn),
            });
        }
    }
}

impl WarnAppender for StaticWarnHandler {
    // AppendWarning implements the StaticWarnHandler.AppendWarning.
    // AppendWarning 对应 Go 中 Lock/defer Unlock 包裹 appendWarningWithLevel 的实现。
    fn AppendWarning(&self, warn: errors::SharedError) {
        self.appendWarningWithLevel(WarnLevelWarning, warn);
    }

    // AppendNote appends a warning with level 'Note'.
    // AppendNote 追加 Note 级别，锁和容量检查由 appendWarningWithLevel 统一处理。
    fn AppendNote(&self, warn: errors::SharedError) {
        self.appendWarningWithLevel(WarnLevelNote, warn);
    }
}

impl WarnHandler for StaticWarnHandler {
    // WarningCount implements the StaticWarnHandler.WarningCount.
    // WarningCount 加锁读取当前 Vec 长度，对应 Go 的 len(h.warnings)。
    fn WarningCount(&self) -> usize {
        let warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        warnings.len()
    }

    // TruncateWarnings implements the StaticWarnHandler.TruncateWarnings.
    // TruncateWarnings 保留 Go 先复制返回切片、再截短内部 warnings 的顺序。
    fn TruncateWarnings(&self, start: isize) -> Vec<SQLWarn> {
        let mut warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        let sz = warnings.len() as isize - start;
        if sz <= 0 {
            return Vec::new();
        }

        // Go 传负 start 会在切片表达式处 panic；也不主动修正这种非法调用。
        let start = start as usize;
        let ret = warnings[start..].to_vec();
        warnings.truncate(start);
        ret
    }

    // CopyWarnings implements the StaticWarnHandler.CopyWarnings.
    // CopyWarnings 返回与内部 Vec 分离的新切片；dst 容量足够时尽量复用传入 Vec。
    fn CopyWarnings(&self, mut dst: Vec<SQLWarn>) -> Vec<SQLWarn> {
        let warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        if dst.capacity() < warnings.len() {
            dst = Vec::with_capacity(warnings.len());
        }
        dst.clear();
        dst.extend_from_slice(&warnings);
        dst
    }
}

impl WarnHandlerExt for StaticWarnHandler {
    // AppendWarnings appends multiple warnings
    // AppendWarnings 批量追加时只在追加前检查当前长度是否小于 MaxUint16，保持 Go 的边界语义。
    fn AppendWarnings(&self, warns: Vec<SQLWarn>) {
        let mut warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        if warnings.len() < u16::MAX as usize {
            warnings.extend(warns);
        }
    }

    // AppendNote appends a warning with level 'Note'.
    // 扩展接口的 AppendNote 与 WarnAppender 的同名方法共享同一追加逻辑。
    fn AppendNote(&self, warn: errors::SharedError) {
        self.appendWarningWithLevel(WarnLevelNote, warn);
    }

    // AppendError appends a warning with level 'Error'.
    // AppendError 追加 Error 级别，供执行器记录更高严重度的 warning。
    fn AppendError(&self, warn: errors::SharedError) {
        self.appendWarningWithLevel(WarnLevelError, warn);
    }

    // GetWarnings returns all warnings in the handler. It's not safe to modify the returned slice.
    // GetWarnings 在 Go 中返回内部切片本身；为避免跨锁返回引用，返回克隆并保留契约说明。
    fn GetWarnings(&self) -> Vec<SQLWarn> {
        let warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        warnings.clone()
    }

    // SetWarnings sets the internal warnings directly.
    // SetWarnings 直接替换内部 Vec；这对应 Go 里的 h.warnings = warns。
    fn SetWarnings(&self, warns: Vec<SQLWarn>) {
        let mut warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");
        *warnings = warns;
    }

    // NumErrorWarnings returns the number of warnings with level 'Error' and the total number of warnings.
    // NumErrorWarnings 遍历内部 Vec 统计 Error 级别数量，并同时返回总数。
    fn NumErrorWarnings(&self) -> (u16, usize) {
        let warnings = self
            .warnings
            .lock()
            .expect("StaticWarnHandler mutex poisoned");

        let mut numError: u16 = 0;
        for w in warnings.iter() {
            if w.Level == WarnLevelError {
                // Go 的 uint16 自增在极端溢出时会回绕；显式使用 wrapping_add 保留该数值语义。
                numError = numError.wrapping_add(1);
            }
        }
        (numError, warnings.len())
    }
}

// ignoreWarn 对应 Go 的空结构体实现，所有追加和读取动作都被丢弃。
/// 空实现：所有追加与读取均丢弃。
pub struct ignoreWarn {}

impl WarnAppender for ignoreWarn {
    fn AppendWarning(&self, _err: errors::SharedError) {}

    fn AppendNote(&self, _err: errors::SharedError) {}
}

impl WarnHandler for ignoreWarn {
    fn WarningCount(&self) -> usize {
        0
    }

    fn TruncateWarnings(&self, _start: isize) -> Vec<SQLWarn> {
        Vec::new()
    }

    fn CopyWarnings(&self, _dst: Vec<SQLWarn>) -> Vec<SQLWarn> {
        Vec::new()
    }
}

// IgnoreWarn is WarnHandler which does nothing
/// 全局忽略型 WarnHandler 单例。
// IgnoreWarn 是全局忽略型 handler；Go 中它是 WarnHandler 接口值，用静态空实现占位。
pub static IgnoreWarn: ignoreWarn = ignoreWarn {};

// funcWarnAppender 保存外部回调，供测试按 level 和 error 观察 warning 追加动作。
/// 测试用：将追加动作转发到外部回调。
pub struct funcWarnAppender {
    pub fn_: Box<dyn Fn(&str, errors::SharedError) + Send + Sync>,
}

impl WarnAppender for funcWarnAppender {
    fn AppendWarning(&self, err: errors::SharedError) {
        (self.fn_)(WarnLevelWarning, err);
    }

    fn AppendNote(&self, err: errors::SharedError) {
        (self.fn_)(WarnLevelNote, err);
    }
}

// NewFuncWarnAppenderForTest creates a `WarnHandler` which will use the function to handle warn
/// 构造仅供测试的回调型 WarnAppender；生产路径不建议使用。
// To have a better performance, it's not suggested to use this function in production.
// NewFuncWarnAppenderForTest 仅供测试注入回调观察 warning；生产路径不建议用闭包避免额外分配。
pub fn NewFuncWarnAppenderForTest<F>(fn_: F) -> Box<dyn WarnAppender>
where
    F: Fn(&str, errors::SharedError) + Send + Sync + 'static,
{
    Box::new(funcWarnAppender { fn_: Box::new(fn_) })
}
