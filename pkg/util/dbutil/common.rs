// Copyright 2026 AsterSQL.
// Copyright 2022 PingCAP, Inc.
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

// dbutil 通用数据库访问辅助：配置/DSN、元数据查询、校验和、统计桶、重试与批量删除。
//
// 文件前半为迁移草稿（保留 Go 调用形状与中文说明）；可执行实现从草稿结束后的 `use` 开始，
// 通过 `QueryExecutor`/`DBExecutor` 抽象访问数据库，避免直接依赖具体驱动。

// dbutil 中数据库配置、SQL 拼装、查询扫描、重试和事务辅助流程。

/* Mechanical draft retained for migration history.
use std::collections::HashMap;
use std::sync::LazyLock;

// DefaultRetryTime is the default retry time to execute sql
pub const DefaultRetryTime: i32 = 10;

// DefaultTimeout is the default timeout for execute sql
// Go 使用 time.Duration = 10 * time.Second；这里保留 Duration 表达式作为。
pub static DefaultTimeout: LazyLock<time::Duration> = LazyLock::new(|| 10 * time::Second);

// SlowLogThreshold defines the duration to log debug log of sql when exec time greater than
pub static SlowLogThreshold: LazyLock<time::Duration> =
    LazyLock::new(|| 200 * time::Millisecond);

// DefaultDeleteRowsNum is the default rows num for delete one time
pub const DefaultDeleteRowsNum: i64 = 100000;

// ErrVersionNotFound means can't get the database's version
pub static ErrVersionNotFound: LazyLock<errors::Error> =
    LazyLock::new(|| errors::New("can't get the database's version"));

// ErrNoData means no data in table
pub static ErrNoData: LazyLock<errors::Error> =
    LazyLock::new(|| errors::New("no data found in table"));

// DBConfig is database configuration.
// DBConfig 对应 Go 结构体和 toml/json tag；保留字段顺序，tag 语义留给后续 serde 接线。
pub struct DBConfig {
    pub Host: String,
    pub User: String,
    pub Password: String,
    pub Schema: String,
    pub Snapshot: String,
    pub Port: i32,
}

impl DBConfig {
    // String returns native format of database configuration
    // String 对应 Go 的 json.Marshal；密码字段在 Go tag 中被 json:"-" 隐藏。
    pub fn String(&self) -> String {
        match json::Marshal(self) {
            Ok(cfg) => String::from_utf8_lossy(&cfg).to_string(),
            Err(_) => "<nil>".to_string(),
        }
    }
}

// GetDBConfigFromEnv returns DBConfig from environment
// GetDBConfigFromEnv 按 Go 默认值读取 MYSQL_* 环境变量，不打开网络连接。
pub fn GetDBConfigFromEnv(schema: String) -> DBConfig {
    let mut host = os::Getenv("MYSQL_HOST");
    if host.is_empty() {
        host = "127.0.0.1".to_string();
    }
    let mut port = strconv::Atoi(os::Getenv("MYSQL_PORT")).unwrap_or_default();
    if port == 0 {
        port = 3306;
    }
    let mut user = os::Getenv("MYSQL_USER");
    if user.is_empty() {
        user = "root".to_string();
    }
    let pswd = os::Getenv("MYSQL_PSWD");

    DBConfig {
        Host: host,
        Port: port,
        User: user,
        Password: pswd,
        Schema: schema,
        Snapshot: String::new(),
    }
}

// OpenDB opens a mysql connection FD
// OpenDB 对应 Go 的 mysql.NewConfig/NewConnector/sql.OpenDB/Ping 流程。
// 当前 Rust 文件仅保留调用形状；真实 driver 初始化、网络连接和 Ping 都不是本迁移任务的完成条件。
pub fn OpenDB(
    cfg: DBConfig,
    vars: HashMap<String, String>,
) -> Result<*mut sql::DB, errors::Error> {
    let mut driverCfg = mysql::NewConfig();
    driverCfg.Params = HashMap::new();
    driverCfg.User = cfg.User;
    driverCfg.Passwd = cfg.Password;
    driverCfg.Net = "tcp".to_string();
    driverCfg.Addr = net::JoinHostPort(cfg.Host, strconv::Itoa(cfg.Port));
    driverCfg.Params.insert("charset".to_string(), "utf8mb4".to_string());

    if !cfg.Snapshot.is_empty() {
        // Snapshot 作为 tidb_snapshot DSN 参数接入；Go 会打日志说明连接使用了快照。
        log::Info("create connection with snapshot", zap::String("snapshot", cfg.Snapshot.clone()));
        driverCfg
            .Params
            .insert("tidb_snapshot".to_string(), cfg.Snapshot);
    }

    for (key, val) in vars {
        // key='val'. add single quote for better compatibility.
        // Go 会给变量值补单引号提升兼容性；这里保持同样的字符串格式。
        driverCfg.Params.insert(key, format!("'{}'", val));
    }

    let c = mysql::NewConnector(driverCfg).map_err(errors::Trace)?;
    let db = sql::OpenDB(c);
    let err = db.Ping();
    err.map_err(errors::Trace)?;
    Ok(db)
}

// CloseDB closes the mysql fd
// CloseDB 对应 Go 的 nil 保护和 db.Close 错误包装。
pub fn CloseDB(db: *mut sql::DB) -> Result<(), errors::Error> {
    if db.is_null() {
        return Ok(());
    }

    unsafe { (*db).Close() }.map_err(errors::Trace)
}

// GetCreateTableSQL returns the create table statement.
// GetCreateTableSQL 通过 SHOW CREATE TABLE 读取建表 SQL，并检查两列 sql.NullString 是否有效。
pub fn GetCreateTableSQL(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    tableName: String,
) -> Result<String, errors::Error> {
    /*
        show create table example result:
        mysql> SHOW CREATE TABLE `test`.`itest`;
        +-------+--------------------------------------------------------------------+
        | Table | Create Table                                                        |
        +-------+--------------------------------------------------------------------+
        | itest | CREATE TABLE `itest` (...) ENGINE=InnoDB DEFAULT CHARSET=utf8 ...   |
        +-------+--------------------------------------------------------------------+
    */
    let query = format!("SHOW CREATE TABLE {}", TableName(schemaName.clone(), tableName.clone()));

    let mut tbl = sql::NullString::default();
    let mut createTable = sql::NullString::default();
    db.QueryRowContext(ctx, query)
        .Scan((&mut tbl, &mut createTable))
        .map_err(errors::Trace)?;
    if !tbl.Valid || !createTable.Valid {
        return Err(errors::NotFoundf(format!("table {}", tableName)));
    }

    Ok(createTable.String)
}

// GetRowCount returns row count of the table.
// if not specify where condition, return total row count of the table.
// GetRowCount 保留可选 where 条件和可变参数扫描 count 的流程。
pub fn GetRowCount(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    tableName: String,
    where_clause: String,
    args: Vec<any::Any>,
) -> Result<i64, errors::Error> {
    /*
        select count example result:
        mysql> SELECT count(1) cnt from `test`.`itest` where id > 0;
        +------+
        | cnt  |
        +------+
        |  100 |
        +------+
    */

    let mut query = format!("SELECT COUNT(1) cnt FROM {}", TableName(schemaName.clone(), tableName.clone()));
    if !where_clause.is_empty() {
        query.push_str(&format!(" WHERE {}", where_clause));
    }
    log::Debug("get row count", zap::String("sql", query.clone()), zap::Reflect("args", &args));

    let mut cnt = sql::NullInt64::default();
    db.QueryRowContext(ctx, query, args)
        .Scan(&mut cnt)
        .map_err(errors::Trace)?;
    if !cnt.Valid {
        return Err(errors::NotFoundf(format!("table `{}.{}`", schemaName, tableName)));
    }

    Ok(cnt.Int64)
}

// GetRandomValues returns some random value. Tips: limitArgs is the value in limitRange.
// GetRandomValues 保留随机排序子查询、collation 拼接、rows.Close 收尾和 rows.Err 检查。
pub fn GetRandomValues(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    table: String,
    column: String,
    num: i32,
    mut limitRange: String,
    limitArgs: Vec<any::Any>,
    mut collation: String,
) -> Result<Vec<String>, errors::Error> {
    /*
        example:
        mysql> SELECT `id` FROM (SELECT `id`, rand() rand_value FROM `test`.`test`
               WHERE `id` COLLATE "latin1_bin" > 0 ... LIMIT 5) rand_tmp
               ORDER BY `id` COLLATE "latin1_bin";
    */

    if limitRange.is_empty() {
        limitRange = "TRUE".to_string();
    }

    if !collation.is_empty() {
        collation = format!(" COLLATE \"{}\"", collation);
    }

    let query = format!(
        "SELECT {0} FROM (SELECT {0}, rand() rand_value FROM {1} WHERE {2} ORDER BY rand_value LIMIT {3})rand_tmp ORDER BY {0}{4}",
        ColumnName(column),
        TableName(schemaName, table),
        limitRange,
        num,
        collation,
    );
    log::Debug("get random values", zap::String("sql", query.clone()), zap::Reflect("args", &limitArgs));

    let mut rows = db.QueryContext(ctx, query, limitArgs).map_err(errors::Trace)?;
    let mut randomValue: Vec<String> = Vec::with_capacity(num as usize);
    while rows.Next() {
        let mut value = sql::NullString::default();
        if let Err(err) = rows.Scan(&mut value) {
            // Go defer rows.Close 会在错误返回前执行；这里显式关闭用于标出资源收尾点。
            rows.Close();
            return Err(errors::Trace(err));
        }
        if value.Valid {
            randomValue.push(value.String);
        }
    }

    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok(randomValue)
}

// GetMinMaxValue return min and max value of given column by specified limitRange condition.
// GetMinMaxValue 执行 MIN/MAX 查询；空结果通过 ErrNoData 区分于 SQL 执行错误。
pub fn GetMinMaxValue(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schema: String,
    table: String,
    column: String,
    mut limitRange: String,
    limitArgs: Vec<any::Any>,
    mut collation: String,
) -> Result<(String, String), errors::Error> {
    /*
        example:
        mysql> SELECT MIN(`id`) as MIN, MAX(`id`) as MAX FROM `test`.`testa` WHERE id > 0 AND id < 10;
    */

    if limitRange.is_empty() {
        limitRange = "TRUE".to_string();
    }

    if !collation.is_empty() {
        collation = format!(" COLLATE \"{}\"", collation);
    }

    let query = format!(
        "SELECT /*!40001 SQL_NO_CACHE */
 MIN({}{}) as MIN, MAX({}{}) as MAX FROM {} WHERE {}",
        ColumnName(column.clone()),
        collation,
        ColumnName(column),
        collation,
        TableName(schema, table),
        limitRange,
    );
    log::Debug("GetMinMaxValue", zap::String("sql", query.clone()), zap::Reflect("args", &limitArgs));

    let mut minv = sql::NullString::default();
    let mut maxv = sql::NullString::default();
    let mut rows = db.QueryContext(ctx, query, limitArgs).map_err(errors::Trace)?;
    while rows.Next() {
        if let Err(err) = rows.Scan((&mut minv, &mut maxv)) {
            rows.Close();
            return Err(errors::Trace(err));
        }
    }

    if !minv.Valid || !maxv.Valid {
        // don't have any data
        rows.Close();
        return Err(ErrNoData.clone());
    }

    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok((minv.String, maxv.String))
}

// GetTimeZoneOffset is to get offset of timezone.
// GetTimeZoneOffset 读取 TiDB/MySQL 当前会话时区相对 UTC 的 time 字符串，并转换为 Duration。
pub fn GetTimeZoneOffset(
    ctx: context::Context,
    db: &dyn QueryExecutor,
) -> Result<time::Duration, errors::Error> {
    let mut timeStr = String::new();
    db.QueryRowContext(ctx, "SELECT cast(TIMEDIFF(NOW(6), UTC_TIMESTAMP(6)) as time);")
        .Scan(&mut timeStr)
        .map_err(errors::Trace)?;
    let mut factor = time::Duration::from(1);
    if timeStr.starts_with('-') || timeStr.starts_with('+') {
        if timeStr.starts_with('-') {
            factor *= -1;
        }
        timeStr = timeStr[1..].to_string();
    }
    let t = time::Parse(time::TimeOnly, timeStr).map_err(errors::Trace)?;

    if t.IsZero() {
        return Ok(time::Duration::from(0));
    }

    let (hour, minute, second) = t.Clock();
    //nolint:durationcheck
    Ok(time::Duration::from(hour * 3600 + minute * 60 + second) * time::Second * factor)
}

// FormatTimeZoneOffset is to format offset of timezone.
// FormatTimeZoneOffset 保留 Go 中正负号、小时和分钟的格式化逻辑。
pub fn FormatTimeZoneOffset(mut offset: time::Duration) -> String {
    let mut prefix = "+";
    if offset < time::Duration::from(0) {
        prefix = "-";
        offset *= -1;
    }
    let hours = offset / time::Hour;
    let minutes = (offset % time::Hour) / time::Minute;

    format!("{}{:02}:{:02}", prefix, hours, minutes)
}

// queryTables 是 GetTables/GetViews 的共享扫描函数，保留 SHOW FULL TABLES 的两列结果处理。
fn queryTables(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    q: String,
) -> Result<Vec<String>, errors::Error> {
    log::Debug("query tables", zap::String("query", q.clone()));
    let mut rows = db.QueryContext(ctx, q).map_err(errors::Trace)?;

    let mut tables: Vec<String> = Vec::with_capacity(8);
    while rows.Next() {
        let mut table = sql::NullString::default();
        let mut tType = sql::NullString::default();
        if let Err(err) = rows.Scan((&mut table, &mut tType)) {
            rows.Close();
            return Err(errors::Trace(err));
        }

        if !table.Valid || !tType.Valid {
            continue;
        }

        tables.push(table.String);
    }

    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok(tables)
}

// GetTables returns name of all tables in the specified schema
// GetTables 排除 VIEW，只返回 BASE TABLE 等普通表名。
pub fn GetTables(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
) -> Result<Vec<String>, errors::Error> {
    /*
        show tables without view: https://dev.mysql.com/doc/refman/5.7/en/show-tables.html
    */
    let query = format!(
        "SHOW FULL TABLES IN `{}` WHERE Table_Type != 'VIEW';",
        escapeName(schemaName)
    );
    queryTables(ctx, db, query)
}

// GetViews returns names of all views in the specified schema
// GetViews 与 GetTables 共用 queryTables，只把过滤条件改为 VIEW。
pub fn GetViews(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
) -> Result<Vec<String>, errors::Error> {
    let query = format!(
        "SHOW FULL TABLES IN `{}` WHERE Table_Type = 'VIEW';",
        escapeName(schemaName)
    );
    queryTables(ctx, db, query)
}

// GetSchemas returns name of all schemas
// GetSchemas 对 SHOW DATABASES 结果逐行扫描 schema 字符串。
pub fn GetSchemas(ctx: context::Context, db: &dyn QueryExecutor) -> Result<Vec<String>, errors::Error> {
    let query = "SHOW DATABASES";
    let mut rows = db.QueryContext(ctx, query).map_err(errors::Trace)?;

    /*
        mysql> SHOW DATABASES;
        +--------------------+
        | Database           |
        +--------------------+
        | information_schema |
        | mysql              |
        | performance_schema |
        | sys                |
        | test_db            |
        +--------------------+
    */
    let mut schemas: Vec<String> = Vec::with_capacity(10);
    while rows.Next() {
        let mut schema = String::new();
        if let Err(err) = rows.Scan(&mut schema) {
            rows.Close();
            return Err(errors::Trace(err));
        }
        schemas.push(schema);
    }
    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok(schemas)
}

// GetCRC32Checksum returns checksum code of some data by given condition
// GetCRC32Checksum 按表列拼接 CRC32/BIT_XOR 校验 SQL；空数据时 Go 返回 0 并记录 warn。
pub fn GetCRC32Checksum(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schemaName: String,
    tableName: String,
    tbInfo: *const model::TableInfo,
    limitRange: String,
    args: Vec<any::Any>,
) -> Result<i64, errors::Error> {
    /*
        calculate CRC32 checksum example:
        mysql> SELECT BIT_XOR(CAST(CRC32(CONCAT_WS(',', id, name, age, CONCAT(ISNULL(id), ...)))AS UNSIGNED)) ...
    */
    let mut columnNames: Vec<String> = Vec::with_capacity(unsafe { (*tbInfo).Columns.len() });
    let mut columnIsNull: Vec<String> = Vec::with_capacity(unsafe { (*tbInfo).Columns.len() });
    for col in unsafe { &(*tbInfo).Columns } {
        columnNames.push(ColumnName(col.Name.O.clone()));
        columnIsNull.push(format!("ISNULL({})", ColumnName(col.Name.O.clone())));
    }

    let query = format!(
        "SELECT BIT_XOR(CAST(CRC32(CONCAT_WS(',', {}, CONCAT({})))AS UNSIGNED)) AS checksum FROM {} WHERE {};",
        columnNames.join(", "),
        columnIsNull.join(", "),
        TableName(schemaName, tableName),
        limitRange,
    );
    log::Debug("checksum", zap::String("sql", query.clone()), zap::Reflect("args", &args));

    let mut checksum = sql::NullInt64::default();
    db.QueryRowContext(ctx, query.clone(), args)
        .Scan(&mut checksum)
        .map_err(errors::Trace)?;
    if !checksum.Valid {
        // if don't have any data, the checksum will be `NULL`
        log::Warn("get empty checksum", zap::String("sql", query), zap::Reflect("args", &args));
        return Ok(0);
    }

    Ok(checksum.Int64)
}

// Bucket saves the bucket information from TiDB.
// Bucket 保留 Go 字段顺序：LowerBound、UpperBound、Count。
pub struct Bucket {
    pub LowerBound: String,
    pub UpperBound: String,
    pub Count: i64,
}

// GetBucketsInfo SHOW STATS_BUCKETS in TiDB.
// GetBucketsInfo 扫描 SHOW STATS_BUCKETS 的多版本列布局，并把 PRIMARY 单列主键名称归一化。
pub fn GetBucketsInfo(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    schema: String,
    table: String,
    tableInfo: *const model::TableInfo,
) -> Result<HashMap<String, Vec<Bucket>>, errors::Error> {
    /*
        example in tidb:
        mysql> SHOW STATS_BUCKETS WHERE db_name= "test" AND table_name="testa";
    */
    let mut buckets: HashMap<String, Vec<Bucket>> = HashMap::new();
    let query = "SHOW STATS_BUCKETS WHERE db_name= ? AND table_name= ?;";
    log::Debug(
        "GetBucketsInfo",
        zap::String("sql", query.to_string()),
        zap::String("schema", schema.clone()),
        zap::String("table", table.clone()),
    );

    let mut rows = db.QueryContext(ctx, query, schema, table).map_err(errors::Trace)?;
    let cols = rows.Columns().map_err(errors::Trace)?;

    while rows.Next() {
        let mut dbName = sql::NullString::default();
        let mut tableName = sql::NullString::default();
        let mut partitionName = sql::NullString::default();
        let mut columnName = sql::NullString::default();
        let mut lowerBound = sql::NullString::default();
        let mut upperBound = sql::NullString::default();
        let mut isIndex = sql::NullInt64::default();
        let mut bucketID = sql::NullInt64::default();
        let mut count = sql::NullInt64::default();
        let mut repeats = sql::NullInt64::default();
        let mut ndv = sql::NullInt64::default();

        // add partiton_name in new version
        // TiDB 不同版本的 SHOW STATS_BUCKETS 列数不同；这里按 Go switch 保留三种 Scan 形状。
        let scan_result = match cols.len() {
            9 => rows.Scan((
                &mut dbName,
                &mut tableName,
                &mut columnName,
                &mut isIndex,
                &mut bucketID,
                &mut count,
                &mut repeats,
                &mut lowerBound,
                &mut upperBound,
            )),
            10 => rows.Scan((
                &mut dbName,
                &mut tableName,
                &mut partitionName,
                &mut columnName,
                &mut isIndex,
                &mut bucketID,
                &mut count,
                &mut repeats,
                &mut lowerBound,
                &mut upperBound,
            )),
            11 => rows.Scan((
                &mut dbName,
                &mut tableName,
                &mut partitionName,
                &mut columnName,
                &mut isIndex,
                &mut bucketID,
                &mut count,
                &mut repeats,
                &mut lowerBound,
                &mut upperBound,
                &mut ndv,
            )),
            _ => Err(errors::New("Unknown struct for buckets info")),
        };
        if let Err(err) = scan_result {
            rows.Close();
            return Err(errors::Trace(err));
        }

        buckets
            .entry(columnName.String.clone())
            .or_insert_with(|| Vec::with_capacity(100))
            .push(Bucket {
                Count: count.Int64,
                LowerBound: lowerBound.String,
                UpperBound: upperBound.String,
            });
    }

    // when primary key is int type, the columnName will be column's name, not `PRIMARY`, check and transform here.
    let indices = FindAllIndex(tableInfo);
    for index in indices {
        if index.Name.O != "PRIMARY" {
            continue;
        }
        if !buckets.contains_key(&index.Name.O) && index.Columns.len() == 1 {
            let pk_col = index.Columns[0].Name.O.clone();
            if !buckets.contains_key(&pk_col) {
                rows.Close();
                return Err(errors::NotFoundf(format!("primary key on {} in buckets info", pk_col)));
            }
            let value = buckets.remove(&pk_col).unwrap_or_default();
            buckets.insert(index.Name.O.clone(), value);
        }
    }

    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok(buckets)
}

// AnalyzeValuesFromBuckets analyze upperBound or lowerBound to string for each column.
// upperBound and lowerBound are looks like '(123, abc)' for multiple fields, or '123' for one field.
// AnalyzeValuesFromBuckets 保留对复合 bucket 字符串的拆分和时间类型解码逻辑。
pub fn AnalyzeValuesFromBuckets(
    valueString: String,
    cols: Vec<*const model::ColumnInfo>,
) -> Result<Vec<String>, errors::Error> {
    // FIXME: maybe some values contains '(', ')' or ', '
    let vStr = valueString.trim_matches(|c| c == '(' || c == ')').to_string();
    let mut values: Vec<String> = vStr.split(", ").map(|v| v.to_string()).collect();
    if values.len() != cols.len() {
        return Err(errors::Errorf(format!("analyze value {} failed", valueString)));
    }

    for (i, col) in cols.iter().enumerate() {
        if IsTimeTypeAndNeedDecode(unsafe { (*(*col)).GetType() }) {
            // check if values[i] is already a time string
            let parsed = types::ParseTime(
                types::DefaultStmtNoWarningContext,
                values[i].clone(),
                unsafe { (*(*col)).GetType() },
                types::MinFsp,
            );
            if parsed.is_ok() {
                continue;
            }

            let value = match DecodeTimeInBucket(values[i].clone()) {
                Ok(value) => value,
                Err(err) => {
                    log::Error(
                        "analyze values from buckets",
                        zap::String("column", unsafe { (*(*col)).Name.O.clone() }),
                        zap::String("value", values[i].clone()),
                        zap::Error(err.clone()),
                    );
                    return Err(errors::Trace(err));
                }
            };

            values[i] = value;
        }
    }

    Ok(values)
}

// DecodeTimeInBucket decodes Time from a packed uint64 value.
// DecodeTimeInBucket 把 TiDB bucket 中 packed uint64 时间值还原为字符串；0 表示空时间。
pub fn DecodeTimeInBucket(packedStr: String) -> Result<String, errors::Error> {
    let packed = strconv::ParseUint(packedStr, 10, 64)?;

    if packed == 0 {
        return Ok(String::new());
    }

    let mut t = types::Time::default();
    t.FromPackedUint(packed)?;

    Ok(t.String())
}

// GetTidbLatestTSO returns tidb's current TSO.
// GetTidbLatestTSO 读取 SHOW MASTER STATUS 的 Position 字段并按十进制解析 TSO。
pub fn GetTidbLatestTSO(
    ctx: context::Context,
    db: &dyn QueryExecutor,
) -> Result<i64, errors::Error> {
    /*
        example in tidb:
        mysql> SHOW MASTER STATUS;
    */
    let mut rows = db.QueryContext(ctx, "SHOW MASTER STATUS").map_err(errors::Trace)?;

    if rows.Next() {
        let fields = match ScanRow(&mut rows) {
            Ok(fields) => fields,
            Err(err) => {
                rows.Close();
                return Err(errors::Trace(err));
            }
        };

        let ts = match strconv::ParseInt(String::from_utf8_lossy(&fields["Position"].Data), 10, 64) {
            Ok(ts) => ts,
            Err(err) => {
                rows.Close();
                return Err(errors::Trace(err));
            }
        };
        rows.Close();
        return Ok(ts);
    }
    rows.Close();
    Err(errors::New("get secondary cluster's ts failed"))
}

// GetDBVersion returns the database's version
// GetDBVersion 执行 SELECT version() 并扫描首行；没有有效值时返回 ErrVersionNotFound。
pub fn GetDBVersion(ctx: context::Context, db: &dyn QueryExecutor) -> Result<String, errors::Error> {
    /*
        example in TiDB:
        mysql> select version();
    */
    let query = "SELECT version()";
    let mut result = db.QueryContext(ctx, query).map_err(errors::Trace)?; //nolint:rowserrcheck

    let mut version = sql::NullString::default();
    if result.Next() {
        if let Err(err) = result.Scan(&mut version) {
            result.Close();
            return Err(errors::Trace(err));
        }
    }

    result.Close();
    if version.Valid {
        return Ok(version.String);
    }

    Err(ErrVersionNotFound.clone())
}

// GetSessionVariable gets server's session variable, although argument is QueryExecutor, (session) system variables may be
// set through DSN
// GetSessionVariable 执行 SHOW VARIABLES LIKE，并把最后扫描到的 Value 返回。
pub fn GetSessionVariable(
    ctx: context::Context,
    db: &dyn QueryExecutor,
    mut variable: String,
) -> Result<String, errors::Error> {
    let query = format!("SHOW VARIABLES LIKE '{}'", variable);
    let mut rows = db.QueryContext(ctx, query).map_err(errors::Trace)?;
    let mut value = String::new();

    /*
        mysql> SHOW VARIABLES LIKE "binlog_format";
        +---------------+-------+
        | Variable_name | Value |
        +---------------+-------+
        | binlog_format | ROW   |
        +---------------+-------+
    */

    while rows.Next() {
        if let Err(err) = rows.Scan((&mut variable, &mut value)) {
            rows.Close();
            return Err(errors::Trace(err));
        }
    }

    let rowsErr = rows.Err();
    rows.Close();
    rowsErr.map_err(errors::Trace)?;
    Ok(value)
}

// GetSQLMode returns sql_mode.
// GetSQLMode 先读取 session sql_mode 字符串，再交给 parser/mysql 解析为 SQLMode。
pub fn GetSQLMode(
    ctx: context::Context,
    db: &dyn QueryExecutor,
) -> Result<tmysql::SQLMode, errors::Error> {
    let sqlMode = GetSessionVariable(ctx, db, "sql_mode".to_string())?;
    let mode = tmysql::GetSQLMode(sqlMode).map_err(errors::Trace)?;
    Ok(mode)
}

// IsTiDB returns true if this database is tidb
// IsTiDB 根据 version() 字符串是否包含 tidb 判断数据库类型。
pub fn IsTiDB(ctx: context::Context, db: &dyn QueryExecutor) -> Result<bool, errors::Error> {
    let version = match GetDBVersion(ctx, db) {
        Ok(version) => version,
        Err(err) => {
            log::Error("get database's version failed", zap::Error(err.clone()));
            return Err(errors::Trace(err));
        }
    };

    Ok(version.to_lowercase().contains("tidb"))
}

// TableName returns `schema`.`table`
// TableName 对 schema/table 分别转义反引号，然后补上 MySQL 标识符引号。
pub fn TableName(schema: String, table: String) -> String {
    format!("`{}`.`{}`", escapeName(schema), escapeName(table))
}

// ColumnName returns `column`
// ColumnName 对单列名补反引号。
pub fn ColumnName(column: String) -> String {
    format!("`{}`", escapeName(column))
}

// escapeName 对应 Go 的 strings.ReplaceAll(name, "`", "``")。
fn escapeName(name: String) -> String {
    name.replace('`', "``")
}

// ReplacePlaceholder will use args to replace '?', used for log.
// tips: make sure the num of "?" is same with len(args)
// ReplacePlaceholder 只用于日志展示，把 ? 替换为带引号的参数。
pub fn ReplacePlaceholder(str_: String, args: Vec<String>) -> String {
    /*
        for example:
        str is "a > ? AND a < ?", args is {'1', '2'},
        this function will return "a > '1' AND a < '2'"
    */
    let newStr = str_.replace('?', "'%s'");
    fmt::Sprintf(newStr, util::StringsToInterfaces(args))
}

// ExecSQLWithRetry executes sql with retry
// ExecSQLWithRetry 保留 Go 中慢日志、忽略错误、可重试错误和 ctx.Done 等待分支。
pub fn ExecSQLWithRetry(
    ctx: context::Context,
    db: &dyn DBExecutor,
    sql_text: String,
    args: Vec<any::Any>,
) -> Result<(), errors::Error> {
    let mut err: Option<errors::Error> = None;
    for i in 0..DefaultRetryTime {
        let startTime = time::Now();
        match db.ExecContext(ctx.clone(), sql_text.clone(), args.clone()) {
            Ok(_) => err = None,
            Err(e) => err = Some(e),
        }
        let takeDuration = time::Since(startTime);
        if takeDuration > *SlowLogThreshold {
            log::Debug(
                "exec sql slow",
                zap::String("sql", sql_text.clone()),
                zap::Reflect("args", &args),
                zap::Duration("take", takeDuration),
            );
        }
        if err.is_none() {
            return Ok(());
        }

        let current = err.clone().unwrap();
        if ignoreError(current.clone()) {
            log::Warn("ignore execute sql error", zap::Error(current));
            return Ok(());
        }

        if !IsRetryableError(current.clone()) {
            return Err(errors::Trace(current));
        }

        log::Warn(
            "exe sql failed, will try again",
            zap::String("sql", sql_text.clone()),
            zap::Reflect("args", &args),
            zap::Error(current.clone()),
        );

        if i == DefaultRetryTime - 1 {
            break;
        }

        // Go 使用 select 等待 ctx.Done 或 10ms 定时器；这里保留同样的取消优先语义。
        select! {
            _ = ctx.Done() => {
                return Err(errors::Trace(ctx.Err()));
            }
            _ = time::After(10 * time::Millisecond) => {}
        }
    }

    Err(errors::Trace(err.unwrap()))
}

// ExecuteSQLs executes some sqls in one transaction
// ExecuteSQLs 在一个事务里按顺序执行多条 SQL；任意失败则 Rollback，全部成功才 Commit。
pub fn ExecuteSQLs(
    ctx: context::Context,
    db: &dyn DBExecutor,
    sqls: Vec<String>,
    args: Vec<Vec<any::Any>>,
) -> Result<(), errors::Error> {
    let mut txn = match db.BeginTx(ctx.clone(), None) {
        Ok(txn) => txn,
        Err(err) => {
            log::Error("exec sqls begin", zap::Error(err.clone()));
            return Err(errors::Trace(err));
        }
    };

    for i in 0..sqls.len() {
        let startTime = time::Now();

        if let Err(err) = txn.ExecContext(ctx.clone(), sqls[i].clone(), args[i].clone()) {
            log::Error(
                "exec sql",
                zap::String("sql", sqls[i].clone()),
                zap::Reflect("args", &args[i]),
                zap::Error(err.clone()),
            );
            let rerr = txn.Rollback();
            if rerr.is_err() {
                // Go 原代码这里记录的是 err 而不是 rerr；保持这个日志参数形状。
                log::Error("rollback", zap::Error(err.clone()));
            }
            return Err(errors::Trace(err));
        }

        let takeDuration = time::Since(startTime);
        if takeDuration > *SlowLogThreshold {
            log::Debug(
                "exec sql slow",
                zap::String("sql", sqls[i].clone()),
                zap::Reflect("args", &args[i]),
                zap::Duration("take", takeDuration),
            );
        }
    }

    if let Err(err) = txn.Commit() {
        log::Error("exec sqls commit", zap::Error(err.clone()));
        return Err(errors::Trace(err));
    }

    Ok(())
}

// ignoreError 聚合当前可忽略错误类型；Go 目前只忽略部分 DDL 错误。
fn ignoreError(err: errors::Error) -> bool {
    // TODO: now only ignore some ddl error, add some dml error later
    ignoreDDLError(err)
}

// ignoreDDLError 识别建库/删库/建表/删表/列/索引已存在等 DDL 幂等错误。
fn ignoreDDLError(err: errors::Error) -> bool {
    let err = errors::Cause(err);
    let mysqlErr = match err.downcast_ref::<mysql::MySQLError>() {
        Some(mysqlErr) => mysqlErr,
        None => return false,
    };

    let errCode = errors::ErrCode(mysqlErr.Number);
    match errCode {
        code if code == infoschema::ErrDatabaseExists.Code()
            || code == infoschema::ErrDatabaseDropExists.Code()
            || code == infoschema::ErrTableExists.Code()
            || code == infoschema::ErrTableDropExists.Code()
            || code == infoschema::ErrColumnExists.Code()
            || code == infoschema::ErrIndexExists.Code() =>
        {
            true
        }
        code if code == dbterror::ErrDupKeyName.Code() => true,
        _ => false,
    }
}

// DeleteRows delete rows in several times. Only can delete less than 300,000 one time in TiDB.
// DeleteRows 按 DefaultDeleteRowsNum 分批递归删除；RowsAffected 小于批量大小时停止。
pub fn DeleteRows(
    ctx: context::Context,
    db: &dyn DBExecutor,
    schemaName: String,
    tableName: String,
    where_clause: String,
    args: Vec<any::Any>,
) -> Result<(), errors::Error> {
    let deleteSQL = format!(
        "DELETE FROM {} WHERE {} limit {};",
        TableName(schemaName.clone(), tableName.clone()),
        where_clause,
        DefaultDeleteRowsNum,
    );
    let result = db.ExecContext(ctx.clone(), deleteSQL, args.clone()).map_err(errors::Trace)?;

    let rows = result.RowsAffected().map_err(errors::Trace)?;

    if rows < DefaultDeleteRowsNum {
        return Ok(());
    }

    DeleteRows(ctx, db, schemaName, tableName, where_clause, args)
}

// getParser gets parser according to sql mode
// getParser 根据传入 sql_mode 初始化 parser；空字符串走 parser.New 默认模式。
fn getParser(sqlModeStr: String) -> Result<*mut parser::Parser, errors::Error> {
    if sqlModeStr.is_empty() {
        return Ok(parser::New());
    }

    let sqlMode = match tmysql::GetSQLMode(tmysql::FormatSQLModeStr(sqlModeStr.clone())) {
        Ok(sqlMode) => sqlMode,
        Err(err) => return Err(errors::Annotatef(err, format!("invalid sql mode {}", sqlModeStr))),
    };
    let parser2 = parser::New();
    unsafe { (*parser2).SetSQLMode(sqlMode) };
    Ok(parser2)
}

// GetParserForDB discovers ANSI_QUOTES in db's session variables and returns a proper parser
// GetParserForDB 从数据库会话读取 SQLMode 后创建 parser；真实数据库访问由 QueryExecutor 提供。
pub fn GetParserForDB(
    ctx: context::Context,
    db: &dyn QueryExecutor,
) -> Result<*mut parser::Parser, errors::Error> {
    let mode = GetSQLMode(ctx, db)?;

    let parser2 = parser::New();
    unsafe { (*parser2).SetSQLMode(mode) };
    Ok(parser2)
}
*/

// ========== 可执行实现：QueryExecutor/DBExecutor 版 dbutil 通用辅助 ==========
use std::collections::HashMap;
use std::env;
use std::fmt;
use std::time::Duration;

use astersql_infoschema::TableInfo;

use crate::interface::{DBExecutor, DbError, QueryExecutor, QueryResult, Value};
use crate::retry::IsRetryableError;
use crate::types::IsTimeTypeAndNeedDecode;

/// SQL 执行默认最大重试次数。
pub const DefaultRetryTime: usize = 10;
/// SQL 执行默认超时（10 秒）。
pub const DefaultTimeout: Duration = Duration::from_secs(10);
/// 超过该耗时的 SQL 可记慢日志（阈值保留，具体打日志由调用方决定）。
pub const SlowLogThreshold: Duration = Duration::from_millis(200);
/// 单次 DELETE 默认批量行数上限（TiDB 侧限制相关）。
pub const DefaultDeleteRowsNum: u64 = 100_000;

/// 数据库连接配置：主机、账号、库名、可选 tidb_snapshot 与端口。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct DBConfig {
    pub Host: String,
    pub User: String,
    pub Password: String,
    pub Schema: String,
    pub Snapshot: String,
    pub Port: u16,
}

/// 序列化为不含密码的 JSON 风格字符串，便于日志输出。
impl fmt::Display for DBConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{{\"host\":\"{}\",\"user\":\"{}\",\"schema\":\"{}\",\"snapshot\":\"{}\",\"port\":{}}}",
            json_escape(&self.Host),
            json_escape(&self.User),
            json_escape(&self.Schema),
            json_escape(&self.Snapshot),
            self.Port
        )
    }
}

fn json_escape(value: &str) -> String {
    let mut escaped = String::with_capacity(value.len());
    for character in value.chars() {
        match character {
            '"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            character if character.is_control() => {
                use std::fmt::Write;
                write!(escaped, "\\u{:04x}", character as u32).unwrap();
            }
            character => escaped.push(character),
        }
    }
    escaped
}

/// 从 `MYSQL_*` 环境变量读取配置，缺省为本机 root@3306。
pub fn GetDBConfigFromEnv(schema: &str) -> DBConfig {
    DBConfig {
        Host: env::var("MYSQL_HOST")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "127.0.0.1".to_owned()),
        Port: env::var("MYSQL_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .filter(|port| *port != 0)
            .unwrap_or(3306),
        User: env::var("MYSQL_USER")
            .ok()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "root".to_owned()),
        Password: env::var("MYSQL_PSWD").unwrap_or_default(),
        Schema: schema.to_owned(),
        Snapshot: String::new(),
    }
}

/// 拼装 MySQL DSN；可附加 `tidb_snapshot` 与会话变量（值带单引号转义）。
pub fn BuildDSN(config: &DBConfig, variables: &HashMap<String, String>) -> String {
    let mut params = vec!["charset=utf8mb4".to_owned()];
    // 历史读：通过 tidb_snapshot 会话变量绑定到指定快照。
    if !config.Snapshot.is_empty() {
        params.push(format!("tidb_snapshot={}", config.Snapshot));
    }
    let mut variables: Vec<_> = variables.iter().collect();
    variables.sort_by_key(|entry| entry.0);
    params.extend(
        variables
            .into_iter()
            .map(|(key, value)| format!("{key}='{}'", value.replace('\'', "\\'"))),
    );
    format!(
        "{}:{}@tcp({}:{})/{}/?{}",
        config.User,
        config.Password,
        config.Host,
        config.Port,
        config.Schema,
        params.join("&")
    )
}

/// 将查询单元格转为字符串；`Null` 返回 `None`。
fn as_string(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Bytes(value) => Some(String::from_utf8_lossy(value).into_owned()),
        Value::Bool(value) => Some(value.to_string()),
        Value::I64(value) => Some(value.to_string()),
        Value::U64(value) => Some(value.to_string()),
        Value::F64(value) => Some(value.to_string()),
    }
}
/// 将查询单元格转为 i64（含字符串解析路径）。
fn as_i64(value: &Value) -> Option<i64> {
    match value {
        Value::I64(value) => Some(*value),
        Value::U64(value) => i64::try_from(*value).ok(),
        _ => as_string(value)?.parse().ok(),
    }
}
/// 取结果集首行；无数据时返回“no data found in table”。
fn first_row(result: QueryResult) -> Result<Vec<Value>, DbError> {
    result.rows.into_iter().next().ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "no data found in table".to_owned(),
    })
}

/// 执行 `SHOW CREATE TABLE`，返回建表语句字符串。
pub fn GetCreateTableSQL(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
) -> Result<String, DbError> {
    let row = first_row(db.QueryContext(
        &format!("SHOW CREATE TABLE {}", TableName(schema, table)),
        &[],
    )?)?;
    row.get(1).and_then(as_string).ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: format!("table {table} not found"),
    })
}
/// 统计表行数；可选 WHERE 条件与绑定参数。
pub fn GetRowCount(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    where_clause: &str,
    args: &[Value],
) -> Result<i64, DbError> {
    let mut query = format!("SELECT COUNT(1) cnt FROM {}", TableName(schema, table));
    if !where_clause.is_empty() {
        query.push_str(" WHERE ");
        query.push_str(where_clause);
    }
    first_row(db.QueryContext(&query, args)?)?
        .first()
        .and_then(as_i64)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: format!("table `{schema}`.`{table}` not found"),
        })
}
/// 在限定范围内随机抽样列值，并按列值（可选 collation）排序返回。
pub fn GetRandomValues(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    column: &str,
    number: usize,
    limit_range: &str,
    limit_args: &[Value],
    collation: &str,
) -> Result<Vec<String>, DbError> {
    let range = if limit_range.is_empty() {
        "TRUE"
    } else {
        limit_range
    };
    let collate = if collation.is_empty() {
        String::new()
    } else {
        format!(" COLLATE \"{collation}\"")
    };
    let query = format!(
        "SELECT {0} FROM (SELECT {0}, rand() rand_value FROM {1} WHERE {2} ORDER BY rand_value LIMIT {3})rand_tmp ORDER BY {0}{4}",
        ColumnName(column),
        TableName(schema, table),
        range,
        number,
        collate
    );
    Ok(db
        .QueryContext(&query, limit_args)?
        .rows
        .into_iter()
        .filter_map(|row| row.first().and_then(as_string))
        .collect())
}
/// 查询指定列在范围内的 MIN/MAX；空表返回无数据错误。
pub fn GetMinMaxValue(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    column: &str,
    limit_range: &str,
    args: &[Value],
    collation: &str,
) -> Result<(String, String), DbError> {
    let range = if limit_range.is_empty() {
        "TRUE"
    } else {
        limit_range
    };
    let collate = if collation.is_empty() {
        String::new()
    } else {
        format!(" COLLATE \"{collation}\"")
    };
    let query = format!(
        "SELECT /*!40001 SQL_NO_CACHE */ MIN({0}{1}) as MIN, MAX({0}{1}) as MAX FROM {2} WHERE {3}",
        ColumnName(column),
        collate,
        TableName(schema, table),
        range
    );
    let row = first_row(db.QueryContext(&query, args)?)?;
    match (
        row.first().and_then(as_string),
        row.get(1).and_then(as_string),
    ) {
        (Some(min), Some(max)) => Ok((min, max)),
        _ => Err(DbError {
            code: 0,
            sql_state: None,
            message: "no data found in table".to_owned(),
        }),
    }
}

/// 解析 `±HH:MM[:SS]` 时区偏移为秒数包装类型。
pub fn ParseTimeZoneOffset(value: &str) -> Result<DurationOffset, String> {
    let (negative, value) = match value.as_bytes().first() {
        Some(b'-') => (true, &value[1..]),
        Some(b'+') => (false, &value[1..]),
        _ => (false, value),
    };
    let fields: Vec<&str> = value.split(':').collect();
    if fields.len() != 3 {
        return Err("invalid timezone offset".to_owned());
    }
    let hours: u64 = fields[0]
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    let minutes: u64 = fields[1]
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    let seconds_field = fields[2].split('.').next().unwrap_or_default();
    let seconds: u64 = seconds_field
        .parse()
        .map_err(|error| format!("invalid timezone offset: {error}"))?;
    if hours >= 24 || minutes >= 60 || seconds >= 60 {
        return Err("invalid timezone offset".to_owned());
    }
    let seconds = (hours * 3600 + minutes * 60 + seconds) as i64;
    Ok(DurationOffset(if negative { -seconds } else { seconds }))
}
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
/// 相对 UTC 的时区偏移（秒）。
pub struct DurationOffset(pub i64);
/// 通过 `TIMEDIFF(NOW(6), UTC_TIMESTAMP(6))` 读取会话时区偏移。
pub fn GetTimeZoneOffset(db: &dyn QueryExecutor) -> Result<DurationOffset, DbError> {
    let row = db.QueryRowContext(
        "SELECT cast(TIMEDIFF(NOW(6), UTC_TIMESTAMP(6)) as time);",
        &[],
    )?;
    let value = row.first().and_then(as_string).ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "timezone offset is NULL".to_owned(),
    })?;
    ParseTimeZoneOffset(&value).map_err(|message| DbError {
        code: 0,
        sql_state: None,
        message,
    })
}
/// 将偏移格式化为 `+HH:MM` / `-HH:MM`。
pub fn FormatTimeZoneOffset(offset: DurationOffset) -> String {
    let sign = if offset.0 < 0 { '-' } else { '+' };
    let seconds = offset.0.unsigned_abs();
    format!("{sign}{:02}:{:02}", seconds / 3600, seconds % 3600 / 60)
}

/// 执行返回表名列表的查询，取每行第一列。
fn queryTables(db: &dyn QueryExecutor, query: &str) -> Result<Vec<String>, DbError> {
    Ok(db
        .QueryContext(query, &[])?
        .rows
        .into_iter()
        .filter_map(|row| {
            match (
                row.first().and_then(as_string),
                row.get(1).and_then(as_string),
            ) {
                (Some(table), Some(_)) => Some(table),
                _ => None,
            }
        })
        .collect())
}
/// 列出 schema 下非 VIEW 的基表名。
pub fn GetTables(db: &dyn QueryExecutor, schema: &str) -> Result<Vec<String>, DbError> {
    queryTables(
        db,
        &format!(
            "SHOW FULL TABLES IN `{}` WHERE Table_Type != 'VIEW';",
            escapeName(schema)
        ),
    )
}
/// 列出 schema 下的视图名。
pub fn GetViews(db: &dyn QueryExecutor, schema: &str) -> Result<Vec<String>, DbError> {
    queryTables(
        db,
        &format!(
            "SHOW FULL TABLES IN `{}` WHERE Table_Type = 'VIEW';",
            escapeName(schema)
        ),
    )
}
/// 执行 `SHOW DATABASES` 返回全部库名。
pub fn GetSchemas(db: &dyn QueryExecutor) -> Result<Vec<String>, DbError> {
    Ok(db
        .QueryContext("SHOW DATABASES", &[])?
        .rows
        .into_iter()
        .filter_map(|row| row.first().and_then(as_string))
        .collect())
}

/// 按列拼接计算 `BIT_XOR(CRC32(...))` 数据校验和；空结果按 0 处理。
pub fn GetCRC32Checksum(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    table_info: &TableInfo,
    limit_range: &str,
    args: &[Value],
) -> Result<i64, DbError> {
    let names: Vec<_> = table_info
        .columns
        .iter()
        .map(|column| ColumnName(&column.name.original))
        .collect();
    let nulls: Vec<_> = names.iter().map(|name| format!("ISNULL({name})")).collect();
    let query = format!(
        "SELECT BIT_XOR(CAST(CRC32(CONCAT_WS(',', {}, CONCAT({})))AS UNSIGNED)) AS checksum FROM {} WHERE {};",
        names.join(", "),
        nulls.join(", "),
        TableName(schema, table),
        limit_range
    );
    Ok(first_row(db.QueryContext(&query, args)?)?
        .first()
        .and_then(as_i64)
        .unwrap_or(0))
}

/// TiDB `SHOW STATS_BUCKETS` 中一个直方图桶：上下界与计数。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct Bucket {
    pub LowerBound: String,
    pub UpperBound: String,
    pub Count: i64,
}
/// 读取指定表的统计信息桶，按列名聚合为 `Bucket` 列表。
pub fn GetBucketsInfo(
    db: &dyn QueryExecutor,
    schema: &str,
    table: &str,
    table_info: &TableInfo,
) -> Result<HashMap<String, Vec<Bucket>>, DbError> {
    let result = db.QueryContext(
        "SHOW STATS_BUCKETS WHERE db_name= ? AND table_name= ?;",
        &[schema.into(), table.into()],
    )?;
    let index = |name: &str| {
        result
            .columns
            .iter()
            .position(|column| column.eq_ignore_ascii_case(name))
    };
    let column_index = index("Column_name").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Column_name column missing".to_owned(),
    })?;
    let count_index = index("Count").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Count column missing".to_owned(),
    })?;
    let lower_index = index("Lower_Bound").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Lower_Bound column missing".to_owned(),
    })?;
    let upper_index = index("Upper_Bound").ok_or_else(|| DbError {
        code: 0,
        sql_state: None,
        message: "Upper_Bound column missing".to_owned(),
    })?;
    let mut buckets: HashMap<String, Vec<Bucket>> = HashMap::new();
    for row in result.rows {
        let column = row
            .get(column_index)
            .and_then(as_string)
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: "Column_name is NULL".to_owned(),
            })?;
        let bucket = Bucket {
            Count: row
                .get(count_index)
                .and_then(as_i64)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Count is NULL or invalid".to_owned(),
                })?,
            LowerBound: row
                .get(lower_index)
                .and_then(as_string)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Lower_Bound is NULL".to_owned(),
                })?,
            UpperBound: row
                .get(upper_index)
                .and_then(as_string)
                .ok_or_else(|| DbError {
                    code: 0,
                    sql_state: None,
                    message: "Upper_Bound is NULL".to_owned(),
                })?,
        };
        buckets.entry(column).or_default().push(bucket);
    }

    // TiDB reports an integer primary-key bucket under the column name rather
    // than `PRIMARY`. Match Go by normalizing that key after scanning rows.
    let has_primary = table_info
        .indices
        .iter()
        .any(|index| index.name.original == "PRIMARY")
        || table_info
            .model_meta
            .as_deref()
            .is_some_and(|meta| meta.Indices.iter().any(|index| index.Primary));
    if has_primary && !buckets.contains_key("PRIMARY") {
        let primary_column = table_info
            .model_meta
            .as_deref()
            .and_then(|meta| meta.Indices.iter().find(|index| index.Primary))
            .and_then(|index| index.Columns.first())
            .map(|column| column.Name.O.clone())
            .or_else(|| {
                (table_info.columns.len() == 1).then(|| table_info.columns[0].name.original.clone())
            })
            .ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: "primary key column metadata missing".to_owned(),
            })?;
        let primary_buckets = buckets.remove(&primary_column).ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: format!("primary key on {primary_column} in buckets info not found"),
        })?;
        buckets.insert("PRIMARY".to_owned(), primary_buckets);
    }
    Ok(buckets)
}

/// 拆分桶边界字符串；时间类型若为 packed 整数则解码为可读时间。
pub fn AnalyzeValuesFromBuckets(value: &str, column_types: &[u8]) -> Result<Vec<String>, String> {
    let mut values: Vec<String> = value
        .trim_matches(['(', ')'])
        .split(", ")
        .map(str::to_owned)
        .collect();
    if values.len() != column_types.len() {
        return Err(format!("analyze value {value} failed"));
    }
    // 可读时间串已含 -/:；纯数字则按 packed 时间解码。
    for (value, column_type) in values.iter_mut().zip(column_types) {
        if IsTimeTypeAndNeedDecode(*column_type) && !is_time_string(value) {
            *value = DecodeTimeInBucket(value)?;
        }
    }
    Ok(values)
}

fn is_time_string(value: &str) -> bool {
    let mut parts = value.split([' ', ':', '-']);
    let components: Vec<_> = parts.by_ref().filter(|part| !part.is_empty()).collect();
    (components.len() == 3 || components.len() == 6)
        && components
            .iter()
            .all(|part| part.chars().all(|c| c.is_ascii_digit()))
}
/// 将 TiDB 桶中 packed uint64 时间还原为 `YYYY-MM-DD HH:MM:SS[.us]`。
pub fn DecodeTimeInBucket(value: &str) -> Result<String, String> {
    let packed: u64 = value
        .parse()
        .map_err(|error| format!("invalid packed time: {error}"))?;
    if packed == 0 {
        return Ok(String::new());
    }
    let year_month = packed >> 46;
    let year = year_month / 13;
    let month = year_month % 13;
    let day = (packed >> 41) & 31;
    let hour = (packed >> 36) & 31;
    let minute = (packed >> 30) & 63;
    let second = (packed >> 24) & 63;
    let microsecond = packed & ((1 << 24) - 1);
    if microsecond == 0 {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}"
        ))
    } else {
        Ok(format!(
            "{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}.{microsecond:06}"
        ))
    }
}

/// 从 `SHOW MASTER STATUS` 的 Position 读取 TiDB 最新 TSO（时间戳预言）。
pub fn GetTidbLatestTSO(db: &dyn QueryExecutor) -> Result<i64, DbError> {
    let result = db.QueryContext("SHOW MASTER STATUS", &[])?;
    let index = result
        .columns
        .iter()
        .position(|column| column == "Position")
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "Position column missing".to_owned(),
        })?;
    result
        .rows
        .first()
        .and_then(|row| row.get(index))
        .and_then(as_i64)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "get secondary cluster's ts failed".to_owned(),
        })
}
/// 执行 `SELECT version()` 返回数据库版本字符串。
pub fn GetDBVersion(db: &dyn QueryExecutor) -> Result<String, DbError> {
    first_row(db.QueryContext("SELECT version()", &[])?)?
        .first()
        .and_then(as_string)
        .ok_or_else(|| DbError {
            code: 0,
            sql_state: None,
            message: "can't get the database's version".to_owned(),
        })
}
/// `SHOW VARIABLES LIKE` 读取会话/系统变量值。
pub fn GetSessionVariable(db: &dyn QueryExecutor, variable: &str) -> Result<String, DbError> {
    let result = db.QueryContext(
        &format!("SHOW VARIABLES LIKE '{}'", variable.replace('\'', "''")),
        &[],
    )?;
    let mut value = String::new();
    for row in result.rows {
        if let Some(column) = row.get(1) {
            value = as_string(column).ok_or_else(|| DbError {
                code: 0,
                sql_state: None,
                message: format!("variable {variable} has invalid value"),
            })?;
        }
    }
    Ok(value)
}
/// 读取会话 `sql_mode`。
pub fn GetSQLMode(db: &dyn QueryExecutor) -> Result<String, DbError> {
    GetSessionVariable(db, "sql_mode")
}
/// 根据 version() 是否包含 `tidb` 判断是否为 TiDB。
pub fn IsTiDB(db: &dyn QueryExecutor) -> Result<bool, DbError> {
    Ok(GetDBVersion(db)?.to_ascii_lowercase().contains("tidb"))
}

/// 生成 `` `schema`.`table` ``，内部反引号翻倍转义。
pub fn TableName(schema: &str, table: &str) -> String {
    format!("`{}`.`{}`", escapeName(schema), escapeName(table))
}
/// 生成 `` `column` `` 标识符。
pub fn ColumnName(column: &str) -> String {
    format!("`{}`", escapeName(column))
}
/// MySQL 标识符转义：将 `` ` `` 替换为 `` `` ``。
pub fn escapeName(name: &str) -> String {
    name.replace('`', "``")
}
/// 日志用：把 SQL 中的 `?` 依次替换为带单引号的参数。
pub fn ReplacePlaceholder(template: &str, args: &[String]) -> String {
    let mut parts = template.split('?');
    let mut result = parts.next().unwrap_or_default().to_owned();
    for (index, part) in parts.enumerate() {
        if let Some(value) = args.get(index) {
            result.push('\'');
            result.push_str(value);
            result.push('\'');
        } else {
            result.push('?');
        }
        result.push_str(part);
    }
    result
}

/// 识别库/表/列/索引已存在等可幂等忽略的 DDL 错误码。
fn ignoreDDLError(error: &DbError) -> bool {
    matches!(error.code, 1007 | 1008 | 1050 | 1051 | 1060 | 1061)
}
fn ignoreError(error: &DbError) -> bool {
    ignoreDDLError(error)
}
/// 带重试执行 SQL；可忽略 DDL 错误直接成功，可重试错误短暂休眠后重试。
pub fn ExecSQLWithRetry(db: &dyn DBExecutor, sql: &str, args: &[Value]) -> Result<(), DbError> {
    let mut last = None;
    for attempt in 0..DefaultRetryTime {
        match db.ExecContext(sql, args) {
            Ok(_) => return Ok(()),
            Err(error) if ignoreError(&error) => return Ok(()),
            Err(error) if IsRetryableError(&error) => {
                last = Some(error);
                if attempt + 1 < DefaultRetryTime {
                    std::thread::sleep(Duration::from_millis(10));
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(last.expect("retry loop always records an error"))
}
/// 在同一事务中顺序执行多条 SQL；任一步失败则 Rollback。
pub fn ExecuteSQLs(
    db: &dyn DBExecutor,
    sqls: &[String],
    args: &[Vec<Value>],
) -> Result<(), DbError> {
    if sqls.len() != args.len() {
        return Err(DbError {
            code: 0,
            sql_state: None,
            message: "sql and args length mismatch".to_owned(),
        });
    }
    let mut transaction = db.BeginTx()?;
    for (sql, args) in sqls.iter().zip(args) {
        if let Err(error) = transaction.ExecContext(sql, args) {
            let _ = transaction.Rollback();
            return Err(error);
        }
    }
    transaction.Commit()
}
/// 按 `DefaultDeleteRowsNum` 分批 DELETE，直到影响行数小于批量上限。
pub fn DeleteRows(
    db: &dyn DBExecutor,
    schema: &str,
    table: &str,
    where_clause: &str,
    args: &[Value],
) -> Result<(), DbError> {
    let sql = format!(
        "DELETE FROM {} WHERE {} limit {};",
        TableName(schema, table),
        where_clause,
        DefaultDeleteRowsNum
    );
    // 影响行数不足一批说明删完；否则继续下一轮 LIMIT 删除。
    loop {
        if db.ExecContext(&sql, args)? < DefaultDeleteRowsNum {
            return Ok(());
        }
    }
}

/// 轻量 parser 配置：目前仅缓存规范化后的 sql_mode 字符串。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ParserConfig {
    pub sql_mode: String,
}
/// 规范化逗号分隔的 sql_mode 列表并封装为 `ParserConfig`。
pub fn getParser(sql_mode: &str) -> Result<ParserConfig, String> {
    const KNOWN_MODES: &[&str] = &[
        "REAL_AS_FLOAT",
        "PIPES_AS_CONCAT",
        "ANSI_QUOTES",
        "IGNORE_SPACE",
        "ONLY_FULL_GROUP_BY",
        "NO_UNSIGNED_SUBTRACTION",
        "NO_DIR_IN_CREATE",
        "POSTGRESQL",
        "ORACLE",
        "MSSQL",
        "DB2",
        "MAXDB",
        "NO_KEY_OPTIONS",
        "NO_TABLE_OPTIONS",
        "NO_FIELD_OPTIONS",
        "MYSQL323",
        "MYSQL40",
        "ANSI",
        "NO_AUTO_VALUE_ON_ZERO",
        "NO_BACKSLASH_ESCAPES",
        "STRICT_TRANS_TABLES",
        "STRICT_ALL_TABLES",
        "NO_ZERO_IN_DATE",
        "NO_ZERO_DATE",
        "INVALID_DATES",
        "ERROR_FOR_DIVISION_BY_ZERO",
        "TRADITIONAL",
        "NO_AUTO_CREATE_USER",
        "HIGH_NOT_PRECEDENCE",
        "NO_ENGINE_SUBSTITUTION",
        "PAD_CHAR_TO_FULL_LENGTH",
        "ALLOW_INVALID_DATES",
        "TIME_TRUNCATE_FRACTIONAL",
    ];
    let mut modes = Vec::new();
    for mode in sql_mode
        .split(',')
        .map(str::trim)
        .filter(|mode| !mode.is_empty())
    {
        let mode = mode.to_ascii_uppercase();
        if !KNOWN_MODES.contains(&mode.as_str()) {
            return Err(format!("invalid sql mode {sql_mode}"));
        }
        if !modes.contains(&mode) {
            modes.push(mode);
        }
    }
    Ok(ParserConfig {
        sql_mode: modes.join(","),
    })
}
/// 从数据库读取 sql_mode 后构造 `ParserConfig`。
pub fn GetParserForDB(db: &dyn QueryExecutor) -> Result<ParserConfig, DbError> {
    Ok(ParserConfig {
        sql_mode: GetSQLMode(db)?,
    })
}
