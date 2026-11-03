# Copyright 2026 AsterSQL.
"""RETURNING-only frozen cases, real libpq and MySQL8 clients; no SQL mocks."""
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


def ordered(rows):
    return sorted(rows, key=lambda row: json.dumps(row))


def run(evidence, port, database, mysql_port):
    evidence.mkdir(parents=True, exist_ok=True)
    pq = Libpq()
    summary = dict(passed=False, cases=0, libpq=pq.PQlibVersion(), cleanup={}, failures=[])
    pq.lib.PQcmdStatus.restype = c.c_char_p
    pq.lib.PQcmdStatus.argtypes = [c.c_void_p]
    connections, mysqls = [], []
    prepared_scope = None
    observations = {}
    table = 'public.returning_doc'
    scan = f'SELECT id,v,note FROM {table} ORDER BY id'
    base = [(1, 10, 'before')]
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

    def close_statement(w):
        obs = w.phase(b'C', b'Ss\0', '3')
        assert not obs['errors'], obs
        assert w.sync()['ready'] == 'I'

    @contextmanager
    def mysql_prepared_int(client, case, sql):
        # Preserve the real statement across before/during/after PG observations.
        stmt = client.mysql_stmt_init(client.conn)
        assert stmt
        try:
            assert client.mysql_stmt_prepare(stmt, sql.encode(), len(sql.encode())) == 0
            assert client.mysql_stmt_param_count(stmt) == 1
            param = MysqlBind()
            value = c.c_int(1)
            length, null = c.c_ulong(4), c.c_bool(False)
            param.buffer_type = 3  # MYSQL_TYPE_LONG, signed int32
            param.buffer = c.addressof(value)
            param.buffer_length = 4
            param.length, param.is_null = c.addressof(length), c.addressof(null)
            assert not client.mysql_stmt_bind_param(stmt, c.byref(param))
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
                assert obs['code'] == 0 and obs['warning_count'] == 0 and not obs['warnings'], obs
                return obs
            yield execute_native
        finally:
            assert not client.mysql_stmt_close(stmt)

    def mysql_query(client, case, sql):
        obs = client.query(sql)
        client.lib.mysql_affected_rows.restype = c.c_ulonglong
        client.lib.mysql_affected_rows.argtypes = [c.c_void_p]
        obs['affected_rows'] = client.lib.mysql_affected_rows(client.conn)
        save('mysql', case, 'query', sql, obs)
        assert obs['code'] == 0 and obs['warning_count'] == 0, obs
        return obs

    success = [
        ('insert-default', [], 'INSERT INTO {t}(v,note) VALUES (7,NULL) RETURNING id AS generated_id,v,note', [(42,7,None)], [(42,7,None)], 'INSERT 0 1'),
        ('insert-multi-star', [], "INSERT INTO {t} VALUES(1,7,NULL),(2,8,'b') RETURNING *", [(1,7,None),(2,8,'b')], [(1,7,None),(2,8,'b')], 'INSERT 0 2'),
        ('insert-expression', [], 'INSERT INTO {t} VALUES(1,7,NULL) RETURNING id,v+100 AS projected,note', [(1,107,None)], [(1,7,None)], 'INSERT 0 1'),
        ('insert-zero', [], 'INSERT INTO {t}(id,v,note) SELECT 1,7,NULL WHERE false RETURNING id,v,note', [], [], 'INSERT 0 0'),
        ('delete-zero', base, 'DELETE FROM {t} WHERE false RETURNING id,v,note', [], base, 'DELETE 0'),
        ('delete-multi', [(1,10,None),(2,20,'b')], 'DELETE FROM {t} RETURNING id,v,note', [(1,10,None),(2,20,'b')], [], 'DELETE 2'),
        ('delete-expression', base, 'DELETE FROM {t} RETURNING id,v+100 AS projected,note', [(1,110,'before')], [], 'DELETE 1'),
        ('update-star', base, 'UPDATE {t} SET v=v+1 RETURNING *', [(1,11,'before')], [(1,11,'before')], 'UPDATE 1'),
        ('update-new', base, 'UPDATE {t} AS r SET v=v+1,note=NULL WHERE id=1 RETURNING r.id,r.v AS "After",r.note,r.v+100 AS projected', [(1,11,None,111)], [(1,11,None)], 'UPDATE 1'),
        ('delete-old', base, 'DELETE FROM {t} AS r WHERE id=1 RETURNING r.*', base, [], 'DELETE 1'),
        ('zero-match', base, 'UPDATE {t} SET v=99 WHERE id=9 RETURNING id,v,note', [], base, 'UPDATE 0'),
        ('multi-row', [(1,10,None),(2,20,'b')], 'UPDATE {t} SET v=v+1 RETURNING id,v,note', [(1,11,None),(2,21,'b')], [(1,11,None),(2,21,'b')], 'UPDATE 2'),
        ('update-same', base, 'UPDATE {t} SET v=v WHERE id=1 RETURNING id,v', [(1,10)], base, 'UPDATE 1'),
        ('gate-insert-open', base, "INSERT INTO {t} VALUES (2,20,'blocked') RETURNING id", [(2,)], base+[(2,20,'blocked')], 'INSERT 0 1'),
        ('gate-update-open', base, 'UPDATE {t} SET v=99 WHERE id=1 RETURNING v', [(99,)], [(1,99,'before')], 'UPDATE 1'),
        ('gate-delete-open', base, 'DELETE FROM {t} WHERE id=1 RETURNING id', [(1,)], [], 'DELETE 1'),
    ]
    try:
        # Native M/N use a separate database on the same Server, never PG syntax.
        m = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        mysqls.append(m)
        n = Mysql('127.0.0.1', mysql_port, 'root', '', database)
        mysqls.append(n)
        for client in mysqls:
            mysql_query(client, 'setup', 'SET autocommit=1')
        mysql_query(m, 'setup', 'CREATE TABLE returning_mysql_doc(id INT PRIMARY KEY,v INT,note TEXT)')
        mysql_query(m, 'setup', "INSERT INTO returning_mysql_doc VALUES(1,10,'before')")
        native_scan = 'SELECT id,v,note FROM returning_mysql_doc ORDER BY id'
        native_base = mysql_query(m, 'before', native_scan)
        mode_base = mysql_query(m, 'before', 'SELECT @@sql_mode,@@transaction_isolation,DATABASE()')
        mysql_query(m, 'before', 'BEGIN')
        mysql_query(m, 'before', 'UPDATE returning_mysql_doc SET v=v+1 WHERE id=1')
        assert mysql_query(m, 'before', native_scan)['rows'] == encoded([(1,11,'before')])
        assert mysql_query(n, 'before', native_scan)['rows'] == encoded(base)
        prepared_scope = mysql_prepared_int(m, 'query-coexist', 'SELECT v,note FROM returning_mysql_doc WHERE id = ?')
        prepared_read = prepared_scope.__enter__()
        prepared_base = prepared_read('before')
        assert prepared_base['rows'] == encoded([(11,'before')])
        for side, dsn in [('reference', os.environ['PG18_REFERENCE_DSN']), ('target', target)]:
            w = Wire(pq, dsn)
            connections.append(w)
            o = Wire(pq, dsn)
            connections.append(o)
            identity = query(w, side, 'identity', 'version', 'SELECT version()', 'I')
            if side == 'reference':
                version = query(w, side, 'identity', 'version-num', 'SHOW server_version_num', 'I')
                assert version['rows'] == encoded([(180006,)]), 'exact PostgreSQL18.6 required; 18.4 is separate'
                summary['reference_build'] = identity['rows']
            for conn in (w,o):
                query(conn, side, 'setup', 'utf8', "SET client_encoding = 'UTF8'", 'I')
                query(conn, side, 'setup', 'utc', "SET TIME ZONE 'UTC'" if side == 'reference' else "SET time_zone = '+00:00'", 'I')
                query(conn, side, 'setup', 'isolation', 'SHOW transaction_isolation' if side == 'reference' else 'SELECT @@transaction_isolation', 'I')
            for name, initial, template, returned, stored, tag in success:
                for extended, commit in itertools.product((False,True), repeat=2):
                    case = f'{name}-{"extended" if extended else "simple"}-{"commit" if commit else "rollback"}'
                    sql = template.format(t=table)
                    oids, values = [], []
                    if name == 'update-new' and extended:
                        sql = f'UPDATE {table} AS r SET v=v+$1,note=$2 WHERE id=1 RETURNING r.id,r.v AS "After",r.note,r.v+$3 AS projected'
                        oids, values = [23,25,23], [1,None,100]
                    try:
                        query(w, side, case, 'create', f'CREATE TABLE {table}(id integer PRIMARY KEY DEFAULT (40 + 2),v integer,note text)', 'I')
                        if initial:
                            literals = ','.join('('+','.join('NULL' if v is None else "'"+v+"'" if isinstance(v,str) else str(v) for v in row)+')' for row in initial)
                            query(w, side, case, 'seed', f'INSERT INTO {table} VALUES {literals}', 'I')
                        for conn in (w,o): readback(conn, side, case, 'initial', initial)
                        desc = None
                        if extended:
                            assert not parse(w, side, case, sql, oids)['errors']
                            desc = save(side, case, 'statement', sql, w.phase(b'D', b'Ss\0', 'T'))
                            assert not desc['errors'] and desc['parameters'] == oids, desc
                            assert w.sync()['ready'] == 'I'
                            for conn in (w,o): readback(conn, side, case, 'after-describe', initial)
                        query(w, side, case, 'begin', 'BEGIN', 'T')
                        result = execute(w, side, case, sql, extended, values)
                        assert not result['errors'] and result['ready'] == 'T', (case,result)
                        assert ordered(result['rows']) == ordered(encoded(returned)) and result['commands'] == [tag], (case,result)
                        if desc:
                            assert desc['metadata'] == result['metadata'], (case,desc,result)
                        readback(w, side, case, 'writer', stored)
                        readback(o, side, case, 'observer', initial)
                        query(w, side, case, 'end', 'COMMIT' if commit else 'ROLLBACK', 'I')
                        for conn in (w,o): readback(conn, side, case, 'final', stored if commit else initial)
                        # Numeric relation OIDs differ between independent catalogs.
                        # Preserve raw IDs, compare presence and exact attribute identity.
                        normalized = dict(rows=ordered(result['rows']), commands=result['commands'], metadata=[x[:5]+[bool(x[5]),x[6]] for x in result['metadata']])
                        observations.setdefault(case,{})[side] = normalized
                        if extended: close_statement(w)
                        summary['cases'] += 1
                    finally:
                        w.sync()
                        query(w, side, case, 'cleanup-rollback', 'ROLLBACK')
                        query(w, side, case, 'cleanup-drop', f'DROP TABLE IF EXISTS {table}', 'I')
            # Runtime atomicity and name resolution: exact phase, I/T/E and
            # prefix rollback are separate observations, never Parse-as-Execute.
            for verb, missing, extended, transaction in itertools.product(('INSERT','UPDATE','DELETE'), (False,True), (False,True), (False,True)):
                case = f'atomic-{verb}-{missing}-{extended}-{transaction}'
                initial = [(8,80,None),(9,90,'before')] if verb == 'INSERT' else [(1,10,None),(2,20,'before')]
                body = {'INSERT': f"INSERT INTO {table} VALUES (1,10,NULL),(2,20,'new')", 'UPDATE': f'UPDATE {table} SET v=v+1 WHERE id<3', 'DELETE': f'DELETE FROM {table} WHERE id<3'}[verb]
                sql = body + ' RETURNING ' + ('missing' if missing else '10/(id-2)') + ' AS boom'
                try:
                    query(w, side, case, 'create', f'CREATE TABLE {table}(id integer PRIMARY KEY,v integer,note text)', 'I')
                    for row in initial:
                        note = 'NULL' if row[2] is None else "'"+row[2]+"'"
                        query(w, side, case, 'seed', f'INSERT INTO {table} VALUES ({row[0]},{row[1]},{note})', 'I')
                    if transaction:
                        query(w, side, case, 'begin', 'BEGIN', 'T')
                        query(w, side, case, 'prefix', f"INSERT INTO {table} VALUES(3,30,'pending')", 'T')
                    if extended:
                        parsed = parse(w, side, case, sql, [])
                        if missing:
                            result = parsed
                            phase = 'parse'
                        else:
                            assert not parsed['errors'], (case, parsed)
                            result = execute(w, side, case, sql, True)
                            phase = 'execute'
                        synced = save(side, case, 'error-sync', sql, w.sync())
                        ready = synced['ready']
                    else:
                        result = execute(w, side, case, sql, False)
                        ready = result.get('tail', result)['ready']
                        phase = 'query'
                    state = '42703' if missing else '22012'
                    assert [e['C'] for e in result['errors']] == [state], (case,result)
                    assert not result['rows'] and not result['commands'], (case,result)
                    assert ready == ('E' if transaction else 'I'), (case,ready)
                    readback(o, side, case, 'observer-error', initial)
                    if transaction:
                        failed = query(w, side, case, 'failed-gate', 'SELECT 1 AS ok')
                        assert [e['C'] for e in failed['errors']] == ['25P02'] and failed['tail']['ready'] == 'E', failed
                        assert w.sync()['ready'] == 'E'
                    query(w, side, case, 'rollback', 'ROLLBACK', 'I')
                    for conn in (w,o): readback(conn, side, case, 'restored', initial)
                    query(w, side, case, 'recovery', 'SELECT 1 AS ok', 'I')
                    observations.setdefault(case,{})[side] = dict(state=state, phase=phase, ready=ready)
                    if extended and not missing: close_statement(w)
                    summary['cases'] += 1
                finally:
                    w.sync()
                    query(w, side, case, 'cleanup-rollback', 'ROLLBACK')
                    query(w, side, case, 'cleanup-drop', f'DROP TABLE IF EXISTS {table}', 'I')
            # PG18 OLD/NEW is legal on the reference but deliberately gated on
            # the product. Record both contracts without comparing unlike results.
            for extended in (False,True):
                case = f'old-new-{extended}'
                sql = f'UPDATE {table} SET v=99 WHERE id=1 RETURNING WITH (OLD AS o, NEW AS n) o.v,n.v'
                try:
                    query(w, side, case, 'create', f'CREATE TABLE {table}(id integer PRIMARY KEY,v integer,note text)', 'I')
                    query(w, side, case, 'seed', f"INSERT INTO {table} VALUES(1,10,'before')", 'I')
                    query(w, side, case, 'begin', 'BEGIN', 'T')
                    query(w, side, case, 'prefix', f"INSERT INTO {table} VALUES(3,30,'pending')", 'T')
                    parsed = parse(w, side, case, sql, []) if extended else None
                    result = parsed if parsed and parsed['errors'] else execute(w, side, case, sql, extended)
                    if side == 'reference':
                        assert not result['errors'] and result['rows'] == encoded([(10,99)]) and result['commands'] == ['UPDATE 1'], result
                        readback(w, side, case, 'writer', [(1,99,'before'),(3,30,'pending')])
                    else:
                        if [e['C'] for e in result['errors']] != ['0A000']:
                            summary['failures'].append(dict(case=case, expected='0A000', actual=result))
                        assert result['errors'] and not result['commands'] and not result['rows'], result
                        assert w.sync()['ready'] == 'E'
                        failed = query(w, side, case, 'failed-gate', 'SELECT 1 AS ok')
                        assert [e['C'] for e in failed['errors']] == ['25P02'], failed
                    readback(o, side, case, 'observer', base)
                    query(w, side, case, 'rollback', 'ROLLBACK', 'I')
                    for conn in (w,o): readback(conn, side, case, 'restored', base)
                    if extended and side == 'reference': close_statement(w)
                    summary['cases'] += 1
                finally:
                    w.sync()
                    query(w, side, case, 'cleanup-rollback', 'ROLLBACK')
                    query(w, side, case, 'cleanup-drop', f'DROP TABLE IF EXISTS {table}', 'I')
            # Each newly bound portal executes once; suspension and completion
            # never repeat non-idempotent writes. Both zero/nonzero result sets.
            for verb, zero, commit in itertools.product(('INSERT','UPDATE','DELETE'),(False,True),(False,True)):
                case = f'portal-{verb}-{zero}-{commit}'
                initial = [(1,10,None),(2,20,'b')]
                predicate = 'false' if zero else 'true'
                sql = {'INSERT': f'INSERT INTO {table} SELECT id+$1,v+1,$2 FROM {table} WHERE {predicate} RETURNING id,v,note', 'UPDATE': f'UPDATE {table} SET v=v+$1,note=$2 WHERE {predicate} RETURNING id,v,note', 'DELETE': f'DELETE FROM {table} WHERE id<=$1 AND {predicate} RETURNING id,v,note'}[verb]
                oids = [23] if verb == 'DELETE' else [23,25]
                try:
                    query(w, side, case, 'create', f'CREATE TABLE {table}(id integer PRIMARY KEY,v integer,note text)', 'I')
                    query(w, side, case, 'seed', f"INSERT INTO {table} VALUES(1,10,NULL),(2,20,'b')", 'I')
                    assert not parse(w, side, case, sql, oids)['errors']
                    desc = save(side, case, 'statement', sql, w.phase(b'D', b'Ss\0', 'T'))
                    assert desc['parameters'] == oids and not desc['errors'], desc
                    assert w.sync()['ready'] == 'I'
                    for conn in (w,o): readback(conn, side, case, 'description-no-write', initial)
                    query(w, side, case, 'begin', 'BEGIN', 'T')
                    stored, rounds = initial[:], []
                    for round in (0,1):
                        value = (10 if round == 0 else 100) if verb == 'INSERT' else round+1
                        note = None if round == 0 else 'next'
                        values = [value] if verb == 'DELETE' else [value,note]
                        bind(w, side, case, values)
                        portal = save(side, case, 'portal', sql, w.phase(b'D', b'Pp\0', 'T'))
                        assert portal['metadata'] == desc['metadata'], portal
                        assert w.sync()['ready'] == 'T'
                        readback(w, side, case, 'bind-no-write', stored)
                        readback(o, side, case, 'bind-observer', initial)
                        returned = [] if zero else ([(i+value,v+1,note) for i,v,n in stored] if verb == 'INSERT' else [(i,v+value,note) for i,v,n in stored] if verb == 'UPDATE' else [row for row in stored if row[0] <= value])
                        if not zero:
                            stored = sorted(stored+returned) if verb == 'INSERT' else returned if verb == 'UPDATE' else [row for row in stored if row[0] > value]
                        tag = ('INSERT 0 ' if verb == 'INSERT' else verb+' ') + str(len(returned))
                        accumulated = []
                        for page in range(len(returned)+1):
                            result = save(side, case, f'execute-{round}-{page}', sql, w.phase(b'E', b'p\0'+struct.pack('!I',1), 'Cs'))
                            assert not result['errors'], result
                            accumulated += result['rows']
                            assert w.sync()['ready'] == 'T'
                            readback(w, side, case, 'page-write-once', stored)
                            readback(o, side, case, 'page-observer', initial)
                            if result['commands']:
                                assert result['commands'] == [('INSERT 0 0' if verb == 'INSERT' else verb+' 0')], result
                                break
                        else:
                            raise AssertionError('portal never completed')
                        assert ordered(accumulated) == ordered(encoded(returned)), (case,accumulated,returned)
                        repeated = save(side, case, 'completed-repeat', sql, w.phase(b'E', b'p\0'+struct.pack('!I',0), 'C'))
                        assert not repeated['rows'] and not repeated['errors'], repeated
                        assert repeated['commands'] == [('INSERT 0 0' if verb == 'INSERT' else verb+' 0')], repeated
                        assert w.sync()['ready'] == 'T'
                        readback(w, side, case, 'repeat-no-write', stored)
                        assert not w.phase(b'C', b'Pp\0', '3')['errors']
                        rounds.append(dict(rows=ordered(accumulated), commands=result['commands'], repeated_commands=repeated['commands'], metadata=[x[:5]+[bool(x[5]),x[6]] for x in desc['metadata']]))
                    query(w, side, case, 'end', 'COMMIT' if commit else 'ROLLBACK', 'I')
                    for conn in (w,o): readback(conn, side, case, 'final', stored if commit else initial)
                    close_statement(w)
                    observations.setdefault(case,{})[side] = rounds
                    summary['cases'] += 1
                finally:
                    w.sync()
                    query(w, side, case, 'cleanup-rollback', 'ROLLBACK')
                    query(w, side, case, 'cleanup-drop', f'DROP TABLE IF EXISTS {table}', 'I')
            # Probe exact boundary, partial last page, unbounded and repeated
            # completion, independently of same-connection Query interleaving.
            for verb, limit, interleave in itertools.product(('INSERT','UPDATE','DELETE'), (0,1,2,3), (False,True)):
                case = f'fetch-count-{verb}-{limit}-{interleave}'
                initial = [(1,10,None),(2,20,'b')]
                sql = {'INSERT': f'INSERT INTO {table} SELECT id+10,v,note FROM {table} RETURNING *', 'UPDATE': f'UPDATE {table} SET v=v+1 RETURNING *', 'DELETE': f'DELETE FROM {table} RETURNING *'}[verb]
                returned = [(11,10,None),(12,20,'b')] if verb == 'INSERT' else [(1,11,None),(2,21,'b')] if verb == 'UPDATE' else initial
                stored = initial+returned if verb == 'INSERT' else returned if verb == 'UPDATE' else []
                try:
                    query(w, side, case, 'create', f'CREATE TABLE {table}(id integer PRIMARY KEY,v integer,note text)', 'I')
                    query(w, side, case, 'seed', f"INSERT INTO {table} VALUES(1,10,NULL),(2,20,'b')", 'I')
                    query(w, side, case, 'begin', 'BEGIN', 'T')
                    assert not parse(w, side, case, sql, [])['errors']
                    bind(w, side, case, [])
                    pages, all_rows = [], []
                    for page in range(4):
                        result = save(side, case, f'page-{page}', sql, w.phase(b'E', b'p\0'+struct.pack('!I',limit), 'Cs'))
                        assert not result['errors'], result
                        all_rows += result['rows']
                        prefix = 'INSERT 0 ' if verb == 'INSERT' else verb+' '
                        if result['commands']:
                            assert result['commands'] == [prefix+str(len(result['rows']))], result
                        else:
                            assert limit and len(result['rows']) == limit and result['tags'].endswith('s'), result
                        pages.append(dict(rows=result['rows'], commands=result['commands'], tags=result['tags']))
                        assert w.sync()['ready'] == 'T'
                        if interleave: readback(w, side, case, 'writer-page', stored)
                        readback(o, side, case, 'observer-page', initial)
                        if result['commands']: break
                    else: raise AssertionError('fetch never completed')
                    assert ordered(all_rows) == ordered(encoded(returned)), (case,all_rows)
                    repeated = save(side, case, 'repeat', sql, w.phase(b'E', b'p\0'+struct.pack('!I',0), 'C'))
                    assert not repeated['errors'] and not repeated['rows'] and repeated['commands'] == [prefix+'0'], repeated
                    assert w.sync()['ready'] == 'T'
                    readback(w, side, case, 'writer-final', stored)
                    query(w, side, case, 'rollback', 'ROLLBACK', 'I')
                    for conn in (w,o): readback(conn, side, case, 'restored', initial)
                    close_statement(w)
                    observations.setdefault(case,{})[side] = pages
                    summary['cases'] += 1
                finally:
                    w.sync()
                    query(w, side, case, 'cleanup-rollback', 'ROLLBACK')
                    query(w, side, case, 'cleanup-drop', f'DROP TABLE IF EXISTS {table}', 'I')
            # PG executes with a pending independent native transaction throughout.
            assert mysql_query(m, side, native_scan)['rows'] == encoded([(1,11,'before')])
            assert mysql_query(n, side, native_scan)['rows'] == encoded(base)
            assert mysql_query(m, side, 'SELECT @@sql_mode,@@transaction_isolation,DATABASE()') == mode_base
            assert prepared_read(side) == prepared_base
        prepared_scope.__exit__(None,None,None)
        differences = {case: sides for case,sides in observations.items() if sides['reference'] != sides['target']}
        (evidence/'differences.json').write_text(json.dumps(differences,indent=2))
        summary['differences'] = len(differences)
        mysql_query(m, 'after', 'ROLLBACK')
        assert mysql_query(m, 'after', native_scan) == native_base
        assert mysql_query(n, 'after', native_scan)['rows'] == encoded(base)
        # Independent prepared UPDATE, with an integer parameter and observer.
        with mysql_prepared_int(m, 'prepared-update', 'UPDATE returning_mysql_doc SET v=v+1 WHERE id = ?') as update_native:
            mysql_query(m, 'prepared-update', 'BEGIN')
            changed = update_native('execute')
            assert changed['affected_rows'] == 1, changed
            assert mysql_query(m, 'prepared-update', native_scan)['rows'] == encoded([(1,11,'before')])
            assert mysql_query(n, 'prepared-update', native_scan)['rows'] == encoded(base)
            mysql_query(m, 'prepared-update', 'ROLLBACK')
            assert mysql_query(m, 'prepared-restored', native_scan) == native_base
            assert mysql_query(n, 'prepared-restored', native_scan)['rows'] == encoded(base)
        assert summary['cases'] == 252, summary
        assert not differences and not summary['failures'], ('differential mismatches', differences, summary['failures'])
        summary['passed'] = True
    except BaseException as error:
        summary['failures'].append(str(error))
        raise
    finally:
        for wire in connections:
            try:
                wire.sync()
                wire.query('ROLLBACK')
                wire.query(f'DROP TABLE IF EXISTS {table}')
            finally:
                wire.close()
        summary['cleanup']['pg_connections'] = len(connections)
        if prepared_scope is not None:
            prepared_scope.__exit__(None,None,None)
        for client in mysqls:
            try:
                client.query('ROLLBACK')
                client.query('DROP TABLE IF EXISTS returning_mysql_doc')
            finally:
                client.close()
        summary['cleanup']['mysql_connections'] = len(mysqls)
        (evidence/'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(Path(sys.argv[1]), int(sys.argv[2]), sys.argv[3], int(sys.argv[4]))
