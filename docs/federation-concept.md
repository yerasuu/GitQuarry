# Federacion entre instancias gitquarry (estilo Mastodon): enlazar instancias, navegar y fork cross-instance

> **DEPRECADO (2026-08-06).** El diseño de este documento — federacion
> viviendo dentro del binario unico `gitquarry`, en `src/federation/` — fue
> reemplazado por un segundo binario separado, `gitquarry-federation`, mas
> gossip P2P discovery y moderacion (roles, ban, rate limiting, listas de
> ban compartidas entre instancias hermanas). Ver
> `docs/federation-binary-split-concept.md` para el diseño vigente —
> mismas 4 capas descritas mas abajo, pero movidas a un proceso separado
> que se habla con `gitquarry-core` solo por HTTP loopback.
> `src/federation/` (todo lo referenciado en el parrafo original de este
> banner) ya no existe — ver `git log --grep federation` para el historial
> completo, incluido el commit de borrado. Este documento queda solo como
> referencia historica del diseño original de las 4 capas.

## Contexto

Hoy un proceso `gitquarry` sirve UN bucket S3 (`--bucket`), totalmente
aislado de cualquier otra instancia. Fork (`/_fork`, `src/web/fork.rs`) solo
copia dentro del MISMO bucket (`ctx.remote(&source).copy_all_to(&ctx.remote(&dest))`,
API S3-a-S3).

Pedido: instancias independientes (cada una con su propio storage,
proveedor S3, cifrado, usuarios — nada compartido) que se puedan **enlazar**
explicitamente entre si (como instancias Mastodon que se conocen), y una vez
enlazadas, ver el contenido publico de la otra a traves de la propia — sin
que eso implique confiar en su storage ni compartir credenciales. Sobre ese
contenido visto, poder hacer fork hacia el propio storage.

Dos capas separadas, igual que Mastodon separa "descubrir/ver perfiles y
posts de otra instancia" de "traer el contenido real" (media, texto):

1. **Enlace + descubrimiento** (nuevo): que instancias se conocen, que repos
   publicos tiene cada una — necesita una API minima entre instancias
   (hoy la UI web es HTML server-rendered, no hay ninguna API para que OTRO
   proceso gitquarry pregunte "que repos publicos tenes").
2. **Contenido real** (reusa lo que ya existe): navegar archivos/commits de
   un repo puntual, y fork — mismo protocolo `git://`/`ssh://` que ya sirve
   cualquier cliente git hoy, sin inventar transporte nuevo.

## Capa 1: enlace entre instancias

### Modelo de datos: `.conf/peers/<nombre>.json`

Mismo patron que `.conf/users/<name>.json`/`.conf/groups/<name>.json`
(`src/auth/access.rs`): un objeto por peer, no un blob unico. Un `Peer`:

```rust
struct Peer {
    name: String,       // como se muestra ("acme-instance")
    web_base_url: String,   // https://otrohost:8080 (para linkear a su UI)
    git_base_url: String,   // git://otrohost:9418 o ssh://otrohost:2222 (para fetch de contenido)
    linked_at: DateTime,
}
```

Gestion: pagina nueva `/_peers` en la UI (mismo patron admin-gated que
`/_users`/`/_settings`, ver `src/web/users.rs`/`src/web/settings.rs`),
solo `is_admin` la edita. Agregar un peer valida su URL
(`src/validate.rs`, nuevo validador de URL de instancia: esquema
`http(s)`/`git`/`ssh`, host, puerto — mismo criterio que el resto de
validate.rs, nunca confiar en la URL cruda) y hace un round-trip contra
`<web_base_url>/_api/instance` (ver abajo) antes de guardarlo: si no
responde o no es un gitquarry real, error claro en vez de guardar un peer
muerto.

**Enlace unidireccional, no mutuo** (asuncion, igual que Mastodon: cualquier
instancia puede "seguir" a otra sin pedirle permiso, porque solo expone lo
que ya es publico): A agrega a B como peer sin que B necesite hacer nada.
Si B tambien quiere ver el contenido de A, agrega a A como peer por su
lado, simetrico pero independiente — dos objetos `Peer` separados en dos
buckets separados, ninguno coordinado.

### API minima que cada instancia expone para las demas

Nuevos endpoints JSON de solo lectura, sin autenticacion (igual de expuestos
que el index publico hoy — mismo gate de visibilidad, nunca exponen repos
`private`):

- `GET /_api/instance`: `{ "name": ..., "version": ... }` — sirve para que
  otro peer valide "esto es un gitquarry real" al enlazar.
- `GET /_api/repos`: lista de repos `public` con metadata liviana
  (`name`, `description`, `topics`, `star_count`, `fork_count`,
  `default_branch`, `updated_at`) — el mismo filtro de visibilidad que ya
  usa el index (`public` only, ver README seccion de visibility), servido
  desde `s3::settings::get_repo_config` de cada repo igual que el index hoy.
- `GET /_api/repos/<name>`: metadata de un repo puntual (lo mismo, para
  cuando ya se conoce el nombre y no hace falta la lista completa).

Estos tres endpoints son la UNICA superficie nueva de "comunicacion entre
instancias" propiamente dicha — el resto (navegar archivos, fork) sigue
yendo por `git://`/`ssh://`, protocolo que ya existe.

### Cache local de lo que dicen los peers

Cada instancia cachea localmente la respuesta de `/_api/repos` de sus peers
(no pega a la red en cada render del index) con un TTL, mismo espiritu que
el barrido de mirrors (`crate::gc`, `--mirrors-max-age`): un refresh
periodico en background (tarea nueva junto a la de `gc.rs`) repuebla esta
cache; un peer caido no rompe el index local, solo muestra su ultima copia
conocida (o la omite si nunca respondio).

### Index federado

El index (`src/web/repo/mod.rs`, handler de `/`) hoy agrupa repos propios
por namespace. Se agrega una seccion nueva por peer enlazado ("repos de
`<peer.name>`"), poblada desde la cache de la capa anterior — mismo
criterio visual que agrupar por namespace, pero la fuente es la cache de
peer en vez de `s3::settings`.

## Capa 2: navegar contenido real y fork (reusa lo existente)

Cuando el usuario clickea un repo remoto especifico (desde la seccion de
peer en el index), la vista de ese repo puntual sigue el mismo patron ya
usado por `AppContext::open_mirror` (`src/app.rs:158`): lock por repo,
`ensure_bare_repo`, sync — solo que la sync viene de `git fetch` contra
`<peer.git_base_url>/<repo>` en vez de `sync_bare_repo_from_s3`
(`src/gitdaemon/mirror.rs:57`). El resto de las vistas
(`summary`/`tree`/`blob`/`log`/`blame`, bajo `src/web/repo/*`) renderiza
igual, porque ya opera sobre el path del mirror, no sobre S3 directo
(`src/web/repo/mod.rs:281`).

- Mirror remoto vive en subcarpeta separada bajo `--mirrors-dir`
  (`_remote/<peer>/<repo>`), mismo `Locks`/`crate::gc` de siempre.
- Nueva funcion `sync_bare_repo_from_remote_url` en `src/gitdaemon/mirror.rs`,
  junto a `sync_bare_repo_from_s3`: `git fetch <url> '+refs/*:refs/*'` via
  `Command::new("git")` (mismo estilo que `src/git/mod.rs`), URL siempre
  despues de `--end-of-options`.
- **Fork** (`src/web/fork.rs`): `ForkForm.source` acepta, ademas de un
  nombre de repo propio, un par `(peer, repo remoto)`. El flujo:
  1. Sincroniza el mirror remoto (arriba).
  2. "Siembra" el repo nuevo en el bucket PROPIO desde ese mirror local —
     operacion inversa a `sync_bare_repo_from_s3`: empaqueta con
     `git::pack_objects_for_push` (`src/git/mod.rs:126`, ya usado hoy por
     el hook `pre-receive`) y fija refs con `Remote::put_manifest`
     (CAS con `expected_etag = None`, `dest` es nuevo). Helper nuevo
     `seed_repo_from_local_mirror` en `src/gitdaemon/mirror.rs`.
  3. Mismo resto del flujo de hoy (`fork.rs:57-131`): valida namespace del
     caller, escribe `RepoConfig` con `forked_from` = URL completa del
     repo de origen (peer + nombre) en vez de nombre local, registra en
     audit log.

## Capa 3: busqueda de contenido remoto (extiende `/_search`)

El index federado (Capa 1) resuelve "aparecer listado"; no resuelve
"aparecer en `/_search`" (`src/web/search/mod.rs:31-53`), que hoy busca por
contenido real (`filenames::search_all`, `commits::search_all` — nombres de
archivo y mensajes de commit via `git ls-tree`/`git log --grep`, cap
`MAX_RESULTS`), no por metadata. Buscar contenido de un repo remoto
requiere el mirror local de la Capa 2 (`_remote/<peer>/<repo>`) — no existe
transporte para "grep remoto" sin bajar el repo.

Dos niveles, sin disparar fetch nuevo en cada busqueda (una query de texto
no debe convertirse en N `git fetch` a peers — costo/latencia impredecible,
y abre un vector de abuso: alguien busca cosas random para forzar al server
a sincronizar medio catalogo ajeno):

- **Metadata (siempre, barato)**: `handler_search_get` suma, a los repos
  locales, un match por substring sobre la cache de peers (`name`,
  `description`, `topics` — mismos campos que ya cachea `/_api/repos` en
  Capa 1). Sin red nueva, reusa la cache con TTL que ya existe.
- **Contenido (solo si ya hay mirror local)**: si `_remote/<peer>/<repo>`
  ya existe (porque alguien lo navego o forkeo antes, via Capa 2),
  `filenames::search_all`/`commits::search_all` lo escanean igual que
  cualquier repo local — es un bare repo mas en disco, el codigo de
  busqueda no necesita distinguir origen. Se agrega a la lista de repos
  candidatos que arma `visible_repo_names`
  (`src/web/repo/mod.rs:201-203`): ademas de namespaces propios, incluye
  los mirrors bajo `_remote/*/*` que existan al momento de la query.

Consecuencia aceptada, no bug: un repo remoto nunca navegado/forkeado solo
aparece por metadata (nombre/descripcion/topics), no por contenido de
archivos o commits, hasta que alguien lo abra una vez y quede el mirror
sincronizado. Resultado de busqueda deberia marcar visualmente "remoto,
solo metadata" vs "remoto, contenido indexado" para que no se lea como
busqueda incompleta por bug.

## Capa 4: colaboracion en repos privados via token delegado (fine-grained)

Capas 1-3 son publico-only por diseno (ver Seguridad, abajo). Pedido nuevo:
un usuario con acceso a un repo `private`/`limited` en la instancia B debe
poder verlo, buscarlo, y colaborar (comentar/abrir PR/push) desde la
instancia A tambien — sin que eso implique confiar credenciales de B ni
dar a A mas de lo que ese usuario puntual ya tiene en B.

Mecanismo: token personal (PAT) **fine-grained** — nunca "actua como yo en
todo mi bucket", siempre acotado a una lista explicita de repos + un scope
`read`/`write`. Distinto del `Peer` (Capa 1): `Peer` es a nivel instancia,
admin-gated, solo contenido publico: esto es a nivel usuario, cada uno
decide su propio token, sin que el admin este en el medio.

### Modelo de datos en B: `AccessToken`

Mismo patron `.conf/*/`-por-entidad que `Peer`/`User`/`Group`
(`src/auth/access.rs`): nuevo modulo `src/auth/access/tokens.rs`, objeto en
`.conf/tokens/<uid>.json`:

```rust
struct AccessToken {
    uid: Uid,
    owner: Uid,              // que User lo creo
    label: String,           // "peer A - browsing", solo display
    token_hash: String,      // sha256 del secreto, nunca el secreto en claro
    repos: BTreeSet<Uid>,    // fine-grained: nunca vacio/None-significa-todos
    write: bool,             // false = read, true = read+write
    created_at: DateTime,
    expires_at: Option<DateTime>,
    last_used_at: Option<DateTime>,
}
```

- **Secreto**: generado una vez al crear (`token_<40 chars random>`),
  mostrado una sola vez en la UI (mismo patron que cualquier secret-shown-
  once), nunca recuperable despues — solo `token_hash` (SHA256 simple,
  no Argon2: el secreto ya es alta entropia/random, no elegido por humano,
  no hace falta KDF lento en cada request de API que lo valida).
- **Indice de lookup**: `TOKEN_INDEX: SecondaryIndex` keyed por
  `sha256(secreto)` → puntero `{owner_username, token_uid}`, mismo
  doctrina "hint, no fact" que `PUBKEY_INDEX`
  (`src/auth/access/pubkey.rs`): se re-verifica `token_hash` contra el
  `AccessToken` real antes de confiar, index vencido/revocado falla cerrado.
- **Techo de permiso, nunca escalada**: el token jamas da mas de lo que el
  usuario ya tiene. Efectivo en un repo:
  `min(access::effective_permission_for_user(user, repo), techo_del_scope)`
  — `write: false` techa en `Permission::Reporter` (confirmado: es el
  mismo piso que ya exige lectura de un repo `private` por git-HTTP y por
  la web UI, `src/git/http.rs:237-246`/`src/web/repo/mod.rs:228` —
  reusa el piso existente, no uno nuevo); `write: true` no techa mas
  alla del permiso real del usuario. Y siempre, ademas, `repo_uid` tiene
  que estar en `token.repos` — si no esta listado, `None` sin importar
  el tier.

  Nota aparte (no de Capa 4, preexistente): `git::ssh`'s `upload-pack`
  (clone/fetch por SSH) hoy no valida ningun piso minimo — un `Guest`
  puede clonar por SSH aunque HTTP/web exijan `Reporter` para lo mismo
  (`src/git/ssh.rs:277-282`, sin el `>= Reporter` que si tiene
  `git/http.rs`). Si el token de Capa 4 alguna vez se acepta tambien
  sobre SSH (hoy el plan es solo Basic-auth sobre smart-HTTP), esta
  inconsistencia hay que cerrarla primero — token con techo `Reporter`
  sobre un transporte que no aplica ese piso a nadie mas seria un techo
  de mentira.
- **Revocacion**: borrar el `AccessToken` + su entrada en `TOKEN_INDEX` —
  instantaneo, mismo best-effort-pero-fail-closed que `release_pubkey`.
- **UI**: `/_profile/<user>/security` (o pestana nueva) — crear (elegir
  repos + scope + expiracion opcional), listar (label/repos/scope/last
  used, nunca el secreto de nuevo), revocar.

### Credencial linkeada en A

Por (usuario local de A, peer): que token de B usa ese usuario cuando A
necesita actuar en su nombre contra B. Nueva entidad, `.conf/peer-tokens/
<user_uid>/<peer_name>.json` o similar — **cifrada** (mandato de cifrado
de siempre, esto es mas sensible que cualquier cache de metadata publica).
UI: "mis credenciales hacia peers" bajo el mismo lugar que gestiona tokens
propios.

### Superficie nueva en B

- `GET /_api/repos` / `/_api/repos/<name>`: con `Authorization: Bearer
  <token>` valido, ademas de lo publico ya devuelto (Capa 1), suma los
  repos `private`/`limited` que ese token puede ver — mismo endpoint,
  respuesta mas rica si viene autenticado.
- `crate::git::http`/SSH: el token se acepta como credential adicional
  (Basic-auth `token@host` sobre smart-HTTP, igual convencion que GitHub)
  — reusa el path de auth existente sin cambios de logica, solo un tipo
  mas de credencial que resuelve a `(User, techo_de_permiso)`. `git
  clone`/`push` sobre un repo privado ya autorizado para ese token
  funciona sin tocar `mirror::hook` — el hook sigue viendo al usuario
  autenticado normal, el token es solo el "como te identificaste", no
  una via de autorizacion paralela.
- **API JSON de escritura** (nueva, la pieza mas grande): hoy
  `repo_pull_requests::handler_create_post`/`handler_comment_post`/
  `handler_merge_post` son POST de formulario HTML gateados por sesion +
  CSRF. Necesitan equivalente JSON autenticado por Bearer token — misma
  logica de dominio (`domain::pull_request::*`), superficie HTTP nueva.
  Sin CSRF: un Bearer token en el header no viaja automatico como una
  cookie, no hay vector CSRF que mitigar ahi.

### En A: pestana de PRs sobre un repo remoto

Capa 2 (mirror git) trae objetos git, nunca PRs — son entidad `s3db` en B,
no viven en ningun ref. Ver/comentar PRs de un repo remoto desde A es
**proxy en vivo**, no cache: A llama la API JSON de B (arriba) con el
token linkeado del viewer, en el momento, nunca precalculado — a
diferencia de Capa 1 (metadata publica, cacheable con TTL) esto es
sensible a ACL por usuario, cachearlo mezclaria viewers.

## Seguridad / limites explicitos

- Nunca se leen credenciales ni bucket del peer — todo pasa por su
  `git://`/`ssh://`/API HTTP publica, igual que cualquier cliente externo.
- Un repo `private` en la instancia remota jamas aparece en `/_api/repos`
  ni es fetcheable via su `git://` sin auth (mismo gate que ya existe hoy
  para clientes externos comunes) — la federacion no es un side-channel
  para ver privados ajenos, **salvo** que el viewer tenga un token
  fine-grained propio listando ese repo (Capa 4) — nunca por ser peer de
  la instancia solamente.
- Token nunca escala el permiso real del usuario en B, nunca cubre repos
  no listados explicitamente, siempre revocable al instante (ver Capa 4).
- `/_peers` gatea a `is_admin`: agregar un peer es una decision operativa
  (quien enlazamos), no algo que cualquier usuario logueado dispare.
  "Ver/fork remoto" para usuarios comunes actua solo sobre peers ya
  enlazados por un admin, nunca sobre una URL libre pegada por el usuario —
  cierra la superficie SSRF-adyacente de "cualquier usuario hace que el
  server pegue a cualquier host".
- `git fetch`/`seed_repo_from_local_mirror` fallan con error claro si el
  peer no responde o el repo no existe/no es publico — nunca fallback
  silencioso.

## Fuera de alcance (no se construye en esta vuelta)

- Enlace mutuo/handshake con aprobacion del otro lado (Mastodon real
  requiere approval en cuentas privadas; aca todo lo federado es publico
  por definicion, no hace falta) — sigue sin hacer falta, Capa 4 es por
  usuario/token, no cambia el enlace `Peer` en si.
- Token con scope mas fino que read/write (ej. "puede mergear pero no
  pushear") — v2 si hace falta, no ahora.
- Fork como "link vivo" que se sigue actualizando solo — sigue siendo una
  foto al momento de forkear, como hoy.
- Cualquier acoplamiento al storage/proveedor S3 del peer: fuera de
  alcance total, nunca se toca directo.
- Disparar `git fetch` a un peer solo porque alguien hizo una busqueda de
  texto: no. Busqueda de contenido remoto se limita a mirrors que ya
  existen por navegacion/fork previos (ver Capa 3) — nunca sincroniza
  a demanda de una query.

## Archivos a tocar (cuando se retome)

- `src/auth/access.rs` (o modulo nuevo `src/auth/peers.rs`, mismo patron):
  `Peer` struct, `.conf/peers/<name>.json`, list/get/put/delete con CAS.
- `src/validate.rs`: validador de URL de instancia (esquema/host/puerto).
- `src/web/peers.rs` (nuevo): handler `/_peers` (admin-gated, alta/baja de
  peers, valida contra `/_api/instance` del candidato antes de guardar).
- `src/web/api.rs` (nuevo) o extension de `src/web/mod.rs`: handlers
  `/_api/instance`, `/_api/repos`, `/_api/repos/<name>` — mismo filtro de
  visibilidad publica que el index.
- Tarea de refresh periodico de la cache de peers: junto a la tarea
  existente de `crate::gc` (barrido de mirrors), arrancada desde
  `src/serve.rs`.
- `src/gitdaemon/mirror.rs`: `sync_bare_repo_from_remote_url` y
  `seed_repo_from_local_mirror`, junto a `sync_bare_repo_from_s3`.
- `src/app.rs`: variante de `open_mirror` para mirrors bajo
  `_remote/<peer>/<repo>`.
- `src/web/fork.rs`: `ForkForm` acepta origen remoto (peer + repo);
  rama de flujo con el seed de arriba.
- `src/web/repo/mod.rs`: index agrega seccion por peer; ruta para abrir un
  repo remoto puntual reusando `summary`/`tree`/`blob`/`log`; `visible_repo_names`
  suma mirrors ya sincronizados bajo `_remote/*/*`.
- `src/web/search/mod.rs`: `handler_search_get` suma match de metadata
  contra cache de peers (name/description/topics), ademas del contenido
  ya cubierto para mirrors remotos existentes.
- `src/auth/access/tokens.rs` (nuevo, Capa 4): `AccessToken`, `TOKEN_INDEX`,
  create/list/revoke, `effective_permission_for_token`.
- `src/auth/access/mod.rs`: `mod tokens;` + re-export, mismo patron que el
  resto de submodulos.
- `src/web/security.rs` (o pestana nueva bajo `/_profile/<user>`): UI de
  tokens propios — crear/listar/revocar — y de "mi credencial hacia
  peer X" (token ajeno linkeado).
- `src/git/http.rs`/`src/git/ssh.rs`: aceptar Bearer/Basic token como
  credential adicional, resolviendo a `(User, techo_de_permiso)` via
  `tokens::resolve_by_token`.
- `src/web/api.rs`: `/_api/repos`/`/_api/repos/<name>` leen header
  `Authorization` opcional, suman privados visibles para ese token.
- `src/web/repo_pull_requests.rs` o modulo nuevo `src/web/api_pulls.rs`:
  variantes JSON de create/comment/merge, autenticadas por Bearer token,
  misma logica de `domain::pull_request::*`.
- `src/web/repo/pulls.rs` (o vista nueva): pestana de PRs sobre un repo
  remoto — proxy en vivo a la API de B con el token linkeado del viewer.

## Verificacion (cuando se implemente)

- `cargo test`: unitarios para el validador de URL de instancia, para
  `Peer` CRUD con CAS, y para `sync_bare_repo_from_remote_url`/
  `seed_repo_from_local_mirror` contra un bare repo local de prueba
  haciendo de "peer" (`git daemon` o un segundo `gitquarry --allow-git-daemon`
  en otro puerto).
- Manual: levantar dos instancias con buckets/MinIO distintos
  (`docker-compose.minio.yml` con puertos separados), enlazar A→B desde
  `/_peers` de A, confirmar que `/_api/repos` de B aparece listado en el
  index de A bajo la seccion del peer, navegar un repo de B desde la UI de
  A (tree/log/blob), forkearlo hacia A, y confirmar que A tiene un repo
  propio nuevo (`forked_from` = referencia a B) sin que B haya sido
  tocado.
- Buscar en A un termino que solo matchea metadata de un repo de B nunca
  navegado: aparece por nombre/descripcion/topics, no por contenido.
  Navegar ese repo una vez (se sincroniza el mirror), repetir con un
  termino que solo existe en un archivo/commit de ese repo: ahora aparece
  tambien por contenido, sin disparar un fetch nuevo en la busqueda misma.
- `cargo clippy --all-targets` limpio (`unsafe_code = forbid`,
  `all = warn`, sin excepciones).
- Capa 4: crear token fine-grained en B acotado a un repo `private`,
  confirmar que `/_api/repos` de B sin token no lo lista, con token si;
  confirmar que un segundo repo `private` no listado en el token nunca
  aparece aunque el usuario tenga acceso real ahi (techo por repo, no por
  usuario); revocar el token y confirmar rechazo inmediato; probar `git
  clone`/`push` sobre ese repo autenticando con el token via smart-HTTP;
  abrir/comentar un PR desde A contra un repo de B usando el token
  linkeado y confirmar que aparece en la vista normal de B tambien (mismo
  dato, no una copia).
