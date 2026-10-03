select cls.xmin as sequence_state_number,
       sq.seqrelid as sequence_id,
       cls.relname as sequence_name,
       pg_catalog.format_type(sq.seqtypid, null) as data_type,
       sq.seqstart as start_value,
       sq.seqincrement as inc_value,
       sq.seqmin as min_value,
       sq.seqmax as max_value,
       sq.seqcache as cache_size,
       sq.seqcycle as cycle_option,
       pg_catalog.pg_get_userbyid(cls.relowner) as "owner"
from pg_catalog.pg_sequence sq
    join pg_class cls on sq.seqrelid = cls.oid
    where cls.relnamespace = ?::oid
--  and pg_catalog.age(cls.xmin) <= #TXAGE
--  and cls.relname in ( :[*f_names] )
