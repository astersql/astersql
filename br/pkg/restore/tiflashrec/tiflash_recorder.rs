// Copyright 2026 AsterSQL.
// Copyright 2022-present PingCAP, Inc.
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

//! TiFlash 副本录制器，对齐 Go `br/pkg/restore/tiflashrec/tiflash_recorder.go`。
//! 在 restore 过程中记录表级 TiFlash 副本配置，事后生成 ALTER 以重建或复位副本。
//! InfoSchema 仅作 TableByID 边界；缺表时静默跳过，避免恢复中途 schema 未齐导致失败。
//! DDL 文本手写对齐 Go ast.Restore 标志，不引入完整 parser 依赖。

use std::collections::HashMap;

/// 镜像 `model.TiFlashReplicaInfo`：副本数与拓扑标签。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TiFlashReplicaInfo {
    /// TiFlash 副本个数。
    pub Count: u64,
    /// 可选拓扑标签；空则不输出 LOCATION LABELS。
    pub LocationLabels: Vec<String>,
}

/// 大小写不敏感标识：O 为原始写法，L 为小写，对齐 TiDB CIStr。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CIStr {
    pub O: String,
    pub L: String,
}

impl CIStr {
    /// 由原始名构造；L 固定为 lowercase，供比较路径使用。
    pub fn new(o: impl Into<String>) -> Self {
        let O = o.into();
        let L = O.to_lowercase();
        Self { O, L }
    }
}

/// DDL 生成所需的最小表元数据（当前仅 Name）。
#[derive(Clone, Debug, Default)]
pub struct TableMeta {
    pub Name: CIStr,
}

/// InfoSchema 边界：DDL 生成通过 TableByID 解析库表名。
pub trait InfoSchema: Send + Sync {
    fn TableByID(&self, id: i64) -> Option<(TableMeta, CIStr)>;
}

/// 恢复期间录制 TiFlash 副本；items 以 tableID 为键。
#[derive(Default)]
pub struct TiFlashRecorder {
    items: HashMap<i64, TiFlashReplicaInfo>,
}

impl TiFlashRecorder {
    /// 空录制器；与 Go `New()` 等价。
    pub fn New() -> Self {
        Self {
            items: HashMap::new(),
        }
    }

    /// 整体替换内部映射（非逐条 upsert），用于从检查点/快照装载。
    pub fn Load(&mut self, items: HashMap<i64, TiFlashReplicaInfo>) {
        self.items = items;
    }

    /// 只读视图，便于测试与导出。
    pub fn GetItems(&self) -> &HashMap<i64, TiFlashReplicaInfo> {
        &self.items
    }

    /// 登记或覆盖表副本配置。
    pub fn AddTable(&mut self, tableID: i64, replica: TiFlashReplicaInfo) {
        self.items.insert(tableID, replica);
    }

    /// 删除表记录；不存在时静默。
    pub fn DelTable(&mut self, tableID: i64) {
        self.items.remove(&tableID);
    }

    /// 遍历顺序不保证；调用方若需稳定输出应自行排序（如测试 ElementsMatch）。
    pub fn Iterate(&self, mut f: impl FnMut(i64, &TiFlashReplicaInfo)) {
        for (k, v) in &self.items {
            f(*k, v);
        }
    }

    /// 表 ID 重写：同 ID 直接返回；否则搬迁条目（旧键删除）。
    pub fn Rewrite(&mut self, oldID: i64, newID: i64) {
        if newID == oldID {
            return;
        }
        if let Some(old) = self.items.remove(&oldID) {
            self.items.insert(newID, old);
        }
    }

    /// 生成“先清零再恢复”的 ALTER 对，供需要重建副本的场景。
    pub fn GenerateResetAlterTableDDLs(&self, info: &dyn InfoSchema) -> Vec<String> {
        let mut items = Vec::with_capacity(self.items.len() * 2);
        self.Iterate(|id, replica| {
            let Some((table, schema)) = info.TableByID(id) else {
                // 缺表：与 Go 一样跳过，不报错。
                return;
            };
            let Ok(reset_spec) = alterTableSpecOf(replica, true) else {
                return;
            };
            items.push(format!(
                "ALTER TABLE {} {reset_spec}",
                EncloseDBAndTable(&schema.O, &table.Name.O)
            ));
            let Ok(spec) = alterTableSpecOf(replica, false) else {
                return;
            };
            items.push(format!(
                "ALTER TABLE {} {spec}",
                EncloseDBAndTable(&schema.O, &table.Name.O)
            ));
        });
        items
    }

    /// 仅生成目标副本设置的 ALTER（不含先清零）。
    pub fn GenerateAlterTableDDLs(&self, info: &dyn InfoSchema) -> Vec<String> {
        let mut items = Vec::with_capacity(self.items.len());
        self.Iterate(|tableId, replica| {
            // 缺表跳过，与 Reset 路径相同策略。
            let Some((table, schema)) = info.TableByID(tableId) else {
                return;
            };
            let Ok(spec) = alterTableSpecOf(replica, false) else {
                return;
            };
            // schema/table 用原始 O 字段，保留用户大小写。
            items.push(format!(
                "ALTER TABLE {} {spec}",
                EncloseDBAndTable(&schema.O, &table.Name.O)
            ));
        });
        items
    }
}

/// 反引号包裹库表；内部 `` ` `` 加倍转义，防注入与语法断裂。
fn EncloseDBAndTable(db: &str, table: &str) -> String {
    format!("`{}`.`{}`", db.replace('`', "``"), table.replace('`', "``"))
}

/// 构造 `SET TIFLASH REPLICA …` 片段，对齐 Go ast.AlterTableSpec.Restore 标志组合。
fn alterTableSpecOf(replica: &TiFlashReplicaInfo, reset: bool) -> Result<String, String> {
    // Matches ast.AlterTableSpec.Restore with
    // RestoreKeyWordUppercase | RestoreNameBackQuotes | RestoreStringSingleQuotes |
    // RestoreStringEscapeBackslash (see Go alterTableSpecOf).
    if reset {
        return Ok("SET TIFLASH REPLICA 0".to_string());
    }
    let mut s = format!("SET TIFLASH REPLICA {}", replica.Count);
    if !replica.LocationLabels.is_empty() {
        // 标签：单引号包裹；`\`→`\\`、`'`→`''`，与 Go Restore 字符串规则一致。
        s.push_str(" LOCATION LABELS ");
        let labels: Vec<String> = replica
            .LocationLabels
            .iter()
            .map(|l| {
                let escaped = l.replace('\\', "\\\\").replace('\'', "''");
                format!("'{escaped}'")
            })
            .collect();
        s.push_str(&labels.join(", "));
    }
    Ok(s)
}
