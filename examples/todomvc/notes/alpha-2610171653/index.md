---
# Each file beside this page, shown at the end of the section with that id.
code:
  leak.c: per-thread
  subcommit.c: schema-clause
---
# stardust alpha-2610171653

- [per-thread arena](#per-thread): 4 MB per thread, never freed. the plugin
  works around it with a fixed thread pool
- [schema clause leak](#schema-clause): about 0.25 MB per commit, never
  freed. fixed in [alpha-2610212237](../alpha-2610212237/#schema-clause)

## per-thread arena {#per-thread}

each os thread that calls into stardust gets its own 4 MB scratch arena,
reused by later calls on that thread. when the thread exits, its arena isn't
freed or given to the next thread, and there's no call to release it. so
memory grows with the number of distinct threads that ever called in, not
with the number of calls.

[`leak.c`](#leak-c) runs the same 400 queries two ways, each in a fresh
process:

- [`MODE=new`](#leak-c): each query on a new thread, joined before the next starts.
  one thread is alive at a time.
- [`MODE=pool`](#leak-c): on 4 threads that live for the whole run.

```
gcc -O2 -I$STARDUST/abi leak.c -L$STARDUST/lib/linux_amd64 -lstardust -lpthread \
  -Wl,-rpath,$STARDUST/lib/linux_amd64 -o leak
STARDUST_LICENSE=... MODE=new ./leak todos.db   # any db with a stored query at todos/list
```

resident size:

| queries | [`MODE=new`](#leak-c) | [`MODE=pool`](#leak-c) |
|---|---|---|
| start | 13 MB | 13 MB |
| 100 | 359 MB | 33 MB |
| 200 | 759 MB | 33 MB |
| 300 | 1159 MB | 33 MB |
| 400 | 1559 MB | 33 MB |

## schema clause leak {#schema-clause}

while a subscription to a query with a schema clause is open, each
commit that changes its result adds memory: about 0.25 MB per commit with
4000 matching entities. releasing the subscription doesn't free it, and a
new subscription doesn't reuse it. the same query without the schema clause
stays flat.

[`subcommit.c`](#subcommit-c) creates a database with a schema and 4000
items matching it. then, twice: it subscribes to the query below, makes 400
commits that each toggle one item, reading the subscription up to each
commit, and releases the subscription.

```
{find [?e ?text ?done] where [[schema item ?e] [?e text ?text] [?e done ?done]] orderBy [?e]}
```

[`SCHEMA=0`](#subcommit-c) runs the same query without its schema clause.
each run is a fresh process and a new database.

```
gcc -O2 -I$STARDUST/abi subcommit.c -L$STARDUST/lib/linux_amd64 -lstardust \
  -Wl,-rpath,$STARDUST/lib/linux_amd64 -o subcommit
STARDUST_LICENSE=... ./subcommit new.db
```

resident size:

| | with schema clause | [`SCHEMA=0`](#subcommit-c) |
|---|---|---|
| round 1: start | 16 MB | 16 MB |
| 100 commits | 50 MB | 25 MB |
| 200 commits | 74 MB | 26 MB |
| 300 commits | 101 MB | 27 MB |
| 400 commits | 126 MB | 31 MB |
| released | 126 MB | 31 MB |
| round 2: start | 125 MB | 29 MB |
| 100 commits | 152 MB | 31 MB |
| 200 commits | 177 MB | 31 MB |
| 300 commits | 203 MB | 31 MB |
| 400 commits | 230 MB | 33 MB |
| released | 230 MB | 33 MB |
