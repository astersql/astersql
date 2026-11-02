# Copyright 2026 AsterSQL.
"""Real libpq/TiKV conflict races; the caller owns both services, never mocks SQL."""
import ctypes as c
import json
import itertools
import os
from pathlib import Path
import signal
import struct
import sys
import time
from pg_function_23_libpq import Libpq, Wire, Mysql


def run(evidence):
    evidence.mkdir(parents=True, exist_ok=True)
    pq = Libpq()
    summary = dict(passed=False, cases=0, libpq=pq.PQlibVersion(), cleanup={})
    connections = []
    native = None
    table = 'pg10_16_' + str(os.getpid()) + '_' + format(time.time_ns(), 'x')
    owned_database = False
    pq.lib.PQcmdStatus.restype = c.c_char_p
    pq.lib.PQcmdStatus.argtypes = [c.c_void_p]
    def save(side, case, sql, result):
        with (evidence / (side + '.jsonl')).open('a') as f:
            f.write(json.dumps(dict(case=case, sql=sql, observation=result)) + '\n')
        return result
    def query(conn, sql):
        result = pq.PQexec(conn, sql.encode())
        tag = (pq.lib.PQcmdStatus(result) or b'').decode()
        obs = pq.observe(result)
        obs['commands'] = [tag] if tag else []
        return obs
    def good(conn, sql):
        obs = query(conn, sql)
        assert obs['status'] in (1, 2), (sql, obs)
        return obs
    def rows(conn):
        return good(conn, f'SELECT id,email,v FROM public.{table} ORDER BY id')['rows']
    def expected(row):
        return [[str(v).encode().hex() if v is not None else None for v in row]]
    def bind(wire, portal, values):
        payload = portal.encode() + b'\0s\0' + struct.pack('!HH', 0, len(values))
        for value in values:
            data = None if value is None else str(value).encode()
            payload += struct.pack('!i', -1 if data is None else len(data))
            if data is not None:
                payload += data
        result = wire.phase(b'B', payload + struct.pack('!H', 0), '2')
        assert not result['errors'], result
        return result
    def wire_good(wire, sql):
        obs = wire.query(sql)
        assert not obs['errors'], (sql, obs)
        return obs
    try:
        native = Mysql('127.0.0.1', int(os.environ['PG10_MYSQL_PORT']), 'root', '')
        created = native.query(f'CREATE DATABASE {table}')
        assert created['code'] == 0, created
        owned_database = True
        target_dsn = os.environ['PG10_TARGET_DSN'] + ' dbname=' + table
        for side, dsn in [('reference', os.environ['PG18_REFERENCE_DSN']), ('target', target_dsn)]:
            admin = pq.PQconnectdb(dsn.encode())
            assert admin and pq.PQstatus(admin) == 0, 'libpq connection failed (DSN redacted)'
            connections.append((side, admin))
            identity = {sql: good(admin, sql) for sql in (['SELECT version()', 'SHOW server_version_num'] if side == 'reference' else ['SELECT version()'])}
            save(side, 'identity', '', identity)
            if side == 'reference':
                assert identity['SHOW server_version_num']['rows'] == expected([180006]), 'exact PG18.6 required'
            # Wire uses libpq for authentication, then supplements explicit protocol phase evidence.
            a, b = Wire(pq, dsn), Wire(pq, dsn)
            try:
                for wire in (a, b):
                    wire_good(wire, "SET SESSION TRANSACTION ISOLATION LEVEL READ COMMITTED" if side == 'target' else "SET SESSION CHARACTERISTICS AS TRANSACTION ISOLATION LEVEL READ COMMITTED")
                    save(side, 'isolation', '', wire_good(wire, 'SELECT @@transaction_isolation' if side == 'target' else 'SHOW transaction_isolation'))
                    wire_good(wire, "SET time_zone = '+00:00'" if side == 'target' else "SET TIME ZONE 'UTC'")
                for mode in ('simple', 'extended'):
                    for returning in (False, True):
                        for end, b_end in itertools.product(('COMMIT', 'ROLLBACK'), repeat=2):
                            for target, predicate in itertools.product(('(id)', '(email)'), ('', ' WHERE '+table+'.v < 15', ' WHERE 15 < '+table+'.v', ' WHERE NULL')):
                                case = f'{mode}-{returning}-{end}-{b_end}-{target}-{predicate}'
                                good(admin, f'CREATE TABLE public.{table} (id integer PRIMARY KEY,email text UNIQUE,v integer NOT NULL CHECK (v > 0))')
                                try:
                                    candidate_id = 1 if target.startswith("(id)") else 2
                                    email = 'b'
                                    email_sql = "'b'"
                                    sql_target = target
                                    sql = f"INSERT INTO public.{table} VALUES ({candidate_id},{email_sql},20) ON CONFLICT {sql_target} DO UPDATE SET v = {table}.v + excluded.v{predicate}"
                                    if returning:
                                        sql += ' RETURNING id,email,v'
                                    if mode == 'extended':
                                        prepared_sql = f'INSERT INTO public.{table} VALUES ($1,$2,$3) ON CONFLICT {sql_target} DO UPDATE SET v = {table}.v + excluded.v{predicate}' + (' RETURNING id,email,v' if returning else '')
                                        parsed = b.phase(b'P', b's\0' + prepared_sql.encode() + b'\0' + struct.pack('!HIII', 3, 23, 25, 23), '1')
                                        assert not parsed['errors'], parsed
                                        described = b.phase(b'D', b'Ss\0', 'Tn')
                                        save(side, case + '-statement', sql, described)
                                        assert not described['errors'] and described['parameters'] == [23,25,23], described
                                        assert b.sync()['ready'] == 'I'
                                    wire_good(a, 'BEGIN')
                                    wire_good(a, f"INSERT INTO public.{table} VALUES (1,{email_sql},10)")
                                    wire_good(a, f'UPDATE public.{table} SET v=30 WHERE id=1')
                                    wire_good(b, 'BEGIN')
                                    assert rows(admin) == [], 'uncommitted A became visible'
                                    if mode == 'extended':
                                        bound = bind(b, 'p', [candidate_id, email, 20])
                                        assert not bound['errors'], bound
                                        portal = b.phase(b'D', b'Pp\0', 'Tn')
                                        save(side, case + '-portal', sql, portal)
                                        assert portal['metadata'] == described['metadata'], (portal, described)
                                        b.send(b'E', b'p\0' + struct.pack('!I', 0))
                                        b.send(b'S')
                                    else:
                                        b.send(b'Q', sql.encode() + b'\0')
                                    # Observe the server's actual waiter before allowing A to finish.
                                    # No time-based guess can release A or make this assertion pass.
                                    deadline = time.monotonic() + 15
                                    barrier = None
                                    while time.monotonic() < deadline:
                                        if side == 'reference':
                                            barrier = good(admin, "SELECT pid FROM pg_stat_activity WHERE wait_event_type='Lock' AND query LIKE 'INSERT INTO public." + table + "%' ")
                                            waiting = bool(barrier['rows'])
                                        else:
                                            barrier = native.query('SELECT TRX_ID,CURRENT_HOLDING_TRX_ID FROM information_schema.data_lock_waits', warnings=False)
                                            assert barrier['code'] == 0, barrier
                                            waiting = bool(barrier['rows'])
                                        if waiting:
                                            break
                                    assert waiting, (case, 'B did not enter an observable lock wait', barrier)
                                    save(side, case + '-wait', '', barrier)
                                    assert rows(admin) == [], 'waiter exposed uncommitted rows'
                                    wire_good(a, end)
                                    result = b.read('Z')
                                    save(side, case, sql, result)
                                    assert not result['errors'] and result['ready'] == 'T', result
                                    inserted = end == 'ROLLBACK'
                                    changed = inserted or predicate in ('', ' WHERE 15 < '+table+'.v')
                                    result_row = [candidate_id, email, 20] if inserted else [1, email, 50 if changed else 30]
                                    assert result['commands'] == ['INSERT 0 ' + str(int(changed))], result
                                    assert result['rows'] == (expected(result_row) if changed and returning else []), result
                                    if returning:
                                        assert [m[:5] for m in (portal['metadata'] if mode == 'extended' else result['metadata'])] == [[23,4,-1,0,'id'],[25,-1,-1,0,'email'],[23,4,-1,0,'v']], result
                                    assert rows(admin) == ([] if inserted else expected([1,email,30]))
                                    readback = wire_good(b, 'SELECT id,email,v FROM public.' + table)
                                    save(side, case + '-writer-readback', '', readback)
                                    assert readback['rows'] == expected(result_row), readback
                                    wire_good(b, b_end)
                                    final_rows = expected(result_row) if b_end == 'COMMIT' else ([] if inserted else expected([1,email,30]))
                                    assert rows(admin) == final_rows
                                    wire_good(b, 'BEGIN')
                                    if mode == 'extended':
                                        bind(b, 'p2', [9, 'pending', 30])
                                        b.send(b'E', b'p2\0' + struct.pack('!I', 0))
                                        b.send(b'S')
                                        rebound = b.read('Z')
                                        save(side, case + '-rebind', prepared_sql, rebound)
                                        assert not rebound['errors'] and rebound['commands'] == ['INSERT 0 1'] and rebound['ready'] == 'T', rebound
                                        assert rebound['rows'] == (expected([9, 'pending', 30]) if returning else []), rebound
                                    else:
                                        wire_good(b, f"INSERT INTO public.{table} VALUES (9,'pending',30)")
                                    wire_good(b, 'ROLLBACK')
                                    assert rows(admin) == final_rows
                                    assert wire_good(b, 'SELECT 1')['rows'] == expected([1])
                                    summary['cases'] += 1
                                finally:
                                    wire_good(a, 'ROLLBACK')
                                    wire_good(b, 'ROLLBACK')
                                    if mode == 'extended':
                                        # Closing a statement also closes its surviving portals.
                                        close = b.phase(b'C', b'Ss\0', '3')
                                        assert not close['errors'], close
                                        assert b.sync()['ready'] == 'I'
                                    good(admin, 'DROP TABLE IF EXISTS public.' + table)
                summary['cleanup'][side] = True
            finally:
                a.close()
                b.close()
        assert summary['cases'] == 256, summary
        summary['passed'] = True
    finally:
        for side, conn in connections:
            try:
                good(conn, 'ROLLBACK')
                good(conn, 'DROP TABLE IF EXISTS public.' + table)
            finally:
                pq.PQfinish(conn)
        if native:
            try:
                if owned_database:
                    obs = native.query('DROP DATABASE ' + table)
                    summary['cleanup']['database'] = obs['code'] == 0
            finally:
                native.close()
        (evidence / 'summary.json').write_text(json.dumps(summary, indent=2))
    print(json.dumps(summary), flush=True)


if __name__ == '__main__':
    signal.signal(signal.SIGTERM, lambda *_: (_ for _ in ()).throw(KeyboardInterrupt()))
    run(Path(sys.argv[1]))
