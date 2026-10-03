select usesuper
from pg_user
where usename = current_user
