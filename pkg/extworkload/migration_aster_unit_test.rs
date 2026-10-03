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

// Manager / 角色谓词与 Go 行为对齐的迁移回归测试。
//
// 通过 FakeClient 记录调用参数、deadline 与指标标签，
// 断言 NewManager、各转发方法及错误传播与 Go 一致。

use astersql_extworkload::*;

/// 记录最近一次调用参数与 context 派生信息的客户端桩。
#[derive(Default)]
struct FakeClient {
    /// 最近一次方法名。
    call: &'static str,
    /// GC safe point 参数。
    safe_point: u64,
    /// GC 生命周期秒数。
    gc_life_time: i64,
    /// TTL 表 ID。
    table_id: i64,
    /// TTL/作业开关。
    enabled: bool,
    /// TTL 作业创建时间。
    create_time: u64,
    /// 自动分析任务 ID。
    task_id: u64,
    /// context 是否设置了 deadline。
    deadline: bool,
    /// 指标标签 (workerType, action)。
    labels: Option<(String, String)>,
    /// 为 true 时所有业务调用返回错误。
    fail: bool,
}

impl FakeClient {
    /// 记录方法名、deadline 与指标标签；按 fail 决定成败。
    fn record(
        &mut self,
        ctx: &context::Context,
        call: &'static str,
    ) -> Result<(), client::ClientError> {
        self.call = call;
        self.deadline = ctx.Deadline();
        self.labels = ctx
            .Value::<manager_impl::metricLabelsKey, manager_impl::metricLabels>(
                manager_impl::metricLabelsKey,
            )
            .map(|v| (v.workerType, v.action));
        if self.fail {
            Err(client::ClientError("boom".into()))
        } else {
            Ok(())
        }
    }
}

impl client::Client for FakeClient {
    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
    fn Close(&mut self) -> Result<(), client::ClientError> {
        Ok(())
    }
    fn Ping(&mut self, ctx: &context::Context) -> Result<(), client::ClientError> {
        self.record(ctx, "Ping")
    }
    fn RegisterGCV2(
        &mut self,
        ctx: &context::Context,
        safe: u64,
        life: i64,
    ) -> Result<(), client::ClientError> {
        self.safe_point = safe;
        self.gc_life_time = life;
        self.record(ctx, "RegisterGCV2")
    }
    fn RecycleGCV2(
        &mut self,
        ctx: &context::Context,
        safe: u64,
    ) -> Result<(), client::ClientError> {
        self.safe_point = safe;
        self.record(ctx, "RecycleGCV2")
    }
    fn UpdateGCLifeTime(
        &mut self,
        ctx: &context::Context,
        life: i64,
    ) -> Result<(), client::ClientError> {
        self.gc_life_time = life;
        self.record(ctx, "UpdateGCLifeTime")
    }
    fn RegisterTTLTask(
        &mut self,
        ctx: &context::Context,
        id: i64,
        enabled: bool,
    ) -> Result<(), client::ClientError> {
        self.table_id = id;
        self.enabled = enabled;
        self.record(ctx, "RegisterTTLTask")
    }
    fn DeleteTTLTableInfo(
        &mut self,
        ctx: &context::Context,
        id: i64,
    ) -> Result<(), client::ClientError> {
        self.table_id = id;
        self.record(ctx, "DeleteTTLTableInfo")
    }
    fn RecycleTTLTask(
        &mut self,
        ctx: &context::Context,
        time: u64,
    ) -> Result<(), client::ClientError> {
        self.create_time = time;
        self.record(ctx, "RecycleTTLTask")
    }
    fn UpdateTTLJobEnable(
        &mut self,
        ctx: &context::Context,
        enabled: bool,
    ) -> Result<(), client::ClientError> {
        self.enabled = enabled;
        self.record(ctx, "UpdateTTLJobEnable")
    }
    fn RegisterAutoAnalyze(
        &mut self,
        ctx: &context::Context,
        id: u64,
    ) -> Result<(), client::ClientError> {
        self.task_id = id;
        self.record(ctx, "RegisterAutoAnalyze")
    }
    fn RecycleAutoAnalyze(
        &mut self,
        ctx: &context::Context,
        id: u64,
    ) -> Result<(), client::ClientError> {
        self.task_id = id;
        self.record(ctx, "RecycleAutoAnalyze")
    }
}

/// 用给定客户端与角色装配 manager 实例。
fn make_manager(cli: FakeClient, role: &str) -> manager_impl::manager {
    manager_impl::manager {
        cli: Box::new(cli),
        role: role.to_owned(),
        meta: keyspacepb::KeyspaceMeta {
            id: 42,
            name: "starter-ks".into(),
            config: Default::default(),
        },
    }
}

/// 从 manager 取出底层 FakeClient 借用。
fn fake(m: &manager_impl::manager) -> &FakeClient {
    m.cli
        .as_ref()
        .as_any()
        .downcast_ref::<FakeClient>()
        .unwrap()
}

/// nil Manager 与专职角色矩阵应对齐 Go 谓词语义。
#[test]
fn role_predicates_match_go_nil_and_dedicated_roles() {
    assert!(!IsEnabled(None));
    assert!(!IsMaster(None));
    assert!(!IsGCV2Worker(None));
    assert!(!IsTTLTaskWorker(None));
    assert!(!IsAutoAnalyzeWorker(None));
    for role in [
        config::RoleMaster,
        config::RoleGCV2Worker,
        config::RoleTTLTaskWorker,
        config::RoleAutoAnalyzeWorker,
    ] {
        let m = make_manager(FakeClient::default(), role);
        let r: &dyn Manager = &m;
        assert_eq!(role == config::RoleMaster, IsMaster(Some(r)));
        assert_eq!(role == config::RoleGCV2Worker, IsGCV2Worker(Some(r)));
        assert_eq!(role == config::RoleTTLTaskWorker, IsTTLTaskWorker(Some(r)));
        assert_eq!(
            role == config::RoleAutoAnalyzeWorker,
            IsAutoAnalyzeWorker(Some(r))
        );
    }
}

/// 关闭配置返回 None；启用但无 keyspace meta 应报错。
#[test]
fn new_manager_disabled_and_nil_meta_match_go() {
    let disabled = config::ExternalWorkload::default();
    assert!(
        NewManager(&context::Background(), None, disabled)
            .unwrap()
            .is_none()
    );
    let enabled = config::ExternalWorkload {
        Enable: true,
        Role: "master".into(),
        TidbPool: "vip".into(),
        ControllerAddr: "127.0.0.1:1".into(),
    };
    let err = NewManager(&context::Background(), None, enabled)
        .err()
        .unwrap();
    assert!(err.to_string().contains("non-nil keyspace meta"));
}

/// 各 Manager 方法的参数、超时、指标标签与错误传播对齐 Go。
#[test]
fn manager_methods_match_go_arguments_deadlines_labels_and_errors() {
    let ctx = context::Background();
    let mut m = make_manager(FakeClient::default(), "master");
    m.InitializeGCV2(&ctx, std::time::Duration::from_secs(3600))
        .unwrap();
    assert_call(&m, "RegisterGCV2", 0, 3600, Some(("gcv2", "init")));
    m.AbortGCV2(&ctx).unwrap();
    assert_call(&m, "RecycleGCV2", u64::MAX, 3600, Some(("gcv2", "abort")));
    m.RegisterGCV2(&ctx, 10, std::time::Duration::from_secs(700))
        .unwrap();
    assert_call(&m, "RegisterGCV2", 10, 700, Some(("gcv2", "register")));
    m.RecycleGCV2(&ctx, 20).unwrap();
    assert_call(&m, "RecycleGCV2", 20, 700, Some(("gcv2", "recycle")));
    m.UpdateGCLifeTime(&ctx, std::time::Duration::from_secs(60))
        .unwrap();
    assert_call(&m, "UpdateGCLifeTime", 20, 60, None);
    m.RegisterTTLTask(&ctx, 11, true).unwrap();
    assert_call(&m, "RegisterTTLTask", 20, 60, Some(("ttl", "register")));
    assert_eq!(11, fake(&m).table_id);
    assert!(fake(&m).enabled);
    m.DeleteTTLTableInfo(&ctx, 12).unwrap();
    assert_call(&m, "DeleteTTLTableInfo", 20, 60, None);
    assert_eq!(12, fake(&m).table_id);
    m.RecycleTTLTask(&ctx, 99).unwrap();
    assert_call(&m, "RecycleTTLTask", 20, 60, Some(("ttl", "recycle")));
    assert_eq!(99, fake(&m).create_time);
    m.UpdateTTLJobEnable(&ctx, false).unwrap();
    assert_call(&m, "UpdateTTLJobEnable", 20, 60, None);
    assert!(!fake(&m).enabled);
    m.RegisterAutoAnalyze(&ctx, 7).unwrap();
    assert_call(
        &m,
        "RegisterAutoAnalyze",
        20,
        60,
        Some(("auto-analyze", "register")),
    );
    assert_eq!(7, fake(&m).task_id);
    m.RecycleAutoAnalyze(&ctx, 8).unwrap();
    assert_call(
        &m,
        "RecycleAutoAnalyze",
        20,
        60,
        Some(("auto-analyze", "recycle")),
    );
    assert_eq!(8, fake(&m).task_id);

    let mut failed = make_manager(
        FakeClient {
            fail: true,
            ..Default::default()
        },
        "master",
    );
    assert_eq!(
        "boom",
        failed
            .RegisterTTLTask(&ctx, 1, true)
            .unwrap_err()
            .to_string()
    );
}

/// 断言 FakeClient 记录的调用名、safe/life 与可选指标标签。
fn assert_call(
    m: &manager_impl::manager,
    call: &str,
    safe: u64,
    life: i64,
    labels: Option<(&str, &str)>,
) {
    let cli = fake(m);
    assert!(cli.deadline);
    assert_eq!(call, cli.call);
    assert_eq!(safe, cli.safe_point);
    assert_eq!(life, cli.gc_life_time);
    assert_eq!(
        labels.map(|(a, b)| (a.to_owned(), b.to_owned())),
        cli.labels
    );
}
