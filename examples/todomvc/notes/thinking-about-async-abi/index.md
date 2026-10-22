---
release: alpha-2610212237
description: "the four stardust calls that wait for the next thing to arrive, and how each could tell an async client when it's ready. applies to alpha-2610212237."
---
# thinking about an async abi

this note describes the C ABI in [alpha-2610212237](../alpha-2610212237/).

## the four calls {#calls}

| call | waits for | async |
|---|:---|:---|
| `stardust_db_subscription_next` | the next result of a live query | yes |
| `events.receive` | the next published event | not yet |
| `events.accept` | the next call to a subject you registered | not yet |
| `events.read` | the next message in an exchange | not yet |

a live query is async with `set_notify`:

```c
typedef void (*stardust_db_subscription_notify)(void *ctx);
uint32_t stardust_db_subscription_set_notify(
    stardust_db_subscription *sub,
    stardust_db_subscription_notify notify,
    void *ctx);
uint32_t stardust_db_subscription_next(
    stardust_db_subscription *sub,
    int64_t max_wait_ticks,
    stardust_db_live_event *event);
```

the three `events` calls go through `stardust_db_plugin_call`, and a plugin request can't carry a function pointer. so they'd need a C function like this one:

```c
typedef void (*stardust_db_notify)(void *ctx);
uint32_t stardust_db_plugin_set_notify(
    stardust_db *db,
    stardust_bytes plugin,
    const stardust_document *target,
    stardust_db_notify notify,
    void *ctx);
```

where `target` is `{owner 1 subscription 10}` for `receive`, `{owner 2 registration 20}` for `accept`, or `{owner 1 handle 30}` for `read`.

all four then work the same way:

1. register `notify` once, before the first call.
2. call with a wait of 0. you get a result, or `TIMEOUT`.
3. after a `TIMEOUT`, stardust calls `notify` once, when the next one is ready. then go back to step 2.

`notify` can run on any thread, and must not call stardust.
