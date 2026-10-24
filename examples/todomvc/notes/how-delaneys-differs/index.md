---
release: alpha-2610230456
thread: async abi
part: 3
description: "delaney's proposal, read through ours: the two diffs to stardust.h, and the api it leaves"
---
# how delaney's differs

[ours](../thinking-about-async-abi/) and [delaney's](../delaneys-execution-abi/) are both changes to `stardust.h`. ours adds a waitset beside the calls that wait today, and leaves every other call as it is. his replaces every call that runs work in the database with one queue, and it gets an async api for everything in exchange.

this note puts his next to ours as a diff, walks through the same two waits both ways, and then maps what the api looks like after his: what's async, what stays sync, and what his proposal doesn't say yet.

names drop their `stardust_db_` prefix, except in code.

## the two diffs {#diffs}

ours removes two lines and adds seven:

```diff
-typedef void (*stardust_db_subscription_notify)(void *ctx);
-int64_t stardust_db_subscription_set_notify(stardust_db_subscription *sub, stardust_db_subscription_notify notify, void *ctx);
+typedef struct stardust_db_waitset stardust_db_waitset;
+typedef void (*stardust_db_waitset_notify)(void *ctx);
+int64_t stardust_db_waitset_open(stardust_db *db, stardust_db_waitset **output);
+int64_t stardust_db_waitset_set_notify(stardust_db_waitset *ws, stardust_db_waitset_notify notify, void *ctx);
+int64_t stardust_db_waitset_add_subscription(stardust_db_waitset *ws, stardust_db_subscription *sub, int64_t token);
+int64_t stardust_db_waitset_add_plugin(stardust_db_waitset *ws, stardust_bytes plugin_name, stardust_bytes operation, const stardust_document *request, int64_t token);
+int64_t stardust_db_waitset_ready(stardust_db_waitset *ws, int64_t max_wait_ticks, int64_t *tokens, int64_t capacity, int64_t *count);
+void stardust_db_waitset_release(stardust_db_waitset *ws);
```

his, as a diff to the same header, removes the calls his replacement table names, and adds three calls and three open options. arguments are cut short here:

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

## a live query, both ways {#live-query}

ours. you subscribe as today, add the subscription to the waitset with a token you pick, and on a notify ask which tokens are ready. then you fetch the result yourself:

```c
stardust_db_query_subscribe(query, parameters, options, &sub);
stardust_db_waitset_add_subscription(ws, sub, 7);

/* after a notify, on a thread that may call stardust */
stardust_db_waitset_ready(ws, 0, tokens, 64, &count);    /* tokens: [7] */
stardust_db_subscription_next(sub, 0, &event);          /* the result */
```

his. you submit a request, and stardust gives it an id. on a wake you poll, and the result is already in what comes back:

```c
stardust_submit(db, /* [{type querySubscribe path todos final true}] */ requests, &id);

/* after a wake, on the dispatcher */
stardust_poll(db, 0, &responses);
/* [{requestId 17 type queryPage result {sequence 1 transaction 42 lastPage true rows [...]}}] */
```

two steps become one. there's no handle for the subscription, no token, and no second call to fetch the result. the id is how you tell it apart from other responses, and how you cancel it.

## events.receive, both ways {#receive}

ours. you create an owner and a subscription as today, with `plugin_call`. then you add the receive to the waitset, and call it without `maxWait` when its token is ready:

```c
stardust_db_waitset_add_plugin(ws, "events", "receive", /* {owner 1 subscription 10} */ request, 8);

/* after a notify, when 8 is ready */
stardust_db_plugin_call(db, "events", "receive", request, &result);
```

his. one request subscribes and keeps delivering. there's no owner, no subscription id and no receive call:

```c
stardust_submit(db, /* [{type eventsSubscribe pattern todos.changed final true}] */ requests, &id);

/* every poll may carry its messages */
/* [{requestId 18 type eventsMessage result {subject todos.changed sequence 7 data {id 1}}}] */
```

sending works the same way: `eventsPush` starts a stream, and later frames carry its id. today's `events.accept` and `events.read` aren't in his example schema, so how calls and exchanges look is still open.

## what it means {#means}

- **ids instead of tokens.** stardust picks the id, so it's unique, and you can't use your own pointer or index as you would an epoll token. a binding keeps a map from id to its waiter.
- **results arrive with the wake.** ours tells you what's ready, and leaves the result in stardust until you fetch it. his moves the result into the queue. so the queue needs limits, set at open, and each operation needs a rule for a consumer that falls behind. a slow consumer either loses updates or gets cancelled. it doesn't stall the others.
- **one reader per database.** one dispatcher polls, and routes every response by id. a program that polls the queue itself can't also use the binding's dispatcher.
- **errors are documents.** a failed request is an error response, not a C status. the C statuses are left for submit and poll failing.
- **operations are schemas.** a request's `type` comes from the schema of the open database: core plus its enabled plugins. a plugin adds request and response types, and its results go into the same queue. that answers the plugin question ours left open, how a plugin tells the waitset a source is ready.
- **SDKs are generated.** the bindings' methods are generated from the schemas, not written by hand around each C call.
- **it breaks much more.** ours breaks `subscription_set_notify`. his breaks every query, write and plugin call, and keeps no compatibility layer.

## the api after his {#landscape}

### async: one queue {#async}

everything that does work in the database, or waits on it, goes through `submit`, `poll` and `cancel`:

- running a query, a page at a time (`query`, answered by `queryPage`s)
- live queries (`querySubscribe`)
- writes: `merge_patch` and mutations
- every plugin operation: events, reactors (`reactorsStatus`), and the rest
- describing the open database (`schema`)

a request can be one call and one answer, a stream of answers, or a stream both ways. they all have one shape.

### sync: setup and local work {#sync}

his proposal keeps these as plain calls. none of them waits on the database:

| what | calls |
|---|:---|
| licensing | `activate`, `license` |
| the database's lifetime | `db_open`, `db_close`, `db_retain`, `db_release` |
| documents | `document_*`, `document_builder_*` |
| codecs | `parse`, `serialize`, `text_*`, `pointer_parse`, `pointer_format` |
| expressions and macros, outside a database | `expr_build`, `expr_evaluate`, `macro_expand`, `macro_build` |

so the sync api is setup, plus work on documents in your own process.

### to be decided {#open}

his proposal doesn't place these:

- **plugin byte streams** (`plugin_stream_*`). they stay until they have "explicit bounded-chunk contracts", then move to the queue.
- **prepared handles**: `query_open` and `mutation_open`. his requests name a stored definition by `path`, with no handle, so the handles may go, and inline definitions may become part of a request.
- **reads**: `read_entity`, `definition_get`, `definition_list`, `plan`.
- **writes he doesn't name**: `definition_patch`, and `bulk_begin`, `bulk_add`, `bulk_finish`, `bulk_abort`.
- **waits he doesn't name**: `indexes_sync`, which blocks up to `max_wait_ticks`, the way ours wanted the `events` calls not to.
- **in a database, or not**: `db_expression_build`, `plugins`, `schema_validate`.

if the rule is "anything that touches the database goes through the queue", all of them move except `plugins` and `schema_validate`, which don't touch a database. that's a question for Delaney.
