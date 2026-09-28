// Copyright 2026 AsterSQL.
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

// 最小 job_id 刷新器的单元测试。
//
// 用脚本化 Manager 依次返回预设的 `get_min_job_id` 结果，
// 验证缓存单调递增，且作业清空返回 0 时不会回退。

use std::sync::{Arc, Mutex};

use crate::{
    Cancellation, Context, Error, JobWrapper, Manager, MinJobIdRefresher, Session,
    new_min_job_id_refresher,
};

/// 按预设响应序列返回 `get_min_job_id`，并记录每次调用的 previous 参数。
struct ScriptedManager {
    responses: Mutex<Vec<Result<i64, Error>>>,
    calls: Mutex<Vec<i64>>,
}

impl ScriptedManager {
    /// 构造带有指定响应队列的脚本 Manager。
    fn new(responses: Vec<Result<i64, Error>>) -> Self {
        Self {
            responses: Mutex::new(responses),
            calls: Mutex::new(Vec::new()),
        }
    }

    /// 返回已记录的 `previous_min_job_id` 调用序列。
    fn calls(&self) -> Vec<i64> {
        self.calls.lock().unwrap().clone()
    }
}

impl Manager for ScriptedManager {
    fn get_job_by_id(&self, _context: &Context, _job_id: i64) -> Result<JobWrapper, Error> {
        Err(Error::Execute("unexpected".into()))
    }

    fn get_job_bytes_by_id_with_session(
        &self,
        _context: &Context,
        _session: &mut dyn Session,
        _job_id: i64,
    ) -> Result<Vec<u8>, Error> {
        Err(Error::Execute("unexpected".into()))
    }

    fn get_mdl_version(&self, _context: &Context, _job_id: i64) -> Result<i64, Error> {
        Err(Error::Execute("unexpected".into()))
    }

    fn get_min_job_id(&self, _context: &Context, previous_min_job_id: i64) -> Result<i64, Error> {
        self.calls.lock().unwrap().push(previous_min_job_id);
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            return Err(Error::Execute("no more scripted responses".into()));
        }
        responses.remove(0)
    }

    fn has_flashback_cluster_job(
        &self,
        _context: &Context,
        _min_job_id: i64,
    ) -> Result<bool, Error> {
        Err(Error::Execute("unexpected".into()))
    }
}

/// 对应 Go 的 TestRefreshMinJobID：连续刷新应单调推进，返回 0 时保持原值。
// test_refresh_min_job_id 对应 Go 的 TestRefreshMinJobID。
#[test]
fn test_refresh_min_job_id() {
    let mgr = Arc::new(ScriptedManager::new(vec![Ok(1), Ok(100), Ok(0)]));
    let refresher: MinJobIdRefresher = new_min_job_id_refresher(mgr.clone());
    let ctx = Context::default();

    refresher.refresh(&ctx);
    assert_eq!(1, refresher.current_min_job_id());
    assert_eq!(vec![0], mgr.calls());

    refresher.refresh(&ctx);
    assert_eq!(100, refresher.current_min_job_id());
    assert_eq!(vec![0, 1], mgr.calls());

    // don't go back when all jobs are done
    // 作业全部完成后系统表返回 0，缓存应保持 100 不回退。
    refresher.refresh(&ctx);
    assert_eq!(100, refresher.current_min_job_id());
    assert_eq!(vec![0, 1, 100], mgr.calls());
}

/// Go 的 Start 在进入 select 检查取消前总会先执行一次 refresh。
#[test]
fn test_start_refreshes_once_when_already_cancelled() {
    let mgr = Arc::new(ScriptedManager::new(vec![Ok(7)]));
    let refresher = new_min_job_id_refresher(mgr.clone());
    let cancellation = Cancellation::default();
    cancellation.cancel();

    refresher.start(&Context::default(), &cancellation);

    assert_eq!(7, refresher.current_min_job_id());
    assert_eq!(vec![0], mgr.calls());
    assert!(!refresher.is_running());
}
