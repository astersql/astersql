# Copyright 2026 AsterSQL.
"""Read-only CTE frozen cases, real libpq and MySQL8 clients; no SQL mocks."""
from contextlib import contextmanager
import ctypes as c
import itertools
import json
import os
from pathlib import Path
import signal
import struct
import sys
from pg_function_23_libpq import Libpq, Wire, Mysql, MysqlBind


def encoded(rows):
    return [[None if v is None else str(v).encode().hex() for v in row] for row in rows]



def run(evidence, port, database, mysql_port):
    evidence.mkdir(parents=True, exist_ok=True)
    pq = Libpq()
    summary = dict(mysql_client_found_rows=False, passed=False, cases=0, mysql_cases=0, libpq=pq.PQlibVersion(), cleanup={}, failures=[])
    pq.lib.PQcmdStatus.restype = c.c_char_p
    pq.lib.PQcmdStatus.argtypes = [c.c_void_p]
    connections, mysqls = [], []
    prepared_scope = None
    observations = {}
    table = 'public.cte_read_doc'
    scan = f'SELECT id,v FROM {table} ORDER BY id'
    base = [(1,10),(2,None)]
    target = f'host=127.0.0.1 port={port} user=root dbname={database} sslmode=disable'

    def save(side, case, phase, sql, obs):
        with (evidence / (side + '.jsonl')).open('a') as f:
            f.write(json.dumps(dict(case=case, phase=phase, sql=sql, observation=obs)) + '\n')
        return obs

    def query(wire, side, case, phase, sql, ready=None):
        obs = save(side, case, phase, sql, wire.query(sql))
        if ready:
            assert not obs['errors'] and obs['ready'] == ready, (case, phase, obs)
        return obs

    def readback(wire, side, case, phase, expected):
        obs = query(wire, side, case, phase, scan)
        assert not obs['errors'] and obs['rows'] == encoded(expected), (case, phase, obs, expected)
        return obs

    def parse(w, side, case, sql, oids):
        obs = save(side, case, 'parse', sql, w.phase(b'P', b's\0' + sql.encode() + b'\0' + struct.pack('!H', len(oids)) + b''.join(struct.pack('!I', o) for o in oids), '1'))
        if obs['errors']:
            save(side, case, 'parse-sync', sql, w.sync())
        return obs

    def bind(w, side, case, values, portal='p'):
        body = portal.encode() + b'\0s\0' + struct.pack('!HH', 0, len(values))
        for value in values:
            data = None if value is None else str(value).encode()
            body += struct.pack('!i', -1 if data is None else len(data)) + (data or b'')
        obs = save(side, case, 'bind', '', w.phase(b'B', body + struct.pack('!H', 0), '2'))
        assert not obs['errors'], obs

    def execute(w, side, case, sql, extended, values=()):
        if not extended:
            return query(w, side, case, 'execute', sql)
        bind(w, side, case, values)
        desc = save(side, case, 'portal', sql, w.phase(b'D', b'Pp\0', 'Tn'))
        assert not desc['errors'], desc
        w.send(b'E', b'p\0' + struct.pack('!I', 0))
        w.send(b'H')
        result = w.read('C')
        sync = w.sync()
        result['ready'] = sync['ready']
        result['metadata'] = desc['metadata']
        return save(side, case, 'execute', sql, result)

    @contextmanager
    def mysql_prepared_int(client, case, sql, values=(1,), allow_warnings=False):
        # Preserve the real statement across before/during/after PG observations.
        stmt = client.mysql_stmt_init(client.conn)
        assert stmt
        try:
            assert client.mysql_stmt_prepare(stmt, sql.encode(), len(sql.encode())) == 0
            assert client.mysql_stmt_param_count(stmt) == len(values)
            params=(MysqlBind*len(values))()
            keep=[]
            for i,value in enumerate(values):
                buf=c.create_string_buffer(value.encode()) if isinstance(value,str) else c.c_int(value)
                length=c.c_ulong(len(value.encode()) if isinstance(value,str) else 4)
                null=c.c_bool(False)
                keep.extend((buf,length,null))
                params[i].buffer_type=253 if isinstance(value,str) else 3
                params[i].buffer=c.addressof(buf)
                params[i].buffer_length=length.value
                params[i].length,params[i].is_null=c.addressof(length),c.addressof(null)
            assert not client.mysql_stmt_bind_param(stmt,params)
            def execute_native(label):
                assert client.mysql_stmt_execute(stmt) == 0
                result = client.mysql_stmt_result_metadata(stmt)
                try:
                    meta = client.metadata(result)
                finally:
                    if result: client.mysql_free_result(result)
                client.lib.mysql_stmt_affected_rows.restype = c.c_ulonglong
                client.lib.mysql_stmt_affected_rows.argtypes = [c.c_void_p]
                obs = dict(metadata=meta, rows=[], code=client.mysql_stmt_errno(stmt),
                           state=client.mysql_stmt_sqlstate(stmt).decode(),
                           affected_rows=client.lib.mysql_stmt_affected_rows(stmt),
                           warning_count=client.mysql_warning_count(client.conn))
                if meta:
                    buffers = [c.create_string_buffer(4096) for _ in meta]
                    lengths = [c.c_ulong() for _ in meta]
                    nulls = [c.c_bool() for _ in meta]
                    errors = [c.c_bool() for _ in meta]
                    binds = (MysqlBind * len(meta))()
                    for i in range(len(meta)):
                        binds[i].buffer_type = 253
                        binds[i].buffer = c.addressof(buffers[i])
                        binds[i].buffer_length = 4096
                        binds[i].length, binds[i].is_null = c.addressof(lengths[i]), c.addressof(nulls[i])
                        binds[i].error = c.addressof(errors[i])
                    assert not client.mysql_stmt_bind_result(stmt, binds)
                    assert client.mysql_stmt_store_result(stmt) == 0
                    while True:
                        rc = client.mysql_stmt_fetch(stmt)
                        if rc == 100: break
                        assert rc == 0 and not any(e.value for e in errors)
                        obs['rows'].append([None if nulls[i].value else buffers[i].raw[:lengths[i].value].hex() for i in range(len(meta))])
                    assert not client.mysql_stmt_free_result(stmt)
                obs['warnings'] = client.query('SHOW WARNINGS', warnings=False)['rows']
                save('mysql', case, label, sql, obs)
                assert obs['code'] == 0 and (allow_warnings or (obs['warning_count'] == 0 and not obs['warnings'])), obs
                return obs
            yield execute_native
        finally:
            assert not client.mysql_stmt_close(stmt)

    def mysql_query(client, case, sql):
        obs = client.query(sql, warnings=False)
        client.lib.mysql_affected_rows.restype = c.c_ulonglong
        client.lib.mysql_affected_rows.argtypes = [c.c_void_p]
        obs['affected_rows'] = client.lib.mysql_affected_rows(client.conn)
        obs['warnings'] = client.query('SHOW WARNINGS', warnings=False)['rows']
        save('mysql', case, 'query', sql, obs)
        assert obs['code'] == 0 and obs['warning_count'] == 0, obs
        return obs

    try:
        for _ in range(2): mysqls.append(Mysql('127.0.0.1',mysql_port,'root','',database))
        m,n=mysqls
        for client in mysqls: mysql_query(client,'setup','SET autocommit=1')
        mysql_query(m,'setup','CREATE TABLE cte_mysql_doc(id INT PRIMARY KEY,v INT)')
        mysql_query(m,'setup','INSERT INTO cte_mysql_doc VALUES(1,10),(2,NULL)')
        mysql_query(m,'identity','SELECT @@sql_mode,@@transaction_isolation,DATABASE()')
        native_sql='WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=2 ORDER BY id'
        native_scan='SELECT id,v FROM cte_mysql_doc ORDER BY id'
        with mysql_prepared_int(m,'before-zero','WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=? ORDER BY id',(3,)) as read:
            assert read('empty')['rows']==[]
        mysql_query(m,'before','BEGIN')
        mysql_query(m,'before','INSERT INTO cte_mysql_doc VALUES(3,30)')
        prepared_scope=mysql_prepared_int(m,'coexist','WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=? ORDER BY id',(2,))
        prepared_read=prepared_scope.__enter__()
        native_base=mysql_query(m,'before',native_sql)
        prepared_base=prepared_read('before')
        assert native_base['rows']==prepared_base['rows']==encoded([(2,None)])
        def coexist(label):
            assert mysql_query(m,label,native_sql)==native_base
            assert prepared_read(label)==prepared_base
            assert mysql_query(m,label,native_scan)['rows']==encoded(base+[(3,30)])
            assert mysql_query(n,label,native_scan)['rows']==encoded(base)
        for side,dsn in [('reference',os.environ['PG18_REFERENCE_DSN']),('target',target)]:
            w,o=Wire(pq,dsn),Wire(pq,dsn)
            connections.extend((w,o))
            identity=query(w,side,'identity','version','SELECT version()','I')
            if side=='reference':
                assert query(w,side,'identity','number','SHOW server_version_num','I')['rows']==encoded([(180006,)])
                summary['reference_build']=identity['rows']
            for conn in (w,o):
                query(conn,side,'setup','encoding',"SET client_encoding='UTF8'",'I')
                query(conn,side,'setup','timezone',"SET TIME ZONE 'UTC'" if side=='reference' else "SET time_zone='+00:00'",'I')
                query(conn,side,'setup','isolation','SHOW transaction_isolation' if side=='reference' else 'SELECT @@transaction_isolation','I')
            def setup(case):
                query(w,side,case,'create',f'CREATE TABLE {table}(id integer PRIMARY KEY,v integer)','I')
                query(w,side,case,'seed',f'INSERT INTO {table} VALUES(1,10),(2,NULL)','I')
                for conn in (w,o): readback(conn,side,case,'before',base)
            def cleanup(case):
                w.sync(); query(w,side,case,'rollback','ROLLBACK','I')
                query(w,side,case,'drop',f'DROP TABLE IF EXISTS {table}','I')
            successes=[
                ('chain',f'WITH a(k,val) AS (SELECT id,v FROM {table}), b AS (SELECT k,val FROM a WHERE k=2) SELECT k,val FROM b ORDER BY k',[(2,None)]),
                ('zero',f'WITH a(k,val) AS (SELECT id,v FROM {table}), b AS (SELECT k,val FROM a WHERE k=3) SELECT k,val FROM b ORDER BY k',[]),
                ('repeat',f'WITH a AS (SELECT v FROM {table} UNION ALL SELECT v FROM {table}) SELECT l.v AS lv,r.v AS rv FROM a AS l CROSS JOIN a AS r ORDER BY lv NULLS LAST,rv NULLS LAST',[(10,10)]*4+[(10,None)]*4+[(None,10)]*4+[(None,None)]*4),
                ('shadow',f'WITH cte_read_doc AS (SELECT 7 AS id) SELECT c.id AS local_id,t.id AS table_id FROM cte_read_doc AS c CROSS JOIN {table} AS t ORDER BY table_id',[(7,1),(7,2)]),
                ('nested','WITH a AS (SELECT 1 AS v) SELECT a.v AS outer_v,x.v AS inner_v FROM a CROSS JOIN (WITH a AS (SELECT 2 AS v) SELECT v FROM a) AS x',[(1,2)]),
                ('partial-alias',f'WITH a(x) AS (SELECT id,v FROM {table}) SELECT x,v FROM a ORDER BY x',base),
                ('unused-error','WITH a AS (SELECT 1/0 AS v) SELECT 7 AS marker',[(7,)])]
            successes += [
                ('partial-zero',f'WITH a(x) AS (SELECT id,v FROM {table} WHERE id=3) SELECT x,v FROM a',[]),
                ('partial-null',f'WITH a(x) AS (SELECT id,v FROM {table} WHERE id=2) SELECT v FROM a',[(None,)]),
                ('duplicate-unused',f'WITH a(x,x) AS (SELECT id,v FROM {table}) SELECT 1::int4 AS ok FROM a',[(1,),(1,)]),
                ('prior-scope',f'WITH a AS (SELECT id FROM {table} WHERE id=1),b AS (WITH a AS (SELECT id FROM a) SELECT id FROM a) SELECT id FROM b',[(1,)]),
                ('self-base','WITH cte_read_doc AS (SELECT id FROM cte_read_doc) SELECT id FROM cte_read_doc ORDER BY id',[(1,),(2,)]),
                ('fresh','WITH cte_read_doc AS (SELECT 7 AS fresh) SELECT fresh FROM cte_read_doc',[(7,)]),
                ('unused-table',f'WITH unused AS (SELECT 1::int4/0::int4 AS bad) SELECT id,v FROM {table} ORDER BY id',base),
                ('unused-zero',f'WITH unused AS (SELECT 1::int4/0::int4 AS bad),a AS (SELECT id,v FROM {table}) SELECT id,v FROM a WHERE id=3',[]),
                ('unused-null',f'WITH unused AS (SELECT 1::int4/0::int4 AS bad) SELECT v FROM {table} WHERE id=2',[(None,)]),
                ('unused-chain',f'WITH unused AS (SELECT 1::int4/0::int4 AS bad),dead AS (SELECT bad FROM unused),a AS (SELECT id,v FROM {table}),b AS (SELECT id,v FROM a) SELECT id,v FROM b ORDER BY id',base),
            ]
            repeat_sql=successes[2][1].split(' ORDER BY')[0]
            successes += [(name,repeat_sql+where,rows) for name,where,rows in (
                ('repeat-left',' WHERE l.v=10 AND r.v IS NULL',[(10,None)]*4),
                ('repeat-right',' WHERE l.v IS NULL AND r.v=10',[(None,10)]*4),
                ('repeat-empty',' WHERE l.v=99',[]))]
            for (name,sql,expected),extended,commit in itertools.product(successes,(False,True),(False,True)):
                case=f'{name}-{extended}-{commit}'
                try:
                    setup(case)
                    query(w,side,case,'begin','BEGIN','T')
                    if extended:
                        assert not parse(w,side,case,sql,[])['errors']
                        desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','T'))
                        assert not desc['errors']
                        assert w.sync()['ready']=='T'
                        for conn in (w,o): readback(conn,side,case,'describe-no-effect',base)
                    result=execute(w,side,case,sql,extended)
                    assert not result['errors'] and result['ready']=='T', (case,result)
                    assert result['rows']==encoded(expected),(case,result)
                    assert result['commands']==[f'SELECT {len(expected)}'],(case,result)
                    assert result['metadata'] and all(col[:4]==[23,4,-1,0] for col in result['metadata']), (case,result)
                    if extended:
                        assert result['metadata']==desc['metadata'],(case,result,desc)
                        assert not w.phase(b'C',b'Pp\0','3')['errors']
                        assert not w.phase(b'C',b'Ss\0','3')['errors']
                        assert w.sync()['ready']=='T'
                    observations.setdefault(case,{})[side]=dict(rows=result['rows'],metadata=[col[:5]+[bool(col[5]),col[6]] for col in result['metadata']],commands=result['commands'])
                    for conn in (w,o): readback(conn,side,case,'after',base)
                    query(w,side,case,'end','COMMIT' if commit else 'ROLLBACK','I')
                    for conn in (w,o): readback(conn,side,case,'final',base)
                    summary['cases']+=1
                finally: cleanup(case)
            errors=[('forward','WITH a AS (SELECT v FROM b), b AS (SELECT 2 AS v) SELECT v FROM a','42P01'),('column','WITH a AS (SELECT 1 AS v) SELECT missing FROM a','42703'),('duplicate','WITH a AS (SELECT 1), a AS (SELECT 2) SELECT * FROM a','42712'),('leak','SELECT k FROM a','42P01'),('recursive','WITH RECURSIVE r(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM r WHERE n<3) SELECT n FROM r ORDER BY n','0A000')]
            errors += [
                ('column-zero','WITH a AS (SELECT 1 AS v WHERE false) SELECT missing FROM a','42703'),
                ('too-many-aliases',f'WITH a(x,y,z) AS (SELECT id,v FROM {table}) SELECT x FROM a','42P10'),
                ('ambiguous',f'WITH a(x,x) AS (SELECT id,v FROM {table}) SELECT x FROM a','42702'),
                ('ambiguous-qualified',f'WITH a(x,x) AS (SELECT id,v FROM {table}) SELECT a.x FROM a','42702'),
                ('unused-invalid',f'WITH unused AS (SELECT missing FROM {table}) SELECT 1 AS ok','42703'),
                ('consumer-runtime',repeat_sql.replace('l.v AS lv,r.v AS rv','10::int4/(l.v-10::int4)::int4 AS bad')+' WHERE l.v=10','22012'),
                ('materialized','WITH a AS MATERIALIZED (SELECT 1 AS v) SELECT v FROM a','0A000'),
                ('not-materialized','WITH a AS NOT MATERIALIZED (SELECT 1 AS v) SELECT v FROM a','0A000'),
                ('search','WITH a AS (SELECT 1 AS v) SEARCH DEPTH FIRST BY v SET ord SELECT v FROM a','0A000'),
                ('cycle','WITH a AS (SELECT 1 AS v) CYCLE v SET mark USING path SELECT v FROM a','0A000'),
                ('lock',f'WITH a AS (SELECT id FROM {table} FOR UPDATE) SELECT id FROM a','0A000'),
                ('volatile','WITH unused AS (SELECT random()) SELECT 1 AS ok','0A000'),
                ('sequence',"WITH unused AS (SELECT nextval('absent')) SELECT 1 AS ok",'0A000'),
            ]
            for (name,sql,state),extended,transaction in itertools.product(errors,(False,True),(False,True)):
                case=f'{name}-{extended}-{transaction}'
                try:
                    setup(case)
                    if transaction:
                        query(w,side,case,'begin','BEGIN','T')
                        query(w,side,case,'prefix',f'INSERT INTO {table} VALUES(3,30)','T')
                    phase='query'
                    if extended:
                        phase='parse'; result=parse(w,side,case,sql,[])
                        if not result['errors']:
                            phase='execute'; result=execute(w,side,case,sql,True)
                    else: result=query(w,side,case,'execute',sql)
                    ready=w.sync()['ready']
                    reference_success={'recursive':[(1,),(2,),(3,)],'materialized':[(1,)],'not-materialized':[(1,)],'lock':[(1,),(2,)]+([(3,)] if transaction else []),'volatile':[(1,)]}
                    limit=state=='0A000'
                    if limit and side=='reference' and name in reference_success:
                        assert not result['errors'] and result['rows']==encoded(reference_success[name]),(case,result)
                        query(w,side,case,'rollback','ROLLBACK','I')
                    else:
                        expected_state=({'search':'42601','cycle':'42601','sequence':'42P01'}.get(name,state) if side=='reference' else state)
                        assert [e['C'] for e in result['errors']]==[expected_state],(case,result)
                        assert not result['rows'] and not result['commands'],result
                        assert ready==('E' if transaction else 'I'),(case,ready)
                        readback(o,side,case,'observer',base)
                        if transaction:
                            failed=query(w,side,case,'failed','SELECT 1 AS ok')
                            assert [e['C'] for e in failed['errors']]==['25P02'] and w.sync()['ready']=='E',failed
                        query(w,side,case,'rollback','ROLLBACK','I')
                        if not limit: observations.setdefault(case,{})[side]=dict(state=state,phase=phase,ready=ready)
                    recovered=query(w,side,case,'recovery','SELECT 1 AS ok','I')
                    assert recovered['rows']==encoded([(1,)])
                    fresh=query(w,side,case,'fresh-scope','WITH a AS (SELECT 7::int4 AS v) SELECT v FROM a','I')
                    assert fresh['rows']==encoded([(7,)]) and fresh['commands']==['SELECT 1'],fresh
                    for conn in (w,o): readback(conn,side,case,'restored',base)
                    summary['cases']+=1
                finally:
                    w.sync(); w.query('ROLLBACK')
                    for tag,body in ((b'C',b'Pp\0'),(b'C',b'Ss\0')):
                        w.phase(tag,body,'3'); w.sync()
                    cleanup(case)
            for commit in (False,True):
                case=f'parameters-{commit}'
                sql=successes[0][1].replace('k=2','k=$1')
                try:
                    setup(case)
                    assert not parse(w,side,case,sql,[23])['errors']
                    desc=save(side,case,'statement',sql,w.phase(b'D',b'Ss\0','T'))
                    assert desc['parameters']==[23] and not desc['errors'],desc
                    w.sync(); query(w,side,case,'begin','BEGIN','T')
                    for value,expected in ((2,[(2,None)]),(3,[]),(None,[])):
                        result=execute(w,side,case,sql,True,[value])
                        assert not result['errors'] and result['rows']==encoded(expected) and result['commands']==[f'SELECT {len(expected)}'],result
                        assert result['metadata']==desc['metadata'],result
                        repeated=save(side,case,'repeat',sql,w.phase(b'E',b'p\0'+struct.pack('!I',0),'C'))
                        assert not repeated['errors'] and not repeated['rows'] and repeated['commands']==['SELECT 0'],repeated
                        assert not w.phase(b'C',b'Pp\0','3')['errors']; w.sync()
                        for conn in (w,o): readback(conn,side,case,'unchanged',base)
                    assert not w.phase(b'C',b'Ss\0','3')['errors']; w.sync()
                    query(w,side,case,'end','COMMIT' if commit else 'ROLLBACK','I')
                    for conn in (w,o): readback(conn,side,case,'final',base)
                    summary['cases']+=1
                finally: cleanup(case)
            coexist(side)
        coexist('after')
        prepared_scope.__exit__(None,None,None); prepared_scope=None
        mysql_query(m,'after','COMMIT')
        for client in (m,n): assert mysql_query(client,'committed',native_scan)['rows']==encoded(base+[(3,30)])
        with mysql_prepared_int(m,'zero','WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=? ORDER BY id',(3,)) as read:
            assert read('three')['rows']==encoded([(3,30)])
        with mysql_prepared_int(m,'zero','WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=? ORDER BY id',(4,)) as read:
            assert read('empty')['rows']==[]
        mysql_query(m,'rollback-setup','DELETE FROM cte_mysql_doc WHERE id=3')
        mysql_query(m,'rollback','BEGIN')
        mysql_query(m,'rollback','INSERT INTO cte_mysql_doc VALUES(3,30)')
        assert mysql_query(n,'rollback-observer',native_scan)['rows']==encoded(base)
        with mysql_prepared_int(m,'rollback-read','WITH a AS (SELECT id,v FROM cte_mysql_doc) SELECT id,v FROM a WHERE id=? ORDER BY id',(3,)) as read:
            assert read('pending')['rows']==encoded([(3,30)])
            mysql_query(m,'rollback','ROLLBACK')
            assert read('restored')['rows']==[]
        for client in (m,n): assert mysql_query(client,'rollback-final',native_scan)['rows']==encoded(base)
        summary['mysql_cases']=2
        differences={case:sides for case,sides in observations.items() if sides['reference']!=sides['target']}
        (evidence/'differences.json').write_text(json.dumps(differences,indent=2))
        summary['differences']=len(differences)
        assert not differences, differences
        assert summary['cases']==308,summary
        summary['passed']=True
    except BaseException as error:
        summary['failures'].append(str(error)); raise
    finally:
        if prepared_scope is not None: prepared_scope.__exit__(None,None,None)
        for index,wire in enumerate(connections):
            try:
                wire.sync(); wire.query('ROLLBACK')
                result=save('reference' if index<2 else 'target','cleanup','drop',f'DROP TABLE IF EXISTS {table}',wire.query(f'DROP TABLE IF EXISTS {table}'))
                assert not result['errors'] and result['ready']=='I',result
                summary['cleanup']['reference' if index<2 else 'target']=True
            finally: wire.close()
        summary['cleanup']['pg_connections']=len(connections)
        for client in mysqls:
            try:
                client.query('ROLLBACK'); client.query('DROP TABLE IF EXISTS cte_mysql_doc')
            finally: client.close()
        summary['cleanup']['mysql_connections']=len(mysqls)
        (evidence/'summary.json').write_text(json.dumps(summary,indent=2))
    print(json.dumps(summary),flush=True)

if __name__=='__main__':
    signal.signal(signal.SIGTERM,lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(Path(sys.argv[1]),int(sys.argv[2]),sys.argv[3],int(sys.argv[4]))
