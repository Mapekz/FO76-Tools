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

## The only backlog

GitHub Issues is the single backlog for all three subprojects — there is no separate notes file. A considered non-decision (a deliberate scope exclusion, a carve-out kept out of the table it looks like it should be in) is recorded as a present-tense comment next to the code it constrains, not filed as an issue and not parked in a notes file.
