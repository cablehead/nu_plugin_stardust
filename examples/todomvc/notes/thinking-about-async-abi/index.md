---
release: alpha-2610212237
description: "which stardust calls wait until something happens, and what async support each has today. applies to alpha-2610212237."
---
# thinking about an async abi

applies to [alpha-2610212237](../alpha-2610212237/).

the C ABI's calls, split by how you use them:

- [waits](#waits): you wait, possibly forever, until something happens. these are the ones an async client has to handle
- [sync](#sync): does a bounded piece of work and returns. listed to check, then ignore

## waits {#waits}

### what you wait for {#wait-list}

ready many times:

| call | waits for | async today |
|---|:---|:---|
| `stardust_db_subscription_next` | a live query's next result | yes: `set_notify` |
| `events.receive` | a published event | none: `maxWait` |
| `events.accept` | an incoming call, as a worker | none: `maxWait` |
| `events.read` | an exchange's next message, finish or cancel | none: `maxWait` |

ready once:

| call | waits for | async today |
|---|:---|:---|
| `stardust_db_indexes_sync` | background indexes built | none: `max_wait_ticks` |
| `backups.sync` | every backup target confirmed | none: `maxWait` |
| `sdk.sync` | the generated SDK published | none: `maxWait` |
| `skills.sync` | the generated skills published | none: `maxWait` |
| `stardust_db_open` | another process's file lock | none: `lock_timeout_ticks`, negative waits forever |

the events ops run through `stardust_db_plugin_call(db, "events", op, request, &result)`. a wait of 0, or none, checks once and reports `TIMEOUT`.

### the async mechanism today {#notify}

only query subscriptions have one:

```c
typedef void (*stardust_db_subscription_notify)(void *ctx);
uint32_t stardust_db_subscription_set_notify(stardust_db_subscription *sub, stardust_db_subscription_notify notify, void *ctx);
uint32_t stardust_db_subscription_next(stardust_db_subscription *sub, int64_t max_wait_ticks, stardust_db_live_event *event);
```

- `set_notify` once, before any `next`
- `next(0)`: an event, or `TIMEOUT`, which arms one wake-up
- `notify` may run on any thread, and must not call stardust
- after `stardust_db_subscription_release` returns, no callback runs

### what interrupts a wait {#interrupts}

| wait | interrupted by | returns |
|---|:---|:---|
| `stardust_db_subscription_next` | `stardust_db_subscription_cancel`, `_release` | `CANCELLED` |
| `events.receive` | `events.unsubscribe`, `events.close_owner` | `CANCELLED` |
| `events.accept` | `events.unregister`, `events.close_owner` | `CANCELLED` |
| `events.read` | `events.close_owner` | `CANCELLED` |
| all of them | `stardust_db_close` | `CLOSED` |

## sync {#sync}

each of these does its own work and returns. some take a while (a big query, a write waiting to be durable), but none waits on something that might not happen.

### setup {#setup}

`stardust_abi_version`, `stardust_activate`, `stardust_license`, `stardust_plugins`, `stardust_db_retain`, `stardust_db_release`

### documents and results {#documents}

`stardust_parse`, `stardust_serialize`, `stardust_text_data`, `stardust_text_error`, `stardust_text_release`, `stardust_pointer_parse`, `stardust_pointer_format`, `stardust_document_root`, `stardust_document_count`, `stardust_document_read_many`, `stardust_document_member`, `stardust_document_pointer`, `stardust_document_pointer_text`, `stardust_document_i64`, `stardust_document_f64`, `stardust_document_error`, `stardust_document_release`, `stardust_document_builder_create`, `stardust_document_builder_add_many`, `stardust_document_builder_seal`, `stardust_document_builder_error`, `stardust_document_builder_release`, `stardust_result_take_document`, `stardust_result_error`, `stardust_result_release`, `stardust_db_result_commit_info`

### reads {#reads}

`stardust_db_read_entity`, `stardust_db_query_open`, `stardust_db_query_run`, `stardust_db_rows_next`, `stardust_db_rows_error`, `stardust_db_rows_release`, `stardust_db_query_error`, `stardust_db_query_release`, `stardust_db_definition_get`, `stardust_db_definition_list`, `stardust_db_plan` (status, history, explain), `objects.get`, `objects.lookup`, `objects.children`, `objects.path`, `objects.files`, `objects.manifest`, `blackholes.preview`, `blackholes.audit`, every `*.status`

### writes {#writes}

return once durable.

`stardust_db_merge_patch`, `stardust_db_mutation_open`, `stardust_db_mutation_run`, `stardust_db_mutation_error`, `stardust_db_mutation_release`, `stardust_db_bulk_begin`, `stardust_db_bulk_add`, `stardust_db_bulk_finish`, `stardust_db_bulk_abort`, `stardust_db_definition_patch`, `stardust_db_plan` (pin, unpin, replan, revert), `objects.create`, `objects.update`, `objects.move`, `objects.remove`, `blackholes.execute`

### expressions {#expressions}

`stardust_expr_build`, `stardust_db_expression_build`, `stardust_macro_expand`, `stardust_macro_build`, `stardust_expr_evaluate`, `stardust_expr_program_error`, `stardust_expr_program_release`

### plugin streams {#streams}

`objects.upload` and `objects.open`, through `stardust_db_plugin_stream_open`, `_write`, `_read`, `_seek`, `_finish`, `_release`

### events actions {#events}

none of these waits: a full queue drops the delivery (`publish`) or cancels the exchange (`send`).

`events.open_owner`, `events.close_owner`, `events.publish`, `events.subscribe`, `events.unsubscribe`, `events.register`, `events.unregister`, `events.call`, `events.send`, `events.finish`, `events.cancel`

### live query setup {#subscriptions}

`stardust_db_query_subscribe`, `stardust_db_subscription_cancel`, `stardust_db_subscription_error`

### to check {#check}

sync, but they wait for other calls to finish:

| call | waits for |
|---|:---|
| `stardust_db_close` | running calls; interrupts waits first |
| `stardust_db_subscription_release` | running `next` calls and callbacks |
| `stardust_db_plugin_call` | whatever the op does: a route, not a call of its own |
