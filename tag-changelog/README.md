# Per-tag changelogs

One file per released tag, each listing only what that tag added over the
previous one. `../CHANGELOG.md` is the other half of the pair and stays
grouped by area rather than by release: it answers "what can this thing
do", these answer "what changed when".

Entries come from the conventional-commit subjects in the tag's own range
(`git log <previous>..<tag> --no-merges`), grouped by type and with the
`chore`/`test` noise that never reaches a user left out.

Written technical, for whoever has to work on this: name the module, the
function, the route, the storage prefix, the constant and its value. State
what broke and what now guards it. Skip the narrative — a bullet that reads
like an announcement is one nobody can act on.

Facts are read from the tree *at that tag*, so a thing renamed later keeps
the name it had then. A shipped file's facts don't change; its wording may
be rewritten, and was once, when this style replaced the prose one.
