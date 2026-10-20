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
  and again after every commit that changes it. Each result renders the
  `#todos-section` and is sent as a `datastar-patch-elements` event.

```
browser --POST--> route --stardust patch--> todos.db
                                               | stardust subscribe
browser <--SSE--- .mj render <--result---------+
```

The metrics panel shows the transaction that produced the current result and
the subscription's event count.

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
- `GET /sse` - SSE stream of the todo section
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
- `queries todos/get`: one todo, by `?id`. Delete checks it.
- `queries todos/completed`: the completed todos. Clear-completed reads it.
- `mutations todos/set`: sets one todo's `completed`. `no_match` means the id
  isn't a todo.
- `mutations todos/set-all`: sets `completed` on every todo that differs.

Each definition selects todos with `[schema todo ?e]`, so a mutation that
would leave a todo off-schema is refused (`schema_violation`, a 500: the page only sends booleans). Plain
patches aren't schema-checked: adding a todo is a patch, since a mutation
can't create an entity, and neither can delete one, so deletes are patches
too.

## Notes

- A viewer that disconnects keeps its subscription until the next commit,
  which ends it. See the plugin README's known limitations.
- http-nu interrupts a disconnected viewer's request and logs it as an error
  (`Operation interrupted`). Other viewers are unaffected.
