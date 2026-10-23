---
description: "per-thread leak: fixed, flat at 19 MB over 400 threads."
---
# stardust alpha-2610230456

- [per-thread leak](#per-thread): fixed
- every number in the C ABI is now `int64_t` or `double`, including return codes and struct fields
- [thinking about an async abi](../thinking-about-async-abi/): unchanged on this release

## fixed {#fixed}

### per-thread leak {#per-thread}

each thread that called stardust left memory behind that was never freed:
4 MB per thread on alpha-2610171653, 0.4 MB on alpha-2610212237. on this
release a new thread for every query stays flat.

| queries | now | before | now, 4 reused threads |
|---|---|---|---|
| start | **15 MB** | 14 MB | 15 MB |
| 100 | **19 MB** | 51 MB | 34 MB |
| 200 | **19 MB** | 89 MB | 34 MB |
| 300 | **19 MB** | 127 MB | 34 MB |
| 400 | **19 MB** | 161 MB | 34 MB |

before is alpha-2610212237, run on the same machine.
