# Issue tracker: Local Markdown

Issues and specs live as Markdown files in `.scratch/`.

## Conventions

- One feature per directory: `.scratch/<feature-slug>/`.
- Spec: `.scratch/<feature-slug>/spec.md`.
- Implementation tickets: `.scratch/<feature-slug>/issues/<NN>-<slug>.md`,
  numbered from `01`, one file per ticket.
- Record triage state as a `Status:` line near the top of each ticket.
  Use the role strings in `triage-labels.md`.
- Append comments and conversation history under `## Comments`.

## Completion

- Keep triage status separate from completion: implementation tickets use
  `State: open` or `State: closed`.
- Check acceptance criteria only when their behavior has been verified.
- Close a ticket after all its acceptance criteria pass, recording the
  implementation commit and verification evidence under `## Comments`.
- If source checks pass but native acceptance is pending, leave the ticket
  open and record the remaining gate explicitly.

## Skill operations

"Publish to the issue tracker" means writing the corresponding local file.
"Fetch the relevant ticket" means reading the referenced ticket file.
