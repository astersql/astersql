// Copyright 2026 AsterSQL.
// Copyright 2020 PingCAP, Inc. Licensed under Apache-2.0.

// Decode targets carry raw bytes; SQL/CSV writers own field classification and escaping.
static COLUMN_TYPES: OnceLock<()> = OnceLock::new();
static DATA_TYPE_STRING: OnceLock<HashSet<&'static str>> = OnceLock::new();
static DATA_TYPE_INT: OnceLock<HashSet<&'static str>> = OnceLock::new();
static DATA_TYPE_NUM: OnceLock<HashSet<&'static str>> = OnceLock::new();
static DATA_TYPE_BIN: OnceLock<HashSet<&'static str>> = OnceLock::new();
pub fn initColumnTypeSets() {
    COLUMN_TYPES.get_or_init(|| {
        let data_type_string_arr = [
            "CHAR",
            "NCHAR",
            "VARCHAR",
            "NVARCHAR",
            "CHARACTER",
            "VARCHARACTER",
            "TIMESTAMP",
            "DATETIME",
            "DATE",
            "TIME",
            "YEAR",
            "SQL_TSI_YEAR",
            "TEXT",
            "TINYTEXT",
            "MEDIUMTEXT",
            "LONGTEXT",
            "ENUM",
            "SET",
            "JSON",
            "NULL",
            "VAR_STRING",
        ];
        let data_type_int_arr = [
            "INTEGER",
            "BIGINT",
            "TINYINT",
            "SMALLINT",
            "MEDIUMINT",
            "INT",
            "INT1",
            "INT2",
            "INT3",
            "INT8",
            "UNSIGNED INT",
            "UNSIGNED BIGINT",
            "UNSIGNED TINYINT",
            "UNSIGNED SMALLINT",
        ];
        let data_type_num_arr = [
            "INTEGER",
            "BIGINT",
            "TINYINT",
            "SMALLINT",
            "MEDIUMINT",
            "INT",
            "INT1",
            "INT2",
            "INT3",
            "INT8",
            "UNSIGNED INT",
            "UNSIGNED BIGINT",
            "UNSIGNED TINYINT",
            "UNSIGNED SMALLINT",
            "FLOAT",
            "REAL",
            "DOUBLE",
            "DOUBLE PRECISION",
            "DECIMAL",
            "NUMERIC",
            "FIXED",
            "BOOL",
            "BOOLEAN",
        ];
        let data_type_bin_arr = [
            "BLOB",
            "TINYBLOB",
            "MEDIUMBLOB",
            "LONGBLOB",
            "LONG",
            "BINARY",
            "VARBINARY",
            "BIT",
            "GEOMETRY",
        ];
        DATA_TYPE_STRING
            .set(data_type_string_arr.into_iter().collect())
            .unwrap();
        DATA_TYPE_INT
            .set(data_type_int_arr.into_iter().collect())
            .unwrap();
        DATA_TYPE_NUM
            .set(data_type_num_arr.into_iter().collect())
            .unwrap();
        DATA_TYPE_BIN
            .set(data_type_bin_arr.into_iter().collect())
            .unwrap();
    });
}
pub fn dataTypeStringContains(s: &str) -> bool {
    initColumnTypeSets();
    DATA_TYPE_STRING.get().unwrap().contains(s)
}
pub fn dataTypeIntContains(s: &str) -> bool {
    initColumnTypeSets();
    DATA_TYPE_INT.get().unwrap().contains(s)
}
pub fn dataTypeNumContains(s: &str) -> bool {
    initColumnTypeSets();
    DATA_TYPE_NUM.get().unwrap().contains(s)
}
pub fn dataTypeBinContains(s: &str) -> bool {
    initColumnTypeSets();
    DATA_TYPE_BIN.get().unwrap().contains(s)
}

pub fn MakeRowReceiver(col_types: &[String]) -> RowReceiverArr {
    RowReceiverArr {
        bound: false,
        data: vec![None; col_types.len()],
    }
}
pub struct RowReceiverArr {
    pub bound: bool,
    data: Vec<Option<Vec<u8>>>,
}
impl RowReceiver for RowReceiverArr {
    fn BindAddress(&mut self, args: &mut [RawBytes]) {
        // The Rust Rows adapter copies values after scanning rather than binding pointers.
        self.bound = true;
        for (dst, src) in self.data.iter_mut().zip(args.iter()) {
            *dst = src.0.clone();
        }
    }
}
impl RowReceiverArr {
    pub fn rawValues(&self) -> &[Option<Vec<u8>>] {
        &self.data
    }
    pub fn GetRawBytes(&self) -> Vec<RawBytes> {
        self.data.iter().cloned().map(RawBytes).collect()
    }
    pub fn appendRawBytes(&self, dst: &mut Vec<RawBytes>) {
        dst.extend(self.data.iter().cloned().map(RawBytes));
    }
}
