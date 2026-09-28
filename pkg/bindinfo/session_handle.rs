// Copyright 2026 AsterSQL.
// Copyright 2019 PingCAP, Inc.
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

// 会话级 SQL 绑定（Session Binding）管理模块。
//
// SQL 绑定（SQL Binding / SQL Plan Binding）是数据库中一种“执行计划绑定”机制：
// 通过把优化器提示（Hint）与某类 SQL 语句（以 SQL 摘要 Digest 标识）关联起来，
// 强制优化器为该类语句生成指定的执行计划，从而在不修改业务 SQL 的前提下稳定
// 查询性能。绑定按作用域分为全局绑定（对所有会话生效）与会话绑定（仅对创建
// 它的当前会话生效，会话结束即失效）。
//
// 本模块实现会话级绑定的处理器（Handle），职责包括：
// - 在当前会话内创建、删除、匹配会话绑定；
// - 使用绑定缓存（`BindingCache`）按 SQL 摘要存取绑定；
// - 在会话状态迁移（如连接在多个计算节点间转移）时，将会话绑定序列化进
//   `SessionStates` 并在目标端反序列化恢复，同时兼容旧版编码格式。

use crate::{
    Binding, BindingCache, BindingValidator, Result, TableName, newBindingCache, prepareHints,
};
use serde::{Deserialize, Serialize};
use std::sync::{Arc, RwLock};

/// 会话状态的可序列化载体。
///
/// 用于会话迁移场景：把会话内的各类状态（此处仅关注会话绑定）编码为
/// 字符串字段，传输到目标节点后再解码恢复。
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SessionStates {
    /// JSON 编码的会话绑定，与 Go `sessionstates.SessionStates.Bindings` 对齐。
    #[serde(rename = "bindings", skip_serializing_if = "String::is_empty")]
    pub Bindings: String,
}

/// 会话级绑定处理器接口，定义会话绑定的完整生命周期操作。
///
/// 每个会话持有一个实现该 trait 的处理器，所有操作只影响当前会话。
pub trait SessionBindingHandle: Send + Sync {
    /// 创建会话绑定：校验并准备各绑定的 Hint 后写入会话缓存。
    /// 同一 SQL 摘要（Digest，SQL 归一化后的哈希指纹）的旧绑定会被覆盖。
    fn CreateSessionBinding(
        &self,
        sctx: &dyn BindingValidator,
        bindings: Vec<Binding>,
    ) -> Result<()>;
    /// 按 SQL 摘要列表删除对应的会话绑定。
    fn DropSessionBinding(&self, sqlDigests: &[String]) -> Result<()>;
    /// 依据当前数据库、去库名摘要（noDBDigest）与语句涉及的表名，
    /// 匹配可用的会话绑定；返回匹配结果及是否命中的标志。
    fn MatchSessionBinding(
        &self,
        current_db: &str,
        noDBDigest: &str,
        tableNames: &[TableName],
    ) -> (Option<Arc<Binding>>, bool);
    /// 返回当前会话中的全部绑定。
    fn GetAllSessionBindings(&self) -> Vec<Arc<Binding>>;
    /// 将全部会话绑定序列化写入 `SessionStates`，用于会话迁移导出。
    fn EncodeSessionStates(&self, sessionStates: &mut SessionStates) -> Result<()>;
    /// 从 `SessionStates` 反序列化并恢复会话绑定，用于会话迁移导入。
    fn DecodeSessionStates(
        &self,
        sctx: &dyn BindingValidator,
        sessionStates: &SessionStates,
    ) -> Result<()>;
    /// 关闭处理器并释放底层缓存资源。
    fn Close(&self);
}

/// `SessionBindingHandle` 的默认实现，内部用绑定缓存按 SQL 摘要存取绑定。
pub struct sessionBindingHandle {
    /// 会话私有的绑定缓存，键为 SQL 摘要。
    cache: Arc<dyn BindingCache>,
    /// 保证批量创建/删除与读取之间具有和 Go `sync.RWMutex` 相同的原子性。
    operation_lock: RwLock<()>,
}

/// 创建一个会话绑定处理器。与 Go 的 map 一样，会话绑定不受全局缓存配额限制。
pub fn NewSessionBindingHandle() -> Arc<dyn SessionBindingHandle> {
    Arc::new(sessionBindingHandle {
        cache: newBindingCache(i64::MAX),
        operation_lock: RwLock::new(()),
    })
}

impl SessionBindingHandle for sessionBindingHandle {
    fn CreateSessionBinding(
        &self,
        sctx: &dyn BindingValidator,
        mut bindings: Vec<Binding>,
    ) -> Result<()> {
        // 先整体校验并解析/规范化绑定携带的 Hint；任何一条失败则整批放弃。
        for binding in &mut bindings {
            prepareHints(sctx, binding)?;
        }
        let _guard = self
            .operation_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for mut binding in bindings {
            binding.Db = binding.Db.to_lowercase();
            let now = crate::BindingTime::now();
            binding.CreateTime = now;
            binding.UpdateTime = now;
            let digest = astersql_parser::DigestNormalized(&binding.OriginalSQL)
                .String()
                .to_owned();
            self.cache.SetBinding(digest, Arc::new(binding))?;
        }
        Ok(())
    }

    fn DropSessionBinding(&self, sqlDigests: &[String]) -> Result<()> {
        let _guard = self
            .operation_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for digest in sqlDigests {
            self.cache.RemoveBinding(digest);
        }
        Ok(())
    }

    fn MatchSessionBinding(
        &self,
        current_db: &str,
        noDBDigest: &str,
        tableNames: &[TableName],
    ) -> (Option<Arc<Binding>>, bool) {
        let _guard = self
            .operation_lock
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.cache
            .MatchingBinding(current_db, noDBDigest, tableNames)
    }

    fn GetAllSessionBindings(&self) -> Vec<Arc<Binding>> {
        let _guard = self
            .operation_lock
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.cache.GetAllBindings()
    }

    fn EncodeSessionStates(&self, sessionStates: &mut SessionStates) -> Result<()> {
        let bindings: Vec<_> = self
            .GetAllSessionBindings()
            .into_iter()
            .map(|binding| (*binding).clone())
            .collect();
        if bindings.is_empty() {
            return Ok(());
        }
        sessionStates.Bindings = serde_json::to_string(&bindings)?;
        Ok(())
    }

    fn DecodeSessionStates(
        &self,
        sctx: &dyn BindingValidator,
        sessionStates: &SessionStates,
    ) -> Result<()> {
        if sessionStates.Bindings.is_empty() {
            return Ok(());
        };

        let values: Vec<serde_json::Value> = serde_json::from_str(&sessionStates.Bindings)?;
        if values.is_empty() {
            return Ok(());
        }
        let mut bindings = if values[0].get("Bindings").is_some() {
            self.decodeOldStyleSessionStates(sessionStates.Bindings.as_bytes())?
        } else {
            serde_json::from_value::<Vec<Binding>>(serde_json::Value::Array(values))?
        };
        for binding in &mut bindings {
            prepareHints(sctx, binding)?;
        }
        let _guard = self
            .operation_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for binding in bindings {
            let digest = astersql_parser::DigestNormalized(&binding.OriginalSQL)
                .String()
                .to_owned();
            self.cache.SetBinding(digest, Arc::new(binding))?;
        }
        Ok(())
    }

    fn Close(&self) {
        let _guard = self
            .operation_lock
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        self.cache.Close();
    }
}

impl sessionBindingHandle {
    /// 解析旧版格式的会话绑定编码。
    ///
    /// 旧版本把记录编码成数组，每条记录持有原始 SQL、DB 和绑定列表。
    /// 展开时把外层的 SQL 与 DB 恢复到每一条绑定上。
    fn decodeOldStyleSessionStates(&self, bindingBytes: &[u8]) -> Result<Vec<Binding>> {
        #[derive(Deserialize)]
        struct BindRecord {
            OriginalSQL: String,
            Db: String,
            Bindings: Vec<Binding>,
        }

        let records: Vec<BindRecord> = serde_json::from_slice(bindingBytes)?;
        let mut bindings = Vec::with_capacity(records.len());
        for record in records {
            for mut binding in record.Bindings {
                binding.OriginalSQL = record.OriginalSQL.clone();
                binding.Db = record.Db.clone();
                bindings.push(binding);
            }
        }
        Ok(bindings)
    }
}

/// 会话绑定信息在会话上下文键值空间中的键类型。
pub type sessionBindInfoKeyType = i32;

/// 返回该键类型的字符串表示（对应 Go 版本的 `String()` 方法）。
pub fn sessionBindInfoKeyType_String(_: sessionBindInfoKeyType) -> String {
    "session_bindinfo".to_owned()
}

/// 同上，返回键类型的字符串表示；保留以匹配原始接口形式。
pub fn String(_: sessionBindInfoKeyType) -> String {
    "session_bindinfo".to_owned()
}

/// 用于在会话上下文中定位会话绑定处理器的固定键值。
pub const SessionBindInfoKeyType: sessionBindInfoKeyType = 0;
