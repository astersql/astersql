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

// 统计信息缓存内部存储抽象。
//
// 定义 `StatsCacheInner` trait，作为 LFU / Map 等具体缓存实现的统一边界，
// 对外提供按表 ID 的读写、容量控制与异步淘汰。

use statistics::Table;
use std::sync::Arc;

/// 统计缓存实现共用的存储边界。
/// Storage boundary used by the statistics cache implementations.
pub trait StatsCacheInner: Send {
    /// 按物理表 ID 查询缓存中的表统计。
    fn Get(&self, tid: i64) -> Option<Arc<Table>>;
    /// 写入或更新表统计；返回是否实际插入/替换成功。
    fn Put(&mut self, tid: i64, table: Arc<Table>) -> bool;
    /// 按物理表 ID 删除缓存项。
    fn Del(&mut self, tid: i64);
    /// 当前缓存占用的内存代价（用于容量与淘汰决策）。
    fn Cost(&self) -> i64;
    /// 返回缓存中全部表统计快照。
    fn Values(&self) -> Vec<Arc<Table>>;
    /// 缓存项数量。
    fn Len(&self) -> usize;
    /// 深拷贝当前缓存实现（用于 Copy-on-Write 式更新）。
    fn Copy(&self) -> Box<dyn StatsCacheInner>;
    /// 设置缓存容量上限。
    fn SetCapacity(&mut self, capacity: i64);
    /// 关闭缓存并释放资源。
    fn Close(&mut self);
    /// 主动触发一次淘汰（evict）。
    fn TriggerEvict(&mut self);
    /// 等待异步更新/淘汰完成后再继续。
    fn WaitForAsyncUpdates(&mut self);
}
