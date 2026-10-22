// Stardust C ABI only. Runs 400 queries one of two ways, printing resident
// size after every 100:
//
//   MODE=new  (default) each query on a new pthread, joined before the next
//             starts
//   MODE=pool on 4 pthreads that live for the whole run, 25 queries each per
//             100
//
//   STARDUST_LICENSE=... MODE=new ./leak <db with a stored query at todos/list>
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "stardust.h"

static stardust_db *db;
static pthread_barrier_t step;

static stardust_bytes bytes(const char *s) { return (stardust_bytes){(const uint8_t *)s, (int64_t)strlen(s)}; }

static long status_mb(const char *key) {
    FILE *f = fopen("/proc/self/status", "r");
    char line[256];
    long kb = 0;
    size_t n = strlen(key);
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, key, n)) kb = atol(line + n);
    fclose(f);
    return kb / 1024;
}

static void report(const char *label) {
    printf("%-12s rss %5ld MB\n", label, status_mb("VmRSS:"));
}

static void query(void) {
    stardust_db_source src = {STARDUST_SOURCE_PATH, 0, bytes("todos/list"), NULL};
    stardust_db_query_options opts = {sizeof opts, 1, 0, 0, 0, 0};
    stardust_db_query *q = NULL;
    stardust_db_rows *rows = NULL;
    stardust_result *page = NULL;
    if (stardust_db_query_open(db, &src, &q) != 0) { puts("query_open failed"); exit(1); }
    if (stardust_db_query_run(q, NULL, &opts, NULL, &rows) != 0) { puts("run failed"); exit(1); }
    stardust_db_rows_next(rows, &page);
    if (page) stardust_result_release(page);
    stardust_db_rows_release(rows);
    stardust_db_query_release(q);
}

static void *one(void *arg) { query(); return NULL; }

// A pool worker: 25 queries, then wait for the report, four times.
static void *worker(void *arg) {
    for (int b = 0; b < 4; b++) {
        for (int i = 0; i < 25; i++) query();
        pthread_barrier_wait(&step);
        pthread_barrier_wait(&step);
    }
    return NULL;
}

int main(int argc, char **argv) {
    int pool = getenv("MODE") && !strcmp(getenv("MODE"), "pool");
    if (stardust_activate(bytes(getenv("STARDUST_LICENSE"))) != 0) { puts("activate failed"); return 1; }
    stardust_db_options o;
    memset(&o, 0, sizeof o);
    o.size = sizeof o;
    o.version = 1;
    stardust_result *r = NULL;
    if (stardust_db_open(bytes(argv[1]), &o, &db, &r) != 0) { puts("open failed"); return 1; }
    query();
    report("start");
    char label[32];
    if (pool) {
        pthread_t ts[4];
        pthread_barrier_init(&step, NULL, 5);
        for (int i = 0; i < 4; i++) pthread_create(&ts[i], NULL, worker, NULL);
        for (int b = 1; b <= 4; b++) {
            pthread_barrier_wait(&step);  // the workers have run this 100
            snprintf(label, sizeof label, "%d queries", b * 100);
            report(label);
            pthread_barrier_wait(&step);
        }
        for (int i = 0; i < 4; i++) pthread_join(ts[i], NULL);
    } else {
        for (int b = 1; b <= 4; b++) {
            for (int i = 0; i < 100; i++) {
                pthread_t t;
                pthread_create(&t, NULL, one, NULL);
                pthread_join(t, NULL);
            }
            snprintf(label, sizeof label, "%d queries", b * 100);
            report(label);
        }
    }
    return 0;
}
