---
description: "the stardust C ABI in alpha-2610212237, grouped by use, each call marked sync or async."
---
# stardust alpha-2610212237 abi

every call in the release's `abi/stardust.h`, and every plugin op, grouped by use. each is marked:

- **sync**: does its own work and returns
- **sync, blocks**: also waits on something outside the call (durability, other calls, a lock), with no timeout
- **sync, timeout**: waits up to a timeout you pass; 0 checks once, then `TIMEOUT`
- **async**: a callback says when to call again without waiting

only [live queries](#live-queries) are async. every [events](#events) wait is sync, timeout.

## setup {#setup}

| call | sync or async | |
|---|:---|:---|
| `stardust_abi_version` | sync | |
| `stardust_activate` | sync | license token |
| `stardust_license` | sync | |
| `stardust_plugins` | sync | plugin libraries found next to the library |
| `stardust_db_open` | sync, timeout | waits for another process's file lock up to `lock_timeout_ticks`; 0 fails at once, negative waits forever |
| `stardust_db_close` | sync, blocks | waits for running calls |
| `stardust_db_retain` | sync | |
| `stardust_db_release` | sync | |

## documents {#documents}

native values, in and out of every other call.

| call | sync or async | |
|---|:---|:---|
| `stardust_parse` | sync | JSON, DUST, EDN or TRON text to a document |
| `stardust_serialize` | sync | a document to text |
| `stardust_text_data` | sync | |
| `stardust_text_error` | sync | |
| `stardust_text_release` | sync | |
| `stardust_document_root` | sync | |
| `stardust_document_count` | sync | |
| `stardust_document_read_many` | sync | |
| `stardust_document_member` | sync | |
| `stardust_document_pointer` | sync | |
| `stardust_document_pointer_text` | sync | |
| `stardust_document_i64` | sync | |
| `stardust_document_f64` | sync | |
| `stardust_document_error` | sync | |
| `stardust_document_release` | sync | |
| `stardust_document_builder_create` | sync | |
| `stardust_document_builder_add_many` | sync | |
| `stardust_document_builder_seal` | sync | |
| `stardust_document_builder_error` | sync | |
| `stardust_document_builder_release` | sync | |
| `stardust_pointer_parse` | sync | |
| `stardust_pointer_format` | sync | |

## results {#results}

| call | sync or async | |
|---|:---|:---|
| `stardust_result_take_document` | sync | |
| `stardust_result_error` | sync | |
| `stardust_result_release` | sync | |
| `stardust_db_result_commit_info` | sync | |

## writes {#writes}

writes return once durable.

| call | sync or async | |
|---|:---|:---|
| `stardust_db_merge_patch` | sync, blocks | one transaction |
| `stardust_db_mutation_open` | sync | stored or inline |
| `stardust_db_mutation_run` | sync, blocks | |
| `stardust_db_mutation_error` | sync | |
| `stardust_db_mutation_release` | sync | |
| `stardust_db_bulk_begin` | sync | |
| `stardust_db_bulk_add` | sync | on a database that already holds facts, each patch commits as it's added: sync, blocks |
| `stardust_db_bulk_finish` | sync, blocks | one commit |
| `stardust_db_bulk_abort` | sync | |

## reads {#reads}

| call | sync or async | |
|---|:---|:---|
| `stardust_db_read_entity` | sync | |
| `stardust_db_query_open` | sync | stored or inline |
| `stardust_db_query_run` | sync | plans and runs; returns a cursor |
| `stardust_db_rows_next` | sync | one page |
| `stardust_db_rows_error` | sync | |
| `stardust_db_rows_release` | sync | |
| `stardust_db_query_error` | sync | |
| `stardust_db_query_release` | sync | |
| `stardust_db_indexes_sync` | sync, timeout | until background indexes are built |

## live queries {#live-queries}

| call | sync or async | |
|---|:---|:---|
| `stardust_db_query_subscribe` | sync | event 1 is the initial result |
| `stardust_db_subscription_set_notify` | async | once, before any `next` |
| `stardust_db_subscription_next` | sync, timeout | with notify: `next(0)`, and `TIMEOUT` arms one wake-up |
| `stardust_db_subscription_cancel` | sync | wakes every waiting `next` |
| `stardust_db_subscription_error` | sync | |
| `stardust_db_subscription_release` | sync, blocks | waits for running `next` calls and callbacks |

the callback may run on any thread and must not call stardust.

## definitions {#definitions}

stored queries, mutations, expressions, macros and plugin definitions.

| call | sync or async | |
|---|:---|:---|
| `stardust_db_definition_patch` | sync, blocks | a commit |
| `stardust_db_definition_get` | sync | |
| `stardust_db_definition_list` | sync | |
| `stardust_db_plan` | sync | status, history and explain read; pin, unpin, replan and revert commit (sync, blocks) |

## expressions {#expressions}

| call | sync or async | |
|---|:---|:---|
| `stardust_expr_build` | sync | |
| `stardust_db_expression_build` | sync | stored or inline, with the database's macros |
| `stardust_macro_expand` | sync | |
| `stardust_macro_build` | sync | |
| `stardust_expr_evaluate` | sync | |
| `stardust_expr_program_error` | sync | |
| `stardust_expr_program_release` | sync | |

## plugins {#plugins}

every plugin op below goes through one of these.

| call | sync or async | |
|---|:---|:---|
| `stardust_db_plugin_call` | as the op | `(db, "events", "receive", request, &result)` |
| `stardust_db_plugin_stream_open` | sync | stream ops: `objects.upload`, `objects.open` |
| `stardust_db_plugin_stream_write` | sync | |
| `stardust_db_plugin_stream_read` | sync | |
| `stardust_db_plugin_stream_seek` | sync | |
| `stardust_db_plugin_stream_finish` | as the op | |
| `stardust_db_plugin_stream_release` | sync | |

### events {#events}

temporary pub/sub and exchanges, within one open database. nothing is stored.

| op | sync or async | |
|---|:---|:---|
| `events.status` | sync | |
| `events.open_owner` | sync | `{owner}` |
| `events.close_owner` | sync | ends everything the owner holds; its waiting calls return `CANCELLED` |
| `events.publish` | sync | never waits: a full queue drops and counts it |
| `events.subscribe` | sync | |
| `events.receive` | sync, timeout | `maxWait` |
| `events.unsubscribe` | sync | |
| `events.register` | sync | |
| `events.unregister` | sync | |
| `events.accept` | sync, timeout | `maxWait` |
| `events.call` | sync | returns a handle; `no_worker` if none is registered |
| `events.send` | sync | never waits: a full queue cancels the exchange |
| `events.finish` | sync | |
| `events.cancel` | sync | |
| `events.read` | sync, timeout | `maxWait` |

### objects {#objects}

files and directories, with content-addressed payloads.

| op | sync or async | |
|---|:---|:---|
| `objects.get` | sync | |
| `objects.lookup` | sync | |
| `objects.children` | sync | |
| `objects.path` | sync | |
| `objects.files` | sync | |
| `objects.manifest` | sync | |
| `objects.create` | sync, blocks | a commit |
| `objects.update` | sync, blocks | a commit |
| `objects.move` | sync, blocks | a commit |
| `objects.remove` | sync, blocks | a commit |
| `objects.upload` | stream | finish publishes the payload: sync, blocks |
| `objects.open` | stream | sync |

### backups, sdk, skills {#background}

background work; each `sync` waits for it.

| op | sync or async | |
|---|:---|:---|
| `backups.status` | sync | |
| `backups.sync` | sync, timeout | until every target confirms; `maxWait` |
| `sdk.status` | sync | |
| `sdk.sync` | sync, timeout | until the generated SDK is published; `maxWait` |
| `skills.status` | sync | |
| `skills.sync` | sync, timeout | until the generated skills are published; `maxWait` |

### blackholes {#blackholes}

| op | sync or async | |
|---|:---|:---|
| `blackholes.preview` | sync | changes nothing |
| `blackholes.execute` | sync, blocks | rewrites history in the database and every backup |
| `blackholes.audit` | sync | |
| `blackholes.status` | sync | |

### reactors {#reactors}

| op | sync or async | |
|---|:---|:---|
| `reactors.status` | sync | reactor changes commit with the write that causes them |
