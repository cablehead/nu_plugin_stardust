---
description: "per-thread arena: open, 4 MB to 0.3 MB per thread. schema clause leak: fixed."
# The repro code, from the release where it was written, shown at the end of
# the section with that id.
code_from: ../alpha-2610171653
code:
  leak.c: per-thread
  subcommit.c: schema-clause
---
# stardust alpha-2610212237

- [per-thread arena](#per-thread): open, 4 MB to 0.3 MB per thread
- [schema clause leak](#schema-clause): fixed
- an asOf parameter can't default to null: [`todos/list-at`](https://github.com/cablehead/nu_plugin_stardust/blob/f5e0c7a/examples/todomvc/serve.nu#L92) defaults ?at to 1
- [`reactors sync`](https://github.com/cablehead/nu_plugin_stardust/commit/e168e50) is gone

## open {#open}

### per-thread arena {#per-thread}

each thread that calls stardust leaves memory behind that's never freed. on
this release it's about 0.3 MB per thread, down from 4 MB. a fixed pool of
threads stays flat.

| queries | now | before | now, pool |
|---|---|---|---|
| start | 17 MB | 13 MB | 17 MB |
| 100 | 47 MB | 359 MB | 37 MB |
| 200 | 80 MB | 759 MB | 37 MB |
| 300 | 109 MB | 1159 MB | 37 MB |
| 400 | 135 MB | 1559 MB | 37 MB |

## fixed {#fixed}

### schema clause leak {#schema-clause}

a subscription to a query with a schema clause leaked memory on every
commit, and kept it after the subscription was released. on this release it
stays flat.

| | now | before | now, no clause |
|---|---|---|---|
| round 1: start | **26 MB** | 16 MB | 26 MB |
| 100 commits | **34 MB** | 50 MB | 34 MB |
| 200 commits | **34 MB** | 74 MB | 34 MB |
| 300 commits | **34 MB** | 101 MB | 34 MB |
| 400 commits | **35 MB** | 126 MB | 35 MB |
| released | **35 MB** | 126 MB | 35 MB |
| round 2: start | **35 MB** | 125 MB | 35 MB |
| 100 commits | **35 MB** | 152 MB | 36 MB |
| 200 commits | **35 MB** | 177 MB | 37 MB |
| 300 commits | **36 MB** | 203 MB | 38 MB |
| 400 commits | **38 MB** | 230 MB | 39 MB |
| released | **38 MB** | 230 MB | 39 MB |
