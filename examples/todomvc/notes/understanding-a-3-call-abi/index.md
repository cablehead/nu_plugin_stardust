---
release: alpha-2610230456
thread: async abi
part: 3
description: "delaney's proposal, as a delta from stardust.h today: every call that does work in a database becomes submit, poll and cancel"
---
# understanding a 3 call abi

in [delaney's proposal](../delaneys-execution-abi/), the rule is:

- **a request**: work an open database runs for you. queries, writes, plugin operations. `submit` sends it, `poll` collects its results, `cancel` stops it.
- **its own call**: everything else. opening and closing a database, licensing, and documents, codecs and expressions in your own process.

this note shows the change from `stardust.h` on alpha-2610230456.

names drop their `stardust_db_` prefix, except in code.

## how much changes {#numbers}

`stardust.h` has 77 calls today:

| calls | count | of 77 |
|:---|---|---|
| removed: the ones his replacement table names | 16 | 21% |
| not placed yet by his proposal | 23 | 30% |
| unchanged | 38 | 49% |
| added: `submit`, `poll`, `cancel` | 3 | |

if every call not placed yet that touches a database also moves, 37 calls go (48%). that leaves 43 calls: 40 that never wait on a database, and the 3 that do all of a database's work.

for scale, [ours](../thinking-about-async-abi/) removes 1 call and adds 6.

## the delta {#delta}

the calls his replacement table names, removed, and the three that replace them. arguments are cut short:

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

the events plugin's owners, subscription ids and receive waits go too. they're plugin operations, not C calls, so they leave no lines in the header.

## a live query, today and after {#live-query}

today. you open the query, subscribe, set a notify, and on each notify fetch the result:

```c
stardust_db_query_open(db, &source, &query);
stardust_db_query_subscribe(query, parameters, options, &sub);
stardust_db_subscription_set_notify(sub, notify, ctx);

/* after a notify, on a thread that may call stardust */
stardust_db_subscription_next(sub, 0, &event);          /* the result */
```

after. you submit a request, and stardust gives it an id. on a wake you poll, and the result is already in what comes back:

```c
stardust_submit(db, /* [{type querySubscribe path todos final true}] */ requests, &id);

/* after a wake, on the dispatcher */
stardust_poll(db, 0, &responses);
/* [{requestId 17 type queryPage result {sequence 1 transaction 42 lastPage true rows [...]}}] */
```

there's no query handle, no subscription handle, no callback per subscription, and no second call to fetch the result. the id tells the response apart from others, and is how you cancel it.

## events.receive, today and after {#receive}

today. you create an owner and a subscription with `plugin_call`, then call `receive`, which blocks a thread for up to `maxWait`:

```c
stardust_db_plugin_call(db, "events", "receive", /* {owner 1 subscription 10 maxWait ...} */ request, &result);
```

after. one request subscribes and keeps delivering. there's no owner, no subscription id and no receive call:

```c
stardust_submit(db, /* [{type eventsSubscribe pattern todos.changed final true}] */ requests, &id);

/* every poll may carry its messages */
/* [{requestId 18 type eventsMessage result {subject todos.changed sequence 7 data {id 1}}}] */
```

sending works the same way: `eventsPush` starts a stream, and later frames carry its id. today's `events.accept` and `events.read` aren't in his example schema, so how calls and exchanges look is still open.

## what it means {#means}

- **ids, not handles.** a running query, subscription or plugin call is an id stardust assigns, not a handle you release. you cancel it by id.
- **results arrive with the wake.** a response carries its result, so the queue needs limits, set at open, and each operation needs a rule for a consumer that falls behind. a slow consumer either loses updates or gets cancelled. it doesn't stall the others.
- **one reader per database.** one dispatcher polls, and routes every response by id. a program that polls the queue itself can't also use the binding's dispatcher.
- **errors are documents.** a failed request is an error response, not a C status. the C statuses are left for submit and poll failing.
- **operations are schemas.** a request's `type` comes from the schema of the open database: core plus its enabled plugins. a plugin adds request and response types, and its results go into the same queue.
- **SDKs are generated.** the bindings' methods are generated from the schemas, not written by hand around each C call.
- **nothing stays compatible.** every query, write and plugin call in every binding changes, and there's no compatibility layer.

## the api after {#landscape}

### async: one queue {#async}

everything that does work in a database, or waits on it, goes through `submit`, `poll` and `cancel`:

- running a query, a page at a time (`query`, answered by `queryPage`s)
- live queries (`querySubscribe`)
- writes: `merge_patch` and mutations
- every plugin operation: events, reactors (`reactorsStatus`), and the rest
- describing the open database (`schema`)

a request can be one call and one answer, a stream of answers, or a stream both ways. they all have one shape.

### sync: setup and local work {#sync}

his proposal keeps these 38 as plain calls. none of them waits on a database:

| what | calls |
|---|:---|
| licensing and version | `activate`, `license`, `abi_version` |
| the database's lifetime | `db_open`, `db_close`, `db_retain`, `db_release` |
| documents | `document_*`, `document_builder_*` |
| codecs | `parse`, `serialize`, `text_*`, `pointer_parse`, `pointer_format` |
| expressions and macros, outside a database | `expr_*`, `macro_expand`, `macro_build` |
| results of those | `result_take_document`, `result_error`, `result_release` |

so the sync api is setup, plus work on documents in your own process.

### not placed yet {#open}

his proposal doesn't place these 23:

- **plugin byte streams** (`plugin_stream_*`, 6 calls). they stay until they have "explicit bounded-chunk contracts", then move to the queue.
- **prepared handles**: `query_open`, `query_release`, `mutation_open`, `mutation_release`. his requests name a stored definition by `path`, with no handle, so the handles may go, and inline definitions may become part of a request.
- **reads**: `read_entity`, `definition_get`, `definition_list`, `plan`.
- **writes he doesn't name**: `definition_patch`, and `bulk_begin`, `bulk_add`, `bulk_finish`, `bulk_abort`.
- **waits he doesn't name**: `indexes_sync`, which blocks up to `max_wait_ticks`.
- **in a database, or not**: `db_expression_build`, `plugins`, `schema_validate`.

if the rule is "anything that touches the database goes through the queue", all of them move except `plugins` and `schema_validate`, which don't touch a database. that's a question for Delaney.
