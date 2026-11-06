# Split del binario: federacion pasa a un segundo binario (`gitquarry-federation`) con beacon discovery/join

> **Implementado.** Las 16 fases del plan de commits (seccion 10) aterrizaron
> — cutover real en `76fefdc` (borrado de `src/federation/`), doc viejo
> deprecado en `8ba1c31`. Workspace hoy: `crates/gitquarry-wire`,
> `crates/gitquarry-core`, `crates/gitquarry-federation` como members reales
> (ver `Cargo.toml`), `CLAUDE.md` ya describe el split de dos binarios como
> arquitectura vigente, no aspiracional. Este documento queda como historial
> de diseño — ver `git log` para el detalle de cada commit, no asumir que
> algo especifico de aca sigue pendiente sin verificar contra el codigo
> actual primero.

## Contexto

Objetivo de hardening: reducir la superficie de ataque/blast radius del
binario unico moviendo todo lo cross-instance (descubrimiento de peers,
browse/fork remoto, PR-proxy, tokens de grano fino) fuera de `gitquarry` a
un binario dedicado `gitquarry-federation`. El descubrimiento pasa de
polling admin-linked-only a gossip P2P (estilo beacon announce/join). Los
dos binarios se hablan solo por HTTP loopback. `gitquarry-core` sigue
siendo completamente usable sin federacion — federacion pasa a ser un
componente opcional, desplegado por separado, no parte del binario unico.

Distinto de `docs/external-mirror-concept.md` (ese es sync con forges
externos no-gitquarry via protocolo git plano) y complementario a
`docs/federation-concept.md` (ese documenta el diseño de federacion
*dentro* del binario unico, ya implementado — este documento es su
sucesor: mismas 4 capas, pero movidas a un proceso separado, mas gossip y
moderacion nueva).

## Estado actual (verificado en el codigo)

- Federacion vive entera en `src/federation/` (23 archivos) dentro del
  crate unico `gitquarry` — `mod.rs` (PeerCache, poll cada 15 min via
  `run_periodic_refresh`, spawneado en `src/serve.rs:246`), `domain/*`,
  `access/*`, `web/*` (handlers axum registrados en `src/webui/mod.rs`
  ~206-284), `entities.rs` (`Peer`, `AccessToken` como entidades s3db en
  `.conf/peers`, `.conf/tokens`), `i18n/*`.
- Acoplamiento que un split debe cruzar via IPC: auth
  (`auth::entities::{Permission,User}`, `access::effective_permission_for_token`),
  storage (`storage::Backend`, `storage::settings`, `storage::pull_requests`),
  git/mirrors (`AppContext::open_remote_mirror`, `git::browse::*`,
  `git::pack_all_objects`), webui `State`/`AppContext` (estado axum
  compartido in-process, sin frontera de API hoy), cripto (`Cipher`).
- `Cipher`, `Backend`, `build_backend` ya viven en `s3db`/`s3db-crypto`
  (`src/crypto.rs`, `src/storage/mod.rs` son re-exports finos) — el
  binario de federacion puede tener acceso independiente a bucket+cipher
  con **cero abstraccion nueva**.
- Workspace hoy: `[workspace] members = ["crates/s3db","crates/s3db-crypto","crates/s3db-macros"]`;
  el binario `gitquarry` es el paquete raiz, no un member — crate de
  binario unico genuino.
- No existe gossip/mDNS hoy; `/_api/instance` es descubrimiento por polling
  HTTP; `PeerCache` es pull-based, admin-linked, unidireccional (estilo
  Mastodon-follow).
- `docs/federation-concept.md` documenta la postura "el admin siempre
  consiente explicitamente el link, nunca un side-channel" — relevante
  porque el diseño de gossip de este documento relaja parcialmente esa
  postura (ver Capa de descubrimiento mas abajo).

## Decisiones de diseño ya resueltas

- **Descubrimiento**: gossip P2P — los binarios de federacion se
  intercambian listas de peers directamente (beacon announce/join), sin
  servidor de rendezvous central.
- **IPC**: HTTP sobre loopback (`127.0.0.1` solamente, nunca `0.0.0.0`)
  entre core y federacion, protegido por un secreto compartido
  (`internal_token`).
- **Provision de `internal_token`**: manual — el operador pone el mismo
  string en `gitquarry.toml` y `gitquarry-federation.toml`. Sin
  auto-generacion ni acoplamiento de filesystem entre las dos configs.
- **Rollout**: la seccion `[federation]` en `gitquarry.toml` es opcional.
  Ausente = las rutas/proxies/lookups de peer simplemente no se registran,
  core corre standalone sin crash y sin links muertos. Permite deploy
  incremental (core primero, federacion despues) en vez de un rollout
  atomico forzado.
- **Confianza de gossip / auto-link**: un candidato gossipeado se
  auto-linkea (pasa a ser un `Peer` real, browsable/forkable/searchable)
  una vez corroborado por **≥3 fuentes independientes ya-linkeadas** *y*
  conocido continuamente por **≥24h** (`first_seen` a `last_seen` en la
  tabla `GossipPeer`). Ambas condiciones requeridas — solo cantidad de
  fuentes es spammeable en rafaga, solo edad se satisface con una sola
  fuente goteando lento. Es una relajacion deliberada de la postura "el
  admin siempre linkea explicitamente" de `federation-concept.md` —
  marcado como cambio de postura de seguridad vivo, no un detalle de
  implementacion. Un peer auto-linkeado pasa igual por el mismo round-trip
  `verify_instance`/`/_api/instance` que un link manual antes de
  persistirse, y un peer baneado (ver Capa de moderacion) nunca es
  elegible para auto-link sin importar la corroboracion.
- **Permisos granulares por peer, rol moderador, ban, rate limiting**
  (agregado a pedido del usuario, motivado por hardening — ver Capa de
  moderacion): `Peer` gana un `access_level` granular en vez de
  linked/no-linked binario; un nuevo flag `is_federation_moderator` de
  instancia permite delegar gestion de peers sin dar `is_admin` completo;
  los peers se pueden banear con motivo obligatorio y siempre publico; el
  trafico entrante de federacion tiene rate limit.
- **Bans compartidos entre instancias hermanas**: dos instancias pueden
  configurarse como "hermanas" para que su lista de bans converja a ser
  identica en su totalidad (ver Capa de instancias hermanas).
- **Deprecacion, no migracion**: `src/federation/` (el modulo viejo dentro
  del binario unico) queda deprecado y se elimina — `gitquarry-federation`
  **no** mueve/adapta ese codigo archivo por archivo, se implementa de
  cero siguiendo el diseño de este documento. La seccion 2 (antes tabla de
  "destino" por archivo) describe que logica equivalente construye cada
  parte nueva, no un `git mv`. `docs/federation-concept.md` pasa a estar
  **deprecado**, superseded por este documento — agregar un banner ahi
  apuntando aca. El borrado real de `src/federation/*` en el binario unico
  es un cutover: se hace cuando `gitquarry-federation` ya reemplaza su
  funcionalidad y esta probado, nunca antes — no debe existir una ventana
  donde ninguno de los dos ofrezca federacion.

## 1. Layout de workspace

```
Cargo.toml                         # pasa a ser manifest de workspace virtual puro
crates/
  s3db/, s3db-crypto/, s3db-macros/    # sin cambios
  gitquarry-wire/                       # NUEVO — DTOs serde planos, sin logica
  gitquarry-core/                       # renombrado desde src/ raiz (el binario sigue llamandose `gitquarry`)
  gitquarry-federation/                 # NUEVO — binario de federacion
```

`gitquarry-wire` se mantiene deliberadamente minimo — solo tipos que
cruzan 2+ de las tres fronteras (core-internal, federation-publica,
peer-a-peer):

- `Uid` (movido desde `src/storage/uid.rs`, ya sin dependencias;
  `storage::uid::Uid` pasa a ser un re-export para que los call sites
  existentes no cambien).
- `InstanceInfo { name, version, total_bans_issued }` — unifica las dos
  definiciones casi-duplicadas de hoy; `total_bans_issued` es nuevo (ver
  Capa de moderacion, contador de accountability contra abuso de bans,
  siempre calculado en vivo desde la coleccion `Peer`, nunca un contador
  guardado que pueda desincronizarse).
- `RepoSummary { name, description, topics, star_count, fork_count }` —
  unifica `federation::PeerRepo` y `federation::web::api::RepoSummary` de
  hoy.
- `PullRequestMeta`, `PrComment`, `PrDetail` — movidos desde
  `storage::pull_requests`, re-exportados de vuelta para que los call
  sites existentes no cambien.

Explicitamente **no compartido**: `Permission`, `User`, `OwnerRef`,
`Peer`, `AccessToken`, `PeerOrigin` — cada uno queda single-sourced en el
binario que lo posee; el otro lado lo alcanza solo via las llamadas IPC
de la seccion 3.

## 2. `src/federation/*` viejo: que se elimina, que logica equivalente nace en `gitquarry-federation`

No es un `git mv`. Cada fila describe que reemplaza, en codigo nuevo, a
la pieza vieja que se borra — la implementacion puede (y probablemente
deba) diferir en detalle de la version actual, esta tabla solo fija la
responsabilidad, no el codigo fuente a copiar.

| Pieza vieja (se elimina) | Reemplazo nuevo |
|---|---|
| `federation/entities.rs` (`Peer`, `AccessToken`) | `gitquarry-federation/src/entities.rs` — mismo rol de datos (Peer/AccessToken como entidades s3db en `.conf/peers`, `.conf/tokens`), mas los campos nuevos de moderacion (seccion 5.5/5.6). Federacion es dueña de su propio `Backend`+`Cipher`. |
| `federation/access/peers.rs` | Access layer nueva en federacion, mas la tabla en memoria `GossipPeer` (Capa de descubrimiento) que no existia antes. |
| `federation/access/tokens.rs` (`resolve_by_token`, `effective_permission_for_token`) | Access layer nueva en federacion que resuelve `(username, AccessToken)` solamente — nunca un `User`/`Permission` completo. La autorizacion real se decide en core via `POST /_internal/authorize`; el viejo `effective_permission_for_token` no se porta, se reemplaza por esa llamada. |
| `federation/access/peer_tokens.rs`, `domain/peer_credential.rs` | Equivalente nuevo en federacion — mismo concepto (`PeerCredential` para llamadas peer-a-peer autenticadas), reusado ademas para el handshake de instancias hermanas (seccion 5.6). |
| `federation/domain/peer.rs`, `domain/token.rs` | Equivalente nuevo en federacion, sin cambios de fondo en el rol que cumplen. |
| `federation/domain/fork.rs` (`fork_from_peer`) | Se divide: la mecanica de git/storage (abrir mirror remoto, pack de objetos, `put_manifest`, bump de indices) nace en core como `gitquarry-core/src/domain/remote_fork.rs`, parametrizada por `(source_git_base_url, source_repo_name)` en vez de un tipo `Peer`. Federacion implementa un `domain::fork::request_fork` fino que valida el link y llama a `POST /_internal/repos/fork-from-remote` en core. |
| `federation/web/{peers,tokens,remote_pulls}.rs` | Handlers axum nuevos en `gitquarry-federation/src/web/*`, sobre un `FederationState` propio; donde el codigo viejo llamaba `ctx.resolve_repo`/`ctx.backend()` directo, el nuevo llama a la API interna de core. |
| `federation/web/api.rs` (`/_api/instance`, `/_api/repos*`) | Handler nuevo en el listener loopback de federacion; el router publico de core gana un proxy byte-passthrough para estos paths. |
| `federation/web/api_pulls.rs` | Handler nuevo en federacion; las llamadas pasan a `/_internal/pull-requests*` en core, con gate por el `/_internal/authorize` propio de federacion. |
| `federation/web/remote_repo.rs` (`/_remote/{peer}/{*repo}`) | **No se elimina** — es la unica pieza que se queda en core sin cambios de fondo (es pesado en browsing de git, re-implementar tree/blob/commit sobre IPC duplicaria logica de core sobre la red). Solo cambia como resuelve `{peer}`: una llamada de lookup a federacion (`GET /_internal-out/peer/{name}`) en vez de leer `.conf/peers` local. `open_remote_mirror` cambia su firma a `(peer_label: &str, git_base_url: &str)` para que core ya no necesite el tipo `Peer`. |
| `federation/i18n/*` | Copy nuevo (o adaptado) en federacion — bajo riesgo, texto de paginas, no logica. |
| `federation/mod.rs` (`PeerCache`, `run_periodic_refresh`) | Reemplazado enteramente por la tabla de gossip (Capa de descubrimiento) — no hay equivalente 1:1, el modelo de refresh cambia de raiz (poll pull-only cada 15 min → gossip P2P con tick propio). |

`docs/federation-concept.md` (el doc que describe la version vieja,
implementada) pasa a llevar un banner de deprecacion apuntando a este
documento como sucesor, en el mismo commit que borra `src/federation/*`.

## 3. IPC HTTP loopback — lista de endpoints

Ambos listeners bindean solo `127.0.0.1`. El puerto web publico de core
sigue siendo el **unico** puerto externamente alcanzable de todo el
sistema — el gossip y la superficie publica `/_api/*` de federacion se
alcanzan desde otras instancias pegandole al puerto publico de core, que
proxea al puerto loopback de federacion. No hace falta un segundo puerto
publico. Ambos lados protegen las llamadas loopback con el secreto
compartido `internal_token` via un middleware axum chico.

**A. Core → Federacion** (proxy de paginas/API, core valida
sesion+CSRF+admin-gate primero, reenvia con header `X-Gitquarry-User`):

- `GET/POST /_peers`, `GET/POST /_profile/{username}/tokens`,
  `POST /_remote_pulls/comment` (proxy con gate de sesion)
- `GET /_api/instance`, `GET /_api/repos[/{*name}]`,
  `GET/POST /_api/pulls*`, `GET /_api/banned-peers` (byte passthrough, sin
  auth salvo `/pulls*`, federacion se auto-autentica/rate-limita, ver
  Capa de moderacion)
- `POST /_federation/gossip/announce`, `GET /_federation/gossip/peers`
  (byte passthrough, Capa de descubrimiento)
- `POST /_federation/sister/handshake`, `POST /_federation/sister/ban-event`,
  `GET /_federation/sister/bans` (byte passthrough, autenticado con
  `PeerCredential` a nivel federacion, Capa de instancias hermanas — son
  llamadas instancia-a-instancia por internet publico como el resto de
  este grupo, no loopback-only; core solo proxea bytes)
- `/_remote/{peer}/{*repo}` se queda como ruta de core; core llama
  `GET /_internal-out/peer/{name}` en federacion → `{web_base_url, git_base_url} | 404`,
  y despues sirve el tree/blob/log el mismo.
- `GET /_internal-out/peers` → lista de peers linkeados, reemplaza
  `AppContext::peer_cache` para la seccion federada del index `/`.
- `GET /_internal-out/peer-repos` → `Vec<{peer_name, repos: Vec<RepoSummary>}>`,
  con fuente en el cache de gossip de federacion (reemplaza los datos de
  `run_periodic_refresh`).

**B. Federacion → Core**:

- `GET /_internal/repos/public` / `/_internal/repos/public/{*name}` /
  `/_internal/repos/by-uid?uid=` → `RepoSummary`, alimenta `/_api/repos*`.
- `POST /_internal/authorize {username, repo, require}` → `{ok: bool}` —
  envuelve `auth::access::effective_permission_for` + la logica de techo
  del lado servidor; federacion nunca recibe `Permission`/`User` por la
  red.
- `POST /_internal/repos/fork-from-remote {source_git_base_url, source_repo_name, dest_full_name, dest_owner_kind, dest_owner_uid, description, peer_label, acting_username}` →
  `{ok, repo_uid} | {ok:false, reason}` — envuelve la mecanica relocada de
  `fork_from_peer` + re-corre el chequeo propio de core de
  namespace/ownership.
- `GET /_internal/pull-requests/list?repo=`,
  `GET /_internal/pull-requests?repo=&number=`, `POST /_internal/pull-requests`,
  `POST /_internal/pull-requests/comment`, `POST /_internal/pull-requests/merge` —
  wrappers finos sobre `domain::pull_request::*` existente sin cambios;
  federacion protege cada uno con `/_internal/authorize` primero.

Nota de compatibilidad futura (no se construye ahora): una futura feature
de `git::http`/`git::ssh` aceptando un token de grano fino como credencial
git necesitaria una llamada mas,
`POST /_internal-out/tokens/resolve {secret, repo_uid}` → `{ok, username, ceiling}`.

## 4. Config

Archivo nuevo `gitquarry-federation.toml`, mismo orden de resolucion que
core (env var > cwd > git root > user global, reimplementado localmente
contra `s3db::BackendConfig`, sin dependencia del modulo de config de
`gitquarry-core`):

```toml
bucket = "s3"
passphrase = "..."                 # debe coincidir con la de core — mismo cipher, mismo namespace .conf/
listen = "127.0.0.1:8181"          # puerto loopback de federacion, core proxea aca
core_internal_url = "http://127.0.0.1:8090/_internal"
internal_token = "..."             # secreto compartido, manual, debe igualar el valor de core
seed_peers = ["https://peer1.example", "https://peer2.example"]   # bootstrap de gossip, opcional
gossip_interval = "5m"
auto_link_min_sources = 3          # peers ya-linkeados independientes que deben reportar un candidato
auto_link_min_age = "24h"          # span minimo first_seen→last_seen antes de ser elegible para auto-link
rate_limit_public_max = 60         # requests por ventana, /_api/* sin autenticar
rate_limit_public_window = "1m"
rate_limit_token_max = 120         # requests por ventana, llamadas autenticadas con bearer token
rate_limit_token_window = "1m"
```

Nueva seccion opcional en `gitquarry.toml`:

```toml
[federation]
internal_bind = "127.0.0.1:8090"
internal_token = "..."             # debe igualar el valor de federacion
federation_url = "http://127.0.0.1:8181"
```

`[federation]` ausente → core no registra rutas de proxy ni lookups de
`/_remote/*`, corre standalone (mismo patron "None significa apagado del
todo" que ya establece `oidc: Option<OidcClient>`).

`.conf/peers`, `.conf/tokens`, `.conf/peer-tokens` **se quedan en el mismo
bucket/namespace**, no se mueven — solo la propiedad del codigo pasa a
federacion.

## 5. Descubrimiento: gossip P2P / beacon discovery-join

Dos conceptos de peer, mantenidos separados:

- **`Peer`** (entidad s3db existente) — linkeado, browsable/forkable/searchable,
  sea agregado por el admin (flujo manual `verify_instance`, sin cambios)
  o auto-linkeado una vez corroborado (abajo).
- **`GossipPeer`** (nuevo, en memoria,
  `RwLock<HashMap<name, {web_base_url, git_base_url, first_seen, last_seen, reported_by: BTreeSet<String>}>>`,
  tope ~500 entradas, se descarta la mas vieja por `last_seen` primero) —
  catalogo de candidatos. `reported_by` trackea que peers ya-*linkeados*
  gossipearon este candidato, usado para el umbral de auto-link.

Join/bootstrap:

1. Al arrancar, por cada entrada de `seed_peers`, federacion hace `POST`
   a `/_federation/gossip/announce` (via el proxy de core del seed) con
   `{instance: {name, web_base_url}, known_peers: [...]}`.
2. El lado que recibe re-verifica independientemente al que anuncia (y
   cada candidato en su `known_peers`) via `/_api/instance` antes de
   agregarlo a su propia tabla `GossipPeer` — nunca confiar en una entrada
   de gossip sin verificar. Responde con su propia tabla
   (anti-entropy push-pull).
3. Cada `gossip_interval` se repite contra una muestra aleatoria de la
   tabla `GossipPeer` *actual* (no solo los seeds originales) — gossip de
   peer-sampling estandar, se propaga transitivamente.
4. `GET /_federation/gossip/peers` — variante solo-pull para un tick que
   no quiere tambien pushear.
5. **Chequeo de auto-link** corre en cada tick sobre entradas `GossipPeer`
   todavia no linkeadas: si `reported_by` (restringido a fuentes que son
   ellas mismas `Peer`s linkeados) tiene ≥`auto_link_min_sources` entradas
   Y `last_seen - first_seen ≥ auto_link_min_age`, se promueve a `Peer`
   real via el mismo camino `verify_instance` que usa un admin-add manual,
   y se loguea el evento de auto-link (visibilidad para el operador de una
   decision que paso sin accion explicita).

Esto reemplaza el tick de 15 min de `run_periodic_refresh`; el refresh de
lista de repos de los peers linkeados pasa a correr en el mismo tick de
gossip, acotado solo a `Peer`s linkeados (el gossip descubre candidatos,
nunca cambia directamente que es browsable/forkable).

## 6. Capa de moderacion: permisos por peer, rol moderador, ban, rate limiting

### Campos nuevos en `Peer` (`gitquarry-federation/src/entities.rs`)

```rust
access_level: PeerAccessLevel,       // BrowseOnly | BrowseAndFork | Full (default Full si lo linkeo el admin a mano, BrowseOnly si fue auto-linkeado)
banned: bool,                        // default false
ban_reason: Option<String>,          // obligatorio no-vacio siempre que banned == true; se valida en la accion de ban, no queda como Option suelto en otro lado
banned_at: Option<DateTime<Utc>>,
banned_by: String,                   // username del admin/moderador que baneo
rate_limit_override: Option<u32>,    // override de requests/ventana especifico para este peer; None = default de instancia
```

`PeerAccessLevel::BrowseOnly` solo permite navegar `/_remote/{peer}/*`.
`BrowseAndFork` suma `domain::fork::request_fork`. `Full` suma PR-proxy de
salida (`/_remote_pulls/*`). Se aplica donde se disparan estas acciones:
el handler `/_remote/{peer}/{*repo}` de core y `domain::fork`/`web::remote_pulls`
de federacion chequean el nivel devuelto por el lookup de peer
(`GET /_internal-out/peer/{name}` gana campos `access_level` y `banned`)
antes de proceder — un peer baneado se rechaza en este mismo chequeo sin
importar `access_level`, sin un camino de codigo separado que se pueda
bypassear.

Los peers auto-linkeados (seccion 5) arrancan en `BrowseOnly`, no `Full`
— corroborado-por-gossip es una señal de confianza mas debil que un
agregado explicito del admin, asi que el camino de auto-link solo otorga
la capacidad minima. Un moderador puede subirla despues a mano.

### Rol federation-moderator (lado core, `auth::entities::User`)

Se agrega `is_federation_moderator: bool` junto al `is_admin: bool`
existente (`src/auth/entities.rs:93-94`), mismo alcance global/de
instancia, misma forma de storage/migracion (campo nuevo,
`#[serde(default)]` para registros existentes). El toggle se expone en la
misma pagina admin-only de edicion de usuario que ya gestiona `is_admin`
— mismo patron, un checkbox mas, solo-admin para otorgarlo.

Cambio de gate: el chequeo `require_admin` que hoy protege `/_peers` y el
resto de rutas de proxy de federacion (seccion 3A) pasa a ser
`require_admin_or_federation_moderator` — `user.is_admin || user.is_federation_moderator`.
Este chequeo se queda enteramente del lado de core (core ya resuelve el
`User` de sesion antes de proxear, seccion 3A) — sin llamada IPC nueva,
federacion sigue sin necesitar nunca `User`/`Permission`. Las acciones de
ban/unban y edicion de access-level pasan por las mismas rutas proxeadas
de `/_peers`, asi que heredan este gate gratis — sin plomeria de permisos
separada para ban especificamente.

### Accion de ban

- Ruta POST nueva (proxeada como el resto de `/_peers`):
  `POST /_peers/{name}/ban {reason: String}` — rechaza `reason` vacio o
  solo-espacios en la validacion (mismo rigor de `validate.rs` que otros
  campos de texto libre). Setea `banned=true`, `ban_reason`, `banned_at`,
  `banned_by=acting_username`. Revoca de inmediato
  browsability/forkability/PR-proxy (el chequeo de `access_level` del
  parrafo anterior corta corto en `banned` primero).
- `POST /_peers/{name}/unban` — limpia `banned`/`ban_reason`/`banned_at`/`banned_by`.
  **No** restaura el `access_level` previo al ban automaticamente —
  unbanear devuelve un peer a `BrowseOnly`, requiriendo que un moderador
  otorgue conscientemente `Full`/`BrowseAndFork` de nuevo, para que un ban
  no se pueda deshacer en silencio con un solo click.
- **Visibilidad publica** (requerimiento explicito del usuario — el motivo
  de ban siempre es publico, no solo-admin): endpoint nuevo sin
  autenticacion en el grupo publico byte-passthrough existente (seccion 3A),
  `GET /_api/banned-peers` → `Vec<{peer_name, git_base_url, ban_reason, banned_at, banned_by}>`
  por cada `Peer` actualmente baneado. Incluye `banned_by` (no solo el
  motivo) para que sea visible *quien* tomo la decision, no solo *por que*
  — este es el mecanismo de transparencia, consistente con el "sin
  side-channel" que ya establece `federation-concept.md`, extendido aca a
  acciones de moderacion. No hay listado publico de *por que nunca se
  linkeo un peer* (eso es solo silencio, no un ban) — solo los bans
  efectivos son publicos.
- **Accountability contra abuso de ban** (requerimiento explicito del
  usuario — una instancia que banea peers para dañarlos debe tener ese
  patron visible en toda la red, para que otras instancias lo noten y
  restrinjan confianza en *ella* manualmente): la respuesta publica de
  descubrimiento existente, `GET /_api/instance` (`federation/web/api.rs`),
  gana un campo `total_bans_issued: u64` — conteo corriente de cada ban
  que esta instancia emitio, siempre calculable desde su propia coleccion
  `Peer` (`count where banned == true`, sin contador separado que
  desincronizar). Combinado con el detalle completo de `/_api/banned-peers`,
  cualquier admin de otra instancia evaluando si linkear/mantener el link
  con esta puede ver tanto el numero titular (detectar una rafaga de un
  vistazo durante descubrimiento rutinario, sin traer la lista completa)
  como los motivos/actores detallados detras. **Deliberadamente manual, no
  automatico**: este plan no agrega ningun descuento automatico de
  corroboracion de gossip, auto-unlink, o trust-scoring basado en
  `total_bans_issued` — es solo exposicion de datos. Un admin que juzga el
  historial de bans de un peer como abusivo lo deslinkea/restringe a mano
  via la UI existente de `/_peers`, misma accion que cualquier otra
  decision de moderacion manual. Automatizar esto se considero y se
  rechazo explicitamente para esta vuelta — una auto-restriccion basada en
  umbral seria en si misma abusable (inundar a una instancia con blancos
  de ban plausibles para disparar el umbral y lograr que la red la
  margine), un problema de diseño mas grande y separado que este split.
- El ban bloquea auto-link futuro directamente (chequeado en el tick de
  auto-link de la seccion 5: se saltea cualquier candidato `GossipPeer`
  cuyo nombre/host matchee un `Peer` baneado) y bloquea re-linkear via
  admin-add manual sin un `unban` explicito primero (el flujo de agregar
  muestra "previamente baneado: `<motivo>`" y requiere unban antes de
  poder crear un link nuevo, en vez de permitir un re-add instantaneo en
  silencio).

### Rate limiting (binario de federacion, solo entrante)

No hay crate de rate-limit en el workspace hoy (chequeado: sin
`governor`/`tower::limit`). Reusa la forma del `LoginAttempts` casero
existente (`src/app.rs:142-186` — `Mutex<HashMap<String, Vec<Instant>>>`,
en memoria, ventana deslizante, `is_locked`/`record_failure`/`clear`),
mismo tradeoff "simple, en memoria, se resetea al reiniciar", sin
dependencia nueva.

`PeerRateLimiter` nuevo en `gitquarry-federation`, dos buckets
independientes:

- **Publico/sin autenticar** (`/_api/instance`, `/_api/repos*`,
  `/_federation/gossip/*`): clave = IP de origen (axum `ConnectInfo`),
  limites desde `gitquarry-federation.toml`'s `rate_limit_public_max`/`rate_limit_public_window`.
- **Autenticado** (`/_api/pulls*` con bearer token): clave =
  `AccessToken.owner` resuelto (coincide con la atribucion por-usuario
  que `resolve_by_token` ya establece), `rate_limit_token_max`/`rate_limit_token_window`.
- `Peer.rate_limit_override` aplica solo cuando la IP del caller resuelve
  al host `git_base_url`/`web_base_url` conocido de un peer *linkeado* —
  marcado como **heuristica, no garantia de identidad criptografica** (la
  correlacion IP↔peer se puede spoofear compartiendo la IP de salida de
  ese peer; atribucion real por-peer necesitaria mTLS o requests
  firmados, explicitamente fuera de alcance para esta vuelta — ver
  pregunta abierta al final).
- Respuesta al pasarse del limite: `429`, misma postura log-and-continue
  del resto de tareas periodicas de federacion.

## 7. Capa de instancias hermanas: listas de ban compartidas

Requerimiento explicito del usuario: dos instancias "hermanas" pueden
compartir su lista de bans de forma que quede identica entre ambas en su
totalidad (no solo visible entre si — realmente sincronizada,
convergiendo al mismo conjunto, incluyendo bans/unbans futuros de
cualquiera de los dos lados).

- **Elevacion, no un tipo de link nuevo**: un peer solo puede pasar a ser
  hermana si ya es un `Peer` linkeado manualmente por el admin (los
  auto-linkeados via gossip, seccion 5, no son elegibles — el status de
  hermana implica confianza para emitir bans, una vara materialmente mas
  alta que acceso de browse/fork, asi que hereda la misma cautela
  "sin confianza unilateral" que hace que el auto-link arranque en
  `BrowseOnly`). Campos nuevos en `Peer`: `sister_requested: bool`,
  `sister_confirmed: bool`.
- **Consentimiento mutuo, no unilateral**: el admin togglea
  `sister_requested=true` de su lado; esto dispara
  `POST /_federation/sister/handshake` al peer (autenticado con el
  secreto `PeerCredential` existente, ya usado para llamadas
  peer-a-peer autenticadas, `federation/access/peer_tokens.rs`/`domain/peer_credential.rs`,
  movido sin cambios segun seccion 2 — reusado aca en vez de inventar un
  segundo mecanismo de intercambio de secreto). El admin del peer debe
  tambien setear `sister_requested=true` de su lado independientemente
  antes de que la respuesta del handshake de su instancia devuelva `ok`;
  recien ahi `sister_confirmed` pasa a `true` en ambos lados y el
  ban-sync se activa. Un solo lado nunca puede prender el ban-sync solo.
- **Mecanica de sync** — dos mecanismos, ambos necesarios para igualdad de
  conjunto real (un diseño solo-push deriva bajo downtime/requests
  perdidos):
  - Push por evento: cualquier ban/unban local llama de inmediato
    `POST /_federation/sister/ban-event {peer_name, git_base_url, action: "ban"|"unban", reason, banned_by, banned_at}`
    a cada peer `sister_confirmed`.
  - Reconciliacion periodica completa: en el mismo tick que el gossip (o
    un `sister_sync_interval` separado, default igual a `gossip_interval`),
    trae `GET /_federation/sister/bans` (lista completa de bans) de cada
    hermana y hace merge-union en ambas direcciones — atrapa lo que un
    push perdido se salteo, garantizando igualdad eventual completa en vez
    de forwarding best-effort.
  - Un blanco de ban desconocido localmente (la hermana baneo a alguien
    que esta instancia nunca linkeo) crea un `Peer` stub minimo
    solo-para-ban — `banned=true` mas campos de identidad solamente,
    `access_level` sin sentido ya que nunca fue linkeado para browse/fork.
    Estar baneado ya no implica haber sido linkeado antes.
- **La procedencia se mantiene honesta**: `banned_by` guarda el username
  del actor *original*, y un campo nuevo `synced_from_sister: Option<String>`
  (nombre de instancia de la hermana, `None` si esta instancia emitio el
  ban ella misma) permite que `/_api/banned-peers` distinga las
  decisiones propias de moderacion de una instancia de las que solo
  heredo.
- **Interaccion con el contador de accountability**: `total_bans_issued`
  en `/_api/instance` cuenta **solo bans donde `synced_from_sister.is_none()`**
  — es decir, decisiones originadas localmente. Contar tambien los
  sincronizados dejaria que las listas de ban identicas y espejadas de
  dos hermanas inflen el conteo visible de ambas instancias por lo que en
  realidad es una sola decision, socavando el proposito de "detectar una
  rafaga real de un vistazo" de ese campo. Este es el unico punto donde
  el diseño de sister-sync y el de accountability-contra-abuso
  interactuarian mal en silencio si no se marca explicitamente.

## 8. Cambios en `serve.rs` / `app.rs` / `webui/mod.rs` (lado core)

- `serve.rs`: se quita el spawn de `federation::run_periodic_refresh`. Si
  `[federation]` esta configurado, se spawnea un router axum chico para
  `/_internal/*` (seccion 3B) bindeado a `internal_bind`, y se registran
  las rutas de proxy de salida (seccion 3A) en el router publico.
- `app.rs`: se quita `peer_cache: federation::PeerCache` de `AppContext`;
  `open_remote_mirror` pasa a tomar `(peer_label: &str, git_base_url: &str)`.
  Se agrega `federation: Option<FederationLink>` (`federation_url`,
  `internal_token`, `reqwest::Client`).
- `webui/mod.rs`: se eliminan los registros de rutas `federation::web::*`,
  reemplazados por un handler de proxy generico parametrizado por prefijo
  de path. `/_remote/{peer}/{*repo}` se queda local, usa la unica llamada
  de lookup de peer. La seccion federada del index pasa a llamar
  `/_internal-out/peer-repos` en vez de leer `peer_cache`, se saltea
  entera si `ctx.federation.is_none()`.
- `search/web/mod.rs`: la rama de federacion (`list_synced_peer_repos`,
  `search_peer_metadata`) pasa a la misma llamada `/_internal-out/peer-repos`,
  con el mismo gate en `ctx.federation.is_some()`. La busqueda de mirrors
  ya-sincronizados `_remote/*/*` locales no se ve afectada.

## 9. Actualizaciones a `CLAUDE.md`

- Linea de arquitectura del inicio: "one binary" pasa a describir dos
  binarios — `gitquarry-core` (ssh/git-smart-HTTP/web, capaz de correr
  standalone) + `gitquarry-federation` (descubrimiento/gossip de peers,
  browse/fork remoto, PR-proxy, tokens de federacion), hablandose solo
  por HTTP loopback. Core corre completamente funcional sin federacion.
- Bullet nuevo en `## Config`: `gitquarry-federation.toml` es un archivo
  separado con la misma resolucion env>cwd>gitroot>userglobal que
  `gitquarry.toml`, nunca se mergean.
- Nota nueva en `## Storage`: `.conf/peers`, `.conf/tokens`,
  `.conf/peer-tokens` son propiedad del codigo de `gitquarry-federation`
  aunque ambos binarios comparten el bucket.
- Bullet nuevo `## IPC`: solo loopback, protegido por `internal_token`;
  los DTOs de wire compartidos viven en `crates/gitquarry-wire`, minimos
  (solo tipos de 2+ fronteras).
- **Aviso explicito, no escondido**: el auto-link de gossip (≥3 fuentes
  linkeadas independientes + ≥24h de corroboracion) es una relajacion
  deliberada de la postura previa "el admin siempre linkea explicitamente"
  de `docs/federation-concept.md` — actualizar el parrafo de postura de
  seguridad de ese doc para describir tanto el camino manual como el
  umbral/razon del auto-link.
- Nota nueva: los bans de peer siempre son publicos
  (`GET /_api/banned-peers`, sin auth) — una garantia de transparencia de
  moderacion, no solo un detalle de implementacion; el motivo de ban es
  obligatorio, nunca opcional en la practica.
- Nota nueva: `is_federation_moderator` es un flag global de `User`
  distinto de `is_admin` — otorga gestion de la pagina de federacion
  (link/unlink/ban/access-level) sin otorgar admin de instancia completo.
- Nota nueva: instancias hermanas sincronizan sus listas de ban completas
  via consentimiento mutuo + `PeerCredential`; `total_bans_issued` solo
  cuenta bans originados localmente, nunca los heredados via sync, para
  que el numero siga siendo una señal real de accountability.

## 10. Plan de commits sugerido (atomico, un concern por commit)

Nota: es deprecacion-y-reescritura, no migracion — `src/federation/` en
`gitquarry-core` sigue funcionando y sirviendo federacion hasta el commit
final de cutover (16). No hay ventana sin federacion disponible.

1. `refactor: extract crates/gitquarry-wire (Uid, InstanceInfo, RepoSummary, PR wire types)`
2. `feat(core): add loopback internal API server + /_internal/* endpoints`
3. `feat: scaffold crates/gitquarry-core, crates/gitquarry-federation workspace members` (mueve `src/` a `crates/gitquarry-core/src/` sin tocar `federation/` todavia; `gitquarry-federation` arranca como binario stub)
4. `feat(federation): implement entities/access/domain in gitquarry-federation` (codigo nuevo, no un mv — ver seccion 2)
5. `feat(federation): implement web handlers wired through core's internal API`
6. `feat(core): add federation proxy routes on the public router`
7. `feat(federation): implement gossip announce/peers endpoints + seed-peer bootstrap + auto-link threshold`
8. `feat(auth): add is_federation_moderator global user flag`
9. `feat(federation): add per-peer access_level and enforce at browse/fork/PR-proxy call sites`
10. `feat(federation): add peer ban/unban with mandatory reason + public /_api/banned-peers`
11. `feat(federation): add total_bans_issued to /_api/instance for ban-abuse accountability`
12. `feat(federation): add per-peer inbound rate limiting`
13. `feat(federation): add sister-instance mutual handshake and ban-list sync`
14. `refactor(core): open_remote_mirror takes (label, git_base_url) instead of federation::Peer; index/search federated sections read from federation's peer-repo API` (last core-side call sites still on old `federation::Peer` get switched over — cutover prep, `gitquarry-federation` verified functionally equivalent first)
15. `chore(core): remove src/federation/ and its wiring (serve.rs spawn, webui routes, AppContext.peer_cache)` — the actual delete, only once 3-14 are done and `gitquarry-federation` is proven to replace it
16. `docs: deprecate federation-concept.md in favor of federation-binary-split-concept.md, update CLAUDE.md for the core/federation binary split`

## Archivos criticos

- `src/federation/mod.rs`, `src/federation/domain/fork.rs`, `src/federation/access/tokens.rs`
- `src/app.rs`, `src/serve.rs`, `src/webui/mod.rs`
- `src/storage/uid.rs`, `src/storage/pull_requests.rs`
- `src/auth/entities.rs` (precedente de `is_admin` para el flag nuevo
  `is_federation_moderator`), `src/settings/web.rs` (precedente de
  `require_admin` para el gate admin-o-moderador nuevo)
- `Cargo.toml`, `CLAUDE.md`, `docs/federation-concept.md`

## Verificacion (cuando se implemente)

- `cargo build --workspace`, `cargo clippy --all-targets` limpio
  (`unsafe_code = forbid` se mantiene en ambos crates nuevos), `cargo fmt`
  segun convencion del proyecto.
- Tests de federacion existentes (estilo socket real, sin mocks, segun
  convencion del proyecto) se re-apuntan al split: levantar ambos
  binarios equivalente-a-in-process (sockets reales en loopback), verificar
  round-trip de proxy para `/_peers`, `/_api/instance`, fork-from-remote,
  PR-proxy create/comment/merge.
- Test nuevo de gossip: tres instancias de federacion, seed-bootstrap una
  en las otras dos, verificar que el descubrimiento transitivo llena
  `GossipPeer` sin auto-linkear bajo el umbral, despues verificar que el
  auto-link dispara solo cuando `auto_link_min_sources` y
  `auto_link_min_age` se satisfacen ambos (reloj falso o umbrales cortos
  en config de test).
- Test nuevo de permisos: un usuario de sesion con
  `is_federation_moderator=true` pero `is_admin=false` puede llegar a
  `/_peers` y banear un peer; un usuario comun no puede. Un peer
  `BrowseOnly` no puede forkear; subirlo a `BrowseAndFork` permite fork
  pero no PR-proxy.
- Test nuevo de ban: banear sin `reason` se rechaza; un peer baneado
  pierde de inmediato browse/fork/PR-proxy; `/_api/banned-peers` lo lista
  con motivo + `banned_by`, alcanzable sin auth; el mismo candidato se
  saltea en el tick de auto-link aunque vuelva a cumplir el umbral de
  corroboracion; `total_bans_issued` en `/_api/instance` incrementa
  despues de un ban y matchea el largo de `/_api/banned-peers`.
- Test nuevo de rate-limit: pasarse de `rate_limit_public_max` desde una
  IP devuelve 429 sin afectar los requests de una segunda IP en la misma
  ventana.
- Test nuevo de sister-sync: dos instancias de federacion,
  `sister_requested` mutuo en ambos lados confirma el handshake y pasa
  `sister_confirmed` a true; un ban de un lado se propaga al otro via el
  push de evento; matar el push (simular un request perdido) y correr el
  tick de reconciliacion periodica igual converge ambas listas de ban al
  mismo conjunto; el unban tambien se propaga. Verificar que
  `sister_requested` unilateral (sin reciprocidad) nunca activa el sync.
  Verificar que `total_bans_issued` en la hermana receptora **no**
  incrementa por un ban sincronizado (solo por los originados
  localmente).
- Manual: correr `gitquarry-core` solo sin seccion `[federation]` —
  confirmar sin crash, sin links muertos de federacion en la UI. Despues
  agregar el binario de federacion + config, confirmar que `/_peers` y
  `/_api/instance` funcionan de punta a punta via el proxy.

## Pregunta abierta (sin resolver, marcada como no-bloqueante)

La atribucion de `Peer.rate_limit_override` es heuristica basada en IP
solamente (Capa de moderacion) — cualquiera que comparta la IP de salida
de un peer baneado/limitado recibe el mismo trato, y un peer puede
esquivar su propio override llamando desde otra IP. Atribucion real
por-peer necesitaria mTLS o requests firmados, deliberadamente fuera de
alcance para esta vuelta. ¿Aceptable como primer paso de hardening, o el
rate limiting por-peer deberia esperar a un mecanismo de identidad mas
fuerte antes de shippear?
