// Stardust C ABI only, one thread. Creates a database with schema `item` and
// 4000 items matching it. Then, twice: subscribes to a query over them, makes
// 400 commits that each toggle one item, reading the subscription up to each
// commit, and releases the subscription. Prints resident size after every
// 100 commits and after each release.
//
//   STARDUST_LICENSE=... ./subcommit new.db
//   SCHEMA=0 drops [schema item ?e] from the query.
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include "stardust.h"

static stardust_db *db;
static int64_t first;  // the item the commits toggle

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

static stardust_document *parse(const char *s) {
    stardust_document *d = NULL;
    if (stardust_parse(bytes(s), STARDUST_DUST, &d) != 0) { puts("parse failed"); exit(1); }
    return d;
}

static int64_t commit(const char *patch) {
    stardust_document *d = parse(patch);
    stardust_result *res = NULL;
    stardust_db_commit_info info = {0};
    if (stardust_db_merge_patch(db, d, NULL, NULL, &res) != 0 || stardust_db_result_commit_info(res, &info) != 0 ||
        info.status != 0) { puts("commit failed"); exit(1); }
    if (!first) {
        // The result's ids holds #_first's entity id.
        stardust_document *out = NULL;
        uint32_t root, ids, id;
        if (stardust_result_take_document(res, &out) != 0 || stardust_document_root(out, &root) != 0 ||
            stardust_document_member(out, root, bytes("ids"), &ids) != 0 ||
            stardust_document_member(out, ids, bytes("#_first"), &id) != 0 || stardust_document_i64(out, id, &first) != 0) {
            puts("no id for #_first");
            exit(1);
        }
        stardust_document_release(out);
    } else {
        stardust_result_release(res);
    }
    stardust_document_release(d);
    return info.transaction;
}

int main(int argc, char **argv) {
    int schema = !getenv("SCHEMA") || atoi(getenv("SCHEMA"));
    if (stardust_activate(bytes(getenv("STARDUST_LICENSE"))) != 0) { puts("activate failed"); return 1; }
    stardust_db_options o;
    memset(&o, 0, sizeof o);
    o.size = sizeof o;
    o.version = 1;
    o.create = 1;
    stardust_result *r = NULL;
    if (stardust_db_open(bytes(argv[1]), &o, &db, &r) != 0) { puts("open failed"); return 1; }
    if (r) stardust_result_release(r);

    stardust_document *def = parse("{type object properties {text {type string} done {type boolean}} "
                                   "required [text done] additionalProperties false}");
    r = NULL;
    if (stardust_db_definition_patch(db, bytes("schemas"), bytes("item"), def, &r) != 0) { puts("define failed"); return 1; }
    stardust_result_release(r);
    stardust_document_release(def);

    // The item to toggle, then 4000 more, 500 per commit.
    commit("{#_first {text \"first\" done false}}");
    static char big[1 << 16];
    for (int done = 0; done < 4000;) {
        int n = snprintf(big, sizeof big, "{");
        for (int k = 0; k < 500; k++, done++)
            n += snprintf(big + n, sizeof big - n, "#_s%d {text \"seed %d\" done false} ", k, done);
        snprintf(big + n, sizeof big - n, "}");
        commit(big);
    }

    stardust_document *qdoc = parse(schema ? "{find [?e ?text ?done] where [[schema item ?e] [?e text ?text] [?e done ?done]] orderBy [?e]}"
                                           : "{find [?e ?text ?done] where [[?e text ?text] [?e done ?done]] orderBy [?e]}");
    for (int round = 1; round <= 2; round++) {
        printf("round %d\n", round);
        stardust_db_source src = {STARDUST_SOURCE_DOCUMENT, 0, {0}, qdoc};
        stardust_db_query *q = NULL;
        stardust_db_subscription *sub = NULL;
        stardust_db_query_options qo = {sizeof qo, 1, 0, 0, 0, 0};
        if (stardust_db_query_open(db, &src, &q) != 0 || stardust_db_query_subscribe(q, NULL, &qo, &sub) != 0) {
            puts("subscribe failed");
            return 1;
        }
        printf("start        rss %5ld MB\n", rss_mb());

        char patch[64];
        for (int i = 1; i <= 400; i++) {
            snprintf(patch, sizeof patch, "{\"%lld\" {done %s}}", (long long)first, i % 2 ? "true" : "false");
            int64_t tx = commit(patch);
            stardust_db_live_event ev;
            do {
                memset(&ev, 0, sizeof ev);
                if (stardust_db_subscription_next(sub, 10000000, &ev) != 0) { puts("next failed"); return 1; }
                if (ev.result) stardust_document_release(ev.result);
            } while (ev.transaction < tx);
            if (i % 100 == 0) printf("%d commits  rss %5ld MB\n", i, rss_mb());
        }

        stardust_db_subscription_release(sub);
        stardust_db_query_release(q);
        printf("released     rss %5ld MB\n", rss_mb());
    }
    return 0;
}
