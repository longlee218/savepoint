# shellcheck shell=sh
# Source this file to point savepoint-state integration tests at the isolated
# provider fleet started from docker/providers.compose.yml.
# Tests whose variable is unset are skipped (and say so), never silently passed.

export SP_TEST_PG_URL="postgres://sp:sp@127.0.0.1:55432/postgres"
export SP_TEST_MYSQL_URLS="mysql://root:sp@127.0.0.1:53306/mysql,mysql://root:sp@127.0.0.1:53307/mysql"
export SP_TEST_REDIS_URLS="redis://127.0.0.1:56379/0,redis://127.0.0.1:56380/0,redis://127.0.0.1:56381/0"
export SP_TEST_REDIS_STACK_URL="redis://127.0.0.1:56382/0"
export SP_TEST_MEMCACHED_ADDR="127.0.0.1:51211"
export SP_TEST_BULLMQ_REDIS_URL="redis://127.0.0.1:56379/5"
