select L.transactionid::varchar::bigint as transaction_id
from pg_catalog.pg_locks L
where L.transactionid is not null
order by pg_catalog.age(L.transactionid) desc
limit 1
