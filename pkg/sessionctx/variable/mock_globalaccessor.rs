// Copyright 2026 AsterSQL.
// Copyright 2018 PingCAP, Inc.
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

// 全局系统变量访问器（GlobalVarAccessor）的测试替身（Mock）。
//
// 生产路径通过访问器读写集群级/实例级系统变量；本模块用内存 HashMap 模拟，
// 区分「普通模式」（查外部注册表）与「测试套件模式」（内部表驱动并严格报错）。

use std::collections::HashMap;

/// Test implementation of the GlobalVarAccessor behavior from Go.
/// 对应 Go GlobalVarAccessor 的测试实现：`vals` 存变量值，`testSuite` 切换行为模式。
pub struct MockGlobalAccessor {
    vals: HashMap<String, String>,
    registered_defaults: HashMap<String, String>,
    testSuite: bool,
}

/// 构造普通模式 Mock：Get 时从调用方传入的 registered 表读取，未知变量返回空串。
pub fn NewMockGlobalAccessor() -> MockGlobalAccessor {
    MockGlobalAccessor {
        vals: HashMap::new(),
        registered_defaults: HashMap::new(),
        testSuite: false,
    }
}

/// The iterator is the Rust equivalent of Go's GetSysVars registry. Keeping it
/// explicit lets a task-local Cargo harness use the real registry contents.
/// 构造测试套件模式 Mock：用 defaults 初始化内部表，未知变量返回错误。
pub fn NewMockGlobalAccessor4Tests(
    defaults: impl IntoIterator<Item = (String, String)>,
) -> MockGlobalAccessor {
    let registered_defaults: HashMap<_, _> = defaults.into_iter().collect();
    MockGlobalAccessor {
        vals: registered_defaults.clone(),
        registered_defaults,
        testSuite: true,
    }
}

impl MockGlobalAccessor {
    /// 读取全局系统变量：普通模式查 registered，测试模式查内部 vals。
    pub fn GetGlobalSysVar(
        &self,
        name: &str,
        registered: Option<&HashMap<String, String>>,
    ) -> Result<String, String> {
        if !self.testSuite {
            // 普通模式：缺失键视为空值，对齐 Go 非测试路径。
            return Ok(registered
                .and_then(|values| values.get(name))
                .cloned()
                .unwrap_or_default());
        }
        self.vals
            .get(name)
            .cloned()
            .ok_or_else(|| format!("Unknown system variable '{name}'"))
    }

    /// 设置全局系统变量：先 validate 规范化，再执行 set_hook，最后写入 vals。
    pub fn SetGlobalSysVar(
        &mut self,
        name: &str,
        value: &str,
        validate: impl FnOnce(&str) -> Result<String, String>,
        mut set_hook: impl FnMut(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.vals.contains_key(name) {
            return Err(format!("Unknown system variable '{name}'"));
        }
        let normalized = validate(value)?;
        set_hook(&normalized)?;
        self.vals.insert(name.to_owned(), normalized);
        Ok(())
    }

    /// 设置实例级系统变量：校验并回调 hook，但不写回内部 vals（对齐 Go 语义）。
    pub fn SetInstanceSysVar(
        &self,
        name: &str,
        value: &str,
        validate: impl FnOnce(&str) -> Result<String, String>,
        set_hook: impl FnOnce(&str) -> Result<(), String>,
    ) -> Result<(), String> {
        if !self.vals.contains_key(name) {
            return Err(format!("Unknown system variable '{name}'"));
        }
        let normalized = validate(value)?;
        set_hook(&normalized)
    }

    /// 仅更新内部表中的全局值，跳过 validate/hook；`_skip_aliases` 保留 Go 签名。
    pub fn SetGlobalSysVarOnly(
        &mut self,
        name: &str,
        value: &str,
        _skip_aliases: bool,
    ) -> Result<(), String> {
        if !self.vals.contains_key(name) {
            return Err(format!("Unknown system variable '{name}'"));
        }
        self.vals.insert(name.to_owned(), value.to_owned());
        Ok(())
    }

    /// 读取 TiDB 系统表中的变量值；Mock 仅支持 `tikv_gc_life_time`（GC 生命周期）。
    pub fn GetTiDBTableValue(&self, name: &str) -> Result<String, String> {
        if name != "tikv_gc_life_time" {
            panic!("not supported");
        }
        self.registered_defaults
            .get(name)
            .cloned()
            .map(Ok)
            .unwrap_or_else(|| panic!("Get SysVar Failed"))
    }

    /// 写入 TiDB 系统表变量；当前 Mock 未实现，直接 panic。
    pub fn SetTiDBTableValue(
        &self,
        _name: &str,
        _value: &str,
        _comment: &str,
    ) -> Result<(), String> {
        panic!("not supported");
    }
}
