# Per-tag changelogs

One file per released tag, each listing only what that tag added over the
previous one. `../CHANGELOG.md` is the other half of the pair and stays
grouped by area rather than by release: it answers "what can this thing
do", these answer "what changed when".

Entries come from the conventional-commit subjects in the tag's own range
(`git log <previous>..<tag> --no-merges`), grouped by type and with the
`chore`/`test` noise that never reaches a user left out. A tag's file is
written when the tag is cut, and never edited afterwards — a shipped
release's notes are a record, not a document. The exception is the file for
a version not yet tagged, which is a draft until the tag exists.
