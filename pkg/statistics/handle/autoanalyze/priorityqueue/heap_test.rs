// Copyright 2026 AsterSQL.
// Copyright 2017 The Kubernetes Authors.
// Copyright 2024 PingCAP, Inc.
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
// Modifications:
// 1. Use "github.com/stretchr/testify/require" to do assertions.
// 2. Test max heap instead of min heap.
// 3. Add a test for the peak API.
// 4. Add a test for the IsEmpty API.
// 5. Remove concurrency and thread-safety tests.
// 6. Add a test for the Len API.
// 7. Remove the BulkAdd related tests.

// 优先队列堆（PqHeapImpl / NewHeap）的单元测试。
//
// 使用轻量 `TestHeapObject` 模拟 AnalysisJob：table_id 作堆 key，val 作权重，
// 覆盖 AddOrUpdate、Delete、Update、Peek/Pop、List 及空堆错误等 API。

use std::any::Any;
use std::collections::HashMap;
use std::fmt;

use crate::{
    AnalysisJob, AnalysisJobJSON, AnalysisRuntime, ERR_HEAP_IS_EMPTY, FailureJobHook, Indicators,
    NewHeap, SuccessJobHook,
};

// testHeapObject 对应 Go 测试对象，tableID 用作堆 key，val 用作权重。
/// 测试用分析作业：仅实现 GetWeight / GetTableID，其余方法 panic。
#[derive(Clone, Debug)]
struct TestHeapObject {
    /// 堆 key（表 ID）。
    table_id: i64,
    /// 权重，越大越优先出堆。
    val: f64,
}

impl fmt::Display for TestHeapObject {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        panic!("implement me")
    }
}

impl AnalysisJob for TestHeapObject {
    fn ValidateAndPrepare(&mut self, _runtime: &dyn AnalysisRuntime) -> (bool, String) {
        panic!("implement me")
    }
    fn Analyze(&mut self, _runtime: &dyn AnalysisRuntime) -> Result<(), String> {
        panic!("implement me")
    }
    fn SetWeight(&mut self, _weight: f64) {
        panic!("implement me")
    }
    fn GetWeight(&self) -> f64 {
        self.val
    }
    fn HasNewlyAddedIndex(&self) -> bool {
        panic!("implement me")
    }
    fn GetIndicators(&self) -> Indicators {
        panic!("implement me")
    }
    fn SetIndicators(&mut self, _indicators: Indicators) {
        panic!("implement me")
    }
    fn GetTableID(&self) -> i64 {
        self.table_id
    }
    fn RegisterSuccessHook(&mut self, _hook: SuccessJobHook) {
        panic!("implement me")
    }
    fn RegisterFailureHook(&mut self, _hook: FailureJobHook) {
        panic!("implement me")
    }
    fn AsJSON(&self) -> AnalysisJobJSON {
        panic!("implement me")
    }
    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// 构造装箱后的测试作业。
fn mk_heap_obj(table_id: i64, val: f64) -> Box<dyn AnalysisJob> {
    Box::new(TestHeapObject { table_id, val })
}

/// 验证插入、同 key 更新权重、删除后 Pop 顺序符合最大堆。
#[test]
fn TestHeap_AddOrUpdate() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 11.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 30.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(1, 13.0)).unwrap(); // This updates object with tableID 1.

    let item = h.Pop().unwrap();
    assert_eq!(4, item.GetTableID());

    let item = h.Pop().unwrap();
    assert_eq!(1, item.GetTableID());

    h.Delete(mk_heap_obj(3, 11.0).as_ref()).unwrap(); // Deletes object with tableID 3.
    h.AddOrUpdate(mk_heap_obj(1, 14.0)).unwrap(); // Updates object with tableID 1.

    let item = h.Pop().unwrap();
    assert_eq!(1, item.GetTableID());

    let item = h.Pop().unwrap();
    assert_eq!(2, item.GetTableID());
}

/// 空堆 Pop 应返回 ERR_HEAP_IS_EMPTY。
#[test]
fn TestHeapEmptyPop() {
    let mut h = NewHeap();
    let err = h.Pop().err().unwrap();
    assert_eq!(ERR_HEAP_IS_EMPTY, err);
}

/// 验证 Delete 后堆顶与后续 Pop 顺序。
#[test]
fn TestHeap_Delete() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 31.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 11.0)).unwrap();

    h.Delete(mk_heap_obj(3, 31.0).as_ref()).unwrap();

    let item = h.Pop().unwrap();
    assert_eq!(4, item.GetTableID());

    h.AddOrUpdate(mk_heap_obj(5, 30.0)).unwrap();
    h.Delete(mk_heap_obj(2, 1.0).as_ref()).unwrap();

    let item = h.Pop().unwrap();
    assert_eq!(5, item.GetTableID());

    let item = h.Pop().unwrap();
    assert_eq!(1, item.GetTableID());
    assert_eq!(0, h.Len());
}

/// 删除不存在的对象时，错误契约与 Go `delete` 保持一致，且堆状态不变。
#[test]
fn delete_missing_object_matches_go_error_and_preserves_heap() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();

    let err = h.Delete(mk_heap_obj(2, 1.0).as_ref()).err().unwrap();

    assert_eq!("object not found", err);
    assert_eq!(1, h.Len());
    assert_eq!(1, h.Peek().unwrap().GetTableID());
}

/// Update 提升权重后 Peek 应指向新堆顶。
#[test]
fn TestHeap_Update() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 31.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 11.0)).unwrap();

    h.Update(mk_heap_obj(4, 50.0)).unwrap();
    assert_eq!(4, h.Peek().unwrap().GetTableID());

    let item = h.Pop().unwrap();
    assert_eq!(4, item.GetTableID());

    h.Update(mk_heap_obj(2, 100.0)).unwrap();
    assert_eq!(2, h.Peek().unwrap().GetTableID());
}

/// Get 按作业表 ID 查找，不存在返回 None。
#[test]
fn TestHeap_Get() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 31.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 11.0)).unwrap();

    let found = h.Get(mk_heap_obj(4, 0.0).as_ref());
    assert!(found.is_some());
    assert_eq!(4, found.unwrap().GetTableID());

    assert!(h.Get(mk_heap_obj(5, 0.0).as_ref()).is_none());
}

/// GetByKey 按表 ID 直接查找。
#[test]
fn TestHeap_GetByKey() {
    let mut h = NewHeap();
    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 31.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 11.0)).unwrap();

    let found = h.GetByKey(4);
    assert!(found.is_some());
    assert_eq!(4, found.unwrap().GetTableID());

    assert!(h.GetByKey(5).is_none());
}

/// List 返回全部作业，且权重与插入一致。
#[test]
fn TestHeap_List() {
    let mut h = NewHeap();
    assert!(h.List().is_empty());

    let items = HashMap::from([(1, 10.0), (2, 1.0), (3, 30.0), (4, 11.0), (5, 30.0)]);
    for (k, v) in &items {
        h.AddOrUpdate(mk_heap_obj(*k, *v)).unwrap();
    }

    let list = h.List();
    assert_eq!(items.len(), list.len());
    for obj in list {
        assert_eq!(items[&obj.GetTableID()], obj.GetWeight());
    }
}

/// ListKeys 返回全部表 ID。
#[test]
fn TestHeap_ListKeys() {
    let mut h = NewHeap();
    assert!(h.ListKeys().is_empty());

    let items = HashMap::from([(1, 10.0), (2, 1.0), (3, 30.0), (4, 11.0), (5, 30.0)]);
    for (k, v) in &items {
        h.AddOrUpdate(mk_heap_obj(*k, *v)).unwrap();
    }

    let list = h.ListKeys();
    assert_eq!(items.len(), list.len());
    for key in list {
        assert!(items.contains_key(&key));
    }
}

/// Peek 不移除元素；空堆报错，非空时返回权重最大者。
#[test]
fn TestHeap_Peek() {
    let mut h = NewHeap();
    let err = h.Peek().err().unwrap();
    assert_eq!(ERR_HEAP_IS_EMPTY, err);

    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(2, 1.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(3, 31.0)).unwrap();
    h.AddOrUpdate(mk_heap_obj(4, 11.0)).unwrap();

    let item = h.Peek().unwrap();
    assert_eq!(3, item.GetTableID());

    let item = h.Pop().unwrap();
    assert_eq!(3, item.GetTableID());
}

/// IsEmpty 随插入/弹出变化。
#[test]
fn TestHeap_IsEmpty() {
    let mut h = NewHeap();
    assert!(h.IsEmpty());

    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    assert!(!h.IsEmpty());

    h.Pop().unwrap();
    assert!(h.IsEmpty());
}

/// Len 反映堆大小。
#[test]
fn TestHeap_Len() {
    let mut h = NewHeap();
    assert_eq!(0, h.Len());

    h.AddOrUpdate(mk_heap_obj(1, 10.0)).unwrap();
    assert_eq!(1, h.Len());

    h.Pop().unwrap();
    assert_eq!(0, h.Len());
}

/// 空堆时 Peek/Pop 错误文案与 ListKeys 空状态保持稳定。
#[test]
fn empty_heap_reports_canonical_error_and_stable_state() {
    let mut heap = NewHeap();
    assert!(heap.IsEmpty());
    assert_eq!(0, heap.Len());
    assert_eq!(ERR_HEAP_IS_EMPTY, heap.Peek().err().unwrap());
    assert_eq!(ERR_HEAP_IS_EMPTY, heap.Pop().err().unwrap());
    assert!(heap.ListKeys().is_empty());
}
