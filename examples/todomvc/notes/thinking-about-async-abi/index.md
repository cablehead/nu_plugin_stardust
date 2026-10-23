---
release: alpha-2610230456
description: "our ask: a waitset, so live queries and the events calls are async the same way"
---
# thinking about an async abi

in [alpha-2610230456](../alpha-2610230456/), a live query can call you back when it has a new result, so an async program can wait for it without tying up a thread. `events.receive`, `events.accept` and `events.read` can't call you back. an async program has to poll them, or block a thread on each one.

names here drop their `stardust_db_` prefix, except in code.

| call | waits for | async |
|---|:---|:---|
| `subscription_next` | the next result of a live query | yes |
| `events.receive` | the next published event | not yet |
| `events.accept` | the next call to a subject you registered | not yet |
| `events.read` | the next message in an exchange | not yet |

## the ask {#ask}

replace `subscription_set_notify` with a waitset. a waitset holds one notify callback for everything an async program waits on, live queries and plugin calls alike, and reports which of them are ready.

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

`waitset_add_plugin` takes what you'd pass to `plugin_call` for the call it watches, without `maxWait`. for the events plugin:

| call | `plugin_name` | `operation` | `request` |
|---|:---|:---|:---|
| `events.receive` | `events` | `receive` | `{owner 1 subscription 10}` |
| `events.accept` | `events` | `accept` | `{owner 2 registration 20}` |
| `events.read` | `events` | `read` | `{owner 1 handle 30}` |

## how it behaves {#behaves}

two things are new:

- each source gets a token, a number you pick when you add it with `waitset_add_subscription` or `waitset_add_plugin`. today each subscription has its own notify callback, set with `subscription_set_notify`, and its `ctx` tells you which one is ready. a waitset has one notify callback for all its sources, set with `waitset_set_notify`, so `waitset_ready` hands back the tokens of the ones that are ready instead. you then make those calls without waiting: `subscription_next` with `max_wait_ticks` 0, or `plugin_call` without `maxWait`.
- `waitset_ready` with `max_wait_ticks` above 0 blocks, so a program without callbacks can wait on everything from one thread.

the rest is the same as `subscription_set_notify` today, for every source in the waitset:

- a source becomes ready again only after its call returns `STARDUST_TIMEOUT`. it appears in the ready list at most once.
- the notify callback fires once each time the ready list goes from empty to not empty. it can run on any thread, and must not call stardust.
- a source that ends (cancelled, unsubscribed, closed) becomes ready, and its call then returns `STARDUST_CANCELLED` or `STARDUST_CLOSED`.

## what it breaks {#breaks}

removing `subscription_set_notify` breaks the Rust and Python bindings. both build their async wait for `subscription_next` on it. each would move to a waitset holding one subscription.

## the rust sdk {#rust}

a sketch against the alpha-2610230456 binding, not compiled.

today `Database::subscribe` gives each subscription its own `Signal`, and registers it with `subscription_set_notify`. with a waitset, the database opens one waitset, and each subscription joins it. its token is a slot in the database's table of wakers:

```diff
-let signal = Signal::new()?;
-native::stardust_db_subscription_set_notify(
-    raw,
-    Some(Signal::notify),
-    Arc::as_ptr(&signal).cast_mut().cast(),
-);
+let slot = self.wakers.insert()?;
+native::stardust_db_waitset_add_subscription(self.waitset, raw, slot);
```

`Subscription::next().await` keeps its shape. it registers its waker, calls `subscription_next` with `max_wait_ticks` 0, and returns `Pending` on `STARDUST_TIMEOUT`.

the `stardust-wake` thread stays, since a notify callback can't call stardust. on each notify it asks the waitset what's ready, and wakes those slots:

```rust
let mut tokens = [0i64; 64];
loop {
    let mut count = 0;
    native::stardust_db_waitset_ready(waitset, 0, tokens.as_mut_ptr(), 64, &mut count);
    for &token in &tokens[..count as usize] {
        wakers.wake(token);
    }
    if count < 64 {
        break;
    }
}
```

the `events` calls get async versions the same way. each source joins once, with `waitset_add_plugin`. then the call checks once, without `maxWait`:

```rust
pub async fn receive(&self, options: &Document) -> Result<Document> {
    // waitset_add_plugin, once per source
    let slot = self.database.watch("events", "receive", options)?;
    std::future::poll_fn(|context| {
        slot.register(context.waker());
        match self.database.plugin_call("events", "receive", Some(options)) {
            Err(error) if error.status == Status::TIMEOUT => Poll::Pending,
            result => Poll::Ready(result),
        }
    })
    .await
}
```

the `Signal` per subscription goes, and with it the binding's cap of 1024 subscriptions per CPU, past which subscribing returns `STARDUST_LOCKED`.
