// Copyright 2026 AsterSQL.
// Copyright 2021 PingCAP, Inc.
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

// 本文件对照 pkg/meta/meta_autoid.go 实现单类 AutoID accessor 与整组 accessor 的组合逻辑。
// 所有读写直接复用 Mutator/TxStructure，保留 rename、drop 与并发分配时的完整 ID 语义。

// AutoID 元数据访问器：对单表的 RowID / AUTO_INCREMENT / AUTO_RANDOM / Sequence 字段做读写。
//
// AutoID 存在 meta 的 `DB:<db_id>` hash 下；rename/drop 可能与分配并发，因此访问器必须保留创建时的完整 schema/table ID。

use crate::errors;
use crate::meta::{
    Mutator, auto_increment_id_key, auto_random_table_id_key, auto_table_id_key, db_key,
    sequence_key,
};
use crate::model;

// AutoIdAccessor 表示某一种 ID 键的读取、覆盖、自增、复制和删除入口。
/// 单种 ID 字段的读写接口：get/put/inc/copy_to/del。
pub trait AutoIdAccessor {
    fn get(&self) -> Result<i64, errors::Error>;
    fn put(&mut self, value: i64) -> Result<(), errors::Error>;
    fn inc(&mut self, step: i64) -> Result<i64, errors::Error>;
    fn copy_to(&mut self, database_id: i64, table_id: i64) -> Result<(), errors::Error>;
    fn del(&mut self) -> Result<(), errors::Error>;
}

// IdEncodeFn 对应 Go 的 func(int64) []byte，通过切换函数选择 RowID、IncrementID 等字段编码。
/// 将 table_id 编码为 meta hash field 名的函数类型。
pub type IdEncodeFn = fn(i64) -> Vec<u8>;

// AutoIdAccessorImpl 保存原始 schema/table ID；rename 后仍使用这对完整 ID，不能只校验当前 schema。
/// 绑定 Mutator 与固定 database/table ID 的具体访问器实现。
pub struct AutoIdAccessorImpl<'a> {
    pub mutator: &'a Mutator,
    pub database_id: i64,
    pub table_id: i64,
    pub id_encode_fn: IdEncodeFn,
}

impl AutoIdAccessor for AutoIdAccessorImpl<'_> {
    // get 从 DB:<database_id> hash 的选定 ID field 读取十进制 int64。
    fn get(&self) -> Result<i64, errors::Error> {
        self.mutator.txn.hget_i64(
            &db_key(self.database_id),
            &(self.id_encode_fn)(self.table_id),
        )
    }

    // put 以十进制字符串覆盖当前 ID 字段。
    fn put(&mut self, value: i64) -> Result<(), errors::Error> {
        self.mutator.txn.hset(
            &db_key(self.database_id),
            &(self.id_encode_fn)(self.table_id),
            value.to_string().as_bytes(),
        )
    }

    // inc 不验证 schema/table 是否仍存在：rename 与 drop 可和 ID 分配并发，必须沿用创建时的完整 ID。
    fn inc(&mut self, step: i64) -> Result<i64, errors::Error> {
        self.mutator.txn.hinc(
            &db_key(self.database_id),
            &(self.id_encode_fn)(self.table_id),
            step,
        )
    }

    // del 删除当前选择的 ID field，底层错误原样向上传播。
    fn del(&mut self) -> Result<(), errors::Error> {
        self.mutator.txn.hdel(
            &db_key(self.database_id),
            &(self.id_encode_fn)(self.table_id),
        )
    }

    // copy_to 用于 rename table 后复制元数据；零值不复制，避免跨版本 BR 恢复覆盖目标表已有 ID。
    fn copy_to(&mut self, database_id: i64, table_id: i64) -> Result<(), errors::Error> {
        let current = self.get()?;
        if current == 0 {
            return Ok(());
        }
        self.mutator.txn.hset(
            &db_key(database_id),
            &(self.id_encode_fn)(table_id),
            current.to_string().as_bytes(),
        )
    }
}

// AutoIdAccessors 表示一张表的 RowID、独立 AUTO_INCREMENT 与 AUTO_RANDOM 三元组控制器。
/// 整组 AutoID（RowID / IncrementID / RandomID）的批量 get/put/del。
pub trait AutoIdAccessors {
    fn get(&mut self) -> Result<model::AutoIdGroup, errors::Error>;
    fn put(&mut self, ids: &model::AutoIdGroup) -> Result<(), errors::Error>;
    fn del(&mut self) -> Result<(), errors::Error>;
}

// AccessorPicker 按 ID 类型选择相应元数据 field；sequence value/cycle 复用同一个底层访问器。
/// 按 ID 种类切换编码函数并返回对应访问器引用。
pub trait AccessorPicker {
    fn row_id(&mut self) -> &mut dyn AutoIdAccessor;
    fn random_id(&mut self) -> &mut dyn AutoIdAccessor;
    fn increment_id(&mut self, table_version: u16) -> &mut dyn AutoIdAccessor;
    fn sequence_value(&mut self) -> &mut dyn AutoIdAccessor;
    fn sequence_cycle(&mut self) -> &mut dyn AutoIdAccessor;
}

// TableInfoVersion5 开始，_tidb_rowid 与 auto_increment 使用不同的 meta field。
/// 从此表版本起 RowID 与 AUTO_INCREMENT 使用分离的 meta field。
const SEP_AUTO_INC_VER: u16 = model::TABLE_INFO_VERSION_5;

// AutoIdAccessorsImpl 复用一个 accessor；每次 picker 调用只替换编码函数，不改变完整表标识。
/// 通过替换 `id_encode_fn` 复用同一访问器实例。
pub struct AutoIdAccessorsImpl<'a> {
    pub access: AutoIdAccessorImpl<'a>,
}

impl AccessorPicker for AutoIdAccessorsImpl<'_> {
    // row_id 选择 TID:<table_id> 字段。
    fn row_id(&mut self) -> &mut dyn AutoIdAccessor {
        self.access.id_encode_fn = auto_table_id_key;
        &mut self.access
    }

    // increment_id 为旧表版本选择共享 TID 字段，新版本选择独立 IID 字段。
    fn increment_id(&mut self, table_version: u16) -> &mut dyn AutoIdAccessor {
        self.access.id_encode_fn = if table_version < SEP_AUTO_INC_VER {
            auto_table_id_key
        } else {
            auto_increment_id_key
        };
        &mut self.access
    }

    // random_id 选择 TARID:<table_id> 字段。
    fn random_id(&mut self) -> &mut dyn AutoIdAccessor {
        self.access.id_encode_fn = auto_random_table_id_key;
        &mut self.access
    }

    // sequence_value 选择 SID:<table_id> 字段。
    fn sequence_value(&mut self) -> &mut dyn AutoIdAccessor {
        self.access.id_encode_fn = sequence_key;
        &mut self.access
    }

    // sequence_cycle 选择 SequenceCycle:<table_id> 字段。
    fn sequence_cycle(&mut self) -> &mut dyn AutoIdAccessor {
        self.access.id_encode_fn = sequence_cycle_key;
        &mut self.access
    }
}

impl AutoIdAccessors for AutoIdAccessorsImpl<'_> {
    // get 按 RowID、IncrementID、RandomID 顺序读取；任一步失败立即返回，不掩盖部分读取错误。
    fn get(&mut self) -> Result<model::AutoIdGroup, errors::Error> {
        let row_id = self.row_id().get()?;
        let increment_id = self.increment_id(SEP_AUTO_INC_VER).get()?;
        let random_id = self.random_id().get()?;
        Ok(model::AutoIdGroup {
            row_id,
            increment_id,
            random_id,
        })
    }

    // put 保持 Go 的写入顺序，前一字段失败时不会继续写后续字段。
    fn put(&mut self, ids: &model::AutoIdGroup) -> Result<(), errors::Error> {
        self.row_id().put(ids.row_id)?;
        self.increment_id(SEP_AUTO_INC_VER).put(ids.increment_id)?;
        self.random_id().put(ids.random_id)
    }

    // del 依次清理三类 ID field；sequence 字段不属于 AutoIDGroup，故不在此处删除。
    fn del(&mut self) -> Result<(), errors::Error> {
        self.row_id().del()?;
        self.increment_id(SEP_AUTO_INC_VER).del()?;
        self.random_id().del()
    }
}

// sequence_cycle_key 对应 Mutator.sequenceCycleKey 的字段编码。
/// 编码 Sequence 循环计数 field：`SequenceCycle:<table_id>`。
pub fn sequence_cycle_key(id: i64) -> Vec<u8> {
    format!("SequenceCycle:{id}").into_bytes()
}

// new_auto_id_accessors 为指定 schema/table 创建整组访问器；默认编码函数会在首次 picker 调用时覆盖。
/// 为指定 database/table 构造整组 AutoID 访问器。
pub fn new_auto_id_accessors(
    mutator: &Mutator,
    database_id: i64,
    table_id: i64,
) -> AutoIdAccessorsImpl<'_> {
    AutoIdAccessorsImpl {
        access: AutoIdAccessorImpl {
            mutator,
            database_id,
            table_id,
            id_encode_fn: auto_table_id_key,
        },
    }
}
