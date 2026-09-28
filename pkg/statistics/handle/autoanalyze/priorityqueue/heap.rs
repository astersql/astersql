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

// 自动 ANALYZE 优先队列堆实现。
//
// 以表 ID 为 key，按作业权重（weight）构成最大堆：权重越高越先被 Peek/Pop。
// 对应 Go priorityqueue 中的 heap，供调度器按优先级挑选待分析作业。

use crate::job::AnalysisJob;
use std::collections::HashMap;

/// 堆为空时 Peek/Pop 返回的错误文案（与 Go 侧一致）。
pub const ERR_HEAP_IS_EMPTY: &str = "heap is empty";

/// 堆中单个条目：持有分析作业及其在 `queue` 中的下标。
struct HeapItem {
    /// 分析作业对象（trait 对象）。
    obj: Box<dyn AnalysisJob>,
    /// 在 `queue` 向量中的位置，便于 O(1) 定位后上浮/下沉。
    index: usize,
}

/// 优先队列堆：`items` 按表 ID 索引作业，`queue` 维护堆序下的表 ID 序列。
#[derive(Default)]
pub struct PqHeapImpl {
    /// 表 ID → 堆条目。
    items: HashMap<i64, HeapItem>,
    /// 堆数组，元素为表 ID；下标 0 为权重最大者。
    queue: Vec<i64>,
}

/// 构造空堆。
pub fn NewHeap() -> PqHeapImpl {
    PqHeapImpl::default()
}

impl PqHeapImpl {
    /// 比较两个堆下标：权重更大者“更优先”（最大堆）。
    fn less(&self, left: usize, right: usize) -> bool {
        let left = self.items.get(&self.queue[left]).expect("heap index");
        let right = self.items.get(&self.queue[right]).expect("heap index");
        left.obj.GetWeight() > right.obj.GetWeight()
    }

    /// 交换两个下标处的表 ID，并同步更新各自在 `items` 中记录的 index。
    fn swap(&mut self, left: usize, right: usize) {
        self.queue.swap(left, right);
        let left_key = self.queue[left];
        let right_key = self.queue[right];
        self.items.get_mut(&left_key).expect("heap key").index = left;
        self.items.get_mut(&right_key).expect("heap key").index = right;
    }

    /// 自底向上调整，使 index 处节点满足堆性质。
    fn sift_up(&mut self, mut index: usize) {
        while index > 0 {
            let parent = (index - 1) / 2;
            if !self.less(index, parent) {
                break;
            }
            self.swap(index, parent);
            index = parent;
        }
    }

    /// 自顶向下调整，将 index 处节点下沉到正确位置。
    fn sift_down(&mut self, mut index: usize) {
        loop {
            let left = index * 2 + 1;
            if left >= self.queue.len() {
                break;
            }
            let right = left + 1;
            // 在左右子节点中选权重更大者。
            let best = if right < self.queue.len() && self.less(right, left) {
                right
            } else {
                left
            };
            if !self.less(best, index) {
                break;
            }
            self.swap(index, best);
            index = best;
        }
    }

    /// 在 index 处作业权重变化后，上浮或下沉以恢复堆序。
    fn fix(&mut self, index: usize) {
        if index > 0 && self.less(index, (index - 1) / 2) {
            self.sift_up(index);
        } else {
            self.sift_down(index);
        }
    }

    /// 按表 ID 插入或更新作业；已存在则替换对象并 `fix`，否则追加后 `sift_up`。
    pub fn AddOrUpdate(&mut self, obj: Box<dyn AnalysisJob>) -> Result<(), String> {
        let key = obj.GetTableID();
        if let Some(item) = self.items.get_mut(&key) {
            let index = item.index;
            item.obj = obj;
            self.fix(index);
            return Ok(());
        }
        let index = self.queue.len();
        self.queue.push(key);
        self.items.insert(key, HeapItem { obj, index });
        self.sift_up(index);
        Ok(())
    }

    /// 更新作业（Go 中与 AddOrUpdate 同义）。
    pub fn Update(&mut self, obj: Box<dyn AnalysisJob>) -> Result<(), String> {
        // Go's update is an alias for addOrUpdate.
        // Go 的 update 是 addOrUpdate 的别名。
        self.AddOrUpdate(obj)
    }

    /// 按表 ID 删除作业：与末尾交换后弹出，再对原位置 `fix`。
    pub fn DeleteByKey(&mut self, key: i64) -> Result<Box<dyn AnalysisJob>, String> {
        let Some(item) = self.items.get(&key) else {
            return Err("object not found".to_owned());
        };
        let index = item.index;
        let last = self.queue.len() - 1;
        // 非末尾则先与末尾交换，保证 pop 掉目标下标。
        if index != last {
            self.swap(index, last);
        }
        self.queue.pop();
        let removed = self.items.remove(&key).expect("heap item").obj;
        if index < self.queue.len() {
            self.fix(index);
        }
        Ok(removed)
    }

    /// 按作业对象的表 ID 删除。
    pub fn Delete(&mut self, obj: &dyn AnalysisJob) -> Result<Box<dyn AnalysisJob>, String> {
        self.DeleteByKey(obj.GetTableID())
    }

    /// 查看堆顶（权重最大）作业，不移除。
    pub fn Peek(&self) -> Result<&dyn AnalysisJob, String> {
        let Some(key) = self.queue.first() else {
            return Err(ERR_HEAP_IS_EMPTY.to_owned());
        };
        Ok(self.items.get(key).expect("heap item").obj.as_ref())
    }

    /// 弹出堆顶作业。
    pub fn Pop(&mut self) -> Result<Box<dyn AnalysisJob>, String> {
        let key = *self
            .queue
            .first()
            .ok_or_else(|| ERR_HEAP_IS_EMPTY.to_owned())?;
        self.DeleteByKey(key)
    }

    /// 按当前堆序返回所有作业引用。
    pub fn List(&self) -> Vec<&dyn AnalysisJob> {
        self.queue
            .iter()
            .filter_map(|key| self.items.get(key).map(|item| item.obj.as_ref()))
            .collect()
    }

    /// 堆中作业数量。
    pub fn Len(&self) -> usize {
        self.queue.len()
    }
    /// 返回当前堆序下的全部表 ID。
    pub fn ListKeys(&self) -> Vec<i64> {
        self.queue.clone()
    }
    /// 按表 ID 查找作业。
    pub fn GetByKey(&self, key: i64) -> Option<&dyn AnalysisJob> {
        self.items.get(&key).map(|item| item.obj.as_ref())
    }
    /// 按作业对象的表 ID 查找。
    pub fn Get(&self, obj: &dyn AnalysisJob) -> Option<&dyn AnalysisJob> {
        self.GetByKey(obj.GetTableID())
    }
    /// 堆是否为空。
    pub fn IsEmpty(&self) -> bool {
        self.queue.is_empty()
    }
}
