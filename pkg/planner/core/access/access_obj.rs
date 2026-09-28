// Copyright 2026 AsterSQL.
// Copyright 2025 PingCAP, Inc.
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

// EXPLAIN / tipb 访问对象（AccessObject）的文本与 protobuf 表示。
//
// 访问对象描述物理算子实际触达的库表、索引与分区，供 EXPLAIN 输出
// 以及序列化到 tipb::ExplainOperator。覆盖扫描（Scan）、其它描述、
// 以及动态分区裁剪（dynamic partition pruning）相关对象。

// 对应 pkg/planner/core/access/access_obj.go，生成 EXPLAIN 访问对象文本与 tipb 消息。

use protobuf::RepeatedField;
use tipb;

/// ScanAccessObject 表示对表的访问，也可同时记录该表的索引和分区。
pub struct ScanAccessObject {
    pub Database: String,
    pub Table: String,
    pub Indexes: Vec<IndexAccess>,
    pub Partitions: Vec<String>,
}

impl ScanAccessObject {
    /// NormalizedString 对应 AccessObject 的归一化输出；具体分区名统一折叠为 `?`。
    pub fn NormalizedString(&self) -> String {
        let mut b = String::new();
        if !self.Table.is_empty() {
            b.push_str("table:");
            b.push_str(&self.Table);
        }
        // 归一化场景隐藏真实分区名，便于计划比对。
        if !self.Partitions.is_empty() {
            b.push_str(", partition:?");
        }
        for index in &self.Indexes {
            if index.IsClusteredIndex {
                b.push_str(", clustered index:");
            } else {
                b.push_str(", index:");
            }
            b.push_str(&index.Name);
            b.push('(');
            b.push_str(&index.Cols.join(", "));
            b.push(')');
        }
        b
    }

    /// String 保留真实分区名，并按 Go 的顺序追加索引名称和列列表。
    pub fn String(&self) -> String {
        let mut b = String::new();
        if !self.Table.is_empty() {
            b.push_str("table:");
            b.push_str(&self.Table);
        }
        if !self.Partitions.is_empty() {
            b.push_str(", partition:");
            b.push_str(&self.Partitions.join(","));
        }
        for index in &self.Indexes {
            if index.IsClusteredIndex {
                b.push_str(", clustered index:");
            } else {
                b.push_str(", index:");
            }
            b.push_str(&index.Name);
            b.push('(');
            b.push_str(&index.Cols.join(", "));
            b.push(')');
        }
        b
    }

    /// SetIntoPB 把扫描对象写入 ExplainOperator，并覆盖其 AccessObjects 为单个 scan 对象。
    pub fn SetIntoPB(&self, pb: Option<&mut tipb::ExplainOperator>) {
        let Some(pb) = pb else {
            // 对应 Go 的 nil protobuf 防御分支；Rust 的有效借用已排除 nil self。
            return;
        };

        let mut pb_obj = tipb::ScanAccessObject::new();
        pb_obj.set_database(self.Database.clone());
        pb_obj.set_table(self.Table.clone());
        pb_obj.set_partitions(RepeatedField::from_vec(self.Partitions.clone()));
        for index in &self.Indexes {
            pb_obj.mut_indexes().push(index.ToPB());
        }
        let mut access = tipb::AccessObject::new();
        access.set_scan_object(pb_obj);
        pb.set_access_objects(RepeatedField::from_vec(vec![access]));
    }
}

/// IndexAccess 表示算子访问的一个索引及其参与列。
/// IsClusteredIndex 为真表示聚簇索引（索引组织即主键行数据）。
pub struct IndexAccess {
    pub Name: String,
    pub Cols: Vec<String>,
    pub IsClusteredIndex: bool,
}

impl IndexAccess {
    /// ToPB 对应 Go 的 protobuf 转换；Rust 借用不能为 nil，因此无需返回 Go 的 nil 分支。
    pub fn ToPB(&self) -> tipb::IndexAccess {
        let mut pb = tipb::IndexAccess::new();
        pb.set_name(self.Name.clone());
        pb.set_cols(RepeatedField::from_vec(self.Cols.clone()));
        pb.set_is_clustered_index(self.IsClusteredIndex);
        pb
    }
}

/// OtherAccessObject 对应 Go 的 string 派生类型，承载无法归入扫描或动态分区的访问描述。
pub struct OtherAccessObject(pub String);

impl OtherAccessObject {
    /// 返回原始描述字符串。
    pub fn String(&self) -> String {
        self.0.clone()
    }

    /// 其他对象不含可归一化标识，因此归一化文本与普通文本相同。
    pub fn NormalizedString(&self) -> String {
        self.String()
    }

    /// SetIntoPB 仅在目标和描述均有效时写入一个 other object。
    pub fn SetIntoPB(&self, pb: Option<&mut tipb::ExplainOperator>) {
        let Some(pb) = pb else { return };
        if self.0.is_empty() {
            return;
        }
        let mut access = tipb::AccessObject::new();
        access.set_other_object(self.0.clone());
        pb.set_access_objects(RepeatedField::from_vec(vec![access]));
    }
}

/// DynamicPartitionAccessObject 表示当前算子的子节点在动态裁剪模式下实际访问的分区。
/// 动态分区裁剪：运行时按谓词决定读哪些分区，而非编译期静态选定。
pub struct DynamicPartitionAccessObject {
    pub Database: String,
    pub Table: String,
    pub AllPartitions: bool,
    pub Partitions: Vec<String>,
    pub Err: String,
}

impl DynamicPartitionAccessObject {
    /// 生成分区访问文本：优先错误，其次 all / dual / 具体分区列表。
    pub fn String(&self) -> String {
        // 错误文本优先，避免把失败的动态裁剪误报为正常分区集合。
        if !self.Err.is_empty() {
            return self.Err.clone();
        }
        if self.AllPartitions {
            return "partition:all".to_owned();
        } else if self.Partitions.is_empty() {
            return "partition:dual".to_owned();
        }
        format!("partition:{}", self.Partitions.join(","))
    }
}

/// DynamicPartitionAccessObjects 对应 Go 的对象指针切片，并为整组对象实现 AccessObject 语义。
pub struct DynamicPartitionAccessObjects(pub Vec<Box<DynamicPartitionAccessObject>>);

impl DynamicPartitionAccessObjects {
    /// 单对象直接委托；多对象时拼接为「分区描述 of 表名」列表。
    pub fn String(&self) -> String {
        if self.0.is_empty() {
            return String::new();
        }
        if self.0.len() == 1 {
            return self.0[0].String();
        }

        let mut b = String::new();
        for (i, access) in self.0.iter().enumerate() {
            if i != 0 {
                b.push_str(", ");
            }
            b.push_str(&access.String());
            b.push_str(" of ");
            b.push_str(&access.Table);
        }
        b
    }

    /// 动态分区集合不隐藏分区信息，归一化结果沿用 Go 的 String 输出。
    pub fn NormalizedString(&self) -> String {
        self.String()
    }

    /// SetIntoPB 保留 Go 的两阶段构造：先创建定长对象切片，再组装指针列表并写入 ExplainOperator。
    pub fn SetIntoPB(&self, pb: Option<&mut tipb::ExplainOperator>) {
        if self.0.is_empty() {
            return;
        }
        let Some(pb) = pb else { return };

        // 定长切片保证索引与输入对齐；含 Err 的项保持零值（对齐 Go continue）。
        let mut pb_obj_slice = vec![tipb::DynamicPartitionAccessObject::new(); self.0.len()];
        for (i, obj) in self.0.iter().enumerate() {
            if !obj.Err.is_empty() {
                // Go 的 continue 会在对应位置留下零值对象，而不是缩短输出切片。
                continue;
            }
            pb_obj_slice[i].set_database(obj.Database.clone());
            pb_obj_slice[i].set_table(obj.Table.clone());
            pb_obj_slice[i].set_all_partitions(obj.AllPartitions);
            pb_obj_slice[i].set_partitions(RepeatedField::from_vec(obj.Partitions.clone()));
        }

        let mut pb_objs = tipb::DynamicPartitionAccessObjects::new();
        pb_objs.set_objects(RepeatedField::from_vec(pb_obj_slice));
        let mut access = tipb::AccessObject::new();
        access.set_dynamic_partition_objects(pb_objs);
        pb.set_access_objects(RepeatedField::from_vec(vec![access]));
    }
}
