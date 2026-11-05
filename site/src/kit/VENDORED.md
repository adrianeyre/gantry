# The Stageshow kit, vendored

`site/src/kit/` is the React kit from the [Stageshow skill](https://github.com/benjitaylor/skills/tree/main/stageshow)
(v1.1.1, commit `299140fafd08df55a9c234e610c3f64af3f45531`), MIT licensed (see `LICENSE`, © 2026 Benji Taylor).

It is copied as it ships, with two changes: `brands/` holds an empty registry
instead of the bundled payment, bank and app artwork, which the Gantry website
never shows, and `phone.tsx` drops an unused import our `noUnusedLocals` rejects.
Update it by copying `kit/` from a newer release over this folder
and emptying `brands/index.ts` again.
