---
release: alpha-2610212237
description: "which stardust calls wait until something happens, and what async support each has today. applies to alpha-2610212237."
---
# thinking about an async abi

this note describes the C ABI in [alpha-2610212237](../alpha-2610212237/).

it splits the calls into two groups:

- [calls that wait](#waits). these block until something happens, and that may never happen. an async client needs a way to wait for these without blocking a thread.
- [calls that don't wait](#sync). these do their own work and return. an async client doesn't need anything new for them. they're listed so you can check each one belongs there.

## calls that wait {#waits}

### what each call waits for {#wait-list}

these calls can become ready again and again, so you call them in a loop:

| call | waits for |
|---|:---|
| `stardust_db_subscription_next` | the next result of a live query |
| `events.receive` | the next published event |
| `events.accept` | the next call to a subject you registered as a worker |
| `events.read` | the next message in an exchange, or the other side finishing or cancelling |

only `stardust_db_subscription_next` can tell an async client when it's ready, through [`stardust_db_subscription_set_notify`](#notify). for the three `events` calls, you can only block, for up to `maxWait`.

these calls wait for one thing to finish, so you call them once:

| call | waits for |
|---|:---|
| `stardust_db_indexes_sync` | background indexes to finish building |
| `backups.sync` | every backup target to confirm the latest commit |
| `sdk.sync` | the generated SDK to be published |
| `skills.sync` | the generated skills to be published |
| `stardust_db_open` | another process to release the database's file lock |

none of these can tell an async client when it's ready. you can only block, for up to `maxWait`, or `max_wait_ticks` for `stardust_db_indexes_sync`, or `lock_timeout_ticks` for `stardust_db_open`, where a negative value blocks forever.

the `events` operations are called through `stardust_db_plugin_call(db, "events", op, request, &result)`.

if you pass a wait of 0, or leave it out, the call checks once and returns `TIMEOUT` straight away when nothing is ready.

### how a live query tells you it's ready {#notify}

this is the only async mechanism in the ABI today:

```c
typedef void (*stardust_db_subscription_notify)(void *ctx);
uint32_t stardust_db_subscription_set_notify(stardust_db_subscription *sub, stardust_db_subscription_notify notify, void *ctx);
uint32_t stardust_db_subscription_next(stardust_db_subscription *sub, int64_t max_wait_ticks, stardust_db_live_event *event);
```

1. call `set_notify` once, before you call `next` for the first time.
2. call `next` with a wait of 0. you get either a result, or `TIMEOUT`.
3. a `TIMEOUT` means stardust will call your `notify` function once, when a result is ready. then go back to step 2.

your `notify` function can be called on any thread. it must not call stardust. once `stardust_db_subscription_release` returns, `notify` won't be called again.

### what stops a wait early {#interrupts}

| a wait in | ends early when you call | and returns |
|---|:---|:---|
| `stardust_db_subscription_next` | `stardust_db_subscription_cancel` or `stardust_db_subscription_release` | `CANCELLED` |
| `events.receive` | `events.unsubscribe` or `events.close_owner` | `CANCELLED` |
| `events.accept` | `events.unregister` or `events.close_owner` | `CANCELLED` |
| `events.read` | `events.close_owner` | `CANCELLED` |
| any of the above | `stardust_db_close` | `CLOSED` |

## calls that don't wait {#sync}

each of these does its own work and returns. some can take a while, such as a big query, or a write that waits until it's safely on disk. but none of them waits for something that might never happen.

### setup {#setup}

`stardust_abi_version`, `stardust_activate`, `stardust_license`, `stardust_plugins`, `stardust_db_retain`, `stardust_db_release`

### documents and results {#documents}

`stardust_parse`, `stardust_serialize`, `stardust_text_data`, `stardust_text_error`, `stardust_text_release`, `stardust_pointer_parse`, `stardust_pointer_format`, `stardust_document_root`, `stardust_document_count`, `stardust_document_read_many`, `stardust_document_member`, `stardust_document_pointer`, `stardust_document_pointer_text`, `stardust_document_i64`, `stardust_document_f64`, `stardust_document_error`, `stardust_document_release`, `stardust_document_builder_create`, `stardust_document_builder_add_many`, `stardust_document_builder_seal`, `stardust_document_builder_error`, `stardust_document_builder_release`, `stardust_result_take_document`, `stardust_result_error`, `stardust_result_release`, `stardust_db_result_commit_info`

### reads {#reads}

`stardust_db_read_entity`, `stardust_db_query_open`, `stardust_db_query_run`, `stardust_db_rows_next`, `stardust_db_rows_error`, `stardust_db_rows_release`, `stardust_db_query_error`, `stardust_db_query_release`, `stardust_db_definition_get`, `stardust_db_definition_list`, `stardust_db_plan` when it reads status, history or explain, `objects.get`, `objects.lookup`, `objects.children`, `objects.path`, `objects.files`, `objects.manifest`, `blackholes.preview`, `blackholes.audit`, and the `status` operation of every plugin

### writes {#writes}

a write returns once its changes are safely on disk.

`stardust_db_merge_patch`, `stardust_db_mutation_open`, `stardust_db_mutation_run`, `stardust_db_mutation_error`, `stardust_db_mutation_release`, `stardust_db_bulk_begin`, `stardust_db_bulk_add`, `stardust_db_bulk_finish`, `stardust_db_bulk_abort`, `stardust_db_definition_patch`, `stardust_db_plan` when it pins, unpins, replans or reverts, `objects.create`, `objects.update`, `objects.move`, `objects.remove`, `blackholes.execute`

### expressions {#expressions}

`stardust_expr_build`, `stardust_db_expression_build`, `stardust_macro_expand`, `stardust_macro_build`, `stardust_expr_evaluate`, `stardust_expr_program_error`, `stardust_expr_program_release`

### plugin streams {#streams}

`objects.upload` and `objects.open` use these: `stardust_db_plugin_stream_open`, `stardust_db_plugin_stream_write`, `stardust_db_plugin_stream_read`, `stardust_db_plugin_stream_seek`, `stardust_db_plugin_stream_finish`, `stardust_db_plugin_stream_release`

### events calls that act {#events}

none of these wait. when the other side's queue is full, `publish` drops the event, and `send` cancels the exchange.

`events.open_owner`, `events.close_owner`, `events.publish`, `events.subscribe`, `events.unsubscribe`, `events.register`, `events.unregister`, `events.call`, `events.send`, `events.finish`, `events.cancel`

### starting and stopping a live query {#subscriptions}

`stardust_db_query_subscribe`, `stardust_db_subscription_cancel`, `stardust_db_subscription_error`
