# stardust alpha-2610212237

the two leaks from [alpha-2610171653](../alpha-2610171653/), rerun on this
release with the same repros, unchanged: [`leak.c`](../alpha-2610171653/leak.c)
and [`subcommit.c`](../alpha-2610171653/subcommit.c).

## subscription with a schema clause: fixed

the changelog: "Live subscriptions to queries with a schema clause release
temporary memory between commits." with the clause, memory now stays flat,
the same as without it. this site's `todos/list` has its schema clause back.

resident size:

| | with schema clause | `SCHEMA=0` |
|---|---|---|
| round 1: start | 26 MB | 26 MB |
| 100 commits | 34 MB | 34 MB |
| 200 commits | 34 MB | 34 MB |
| 300 commits | 34 MB | 34 MB |
| 400 commits | 35 MB | 35 MB |
| released | 35 MB | 35 MB |
| round 2: start | 35 MB | 35 MB |
| 100 commits | 35 MB | 36 MB |
| 200 commits | 35 MB | 37 MB |
| 300 commits | 36 MB | 38 MB |
| 400 commits | 38 MB | 39 MB |
| released | 38 MB | 39 MB |

## per-thread memory: smaller, still never reclaimed

not in the changelog. a new thread per query now leaves about 0.3 MB behind
instead of 4 MB, and memory still grows with every new thread. the 4-thread
pool stays flat, so the plugin keeps its pool.

resident size:

| queries | `MODE=new` | `MODE=pool` |
|---|---|---|
| start | 17 MB | 17 MB |
| 100 | 47 MB | 37 MB |
| 200 | 80 MB | 37 MB |
| 300 | 109 MB | 37 MB |
| 400 | 135 MB | 37 MB |

## changes this site needed

- an `asOf` parameter can't default to `null` any more (`Invalid_Temporal`).
  `0` and `-1` are refused too; `1` is accepted. `todos/list-at` now
  defaults `?at` to 1; every caller passes it.
- `reactors sync` is gone: reactor changes now commit with the write that
  causes them. only the plugin's smoke test called it.
