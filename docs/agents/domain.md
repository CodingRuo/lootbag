# Domain Docs

Use a single-context layout: root `GLOSSARY.md` and `docs/adr/`.

## Before exploring

Read `GLOSSARY.md` and ADRs relevant to the area being explored.
If they do not exist, proceed silently.

## Writing rules

Create the glossary lazily when the first domain term is resolved.
Keep it limited to domain definitions, without implementation details.
Use its canonical vocabulary consistently.

Create ADRs lazily for decisions that are hard to reverse, surprising
without context, and the result of a real trade-off.
Use sequential filenames: `docs/adr/0001-<slug>.md`.

Surface conflicts with existing ADRs explicitly.
