---
release: alpha-2610230456
thread: async abi
part: 2
description: "delaney's proposal: one response queue per database, for every operation"
code: {requests.h: c}
---
# delaney's: a stable c execution abi

delaney's proposal, from [his gist](https://gist.github.com/delaneyj/9f5b2d9fc5eca74d8a3ccf2c1ecf9d44), in brief. it's a proposal: none of it is implemented yet.

the database gets one response queue. every operation, from core or from a plugin, is a request document you submit, and its results come back through that queue. a new operation adds a schema, not a C function.

## what it replaces {#replaces}

| today | becomes |
|---|:---|
| query cursors, page-next calls, subscription handles | requests that produce pages or continuing updates |
| `subscription_set_notify`, and our waitset | one optional wake callback per database |
| events owners, subscription ids, repeated receive waits | streaming requests owned by the database |
| mutations and `plugin_call` | typed request documents, submitted through one function |
| result handles and diagnostic accessors | response documents, with results and errors |

opening and closing a database, licensing, documents, builders and codecs stay as they are.

## the c interface {#c}

three calls. the wake callback, and limits for the queue, are database-open options, fixed at open.

## requests {#requests}

`submit` takes an array of requests. stardust gives the new ones ids, as one block in order, and returns the first. one submit starts two subscriptions and a push stream:

```text
[
  {type querySubscribe path todos final true}
  {type eventsSubscribe pattern todos.changed final true}
  {type eventsPush subject todos.import data {id 1}}
]
```

if the first id is 17, these are 17, 18 and 19. more input to a stream names its id:

```text
[
  {requestId 19 type eventsPush data {id 2}}
  {requestId 19 type eventsPush final true}
]
```

a submit accepts every request or none. accepted requests run independently, so a batch isn't a transaction.

## responses {#responses}

`poll` returns an array of responses, each with a `requestId`, a `type`, and a `result` or an `error`:

```text
[
  {requestId 17 type queryPage result {sequence 1 transaction 42 lastPage true rows [{id 1}]}}
  {requestId 18 type eventsMessage result {subject todos.changed sequence 7 data {id 1}}}
  {requestId 19 type completed final true error {code failed message Failed}}
]
```

| marker | means |
|---|:---|
| request `final true` | input is closed. output can continue |
| response `final true` | the request has ended |
| `lastPage true` | this snapshot has ended. a subscription continues |

a wait of 0 checks once; a wait above 0 blocks until the first response. `[]` means nothing yet. `STARDUST_END` means the database is closed and its queue is empty. a failed operation is an error response, not a C status.

## schemas {#schemas}

each open database describes its requests and responses, core plus its enabled plugins, in answer to `{type schema final true}`. the Rust, Python and Go SDKs are generated from those schemas as typed methods.

## threads {#threads}

any thread can submit or cancel. one dispatcher per database polls the queue and routes each response, by `requestId`, into a bounded buffer for its request. an async binding arms a waiter, checks with a wait of 0, then awaits the wake. an empty check arms the wake. an idle request doesn't hold a thread.

the wake callback can run on any thread, even before `submit` returns, and mustn't call stardust. calls to it are one at a time per database, and none run after close returns.
