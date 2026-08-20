# gitquarry — implementado vs. faltante, sección por sección

> Basado en el snapshot de `feature-analysis.md` (2026-08-09). Es una foto del
> código, no un contrato: los gaps marcados como "no mencionado" pueden existir
> en el código y simplemente no estar documentados en el snapshot — se marcan
> como tales y no como ausencias confirmadas.

---

## Arquitectura base

**Implementado**
- Dos binarios (`gitquarry-core` standalone + `gitquarry-federation` opcional) hablándose por HTTP loopback con `internal_token` compartido.
- Split reciente `gitquarry-webui` (presentación) / `gitquarry-server` (binario, wiring) dentro de lo que antes era un core monolítico; `core` quedó lib-only.

**Faltante / no mencionado**
- Sin mención de circuit breaker o degradación explícita si `federation` cae mientras `core` sigue corriendo (se infiere fail-closed por el resto del doc, pero no está descrito acá).
- Sin discovery automático entre los dos binarios más allá del token compartido — la configuración de "dónde está el otro proceso" parece manual.
- Sin mención de clustering horizontal a este nivel (ver también `gitquarry-server` más abajo).

---

## gitquarry-core

### `domain/issue.rs`

**Implementado**
- Lifecycle completo: `create`, `get`/`list` con filtro por `IssueStatus`, `edit` vía `IssueEdit<'a>` (get-etag + mutate + CAS-put en un paso), `transition`/`close`/`reopen` que rechaza no-ops, comment CRUD.
- Validación previa a escritura: labels deben existir en `RepoConfig.labels`, assignees deben resolver a usuario real, milestone debe resolver vía `milestones::get_milestone`.
- Comment edit/delete restringido a autor, sin override de maintainer.

**Faltante / gaps**
- Sin issue templates.
- Sin fechas límite (due dates) ni time tracking.
- Sin issues enlazados (blocks/blocked-by), solo el auto-close por keyword en merge de PR (ver `pull_request/mod.rs`).
- Sin sub-issues / tareas anidadas.
- Sin reacciones a comentarios.
- Comment delete **solo por autor**: a diferencia de GitHub/GitLab, un maintainer no puede borrar un comentario ajeno abusivo sin pasar por otro mecanismo — es una limitación de moderación real, no solo un detalle técnico.
- Sin acciones masivas (bulk edit/close de varios issues a la vez).

### `domain/milestone.rs`

**Implementado**
- CRUD simple, mismo patrón `transition` que issues.

**Faltante / gaps**
- Sin back-reference indexada a issues/PRs que apuntan al milestone (se resuelve en lectura) — funciona, pero implica costo de cómputo en cada vista de progreso en vez de un contador mantenido.
- Sin burndown/progress automático mencionado.
- Sin jerarquía de milestones ni vista de roadmap.

### `domain/pull_request/mod.rs` (833 líneas, el módulo más rico)

**Implementado**
- PRs cross-fork o mismo-repo; `create` exige que `source_repo` sea el propio target o un fork cuyo `RepoConfig::forked_from` apunte de vuelta.
- Status machine `Open/Closed/Merged`; `is_draft` como bool separado.
- Reviews append-only (cambiar de opinión crea fila nueva), solo cuenta el último veredicto por reviewer (`review_summary`); bloquea auto-aprobación del propio autor.
- Merge vía `git merge-tree --write-tree` (sin working tree), 3 estrategias (`Merge/Squash/Rebase`). `merge` exige Open + no-draft + ningún `ChangesRequested` pendiente — las aprobaciones **no** son requisito, solo el rechazo bloquea.
- Auto-cierre de issues referenciados con `closes #N` al mergear.
- `mark_merged_externally` — path idempotente sin tocar git, para el sync Track B con forges externos.

**Faltante / gaps**
- Sin reviewers requeridos / CODEOWNERS.
- Sin merge queue / merge trains.
- Sin status checks obligatorios — consecuencia directa de no tener CI integrado; hoy nada puede bloquear un merge por resultado de build/test.
- Sin "suggested changes" aplicables directamente desde el diff.
- Sin PR templates.
- No queda claro en el doc si un push nuevo a la rama invalida approvals previos automáticamente (dado que las reviews son append-only e inmutables, esto es una ambigüedad a verificar en código, no un gap confirmado).
- Sin auto-merge programado ("mergear cuando se cumplan las condiciones").

### `domain/release.rs`

**Implementado**
- Envuelve un tag existente; un release por tag (CAS `expected_etag = None`).
- `generate_source_archives`: tar.gz + zip desde un único `git archive`, subidos como assets normales.
- (Confirmado en la sección de roadmap, no detallado acá): soporte de draft/prerelease.

**Faltante / gaps**
- Sin generación automática de release notes desde commits/PRs mergeados desde el último tag (feature estándar en GitHub).
- Sin firma/verificación (GPG o sigstore) de los assets generados.
- Sin integración con package registry para publicar el release como paquete versionado — pero esto depende de que exista un package registry, que hoy no existe.

### `domain/remote_fork.rs`

**Implementado**
- Fork desde un peer federado (capa 2 de federación): trae refs del mirror remoto, empaqueta y sube objetos, escribe `RepoConfig` con `forked_from_peer`, siempre `Visibility::Public`.

**Faltante / gaps**
- Fork remoto **siempre público** — no hay opción de fork privado cross-peer, lo cual limita casos de uso donde alguien quiere forkear un repo público de otro peer para trabajar en privado antes de proponer un PR de vuelta.
- No se menciona si hay re-sync incremental posterior al fork inicial (¿se puede volver a traer commits nuevos del upstream peer después de forkear, o es una copia de un solo momento?) — pregunta abierta, no resuelta en el snapshot.
- Sin mención de manejo de colisión de nombres entre repos de distintos peers.

### `domain/repo.rs` (804 líneas, el más grande)

**Implementado**
- `create` es el único punto de creación de un repo (corrigió un bug histórico de push implícito por SSH que saltaba `RepoConfig`).
- `rename` es solo metadata (storage keyed por uid, no mueve nada).
- `delete` es irreversible.
- `fork` es duplicación real vía `Remote::copy_all_to`, resetea contadores, no hereda `archived`.
- `delete_branch`/`delete_tag` rechazan protegidos incondicionalmente, incluso para el owner.

**Faltante / gaps**
- Sin "transfer ownership" a otro namespace descrito explícitamente (rename cambia el nombre, pero mover un repo a otro dueño/organización es otra cosa y no aparece).
- Sin repo templates ("crear repo a partir de este como plantilla").
- `delete` irreversible de inmediato, sin papelera/período de gracia — GitHub sí ofrece una ventana de recuperación tras borrar; esto es una diferencia operacional real, no solo cosmética.
- Sin mención de qué pasa con issues/PRs abiertos al hacer fork (¿se copian? ¿quedan huérfanos del lado del fork?).

### Storage (`storage/`)

**Implementado**
- Dos namespaces disjuntos: `repos/` (git data) y `.conf/index/` (índices, tratados como "hints, no hechos", siempre re-verificados contra `RepoConfig`).
- `RepoConfig` es `Entity` de s3db, key = uid; issues/milestones/PRs usan `Collection::new_owned` con prefix runtime.
- Contadores de número (issue/milestone/PR) como secuencias independientes, CAS get/increment/put con retry (máx 8).
- Tipos de PR (meta/comment/review) viven en `gitquarry-wire` por cruzar el límite de IPC; issues/milestones se quedan locales.
- `storage/remote.rs`: `copy_all_to` (fork) es re-encrypt-and-copy real, no copy server-side — el AEAD ata el ciphertext a su key vía AAD, así que una copia cruda sería indescifrable en el prefix nuevo. Orden de subida: `.pack` antes que `.idx`.
- `storage/audit.rs`: trail append-only, keys mes-bucketed, cálculo de fecha proleptic-Gregorian hecho a mano (Howard Hinnant) para evitar una dependencia; `record` es fire-and-forget, nunca falla la acción que describe.

**Faltante / gaps**
- Sin cuotas de almacenamiento mencionadas — a diferencia de, por ejemplo, Codeberg (que sí aplica límites por usuario/organización), acá un solo repo o usuario podría, en teoría, consumir espacio sin tope.
- Sin lifecycle policies / compactación automática de objetos huérfanos fuera de `fsck` manual.
- Sin backup/replicación multi-región mencionados más allá de que S3 es la fuente de verdad.
- Retry CAS con máximo 8 pero sin backoff explícito descrito — bajo alta contención podría agotar reintentos sin éxito.
- `audit.rs` fire-and-forget es una decisión consciente (no bloquea la acción principal), pero implica que el trail de auditoría **no está garantizado**: una falla silenciosa de storage puede dejar huecos indetectables, algo relevante si alguna vez se necesita para compliance.

### Protocolos git (`git/`)

**Implementado**
- `git/http.rs`: smart-HTTP como middleware axum en el mismo puerto que la UI; Basic auth + tokens de federación de grano fino.
- `git/ssh.rs`: servidor SSH embebido (russh), pubkey-only, identidad por fingerprint (no username); host key persistida cifrada en el bucket.
- `git/lfs.rs`: batch API + transferencia directa, content-addressed por sha256, **sin locking API**.
- Redacción de tokens en toda función que maneja URLs con credenciales antes de que el error escape del módulo.

**Faltante / gaps**
- LFS **sin locking API** (explícito): proyectos con binarios grandes no versionables por texto (assets de diseño, juegos) pierden la capacidad de "lockear" un archivo mientras alguien lo edita, algo que GitHub/GitLab sí ofrecen.
- Sin mención de verificación de firmas de commit (GPG/SSH signing → badge "Verified").
- Sin mención de rate limiting / anti-abuso a nivel de protocolo git (clone floods, scraping agresivo).
- SSH resuelto por fingerprint, no por certificados CA-signed (relevante para despliegues enterprise con rotación de claves centralizada).

### Mirror

**(a) Mirror local/interno — implementado**
- Pre-receive hook: rechaza pushes a repos archivados, aplica reglas de protected branch/tag (fast-forward-only, tags inmutables, bypass owner/admin), detecta escrituras S3 concurrentes vía CAS del manifest, rechaza blobs grandes sin convertir a LFS, dispara sync de mirrors externos en detached.
- GC por LRU (edad + tamaño), sin relación con mirroring externo.

**Faltante / gaps (a)**
- GC puramente LRU: sin pinning manual para mantener repos "calientes" en cache aunque tengan poco tráfico reciente.

**(b) External mirror — implementado**
- Sync bidireccional de refs y PRs con GitHub/GitLab/Codeberg vía git plano + REST, sin SDKs de proveedor.
- Tres direcciones: `PushOnly`/`PullOnly`/`Bidirectional`.
- Política de conflicto: comparación 3-way (sha local/remoto/last-synced) por ref, fast-forward automático en cualquier dirección, divergencia real se marca `Diverged` en la UI — **nunca** auto-mergea ni fuerza.
- Track B (PR/MR sync) con adapters finos por proveedor (GitHub/GitLab/Codeberg) y bookkeeping anti-loop (campo `origin`, cursors de sync).

**Faltante / gaps (b)**
- Solo tres proveedores soportados (GitHub, GitLab, Codeberg); agregar uno nuevo requiere un adapter dedicado en Track B, no hay un modo genérico.
- El estado `Diverged` "se marca en la UI" pero no está descrito el flujo de resolución manual (¿el usuario elige qué lado gana? ¿se resuelve solo con push manual?).
- No queda claro si los objetos LFS se sincronizan como parte de Track A o si el sync externo cubre solo refs de git puro.

### Auth (`auth/`)

**Implementado**
- Cinco métodos: SSH pubkey (fingerprint-index), password local (Argon2id), TOTP (RFC 6238 hand-rolled, ~150 líneas, sin dependencia externa), OIDC (discovery-based, con guard SSRF explícito), token de federación (resuelto vía loopback a `gitquarry-federation`).
- `Permission::{Guest,Reporter,Developer,Maintainer,Owner}` como enum ordenado (el `Ord` es load-bearing para `.max()` entre grants de team).
- Invariante: un namespace es username o group name, nunca ambos, forzado por CAS compartido (`NAMESPACE_INDEX`).
- `effective_permission` en cascada: grant explícito por repo → owner implícito por namespace propio → owner implícito por dueño de grupo → mayor permission de team que cubra el repo → `default_permission`.

**Faltante / gaps**
- Sin WebAuthn/passkeys — solo TOTP como segundo factor, mientras GitHub/GitLab/Codeberg ya soportan passkeys.
- Sin SAML ni LDAP/Active Directory (relevante para SSO enterprise).
- Sin OAuth2 **como provider** (explícito en el roadmap) — bloquea que apps de terceros hagan "Login with gitquarry".
- Sin mención de recovery codes para TOTP (¿qué pasa si el usuario pierde el dispositivo?).
- No está claro si existen personal access tokens de larga duración con scopes granulares para uso local (fuera del contexto de tokens de federación).
- Sin rate limiting de intentos de login mencionado explícitamente, más allá de la protección contra timing attacks vía dummy-hash.

### i18n (`i18n/`)

**Implementado**
- Sin crate de i18n externo (`fluent`, `rust-i18n` descartados a propósito). Macro `msg!` hand-rolled que expande cada mensaje en una función real `fn(Locale, args...) -> String`, sin arm wildcard — un locale sin un mensaje es error de compilación, no fallback silencioso.
- Solo dos locales, `Es`/`En`, default `En`.

**Faltante / gaps**
- Cobertura de idiomas muy por debajo de GitHub/GitLab/Codeberg (este último con decenas de idiomas vía Weblate).
- Sin infraestructura de contribución de traducciones tipo Weblate/Crowdin — migración manual.
- Migración incremental incompleta: quedan strings en español sin convertir fuera de `i18n/` (deuda técnica activa, mencionada explícitamente en el snapshot).

### Comandos (`commands/`)

**Implementado**
- `serve` (arranca ssh+web juntos), `pre-receive-hook`/`sync-mirrors` (internos), `fsck` (escanea huérfanos, solo auto-borra placeholders vacíos), `repack` (consolida packs pequeños), `backfill-repo-index` (backfill idempotente para repos/grants previos a su existencia).
- Convención uniforme `--bucket NAME [--apply]`, dry-run por defecto.

**Faltante / gaps**
- Sin comando de backup/restore explícito más allá de que S3 es la fuente de verdad.
- Sin comando de import masivo directo desde un forge externo fuera del flujo de fork federado.
- `fsck` solo auto-borra placeholders vacíos — huérfanos reales requieren intervención manual (razonable como salvaguarda, pero limita la auto-recuperación).

---

## gitquarry-federation

**Implementado**
- **Gossip**: `GossipTable` in-memory (cap 500), auto-link con doble threshold (≥3 fuentes independientes + ≥24h de edad), promoción siempre a `PeerAccessLevel::BrowseOnly` (nunca directo a sister instance).
- **Browse/fork remoto**: dividido entre procesos — federation valida (peer linkeado, no baneado, tier suficiente) y reenvía; los mecanismos git pesados viven en `core::domain::remote_fork`.
- **PR-proxy**: lectura del lado core, escritura del lado federation (bearer token del peer); `authorize()` combina un ceiling local con una llamada viva a `/_internal/authorize` en core.
- **Tokens de grano fino**: `AccessToken` scoped por repo (`BTreeSet<Uid>`) + bool `write` como ceiling, resolución fail-closed con re-verificación completa en cada uso.
- **Moderación/bans**: ban exige reason obligatorio; unban resetea siempre a `BrowseOnly` (nunca restaura el tier previo); transparencia pública sin auth en `/_api/banned-peers`; `total_bans_issued` cuenta solo bans originados localmente.
- **Sister sync**: handshake de mutuo consentimiento + reconciliación periódica de listas de bans vía `PeerCredential` compartido.
- IPC con core: HTTP loopback, header `x-internal-token` en ambas direcciones, nunca un segundo puerto público.

**Faltante / gaps**
- **Gap conocido y deliberado, documentado en el propio código fuente**: sin identidad criptográfica de peer (mTLS/signed requests) — el rate-limit override es heurística de IP-text-match. Es una superficie de spoofing real: alguien que controle o falsifique la IP de origen podría intentar explotar el override.
- `unban` resetea siempre a `BrowseOnly`, nunca restaura el tier previo — decisión conservadora, pero implica trabajo manual repetido para re-promover un peer tras un ban erróneo o temporal.
- Sin federación de identidad de usuario: un usuario de un peer no puede autenticarse en otro peer con la misma cuenta.
- El PR-proxy federado parece limitado a proxyar sobre repos ya conocidos vía fork/browse, no a una vista federada unificada de issues/PRs cross-instancia (a diferencia de la visión a largo plazo de proyectos como ForgeFed).
- Sin mención de qué ocurre cuando la red gossip supera el cap de 500 entradas (¿eviction? ¿priorización por actividad?).

---

## gitquarry-webui + gitquarry-server + gitquarry-wire

### Webui

**Implementado**
- Cero JS de cliente salvo un archivo (`/_assets/app.js`); CSP `script-src 'self'` estricta.
- Browsing: tree/blob/blame/log (con grep)/compare/stats/refs, render de README, resaltado de sintaxis.
- Issues: lista/detalle/creación, comentarios, labels, assignees, milestone, close/reopen.
- Pull requests: diff anotado con comentarios inline, reviews (approve/request-changes/comment), merge con las 3 estrategias, draft toggle, edición de metadata.
- Milestones: vista única con formulario de creación inline.
- Releases: notas markdown, assets descargables.
- Repo settings: general/default-branch/permisos/protected branches-tags/topics/labels/límite LFS/mirrors externos/mover repo/danger-zone (delete con confirmación por nombre).
- Search cross-repo: fan-out sobre repos visibles + peers federados sincronizados, cap de 100 resultados por sección, nunca dispara un `git fetch` on-demand.
- Audit log: feed de solo lectura, admin-only.

**Faltante / gaps (explícitos en el roadmap)**
- **Sin wiki.**
- **Sin CI/CD** — impacta directamente en la ausencia de status checks en PRs.
- **Sin webhooks** — sin integraciones externas (Slack, Discord, deploy triggers) salvo desarrollo a medida.
- **Sin notificaciones** — ni email ni in-app; el usuario debe revisar manualmente.
- **Sin package registry.**

**Faltante / gaps (inferidos, no explícitos)**
- Sin project boards / kanban.
- Sin discussions tipo foro.
- Search cross-repo con fan-out en vivo (no un índice dedicado) y cap de 100 por sección — la latencia crece con el número de repos consultados, y no escala igual que un índice tipo Elasticsearch cuando la cantidad de repos visibles es grande.
- Cero JS de cliente es una elección deliberada de privacidad/CSP, pero implica renunciar a UI más rica (edición de código in-browser, previews interactivos) que sí tienen GitHub/GitLab.

### Auth web

**Implementado**
- Login por password (Argon2id, dummy-hash para tiempo constante en usuarios inexistentes) u OIDC (PKCE+CSRF+nonce), con TOTP opcional apilado encima de cualquiera de los dos.
- Sesiones stateless: selladas con AEAD, sin store server-side, TTL de 7 días; el `User` se re-resuelve de storage en cada request, así que la revocación (de la cuenta) es inmediata.
- Autoservicio de 2FA: enroll con QR, confirmación antes de activar, disable exige re-auth (password o código TOTP vivo, no solo sesión activa).

**Faltante / gaps**
- Sin passkeys/WebAuthn (mismo gap que en `auth/`).
- Sesiones stateless con TTL fijo de 7 días: revocar **una** sesión específica sin invalidar todas las demás es estructuralmente más difícil que con sesiones con store server-side — vale la pena flaggearlo como limitación arquitectónica, no solo de producto.
- Sin mención de alertas de login desde nuevo dispositivo/ubicación.

### gitquarry-server

**Implementado**
- Un solo binario `gitquarry`, dispatch de subcomandos.
- `serve.rs`: resuelve config o dispara el wizard de setup, arma el `AppContext`, lanza tareas de fondo (GC de mirror, sync de mirrors externos), corre ssh:// + webui concurrente en el mismo proceso vía `tokio::select!`.

**Faltante / gaps**
- Proceso único — sin modo cluster/HA descrito; si el proceso cae, todo el core cae, y no hay failover mencionado.
- Sin mención de healthchecks/métricas expuestas (tipo endpoint Prometheus) para observabilidad operacional.

### gitquarry-wire

**Implementado**
- DTOs mínimos compartidos entre los 2-3 boundaries de IPC: `Uid` (hex 128-bit estable, inmutable a través de renames), `InstanceInfo`, `RepoSummary`, modelo completo de PR wire (única fuente de verdad para PRs entre core y federation).

**Faltante / gaps**
- Sin versionado explícito del protocolo wire descrito — dado que `core` y `federation` se despliegan como binarios separados, la compatibilidad hacia atrás entre versiones desplegadas de forma independiente es una pregunta arquitectónica abierta que el snapshot no responde.

---

## Síntesis del roadmap (tal como lo declara el propio snapshot)

**Hecho**: LFS, 2FA, releases (con draft/prerelease), permisos de teams, search, PRs, issues, mirror externo (Track A+B), federación completa (4 capas + binary split).

**Pendiente**: wiki, CI, webhooks, notificaciones, package registry, OAuth2 como provider.

### Lectura de prioridad (no está en el doc original, es análisis)

Si hay que ordenar los pendientes por impacto en adopción real:

1. **CI + webhooks + notificaciones** — el bloque de mayor impacto. Sin CI no hay status checks en PRs (afecta directamente la calidad del merge machinery ya construido); sin webhooks no hay integraciones externas; sin notificaciones, el flujo de colaboración depende de que la gente entre a mirar manualmente. Los tres juntos son lo que más limita el uso en equipo a escala hoy.
2. **Wiki y package registry** — impacto medio; son features esperadas por comparación directa con GitHub/GitLab/Codeberg, pero no bloquean el flujo central de código/issues/PRs que ya está completo.
3. **OAuth2 como provider** — impacto a mediano plazo, relevante sobre todo si se busca que terceros construyan integraciones "Login with gitquarry"; no bloquea el uso directo de la plataforma.

Fuera del roadmap declarado, los gaps estructurales más notables detectados en este análisis son: ausencia de cuotas de storage, delete de repo sin papelera de recuperación, LFS sin locking API, y la falta de identidad criptográfica de peer en federación (gap ya reconocido y documentado como fuera de scope por el propio proyecto).****