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

if every call not placed yet that touches a database also moves, 37 calls go (48%). that leaves 43 calls: 40 that never wait on a database, and the 3 that do all of a database's work.

[the waitset ask](../thinking-about-async-abi/) removes 1 call and adds 6.

## removed and added {#delta}

the 16 removed calls and the 3 added calls, with arguments cut short:

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

## events.receive, today and after {#receive}

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

sending works the same way: `eventsPush` starts a stream, and later frames carry the stream's id. today's `events.accept` and `events.read` aren't in the example schema, so how calls and exchanges look is still open.

## other changes {#means}

- **ids, not handles.** stardust assigns each request an id. there's no handle to release.
- **responses hold the results.** the queue has limits, set at open. each operation has a rule for a consumer that falls behind: it loses updates or is cancelled. other requests aren't held up.
- **one reader per database.** one dispatcher calls `poll` and routes each response by id. a program that calls `poll` itself can't also use a binding's dispatcher.
- **errors are documents.** a failed request returns an error response. C statuses only report `submit` or `poll` failing.
- **operations are schemas.** each open database has a schema of request and response types: core plus the enabled plugins. a plugin adds types, not C calls.
- **SDKs are generated** from the schemas.
- **no compatibility layer.** every binding's queries, writes and plugin calls change.

## the api after {#landscape}

### async: one queue {#async}

these go through `submit`, `poll` and `cancel`:

- running a query, a page at a time (`query`, answered by `queryPage`s)
- live queries (`querySubscribe`)
- writes: `merge_patch` and mutations
- every plugin operation: events, reactors (`reactorsStatus`), and the rest
- describing the open database (`schema`)

a request can return one response or many, and can take input as a stream.

### sync: setup and local work {#sync}

these 38 don't change. none of them waits on a database:

| what | calls |
|---|:---|
| licensing and version | `activate`, `license`, `abi_version` |
| the database's lifetime | `db_open`, `db_close`, `db_retain`, `db_release` |
| documents | `document_*`, `document_builder_*` |
| codecs | `parse`, `serialize`, `text_*`, `pointer_parse`, `pointer_format` |
| expressions and macros, outside a database | `expr_*`, `macro_expand`, `macro_build` |
| results of those | `result_take_document`, `result_error`, `result_release` |

### not placed yet {#open}

the proposal doesn't place these 23:

- **plugin byte streams** (`plugin_stream_*`, 6 calls). they stay until they have "explicit bounded-chunk contracts", then move to the queue.
- **prepared handles**: `query_open`, `query_release`, `mutation_open`, `mutation_release`. the example requests name a stored definition by `path`, with no handle, so the handles may go, and inline definitions may become part of a request.
- **reads**: `read_entity`, `definition_get`, `definition_list`, `plan`.
- **other writes**: `definition_patch`, and `bulk_begin`, `bulk_add`, `bulk_finish`, `bulk_abort`.
- **other waits**: `indexes_sync`, which blocks up to `max_wait_ticks`.
- **in a database, or not**: `db_expression_build`, `plugins`, `schema_validate`.

if everything that touches a database becomes a request, all of these move except `plugins` and `schema_validate`. this is a question for Delaney.
