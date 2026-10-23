---
release: alpha-2610230456
description: "brainstorming async notify for stardust"
---
# thinking about an async abi

in [alpha-2610230456](../alpha-2610230456/), four calls wait for the next thing to arrive. `stardust_db_subscription_next` can notify an async client when it's ready. `events.receive`, `events.accept` and `events.read` can't, so the client polls them, blocks on them, or runs each on its own thread.

| call | waits for | async |
|---|:---|:---|
| `stardust_db_subscription_next` | the next result of a live query | yes |
| `events.receive` | the next published event | not yet |
| `events.accept` | the next call to a subject you registered | not yet |
| `events.read` | the next message in an exchange | not yet |

a live query is async with `set_notify`:

```c
typedef void (*stardust_db_subscription_notify)(void *ctx);
int64_t stardust_db_subscription_set_notify(
    stardust_db_subscription *sub,
    stardust_db_subscription_notify notify,
    void *ctx);
int64_t stardust_db_subscription_next(
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

int64_t stardust_db_waitset_open(
    stardust_db *db,
    stardust_db_waitset **out);
int64_t stardust_db_waitset_set_notify(
    stardust_db_waitset *ws,
    stardust_db_waitset_notify notify,
    void *ctx);
int64_t stardust_db_waitset_add_subscription(
    stardust_db_waitset *ws,
    stardust_db_subscription *sub,
    int64_t token);
int64_t stardust_db_waitset_ready(
    stardust_db_waitset *ws,
    int64_t max_wait_ticks,
    int64_t *tokens,
    int64_t capacity,
    int64_t *count);
void stardust_db_waitset_release(stardust_db_waitset *ws);
```

a live query joins with `add_subscription`. the `events` calls join through the plugin, with the waitset's id:

```
events.watch {owner 1 subscription 10 waitset 3 token 42}   # receive
events.watch {owner 2 registration 20 waitset 3 token 43}   # accept
events.watch {owner 1 handle 30 waitset 3 token 44}         # read
```
