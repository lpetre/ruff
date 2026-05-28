# ty use-def map: analysis of two flow-reference cases (dead-cst)

## TL;DR

Neither case is a bug in ty's use-def map, and I'd recommend **against** changing
its core semantics. In both cases the use-def map **already retains** the bindings
dead-cst needs. The divergence is that `bindings_at_use` is the type-checker's
*flow-sensitive* view, whereas dead-cst wants a *runtime-union* view. ty exposes
that second view through (a) a different API (`reachable_symbol_bindings`) and
(b) the consumer's choice of whether to evaluate each binding's reachability
constraint.

Both fixes are consumer-side (in dead-cst), built on ty's existing public API.

All line references are against the checkout at the time of writing
(`crates/ty_python_core`, `crates/ty_python_semantic`).

---

## The mechanism: two parallel binding stores

When ty records a binding, it writes into **two** parallel structures:

| View | Backing store | Shadowing | API |
| --- | --- | --- | --- |
| Flow-sensitive | per-place `Bindings` | `PreviousDefinitions::AreShadowed` — clears prior live bindings on a straight-line rebind (`use_def/place_state.rs:273-274`) | `bindings_at_use`, `end_of_scope_symbol_bindings` |
| Reachable union | `reachable_definitions_by_symbol` | `PreviousDefinitions::AreKept` — never shadows; union of every binding of the symbol in the scope (`use_def.rs:1386-1392`) | `reachable_symbol_bindings` (`use_def.rs:552-558`) |

Every binding in either view carries a `reachability_constraint`
(`use_def.rs:807, 817`). Whether a binding "counts" depends on whether the
consumer **evaluates** that constraint. ty's type-inference consumer,
`place_from_bindings_impl`, evaluates it and drops always-false bindings
(`place.rs:1420-1423`).

So the use-def map is *not* lossy here — it records both flow-sensitive and
union views, and the narrowing is applied at consumption time, not build time.

---

## Case A — sibling submodule imports sharing a root binding

```python
import a.foo   # binds `a`  (def A)
import a.bar   # binds `a`  (def B)  -> AreShadowed clears A from flow state
a.foo.x()      # bindings_at_use(a) = {B} only
a.bar.z()
```

### Why this is correct flow semantics

The local *name* `a` is last-write-wins. ty resolves `a.foo` as a submodule
attribute on module `a` regardless of which `import` statement bound the name.
The submodule side-effect — `import a.foo` also makes the `a.foo` submodule
importable/attribute-accessible — is an import-system fact that is orthogonal to
the name binding, so ty has no reason to model it on the `a` use-chain.

### The info is not lost

`reachable_symbol_bindings(a)` returns **both** A and B (both `AreKept`, both with
`ALWAYS_TRUE` reachability in straight-line code).

### Fix (dead-cst)

For an attribute-chain use, pull `reachable_symbol_bindings` for the root symbol
and emit the alias edge to the import whose dotted submodule path matches the
chain prefix (`a.foo.x` → `.foo` → def A; `a.bar.z` → `.bar` → def B). The
path-match also prevents over-inclusion: a genuinely-unused `import a.baz` won't
match any chain prefix, so it correctly keeps zero in-edges.

---

## Case B — `if TYPE_CHECKING / else` drops the live (runtime) branch

```python
from typing import TYPE_CHECKING
if TYPE_CHECKING:
    from a import SomeClass   # def A, reachability "TYPE_CHECKING is truthy"
else:
    from b import SomeClass   # def B, reachability "TYPE_CHECKING is falsy"
SomeClass()
```

### No build-time pruning happens

The `if` visitor (`builder.rs:2792-2884`):

1. records the if-body under reachability `TYPE_CHECKING is truthy`;
2. restores the falsy snapshot and records the else-body under
   `TYPE_CHECKING is falsy` (`record_negated_reachability_constraint`, line 2839);
3. `flow_merge`s all branch snapshots.

`in_type_checking_block` only tags ranges for the `is_range_in_type_checking_block`
query — it does **not** drop bindings.

### Consequence

`bindings_at_use(SomeClass())` **already yields both** def A and def B (distinct
def ids, kept via the `Left`/`Right` arms of the merge, `place_state.rs:353`).

The else branch disappears only when a consumer **evaluates** its reachability
constraint to always-false — exactly what `place_from_bindings_impl` does at
`place.rs:1423`, because ty narrows `TYPE_CHECKING` to `True` (by design; matches
mypy/pyright).

This also explains the control: with a runtime guard (`if random() > 0.5`) the
predicate evaluates to `Ambiguous`, not `AlwaysFalse`, so the existing filter
keeps both branches.

### Fix (dead-cst)

Iterate the raw bindings and **do not** apply the static-reachability filter
(i.e. don't mirror the `evaluate().is_always_false()` drop). Equivalently, use
`reachable_symbol_bindings`. Both branches then contribute alias + upstream edges.

---

## Recommendation on the ty side

Do **not** change `bindings_at_use` or the build-time recording — flow-sensitivity
and reachability narrowing are load-bearing for the type checker, and both cases
are already recoverable from the existing public API:

- **Case A** → `reachable_symbol_bindings` + submodule-path match.
- **Case B** → consume bindings without the static-reachability filter.

### Caveat for `reachable_*`

`reachable_symbol_bindings` uses `BoundnessAnalysis::AssumeBound` (`use_def.rs:557`)
and is **scope-wide, not use-position-sensitive** — it includes bindings that
appear after the use or in sibling branches. For dead-cst's "never delete a needed
import" invariant this is the *safe* direction (conservative over-keeping), and the
path-match keeps Case A precise.

### Optional ty-side ergonomics (not required)

If dead-cst wants a single reusable entry point, a thin documented helper such as
`runtime_union_bindings(use_id)` could live either in dead-cst or as a convenience
method on `UseDefMap`. It adds no new information — it just packages "union the
reachable bindings and skip the static-reachability filter." My default would be to
keep it in dead-cst unless ty itself grows a second consumer with the same need.

---

## Key references

- `crates/ty_python_core/src/use_def.rs`
  - `bindings_at_use` — `:434`
  - `reachable_symbol_bindings` / `reachable_bindings` — `:542-558`
  - `end_of_scope_symbol_bindings` — `:521`
  - `BindingWithConstraints { binding, narrowing_constraint, reachability_constraint }` — `:814-818`
  - `record_declaration_and_binding` writing both stores (`AreKept` for reachable) — `:1355-1393`
- `crates/ty_python_core/src/use_def/place_state.rs`
  - `PreviousDefinitions` (`AreShadowed` / `AreKept`) — `:88-98`
  - `Bindings::record_binding` (shadow clears live set) — `:258-281`
  - `Bindings::merge` (keeps distinct defs from both branches) — `:312-357`
- `crates/ty_python_core/src/builder.rs`
  - `Stmt::If` visitor (records both branches, no build-time pruning) — `:2792-2884`
- `crates/ty_python_semantic/src/place.rs`
  - `place_from_bindings_impl` (evaluates + drops always-false bindings) — `:1361`, filter at `:1420-1423`
