---
release: alpha-2610230456
thread: async abi
part: 3
description: "how delaney's response queue differs from our waitset"
---
# how delaney's differs

[ours](../thinking-about-async-abi/) adds a waitset next to today's calls. [delaney's](../delaneys-execution-abi/) replaces the calls.

| | ours | delaney's |
|---|:---|:---|
| what you wait on | a waitset you open | the database's queue |
| wake callback | `waitset_set_notify` | set at open |
| which source | a token you pick | an id stardust assigns |
| a wake tells you | which sources are ready | nothing. `poll` hands you the results |
| then you | call `subscription_next` or `plugin_call` | handle the results |
| live queries | `subscription_next` as today | `querySubscribe` requests |
| events | `plugin_call` as today, plus `waitset_add_plugin` | `eventsSubscribe` and `eventsPush` requests |
| errors | C statuses | error responses |
| stop | release the handle | `cancel` the id |
| plugins | a readiness hook per waiting call | an operation is a schema. results go to the queue |
| SDKs | the bindings change where they wait | generated from the schemas |
| breaks | `subscription_set_notify` | every execution call |

## what stays {#stays}

- one wake for everything, from any thread, which mustn't call stardust.
- an empty check arms the next wake. ours: a `TIMEOUT`. his: a `poll` that returns `[]`.
- one thread or task drains and routes. ours: `stardust-wake`. his: the dispatcher.
- no thread per waiting call.

## what's new in his {#new}

- **results are pushed, not pulled.** ours leaves results in stardust until you call. his queues them, so each request needs a bounded buffer and a policy for a slow consumer.
- **streams in.** `eventsPush` sends frames to a running request. ours has nothing like it.
- **batches.** one `submit` starts many requests, and accepts all of them or none.
- **one dispatcher.** a program that polls the queue itself can't share it with the binding's dispatcher.
- **no `add_plugin`.** a plugin adds schemas, so the plugin question goes away.
