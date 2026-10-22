---
release: alpha-2610212237
description: "the four stardust calls that wait for the next thing to arrive, and how each could tell an async client when it's ready. applies to alpha-2610212237."
---
# thinking about an async abi

this note describes the C ABI in [alpha-2610212237](../alpha-2610212237/).

## the four calls {#calls}

each of these calls waits for the next thing to arrive. you call it in a loop, for as long as you're interested.

| call | waits for |
|---|:---|
| `stardust_db_subscription_next` | the next result of a live query |
| `events.receive` | the next published event on a subscription |
| `events.accept` | the next call to a subject you registered as a worker |
| `events.read` | the next message in an exchange, or the other side finishing or cancelling |

the three `events` calls go through `stardust_db_plugin_call(db, "events", op, request, &result)`, with a request such as `{owner 1 subscription 10 maxWait {#dur PT5S}}`.

## the one that's already async {#subscription}

a live query can tell you when its next result is ready:

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

1. call `set_notify` once, before you call `next` for the first time.
2. call `next` with a wait of 0. you get either a result, or `TIMEOUT`.
3. a `TIMEOUT` means stardust will call your `notify` function once, when a result is ready. when it does, go back to step 2.

your `notify` function can be called on any thread, and it must not call stardust. in an async runtime it just wakes the task that's waiting. once `stardust_db_subscription_release` returns, `notify` won't be called again.

## the three that aren't yet {#events}

`events.receive`, `events.accept` and `events.read` can only block a thread, for up to `maxWait`. there's no way to be told when they're ready.

they could work the same way as a live query. the only difference is that a plugin request is a document, and a document can't carry a function pointer. so registering the callback needs its own C function, for example:

```c
typedef void (*stardust_db_notify)(void *ctx);
uint32_t stardust_db_plugin_set_notify(
    stardust_db *db,
    stardust_bytes plugin,
    const stardust_document *target,
    stardust_db_notify notify,
    void *ctx);
```

`target` names what to watch:

| to make this async | target |
|---|:---|
| `events.receive` | `{owner 1 subscription 10}` |
| `events.accept` | `{owner 2 registration 20}` |
| `events.read` | `{owner 1 handle 30}` |

then the same three steps apply:

1. call `stardust_db_plugin_set_notify` once, before the first `receive`, `accept` or `read` on that target.
2. call `receive`, `accept` or `read` with a `maxWait` of 0, or none. you get either a result, or `TIMEOUT`.
3. a `TIMEOUT` means stardust will call your `notify` function once, when the next one is ready. when it does, go back to step 2.

as with a live query, your `notify` function can be called on any thread, and it must not call stardust. it won't be called again once the target is gone: after `events.unsubscribe`, after `events.unregister`, once the exchange has ended, or after `events.close_owner`.

## open question {#question}

this gives each subscription, registration and exchange its own callback. a connection that holds several of them has several callbacks to wire up.

an owner already holds all of a connection's subscriptions, registrations and exchanges. so another option is one callback per owner, called when any of them is ready, plus one call that returns whichever is ready next.
