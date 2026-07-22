# Issue tracker: GitHub

Issues for FO76-Tools live in GitHub Issues ([`Mapekz/FO76-Tools`](https://github.com/Mapekz/FO76-Tools)), managed via the `gh` CLI.

## Conventions

- **Create an issue**: `gh issue create --title "..." --body "..."`. Use a heredoc for multi-line bodies.
- **Read an issue**: `gh issue view <number> --comments`, filtering comments by `jq` and also fetching labels.
- **List issues**: `gh issue list --state open --json number,title,body,labels,comments --jq '[.[] | {number, title, body, labels: [.labels[].name], comments: [.comments[].body]}]'` with appropriate `--label` and `--state` filters.
- **Comment on an issue**: `gh issue comment <number> --body "..."`
- **Apply / remove labels**: `gh issue edit <number> --add-label "..."` / `--remove-label "..."`. Note: `gh` does **not** auto-create missing labels — they must already exist in the repo (see `triage-labels.md`; the canonical set is already created).
- **Close**: `gh issue close <number> --comment "..."`

`gh` infers the repo from `git remote -v` automatically when run inside this clone.

## When a skill says "publish to the issue tracker"

Create a GitHub issue.

## When a skill says "fetch the relevant ticket"

Run `gh issue view <number> --comments`.

## Relationship to `todos.md`

`todos.md` at the repo root remains the hand-maintained, priority-ordered backlog grouped by subproject (`ba2/`, `esm/`, `esm-viewer/`) — it is **not** replaced or superseded by this convention. GitHub Issues is what these skills read from and write to when asked to file, triage, or close a *tracked issue*; `todos.md` is for the user's own informal, dated backlog notes. Don't auto-migrate entries between the two.
