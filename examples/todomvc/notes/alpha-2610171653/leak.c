// Stardust C ABI only. Creates an empty database, then runs the same inline
// query 400 times one of two ways, printing resident size after every 100:
//
//   MODE=new  (default) each query on a new pthread, joined before the next
//             starts
//   MODE=pool on 4 pthreads that live for the whole run, 25 queries each per
//             100
//
//   STARDUST_LICENSE=... MODE=new ./leak new.db
//
// What grows per new thread, as measured (alpha-2610171653 and
// alpha-2610212237):
//
//   - an inline query (STARDUST_SOURCE_DOCUMENT): every one tried, with one
//     clause or two, with or without orderBy, on an empty database or one
//     with matching entities
//   - a stored query (STARDUST_SOURCE_PATH) with orderBy, when its result
//     is empty or it joins two clauses
//
// What stays flat: a stored query with one clause and matching rows, with or
// without orderBy, and a stored query with two clauses and no orderBy on an
// empty database.
#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "stardust.h"

static stardust_db *db;
static stardust_document *query_doc;
static pthread_barrier_t step;

static stardust_bytes bytes(const char *s) { return (stardust_bytes){(const uint8_t *)s, (int64_t)strlen(s)}; }

static long rss_mb(void) {
    FILE *f = fopen("/proc/self/status", "r");
    char line[256];
    long kb = 0;
    while (fgets(line, sizeof line, f))
        if (!strncmp(line, "VmRSS:", 6)) kb = atol(line + 6);
    fclose(f);
    return kb / 1024;
}

static void report(const char *label) { printf("%-12s rss %5ld MB\n", label, rss_mb()); }

static void query(void) {
    stardust_db_source src = {STARDUST_SOURCE_DOCUMENT, 0, {0}, query_doc};
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
    o.create = 1;
    stardust_result *r = NULL;
    if (stardust_db_open(bytes(argv[1]), &o, &db, &r) != 0) { puts("open failed"); return 1; }
    if (r) stardust_result_release(r);
    const char *text = "{find [?e ?text] where [[?e text ?text]]}";
    if (stardust_parse(bytes(text), STARDUST_DUST, &query_doc) != 0) { puts("parse failed"); return 1; }
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
