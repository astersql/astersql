# Copyright 2026 AsterSQL.
"""Task 08.22 array_append observations. Reference credentials are never archived."""
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


CASE_IDS = ('C-E-null-incompatible','C-recover-null-incompatible','C-txn-null-incompatible','C-Q-basic', 'C-Q-wide', 'I-before', 'I-middle', 'I-after', 'C-Q-empty', 'C-Q-null-array', 'C-Q-null-both', 'C-Q-null-element', 'C-Q-lower-zero', 'C-Q-lower-negative', 'C-Q-near-upper', 'C-Q-zero', 'C-Q-nested', 'C-Q-row', 'C-Q-row-zero', 'C-E-signature', 'C-recover-signature', 'C-txn-signature', 'C-E-multidim', 'C-recover-multidim', 'C-txn-multidim', 'C-E-upper-boundary', 'C-recover-upper-boundary', 'C-txn-upper-boundary', 'C-bind-0-0-0-0', 'C-bind-0-0-0-1', 'C-bind-0-0-0-2', 'C-bind-0-0-0-3', 'C-bind-0-0-1-0', 'C-bind-0-0-1-1', 'C-bind-0-0-1-2', 'C-bind-0-0-1-3', 'C-bind-0-1-0-0', 'C-bind-0-1-0-1', 'C-bind-0-1-0-2', 'C-bind-0-1-0-3', 'C-bind-0-1-1-0', 'C-bind-0-1-1-1', 'C-bind-0-1-1-2', 'C-bind-0-1-1-3', 'C-bind-1-0-0-0', 'C-bind-1-0-0-1', 'C-bind-1-0-0-2', 'C-bind-1-0-0-3', 'C-bind-1-0-1-0', 'C-bind-1-0-1-1', 'C-bind-1-0-1-2', 'C-bind-1-0-1-3', 'C-bind-1-1-0-0', 'C-bind-1-1-0-1', 'C-bind-1-1-0-2', 'C-bind-1-1-0-3', 'C-bind-1-1-1-0', 'C-bind-1-1-1-1', 'C-bind-1-1-1-2', 'C-bind-1-1-1-3', 'M-Q-array-append', 'M-prepare-array-append', 'M-Q-health', 'M-prepared-health')

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
        observations=[save(side,case,query(conn,sql)) for side,conn in conns.items()]
        for obs in observations:
            if state:
                assert obs['status']==7 and obs['state']==state and not obs['rows'],(case,obs)
                assert all(obs['errors'][k] for k in 'SVCM'),obs
            else:
                assert obs['status']==2 and obs['metadata']==[[1016,-1,-1,0,'v',0,0]],(case,obs)
                assert obs['rows']==([] if zero else [[None if expected is None else expected.encode().hex()]]),(case,obs)
        for obs in observations[1:]:compare(observations[0],obs)
        passed.append(case)
    def interleave(label):
        for side,conn in conns.items():
            obs=save(side,'I-'+label,query(conn,'SELECT 1::int4 AS v'));assert obs['status']==2 and obs['rows']==[['31']]
        observations=[save('mysql-'+side,'I-'+label,m.query('SELECT v FROM health')) for side,m in mysqls.items()]
        for obs in observations:assert obs['code']==0 and obs['rows']==[['37']]
        mysql_compare(*observations)
        observations=[save('mysql-'+side,'I-'+label,m.query('SELECT 1 AS v'),phase='native-behavior') for side,m in mysqls.items()]
        try:
            mysql_compare(*observations)
            for obs in observations:assert obs['code']==0 and obs['rows']==[['31']]
            passed.append('I-'+label)
        except AssertionError as error:
            failures['I-'+label]=str(error)
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
        rust_binary = os.environ.get('PG08_22_RUST_BINARY')
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
        paired('C-Q-basic',"SELECT array_append('{1}'::int4[],2::int8) AS v",'{1,2}')
        paired('C-Q-wide',"SELECT array_append('{1}'::int4[],2147483648::int8) AS v",'{1,2147483648}')
        paired('C-Q-empty' ,"SELECT array_append('{}'::int4[],7::int8) AS v",'{7}',zero=False)
        paired('C-Q-null-array','SELECT array_append(NULL::int4[],7::int8) AS v','{7}',zero=False)
        paired('C-Q-null-both','SELECT array_append(NULL::int4[],NULL::int8) AS v','{NULL}',zero=False)
        paired('C-Q-null-element',"SELECT array_append('{1,NULL}'::int4[],NULL::int8) AS v",'{1,NULL,NULL}',zero=False)
        paired('C-Q-lower-zero',"SELECT array_append('[0:1]={10,20}'::int4[],7::int8) AS v",'[0:2]={10,20,7}',zero=False)
        paired('C-Q-lower-negative',"SELECT array_append('[-2:-1]={10,20}'::int4[],7::int8) AS v",'[-2:0]={10,20,7}',zero=False)
        paired('C-Q-near-upper',"SELECT array_append('[2147483645:2147483645]={1}'::int4[],7::int8) AS v",'[2147483645:2147483646]={1,7}',zero=False)
        paired('C-Q-zero','SELECT array_append(NULL::int4[],NULL::int8) AS v WHERE false',None,zero=True)
        paired('C-Q-nested',"SELECT array_append(array_append('{}'::int4[],1::int4),7::int8) AS v",'{1,7}',zero=False)

        for label,sql,state in [
            ('null-incompatible','SELECT array_append(NULL::int4[],NULL::text) AS v','42883'),
            ('signature',"SELECT array_append('x'::text,7::int8) AS v",'42883'),
            ('multidim',"SELECT array_append('{{1,2},{3,4}}'::int4[],7::int8) AS v",'22000'),
            ('upper-boundary',"SELECT array_append('[2147483646:2147483646]={1}'::int4[],7::int8) AS v",'54000')]:
            paired('C-E-'+label,sql,state=state)
            for side,conn in conns.items():
                obs=save(side,'C-recover-'+label,query(conn,'SELECT 1::int4 AS v'));assert obs['rows']==[['31']]
            passed.append('C-recover-'+label)
            for side,conn in conns.items():
                assert query(conn,'BEGIN')['status']==1
                obs=save(side,'C-txn-'+label,query(conn,sql),phase='query-error')
                assert obs['state']==state and transaction_status(conn)==3
                assert query(conn,'ROLLBACK')['status']==1 and transaction_status(conn)==0
                assert query(conn,'SELECT 1::int4 AS v')['rows']==[['31']]
            passed.append('C-txn-'+label)
        assert query(conns['reference'],f'CREATE TEMP TABLE {source_table}(value integer)')['status']==1
        reference_table_created=True
        assert query(conns['reference'],f'INSERT INTO {source_table} VALUES (2)')['status']==1
        assert mysqls['aster'].query(f'CREATE TABLE {source_table}(value INT)')['code']==0
        assert mysqls['aster'].query(f'INSERT INTO {source_table} VALUES (2)')['code']==0
        # A real row filters the typed array projection; array storage/column DDL is upstream.
        row_sql="SELECT array_append('[0:1]={10,20}'::int4[],7::int8) AS v FROM "+source_table+' WHERE value=2'
        paired('C-Q-row',row_sql,'[0:2]={10,20,7}')
        paired('C-Q-row-zero',row_sql+' AND false',zero=True)
        # Binary observations validate resulting shape, lower bounds and element NULLs.
        interleave('middle')
        for empty in (0,1):
            name=('arr22_'+str(empty)).encode();sql='SELECT array_append($1::int4[],$2::int8) AS v'+(' WHERE false' if empty else '')
            for side,conn in conns.items():
                assert save(side,'parse-'+str(empty),pq.observe(pq.PQprepare(conn,name,sql.encode(),0,None)))['status']==1
                desc=save(side,'describe-'+str(empty),pq.observe(pq.PQdescribePrepared(conn,name)))
                assert desc['parameters']==[1007,20] and desc['metadata']==[[1016,-1,-1,0,'v',0,0]],desc
            for infmt in (0,1):
                for outfmt in (0,1):
                    for index in range(4):
                        val=None if index==1 else (struct.pack('!9i',1,0,23,2,0,4,10,4,20) if infmt else b'[0:1]={10,20}')
                        if index==2:val=struct.pack('!3i',0,0,23) if infmt else b'{}'
                        item=None if index==2 else (struct.pack('!q',2147483648 if index==3 else 7) if infmt else b'2147483648' if index==3 else b'7')
                        encoded=[val,item];case=f'C-bind-{empty}-{infmt}-{outfmt}-{index}';observations=[]
                        for side,conn in conns.items():
                            obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,name,2,(c.c_char_p*2)(*encoded),(c.c_int*2)(*[0 if v is None else len(v) for v in encoded]),(c.c_int*2)(infmt,infmt),outfmt)),phase='Execute')
                            assert obs['status']==2 and obs['metadata']==[[1016,-1,-1,outfmt,'v',0,0]],(case,obs)
                            expected=('{7}' if index==1 else '{NULL}' if index==2 else '[0:2]={10,20,2147483648}' if index==3 else '[0:2]={10,20,7}')
                            lower,elements=(1,[7]) if index==1 else (1,[None]) if index==2 else (0,[10,20,2147483648]) if index==3 else (0,[10,20,7])
                            raw=struct.pack('!5i',1,int(None in elements),20,len(elements),lower)+b''.join(struct.pack('!i',-1) if v is None else struct.pack('!iq',8,v) for v in elements)
                            target=(raw if outfmt else expected.encode()).hex()
                            assert obs['rows']==([] if empty else [[target]]),(case,obs)
                            observations.append(obs)
                        for obs in observations[1:]:compare(observations[0],obs)
                        passed.append(case)
        sql='SELECT array_append(JSON_ARRAY(),7) AS v'
        observations=[save('mysql-'+side,'M-Q-array-append',m.query(sql)) for side,m in mysqls.items()]
        for obs in observations:assert obs['code']==1305 and obs['state']=='42000' and not obs['rows'],obs
        mysql_compare(*observations);passed.append('M-Q-array-append')
        observations=[save('mysql-'+side,'M-prepare-array-append',m.prepared(sql,[()])) for side,m in mysqls.items()]
        for phases in observations:assert any(o['code']==1305 and o['state']=='42000' for o in phases),phases
        assert observations[0]==observations[1],observations
        passed.append('M-prepare-array-append')
        observations=[save('mysql-'+side,'M-Q-health',m.query('SELECT 1 AS v')) for side,m in mysqls.items()]
        try:
            mysql_compare(*observations)
            for obs in observations:assert obs['rows']==[['31']] and obs['code']==0
            passed.append('M-Q-health')
        except AssertionError as error:
            failures['M-Q-health']=str(error)
        observations=[save('mysql-'+side,'M-prepared-health',m.prepared('SELECT v FROM health',[()])) for side,m in mysqls.items()]
        for phases in observations:
            assert [p['phase'] for p in phases]==['prepare','execute'],phases
            assert all(p['code']==0 for p in phases),phases
            assert phases[1]['rows']==[['37']],phases
        assert observations[0]==observations[1],observations
        passed.append('M-prepared-health')
        interleave('after')
        assert len(passed)==len(set(passed)) and set(passed)|set(failures)==set(CASE_IDS)
        assert not failures,failures
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
        category=lambda x: ('binding' if x.startswith('C-bind-') else 'interleaved' if x.startswith('I-') else 'mysql' if x.startswith('M-') else 'error' if x.startswith('C-E-') else 'recovery' if x.startswith('C-recover-') else 'transaction' if x.startswith('C-txn-') else 'zero' if x in ('C-Q-zero','C-Q-row-zero') else 'null' if x in ('C-Q-null-both','C-Q-null-element') else 'success')
        cleanup.update(failures=failures,connections_closed=True,negative_self_checks=3,expected=len(CASE_IDS),actual=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(CASE_IDS)-len(attempted),unique=len(attempted),case_ids=passed,expected_case_ids=CASE_IDS,
                       expected_counts={k:sum(category(x)==k for x in CASE_IDS) for k in set(map(category,CASE_IDS))},
                       counts={k:sum(category(x)==k for x in passed) for k in set(map(category,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(cleanup,indent=2))
    print('pg08_22_clients: 64 unique scenarios passed; 3 negative self-checks')

if __name__=='__main__':
    def interrupted(signum,_frame):raise SystemExit(128+signum)
    signal.signal(signal.SIGTERM,interrupted)
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
