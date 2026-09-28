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
// limitations under the License.

// Manager 构造、方法转发与错误传播的单元测试。
//
// 使用 stub 控制器地址与 FakeClient，验证 NewManager 生命周期、
// Ping 失败路径，以及各业务方法注入的 deadline / 指标标签。

use std::any::Any;
use std::sync::{Arc, Mutex};

use astersql_extworkload::client::{Client, ClientError};
use astersql_extworkload::manager_impl::{manager, metricLabels, metricLabelsKey};
use astersql_extworkload::{Manager, ManagerError, NewManager, config, context, keyspacepb};

/// 关闭→None；缺 meta→错误；stub 成功→可查询 Role/Meta 并 Close。
#[test]
fn test_new_manager_lifecycle() {
    let context = context::Background();
    let meta = keyspacepb::KeyspaceMeta {
        id: 42,
        name: "starter-ks".to_owned(),
    };

    assert!(
        NewManager(&context, None, config::ExternalWorkload::default())
            .expect("disabled manager creation should succeed")
            .is_none()
    );

    let mut enabled = config::ExternalWorkload {
        Enable: true,
        Role: config::RoleMaster.to_owned(),
        TidbPool: "vip-tidb-pool".to_owned(),
        ControllerAddr: String::new(),
    };
    let error = NewManager(&context, None, enabled.clone())
        .err()
        .expect("enabled manager without keyspace metadata must fail");
    assert!(error.to_string().contains("non-nil keyspace meta"));

    enabled.ControllerAddr = "stub://ok".to_owned();
    let mut manager = NewManager(&context, Some(&meta), enabled)
        .expect("stub controller should accept Ping")
        .expect("enabled manager should be present");
    assert_eq!(config::RoleMaster, manager.Role());
    assert_eq!(Some(&meta), manager.Meta());
    manager.Close().expect("manager close should succeed");
}

/// stub://ping-error 应使 NewManager 在 Ping 阶段失败。
#[test]
fn test_new_manager_ping_failure() {
    let context = context::Background();
    let meta = keyspacepb::KeyspaceMeta {
        id: 1,
        name: "ks".to_owned(),
    };
    let config = config::ExternalWorkload {
        Enable: true,
        Role: config::RoleMaster.to_owned(),
        TidbPool: "super-vip-tidb-pool".to_owned(),
        ControllerAddr: "stub://ping-error".to_owned(),
    };

    let error = NewManager(&context, Some(&meta), config)
        .err()
        .expect("controller Ping error must reject manager creation");
    assert!(
        error
            .to_string()
            .contains("ping external workload controller")
    );
}

/// FakeClient 累积的最近一次调用观测状态。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct FakeState {
    /// 方法名。
    call: String,
    /// GC safe point。
    safe_point: u64,
    /// GC 生命周期。
    gc_life_time: i64,
    /// TTL 表 ID。
    table_id: i64,
    /// TTL 作业是否启用。
    ttl_job_enable: bool,
    /// 已完成 TTL 作业创建时间。
    completed_job_create_time: u64,
    /// 自动分析任务 ID。
    task_id: u64,
    /// 请求 context 是否带 deadline。
    deadline_set: bool,
    /// 指标 (workerType, action)。
    labels: Option<(String, String)>,
}

/// 可注入错误并共享 FakeState 的客户端桩。
struct FakeClient {
    /// 跨调用共享的观测状态。
    state: Arc<Mutex<FakeState>>,
    /// 若设置则业务方法统一返回该错误。
    error: Option<ClientError>,
}

impl FakeClient {
    /// 记录方法名、deadline 与从 context 取出的指标标签。
    fn record(&self, context: &context::Context, call: &str) {
        let mut state = self.state.lock().expect("fake client state poisoned");
        state.call = call.to_owned();
        state.deadline_set = context.Deadline();
        state.labels = context
            .Value::<metricLabelsKey, metricLabels>(metricLabelsKey)
            .map(|labels| (labels.workerType, labels.action));
    }

    /// 按 error 字段返回 Ok 或 Err。
    fn result(&self) -> Result<(), ClientError> {
        self.error.clone().map_or(Ok(()), Err)
    }
}

impl Client for FakeClient {
    fn as_any(&self) -> &dyn Any {
        self
    }

    fn Close(&mut self) -> Result<(), ClientError> {
        Ok(())
    }

    fn Ping(&mut self, _context: &context::Context) -> Result<(), ClientError> {
        Ok(())
    }

    fn RegisterGCV2(
        &mut self,
        context: &context::Context,
        safe_point: u64,
        gc_life_time: i64,
    ) -> Result<(), ClientError> {
        self.record(context, "RegisterGCV2");
        let mut state = self.state.lock().expect("fake client state poisoned");
        state.safe_point = safe_point;
        state.gc_life_time = gc_life_time;
        drop(state);
        self.result()
    }

    fn RecycleGCV2(
        &mut self,
        context: &context::Context,
        safe_point: u64,
    ) -> Result<(), ClientError> {
        self.record(context, "RecycleGCV2");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .safe_point = safe_point;
        self.result()
    }

    fn UpdateGCLifeTime(
        &mut self,
        context: &context::Context,
        gc_life_time: i64,
    ) -> Result<(), ClientError> {
        self.record(context, "UpdateGCLifeTime");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .gc_life_time = gc_life_time;
        self.result()
    }

    fn RegisterTTLTask(
        &mut self,
        context: &context::Context,
        table_id: i64,
        enabled: bool,
    ) -> Result<(), ClientError> {
        self.record(context, "RegisterTTLTask");
        let mut state = self.state.lock().expect("fake client state poisoned");
        state.table_id = table_id;
        state.ttl_job_enable = enabled;
        drop(state);
        self.result()
    }

    fn DeleteTTLTableInfo(
        &mut self,
        context: &context::Context,
        table_id: i64,
    ) -> Result<(), ClientError> {
        self.record(context, "DeleteTTLTableInfo");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .table_id = table_id;
        self.result()
    }

    fn RecycleTTLTask(
        &mut self,
        context: &context::Context,
        create_time: u64,
    ) -> Result<(), ClientError> {
        self.record(context, "RecycleTTLTask");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .completed_job_create_time = create_time;
        self.result()
    }

    fn UpdateTTLJobEnable(
        &mut self,
        context: &context::Context,
        enabled: bool,
    ) -> Result<(), ClientError> {
        self.record(context, "UpdateTTLJobEnable");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .ttl_job_enable = enabled;
        self.result()
    }

    fn RegisterAutoAnalyze(
        &mut self,
        context: &context::Context,
        task_id: u64,
    ) -> Result<(), ClientError> {
        self.record(context, "RegisterAutoAnalyze");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .task_id = task_id;
        self.result()
    }

    fn RecycleAutoAnalyze(
        &mut self,
        context: &context::Context,
        task_id: u64,
    ) -> Result<(), ClientError> {
        self.record(context, "RecycleAutoAnalyze");
        self.state
            .lock()
            .expect("fake client state poisoned")
            .task_id = task_id;
        self.result()
    }
}

/// 装配带 FakeClient 的 manager，执行一次调用并返回观测状态。
fn run_manager_call(call: impl FnOnce(&mut manager) -> Result<(), ManagerError>) -> FakeState {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let client = FakeClient {
        state: Arc::clone(&state),
        error: None,
    };
    let mut manager = manager {
        cli: Box::new(client),
        role: config::RoleMaster.to_owned(),
        meta: keyspacepb::KeyspaceMeta {
            id: 1,
            name: "ks".to_owned(),
        },
    };
    call(&mut manager).expect("manager call should succeed");
    let result = state.lock().expect("fake client state poisoned").clone();
    result
}

/// 断言状态中的指标标签等于期望的 worker/action。
fn assert_labels(state: &FakeState, worker_type: &str, action: &str) {
    assert_eq!(
        Some((worker_type.to_owned(), action.to_owned())),
        state.labels
    );
}

/// 各方法应设置请求超时，并按 Go 规则附加或省略指标标签。
#[test]
fn test_manager_methods_set_deadline_and_metrics() {
    let background = context::Background();

    let state = run_manager_call(|manager| manager.InitializeGCV2(&background));
    assert_eq!("RegisterGCV2", state.call);
    assert_eq!(0, state.safe_point);
    assert_eq!(600, state.gc_life_time);
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleGCV2Worker, "init");

    let state = run_manager_call(|manager| manager.AbortGCV2(&background));
    assert_eq!("RecycleGCV2", state.call);
    assert_eq!(u64::MAX, state.safe_point);
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleGCV2Worker, "abort");

    let state = run_manager_call(|manager| manager.RegisterGCV2(&background, 10, 600));
    assert_eq!(
        ("RegisterGCV2", 10, 600),
        (state.call.as_str(), state.safe_point, state.gc_life_time)
    );
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleGCV2Worker, "register");

    let state = run_manager_call(|manager| manager.RecycleGCV2(&background, 20));
    assert_eq!(("RecycleGCV2", 20), (state.call.as_str(), state.safe_point));
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleGCV2Worker, "recycle");

    let state = run_manager_call(|manager| manager.UpdateGCLifeTime(&background, 60));
    assert_eq!(
        ("UpdateGCLifeTime", 60),
        (state.call.as_str(), state.gc_life_time)
    );
    assert!(state.deadline_set);
    assert_eq!(None, state.labels);

    let state = run_manager_call(|manager| manager.RegisterTTLTask(&background, 11, true));
    assert_eq!(
        ("RegisterTTLTask", 11, true),
        (state.call.as_str(), state.table_id, state.ttl_job_enable)
    );
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleTTLTaskWorker, "register");

    let state = run_manager_call(|manager| manager.DeleteTTLTableInfo(&background, 12));
    assert_eq!(
        ("DeleteTTLTableInfo", 12),
        (state.call.as_str(), state.table_id)
    );
    assert!(state.deadline_set);
    assert_eq!(None, state.labels);

    let state = run_manager_call(|manager| manager.RecycleTTLTask(&background, 99));
    assert_eq!(
        ("RecycleTTLTask", 99),
        (state.call.as_str(), state.completed_job_create_time)
    );
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleTTLTaskWorker, "recycle");

    let state = run_manager_call(|manager| manager.UpdateTTLJobEnable(&background, true));
    assert_eq!(
        ("UpdateTTLJobEnable", true),
        (state.call.as_str(), state.ttl_job_enable)
    );
    assert!(state.deadline_set);
    assert_eq!(None, state.labels);

    let state = run_manager_call(|manager| manager.RegisterAutoAnalyze(&background, 7));
    assert_eq!(
        ("RegisterAutoAnalyze", 7),
        (state.call.as_str(), state.task_id)
    );
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleAutoAnalyzeWorker, "register");

    let state = run_manager_call(|manager| manager.RecycleAutoAnalyze(&background, 8));
    assert_eq!(
        ("RecycleAutoAnalyze", 8),
        (state.call.as_str(), state.task_id)
    );
    assert!(state.deadline_set);
    assert_labels(&state, config::RoleAutoAnalyzeWorker, "recycle");
}

/// 底层客户端错误应原样向上传播。
#[test]
fn test_manager_method_error_propagation() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let client = FakeClient {
        state,
        error: Some(ClientError("boom".to_owned())),
    };
    let mut manager = manager {
        cli: Box::new(client),
        role: config::RoleMaster.to_owned(),
        meta: keyspacepb::KeyspaceMeta {
            id: 1,
            name: "ks".to_owned(),
        },
    };

    let error = manager
        .RegisterTTLTask(&context::Background(), 1, true)
        .expect_err("client error must propagate");
    assert_eq!("boom", error.to_string());
}
