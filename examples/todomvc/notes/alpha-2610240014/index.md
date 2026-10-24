---
description: "integers and floats are separate types. the C ABI is unchanged."
---
# stardust alpha-2610240014

- [integers and floats](#numbers) are separate types in queries and mutations
- `stardust.h` is the same as on [alpha-2610230456](../alpha-2610230456/)
- [thinking about an async abi](../thinking-about-async-abi/): unchanged on this release

## integers and floats {#numbers}

queries and mutations now tell integers from floats:

- arithmetic and ordered comparisons that mix the two fail. convert one side with `toInt` or `toFloat`.
- integer division returns an integer.
- `abs`, `incr` and `decr` fail on integer overflow. `incrWrap` and `decrWrap` wrap around instead.
- `min`, `max` and `clamp` take one numeric type, and fail on non-finite numbers.

todomvc doesn't do arithmetic in its queries or mutations, so none of these change it.

## upgrading the plugin {#upgrade}

the plugin was on alpha-2610212237. one line changed: `page_size` in the run options is an `i64` now, not a `u32`. that came with alpha-2610230456, which made every number in the C ABI an `int64_t` or a `double`.

the smoke test passes, and so do these todomvc requests, on a copy of a local database:

- add, toggle, toggle all, delete and clear completed
- the same toggle sent twice
- time travel to an old transaction
