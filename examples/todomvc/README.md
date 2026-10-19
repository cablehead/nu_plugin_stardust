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
- **Reads.** Each viewer opens `GET /todos/updates`, an SSE stream of
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
- `GET /todos/updates` - SSE stream of the todo section
- `POST /todos/add` - add a todo (Datastar signals `{"input": "text"}`)
- `POST /todos/{id}/toggle` - toggle a todo
- `POST /todos/toggle-all` - complete all, or reopen all if all are done
- `DELETE /todos/{id}` - delete a todo
- `DELETE /todos/completed` - clear completed todos

`{id}` is the todo's Stardust entity id. Toggle and delete answer 404 for an
id that isn't a todo.

## Stored definitions

`serve.nu` defines these at startup:

- `queries todos/list`: every todo, oldest first. Viewers subscribe to it.
- `queries todos/completed`: one todo's flag, by `?id`. Toggle reads it.
- `mutations todos/set-all`: sets `completed ?done` on every todo that
  differs. Toggle-all runs it.

Mutation patches take literal values and query variables, not expressions,
and can't delete entities. So toggle reads the flag then patches its
negation, and clear-completed queries the ids then patches each to `null`.

## Notes

- A viewer that disconnects keeps its subscription until the next commit,
  which ends it. See the plugin README's known limitations.
- http-nu interrupts a disconnected viewer's request and logs it as an error
  (`Operation interrupted`). Other viewers are unaffected.
