# Datastar TodoMVC on Stardust

[TodoMVC](https://data-star.dev/examples/todomvc) served by
[http-nu](https://github.com/cablehead/http-nu), with its state in a
[Stardust](https://www.stardustdb.com) database. Every viewer sees every
change live: open it in two windows and edit in either.

A port of xs's
[datastar-todomvc](https://github.com/cablehead/xs/tree/main/examples/datastar-todomvc),
which keeps its state as an event log in cross.stream and folds it into the
todo list with a reducer (`projection.nu`). Here Stardust holds the todo
list itself, so the reducer is gone:

- **Writes.** Each request commits one Stardust patch or mutation and
  returns 204. It renders nothing.
- **Reads.** Each viewer opens `GET /sse`, an SSE stream of
  `stardust subscribe $db todos/list`. Stardust sends the whole result now
  and again after every commit that changes it. Each result renders
  `#todos`, the header and the list, directly from the event
  (`templates/todos.html` reads its rows, `transaction` and `sequence`) and
  is sent as a `datastar-patch-elements` event.

```
browser --POST/PATCH/DELETE--> route --patch or mutate--> todos.db
                                                            | stardust subscribe
browser <--SSE---------------- .mj render <--result---------+
```

The metrics panel shows the transaction that produced the current result and
the subscription's event count.

The slider travels back through the list's history, one change per step.
Every write tags its transaction with the kind of change it made
(`#_tx {changes todos kind added}`; `#_tx` is the transaction's own entity),
and `todos/changes` lists the tagged transactions. Their kinds are drawn as
colored marks on the slider. Anywhere left of the right edge, the page shows
the list as it was (`todos/list-at`, a query with `bind {with {db {asOf
?at}}}`), with a header saying which change it is and what it did, e.g.
`Change 7 of 10 . Reopened "milk"`. It's read-only: an edit made while
viewing the past would land on the present, so the server renders the state
as text, with no checkboxes, buttons or new-todo input. Back at the right
edge, or with Now, it's live again.

The slider is `<scrub-knob>` (`static/scrub-knob.js`), from http-nu's 2048
example. A press away from the thumb jumps there, and a drag spreads the
whole history across the track, at least 4 pixels per step. The arrow keys
and the buttons beside it step by one. It sits above the list, and a past
change's header and rows are the same height as live ones, so nothing moves
as you scrub.

Transactions from before changes were tagged aren't steps: a database from
an earlier version of this example starts its history at its current list.

Buttons come in three tiers, as in the 2048 example: primary (solid accent:
Clear completed, and Now while live), secondary (a slab that turns accent on
hover: the step buttons) and tertiary (bare text: each row's Delete).

## Requirements

- http-nu 0.18 (embeds nu 0.116, Datastar 1.0.4 with `--datastar`, and
  `.mj` templates)
- the plugin, built: `cargo build --release` in the repo root
- a Stardust license token, as in the [plugin README](../../README.md)

## Run

```bash
cd examples/todomvc
http-nu --datastar --plugin ../../target/release/nu_plugin_stardust :3001 serve.nu
```

Visit http://localhost:3001. The database is `todos.db` next to `serve.nu`;
delete it to start over. `serve.nu` finds its files relative to itself, so it
can be run or `source`d from any directory.

On [cross.stream](https://www.cross.stream) sites, the database goes in
`$env.CROSS_STREAM_SITE_STATE`, which survives deploys, and the license is
read from `stardust.license` there if `$env.STARDUST_LICENSE` isn't set.

Don't use `-w`: a reload opens `todos.db` again while the old script still
holds it, and Stardust refuses with `locked`.

## API

- `GET /` - the page
- `GET /sse` - SSE stream of the todo section. With the signal `at` set to a
  transaction, it renders that transaction once, read-only, and ends.
- `POST /` - add a todo (Datastar signals `{"input": "text"}`)
- `PATCH /` - set every todo's flag (`{"completed": true}`)
- `PATCH /{id}` - set one todo's flag (`{"completed": true}`)
- `DELETE /{id}` - delete a todo
- `DELETE /completed` - delete the completed todos
- `GET /static/*` - files in `static/`

`{id}` is the todo's Stardust entity id; an id that isn't a todo gets 404.
The PATCH body is the state to set, not a toggle, so repeating a request is
harmless. The page sends the opposite of the state it rendered.

## Stored definitions

`serve.nu` defines these at startup:

- `schemas todo`: `{text, completed}`, a non-empty string and a boolean,
  nothing else.
- `queries todos/list`: every todo, oldest first. Viewers subscribe to it.
- `queries todos/list-at`: the same, as of transaction `?at`.
- `queries todos/get`: one todo, by `?id`. Delete checks it.
- `queries todos/completed`: the completed todos. Clear-completed reads it.
- `queries todos/changes`: the tagged transactions and their kinds, oldest
  first. The slider's steps.

Two mutations are built in `serve.nu` and run inline instead:

- `set-one`: sets one todo's `completed`. `no_match` means the id isn't a
  todo.
- `set-all`: sets `completed` on every todo that differs.

They aren't stored because their tag can't be a parameter (`?kind` in a
`#_tx` is `Invalid_Patch`), and Stardust refuses to store a mutation whose
patch has a `#_tx` at all.

A `null` parameter default makes `?id` required. `?completed` can't default
to `null`: a mutation's patch is checked with its defaults filled in, and
`{completed null}` would delete the field. The routes require it instead.

Each query and mutation selects todos with `[schema todo ?e]`, so a mutation that
would leave a todo off-schema is refused with `schema_violation`, a 500. The
page only sends booleans, so only a hand-made request gets one.

Plain patches aren't schema-checked, and two writes have to be patches:
adding a todo, because a mutation can't create an entity, and deleting, because
a mutation can't delete one. `serve.nu` checks the text before adding, and
checks the id is a todo (`todos/get`) before deleting.

## Notes

- A viewer that disconnects keeps its subscription until the next commit,
  which ends it. See the plugin README's known limitations.
- http-nu interrupts a disconnected viewer's request and logs it as an error
  (`Operation interrupted`). Other viewers are unaffected.
