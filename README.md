# nu_plugin_stardust

A [Nushell](https://www.nushell.sh) plugin that embeds the
[Stardust](https://www.stardustdb.com) database. Open a database, write to it,
run queries as streaming nu tables, and follow live query results, all from
the nu prompt.

This is a prototype against Stardust's Rust binding (release
`alpha-2610212237`). It doesn't include Stardust itself: you need your own
release download and license token.

## Requirements

- Nushell **0.116.0**. Plugins must match nu's version exactly; change the
  `nu-plugin` / `nu-protocol` pins in `Cargo.toml` for another nu.
- Rust 1.85 or newer.
- A Stardust release for your platform, extracted **next to this repo**:

  ```
  ~/stardust-alpha-2610212237/   # the release
  ~/nu_plugin_stardust/          # this repo
  ```

  `Cargo.toml` points at `../stardust-alpha-2610212237/abi/bindings/rust`.
  Edit that path for another release or location.
- A Stardust license token (the `v4.public.…` text from your account page).

## Build and register

```nu
cargo build --release
plugin add target/release/nu_plugin_stardust
plugin use stardust
```

The plugin links Stardust's static archive, so the binary is self-contained and
nu can launch it without `LD_LIBRARY_PATH`.

## License token

Stardust reads no environment variable or file itself, so the plugin loads the
token for it, from the first of:

1. `$env.STARDUST_LICENSE`
2. `~/.config/stardust/license`

```nu
mkdir ~/.config/stardust
"v4.public.…" | save -f ~/.config/stardust/license
chmod 600 ~/.config/stardust/license

stardust license    # holder, tier and dates; never the token
```

## Walkthrough

### Open a database and write to it

```nu
let db = stardust open ~/people.db

# keys like #_ada name new entities
'{
  #_ada {name Ada role engineer age 36}
  #_bob {name Bob role engineer age 41}
  #_cy  {name Cy  role designer age 29}
}' | stardust patch $db
# => {status: committed, transaction: 4, facts: 9, committed_at: …, ids: {#_ada: 1, #_bob: 2, #_cy: 3}}
```

### Documents: records or DUST

Anywhere a command takes a document (a patch, a query or mutation, a
definition, `--params`, `--input`), it accepts:

- a record or list: nu data
- a string: DUST text, or JSON or EDN with `--format json` / `--format edn`
- binary: TRON

So the patch above can also be a record. Quote the `#_` keys there, since `#`
starts a nu comment:

```nu
{'#_ada': {name: Ada role: engineer age: 36}} | stardust patch $db
```

A string that holds only a name, like `people/by_age`, is a stored
definition's path.

### Query

Rows stream in one page at a time, so they work with every nu table command,
and `| first n` stops reading early:

```nu
stardust query $db {find: [?name ?age] where: [[?e name ?name] [?e age ?age]]}
| where age > 30
| sort-by age
```

Or let Stardust filter and sort, which can use its indexes:

```nu
stardust query $db '{
  find    [?name ?age]
  where   [[?e name ?name] [?e age ?age] [> ?age 30]]
  orderBy [?age]
}'
```

### Stored definitions

```nu
stardust define $db queries people/by_age '{
  find       [?name]
  parameters {min 18}
  where      [[?e name ?name] [?e age ?age] [>= ?age ?min]]
}'

stardust query $db people/by_age --params {min: 40}
stardust definitions $db queries                 # => [people/by_age]
stardust definition $db queries people/by_age
stardust define $db queries people/by_age null   # delete
```

### Mutations

```nu
stardust mutate $db '{query {find [?e] where [[?e name Ada]]} patch {?e {age 37}}}'
# => committed, then no_change if you run it again
```

### Live queries

`stardust subscribe` streams the query's result now, and again after every
commit that changes it. Each event is the **whole** result; `superseded` counts
intermediate results skipped during a burst of commits.

```nu
stardust subscribe $db people/by_age | each { $in.result }   # Ctrl-C to stop
```

To keep the prompt free, run the subscription in a background job and have it
send events to the main thread's mailbox:

```nu
let sub = job spawn {
    stardust subscribe $db people/by_age | each {|ev| $ev | job send 0 --tag 7 }
}

job recv --tag 7                    # the current result
'{#_dee {name Dee age 52}}' | stardust patch $db
job recv --tag 7 --timeout 5sec     # the result after that commit

job kill $sub
```

### Expressions

```nu
stardust eval '[* ?n 2]' --input {n: 21}     # => 42 (a string is DUST text)
stardust eval [+ ?x 1] --input {x: 41}       # => 42 (or nu data)
```

### Formats

`from` and `to` for Stardust's DUST, EDN and TRON. DUST and EDN are text
(`--width` wraps lines); TRON is binary. `open` and `save` use them by file
extension.

```nu
open people.dust
{a: 1 b: [x y]} | to edn                # EDN text
{a: 1} | save data.tron                 # `save` runs `to tron`
open data.tron                          # => {a: 1}
```

### Stardust plugins

Stardust's own plugins (reactors, backups, objects, ...) are off unless
named at open.

```nu
stardust plugins | select name description

let db = stardust open ~/people.db --plugins {reactors: {}}
stardust plugin-call $db reactors status
```

### Values

| nu | Stardust |
| --- | --- |
| nothing, bool, int, float, string | null, boolean, i64, f64, string |
| list, record | array, object (member order kept) |
| date | `{#utc "2026-10-07T12:34:56Z"}` (100 ns resolution) |
| duration | `{#dur "PT1.5S"}` |
| filesize | i64 bytes |

Entity references stay as `{#link id}` records.

## How it works

- **Handles.** `stardust open` returns a custom value holding an ID. The
  plugin keeps the open `Database` in a table; nu tells the plugin when the
  last copy of a handle is dropped, and the database closes then.
  `stardust close $db` closes it immediately.
- **Staying alive.** nu stops idle plugins, which would close every database.
  While any handle is open the plugin disables that with
  `engine.set_gc_disabled(true)`, and re-enables it when the last one closes.
- **Threads.** In this release the database and its handles are `Send + Sync`,
  so one plugin process serves every call. All jobs in a nu session share it.
  Every call into Stardust runs on a small fixed pool of threads
  (`src/pool.rs`): Stardust keeps memory for each thread that calls it, even
  after the thread exits, and nu runs plugin calls on new threads.
- **Subscriptions.** The binding's `next` is async; nu streams are iterators.
  So the stream polls `next().await` on its own thread, parking until
  Stardust wakes it.
  - Ctrl-C: a signal handler (`engine.register_signal_handler`) calls a
    `Canceller`, which wakes the waiting `next` at once.
  - nu exiting: nothing tells a waiting iterator that nu has gone, and the
    plugin can't exit while that thread runs. So between events the stream
    waits at most 1 s, pings nu, and ends when the ping fails. A `next`
    future dropped mid-wait loses no event.

## Example: TodoMVC

[`examples/todomvc`](examples/todomvc) is the Datastar TodoMVC served by
http-nu, which loads this plugin with `--plugin`. Writes are patches, and
each viewer streams a live query over SSE.

## Known limitations

- **One process per database file.** Stardust takes an exclusive lock, so a
  second nu session can't open a database that another has open (`locked`).
  Use jobs within one session, as above.
- **Ctrl-C cancels every subscription in the plugin.** nu sends the interrupt
  to the whole plugin without saying which call it was for, so interrupting any
  foreground command also ends subscriptions running in background jobs.
- **`job kill` doesn't reach the plugin.** A killed job's subscription keeps
  running until its next event, then stops.
- **Abandoned streams linger.** After `stardust subscribe … | first n`, the
  subscription ends at its next event, not immediately.

The last three come from the same gap: nu doesn't tell a plugin's iterator
that its reader has gone.

## Smoke test

`smoke.nu` drives every command with the release's example data
(`abi/examples/data`) and checks each result against the file's expected
values, including a subscription receiving a commit from a background job:

```nu
nu smoke.nu
# eval: ok, patch: ok, query: ok, mutate: committed then no_change,
# definitions: ok, documents: ok, subscribe: ok, time: ok, formats: ok, plugins: ok, all ok
```

## Layout

```
src/main.rs      serve_plugin
src/plugin.rs    open databases, handles, license loading
src/commands.rs  the commands
src/convert.rs   nu values <-> Stardust documents (with unit tests)
smoke.nu         end-to-end test
examples/todomvc Datastar TodoMVC on http-nu, state in Stardust
```
