// Copyright 2026 AsterSQL.
// Copyright 2026 PingCAP, Inc.
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

// 存储层 Manager 的 mock。
//
// 覆盖节点 CPU 核数查询、按 ID 取任务、按 ID 修改任务等接口，
// 实现 `storage::Manager` trait 以便注入真实调用路径。
// limitations under the License.

use astersql_dxf_framework_storage as storage;

use crate::Handler;

// 存储结果别名。
type StorageResult<T> = Result<T, storage::Error>;

/// 存储 Manager mock：各方法一个 Handler。
#[derive(Default)]
pub struct MockManager {
    /// 返回当前节点 CPU 核数（常用于 slot 计算）。
    pub GetCPUCountOfNode: Handler<dyn FnMut(storage::Context) -> StorageResult<i32> + Send>,
    /// 按任务 ID 读取完整任务。
    pub GetTaskByID:
        Handler<dyn FnMut(storage::Context, i64) -> StorageResult<storage::proto::Task> + Send>,
    /// 按任务 ID 应用 ModifyParam 修改。
    pub ModifyTaskByID: Handler<
        dyn FnMut(storage::Context, i64, storage::proto::ModifyParam) -> StorageResult<()> + Send,
    >,
}

/// 期望记录器别名。
pub type MockManagerMockRecorder = MockManager;

/// GoMock 风格 API 与派发。
impl MockManager {
    /// 返回期望记录器。
    pub fn EXPECT(&mut self) -> &mut MockManagerMockRecorder {
        self
    }

    /// GoMock 标记占位。
    pub fn ISGOMOCK(&self) {}

    /// 派发 GetCPUCountOfNode。
    pub fn GetCPUCountOfNode(&self, context: storage::Context) -> StorageResult<i32> {
        self.GetCPUCountOfNode
            .invoke("MockManager.GetCPUCountOfNode", |handler| handler(context))
    }

    /// 派发 GetTaskByID。
    pub fn GetTaskByID(
        &self,
        context: storage::Context,
        task_id: i64,
    ) -> StorageResult<storage::proto::Task> {
        self.GetTaskByID
            .invoke("MockManager.GetTaskByID", |handler| {
                handler(context, task_id)
            })
    }

    /// 派发 ModifyTaskByID。
    pub fn ModifyTaskByID(
        &self,
        context: storage::Context,
        task_id: i64,
        param: storage::proto::ModifyParam,
    ) -> StorageResult<()> {
        self.ModifyTaskByID
            .invoke("MockManager.ModifyTaskByID", |handler| {
                handler(context, task_id, param)
            })
    }
}

/// 实现 storage::Manager，转发到 Handler。
impl storage::Manager for MockManager {
    fn GetCPUCountOfNode(&self, context: storage::Context) -> StorageResult<i32> {
        self.GetCPUCountOfNode(context)
    }

    fn GetTaskByID(
        &self,
        context: storage::Context,
        task_id: i64,
    ) -> StorageResult<storage::proto::Task> {
        self.GetTaskByID(context, task_id)
    }

    fn ModifyTaskByID(
        &self,
        context: storage::Context,
        task_id: i64,
        param: storage::proto::ModifyParam,
    ) -> StorageResult<()> {
        self.ModifyTaskByID(context, task_id, param)
    }
}

/// 构造空期望 MockManager。
pub fn NewMockManager<C: ?Sized>(_controller: &C) -> MockManager {
    MockManager::default()
}
