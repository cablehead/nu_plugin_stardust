---
release: alpha-2610230456
thread: async abi
part: 2
tables: words
description: "delaney's proposal: one response queue per database, for every operation"
source: https://gist.github.com/delaneyj/9f5b2d9fc5eca74d8a3ccf2c1ecf9d44
code:
  requests.h: long-term-boundary
  requests.c: long-term-boundary
  requests.rs: long-term-boundary
  requests.py: long-term-boundary
  requests.go: long-term-boundary
  requests.schema.json: long-term-boundary
  responses.schema.json: long-term-boundary
  responses.dust: long-term-boundary
---
# Stable C execution ABI

One database-owned response queue serves core operations and enabled plugins. New operations add schemas and generated language methods. The C execution interface stays fixed.

This is a proposal. The attached Rust, Python, and Go examples describe generated APIs that are not implemented yet.

## Replacement

| Existing interface | Replacement |
| --- | --- |
| Query cursors, page-next calls, and subscription handles | Requests producing pages or continuing updates |
| Subscription `set_notify` and proposed waitsets | One optional wake callback per database |
| Events owners, subscription IDs, and repeated receive waits | Streaming requests owned by the database |
| Mutation and plugin-call entry points | Typed request documents submitted through one function |
| Separate result handles and diagnostic accessors on these paths | Response documents containing results and errors |

Database lifecycle, licensing, documents, builders, codecs, and standalone expression/macro construction remain separate. Byte-stream exports need explicit bounded-chunk contracts before removal. There is no compatibility layer or second implementation behind the old execution functions.

## C interface

```c
typedef void (*stardust_wake)(void *ctx);

int64_t stardust_submit(
    stardust_db *db, const stardust_document *requests,
    int64_t *firstRequestId);

int64_t stardust_poll(
    stardust_db *db, int64_t max_wait_ticks,
    stardust_document **output);

int64_t stardust_cancel(stardust_db *db, int64_t requestId);
```

The existing database-open options gain `responses` (a limits document), `wake`, and `wake_ctx`. They are fixed at open. Limits and future execution options stay in documents. The existing database handle owns the queue. No channel handle or separate queue lifetime is needed.

`submit` takes an array of input frames, including one-item batches. New requests omit `requestId`. The database generates IDs and returns the first one through `firstRequestId`. Generated language methods hide IDs inside typed request objects.

For example, one submission starts two subscriptions and one push stream. None of these new requests contains a `requestId` field:

```text
[
  {type querySubscribe path todos final true}
  {type eventsSubscribe pattern todos.changed final true}
  {type eventsPush subject todos.import data {id 1}}
]
```

For N new requests, submission atomically reserves one contiguous ID block. The ith new request receives `firstRequestId + i`, starting at zero. Concurrent submissions cannot interleave their blocks. No ID array is returned, and IDs are not reused during the database's lifetime. An ID allocation that would overflow is rejected without wrapping.

If the library returns `firstRequestId = 17`, the query subscription is 17, the event subscription is 18, and the push stream is 19. These are assigned IDs, not caller-selected values.

The next submission feeds and closes that existing push stream. Its frames therefore include the assigned ID:

```text
[
  {requestId 19 type eventsPush data {id 2}}
  {requestId 19 type eventsPush final true}
]
```

This call returns `firstRequestId = 0` because it starts no new requests. `requests.c` demonstrates deriving the actual stream ID from the first call, rather than hard-coding 19. Generated Rust/Python/Go methods perform these steps internally and store the assigned ID in their request object.

Continuation frames contain the previously assigned `requestId`, populated by the binding. They consume no new IDs. Mixed batches number only their new requests, in input order. Frames for the same stream are applied in array order. An empty batch or a batch containing only continuations returns `OK` with `firstRequestId` zero.

Admission is all-or-nothing: validate and reserve resources for every frame before accepting any. On submission failure, no frame is accepted and `firstRequestId` is zero. Accepted operations execute independently, so batch submission is not a database transaction. Before return, submission copies input values or keeps ownership references. Submission does not wait for execution.

`poll` returns an owned native array document. Release it with the existing document-release function. Zero wait checks immediately. Positive waits use 100-nanosecond ticks and wait only for the first response, not a full batch. Expiration returns `OK` with `[]`. `STARDUST_END` means the database is closed and its queue is empty.

Accepted operation failures appear in responses. C statuses report submission or polling failures that cannot return such a response. An open database with no active requests still returns `[]`.

## Response and streaming contract

```text
[
  {requestId 17 type queryPage result {sequence 1 transaction 42 lastPage true rows [{id 1}]}}
  {requestId 18 type eventsMessage result {subject todos.changed sequence 7 data {id 1}}}
  {requestId 19 type completed final true error {code failed message Failed}}
]
```

Each item has `requestId`, a camelCase `type`, and exactly one of `result` or `error`. A batch can contain any mixture. Errors contain `code`, `message`, and optional structured `details`. `result null` remains explicit because it identifies success.

| Marker | Meaning |
| --- | --- |
| Request `final true` | Input is closed. Output can continue. |
| Response `final true` | The request ends. Further input is rejected. |
| Query page `lastPage true` | This snapshot ends. A subscription can continue. |

Streaming input reuses its assigned request ID through `submit`. Generated `push` and `finish` methods fill that metadata automatically. Single-input methods automatically send `final true`, including subscriptions whose output continues. Cancellation aborts both directions. Collecting the final response releases request state, but does not recycle its ID.

Producers omit metadata equal to declared schema defaults: `final` and `lastPage` default to false, while `page`, `superseded`, and `dropped` default to zero. Fresh Go/Odin result structs already have those values. Rust/Python types declare the defaults. User payload values are preserved.

Defaults are decoder contracts. JSON Schema validation does not insert them.

Pages retain their snapshot sequence and transaction. One request finishes a snapshot's pages before starting another. Different requests can interleave. Batching does not change mutation atomicity.

## Schemas and generated types

**The effective schemas depend on the opened database.** Their `oneOf` alternatives contain core types plus the enabled plugins' types, with any relevant startup constraints. The attached schema files illustrate a database with Events and Reactors enabled. They are not universal schemas for every database.

A core request exposes the effective request and response schemas:

```text
[{type schema final true}]
```

Its `schema` response contains `result.requests` and `result.responses`. Core completion/error responses remain available when a requested plugin is disabled or unknown. Such requests can therefore return a schema-valid `completed` error, such as `unavailable`.

The operation registry associates input schemas, output schemas, and each direction's lifetime. Generation produces language request types, response variants, and typed methods from those contracts. Generic bindings can ship types for all supported plugins. A method's presence does not mean its plugin is enabled.

Database-specific SDK generation uses the effective schemas and stored definitions to produce a further typed layer over the bindings. It remains a development plugin, not a network client. Both layers use native documents without a JSON text round trip.

## Threading and ownership

Any application thread may submit or cancel. Bounded library workers execute accepted work independently of polling. Exactly one dispatcher polls each database and routes responses by `requestId`. It has no native-thread affinity, and the queue is not per thread.

The wake callback can run on any native thread, including before submission returns. Calls are serialized per database and occur outside internal locks. The callback only signals the host scheduler. It must not call Stardust or run application handlers. Keep `wake_ctx` valid until database close returns.

Blocking callers can wait in native `poll`. Async bindings install a waiter before a zero-wait check, then await a latched wake signal. An empty check atomically arms notification. Idle requests do not occupy a native thread each.

Generated methods share one dispatcher: a goroutine in Go, a task on the owning asyncio loop in Python, or a Rust dispatcher. Application tasks receive typed batches through bounded per-request buffers. They do not compete to poll the native queue. Polling and decoding work must be bounded or use bounded helpers outside event-loop/runtime workers.

The binding serializes submission and routing registration against response dispatch. It derives each new ID from the returned block and installs all mappings before dispatching their responses. The native wake callback only signals and never acquires this registration lock. No application handler runs under that lock. This handles responses that arrive before `submit` returns.

Raw consumers implement their own dispatcher and cannot poll alongside the managed binding dispatcher. Queue, batch, input, and language-buffer limits are fixed or bounded. A slow consumer follows its operation's overflow policy or is canceled without blocking every other request.

Database close cancels work, wakes polls, and waits for native calls and callbacks. No callback runs after close returns. To collect queued responses afterward, retain a database reference before close, poll until `END`, then release it. Returned documents remain valid independently. Bindings perform blocking cleanup outside async runtime workers.

## Long-term boundary

The C ABI carries native documents and manages execution, waiting, cancellation, and ownership. Operation names, input shapes, output variants, and plugin selection belong to schemas. Adding an operation extends contracts and generated APIs without adding execution exports or another async mechanism.

New operation errors add document codes, not C status values. Pages, live updates, and client streaming share the same request IDs, completion markers, and response batches.

This stabilizes the binary execution boundary. Schemas remain public operation contracts, generated and validated from the same registry.

The attached header, language syntax, schemas, and DUST examples have artifact checks. Runtime behavior still requires implementation and native binding tests.
