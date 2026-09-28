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

// DXF 节点资源缓存与 dist_framework_meta 元数据操作。
//
// 对应 Go 的 nodes.go：维护本机 NodeResource 快照，并提供 Init/Recover
// Meta、清理死亡节点、查询忙碌节点与已用 slot、按角色取 CPU 等接口。

// 从 pkg/dxf/framework/storage/nodes.go 迁移，保持节点资源缓存和元数据 SQL 行为一致。

// nodeResource 对应 Go 的 atomic.Pointer[proto.NodeResource]；读写通过 RwLock 保持线程安全快照语义。
/// 进程内节点资源快照（对齐 Go atomic.Pointer[NodeResource]）。
static nodeResource: std::sync::RwLock<Option<proto::NodeResource>> =
    std::sync::RwLock::new(Some(proto::NodeResource {
        TotalCPU: 8,
        TotalMem: 16 * 1024 * 1024 * 1024,
        TotalDisk: 100 * 1024 * 1024 * 1024,
    }));

// GetNodeResource gets the node resource.
/// 读取当前节点资源快照副本。
pub fn GetNodeResource() -> Option<proto::NodeResource> {
    nodeResource
        .read()
        .expect("node resource lock poisoned")
        .as_ref()
        .map(|resource| {
            proto::NewNodeResource(resource.TotalCPU, resource.TotalMem, resource.TotalDisk)
        })
}

// SetNodeResource sets the node resource.
/// 写入/替换节点资源快照。
pub fn SetNodeResource(rc: proto::NodeResource) {
    *nodeResource.write().expect("node resource lock poisoned") = Some(rc);
}

// GetDXFCPUCount returns the cpu count usable to DXF.
/// 返回 DXF 可用 CPU 核数；资源未设置时为 0。
pub fn GetDXFCPUCount() -> i32 {
    if let Some(rc) = GetNodeResource() {
        return rc.TotalCPU;
    }
    0
}

impl TaskManager {
    // InitMeta insert the manager information into dist_framework_meta.
    /// 在新 session 中初始化/更新 dist_framework_meta 中本节点记录。
    pub fn InitMeta(&self, ctx: Context, tidbID: String, role: String) -> Result<(), Error> {
        self.WithNewSession(|se| {
            self.InitMetaSession(ctx.clone(), se, tidbID.clone(), role.clone())
        })
    }

    // InitMetaSession insert the manager information into dist_framework_meta.
    // if the record exists, update the cpu_count and role.
    /// 插入或更新 host/role/cpu_count；存在则刷新 cpu_count 与 role。
    pub fn InitMetaSession(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        execID: String,
        role: String,
    ) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let cpuCount = GetDXFCPUCount();
        sqlexec::ExecSQL(
            ctx,
            se.GetSQLExecutor(),
            "insert into mysql.dist_framework_meta(host, role, cpu_count, keyspace_id) values (%?, %?, %?, -1) on duplicate key update cpu_count = %?, role = %?",
            vec![
                execID.into(),
                role.clone().into(),
                cpuCount.into(),
                cpuCount.into(),
                role.into(),
            ],
        )?;
        Ok(())
    }

    // RecoverMeta insert the manager information into dist_framework_meta.
    // if the record exists, update the cpu_count.
    // Don't update role for we only update it in `set global tidb_service_scope`.
    // if not there might has a data race.
    /// 恢复 meta：仅更新 cpu_count，不改 role（避免与 service_scope 竞态）。
    pub fn RecoverMeta(&self, ctx: Context, execID: String, role: String) -> Result<(), Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let cpuCount = GetDXFCPUCount();
        self.ExecuteSQLWithNewSession(
            ctx,
            "insert into mysql.dist_framework_meta(host, role, cpu_count, keyspace_id) values (%?, %?, %?, -1) on duplicate key update cpu_count = %?",
            vec![execID.into(), role.into(), cpuCount.into(), cpuCount.into()],
        )?;
        Ok(())
    }

    // DeleteDeadNodes deletes the dead nodes from mysql.dist_framework_meta.
    /// 事务内按 host 列表删除死亡节点的 meta 记录。
    pub fn DeleteDeadNodes(&self, ctx: Context, nodes: Vec<String>) -> Result<(), Error> {
        if nodes.is_empty() {
            return Ok(());
        }
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewTxn(ctx.clone(), |se| {
            let mut deleteSQL = String::new();
            sqlescape::FormatSQL(
                &mut deleteSQL,
                "delete from mysql.dist_framework_meta where host in(",
            )?;
            let deleteElems: Vec<String> =
                nodes.iter().map(|node| format!("\"{}\"", node)).collect();
            deleteSQL.push_str(&deleteElems.join(", "));
            deleteSQL.push(')');
            sqlexec::ExecSQL(ctx.clone(), se.GetSQLExecutor(), deleteSQL, vec![])?;
            Ok(())
        })
    }

    // GetAllNodes gets nodes in dist_framework_meta.
    /// 按 host 排序返回 dist_framework_meta 中全部托管节点。
    pub fn GetAllNodes(&self, ctx: Context) -> Result<Vec<proto::ManagedNode>, Error> {
        let r = tracing::StartRegion(ctx.clone(), "TaskManager.GetAllNodes");
        let mut nodes: Vec<proto::ManagedNode> = Vec::new();
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        let err = self.WithNewSession(|se| {
            nodes = self.getAllNodesWithSession(ctx.clone(), se)?;
            Ok(())
        });
        r.End();
        err?;
        Ok(nodes)
    }

    /// 在指定 session 上查询全部托管节点。
    fn getAllNodesWithSession(
        &self,
        ctx: Context,
        se: sessionctx::Context,
    ) -> Result<Vec<proto::ManagedNode>, Error> {
        let r = tracing::StartRegion(ctx.clone(), "TaskManager.getAllNodesWithSession");
        let rs = sqlexec::ExecSQL(
            ctx,
            se.GetSQLExecutor(),
            "select host, role, cpu_count from mysql.dist_framework_meta order by host",
            vec![],
        )?;
        let mut nodes = Vec::with_capacity(rs.len());
        for row in rs {
            nodes.push(proto::ManagedNode {
                ID: row.GetString(0),
                Role: row.GetString(1),
                CPUCount: row.GetInt64(2) as i32,
            });
        }
        r.End();
        Ok(nodes)
    }

    // GetBusyNodes gets nodes that are currently running subtasks.
    /// 返回当前有 pending/running 子任务的 exec_id 列表。
    pub fn GetBusyNodes(&self, ctx: Context) -> Result<Vec<schstatus::Node>, Error> {
        let mut execIDs: Vec<schstatus::Node> = Vec::new();
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        self.WithNewSession(|se| {
            let rs = sqlexec::ExecSQL(
                ctx.clone(),
                se.GetSQLExecutor(),
                "select distinct exec_id from mysql.tidb_background_subtask where state in (%?, %?)",
                vec![proto::SubtaskStatePending.into(), proto::SubtaskStateRunning.into()],
            )?;
            execIDs = rs.into_iter().map(|row| schstatus::Node { ID: row.GetString(0), IsOwner: false }).collect();
            Ok(())
        })?;
        Ok(execIDs)
    }

    // GetUsedSlotsOnNodes implements the scheduler.TaskManager interface.
    /// 按 exec_id 汇总已用 concurrency（slot）；同 task 先取 max 再求和。
    pub fn GetUsedSlotsOnNodes(&self, ctx: Context) -> Result<HashMap<String, i32>, Error> {
        injectfailpoint::DXFRandomErrorWithOnePercent()?;
        // Go 用 max(concurrency) 折叠同一 task_key 的 subtask，再按 exec_id 求和。
        let rs = self.ExecuteSQLWithNewSession(
            ctx,
            "select exec_id, sum(concurrency) from (select exec_id, task_key, max(concurrency) concurrency from mysql.tidb_background_subtask where state in (%?, %?) group by exec_id, task_key) a group by exec_id",
            vec![proto::SubtaskStatePending.into(), proto::SubtaskStateRunning.into()],
        )?;
        let mut slots = HashMap::with_capacity(rs.len());
        for row in rs {
            let val = row.GetMyDecimal(1).ToInt().0;
            slots.insert(row.GetString(0), val as i32);
        }
        Ok(slots)
    }

    // GetCPUCountOfNode gets the cpu count of node.
    /// 取任一有效托管节点的 CPU 数（arbitrary=true）。
    pub fn GetCPUCountOfNode(&self, ctx: Context) -> Result<i32, Error> {
        let mut cnt = 0;
        self.WithNewSession(|se| {
            cnt = self.getCPUCountOfNodeByRole(ctx.clone(), se, String::new(), true)?;
            Ok(())
        })?;
        Ok(cnt)
    }

    // GetCPUCountOfNodeByRole gets the cpu count of node by role.
    /// 按 role 匹配托管节点并返回其 CPU 数。
    pub fn GetCPUCountOfNodeByRole(&self, ctx: Context, role: String) -> Result<i32, Error> {
        let mut cnt = 0;
        self.WithNewSession(|se| {
            cnt = self.getCPUCountOfNodeByRole(ctx.clone(), se, role.clone(), false)?;
            Ok(())
        })?;
        Ok(cnt)
    }

    // getCPUCountOfNodeByRole gets the cpu count of managed node by role,
    // returns error when there's no node or no node has valid cpu count.
    /// 内部实现：无节点或无有效 CPU 时返回错误。
    fn getCPUCountOfNodeByRole(
        &self,
        ctx: Context,
        se: sessionctx::Context,
        role: String,
        arbitrary: bool,
    ) -> Result<i32, Error> {
        let nodes = self.getAllNodesWithSession(ctx, se)?;
        if nodes.is_empty() {
            return Err(Error::new("no managed nodes"));
        }
        let mut cpuCount = 0;
        for n in nodes {
            if !arbitrary && n.Role != role {
                continue;
            }
            if n.CPUCount > 0 {
                cpuCount = n.CPUCount;
                break;
            }
        }
        if cpuCount == 0 {
            return Err(Error::new(
                "no managed node have enough resource for dist task",
            ));
        }
        Ok(cpuCount)
    }
}

// init 对应 Go 文件的 init：domain 未启动的测试场景使用默认节点资源。
/// 测试/未启动 domain 场景的默认节点资源初始化。
pub fn init() {
    SetNodeResource(proto::NewNodeResource(
        8,
        16 * units::GiB,
        (100 * units::GiB) as u64,
    ));
}
