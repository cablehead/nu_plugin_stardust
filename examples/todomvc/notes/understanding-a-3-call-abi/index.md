---
release: alpha-2610230456
thread: async abi
part: 3
description: "delaney's proposal replaces the calls for queries, writes and plugin operations with submit, poll and cancel"
---
# understanding a 3 call abi

[delaney's proposal](../delaneys-execution-abi/) replaces the calls for queries, writes and plugin operations with three calls: `submit` sends a request document, `poll` returns response documents, and `cancel` stops a request. the calls for opening and closing a database, licensing, documents, codecs and expressions don't change.

this note compares the proposal with `stardust.h` on [alpha-2610230456](../alpha-2610230456/). names drop the `stardust_db_` prefix, except in code.

## how much changes {#numbers}

`stardust.h` has 77 calls today:

| calls | count | of 77 |
|:---|---|---|
| removed: named in the proposal | 16 | 21% |
| not placed yet | 23 | 30% |
| unchanged | 38 | 49% |
| added: `submit`, `poll`, `cancel` | 3 | |

## removed and added {#delta}

```diff
 /* queries: run, page through, subscribe */
-int64_t stardust_db_query_run(stardust_db_query *query_handle, ...);
-int64_t stardust_db_rows_next(stardust_db_rows *rows, stardust_result **page);
-void stardust_db_rows_release(stardust_db_rows *rows);
-int64_t stardust_db_query_subscribe(stardust_db_query *query_handle, ...);
-typedef void (*stardust_db_subscription_notify)(void *ctx);
-int64_t stardust_db_subscription_set_notify(stardust_db_subscription *sub, ...);
-int64_t stardust_db_subscription_next(stardust_db_subscription *sub, ...);
-void stardust_db_subscription_cancel(stardust_db_subscription *sub);
-void stardust_db_subscription_release(stardust_db_subscription *sub);
 /* writes and plugins */
-int64_t stardust_db_merge_patch(stardust_db *db, ...);
-int64_t stardust_db_mutation_run(stardust_db_mutation *mutation, ...);
-int64_t stardust_db_plugin_call(stardust_db *db, ...);
 /* result handles and diagnostics on these paths */
-int64_t stardust_db_result_commit_info(const stardust_result *result, ...);
-int64_t stardust_db_query_error(const stardust_db_query *handle, ...);
-int64_t stardust_db_mutation_error(const stardust_db_mutation *handle, ...);
-int64_t stardust_db_rows_error(const stardust_db_rows *rows, ...);
-int64_t stardust_db_subscription_error(const stardust_db_subscription *sub, ...);
 /* one queue per database */
+typedef void (*stardust_wake)(void *ctx);
+int64_t stardust_submit(stardust_db *db, const stardust_document *requests, int64_t *firstRequestId);
+int64_t stardust_poll(stardust_db *db, int64_t max_wait_ticks, stardust_document **output);
+int64_t stardust_cancel(stardust_db *db, int64_t requestId);
 /* stardust_db_options gains responses, wake and wake_ctx */
```

## subscribing to a live query {#live-query}

today, a live query needs a query handle, a subscription and a notify callback, and a call to fetch each result:

```c
stardust_db_query_open(db, &source, &query);
stardust_db_query_subscribe(query, parameters, options, &sub);
stardust_db_subscription_set_notify(sub, notify, ctx);

/* after a notify, on a thread that may call stardust */
stardust_db_subscription_next(sub, 0, &event);          /* the result */
```

in the proposal, a live query is a request. `submit` returns an id, and `poll` returns the results with that id:

```c
stardust_submit(db, /* [{type querySubscribe path todos final true}] */ requests, &id);

/* after a wake, on the dispatcher */
stardust_poll(db, 0, &responses);
/* [{requestId 17 type queryPage result {sequence 1 transaction 42 lastPage true rows [...]}}] */
```

there's no query handle, subscription handle or callback per subscription, and no call to fetch the result. `cancel` takes the id.

## receiving events {#receive}

today, `receive` needs an owner and a subscription, each made with `plugin_call`, and blocks a thread for up to `maxWait`:

```c
stardust_db_plugin_call(db, "events", "receive", /* {owner 1 subscription 10 maxWait ...} */ request, &result);
```

in the proposal, one `eventsSubscribe` request returns messages until it's cancelled. there's no owner, subscription id or receive call:

```c
stardust_submit(db, /* [{type eventsSubscribe pattern todos.changed final true}] */ requests, &id);

/* every poll may carry messages for 18 */
/* [{requestId 18 type eventsMessage result {subject todos.changed sequence 7 data {id 1}}}] */
```

## what this means {#means}

- **calls that run on an open database become async. the rest stay sync.** queries, live queries, writes and plugin operations become requests through `submit`, `poll` and `cancel`. the other 38 calls don't change.
- **results are pushed, not pulled.** today a result stays in stardust until a call asks for it. in the proposal it goes on a queue with limits, set at open. a consumer that falls behind loses updates, or the request is cancelled. only one dispatcher can read the queue.
