# Copyright 2026 AsterSQL.
"""Analyzed pg_typeof identity, NULLs and protocol metadata through native clients."""
import copy
import ctypes as c
import hashlib
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
from pg_function_23_libpq import Libpq, compare, self_test
from pg_function_27_libpq import Mysql
from pg_function_25_libpq import mysql_compare

CASES = (
 ('int4','pg_typeof(1::int4)','integer',23,False),
 ('nullint4','pg_typeof(NULL::int4)','integer',23,False),
 ('nulltext','pg_typeof(NULL::text)','text',25,False),
 ('text',"pg_typeof('hi'::text)",'text',25,False),
 ('nested','COALESCE(pg_typeof(NULL::int4),pg_typeof(NULL::int4))','integer',23,False),
 ('nestedtype','pg_typeof(pg_typeof(NULL::text))','regtype',2206,False),
 ('row','pg_typeof(v)','integer',23,False),
 ('zero','pg_typeof(v)','integer',23,True),
 ('allnull','pg_typeof(NULL::int4)','integer',23,False),
)
CASE_IDS = tuple([f'PG-{label}-{mode}' for label,*_ in CASES for mode in ('Q','P0','P1')]
 + [f'error-{case}-{phase}' for case in ('arity','cast') for phase in ('Q','P','txn')]
 + [f'bind-{tp}-{inf}-{outf}-{value}' for tp in ('int4','text') for inf in (0,1) for outf in (0,1) for value in ('value','null','again')]
 + [f'M-{fn}-{mode}' for fn in ('current_database','current_schema','pg_typeof','pg_backend_pid','database','version','connection_id','health') for mode in ('Q','P')]
 + ['I-before','I-middle','I-after'])
assert len(CASE_IDS)==76 and len(set(CASE_IDS))==76


def run(port, database, evidence, mysql_port):
    assert database.replace('_','').isalnum()
    evidence.mkdir(parents=True, exist_ok=False)
    pq = Libpq()
    assert pq.PQlibVersion() >= 180000
    tx = pq.lib.PQtransactionStatus;tx.restype=c.c_int;tx.argtypes=[c.c_void_p]
    conns, mysqls, passed, attempted, cleanup = {}, {}, [], set(), {}
    failures = {}
    native_facts = []
    owned_pg, owned_mysql = [], False
    def query(conn, sql): return pq.observe(pq.PQexec(conn, sql.encode()))
    def save(side, case, obs, phase='query'):
        if case in CASE_IDS: attempted.add(case)
        with (evidence/(side+'.jsonl')).open('a') as f:f.write(json.dumps(dict(case_id=case,phase=phase,observation=obs))+'\n')
        return obs
    def check_pg(obs, name, oid, zero=False, fmt=0):
        assert obs['status']==2 and obs['metadata']==[[2206,4,-1,fmt,'v',0,0]],obs
        payload=(oid.to_bytes(4,'big') if fmt else name.encode()).hex()
        assert obs['rows']==([] if zero else [[payload]]),obs
    def interleave(label):
        case='I-'+label
        for side,conn in conns.items():
            obs=save(side,case,query(conn,'SELECT pg_typeof(1::int4) AS v'))
            check_pg(obs,'integer',23)
        peers=[save('mysql-'+side,case,m.query('SELECT v FROM health')) for side,m in mysqls.items()]
        for obs in peers:assert obs['code']==0 and obs['rows']==[['37']],obs
        mysql_compare(*peers);passed.append(case)
    try:
        admin=pq.PQconnectdb(os.environ['PG18_REFERENCE_DSN'].encode())
        assert admin and pq.PQstatus(admin)==0,'reference connection failed (DSN redacted)'
        identity={sql:query(admin,sql) for sql in ('SELECT version()','SHOW server_version_num','SHOW server_encoding','SHOW client_encoding','SHOW lc_time','SHOW lc_numeric','SHOW lc_monetary','SHOW TimeZone','SHOW search_path')}
        assert identity['SHOW server_version_num']['rows']==[['313830303036']]
        save('reference','identity',identity)
        repo=Path(__file__).resolve().parents[3]
        bins={}
        for key,env,name in [('postgres','PG08_REFERENCE_BIN','postgres'),('mysql','PG08_MYSQL_BIN','mysqld')]:
            binary=Path(os.environ[env])/name
            bins[key]=dict(version=subprocess.check_output([str(binary),'--version'],text=True).strip(),sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
        tz=Path(os.environ['PG08_TZDATA_SOURCE']).read_bytes()
        save('build','identity',dict(head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),fixture_sha256=hashlib.sha256(Path(__file__).read_bytes()).hexdigest(),libpq=pq.PQlibVersion(),binaries=bins,tzdata_version=tz.splitlines()[0].decode(),tzdata_sha256=hashlib.sha256(tz).hexdigest(),source_diff_sha256=hashlib.sha256(subprocess.check_output(['git','diff','HEAD','--','pkg'],cwd=repo)).hexdigest(),rust_binary_sha256=hashlib.sha256(Path(os.environ['PG08_30_RUST_BINARY']).read_bytes()).hexdigest()))
        for suffix in ('','_b'):
            db=database+suffix
            assert query(admin,f'CREATE DATABASE {db}')['status']==1
            owned_pg.append(db)
            for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']+f' dbname={db}'),('aster',f'host=127.0.0.1 port={port} user=root dbname={db}')]:
                key=side+('-B' if suffix else '-A')
                conn=pq.PQconnectdb(dsn.encode());assert conn and pq.PQstatus(conn)==0,'login failed (DSN redacted)';conns[key]=conn
                assert query(conn,'CREATE TABLE pg0830_source(v integer)')['status']==1
                assert query(conn,'INSERT INTO pg0830_source VALUES(7)')['status']==1
        mysqls['reference']=Mysql(os.environ['MYSQL8_REFERENCE_HOST'],int(os.environ['MYSQL8_REFERENCE_PORT']),os.environ['MYSQL8_REFERENCE_USER'],os.environ['MYSQL8_REFERENCE_PASSWORD'])
        mysqls['aster']=Mysql('127.0.0.1',mysql_port,'root','',database)
        assert mysqls['reference'].query(f'CREATE DATABASE {database}')['code']==0;owned_mysql=True
        assert mysqls['reference'].query(f'USE {database}')['code']==0
        for side,m in mysqls.items():
            save('mysql-'+side,'identity',{sql:m.query(sql) for sql in ('SELECT version()','SELECT @@sql_mode','SELECT @@character_set_connection','SELECT @@collation_connection')})
            server_info=m.lib.mysql_get_server_info;server_info.restype=c.c_char_p;server_info.argtypes=[c.c_void_p]
            thread=m.lib.mysql_thread_id;thread.restype=c.c_ulong;thread.argtypes=[c.c_void_p]
            save('mysql-'+side,'native-identity',dict(client_version=m.mysql_get_client_version(),server_info=server_info(m.conn).decode(),connection_id=thread(m.conn)))
            assert m.query('SET NAMES utf8mb4 COLLATE utf8mb4_bin')['code']==0
            assert m.query('CREATE TABLE health(v INT)')['code']==0
            assert m.query('INSERT INTO health VALUES(7)')['code']==0
        interleave('before')
        for label,expr,name,oid,zero in CASES:
            sql=f'SELECT {expr} AS v'+(' FROM pg0830_source' if label in ('row','zero','allnull') else '')+(' WHERE false' if zero else '')
            for mode in ('Q','P0','P1'):
                case=f'PG-{label}-{mode}';peers={}
                for side,conn in conns.items():
                    fmt=0 if mode=='Q' else int(mode[-1])
                    if mode=='Q':obs=query(conn,sql)
                    else:
                        statement=('type_'+label+mode).encode()
                        assert pq.observe(pq.PQprepare(conn,statement,sql.encode(),0,None))['status']==1
                        desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)))
                        assert desc['parameters']==[] and desc['metadata']==[[2206,4,-1,0,'v',0,0]],desc
                        obs=pq.observe(pq.PQexecPrepared(conn,statement,0,None,None,None,fmt))
                    save(side,case,obs,mode);check_pg(obs,name,oid,zero,fmt);peers[side]=obs
                for suffix in ('A','B'):compare(peers['reference-'+suffix],peers['aster-'+suffix])
                passed.append(case)
        for tp,oid,name in [('int4',23,'integer'),('text',25,'text')]:
          for inf in (0,1):
           for outf in (0,1):
            statement=f'bound_{tp}_{inf}_{outf}'.encode()
            for side,conn in conns.items():
                assert pq.observe(pq.PQprepare(conn,statement,f'SELECT pg_typeof($1::{tp}) AS v'.encode(),0,None))['status']==1
                desc=save(side,'describe',pq.observe(pq.PQdescribePrepared(conn,statement)))
                assert desc['parameters']==[oid] and desc['metadata']==[[2206,4,-1,0,'v',0,0]],desc
            for label,value in [('value',7),('null',None),('again',9)]:
                case=f'bind-{tp}-{inf}-{outf}-{label}';peers={}
                payload=None if value is None else value.to_bytes(4,'big') if tp=='int4' and inf else str(value).encode()
                for side,conn in conns.items():
                    values=(c.c_char_p*1)(payload);lengths=(c.c_int*1)(0 if payload is None else len(payload));formats=(c.c_int*1)(inf)
                    obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,statement,1,values,lengths,formats,outf)),'bind-execute')
                    check_pg(obs,name,oid,False,outf);peers[side]=obs
                for suffix in ('A','B'):compare(peers['reference-'+suffix],peers['aster-'+suffix])
                passed.append(case)
        interleave('middle')
        for label,sql in [('arity','SELECT pg_typeof() AS v'),('cast',"SELECT pg_typeof('bad'::int4) AS v")]:
          for phase in ('Q','P','txn'):
            case=f'error-{label}-{phase}';peers={}
            for side,conn in conns.items():
                if phase=='txn':assert query(conn,'BEGIN')['status']==1
                if phase=='P':
                    obs=save(side,case,pq.observe(pq.PQprepare(conn,b'bad',sql.encode(),0,None)),'parse-analyze')
                    if obs['status']==1:obs=save(side,case,pq.observe(pq.PQexecPrepared(conn,b'bad',0,None,None,None,0)),'plan-execute')
                else:obs=save(side,case,query(conn,sql),phase)
                assert obs['status']==7 and obs['state']==('42883' if label=='arity' else '22P02') and not obs['rows'] and not obs['metadata'],obs
                assert all(obs['errors'][key] for key in 'SVCM'),obs
                peers[side]=obs
                if phase=='txn':
                    assert tx(conn)==3
                    assert query(conn,'ROLLBACK')['status']==1 and tx(conn)==0
                # libpq synchronizes failed extended Parse; next query must succeed.
                health=save(side,case,query(conn,'SELECT 1::int4 AS v'),'recovery');assert health['rows']==[['31']],health
                assert query(conn,'SELECT v FROM pg0830_source')['rows']==[['37']]
            for suffix in ('A','B'):compare(peers['reference-'+suffix],peers['aster-'+suffix])
            passed.append(case)
        for fn in ('current_database','current_schema','pg_typeof','pg_backend_pid','database','version','connection_id','health'):
          sql='SELECT '+('v FROM health' if fn=='health' else fn+('('+('1' if fn=='pg_typeof' else '')+')'))+' AS v' if fn!='health' else 'SELECT v FROM health'
          for mode in ('Q','P'):
            case=f'M-{fn}-{mode}';peers={}
            try:
                for side,m in mysqls.items():
                    obs=save('mysql-'+side,case,m.query(sql) if mode=='Q' else m.prepared(sql,[()]),mode)
                    phases=[obs] if mode=='Q' else obs
                    if fn.startswith(('current_','pg_')):
                        assert any(p['code']==1305 and p['state']=='42000' for p in phases),obs
                    else:
                        assert all(p['code']==0 for p in phases),obs
                        result=phases[-1]
                        if fn=='database':assert result['rows']==[[database.encode().hex()]],obs
                        elif fn=='health':assert result['rows']==[['37']],obs
                        elif fn=='version':
                            # VERSION must match the independent client's handshake identity.
                            version=m.lib.mysql_get_server_info;version.restype=c.c_char_p;version.argtypes=[c.c_void_p]
                            assert result['rows']==[[version(m.conn).hex()]],dict(observation=obs,handshake_version=version(m.conn).decode())
                        else:
                            thread=m.lib.mysql_thread_id;thread.restype=c.c_ulong;thread.argtypes=[c.c_void_p]
                            assert int(bytes.fromhex(result['rows'][0][0]))==thread(m.conn),obs
                    peers[side]=obs
                if fn in ('database','version','connection_id'):native_facts.append(case)
                if fn not in ('version','connection_id'):
                    if mode=='Q':mysql_compare(peers['reference'],peers['aster'])
                    else:assert peers['reference']==peers['aster'],peers
                else:
                    # Values are connection/build facts; compare all column definitions and diagnostics.
                    a,b=copy.deepcopy(peers['reference']),copy.deepcopy(peers['aster'])
                    for left,right in zip([a] if mode=='Q' else a,[b] if mode=='Q' else b):
                        if fn=='version':
                            for side,record in [('reference',left),('aster',right)]:
                                # VERSION's descriptor width follows its own handshake
                                # identity under utf8mb4, including Prepare and Execute.
                                raw=mysqls[side].lib.mysql_get_server_info(mysqls[side].conn)
                                for field in record['metadata']:
                                    assert field[4]==len(raw)*4,(case,side,field,len(raw))
                                    field[4]=0
                        left['rows']=right['rows']=[]
                    assert a==b,(case,a,b)
                passed.append(case)
            except AssertionError as error:
                failures[case] = str(error)
        interleave('after')
        assert set(passed)|set(failures)==set(CASE_IDS) and len(passed)==len(set(passed))
        assert not failures, failures
    finally:
        for key,conn in conns.items():
            if conn:
                if tx(conn) in (2,3):query(conn,'ROLLBACK')
                cleanup[key+'-table-dropped']=query(conn,'DROP TABLE IF EXISTS pg0830_source')['status']==1
                pq.PQfinish(conn)
        if 'admin' in locals() and admin:
            cleanup['reference_database_dropped']=all(query(admin,f'DROP DATABASE {db}')['status']==1 for db in owned_pg)
            pq.PQfinish(admin)
        if owned_mysql:cleanup['mysql-reference-database-dropped']=mysqls['reference'].query(f'DROP DATABASE {database}')['code']==0
        for m in mysqls.values():m.close()
        cat=lambda x:'binding' if x.startswith('bind-') else 'zero' if x.startswith('PG-zero-') else 'null' if x.startswith(('PG-null','PG-allnull')) else 'binding' if x.endswith(('P0','P1')) else 'zero' if '-1-' in x else 'error-recovery' if x.startswith('error-') else 'interleaved' if x.startswith('I-') else 'mysql' if x.startswith('M-') else 'success'
        summary=dict(failures=failures,mysql_native_facts_passed=native_facts, null_input="typed int4/text NULL and NULL rebinding are nonstrict", recovery_groups=6, expected=len(CASE_IDS),actual=len(attempted),unique=len(attempted),passed=len(passed),failed=len(attempted-set(passed)),skipped=len(set(CASE_IDS)-attempted),negative_self_checks=3,connections_closed=True,reference_database_dropped=cleanup.get('reference_database_dropped',False),cleanup=cleanup,case_ids=passed,expected_case_ids=CASE_IDS,counts={k:sum(cat(x)==k for x in passed) for k in set(map(cat,CASE_IDS))})
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    assert all(cleanup.values()),cleanup
    print(json.dumps(summary))

if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_:sys.exit(143))
    assert self_test()==3
    if sys.argv[1:]!=['--self-test']:run(int(sys.argv[1]),sys.argv[2],Path(sys.argv[3]),int(sys.argv[4]))
