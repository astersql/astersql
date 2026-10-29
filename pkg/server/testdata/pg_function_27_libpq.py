# Copyright 2026 AsterSQL.
"""Exclusive-function native client evidence; no equivalence exemptions.

Arguments: PG_PORT DATABASE EVIDENCE MYSQL_PORT; --self-test is offline;
--reference-only EVIDENCE exercises independent references without approval.
Uses task23 PG18_REFERENCE_DSN/PG_LIBPQ_LIBRARY, task24 MYSQL8_REFERENCE_*
and MYSQL8_CLIENT_LIBRARY. PG_FUNCTION27_BUILD_INFO comes from the Rust
service's GetTiDBInfo(), never from a reference build. Credentials are not saved.
"""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import struct
import subprocess
import sys

from pg_function_23_libpq import Libpq, Wire, compare_wire
from pg_function_24_libpq import Mysql as NativeMysql
from pg_function_23_libpq import MysqlBind
from pg_function_25_libpq import mysql_compare, pg_compare, self_test


class Mysql(NativeMysql):
    """Task24's verified C statement ABI, extended to zero or tuple parameters."""
    def prepared(self, sql, values):
        """Real COM_STMT_PREPARE/BIND/EXECUTE using the independent native client."""
        stmt = self.mysql_stmt_init(self.conn)
        assert stmt
        observations = []
        try:
            rc = self.mysql_stmt_prepare(stmt, sql.encode(), len(sql.encode()))
            result = self.mysql_stmt_result_metadata(stmt) if rc == 0 else None
            try:
                meta = self.metadata(result)
            finally:
                if result:
                    self.mysql_free_result(result)
            observations.append(dict(phase='prepare', code=self.mysql_stmt_errno(stmt),
                                     state=self.mysql_stmt_sqlstate(stmt).decode(), metadata=meta,
                                     parameters=self.mysql_stmt_param_count(stmt),
                                     warning_count=self.mysql_warning_count(self.conn),
                                     warnings=self.query('SHOW WARNINGS', warnings=False)['rows']))
            if rc:
                return observations
            for value in values:
                values_tuple = value if isinstance(value, tuple) else (value,)
                assert len(values_tuple) == self.mysql_stmt_param_count(stmt)
                params = (MysqlBind * len(values_tuple))()
                keepalive = []
                for i, item in enumerate(values_tuple):
                    length, null = c.c_ulong(0 if item is None else len(item)), c.c_bool(item is None)
                    buffer = c.create_string_buffer(item or b'')
                    keepalive.append((length, null, buffer))
                    params[i].buffer_type = 253
                    params[i].buffer = c.cast(buffer, c.c_void_p)
                    params[i].buffer_length = length.value
                    params[i].length, params[i].is_null = c.addressof(length), c.addressof(null)
                if values_tuple:
                    assert not self.mysql_stmt_bind_param(stmt, params)
                rc = self.mysql_stmt_execute(stmt)
                obs = dict(phase='execute', input=[None if item is None else item.hex() for item in values_tuple],
                           code=self.mysql_stmt_errno(stmt), state=self.mysql_stmt_sqlstate(stmt).decode(),
                           rows=[], metadata=[], warning_count=self.mysql_warning_count(self.conn))
                if rc == 0:
                    result = self.mysql_stmt_result_metadata(stmt)
                    try:
                        obs['metadata'] = self.metadata(result)
                    finally:
                        if result:
                            self.mysql_free_result(result)
                    buffers = [c.create_string_buffer(4096) for _ in obs['metadata']]
                    lengths = [c.c_ulong() for _ in buffers]
                    nulls = [c.c_bool() for _ in buffers]
                    errors = [c.c_bool() for _ in buffers]
                    binds = (MysqlBind * len(buffers))()
                    for i in range(len(buffers)):
                        binds[i].buffer_type = 253
                        binds[i].buffer = c.cast(buffers[i], c.c_void_p)
                        binds[i].buffer_length = len(buffers[i])
                        binds[i].length, binds[i].is_null = c.addressof(lengths[i]), c.addressof(nulls[i])
                        binds[i].error = c.addressof(errors[i])
                    assert not self.mysql_stmt_bind_result(stmt, binds)
                    stored = self.mysql_stmt_store_result(stmt)
                    if stored:
                        # Native execute may receive metadata before evaluation fails
                        # during result buffering. Keep the actual descriptor and stage.
                        obs['code'] = self.mysql_stmt_errno(stmt)
                        obs['state'] = self.mysql_stmt_sqlstate(stmt).decode()
                        obs['warning_count'] = self.mysql_warning_count(self.conn)
                        self.mysql_stmt_free_result(stmt)
                        obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
                        observations.append(obs)
                        continue
                    while True:
                        rc = self.mysql_stmt_fetch(stmt)
                        if rc == 100:  # MYSQL_NO_DATA
                            break
                        assert rc == 0 and not any(e.value for e in errors), 'native result truncation/error'
                        obs['rows'].append([None if nulls[i].value else buffers[i].raw[:lengths[i].value].hex()
                                            for i in range(len(buffers))])
                    assert not self.mysql_stmt_free_result(stmt)
                # EOF diagnostics become visible only after buffering/fetching.
                # Preserve the header-stage count independently; never replace
                # either raw count with the length of SHOW WARNINGS.
                obs['execute_header_warning_count'] = obs['warning_count']
                obs['warning_count'] = self.mysql_warning_count(self.conn)
                obs['warnings'] = self.query('SHOW WARNINGS', warnings=False)['rows']
                observations.append(obs)
            return observations
        finally:
            assert not self.mysql_stmt_close(stmt)

# Exact source18-21 PG calls. Dollar parameters are tested only at Parse.
PG_CALLS = (
    ('i-value', "ifnull('ok'::char(8),'backup'::char(12))"),
    ('i-null', "ifnull(NULL::char(8),'backup'::char(12))"),
    ('i-both-null', 'ifnull(NULL::char(8),NULL::char(12))'),
    ('i-invalid', "ifnull('bad'::int4,2)"),
    ('i-short', 'ifnull(7::int4,1/0)'),
    ('i-param', 'ifnull($1::text,$2::text)'),
    ('d-value', "date_format('2026-01-02','%Y-%m-%d')"),
    ('d-null', "date_format(NULL::text,'%Y')"),
    ('d-mask-null', "date_format('2026-01-02',NULL::text)"),
    ('d-empty', "date_format('2026-01-02','')"),
    ('d-invalid', "date_format('bad'::int4,'%Y')"),
    ('d-zero', "date_format('0000-00-00','%Y')"),
    ('d-param', 'date_format($1::text,$2::text)'),
    ('j-value', '''json_extract('{"a":1}','$.a')'''),
    ('j-null', "json_extract(NULL::text,'$.a')"),
    ('j-path-null', "json_extract('{}',NULL::text)"),
    ('j-invalid', "json_extract('bad'::int4,'$InvalidPath')"),
    ('j-path-invalid', "json_extract('{}','$InvalidPath')"),
    ('j-json-null', "json_extract('null','$')"),
    ('j-arity', "json_extract('{}')"),
    ('j-param', 'json_extract($1::text,$2::text)'),
    ('v-value', 'tidb_version()'), ('v-null', 'tidb_version(NULL)'),
    ('v-invalid', "tidb_version('bad'::int4)"),
    ('v-param', 'tidb_version($1::text)'),
)
MYSQL_CASES = (
    ('i-value', "IFNULL(CAST('ok' AS CHAR(8)),CAST('backup' AS CHAR(12)))"),
    ('i-null', "IFNULL(CAST(NULL AS CHAR(8)),CAST('backup' AS CHAR(12)))"),
    ('i-both-null', 'IFNULL(CAST(NULL AS CHAR(8)),CAST(NULL AS CHAR(12)))'),
    ('i-untyped-null', 'IFNULL(NULL,NULL)'),
    ('i-empty', "IFNULL(CAST('' AS CHAR(8)),CAST('backup' AS CHAR(12)))"),
    ('i-unicode', "IFNULL(CAST('你好' AS CHAR(8)),CAST('backup' AS CHAR(12)))"),
    ('i-mixed', "IFNULL(CAST(7 AS SIGNED),CAST('backup' AS CHAR(12)))"),
    ('d-value', "DATE_FORMAT('2026-01-02','%Y-%m-%d')"),
    ('d-null', "DATE_FORMAT(NULL,'%Y')"),
    ('d-mask-null', "DATE_FORMAT('2026-01-02',NULL)"),
    ('d-empty', "DATE_FORMAT('2026-01-02','')"),
    ('d-mask-wide', "DATE_FORMAT('2026-01-02','%r%r')"),
    ('d-num-zero', "DATE_FORMAT(0,'%Y')"), ('d-dec-zero', "DATE_FORMAT(0.0,'%Y')"),
    ('d-zero-mask', "DATE_FORMAT('0000-00-00','0')"),
    ('d-short', "DATE_FORMAT(NULL,ABS(CAST('-9223372036854775808' AS SIGNED)))"),
    ('d-first-error', "DATE_FORMAT(ABS(CAST('-9223372036854775808' AS SIGNED)),NULL)"),
    ('d-mask-error', "DATE_FORMAT('2026-01-02',ABS(CAST('-9223372036854775808' AS SIGNED)))"),
) + tuple((f'd-invalid-{i}', f"DATE_FORMAT('{value}','%Y-%m-%d')") for i,value in enumerate(('bad','0000-00-00','2026-00-02','2026-02-31'))) + (
    ('j-value', '''JSON_EXTRACT('{"a":1}','$.a')'''),
    ('j-json-null', '''JSON_EXTRACT('{"a":null}','$.a')'''),
    ('j-missing', "JSON_EXTRACT('{}','$.missing')"),
    ('j-null', "JSON_EXTRACT(NULL,'$.a')"), ('j-path-null', "JSON_EXTRACT('{}',NULL)"),
    ('j-both-null', 'JSON_EXTRACT(NULL,NULL)'), ('j-root-null', "JSON_EXTRACT('null','$')"),
    ('j-multi', '''JSON_EXTRACT('{"a": [{"aa": [{"aaa": 1}]}], "aaa": 2}', '$.a[0].aa[0].aaa', '$.aaa')'''),
    ('j-num', "JSON_EXTRACT(1,'$')"), ('j-invalid', "JSON_EXTRACT('bad','$.a')"),
    ('j-path-invalid', "JSON_EXTRACT('{}','$InvalidPath')"),
    ('j-invalid-before-null', "JSON_EXTRACT('{}','$InvalidPath',NULL)"),
    ('j-second-path-invalid', '''JSON_EXTRACT('{"a":1}','$.a','$InvalidPath')'''),
    ('j-arity', "JSON_EXTRACT('{}')"),
    ('i-arity-zero', 'IFNULL()'), ('i-arity-one', 'IFNULL(1)'), ('i-arity-three', 'IFNULL(1,2,3)'),
    ('d-arity', "DATE_FORMAT('2026-01-02')"),
    ('v-value', 'TIDB_VERSION()'), ('v-arity', 'TIDB_VERSION(NULL)'),
)
MASK = '%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %U %u %V %v %a %W %w %X %x %Y %y %%'
MYSQL_CASES += tuple((f'd-go-{i}', f"DATE_FORMAT('{date}','{mask}')") for i,date,mask in (
    (0,'2010-01-07 23:12:34.12345',MASK), (1,'2012-12-21 23:12:34.123456',MASK),
    (2,'0000-01-01 00:00:00.123456','%b %M %m %c %D %d %e %j %k %h %i %p %r %T %s %f %v %x %Y %y %%'),
    (3,'2016-09-3 00:59:59.123456','abc'+MASK.replace(' %y %%', ' %y!123 %%xyz %z')),
    (4,'2012-10-01 00:00:00','%b %M %m %c %D %d %e %j %k %H %i %p %r %T %s %f %v %x %Y %y %%'),
))
MYSQL_EXTENDED = (
    ('i-pair', 'IFNULL(CAST(? AS CHAR(8)),CAST(? AS CHAR(12)))',
     ((b'ok',b'backup'),(None,b'backup'),(None,None),(b'',b'backup'),('你好'.encode(),None),(None,b'again'))),
    ('d-pair', 'DATE_FORMAT(CAST(? AS CHAR(26)),CAST(? AS CHAR(8)))',
     ((b'2026-01-02',b'%Y-%m-%d'),(None,b'%Y'),(b'2026-01-02',None),(b'2026-01-02',b''),(b'2026-01-02',b'%Y'),(b'bad',b'%Y'),(b'2026-01-02',b'%Y'))),
    ('j-pair', 'JSON_EXTRACT(?,?)',
     ((b'{"a":1}',b'$.a'),(b'{"a":null}',b'$.a'),(b'{}',b'$.missing'),(None,b'$.a'),(b'{}',None),(b'{"a":2}',b'$.a'),(b'bad',b'$.a'),(b'{}',b'$InvalidPath'),(b'{"a":2}',b'$.a'))),
    ('v-zero', 'TIDB_VERSION()', ((),(),())),
    ('v-arity', 'TIDB_VERSION(?)', ((None,),)),
)


def build_contract(obs, expected=None, empty=False):
    """Different builds have distinct bytes and lengths; each must describe its own."""
    assert obs['code'] == 0 and obs['state'] == '00000'
    if 'parameters' in obs:
        assert obs['parameters'] == 0
    assert obs.get('warning_count', 0) == 0 and obs.get('warnings', []) == []
    assert len(obs['metadata']) == 1
    name, code, flags, charset, length, decimals = obs['metadata'][0]
    assert (name, code, flags, charset, decimals) == ('v', 253, 1, 46, 31)
    if expected is not None:
        assert length == len(expected)*4
        if 'rows' in obs:
            assert obs['rows'] == ([] if empty else [[expected.hex()]])
    elif 'rows' in obs and not empty:
        assert len(obs['rows']) == 1 and obs['rows'][0][0]
        assert length == len(bytes.fromhex(obs['rows'][0][0]))*4


def negative_controls():
    total = self_test()
    expected = b'own service build'
    original = dict(code=0, state='00000', metadata=[['v',253,1,46,len(expected)*4,31]],
                    rows=[[expected.hex()]], warning_count=0, warnings=[])
    for key in ('rows','metadata','state','warnings'):
        changed = copy.deepcopy(original)
        changed[key] = '22003' if key == 'state' else [] if key != 'warnings' else [['corruption']]
        try:
            build_contract(changed, expected)
        except AssertionError:
            total += 1
        else:
            raise AssertionError('build contract accepted '+key+' corruption')
    return total


def run(dsns, evidence, aster_mysql=None):
    evidence.mkdir(parents=True, exist_ok=False)
    pq = Libpq()
    conns, wires, mysqls, records, failures = {}, {}, {}, {}, []
    counts = dict(assertions=0, result_scenarios=0, interleaved_probes=0)
    table = f'function27_guard_{os.getpid()}'
    wire_table = table+'_wire'
    owned_pg, reference_database, cleanup = [], None, {}
    expected_build = os.environ.get('PG_FUNCTION27_BUILD_INFO', '').encode()

    def check(ok, label):
        counts['assertions'] += 1
        if not ok:
            failures.append(label)

    def archive(domain, side, case, phase, obs):
        peers = records.setdefault((domain,case,phase), {})
        assert side not in peers
        peers[side] = obs
        if domain == 'mysql' and obs.get('phase') == 'execute' and obs['code'] == 0:
            check(obs['warning_count'] == len(obs['warnings']), f'{side}/{case}/{phase}: completed warning count')
        with (evidence/f'{domain}-{side}.jsonl').open('a') as stream:
            stream.write(json.dumps(dict(case_id=case, phase=phase, observation=obs))+'\n')
        return obs

    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))

    def probe(label):
        for side,conn in conns.items():
            obs = archive('pg-probe',side,label,'query',query(conn,'SELECT 1::int4 AS v'))
            check(obs['status'] == 2 and obs['rows'] == [['31']], f'{side}/{label}: PG recovery')
            counts['interleaved_probes'] += 1
        for side,mysql in mysqls.items():
            obs = archive('mysql-probe',side,label,'query',mysql.query('SELECT 1 AS v'))
            check(obs['code'] == 0 and obs['rows'] == [['31']], f'{side}/{label}: MySQL recovery')
            counts['interleaved_probes'] += 1

    def reject(case, sql, parse_only=False, transaction=False):
        for side,conn in conns.items():
            wire = wires[side]
            if transaction:
                check(query(conn,'BEGIN')['status'] == 1, f'{side}/{case}: BEGIN')
                check(wire.query('BEGIN')['ready'] == 'T', f'{side}/{case}: raw BEGIN')
            if not parse_only:
                obs = archive('pg',side,case,'query',query(conn,sql))
                counts['result_scenarios'] += 1
                check(obs['status'] == 7 and obs['state'] in ('42883','22P02') and not obs['metadata'] and not obs['rows'], f'{side}/{case}: Q early refusal')
                # Raw Q independently proves the absence of any success frames.
                obs = archive('wire',side,case,'query',wire.query(sql.replace(table,wire_table)))
                counts['result_scenarios'] += 1
                check(obs['tags'] == 'E' and obs.get('tail',{}).get('tags') == 'Z' and not obs['metadata'] and not obs['rows'], f'{side}/{case}: Q no success frames')
                if transaction:
                    query(conn,'ROLLBACK'); query(conn,'BEGIN')
                    wire.query('ROLLBACK'); wire.query('BEGIN')
            obs = archive('pg',side,case,'parse',pq.observe(pq.PQprepare(conn,case.encode(),sql.encode(),0,None)))
            check(obs['status'] == 7 and obs['state'] in ('42883','22P02') and not obs['metadata'] and not obs['rows'], f'{side}/{case}: Parse early refusal')
            wire.send(b'P',case.encode()+b'\0'+sql.replace(table,wire_table).encode()+b'\0\0\0')
            wire.send(b'D',b'S'+case.encode()+b'\0'); wire.send(b'E',b'\0\0\0\0\0'); wire.send(b'S')
            obs = wire.read('Z')
            if obs['errors']:
                obs['tail'] = wire.read('Z')
            archive('wire',side,case,'rejected-pipeline',obs)
            check(obs['tags'] == 'E' and not obs['metadata'] and not obs['rows'], f'{side}/{case}: no Parse/Describe/Execute success')
            check(obs.get('tail',{}).get('ready') == ('E' if transaction else 'I'), f'{side}/{case}: Sync state')
            if transaction:
                check(query(conn,'ROLLBACK')['status'] == 1, f'{side}/{case}: ROLLBACK')
                check(wire.query('ROLLBACK')['ready'] == 'I', f'{side}/{case}: raw ROLLBACK')
            obs = archive('pg-effect',side,case,'query',query(conn,f'SELECT (v+0)::int4 AS v FROM {table}'))
            check(obs['status'] == 2 and obs['rows'] == [['31']], f'{side}/{case}: no writes')
            obs = archive('wire-effect',side,case,'query',wire.query(f'SELECT (v+0)::int4 AS v FROM {wire_table}'))
            check(obs['rows'] == [['31']] and obs['ready'] == 'I', f'{side}/{case}: raw no writes')
        probe(case)

    try:
        repo = Path(__file__).resolve().parents[3]
        binary = os.environ.get('PG_FUNCTION27_RUST_BINARY')
        (evidence/'build.json').write_text(json.dumps(dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),
            fixtures={name:hashlib.sha256(Path(__file__).with_name(name).read_bytes()).hexdigest() for name in ('pg_function_27_libpq.py','pg_function_23_libpq.py','pg_function_24_libpq.py','pg_function_25_libpq.py')},
            cargo_lock_sha256=hashlib.sha256((repo/'Cargo.lock').read_bytes()).hexdigest(),
            source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg'],cwd=repo)).hexdigest(),
            rust_binary_sha256=hashlib.sha256(Path(binary).read_bytes()).hexdigest() if binary else None)))
        (evidence/'scenarios.json').write_text(json.dumps(dict(pg_calls=PG_CALLS,mysql_query=MYSQL_CASES,mysql_prepared=[(case,sql,[[None if v is None else v.hex() for v in row] for row in values]) for case,sql,values in MYSQL_EXTENDED]),indent=2))
        for side,dsn in dsns:
            conn = pq.PQconnectdb(dsn.encode()); assert conn
            conns[side] = conn
            assert pq.PQstatus(conn) == 0, f'{side}: login failed (DSN redacted)'
            identity = {sql:query(conn,sql) for sql in ('SELECT version()','SHOW server_version_num','SHOW client_encoding','SHOW standard_conforming_strings')}
            (evidence/f'pg-{side}-identity.json').write_text(json.dumps(dict(libpq=pq.PQlibVersion(),observations=identity)))
            check(pq.PQlibVersion() >= 180000, 'libpq18 required')
            if side == 'reference' and len(dsns) == 2:
                check(identity['SHOW server_version_num']['rows'] == [['313830303036']], 'PG18.6 unavailable; actual version archived separately')
            wires[side] = Wire(pq,dsn)
            check(query(conn,f'CREATE TABLE {table}(v integer)')['status'] == 1, f'{side}: own guard table')
            owned_pg.append(side)
            check(query(conn,f'INSERT INTO {table} VALUES(1)')['status'] == 1, f'{side}: guard row')
            check(wires[side].query(f'CREATE TABLE {wire_table}(v integer)')['tags'] == 'CZ', f'{side}: raw own guard')
            check(wires[side].query(f'INSERT INTO {wire_table} VALUES(1)')['tags'] == 'CZ', f'{side}: raw guard row')
        if aster_mysql:
            mysqls['aster'] = Mysql('127.0.0.1',aster_mysql[0],'root','',aster_mysql[1])
            check(bool(expected_build), 'own service build identity required')
        if os.environ.get('MYSQL8_REFERENCE_PORT'):
            mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'],int(os.environ['MYSQL8_REFERENCE_PORT']),os.environ['MYSQL8_REFERENCE_USER'],os.environ['MYSQL8_REFERENCE_PASSWORD'])
            reference_database = aster_mysql[1] if aster_mysql else f'function27_ref_{os.getpid()}'
            assert reference_database.replace('_','').isalnum()
            assert mysqls['reference'].query(f'CREATE DATABASE {reference_database}')['code'] == 0
            assert mysqls['reference'].query(f'USE {reference_database}')['code'] == 0
        else:
            check(False,'MySQL/TiDB reference missing')
        for side,mysql in mysqls.items():
            identity = {sql:mysql.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection')}
            text = ''.join(bytes.fromhex(cell).decode() for row in identity['SELECT version()']['rows'] for cell in row if cell)
            (evidence/f'mysql-{side}-identity.json').write_text(json.dumps(dict(client_version=mysql.mysql_get_client_version(),observations=identity,reference_family='TiDB' if 'tidb' in text.lower() else 'MySQL')))
            if side == 'reference' and len(dsns) == 2:
                check('tidb' in text.lower(), 'matching Go/TiDB reference unavailable; MySQL8 observations retained')
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
            assert mysql.query("SET sql_mode='STRICT_TRANS_TABLES,NO_ZERO_IN_DATE,NO_ZERO_DATE,ERROR_FOR_DIVISION_BY_ZERO,NO_ENGINE_SUBSTITUTION'")['code'] == 0
        probe('before')
        for prefix in ('','pg_catalog.'):
            for base,call in PG_CALLS:
                for empty in (False,True):
                    for tx in (False,True):
                        case = f'{base}-catalog{int(bool(prefix))}-empty{int(empty)}-tx{int(tx)}'
                        reject(case,f'SELECT {prefix}{call} AS v'+(' WHERE false' if empty else ''),'$1' in call,tx)
            for base,call in (('i','ifnull(1,2)'),('d',"date_format('bad','%Y')"),('j',"json_extract('{}','$.a')"),('v',"tidb_version('bad'::int4)")):
                reject(f'insert-{base}-catalog{int(bool(prefix))}',f'INSERT INTO {table} VALUES({prefix}{call})')
        # Successful independent PG control: full extended descriptor and both formats.
        for side,conn in conns.items():
            name = b'function27_control'; sql = b'SELECT $1::int4 AS v'
            check(pq.observe(pq.PQprepare(conn,name,sql,0,None))['status'] == 1,f'{side}: control Parse')
            desc = archive('pg-control',side,'control','statement-describe',pq.observe(pq.PQdescribePrepared(conn,name)))
            check(desc['parameters'] == [23] and desc['metadata'] == [[23,4,-1,0,'v',0,0]], f'{side}: control signature')
            wire = wires[side]
            check(wire.phase(b'P',name+b'\0'+sql+b'\0\0\0','1')['tags'] == '1',f'{side}: raw control Parse')
            archive('wire-control',side,'control','statement-describe',wire.phase(b'D',b'S'+name+b'\0','Tn'))
            for fmt in (0,1):
                obs = archive('pg-control',side,f'control-{fmt}','execute',pq.observe(pq.PQexecPrepared(conn,name,1,(c.c_char_p*1)(b'1'),(c.c_int*1)(1),(c.c_int*1)(0),fmt)))
                counts['result_scenarios'] += 1
                expected = (b'1' if fmt == 0 else struct.pack('!i',1)).hex()
                check(obs['rows'] == [[expected]] and obs['metadata'] == [[23,4,-1,fmt,'v',0,0]],f'{side}: control result')
                body = b'\0'+name+b'\0'+struct.pack('!HHHi',1,0,1,1)+b'1'+struct.pack('!HH',1,fmt)
                check(wire.phase(b'B',body,'2')['tags'] == '2',f'{side}: control Bind')
                desc = archive('wire-control',side,f'control-{fmt}','portal-describe',wire.phase(b'D',b'P\0','Tn'))
                check(desc['metadata'] == [[23,4,-1,fmt,'v',0,0]],f'{side}: portal control')
                wire.send(b'E',b'\0\0\0\0\0'); wire.send(b'S')
                obs = archive('wire-control',side,f'control-{fmt}','execute',wire.read('Z'))
                counts['result_scenarios'] += 1
                check(obs['rows'] == [[expected]] and obs['tags'] == 'DCZ',f'{side}: raw control execute')
        probe('middle')
        if os.environ.get('PG_FUNCTION27_CHECKPOINT'):
            print('PG_FUNCTION27_MID',flush=True)
            assert sys.stdin.readline().strip() == 'continue', 'MySQL checkpoint interrupted'
        for case,expression in MYSQL_CASES:
            for empty in (False,True):
                label = f'{case}-empty{int(empty)}'
                sql = f'SELECT {expression} AS v'+(' WHERE false' if empty else '')
                for side,mysql in mysqls.items():
                    obs = archive('mysql',side,label,'query',mysql.query(sql))
                    counts['result_scenarios'] += 1
                    prepared = mysql.prepared(sql,((),))
                    for index,item in enumerate(prepared):
                        archive('mysql',side,label,f'constant-prepared-{index}',item)
                        if index:
                            counts['result_scenarios'] += 1
                    if case == 'v-value' and (side == 'aster' or obs['code'] == 0):
                        for item in (obs,*prepared):
                            try:
                                build_contract(item,expected_build if side == 'aster' else None,empty)
                            except AssertionError:
                                check(False,f'{side}/{label}: own build contract')
                            else:
                                check(True,f'{side}/{label}: own build contract')
                probe(label)
        for case,expression,values in MYSQL_EXTENDED:
            for empty in (False,True):
                label = f'bind-{case}-empty{int(empty)}'
                for side,mysql in mysqls.items():
                    observations = mysql.prepared(f'SELECT {expression} AS v'+(' WHERE false' if empty else ''),values)
                    for index,obs in enumerate(observations):
                        archive('mysql',side,label,f'prepared-{index}',obs)
                        if index:
                            counts['result_scenarios'] += 1
                    if case == 'v-zero' and side == 'aster':
                        for obs in observations:
                            try:
                                build_contract(obs,expected_build,empty)
                            except AssertionError:
                                check(False,f'{side}/{label}: rebound own build identity')
                            else:
                                check(True,f'{side}/{label}: rebound own build identity')
                    if case != 'v-arity' and (side == 'aster' or case != 'v-zero'):
                        check(len(observations) == len(values)+1,f'{side}/{label}: all rebindings')
                probe(label)
        # Source18 runtime table operands keep short-circuit and first-arm errors real.
        for side,mysql in mysqls.items():
            assert mysql.query('CREATE TABLE ifnull_runtime(a CHAR(8),z BIGINT)')['code'] == 0
            assert mysql.query("INSERT INTO ifnull_runtime VALUES('ok',-9223372036854775808)")['code'] == 0
        for turn,null in enumerate((False,True,False)):
            for side,mysql in mysqls.items():
                assert mysql.query('UPDATE ifnull_runtime SET a='+('NULL' if null else "'ok'"))['code'] == 0
            for first in (False,True):
                expression = 'IFNULL(CAST(ABS(CAST(z AS SIGNED)) AS CHAR(24)),a)' if first else 'IFNULL(a,CAST(ABS(CAST(z AS SIGNED)) AS CHAR(24)))'
                label = f'i-runtime-null{int(null)}-first{int(first)}'
                # Third turn must have its own ID; input sequence is immutable.
                label += '-turn'+str(turn)
                for side,mysql in mysqls.items():
                    obs = archive('mysql',side,label,'query',mysql.query(f'SELECT {expression} AS v FROM ifnull_runtime'))
                    counts['result_scenarios'] += 1
                    check(obs['code'] == (1690 if null or first else 0),f'{side}/{label}: reached branch')
                    observations = mysql.prepared(f'SELECT {expression} AS v FROM ifnull_runtime',((),))
                    for index,item in enumerate(observations):
                        archive('mysql',side,label,f'prepared-{index}',item)
                        if index:
                            counts['result_scenarios'] += 1
                    effect = archive('mysql-effect',side,label,'query',mysql.query('SELECT a,z FROM ifnull_runtime'))
                    check(effect['rows'] == [[None if null else '6f6b','2d39323233333732303336383534373735383038']], f'{side}/{label}: unchanged table')
                probe(label)
        probe('after')
        (evidence/'candidate-domains.json').write_text(json.dumps([
            dict(name=name, parameters=parameters, dialect='MySQL only',
                 pg='unqualified/pg_catalog Q/Parse refusal and no writes recorded',
                 mysql='Query/constant Prepare/two-parameter rebind values, metadata and diagnostics recorded',
                 nulls='input order and reached errors recorded',
                 collation='utf8mb4_bin session; no other collation approval',
                 folding_cache='literal and prepared inputs observed; optimization not proven',
                 pushdown='not authorized', equivalence='pending exact reference and all fields')
            for name,parameters in (('ifnull',2),('date_format',2),('json_extract','2 or more'),('tidb_version',0))],indent=2))
        if len(dsns) == 2:
            for (domain,case,phase),peers in records.items():
                check(set(peers) == {'reference','aster'},f'{domain}/{case}/{phase}: both observations')
                if set(peers) != {'reference','aster'}:
                    continue
                # Build text is inherently build-specific; preserve raw observations and
                # require each successful side's own build contract before compatibility.
                if domain == 'mysql' and case.startswith(('v-value-','bind-v-zero-')) and all(x['code'] == 0 for x in peers.values()):
                    for obs in peers.values():
                        try:
                            build_contract(obs,empty='empty1' in case)
                        except AssertionError:
                            check(False,f'{case}/{phase}: independent build contract')
                        else:
                            check(True,f'{case}/{phase}: independent build contract')
                    continue
                comparator = mysql_compare if domain.startswith('mysql') else compare_wire if domain.startswith('wire') else pg_compare
                counts['assertions'] += 1
                try:
                    comparator(peers['reference'],peers['aster'])
                except AssertionError as error:
                    failures.append(f'{domain}/{case}/{phase}: {error}')
    except BaseException as error:
        failures.append('client aborted: '+type(error).__name__)
        raise
    finally:
        for side in owned_pg:
            query(conns[side],'ROLLBACK')
            cleanup['pg-'+side+'-table-dropped'] = query(conns[side],f'DROP TABLE {table}')['status'] == 1
            wires[side].query('ROLLBACK')
            cleanup['wire-'+side+'-table-dropped'] = wires[side].query(f'DROP TABLE {wire_table}')['tags'] == 'CZ'
        for wire in wires.values():
            wire.close()
        for conn in conns.values():
            pq.PQfinish(conn)
        for side,mysql in mysqls.items():
            cleanup['mysql-'+side+'-table-dropped'] = mysql.query('DROP TABLE IF EXISTS ifnull_runtime')['code'] == 0
        if reference_database:
            cleanup['reference_database_dropped'] = mysqls['reference'].query(f'DROP DATABASE {reference_database}')['code'] == 0
        for mysql in mysqls.values():
            mysql.close()
        check(all(cleanup.values()), 'owned table/database cleanup')
        breakdown = {}
        for (domain,case,phase),peers in records.items():
            if domain in ('pg','wire','pg-control','wire-control','mysql') and (phase in ('query','execute') or ('prepared-' in phase and not phase.endswith('-0'))):
                key = domain+('-query' if phase == 'query' else '-execute')
                breakdown[key] = breakdown.get(key,0)+len(peers)
        check(sum(breakdown.values()) == counts['result_scenarios'],'scenario accounting')
        summary = dict(**counts,result_breakdown=breakdown,negative_controls=negative_controls(),failures=failures,
                       observation_keys=len(records),pg_connections_closed=len(conns)+len(wires),mysql_connections_closed=len(mysqls),cleanup=cleanup,
                       scope='IFNULL/DATE_FORMAT/JSON_EXTRACT/TIDB_VERSION; no alias, folding or pushdown approval')
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary),flush=True)
    assert records and counts['result_scenarios'] and not failures,'exclusive function differential failed'


if __name__ == '__main__':
    def terminate(signum, frame):
        raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM,terminate)
    if sys.argv[1:] == ['--self-test']:
        print(json.dumps(dict(negative_controls=negative_controls(),pg_inputs=len(PG_CALLS),mysql_inputs=len(MYSQL_CASES),mysql_signatures=len(MYSQL_EXTENDED))))
    elif sys.argv[1] == '--reference-only':
        run([('reference',os.environ['PG18_REFERENCE_DSN'])],Path(sys.argv[2]))
    else:
        port,database,evidence,mysql_port = sys.argv[1:]
        run([('reference',os.environ['PG18_REFERENCE_DSN']),('aster',f'host=127.0.0.1 port={int(port)} user=root dbname={database} connect_timeout=10')],Path(evidence),(int(mysql_port),database))
