---
release: alpha-2610212237
description: "the stardust calls that wait for the next thing to arrive, and a brainstorm on a waitset that would tell an async client when any of them is ready. applies to alpha-2610212237."
---
# thinking about an async abi

in [alpha-2610212237](../alpha-2610212237/), four calls wait for the next thing to arrive. `stardust_db_subscription_next` can notify an async client when it's ready. `events.receive`, `events.accept` and `events.read` can't, so the client polls them, blocks on them, or runs each on its own thread.

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

register `notify` once, then call `next` with a wait of 0. you get a result, or `TIMEOUT`. after a `TIMEOUT`, stardust calls `notify` once, when the next one is ready. `notify` can run on any thread, and must not call stardust.

the three `events` calls go through `stardust_db_plugin_call`. a plugin request can't carry a function pointer, so they have no way to register a `notify`.

## a suggestion: a waitset

this is a brainstorm. a waitset, like a small `epoll`, could make all four calls async the same way. it holds the `notify`, and reports which calls are ready. you then make those calls to get the results.

```c
typedef struct stardust_db_waitset stardust_db_waitset;
typedef void (*stardust_db_waitset_notify)(void *ctx);

uint32_t stardust_db_waitset_open(
    stardust_db *db,
    stardust_db_waitset **out);
uint32_t stardust_db_waitset_set_notify(
    stardust_db_waitset *ws,
    stardust_db_waitset_notify notify,
    void *ctx);
uint32_t stardust_db_waitset_add_subscription(
    stardust_db_waitset *ws,
    stardust_db_subscription *sub,
    uint64_t token);
uint32_t stardust_db_waitset_ready(
    stardust_db_waitset *ws,
    int64_t max_wait_ticks,
    uint64_t *tokens,
    size_t capacity,
    size_t *count);
void stardust_db_waitset_release(stardust_db_waitset *ws);
```

a live query joins with `add_subscription`. the `events` calls join through the plugin, with the waitset's id:

```
events.watch {owner 1 subscription 10 waitset 3 token 42}   # receive
events.watch {owner 2 registration 20 waitset 3 token 43}   # accept
events.watch {owner 1 handle 30 waitset 3 token 44}         # read
```

you pick each token. `ready` returns it when its call has something.

all four calls then work the same way:

1. open a waitset, set `notify`, and add each subscription, registration and exchange you want to wait on.
2. when `notify` runs, call `ready` with a wait of 0 to take the ready tokens.
3. for each token, make its call with a wait of 0 until it returns `TIMEOUT`. that rearms it.

a token becomes ready once per `TIMEOUT`, and is in the ready set at most once, so the set never holds more tokens than there are sources. `notify` runs once each time the set goes from empty to not empty. when a source ends, its token becomes ready too, and its call reports `CANCELLED` or `CLOSED`.

a client without callbacks can call `ready` with a wait instead, and wait on every source from one thread.

`ready` returns tokens because the results have different types. a live query returns a `stardust_db_live_event`, and the `events` calls return documents. each call keeps its own result and errors. an `events.next` that returns the next result for a whole owner could still be added on top.

`set_notify` would be the same as a waitset with one subscription in it. a later plugin with a call that waits could use the waitset too, with no new C function.

open questions:

- is there a `remove`, or does a source only leave when it ends?
- one waitset per database, or as many as you like, say one per event loop?
