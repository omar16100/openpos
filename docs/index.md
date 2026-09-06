# Documentation index: openpos

Offline-first point of sale for small retail. AGPL-3.0, free self-host, paid managed cloud.

## Conventions

- Dated docs: `DDMMYYYY_topic.md`. Evergreen docs: `topic.md`.
- Every doc opens with purpose, status and last-updated date.
- `c4model.md` is the architecture source of truth. Read it before any architecture change and
  update it for every change to containers, components, services, dependencies or data flows.

## Categories

| Category | Required sections |
|---|---|
| Spec | What it is, scope, feature inventory, architecture decisions, data model, error handling, testing, open questions |
| Architecture | C4 levels, containers, components, data flows, decisions log |
| Plan | Context, phases, status log, deviations |

## Documents

| Doc | Category | Status | Purpose |
|---|---|---|---|
| [06092026_openpos_feature_spec.md](06092026_openpos_feature_spec.md) | Spec | Approved design | v1 feature inventory, scope, architecture decisions, sync protocol |
| [c4model.md](c4model.md) | Architecture | Current | Containers, components, data flows, decisions log |

## Applications

| Path | What it is |
|---|---|
| `apps/till-web/` | The till screen: Svelte 5 and Vite, core in a worker on OPFS. See its README for what works and what does not |
| `demo/` | Two browser smoke pages that prove the core behaves in a browser as it does natively |

## Related work outside this repo

- `/Users/macmini/projects/pos-eval/docs/pos_evaluation_results.md` measured evaluation of Odoo,
  ERPNext + POS Awesome, NexoPOS and Chromis. The source of the cold-start-offline wedge.
- `/Users/macmini/projects/pos-eval/docs/pos_feature_matrix.md` feature extraction from the two
  leading systems, which seeded this spec's inventory.
- `/Users/macmini/projects/codex/openpos_architecture_review.txt` adversarial architecture review.
- `/Users/macmini/projects/codex/openpos_export_import_review.txt` review of the export and import
  work, six issues raised, four fixed and two documented as needing a larger change.
- The 2026-09-06 adversarial review of the whole implementation is tracked in `../todo.md` under
  "From the adversarial review", with each finding either ticked and covered by a test that fails
  without the fix, or left open with the reason.
