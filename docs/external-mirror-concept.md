# Mirror externo (GitHub / GitLab / Codeberg): sync bidireccional de refs y pull requests

> **Track A y Track B implementadas completas (commits 1-12).** Entity+CRUD,
> plomería git, sync push, sync pull+divergencia, tick periódico, tab de UI
> (`webui::repo_settings::mirrors`), y — Track B — origin en `PrComment`,
> `ForgeApi` + clientes GitHub/GitLab/Codeberg, motor de sync de PRs
> (`mirror::external::pr_sync`) y su estado en la misma tab de UI, todo
> aterrizado. El código real difiere del plan original en varios puntos,
> anotados en las secciones correspondientes más abajo — no asumir que el
> texto viejo describe el estado actual sin chequear contra el código.
> Verificación manual contra cuentas reales de GitHub/GitLab/Codeberg sigue
> pendiente (ver sección de Verificación) — lo implementado está probado
> contra un `ForgeApi` fake in-memory y tests unitarios/de integración
> locales, nunca contra las APIs reales. La tab de UI en particular quedó
> más acotada que "formulario add/edit" sugería: edit inline cubre
> label/url/token/provider/direction/enabled/sync_pull_requests, pero no hay
> toggle JS mostrar/ocultar — el form de edit de cada mirror siempre está
> expandido en su propia fila.

## Contexto

`gitquarry` hoy solo habla consigo mismo (ssh/git-smart-HTTP) más un enlace
de federación unidireccional entre instancias gitquarry (`docs/federation-concept.md`).
Falta: mantener un repo sincronizado con una copia en un forge externo
(GitHub/GitLab/Codeberg) — para backup/visibilidad (push), para adoptar un
repo externo sin abandonarlo (pull), o ambas direcciones.

Distinto de federación: el otro lado no es una instancia gitquarry, no hay
`/_api/instance`, no hay modelo de entidad compartido — solo protocolo git
plano (fetch/push) que cualquier host ya habla, más (en la Track B) tres
APIs REST distintas para PRs/MRs.

Dos tracks, Track B construye sobre Track A y aterriza después de que
Track A esté verificada funcionando, no en paralelo. Cada fase es su propio
commit (o pocos), no un commit gigante al final.

## Track A: mirror de refs (branches/tags)

### Diseño ya acordado

- Protocolo git puro (fetch/push por https con token embebido en la URL,
  `https://x-access-token:<token>@host/...`) — sin SDK de proveedor.
  "Provider" (GitHub/GitLab/Codeberg/Other) es solo etiqueta de UI, cero
  ramificación de código.
- Tres direcciones por mirror: `PushOnly` / `PullOnly` / `Bidirectional`.
- Disparador: barrido periódico (no webhook entrante — queda para más
  adelante, se solapa con el ítem general de webhooks todavía abierto en
  el roadmap del forge).
- Política de conflicto: comparación de 3 vías por ref (sha local, sha
  externo, último sha sincronizado). Fast-forward gana automático en la
  dirección que se movió. Si ambos lados divergieron (ninguno es ancestro
  del otro) → **frenar el sync de ese ref, marcar "diverged" en la UI,
  nunca auto-mergear ni forzar ningún lado.**
- Prevención de loop: solo push/pull de un ref si su sha actual difiere
  del último sha sincronizado guardado para ese ref.
- Credencial guardada de forma reversible (string plano, no hash) — el
  token se reusa en cada sync, no puede ser hash unidireccional como
  `AccessToken::token_hash`. El storage layer ya cifra todo en reposo
  automáticamente, sin manejo manual de cripto.

### Entidad `ExternalMirror`

Nuevo submódulo `src/mirror/external/`, split `access.rs`/`domain.rs`
igual que `src/federation/access/peer_tokens.rs` + `src/federation/domain/peer_credential.rs`
(precedente más cercano para un secreto reversible reusable por target).

- Colección hand-built (no `#[derive(Entity)]` — mismo motivo que
  `peer_tokens.rs`/`storage/pull_requests.rs`: el path necesita el `Uid`
  del repo en runtime, y `#[entity(collection = "...")]` solo acepta
  literal de compile-time). `Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/mirrors"))`,
  igual patrón que `storage/pull_requests.rs:83`'s `pulls_collection`.
  Clave: id corto generado por mirror (un repo puede tener más de un
  mirror externo, incluso al mismo provider).
- `ExternalMirrorProvider`: `GitHub | GitLab | Codeberg | Other`.
- `SyncDirection`: `PushOnly | PullOnly | Bidirectional`.
- Campos: `remote_url: String` (sin token embebido, separado — misma
  separación que `Peer.git_base_url`/`PeerCredential.secret`),
  `token: String`, `provider`, `direction`, `enabled: bool`,
  `label: String`, `last_synced: BTreeMap<String, String>` (ref → último
  sha sincronizado — la memoria de la comparación de 3 vías),
  `ref_status: BTreeMap<String, RefSyncStatus>` (`InSync | Diverged | Error(String)`,
  UI-facing, separado de `last_synced` para que un ref divergido nunca
  pierda su último sha bueno conocido), `created_at`/`updated_at`.
- Validación (`validate_external_mirror`): `remote_url` reusa la lógica
  tipo `validate_instance_url` de `src/validate.rs`; token no vacío,
  ≤512 chars, sin caracteres de control (mismo shape que `peer_tokens.rs`).
- CRUD en `access.rs`: `set_external_mirror` (create con etag `None` al
  agregar, update CAS-etag al editar — necesita ambos paths, a diferencia
  del upsert incondicional de `set_peer_credential`, porque la UI necesita
  identidad estable por mirror a través de ediciones), `get_external_mirror`,
  `list_external_mirrors(uid)`, `delete_external_mirror`, y
  `update_last_synced(uid, mirror_id, ref_name, sha)` con su propio retry
  CAS (mismo shape que `pull_requests.rs`) para que dos refs del mismo
  mirror sincronizando en paralelo no se pisen.
- `domain.rs`: wrapper fino a nivel `AppContext` (`set`/`list`/`get`/`delete`),
  `set` corre `access::validate_external_mirror` antes de guardar —
  siguiendo el shape de `federation/domain/peer_credential.rs`.

### Plomería git nueva (`crates/gitquarry-core/src/git/mod.rs`) — implementada

- `push_to_remote_url(bare_repo: &Path, url: &str, refspecs: &[String]) -> Result<()>`
  — shell a `git push --end-of-options <url> <refspec>...`, misma
  disciplina de args no confiables que ya usa `fetch_remote_url`. **No**
  reusa `pack_objects_for_push` (esa arma un pack para el path de subida a
  S3 del pre-receive hook; `git push` hace su propia negociación de wire).
- `ls_remote_refs(url: &str) -> Result<HashMap<String, String>>` — `git
  ls-remote --end-of-options <url>`, parseado a mapa ref→sha.
- `list_refs_with_sha(git_dir: &Path) -> Result<HashMap<String, String>>` —
  **no estaba en el plan original**: `list_refs` (ya existente) solo da
  nombres, y la comparación de 3 vías necesita sha local por ref para
  comparar contra `ls_remote_refs`/`last_synced`. `for-each-ref --format=%(refname)%09%(objectname)`.
- `fetch_ref_from_remote_url(bare_repo: &Path, url: &str, ref_name: &str) -> Result<()>`
  — **no estaba en el plan original bajo ese nombre**: el "fetch acotado a
  ese ref" que la sección de orquestación ya mencionaba, ahora con firma
  concreta. `git fetch --end-of-options <url> <ref_name>` sin refspec de
  destino — solo trae objetos, nunca mueve un ref local por sí sola; el
  caller llama `update_ref` explícitamente después, ya con el sha que
  conoce de antemano vía `ls_remote_refs`.
- `redact_url(url: &str) -> String` — igual que lo planeado: quita el
  segmento userinfo (`://<user>:<token>@` → `://`) de una URL, escaneando
  cada ocurrencia de `://`. `push_to_remote_url`/`ls_remote_refs`/
  `fetch_ref_from_remote_url` redactan **en su propio `map_err`**, antes de
  que el error salga de `git/mod.rs` — no en el punto donde
  `mirror::external` lo guarda en `RefSyncStatus::Error`, como sugería la
  redacción original de esta sección. Un solo punto de redacción por
  función, no dos capas confiando la una en la otra.
- **Cambio de firma, no anticipado**: `is_ancestor(ancestor: &str, descendant: &str)`
  pasó a `is_ancestor(git_dir: &Path, ancestor: &str, descendant: &str)`.
  La versión vieja no tomaba directorio — corría `git merge-base` en el cwd
  heredado del proceso, que solo coincidía con el repo correcto porque su
  único caller (`mirror::hook::check_protected`, la comparación de
  fast-forward de branch protegida) es un *subproceso hook* que git ya
  invoca con cwd = `GIT_DIR`. `plan_ref_sync` (ver más abajo) necesita
  comparar shas de dos repos potencialmente distintos desde un proceso
  servidor normal, sin ese cwd implícito — así que se le agregó el
  parámetro, y `check_protected`/su único call site (`run_pre_receive_hook`)
  se actualizaron para pasar `GIT_DIR` explícito en vez de confiar en el cwd.

### Orquestación de sync (`crates/gitquarry-core/src/mirror/external.rs`) — implementada

Sibling de `gc.rs`/`hook.rs`. `sync_repo_mirrors(ctx, resolved)` itera los
mirrors habilitados de un repo con concurrencia acotada
(`MIRROR_SYNC_CONCURRENCY = 8`, mismo patrón que `DOWNLOAD_CONCURRENCY` en
`mirror/mod.rs`). `sync_mirror_once(ctx, resolved, mirror)` hace la
comparación de 3 vías por ref, delegada a una función pura(-ish)
`plan_ref_sync(bare_repo, local, remote, last_synced, direction) -> Result<Option<RefAction>>`
separada del propio bucle de I/O — mismo shape en espíritu que lo
planeado, con la lógica de decisión testeable por sí sola contra un repo
git real chico (`RefAction::{Record, Push, Pull, Diverged}`):

- refs locales vía `list_refs_with_sha`, refs remotos vía `ls_remote_refs`,
  comparados contra `mirror.last_synced[ref]`.
- local sin cambios, remoto se movió → `Pull` (si la dirección lo permite):
  `sync_mirror_once` hace `fetch_ref_from_remote_url` + `update_ref`
  explícito.
- remoto sin cambios, local se movió → `Push` (si la dirección lo permite):
  `push_to_remote_url` para ese ref.
- ambos se movieron (sin baseline conocido cuenta como "ambos se
  movieron" también), uno es ancestro del otro (`is_ancestor`) →
  fast-forward en la dirección que avanzó. **Detalle no anticipado**:
  antes de llamar `is_ancestor`/decidir un `Pull`, `sync_mirror_once`
  hace un `fetch_ref_from_remote_url` previo si el remoto cambió — los
  objetos del lado remoto tienen que existir en `bare_repo` antes de que
  `git merge-base --is-ancestor` pueda resolverlos, cosa que el plan
  original no señalaba explícitamente. Se salta a propósito cuando el
  único cambio fue del lado local (push puro) para no pagar una llamada de
  red en el caso más común.
- ambos se movieron, divergieron → `RefSyncStatus::Diverged`, se saltea,
  **nunca** fuerza ningún lado.
- éxito: `update_last_synced`. Error de git en un ref puntual:
  `RefSyncStatus::Error(e.to_string())` solo para ese ref (ya redactado,
  ver plomería git arriba), continúa — un ref/mirror malo nunca aborta el
  barrido completo (misma resiliencia log-and-continue de
  `federation::refresh_once`/`gc::sweep_once`).

### Índice nuevo para el tick periódico — no estaba en el plan original

`storage::settings::list_visible_repo_uids`/`list_owned_repo_uids` son los
únicos listados de repos que ya existían, y ambos son parciales (filtrados
por visibilidad o por owner) — no hay ningún "listar todos los repos del
bucket" genérico, y un mirror puede estar en un repo `Private`. Se agregó
`.conf/index/mirrors-enabled/<repo-uid>.json` (`access::set_mirrors_enabled_index`/
`list_repos_with_enabled_mirrors`), mismo patrón "hint, no fact" que
`storage::settings::visible_index`: `mirror::external::domain` lo recalcula
después de cada create/update/delete (releyendo los mirrors reales del
repo), y `run_periodic_sync`'s tick solo lo usa para saber qué repos
visitar, nunca como la fuente de verdad de si un mirror sigue habilitado —
eso lo re-chequea `sync_repo_mirrors` igual, filtrando por `enabled` de
nuevo.

### Tick periódico

`run_periodic_sync(ctx: Arc<AppContext>)` en `src/mirror/external.rs`,
mismo shape que `mirror/gc.rs`'s `run_periodic_sweep`/`sweep_once` y
`federation::run_periodic_refresh`/`refresh_once` (loop con
`tokio::time::interval`, lista todos los repos, log-and-continue por
ítem). Se spawnea en `src/serve.rs` (~línea 243-246) como un tercer
`tokio::spawn` detached, al lado de los spawns existentes de
`gc::run_periodic_sweep` y `federation::run_periodic_refresh` — nunca
dentro del `select!` de ssh/web, para que un fallo de sync no tumbe los
servidores.

### Tab de UI — implementada

`crates/gitquarry-core/src/webui/repo_settings/mirrors.rs`, registrada en
`repo_settings/mod.rs` junto a `general.rs`/`protected_branches.rs`/etc.
Vista lista: una fila por mirror (label, provider, dirección, pills de
estado por ref), botón "sync now" (llama `sync_repo_mirrors` inline —
bloquea el request, aceptado a propósito, ver el doc comment de
`mirrors::save_mirror`), formulario de edit inline por fila (label, url,
token, provider, dirección, enabled) + un formulario de add al final,
mismos campos. `federation/web/tokens.rs` (el precedente que este plan
citaba para "mostrar secreto una sola vez") ya no existe en este binario —
se movió a `gitquarry-federation` con el split. El patrón se reimplementó
igual pero contra `auth::web::security`'s `pending_totp_secret`, el
precedente vivo más cercano en `gitquarry-core`: `PageChrome::pending_mirror_token`
viaja `(mirror_id, token)` desde el handler POST al render, solo en la
respuesta de un `add_mirror`/`edit_mirror` que efectivamente rotó el
token, nunca re-derivado de storage en una carga posterior. Delete: POST
simple, sin confirmación tipeada (más liviano que `danger_zone.rs`, más
cerca de `topics.rs`), como estaba planeado.

**No anticipado**: el edit necesita el etag *que el browser vio al cargar
la fila* como guarda CAS, no uno recién refetcheado en el propio POST (eso
volvería el chequeo un no-op) — `access::list_external_mirrors` (usada por
el tick de sync, sin necesidad de etag) se separó de
`access::list_external_mirrors_with_etags`/`domain::list_with_etags`
(usada solo por la tab de UI), en vez de que la firma original creciera un
etag que el path de sync nunca usa.

## Track B: sync bidireccional de pull requests / MRs

Elegido explícitamente por el usuario (no default) sobre la opción más
angosta de solo-visibilidad: create/comment/merge se propagan en ambas
direcciones, no solo lectura. Feature bastante más grande que el mirror de
refs — tres APIs REST distintas (GitHub, GitLab, Codeberg/Gitea), sin
shape de wire compartido entre ellas, riesgo real de loops de
comentario/estado si el anti-loop bookkeeping no está bien. Se apoya
estrictamente sobre Track A: sync de PRs es opt-in por `ExternalMirror`
(`sync_pull_requests: bool`, default `false`) y reusa `remote_url`/`token`
de ese mismo mirror — sin credencial separada.

### Modelo de datos adicional

- `ExternalMirror` suma `sync_pull_requests: bool`.
- Nueva colección por mirror `ExternalPrLink`:
  `Collection::new_owned(format!("{REPOS_PREFIX}/{uid}/mirrors/{mirror_id}/pr-links"))`,
  keyed por número de PR local (scoped bajo un mirror puntual, no todo el
  repo — un repo puede tener varios `ExternalMirror`s, cada uno con PR
  sync independiente). Campos: `external_number: u64`,
  `synced_remote_comment_ids: BTreeSet<u64>` (filtro de import, ver
  abajo), `remote_updated_cursor: String` (el `updated_at` propio del
  provider, usado como parámetro `updated_since` para que cada tick no
  relistee todo).
- `storage::pull_requests::PrComment` suma un campo aditivo:
  `origin: Option<String>` (`#[serde(default)]`, id de mirror o `None`
  para comentario genuinamente local) — el filtro de export ("qué
  comentarios locales son míos para empujar afuera") es simplemente
  `origin.is_none()`; un comentario traído desde el lado externo queda
  estampado con el id del mirror para nunca reenviarse como si fuera
  nuevo.
- El `status` local de un PR sincronizado puede pasar a `Merged` de dos
  formas: vía `domain::pull_request::merge` propio de gitquarry (sin
  cambios), o observado como mergeado upstream durante un barrido de
  sync — este segundo caso necesita un `mark_merged_externally(ctx, target, number, merge_commit_sha)`
  nuevo y chico en `src/domain/pull_request.rs` que solo cambia los campos
  de status/`merged_at`/`merge_commit_sha` (CAS-write, mismo shape que ya
  usa `transition`), **nunca** llama al path real `merge-tree`/`commit-tree` —
  ese merge ya pasó del lado del provider y su commit va a llegar
  localmente vía el pull-mirror de Track A, no por acá.

### Clientes de provider

Nuevo `src/mirror/external/forge/` — un trait, tres implementaciones
finas, mismo patrón "un shape compartido, varios adaptadores finos" que ya
estableció `storage::pull_requests::PrDetail` para el proxy de PRs de
federación (`pull_requests.rs:185-199`):

- `mod.rs`: trait `ForgeApi` (`list_pull_requests(since: Option<&str>)`,
  `get_pull_request(number)`, `create_pull_request(new: NewRemotePr)`,
  `update_pull_request(number, patch: PrPatch)` — título/descripción/
  estado, `merge_pull_request(number)`, `list_comments(number)`,
  `add_comment(number, body)`), más los structs compartidos `RemotePr`/
  `RemoteComment`/`NewRemotePr`/`PrPatch` a los que cada impl mapea.
- `github.rs`, `gitlab.rs`, `codeberg.rs`: cada uno parsea `owner/repo`
  de `remote_url`, arma su propio header de auth (GitHub
  `Authorization: Bearer <token>`, GitLab `PRIVATE-TOKEN: <token>`,
  Codeberg/Gitea `Authorization: token <token>`), pega contra sus propios
  paths REST. Construcción de `reqwest::Client` sigue `federation/mod.rs:125-130`'s
  `http_client()` (builder con timeout) — reusa la misma dependencia
  (`reqwest = "0.13.4"`, rustls+json, `default-features = false`) agregada
  para federación capa 1, sin crate nueva.
- Se construye GitHub primero, se valida el shape del trait y el
  bookkeeping anti-loop contra una API real, y recién después GitLab/
  Codeberg como impls chicas — no las tres a la vez (regla de no-scope-
  especulativo de CLAUDE.md: validar la abstracción contra un target real
  antes de multiplicarla por tres).

### Bookkeeping anti-loop (la parte con más riesgo de romperse)

- **Comentarios hacia afuera**: al empujar comentarios locales al lado
  externo, solo son candidatos los que tienen `origin.is_none()` — un
  comentario que llegó *desde* ese mirror nunca se le reenvía.
- **Comentarios hacia adentro**: después de postear un comentario local
  vía `add_comment`, su id de comentario remoto devuelto se agrega
  inmediatamente a `synced_remote_comment_ids` — así el próximo
  `list_comments` del barrido lo reconoce como "ya conocido" y **no** lo
  reimporta localmente como un comentario duplicado. Garantía más fuerte
  que un cursor por conteo (un conteo se puede confundir con posts
  concurrentes entre dos barridos; un set de ids no).
- **Campos del PR (título/descripción/estado)**: last-write-wins por
  timestamp `updated_at` del provider comparado contra
  `remote_updated_cursor` — sin merge de 3 vías de campos de texto (a
  diferencia de la política de divergencia de refs de Track A, acá no hay
  "fast-forward" con sentido para una edición de título). Si ambos lados
  editaron desde el último sync, gana el más reciente y el otro se
  sobreescribe en silencio — marcado acá como la única garantía
  deliberadamente más débil de este plan; se muestra explícito en la UI
  del link ("in sync as of ...") para que no quede oculto.
- **Rate limits**: el cursor `updated_since` mantiene barato el
  `list_pull_requests` de cada tick (solo PRs tocados desde el último
  sync), mismo intervalo de tick que el barrido periódico de Track A — sin
  loop de polling separado más rápido para PRs.

### Descubrimiento de PRs nuevos/existentes

En cada barrido de mirror (solo si `sync_pull_requests` está prendido,
después del paso de sync de refs): lista PRs externos actualizados desde
el cursor → cualquiera sin `ExternalPrLink` genera un
`PullRequestMeta` local nuevo (`domain::pull_request::create`, reusada sin
cambios — su check de existencia de branch se satisface porque el sync de
refs de Track A ya bajó localmente las branches head/base) más una fila de
link nueva. Simétrico: cualquier PR local sin link, en un repo cuyo mirror
tiene `sync_pull_requests` prendido, se crea del lado externo vía
`create_pull_request` y se linkea. Un `merge()` local (función existente,
`src/domain/pull_request.rs:272-348`) — si el PR mergeado tiene link —
llama best-effort a `merge_pull_request` en el provider después, log-and-
warn si falla (mismo patrón "ya tuvo éxito local, la metadata es
secundaria" que esa función ya usa en su manejo de conflicto CAS al final,
~línea 339-345, no un patrón nuevo).

## Fase 0

Este mismo documento — antes de tocar `src/`.

## Archivos a tocar

**Track A:**
- `src/mirror/external/access.rs` (nuevo)
- `src/mirror/external/domain.rs` (nuevo)
- `src/mirror/external.rs` (nuevo — orquestación + tick periódico, `mod external;` en `src/mirror/mod.rs`)
- `src/git/mod.rs` (agrega `push_to_remote_url`, `ls_remote_refs`, `redact_url`)
- `src/serve.rs` (~línea 243: un spawn más)
- `src/webui/repo_settings/mirrors.rs` (nuevo)
- `src/webui/repo_settings/mod.rs` (registra el tab nuevo)

**Track B:**
- `src/mirror/external/forge/mod.rs`, `github.rs`, `gitlab.rs`, `codeberg.rs` (nuevos)
- `src/mirror/external/access.rs` (CRUD de `ExternalPrLink`, extiende `ExternalMirror` con `sync_pull_requests`)
- `src/storage/pull_requests.rs` (agrega `PrComment.origin`)
- `src/domain/pull_request.rs` (agrega `mark_merged_externally`; `merge()` gana la llamada best-effort a merge externo)
- `src/mirror/external.rs` (`sync_mirror_once` gana el paso de sync de PRs)
- `src/webui/repo_settings/mirrors.rs` (checkbox de PR-sync + estado de link por PR)

## Verificación

Sigue la convención existente del codebase (tests de federación,
`mirror/mod.rs`) de sockets/git reales, no mocks:

- **Test push-mirror**: levanta una instancia `webui::run_server` sirviendo
  un repo sobre su propio git-http; crea un `ExternalMirror` apuntando a la
  URL de esa misma instancia para un segundo repo (truco "remoto real sin
  segundo proceso" ya usado en `mirror/mod.rs` y `federation/domain/fork.rs`),
  con un token que la auth de esa instancia acepte; push local; llama
  `sync_mirror_once`; verifica que el ref target existe con el mismo sha y
  `last_synced` se actualizó.
- **Test pull-mirror + divergencia**: siembra dos commits divergentes en el
  mismo ref (uno local, uno del lado "remoto") con `last_synced` en su
  ancestro común; llama `sync_mirror_once`; verifica que ningún lado se
  forzó y `ref_status` es `Diverged`. Un segundo caso donde solo el remoto
  se movió (fast-forward limpio) verifica que el ref local se actualiza y
  el estado queda `InSync`.
- `cargo fmt`, `cargo clippy --all-targets` limpio (`unsafe_code = forbid`),
  suite completa en verde.
- Click-through manual una vez que aterrice la fase de UI: agregar un
  mirror apuntando a un repo scratch de GitHub/Codeberg con un PAT real,
  push local, confirmar que aparece del lado externo; push externo, esperar
  un tick (o "sync now"), confirmar que llega local; forzar un commit
  divergente en ambos lados y confirmar que la UI muestra `Diverged` sin
  que ningún lado se sobreescriba en silencio.
- **Track B**: el trait `ForgeApi` permite mockear GitHub con una impl
  in-memory solo-para-test para la lógica anti-loop (filtro por set de ids
  de comentario, filtro por `origin`, comparación de cursor) — suficiente-
  mente pura para testear sin llamada real a la API. Manual (inevitable —
  cuentas reales de GitHub/GitLab/Codeberg, no hay fake local de sus APIs
  en este codebase): repo scratch + PAT en cada provider. Abrir un PR en
  gitquarry, confirmar que aparece en el provider; comentar en cada lado,
  confirmar que el otro lado lo muestra exactamente una vez (no duplicado,
  no en loop) después de dos ticks consecutivos; mergear local, confirmar
  que el PR del provider queda mergeado; abrir+mergear un PR del lado del
  provider, confirmar que el PR local pasa a `Merged` y el merge commit
  llega vía el pull-mirror de Track A, no un segundo merge commit local.

## Plan de commits (atómico)

0. [x] `docs: add external-mirror-concept design doc`

**Track A:**
1. [x] `feat(git): add push_to_remote_url and ls_remote_refs`
2. [x] `feat(mirror): add ExternalMirror entity and CRUD`
3. [x] `feat(mirror): sync push-mirror direction`
4. [x] `feat(mirror): sync pull-mirror direction with divergence detection`
5. [x] `feat(mirror): wire periodic external-mirror sync tick`
6. [x] `feat(web): add external mirrors settings tab`

**Track B (después de que Track A esté verificada funcionando):**
7. [x] `feat(storage): add origin field to PrComment`
8. [x] `feat(mirror): add ForgeApi trait and GitHub client`
9. [x] `feat(mirror): sync pull requests against a linked GitHub mirror`
10. [x] `feat(mirror): add GitLab forge client`
11. [x] `feat(mirror): add Codeberg forge client`
12. [x] `feat(web): surface PR-sync status on the mirrors settings tab`

**Track B implementada completa.** Verificación real contra cuentas GitHub/
GitLab/Codeberg sigue pendiente (inevitable, ver sección de Verificación
más abajo) — lo que aterrizó está probado contra un `ForgeApi` fake
in-memory (`mirror::external::pr_sync::tests::FakeForge`), no contra las
APIs reales. Desviaciones del plan original, encontradas durante la
implementación:

- **Gap real en el bookkeeping anti-loop saliente, no anticipado por el
  doc**: `origin.is_none()` solo. Un comentario genuinamente local nunca
  cambia de `origin` al exportarse, así que ese filtro solo no evita
  reexportarlo en cada tick siguiente. Se agregó
  `ExternalPrLink::synced_local_comment_keys: BTreeSet<String>`, poblado
  con la clave de storage de cada comentario ya exportado — para eso
  `storage::pull_requests::list_pr_comments_with_keys` es nuevo
  (`PrComment` nunca tuvo un id propio en el wire shape).
- **`domain::pull_request::create` no acepta un status inicial**: un PR
  externo descubierto ya-cerrado se crea `Open` y se cierra con una llamada
  a `close()` inmediata aparte (`pr_sync::create_local_pr_from_remote`); uno
  ya-mergeado se deja `Open` a propósito, porque el chequeo de propagación
  de merge que corre justo después no depende del cursor y dispara solo en
  el siguiente pase igual, vía el camino real `mark_merged_externally`.
- **GitLab**: la API base se deriva del host de `remote_url` (`/api/v4`),
  nunca hardcodeada a `gitlab.com` — GitLab es comúnmente self-hosted, a
  diferencia de GitHub. El número visible de una MR es `iid` (no el `id`
  global de GitLab). Un merge estrategia fast-forward/squash deja
  `merge_commit_sha` en `null` y reporta el sha real en `sha` — el mapeo
  cae a ese campo para que `mark_merged_externally` nunca reciba `None`.
  `list_comments` filtra "notes" con `system: true` (GitLab mezcla notas de
  sistema — "cambió la descripción", etc. — con comentarios reales en el
  mismo listado).
- **Codeberg/Gitea**: mismo patrón que GitLab de derivar la API base del
  host (`/api/v1`) — Codeberg es solo una instancia Gitea entre muchas
  self-hosteables. Shape de resto (`merged: bool` + `state`, comentarios
  como issue comments) igual a GitHub.
- **Ninguno de los tres clientes tiene un `since` real del lado servidor**
  para `list_pull_requests` — se pide todo ordenado por `updated_at`
  descendente y se corta client-side con `take_while`, no un query param
  del proveedor (cada API tiene semántica ligeramente distinta ahí, no vale
  la pena confiar en ninguna de las tres).
