---
description: "per-thread leak: about 4 MB per thread, never freed. schema clause leak: about 0.25 MB per commit, never freed."
# Each file beside this page, shown at the end of the section with that id.
code:
  leak.c: per-thread
  subcommit.c: schema-clause
---
# stardust alpha-2610171653

- [per-thread leak](#per-thread): about 4 MB per thread, never freed. the
  plugin uses a fixed thread pool
- [schema clause leak](#schema-clause): about 0.25 MB per commit, never
  freed. fixed in [alpha-2610212237](../alpha-2610212237/#schema-clause)

## per-thread leak {#per-thread}

memory grows with every new thread that calls stardust, and isn't freed when
the thread exits. a fixed pool of threads stays flat.

[`leak.c`](#leak-c) creates a new, empty database and runs one inline query
400 times: on a new thread each ([`MODE=new`](#leak-c)), or on 4 reused
threads ([`MODE=pool`](#leak-c)).

```
gcc -O2 -I$STARDUST/abi leak.c -L$STARDUST/lib/linux_amd64 -lstardust -lpthread \
  -Wl,-rpath,$STARDUST/lib/linux_amd64 -o leak
STARDUST_LICENSE=... MODE=new ./leak new.db
```

resident size:

| queries | a new thread each | 4 reused threads |
|---|---|---|
| start | 15 MB | 15 MB |
| 100 | 373 MB | 48 MB |
| 200 | 781 MB | 48 MB |
| 300 | 1189 MB | 48 MB |
| 400 | 1597 MB | 48 MB |

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
