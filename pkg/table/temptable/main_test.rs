// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// `temptable` 测试夹具（对应 Go TestMain / mocked retriever）。
//
// 提供 MockedInfoSchema、MockedRetriever、MockedSnapshot 等，
// 供拦截器测试记录方法调用、注入错误并模拟 Snapshot（一致性读视图）。

use std::any::Any;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::{
    CiString, InfoSchema, Key, KvIterator, Retriever, SchemaState, Snapshot, Table, TableInfo,
    TempTableError, TempTableType, ValueEntry,
};

#[derive(Default)]
/// 按 table_id 记录 TempTableType 的轻量 InfoSchema。
pub struct MockedInfoSchema {
    tables: Mutex<HashMap<i64, TempTableType>>,
}

/// 空 MockedInfoSchema。
pub fn new_mocked_info_schema() -> Arc<MockedInfoSchema> {
    Arc::new(MockedInfoSchema::default())
}

impl MockedInfoSchema {
    /// 批量为给定 ID 设置临时表类型并链式返回。
    pub fn add_table(self: &Arc<Self>, temp_type: TempTableType, ids: &[i64]) -> Arc<Self> {
        {
            let mut tables = self.tables.lock().unwrap();
            for id in ids {
                tables.insert(*id, temp_type);
            }
        }
        Arc::clone(self)
    }
}

impl InfoSchema for MockedInfoSchema {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn table_by_id(&self, tbl_id: i64) -> Option<Arc<Table>> {
        let temp_type = *self.tables.lock().unwrap().get(&tbl_id)?;
        let info = TableInfo {
            id: tbl_id,
            name: CiString::new(format!("tb{tbl_id}")),
            temp_table_type: temp_type,
            state: SchemaState::Public,
            ..TableInfo::default()
        };
        Some(Arc::new(Table::from_metadata(info)))
    }

    fn has_temporary_table(&self) -> bool {
        self.tables
            .lock()
            .unwrap()
            .values()
            .any(|temp_type| *temp_type != TempTableType::None)
    }
}

/// 模拟提交时间戳（CommitTS），用于 Snapshot Get/BatchGet。
pub const MOCK_COMMIT_TS: u64 = 1024;

#[derive(Clone)]
/// 一次被记录的 Retriever/Snapshot 方法调用。
pub struct MethodInvoke {
    pub method: String,
    pub args: Vec<InvokeArg>,
    pub ret_err: Option<String>,
    pub ret_iter: Option<Arc<Mutex<MockedIter>>>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
/// 调用参数：单键或键列表。
pub enum InvokeArg {
    Key(Key),
    Keys(Vec<Key>),
}

/// 可注入 next 错误的内存迭代器状态。
pub struct MockedIter {
    data: Vec<(Key, ValueEntry)>,
    position: usize,
    closed: bool,
    next_err: Option<TempTableError>,
}

impl MockedIter {
    fn new(data: Vec<(Key, ValueEntry)>) -> Arc<Mutex<Self>> {
        Arc::new(Mutex::new(Self {
            data,
            position: 0,
            closed: false,
            next_err: None,
        }))
    }

    /// 注入下一次 `next` 返回的错误。
    pub fn inject_next_error(&mut self, err: TempTableError) {
        self.next_err = Some(err);
    }

    pub fn closed(&self) -> bool {
        self.closed
    }
}

/// KvIterator 句柄：缓存当前键值并驱动 MockedIter。
pub struct MockedIterHandle {
    inner: Arc<Mutex<MockedIter>>,
    current_key: Key,
    current_value: ValueEntry,
}

impl MockedIterHandle {
    fn new(inner: Arc<Mutex<MockedIter>>) -> Box<Self> {
        let mut iter = Box::new(Self {
            inner,
            current_key: Vec::new(),
            current_value: ValueEntry::default(),
        });
        iter.sync_current();
        iter
    }

    fn sync_current(&mut self) {
        let guard = self.inner.lock().unwrap();
        if !guard.closed && guard.position < guard.data.len() {
            self.current_key = guard.data[guard.position].0.clone();
            self.current_value = guard.data[guard.position].1.clone();
        }
    }
}

impl KvIterator for MockedIterHandle {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn valid(&self) -> bool {
        let guard = self.inner.lock().unwrap();
        !guard.closed && guard.position < guard.data.len()
    }

    fn key(&self) -> &[u8] {
        &self.current_key
    }

    fn value(&self) -> &ValueEntry {
        &self.current_value
    }

    fn next(&mut self) -> Result<(), TempTableError> {
        {
            let mut guard = self.inner.lock().unwrap();
            if let Some(err) = guard.next_err.clone() {
                return Err(err);
            }
            if !guard.closed && guard.position < guard.data.len() {
                guard.position += 1;
            }
        }
        self.sync_current();
        Ok(())
    }

    fn close(&mut self) {
        self.inner.lock().unwrap().closed = true;
    }

    fn closed(&self) -> bool {
        self.inner.lock().unwrap().closed
    }
}

/// 可配置允许方法、错误注入与调用日志的 Retriever（键值读取抽象）。
pub struct MockedRetriever {
    data: Mutex<Vec<(Key, Vec<u8>)>>,
    data_map: Mutex<HashMap<Key, Vec<u8>>>,
    commit_ts: Mutex<u64>,
    invokes: Mutex<Vec<MethodInvoke>>,
    allow_invokes: Mutex<Option<HashMap<String, ()>>>,
    error_map: Mutex<HashMap<String, TempTableError>>,
    return_commit_ts: Mutex<bool>,
}

/// 空 MockedRetriever。
pub fn new_mocked_retriever() -> Arc<MockedRetriever> {
    Arc::new(MockedRetriever {
        data: Mutex::new(Vec::new()),
        data_map: Mutex::new(HashMap::new()),
        commit_ts: Mutex::new(0),
        invokes: Mutex::new(Vec::new()),
        allow_invokes: Mutex::new(None),
        error_map: Mutex::new(HashMap::new()),
        return_commit_ts: Mutex::new(false),
    })
}

impl MockedRetriever {
    /// `None` value means Go `nil` / empty bytes stored under the key.
    /// 设置有序键值；`None` 值表示 Go 侧 nil/空字节。
    pub fn set_data(self: &Arc<Self>, data: Vec<(Key, Option<Vec<u8>>)>) -> Arc<Self> {
        let mut sorted: Vec<(Key, Vec<u8>)> = data
            .into_iter()
            .map(|(k, v)| (k, v.unwrap_or_default()))
            .collect();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        let mut data_map = HashMap::new();
        for (key, value) in &sorted {
            data_map.insert(key.clone(), value.clone());
        }
        *self.data.lock().unwrap() = sorted;
        *self.data_map.lock().unwrap() = data_map;
        Arc::clone(self)
    }

    /// 注入或清除某方法名的错误。
    pub fn inject_method_error(
        self: &Arc<Self>,
        method: &str,
        err: Option<TempTableError>,
    ) -> Arc<Self> {
        let mut error_map = self.error_map.lock().unwrap();
        match err {
            Some(error) => {
                error_map.insert(method.to_string(), error);
            }
            None => {
                error_map.remove(method);
            }
        }
        Arc::clone(self)
    }

    /// 白名单：仅允许列出的方法被调用。
    pub fn set_allowed_method(self: &Arc<Self>, methods: &[&str]) -> Arc<Self> {
        let mut allow = HashMap::new();
        for method in methods {
            allow.insert((*method).to_string(), ());
        }
        *self.allow_invokes.lock().unwrap() = Some(allow);
        Arc::clone(self)
    }

    /// 清空调用日志。
    pub fn reset_invokes(&self) {
        self.invokes.lock().unwrap().clear();
    }

    /// 返回已记录调用。
    pub fn get_invokes(&self) -> Vec<MethodInvoke> {
        self.invokes.lock().unwrap().clone()
    }

    /// Get 是否附带 commit_ts。
    pub fn set_return_commit_ts(&self, enabled: bool) {
        *self.return_commit_ts.lock().unwrap() = enabled;
    }

    /// 设置模拟 commit_ts。
    pub fn set_commit_ts(&self, commit_ts: u64) {
        *self.commit_ts.lock().unwrap() = commit_ts;
    }

    /// 未在白名单则 panic，模拟 Go 严格断言。
    fn check_method_invoke_allowed(&self, method: &str) {
        let allow = self.allow_invokes.lock().unwrap();
        let Some(allow) = allow.as_ref() else {
            panic!("Invoke for '{method}' is not allowed, should allow it first");
        };
        assert!(
            allow.contains_key(method),
            "Invoke for '{method}' is not allowed, should allow it first"
        );
    }

    /// 查询方法错误注入。
    fn get_method_err(&self, method: &str) -> Option<TempTableError> {
        self.error_map.lock().unwrap().get(method).cloned()
    }

    /// 追加一条调用记录。
    fn append_invoke(
        &self,
        method: &str,
        args: Vec<InvokeArg>,
        ret_err: Option<String>,
        ret_iter: Option<Arc<Mutex<MockedIter>>>,
    ) {
        self.invokes.lock().unwrap().push(MethodInvoke {
            method: method.to_string(),
            args,
            ret_err,
            ret_iter,
        });
    }

    /// 按开关返回 commit_ts 或 0。
    fn commit_ts_for_get(&self) -> u64 {
        if *self.return_commit_ts.lock().unwrap() {
            *self.commit_ts.lock().unwrap()
        } else {
            0
        }
    }
}

impl Retriever for MockedRetriever {
    fn get(&self, key: &[u8]) -> Result<ValueEntry, TempTableError> {
        self.check_method_invoke_allowed("Get");
        let result = if let Some(err) = self.get_method_err("Get") {
            Err(err)
        } else {
            match self.data_map.lock().unwrap().get(key) {
                // Present (even empty/nil) => Ok entry, matching Go mockedRetriever.Get.
                Some(value) => Ok(ValueEntry::new(value.clone(), self.commit_ts_for_get())),
                None => Err(TempTableError::KeyNotExist),
            }
        };
        let ret_err = result.as_ref().err().map(std::string::ToString::to_string);
        self.append_invoke("Get", vec![InvokeArg::Key(key.to_vec())], ret_err, None);
        result
    }

    fn iter(&self, start: &[u8], end: &[u8]) -> Result<Box<dyn KvIterator>, TempTableError> {
        self.check_method_invoke_allowed("Iter");
        if let Some(err) = self.get_method_err("Iter") {
            self.append_invoke(
                "Iter",
                vec![InvokeArg::Key(start.to_vec()), InvokeArg::Key(end.to_vec())],
                Some(err.to_string()),
                None,
            );
            return Err(err);
        }
        let mut data = Vec::new();
        for (key, value) in self.data.lock().unwrap().iter() {
            if key.as_slice() >= start && (end.is_empty() || key.as_slice() < end) {
                data.push((key.clone(), ValueEntry::new(value.clone(), 0)));
            }
        }
        let mocked = MockedIter::new(data);
        if let Some(next_err) = self.get_method_err("IterNext") {
            mocked.lock().unwrap().inject_next_error(next_err);
        }
        self.append_invoke(
            "Iter",
            vec![InvokeArg::Key(start.to_vec()), InvokeArg::Key(end.to_vec())],
            None,
            Some(Arc::clone(&mocked)),
        );
        Ok(MockedIterHandle::new(mocked))
    }

    fn iter_reverse(
        &self,
        end: &[u8],
        start: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        self.check_method_invoke_allowed("IterReverse");
        if let Some(err) = self.get_method_err("IterReverse") {
            self.append_invoke(
                "IterReverse",
                vec![InvokeArg::Key(end.to_vec())],
                Some(err.to_string()),
                None,
            );
            return Err(err);
        }
        let data_guard = self.data.lock().unwrap();
        let mut data = Vec::new();
        for (key, value) in data_guard.iter().rev() {
            if (end.is_empty() || key.as_slice() < end)
                && (start.is_empty() || key.as_slice() >= start)
            {
                data.push((key.clone(), ValueEntry::new(value.clone(), 0)));
            }
        }
        drop(data_guard);
        let mocked = MockedIter::new(data);
        if let Some(next_err) = self.get_method_err("IterReverseNext") {
            mocked.lock().unwrap().inject_next_error(next_err);
        }
        self.append_invoke(
            "IterReverse",
            vec![InvokeArg::Key(end.to_vec())],
            None,
            Some(Arc::clone(&mocked)),
        );
        Ok(MockedIterHandle::new(mocked))
    }
}

/// 包装 MockedRetriever 的 Snapshot，支持 BatchGet。
pub struct MockedSnapshot {
    pub retriever: Arc<MockedRetriever>,
}

/// 构造 Snapshot 并设置 MOCK_COMMIT_TS。
pub fn new_mocked_snapshot(retriever: Arc<MockedRetriever>) -> Arc<MockedSnapshot> {
    retriever.set_commit_ts(MOCK_COMMIT_TS);
    Arc::new(MockedSnapshot { retriever })
}

impl MockedSnapshot {
    pub fn set_allowed_method(self: &Arc<Self>, methods: &[&str]) -> Arc<Self> {
        self.retriever.set_allowed_method(methods);
        Arc::clone(self)
    }

    pub fn inject_method_error(
        self: &Arc<Self>,
        method: &str,
        err: Option<TempTableError>,
    ) -> Arc<Self> {
        self.retriever.inject_method_error(method, err);
        Arc::clone(self)
    }

    pub fn reset_invokes(&self) {
        self.retriever.reset_invokes();
    }

    pub fn get_invokes(&self) -> Vec<MethodInvoke> {
        self.retriever.get_invokes()
    }

    pub fn set_return_commit_ts(&self, enabled: bool) {
        self.retriever.set_return_commit_ts(enabled);
    }
}

impl Retriever for MockedSnapshot {
    fn get(&self, key: &[u8]) -> Result<ValueEntry, TempTableError> {
        self.retriever.get(key)
    }

    fn iter(&self, start: &[u8], end: &[u8]) -> Result<Box<dyn KvIterator>, TempTableError> {
        self.retriever.iter(start, end)
    }

    fn iter_reverse(
        &self,
        end: &[u8],
        start: &[u8],
    ) -> Result<Box<dyn KvIterator>, TempTableError> {
        self.retriever.iter_reverse(end, start)
    }
}

impl Snapshot for MockedSnapshot {
    fn batch_get(&self, keys: &[Key]) -> Result<HashMap<Key, ValueEntry>, TempTableError> {
        self.retriever.check_method_invoke_allowed("BatchGet");
        let result = if let Some(err) = self.retriever.get_method_err("BatchGet") {
            Err(err)
        } else {
            let commit_ts = self.retriever.commit_ts_for_get();
            let mut data = HashMap::new();
            let data_map = self.retriever.data_map.lock().unwrap();
            for key in keys {
                if let Some(value) = data_map.get(key) {
                    data.insert(key.clone(), ValueEntry::new(value.clone(), commit_ts));
                }
            }
            Ok(data)
        };
        let ret_err = result.as_ref().err().map(std::string::ToString::to_string);
        self.retriever.append_invoke(
            "BatchGet",
            vec![InvokeArg::Keys(keys.to_vec())],
            ret_err,
            None,
        );
        result
    }
}
