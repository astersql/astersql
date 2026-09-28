// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

//! Test case manager (Go `tests/llmtest/testcase/testcase.go`).

// 本文件对应 `tests/llmtest/testcase/testcase.rs`，本次任务只补中文解释，不改行为。
// 本文件承载实际测试逻辑或关键辅助逻辑。
// 中文注释围绕职责、约束和阶段展开。
// 阅读长函数时可按准备、执行、校验、清理四段理解。
// 与 Go 对齐的地方会强调不能随意删减的行为。
// 新增中文只解释现有行为，不改控制流。
// 长列表和常量区会补充它们被保留的原因。
use crate::stubs::{AnyValue, json};
use std::collections::HashMap;
use std::fs;
use std::io;
use std::sync::Mutex;

/// Case represents a test case.
///
/// Corresponds to Go `Case` with the same JSON field names.
#[derive(Clone, Debug, Default, PartialEq)]
// `Case` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct Case {
    pub sql: String,
    pub args: Option<Vec<AnyValue>>,
    pub pass: bool,
    pub known: bool,
    pub comment: String,
}

// 这里实现 `Case` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Case {
    // `to_json` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn to_json(&self) -> AnyValue {
        AnyValue::Object(vec![
            ("sql".to_string(), AnyValue::String(self.sql.clone())),
            (
                "args".to_string(),
                match &self.args {
                    None => AnyValue::Null,
                    Some(a) => AnyValue::Array(a.clone()),
                },
            ),
            ("pass".to_string(), AnyValue::Bool(self.pass)),
            ("known".to_string(), AnyValue::Bool(self.known)),
            (
                "comment".to_string(),
                AnyValue::String(self.comment.clone()),
            ),
        ])
    }

    // `from_json` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    fn from_json(v: &AnyValue) -> Result<Self, String> {
        let AnyValue::Object(fields) = v else {
            return Err("case must be object".to_string());
        };
        // encoding/json applies every matching object member in input order, so
        // duplicate fields keep the last value. Struct field matching also
        // accepts an ASCII case-insensitive match when decoding JSON tags.
        let get = |key: &str| -> Option<&AnyValue> {
            fields
                .iter()
                .rev()
                .find(|(k, _)| k.eq_ignore_ascii_case(key))
                .map(|(_, val)| val)
        };
        let sql = match get("sql") {
            Some(AnyValue::String(s)) => s.clone(),
            Some(AnyValue::Null) | None => String::new(),
            _ => return Err("sql must be string".to_string()),
        };
        let args = match get("args") {
            None | Some(AnyValue::Null) => None,
            Some(AnyValue::Array(a)) => Some(a.clone()),
            _ => return Err("args must be array or null".to_string()),
        };
        let pass = match get("pass") {
            Some(AnyValue::Bool(b)) => *b,
            Some(AnyValue::Null) | None => false,
            _ => return Err("pass must be bool".to_string()),
        };
        let known = match get("known") {
            Some(AnyValue::Bool(b)) => *b,
            Some(AnyValue::Null) | None => false,
            _ => return Err("known must be bool".to_string()),
        };
        let comment = match get("comment") {
            Some(AnyValue::String(s)) => s.clone(),
            Some(AnyValue::Null) | None => String::new(),
            _ => return Err("comment must be string".to_string()),
        };
        Ok(Self {
            sql,
            args,
            pass,
            known,
            comment,
        })
    }
}

// `ManagerInner` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
struct ManagerInner {
    path: String,
    cases: HashMap<String, Vec<Case>>,
}

/// Manager manages the test cases.
///
/// Go uses `sync.Mutex` around path and cases; Rust keeps the same method-level locking.
// `Manager` 承载这一层需要长期保存或暴露的状态。
// 字段通常只覆盖当前测试真正依赖的最小语义闭包。
pub struct Manager {
    inner: Mutex<ManagerInner>,
}

/// Open creates a new Manager on a given path.
///
/// Corresponds to Go `Open(path string)`.
// `open` 负责组织当前阶段的输入、状态或资源。
// 阅读时重点看它如何约束调用顺序和失败返回。
pub fn open(path: impl Into<String>) -> io::Result<Manager> {
    let manager = Manager {
        inner: Mutex::new(ManagerInner {
            path: path.into(),
            cases: HashMap::new(),
        }),
    };
    manager.load_from_file()?;
    Ok(manager)
}

// 这里实现 `Manager` 的行为方法和资源回收语义。
// 阅读这一段时，优先关注进入和离开方法时的状态变化。
impl Manager {
    /// Test / internal: construct without loading (empty cases).
    // `new_empty` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn new_empty(path: impl Into<String>) -> Self {
        Self {
            inner: Mutex::new(ManagerInner {
                path: path.into(),
                cases: HashMap::new(),
            }),
        }
    }

    /// loadFromFile loads the test cases from the file.
    // `load_from_file` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    fn load_from_file(&self) -> io::Result<()> {
        let mut g = self.inner.lock().expect("Go m.mu.Lock in loadFromFile");
        let bytes = fs::read(&g.path)?;
        let root =
            json::unmarshal(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        g.cases =
            cases_from_json(&root).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        Ok(())
    }

    /// Save saves the test cases to the file.
    ///
    /// Go uses `json.MarshalIndent(m.cases, "", "  ")` then `os.WriteFile(..., 0644)`.
    // `save` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    pub fn save(&self) -> io::Result<()> {
        let g = self.inner.lock().expect("Go m.mu.Lock in Save");
        let root = cases_to_json(&g.cases);
        let bytes = json::marshal_indent(&root).into_bytes();
        // Go WriteFile mode 0644 (before umask) on create.
        write_file_0644(&g.path, &bytes)?;
        Ok(())
    }

    /// AppendCase appends a test case to the manager.
    // `append_case` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    pub fn append_case(&self, group: impl Into<String>, c: Case) {
        let mut g = self.inner.lock().expect("Go m.mu.Lock in AppendCase");
        g.cases.entry(group.into()).or_default().push(c);
    }

    /// ExistCases returns the test cases in a group.
    ///
    /// Go returns the slice header under the map; Rust clones to keep the lock scoped.
    // `exist_cases` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn exist_cases(&self, group: &str) -> Vec<Case> {
        let g = self.inner.lock().expect("Go m.mu.Lock in ExistCases");
        g.cases.get(group).cloned().unwrap_or_default()
    }

    /// AllGroups returns all the groups.
    ///
    /// Go map iteration order is undefined; Rust HashMap order is likewise unstable.
    // `all_groups` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn all_groups(&self) -> Vec<String> {
        let g = self.inner.lock().expect("Go m.mu.Lock in AllGroups");
        g.cases.keys().cloned().collect()
    }

    /// Path of the backing file (for tests).
    // `path` 承担当前文件中的一段辅助职责或状态转换。
    // 中文注释会提示它依赖哪些前置条件。
    pub fn path(&self) -> String {
        self.inner.lock().expect("Go m.mu.Lock").path.clone()
    }

    /// Mutable access used by `run_ab_test` (holds the same mutex as Go `m.mu`).
    // `with_cases_mut` 负责组织当前阶段的输入、状态或资源。
    // 阅读时重点看它如何约束调用顺序和失败返回。
    pub(crate) fn with_cases_mut<R>(
        &self,
        f: impl FnOnce(&mut HashMap<String, Vec<Case>>) -> R,
    ) -> R {
        let mut g = self.inner.lock().expect("Go m.mu.Lock in RunABTest");
        f(&mut g.cases)
    }
}

// `cases_to_json` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn cases_to_json(cases: &HashMap<String, Vec<Case>>) -> AnyValue {
    let mut fields = Vec::with_capacity(cases.len());
    for (group, list) in cases {
        let arr = list.iter().map(Case::to_json).collect::<Vec<_>>();
        fields.push((group.clone(), AnyValue::Array(arr)));
    }
    AnyValue::Object(fields)
}

// `cases_from_json` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn cases_from_json(root: &AnyValue) -> Result<HashMap<String, Vec<Case>>, String> {
    let AnyValue::Object(fields) = root else {
        return Err("root must be object".to_string());
    };
    let mut out = HashMap::new();
    for (group, val) in fields {
        let AnyValue::Array(items) = val else {
            return Err(format!("group {group} must be array"));
        };
        let mut list = Vec::with_capacity(items.len());
        for item in items {
            list.push(Case::from_json(item)?);
        }
        out.insert(group.clone(), list);
    }
    Ok(out)
}

// `write_file_0644` 承担当前文件中的一段辅助职责或状态转换。
// 中文注释会提示它依赖哪些前置条件。
fn write_file_0644(path: &str, bytes: &[u8]) -> io::Result<()> {
    use std::fs::OpenOptions;
    use std::io::Write;

    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o644);
    }
    let mut f = opts.open(path)?;
    f.write_all(bytes)?;
    Ok(())
}
