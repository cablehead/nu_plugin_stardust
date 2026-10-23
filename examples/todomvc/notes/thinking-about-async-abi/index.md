---
release: alpha-2610230456
description: "our ask: a waitset, so live queries and the events calls are async the same way"
---
# thinking about an async abi

in [alpha-2610230456](../alpha-2610230456/), a live query can call you back when it has a new result, so an async program can wait for it without tying up a thread. the events plugin's `receive`, `accept` and `read` can't call you back. an async program has to poll them, or block a thread on each one.

| call | waits for | async |
|---|:---|:---|
| `stardust_db_subscription_next` | the next result of a live query | yes |
| `events.receive` | the next published event | not yet |
| `events.accept` | the next call to a subject you registered | not yet |
| `events.read` | the next message in an exchange | not yet |

## the ask {#ask}

replace `stardust_db_subscription_set_notify` with a waitset. a waitset holds one notify callback for everything an async program waits on, live queries and plugin calls alike, and reports which of them are ready.

the change to `stardust.h`. lines starting `-` are removed, lines starting `+` are added:

```diff
 typedef struct stardust_db_subscription stardust_db_subscription;
-typedef void (*stardust_db_subscription_notify)(void *ctx);
-int64_t stardust_db_subscription_set_notify(stardust_db_subscription *sub, stardust_db_subscription_notify notify, void *ctx);
 int64_t stardust_db_subscription_next(stardust_db_subscription *sub, int64_t max_wait_ticks, stardust_db_live_event *event);

+typedef struct stardust_db_waitset stardust_db_waitset;
+typedef void (*stardust_db_waitset_notify)(void *ctx);
+int64_t stardust_db_waitset_open(stardust_db *db, stardust_db_waitset **output);
+int64_t stardust_db_waitset_set_notify(stardust_db_waitset *ws, stardust_db_waitset_notify notify, void *ctx);
+int64_t stardust_db_waitset_add_subscription(stardust_db_waitset *ws, stardust_db_subscription *sub, int64_t token);
+int64_t stardust_db_waitset_add_plugin(stardust_db_waitset *ws, stardust_bytes plugin_name, stardust_bytes operation, const stardust_document *request, int64_t token);
+int64_t stardust_db_waitset_ready(stardust_db_waitset *ws, int64_t max_wait_ticks, int64_t *tokens, int64_t capacity, int64_t *count);
+void stardust_db_waitset_release(stardust_db_waitset *ws);
```

`stardust_db_waitset_add_plugin` takes what you'd pass to `stardust_db_plugin_call` for the call it watches, without `maxWait`. for the events plugin:

| operation | request | waits for |
|---|:---|:---|
| `receive` | `{owner 1 subscription 10}` | the next published event |
| `accept` | `{owner 2 registration 20}` | the next call to a subject you registered |
| `read` | `{owner 1 handle 30}` | the next message in an exchange |

## how it behaves {#behaves}

two things are new:

- each source gets a token, a number you pick when you add it with `stardust_db_waitset_add_subscription` or `stardust_db_waitset_add_plugin`. today each subscription has its own notify callback, set with `stardust_db_subscription_set_notify`, and its `ctx` tells you which one is ready. a waitset has one notify callback for all its sources, set with `stardust_db_waitset_set_notify`, so `stardust_db_waitset_ready` hands back the tokens of the ones that are ready instead. you then make those calls without waiting: `stardust_db_subscription_next` with `max_wait_ticks` 0, or `stardust_db_plugin_call` without `maxWait`.
- `stardust_db_waitset_ready` with `max_wait_ticks` above 0 blocks, so a program without callbacks can wait on everything from one thread.

the rest is the same as `stardust_db_subscription_set_notify` today, for every source in the waitset:

- a source becomes ready again only after its call returns `STARDUST_TIMEOUT`. it appears in the ready list at most once.
- the notify callback fires once each time the ready list goes from empty to not empty. it can run on any thread, and must not call stardust.
- a source that ends (cancelled, unsubscribed, closed) becomes ready, and its call then returns `STARDUST_CANCELLED` or `STARDUST_CLOSED`.

## what it breaks {#breaks}

removing `stardust_db_subscription_set_notify` breaks the Rust and Python bindings. both build their async wait for `stardust_db_subscription_next` on it. each would move to a waitset holding one subscription.
