# Documentation index: openpos

Offline-first point of sale for small retail. AGPL-3.0-only, free self-host, paid managed cloud.

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
| Guide | What it is for, the commands, the settings, what will surprise you |

## Documents

| Doc | Category | Status | Purpose |
|---|---|---|---|
| [06092026_openpos_feature_spec.md](06092026_openpos_feature_spec.md) | Spec | Approved design | v1 feature inventory, scope, architecture decisions, sync protocol |
| [c4model.md](c4model.md) | Architecture | Current | Containers, components, data flows, decisions log |
| [running.md](running.md) | Guide | Current | Starting the server, the till and the back office; settings; tests; reaching the failure paths |
| [languages.md](languages.md) | Guide | Current | Where the words live, which languages a shop offers its staff, how a refusal carries a code and its figures, and what is still English |
| [13092026_tax_invoice.md](13092026_tax_invoice.md) | Guide | Current | What the VAT and Supplementary Duty Act, 2012 says section by section, what this product does about each, and what is not claimed |
| [07092026_voice_lookup_plan.md](07092026_voice_lookup_plan.md) | Plan | Phases 0-3 done, working in a browser | Speaking an item onto a ticket in Bangla, offline; what is refused rather than guessed at |
| [27092026_licence_and_ci_plan.md](27092026_licence_and_ci_plan.md) | Plan | Done | The canonical AGPL text in `LICENSE`, the README brought in line with the compose file, and CI |

## Applications

| Path | What it is |
|---|---|
| `apps/admin/` | The back office: shop details, people, prices, and codes that enrol more tills |
| `apps/till-web/` | The till screen: Svelte 5 and Vite, core in a worker on OPFS. See its README for what works and what does not |
| `demo/` | Two browser smoke pages that prove the core behaves in a browser as it does natively |

## Related work outside this repo

The first four live in the author's own workspace and are not published.

- `pos_evaluation_results.md`, a measured evaluation of Odoo, ERPNext + POS Awesome, NexoPOS and
  Chromis. The source of the cold-start-offline wedge.
- `pos_feature_matrix.md`, a feature extraction from the two leading systems, which seeded this
  spec's inventory.
- An adversarial architecture review.
- A review of the export and import work: six issues raised, four fixed and two documented as
  needing a larger change.
- The 2026-09-06 adversarial review of the whole implementation is tracked in `../todo.md` under
  "From the adversarial review", with each finding either ticked and covered by a test that fails
  without the fix, or left open with the reason.
