select ft.ftrelid as table_id,
       srv.srvname as table_server,
       ft.ftoptions as table_options,
       pg_catalog.pg_get_userbyid(cls.relowner) as "owner"
from pg_catalog.pg_foreign_table ft
     left outer join pg_catalog.pg_foreign_server srv on ft.ftserver = srv.oid
     join pg_catalog.pg_class cls on ft.ftrelid = cls.oid
where cls.relnamespace = ?::oid
  --  and pg_catalog.age(ft.xmin) <= #TXAGE
  --  and cls.relname in ( :[*f_names] )
order by table_id
