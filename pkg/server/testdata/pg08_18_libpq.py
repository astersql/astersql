# Copyright 2026 AsterSQL.
"""Task 08.18 numeric to_char observations. Reference credentials are never archived."""
import ctypes as c
import hashlib
import json
import os
import signal
from pathlib import Path
import struct
import subprocess
import sys
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare


CASE_IDS = ('I-before', 'I-middle', 'I-after', 'C-Q-main', 'C-Q-negative', 'C-Q-spaces', 'C-Q-zero-fill', 'C-Q-fraction', 'C-Q-optional', 'C-Q-round', 'C-Q-negative-round', 'C-Q-overflow', 'C-Q-zero', 'C-Q-fill-zero', 'C-Q-literal-spaces', 'C-Q-quoted', 'C-Q-empty', 'C-Q-null', 'C-Q-null-template', 'C-Q-exact', 'C-Q-zero-row', 'C-Q-nested', 'C-E-vdecimal', 'C-recover-vdecimal', 'C-E-signature', 'C-recover-signature', 'C-Q-row', 'C-Q-row-nested', 'C-Q-row-zero', 'C-bind-0-0-0-0', 'C-bind-0-0-0-1', 'C-bind-0-0-0-2', 'C-bind-0-0-0-3', 'C-bind-0-0-1-0', 'C-bind-0-0-1-1', 'C-bind-0-0-1-2', 'C-bind-0-0-1-3', 'C-bind-0-1-0-0', 'C-bind-0-1-0-1', 'C-bind-0-1-0-2', 'C-bind-0-1-0-3', 'C-bind-0-1-1-0', 'C-bind-0-1-1-1', 'C-bind-0-1-1-2', 'C-bind-0-1-1-3', 'C-bind-1-0-0-0', 'C-bind-1-0-0-1', 'C-bind-1-0-0-2', 'C-bind-1-0-0-3', 'C-bind-1-0-1-0', 'C-bind-1-0-1-1', 'C-bind-1-0-1-2', 'C-bind-1-0-1-3', 'C-bind-1-1-0-0', 'C-bind-1-1-0-1', 'C-bind-1-1-0-2', 'C-bind-1-1-0-3', 'C-bind-1-1-1-0', 'C-bind-1-1-1-1', 'C-bind-1-1-1-2', 'C-bind-1-1-1-3', 'C-context-utc', 'C-context-ny', 'C-isolation', 'C-txn-signature', 'C-E-date-format', 'M-Q-date-format', 'M-prepare-date-format', 'M-Q-empty', 'M-prepare-empty', 'M-Q-format', 'M-prepare-format', 'M-Q-to-char', 'M-prepare-to-char', 'C-Q-locale-de-main', 'C-Q-locale-de-fixed', 'C-Q-locale-c-currency', 'C-Q-locale-de-currency', 'C-Q-locale-both', 'C-Q-locale-sign-positive', 'C-Q-locale-sign-negative', 'C-Q-locale-sign-trailing', 'C-Q-locale-padding', 'C-Q-locale-small', 'C-Q-locale-zero', 'C-Q-locale-quoted', 'C-Q-locale-null', 'C-Q-locale-template-null', 'C-context-cache-c', 'C-context-cache-numeric', 'C-context-cache-monetary', 'C-context-cache-both', 'C-context-txn-lc_numeric', 'C-context-txn-lc_monetary', 'C-E-lc_numeric', 'C-recover-lc_numeric', 'C-E-lc_monetary', 'C-recover-lc_monetary')
LOCALE_CASES = [('de-main', 'de_DE.UTF-8', 'C', '1234.5', 'FM9G999D0', '1.234,5'), ('de-fixed', 'de_DE.UTF-8', 'C', '1234.5', 'FM9,999.0', '1,234.5'), ('c-currency', 'C', 'C', '1234.5', 'FML9G999D0', ' 1,234.5'), ('de-currency', 'C', 'de_DE.UTF-8', '1234.5', 'FML9G999D0', '€1,234.5'), ('both', 'de_DE.UTF-8', 'de_DE.UTF-8', '1234.5', 'FML9G999D0', '€1.234,5'), ('sign-positive', 'de_DE.UTF-8', 'de_DE.UTF-8', '1234.5', 'FMS9G999D0', '+1.234,5'), ('sign-negative', 'de_DE.UTF-8', 'de_DE.UTF-8', '-1234.5', 'FMS9G999D0', '-1.234,5'), ('sign-trailing', 'de_DE.UTF-8', 'de_DE.UTF-8', '-1234.5', 'FM9G999D0S', '1.234,5-'), ('padding', 'de_DE.UTF-8', 'de_DE.UTF-8', '12.5', '9G999D0', '    12,5'), ('small', 'de_DE.UTF-8', 'de_DE.UTF-8', '0.1', 'FM9G999D0', ',1'), ('zero', 'de_DE.UTF-8', 'de_DE.UTF-8', '0', 'FM9G999D0', ',0'), ('quoted', 'de_DE.UTF-8', 'de_DE.UTF-8', '12.5', '"D G S L" FM999D0', 'D G S L 12,5'), ('null', 'de_DE.UTF-8', 'de_DE.UTF-8', None, 'FML9G999D0S', None), ('template-null', 'de_DE.UTF-8', 'de_DE.UTF-8', '1234.5', None, None)]


def run(port, database, evidence, mysql_port):
    assert database.replace("_", "").isalnum(), "invalid owned database name"
    evidence.mkdir(parents=True, exist_ok=False)
    repository = Path(__file__).resolve().parents[3]
    pq = Libpq()
    parameter_status = pq.lib.PQparameterStatus
    parameter_status.restype = c.c_char_p
    parameter_status.argtypes = [c.c_void_p,c.c_char_p]
    transaction_status = pq.lib.PQtransactionStatus
    transaction_status.restype = c.c_int
    transaction_status.argtypes = [c.c_void_p]
    conns, mysqls, passed = {}, {}, []
    failures = {}
    attempted = set()
    owned_database = False
    reference_table_created = False
    original_zones = {}
    original_locales = {}
    source_table = "ts_" + database
    cleanup = {}
    def save(side, case, observation, phase="query"):
        if case.startswith(('C-Q-', 'C-E-', 'C-recover-', 'C-bind-', 'C-txn-', 'C-context-', 'C-isolation', 'M-', 'I-')):
            attempted.add(case)
        with (evidence / (side + '.jsonl')).open('a') as out:
            out.write(json.dumps(dict(case_id=case, phase=phase, observation=observation)) + '\n')
        return observation
    def query(conn, sql):
        return pq.observe(pq.PQexec(conn, sql.encode()))
    def set_zone(side, zone):
        # Use the existing native setting spelling on AsterSQL, as task 09 does.
        sql = ("SET TIME ZONE '" if side == 'reference' else "SET time_zone = '") + zone.replace("'", "''") + "'"
        assert query(conns[side], sql)['status'] == 1
    def paired(case, sql, expected=None, zero=False, state=''):
        observations = [save(side, case, query(conn, sql)) for side, conn in conns.items()]
        for obs in observations:
            if state:
                assert obs['status'] == 7 and obs['state'] == state, case
                assert not obs['rows'] and all(obs['errors'][k] for k in 'SVCM'), case
            else:
                assert obs['status'] == 2, (case, obs)
                assert obs['metadata'] == [[25, -1, -1, 0, 'v', 0, 0]], case
                assert obs['rows'] == ([] if zero else [[None if expected is None else expected.encode().hex()]]), case
        for obs in observations[1:]: compare(observations[0], obs)
        passed.append(case)
    def interleave(label):
        for side, conn in conns.items():
            obs = save(side, 'I-'+label, query(conn, 'SELECT 1::int4 AS v'))
            assert obs['rows'] == [['31']] and obs['status'] == 2
        observations = [save('mysql-'+side, 'I-'+label, mysql.query('SELECT v FROM health'), phase='health') for side, mysql in mysqls.items()]
        for obs in observations: assert obs['rows'] == [['37']] and obs['code'] == 0
        mysql_compare(*observations)
        behavior = [save('mysql-'+side, 'I-'+label, mysql.query("SELECT EXTRACT(SECOND FROM CAST('2024-01-02 00:00:40.123456' AS DATETIME(6))) AS v"), phase='native-behavior') for side, mysql in mysqls.items()]
        for obs in behavior:
            assert obs['code'] == 0 and obs['rows'] == [['3430']]
        mysql_compare(*behavior)
        passed.append('I-'+label)
    try:
        dsns = {'reference': os.environ['PG18_REFERENCE_DSN'],
                'A': f'host=127.0.0.1 port={port} user=root dbname={database}',
                'B': f'host=127.0.0.1 port={port} user=root dbname={database}'}
        for side, dsn in dsns.items():
            conn = pq.PQconnectdb(dsn.encode()); conns[side] = conn
            assert conn and pq.PQstatus(conn) == 0, 'PG connection failed (DSN redacted)'
        original_locales = {side:{key:bytes.fromhex(query(conn,'SHOW '+key)['rows'][0][0]).decode() for key in ('lc_numeric','lc_monetary')} for side,conn in conns.items()}
        original_zones = {side:parameter_status(conn,b'TimeZone').decode() for side,conn in conns.items()}
        identity = {sql: query(conns['reference'], sql) for sql in ('SELECT version()', 'SHOW server_version_num', 'SHOW server_encoding', 'SHOW client_encoding', 'SHOW lc_time', 'SHOW lc_numeric', 'SHOW lc_monetary', 'SHOW TimeZone')}
        assert identity['SHOW server_version_num']['rows'] == [['313830303036']]
        save('reference', 'identity', identity)
        save('build', 'identity', dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repository,text=True).strip(), fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(), libpq=pq.PQlibVersion(), source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg/expression','pkg/server','pkg/session','pkg/types'],cwd=repository)).hexdigest(), cargo_lock_sha256=hashlib.sha256((repository/'Cargo.lock').read_bytes()).hexdigest()))
        reference_bin = os.environ.get('PG08_REFERENCE_BIN')
        build = dict()
        if reference_bin:
            binary = Path(reference_bin) / 'postgres'
            sharedir = Path(subprocess.check_output([str(Path(reference_bin)/'pg_config'),'--sharedir'], text=True).strip())
            timezone = sharedir / 'timezone' / 'Etc' / 'UTC'
            build.update(postgres_version=subprocess.check_output([str(binary),'--version'],text=True).strip(),
                         postgres_sha256=hashlib.sha256(binary.read_bytes()).hexdigest(),
                         configure=subprocess.check_output([str(Path(reference_bin)/'pg_config'),'--configure'],text=True).strip(),
                         tzdata_utc_sha256=hashlib.sha256(timezone.read_bytes()).hexdigest())
        tzdata_source = os.environ.get('PG08_TZDATA_SOURCE')
        if tzdata_source:
            tzdata = Path(tzdata_source).read_bytes()
            build['tzdata_version'] = tzdata.splitlines()[0].decode()
            build['tzdata_sha256'] = hashlib.sha256(tzdata).hexdigest()
        mysql_bin = os.environ.get('PG08_MYSQL_BIN')
        if mysql_bin:
            binary = Path(mysql_bin) / 'mysqld'
            build['mysql_version'] = subprocess.check_output([str(binary),'--version'],text=True).strip()
            build['mysql_sha256'] = hashlib.sha256(binary.read_bytes()).hexdigest()
        rust_binary = os.environ.get('PG08_18_RUST_BINARY')
        if rust_binary:
            build['rust_binary_sha256'] = hashlib.sha256(Path(rust_binary).read_bytes()).hexdigest()
        save('build', 'binary-identity', build)
        mysqls['reference'] = Mysql(os.environ['MYSQL8_REFERENCE_HOST'], int(os.environ['MYSQL8_REFERENCE_PORT']), os.environ['MYSQL8_REFERENCE_USER'], os.environ['MYSQL8_REFERENCE_PASSWORD'])
        save('build', 'mysql-client', dict(version=mysqls['reference'].mysql_get_client_version()))
        mysqls['aster'] = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code'] == 0
        owned_database = True
        assert mysqls['reference'].query(f'USE {database}')['code'] == 0
        for side, mysql in mysqls.items():
            save('mysql-'+side,'identity',{sql:mysql.query(sql) for sql in ('SELECT version()', 'SELECT @@sql_mode', 'SELECT @@character_set_connection', 'SELECT @@collation_connection')})
            assert mysql.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code'] == 0
            assert mysql.query('CREATE TABLE health(v INT)')['code'] == 0
            assert mysql.query('INSERT INTO health VALUES (7)')['code'] == 0
        interleave('before')
        paired('C-Q-main',"SELECT to_char('1234.5'::numeric,'FM9G999D0'::text) AS v",'1,234.5',zero=False)
        paired('C-Q-negative',"SELECT to_char('-148.5'::numeric,'FM999D990'::text) AS v",'-148.500',zero=False)
        paired('C-Q-spaces',"SELECT to_char('12'::numeric,'9999'::text) AS v",'   12',zero=False)
        paired('C-Q-zero-fill',"SELECT to_char('12'::numeric,'0000'::text) AS v",' 0012',zero=False)
        paired('C-Q-fraction',"SELECT to_char('0.1'::numeric,'FM9.990'::text) AS v",'.100',zero=False)
        paired('C-Q-optional',"SELECT to_char('12.3'::numeric,'FM99.999'::text) AS v",'12.3',zero=False)
        paired('C-Q-round',"SELECT to_char('9.995'::numeric,'FM99.00'::text) AS v",'10.00',zero=False)
        paired('C-Q-negative-round',"SELECT to_char('-9.995'::numeric,'FM99.00'::text) AS v",'-10.00',zero=False)
        paired('C-Q-overflow',"SELECT to_char('99.995'::numeric,'99.99'::text) AS v",' ##.##',zero=False)
        paired('C-Q-zero',"SELECT to_char('0'::numeric,'999'::text) AS v",'   0',zero=False)
        paired('C-Q-fill-zero',"SELECT to_char('0'::numeric,'FM999'::text) AS v",'0',zero=False)
        paired('C-Q-literal-spaces',"SELECT to_char('12'::numeric,'  FM999  '::text) AS v",'  12  ',zero=False)
        paired('C-Q-quoted','SELECT to_char(\'148.5\'::numeric,\'"x"FM999D990\'::text) AS v','x148.500',zero=False)
        paired('C-Q-empty',"SELECT to_char('1'::numeric,''::text) AS v",'',zero=False)
        paired('C-Q-null',"SELECT to_char(NULL::numeric,'FM999'::text) AS v",None,zero=False)
        paired('C-Q-null-template',"SELECT to_char('1'::numeric,NULL::text) AS v",None,zero=False)
        paired('C-Q-exact',"SELECT to_char('9007199254740993'::numeric,'FM9999999999999999.0'::text) AS v",'9007199254740993.0',zero=False)
        paired('C-Q-zero-row',"SELECT to_char(NULL::numeric,'FM999'::text) AS v WHERE false",None,zero=True)
        paired('C-Q-nested',"SELECT to_char(round('148.5'::numeric,1::int4),'FM999D990'::text) AS v",'148.500',zero=False)
        paired('C-E-vdecimal',"SELECT to_char('1'::numeric,'99.9V99'::text) AS v",state='42601')
        for side,conn in conns.items():
            assert save(side,'C-recover-vdecimal',query(conn,'SELECT 1::int4 AS v'))['rows']==[['31']]
        passed.append('C-recover-vdecimal')
        paired('C-E-signature',"SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v",state='42883')
        for side,conn in conns.items():
            obs=save(side,'C-recover-signature',query(conn,'SELECT 1::int4 AS v'));assert obs['rows']==[['31']]
        passed.append('C-recover-signature')
        def settings(numeric,monetary,sides=None):
            for side in (sides if sides is not None else conns):
                for key,value in [('lc_numeric',numeric),('lc_monetary',monetary)]:
                    obs=query(conns[side],f"SET {key}='{value}'")
                    assert obs['status']==1,(side,key,obs)
        for label,numeric,monetary,source,template,wanted in LOCALE_CASES:
            settings(numeric,monetary)
            source='NULL' if source is None else "'"+source+"'"
            template='NULL' if template is None else "'"+template+"'"
            paired('C-Q-locale-'+label,f'SELECT to_char({source}::numeric,{template}::text) AS v',wanted)
        settings('C','C')
        cache_sql=b"SELECT to_char('1234.5'::numeric,'FML9G999D0S'::text) AS v"
        for conn in conns.values():
            assert pq.observe(pq.PQprepare(conn,b'locale18',cache_sql,0,None))['status']==1
        for label,numeric,monetary,wanted in [('c','C','C',' 1,234.5+'),('numeric','de_DE.UTF-8','C',' 1.234,5+'),('monetary','C','de_DE.UTF-8','€1,234.5+'),('both','de_DE.UTF-8','de_DE.UTF-8','€1.234,5+')]:
            case='C-context-cache-'+label;observations=[]
            settings(numeric,monetary,['reference','A'])
            for side in ('reference','A'):
                obs=save(side,case,pq.observe(pq.PQexecPrepared(conns[side],b'locale18',0,None,None,None,0)),phase='Execute')
                assert obs['rows']==[[wanted.encode().hex()]] and obs['metadata']==[[25,-1,-1,0,'v',0,0]],obs
                observations.append(obs)
            compare(*observations)
            obs=save('B',case,pq.observe(pq.PQexecPrepared(conns['B'],b'locale18',0,None,None,None,0)),phase='Execute-isolated')
            assert obs['rows']==[[b' 1,234.5+'.hex()]],obs
            passed.append(case)
        settings('C','C')
        for key in ('lc_numeric','lc_monetary'):
            case='C-context-txn-'+key
            for side,conn in conns.items():
                for sql in ['BEGIN',f"SET {key}='de_DE.UTF-8'",'SAVEPOINT locale18',f"SET LOCAL {key}='C'",'ROLLBACK TO SAVEPOINT locale18']:
                    obs=save(side,case,query(conn,sql),phase='setting');assert obs['status']==1,(sql,obs)
                assert query(conn,'SHOW '+key)['rows']==[[b'de_DE.UTF-8'.hex()]]
                assert query(conn,'ROLLBACK')['status']==1
                assert query(conn,'SHOW '+key)['rows']==[[b'C'.hex()]]
                for sql in ['BEGIN',f"SET {key}='de_DE.UTF-8'",f"SET LOCAL {key}='C'",'COMMIT']:
                    assert query(conn,sql)['status']==1
                assert query(conn,'SHOW '+key)['rows']==[[b'de_DE.UTF-8'.hex()]]
                assert query(conn,'RESET '+key)['status']==1
                assert query(conn,'SHOW '+key)['rows']==[[b'C'.hex()]]
            passed.append(case)
            paired('C-E-'+key,f"SET {key}='this_locale_does_not_exist'",state='22023')
            for side,conn in conns.items():
                obs=save(side,'C-recover-'+key,query(conn,'SELECT 1::int4 AS v'));assert obs['rows']==[['31']]
            passed.append('C-recover-'+key)
        settings('de_DE.UTF-8','de_DE.UTF-8')
        assert query(conns['reference'],f'CREATE TEMP TABLE {source_table}(value varchar(100))')['status']==1
        reference_table_created=True
        assert query(conns['reference'],f"INSERT INTO {source_table} VALUES ('148.5')")['status']==1
        assert mysqls['aster'].query(f'CREATE TABLE {source_table}(value varchar(100))')['code']==0
        assert mysqls['aster'].query(f"INSERT INTO {source_table} VALUES ('148.5')")['code']==0
        paired('C-Q-row',f"SELECT to_char(value::numeric,'FML999D0S'::text) AS v FROM {source_table}",'€148,5+')
        paired('C-Q-row-nested',f"SELECT to_char(round(value::numeric,1::int4),'FML999D0S'::text) AS v FROM {source_table}",'€148,5+')
        paired('C-Q-row-zero',f"SELECT to_char(value::numeric,'FM999D990'::text) AS v FROM {source_table} WHERE false",zero=True)
        settings('C','C')
        interleave('middle')
        for empty in (0,1):
            for side in conns: set_zone(side, 'UTC')
            name=('pg08_'+str(empty)).encode();sql='SELECT to_char($1::numeric,$2::text) AS v'+(' WHERE false' if empty else '')
            for side,conn in conns.items():
                assert save(side,'parse-'+str(empty),pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe-'+str(empty),pq.observe(pq.PQdescribePrepared(conn,name)))
                assert desc['parameters']==[1700,25] and desc['metadata']==[[25,-1,-1,0,'v',0,0]],desc
            settings('de_DE.UTF-8','de_DE.UTF-8')
            for side in conns: set_zone(side, 'America/New_York')
            for infmt in (0,1):
                for outfmt in (0,1):
                    for index in range(4):
                        val=None if index==1 else (bytes.fromhex('000200000000000100941388') if infmt else b'148.5')
                        unit=b'99.9V99' if index==3 else (b'FM000.0' if index==2 else b'FM999D990')
                        encoded=[val,unit];case=f'C-bind-{empty}-{infmt}-{outfmt}-{index}';observations=[]
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,name,2,(c.c_char_p*2)(*encoded),(c.c_int*2)(0 if val is None else len(val),len(unit)),(c.c_int*2)(infmt,infmt),outfmt)),phase='Execute')
                            if index==3 and not empty:
                                assert obs['status']==7 and obs['state']=='42601' and not obs['rows'],(case,obs)
                                assert save(side,case,query(conn,'SELECT 1::int4 AS v'),phase='Sync-recovery')['rows']==[['31']]
                                observations.append(obs)
                                continue
                            assert obs['status']==2 and obs['metadata']==[[25,-1,-1,outfmt,'v',0,0]],(case,obs)
                            target=None if index==1 else (b'148.5' if index==2 else b'148,500').hex()
                            assert obs['rows']==([] if empty else [[target]]),(case,obs)
                            observations.append(obs)
                        for obs in observations[1:]:compare(observations[0],obs)
                        passed.append(case)
        settings('C','C')
        # A cached numeric expression stays independent of the Execute connection zone.
        constant_sql=b"SELECT to_char('148.5'::numeric,'FM999D990'::text) AS v"
        for side,conn in conns.items():
            set_zone(side,'UTC')
            assert pq.observe(pq.PQprepare(conn,b'constant18',constant_sql,0,None))['status']==1
        for label,zone,wanted in [('utc','UTC','148.500'),('ny','America/New_York','148.500')]:
            observations=[];case='C-context-'+label
            for side,conn in conns.items():
                set_zone(side,zone)
                obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,b'constant18',0,None,None,None,0)),phase='Execute')
                assert obs['rows']==[[wanted.encode().hex()]] and obs['metadata']==[[25,-1,-1,0,'v',0,0]],obs
                observations.append(obs)
            for obs in observations[1:]:compare(observations[0],obs)
            passed.append(case)
        set_zone('B','UTC')
        sql=constant_sql.decode()
        for side,expected in [('reference','148.500'),('A','148.500'),('B','148.500')]:
            obs=save(side,'C-isolation',query(conns[side],sql))
            assert obs['rows']==[[expected.encode().hex()]],obs
        passed.append('C-isolation')
        errors=[]
        for side,conn in conns.items():
            case='C-txn-signature'
            assert save(side,case,query(conn,'BEGIN'),phase='begin')['status']==1
            assert transaction_status(conn)==2
            obs=save(side,case,pq.observe(pq.PQprepare(conn,b'bad_signature',b"SELECT to_char('2024-01-02'::text,'YYYY'::text) AS v",0,None)),phase='Parse')
            assert obs['status']==7 and obs['state']=='42883' and all(obs['errors'][k] for k in 'SVCM')
            assert transaction_status(conn)==3
            assert save(side,case,query(conn,'ROLLBACK'),phase='rollback')['status']==1
            assert transaction_status(conn)==0
            assert save(side,case,query(conn,'SELECT 1::int4 AS v'),phase='recovery')['rows']==[['31']]
            errors.append(obs)
        for obs in errors[1:]:compare(errors[0],obs)
        passed.append('C-txn-signature')
        paired('C-E-date-format',"SELECT date_format('2024-01-02'::timestamp,'%Y'::text) AS v",state='42883')
        # Independently verified on MySQL 8.0.46: empty DATE_FORMAT is SQL NULL
        # for Query and Prepare/Execute; it is separate from name-resolution errors.
        for label,sql,wanted in [('date-format', "DATE_FORMAT(CAST('2024-01-02 03:04:05.123456' AS DATETIME(6)),'%Y-%m-%d %H:%i:%s.%f')", '2024-01-02 03:04:05.123456'), ('empty', "DATE_FORMAT(CAST('2024-01-02' AS DATETIME),'')", None), ('format', "FORMAT(12345.678,2,'de_DE')", '12.345,68'), ('to-char', "to_char(CAST(148.5 AS DECIMAL(10,3)),'FM999D990')", None)]:
            query_sql='SELECT '+sql+' AS v'
            observations=[save('mysql-'+side,'M-Q-'+label,m.query(query_sql)) for side,m in mysqls.items()]
            try:
                for obs in observations:
                    if label == 'to-char':assert obs['code']==1305 and obs['state']=='42000' and not obs['rows'],obs
                    else:assert obs['code']==0 and obs['rows']==[[None if wanted is None else wanted.encode().hex()]],obs
                mysql_compare(*observations)
                passed.append('M-Q-'+label)
            except AssertionError as error:
                failures['M-Q-'+label]=str(error)
            observations=[save('mysql-'+side,'M-prepare-'+label,m.prepared(query_sql,[()])) for side,m in mysqls.items()]
            try:
                for phases in observations:
                    if label == 'to-char':assert any(o['code']==1305 and o['state']=='42000' for o in phases),phases
                    else:assert phases[-1]['code']==0 and phases[-1]['rows']==[[None if wanted is None else wanted.encode().hex()]],phases
                assert observations[0]==observations[1],observations
                passed.append('M-prepare-'+label)
            except AssertionError as error:
                failures['M-prepare-'+label]=str(error)
        interleave('after')
        assert len(passed)==len(set(passed))
        assert set(passed)|set(failures)==set(CASE_IDS)
        assert not failures, failures
    finally:
        # A failure assertion may leave the owned connection in an aborted transaction.
        for conn in conns.values():
            if conn and transaction_status(conn) in (2,3):
                assert query(conn, 'ROLLBACK')['status'] == 1
        if reference_table_created:
            cleanup['reference_table_dropped'] = query(conns['reference'], f'DROP TABLE {source_table}')['status'] == 1
        if owned_database:
            cleanup['reference_database_dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        for side,conn in conns.items():
            if conn:
                if side in original_zones:
                    set_zone(side, original_zones[side])
                if side in original_locales:
                    for key,value in original_locales[side].items():
                        assert query(conn,f"SET {key}='{value}'")['status']==1
                pq.PQfinish(conn)
        cleanup['locales_restored']=len(original_locales)==len(conns)
        cleanup['timezones_restored']=len(original_zones)==len(conns)
        cleanup.update(
            connections_closed=True,
            failures=failures,
            negative_self_checks=3,
            expected=len(CASE_IDS),
            expected_counts={'success': 30, 'null': 4, 'zero': 2, 'error': 5, 'recovery': 4, 'binding': 32, 'context': 9, 'transaction': 1, 'mysql': 8, 'interleaved': 3},
            actual=len(attempted),
            passed=len(passed),
            failed=len(attempted-set(passed)),
            skipped=len(CASE_IDS)-len(attempted),
            unique=len(attempted),
            case_ids=passed,
            expected_case_ids=CASE_IDS,
            counts=dict(
                success=sum(x.startswith('C-Q-') and x not in ('C-Q-null','C-Q-null-template','C-Q-zero-row','C-Q-row-zero','C-Q-locale-null','C-Q-locale-template-null') for x in passed),
                null=sum(x in ('C-Q-null','C-Q-null-template','C-Q-locale-null','C-Q-locale-template-null') for x in passed),
                zero=sum(x in ('C-Q-zero-row','C-Q-row-zero') for x in passed),
                error=sum(x.startswith('C-E-') for x in passed),
                recovery=sum(x.startswith('C-recover-') for x in passed),
                binding=sum(x.startswith('C-bind-') for x in passed),
                context=sum(x.startswith('C-context-') or x=='C-isolation' for x in passed),
                transaction=sum(x.startswith('C-txn-') for x in passed),
                mysql=sum(x.startswith('M-') for x in passed),
                interleaved=sum(x.startswith('I-') for x in passed),
            ),
        )
        (evidence/'summary.json').write_text(json.dumps(cleanup,indent=2))
    print('pg08_18_clients: 98 unique scenarios passed; 3 negative self-checks')

if __name__=='__main__':
    def interrupted(signum, _frame):
        raise SystemExit(128 + signum)
    signal.signal(signal.SIGTERM, interrupted)
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:
        run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
