# gitquarry — plan de paridad (gaps reales) y progreso

> Deriva de `feature-compare.md` (2026-08-09). Cubre solo los gaps marcados
> como "Faltante/gaps" sección por sección, **no** las 6 features grandes de
> "Pendiente" (wiki, CI, webhooks, notificaciones, package registry, OAuth2
> provider) — esas son otro roadmap. Excluye también lo marcado como
> decisión deliberada de arquitectura (ver sección final).
>
> Legend: `[ ]` pendiente · `[~]` en progreso · `[x]` hecho

---

## Tier 1 — alto valor, bajo/medio esfuerzo

- [x] Repo delete: soft-delete con ventana de gracia (hoy irreversible al toque)
- [x] LFS locking API
- [x] TOTP recovery codes
- [x] Rate limiting de intentos de login
- [x] CAS retry (storage) con backoff explícito, no solo cap de 8
- [x] Issue comments: permitir a maintainer borrar comentario ajeno (hoy solo autor)
- [x] Milestone: back-reference indexada a issues (contador mantenido, no recomputado en cada vista — MRs no tienen campo `milestone` en esta versión)
- [x] Healthchecks/métricas expuestas (endpoint tipo Prometheus)

## Tier 2 — valor medio, esfuerzo medio

- [x] Storage quotas por usuario/organización
- [x] MR: reviewers requeridos / CODEOWNERS
- [x] MR: templates
- [x] Issue templates
- [x] Issues: acciones masivas (bulk close/reopen)
- [x] Repo templates ("crear repo desde plantilla")
- [x] Transfer ownership de repo a otro namespace — ya existía (`webui::repo_settings::move_repo`), doc estaba desactualizado; no requirió cambios de código
- [~] WebAuthn / passkeys — pospuesto: necesita dependencia cripto nueva + ceremonia JS de navegador que no se puede probar en este entorno (sin browser/authenticator real); retomar en sesión dedicada con verificación manual
- [x] Project boards / kanban
- [x] Release notes: generación automática desde commits/MRs mergeados desde el último tag
- [x] Release assets: firma/verificación (GPG, vía el binario `gpg` del sistema — no sigstore)
- [x] Verificación de firmas de commit (GPG → badge "Verified" contra las `gpg_keys` propias del usuario; SSH signing queda fuera — ver nota abajo)
- [x] i18n: terminar migración de strings en español sueltos fuera de `i18n/` — ya estaba hecho, verificado con grep de acentos/palabras comunes en español sobre `crates/*/src`: cero hits en strings de UI; lo único en español fuera de `i18n/` son citas textuales de docs de diseño dentro de comentarios (`gossip.rs`, `mr_sync/mod.rs`, federation `web/*.rs`), no strings de usuario
- [x] Wire protocol: versionado explícito core↔federation
- [x] Rate limiting / anti-abuso a nivel de protocolo git (clone floods, scraping) — cubre `info/refs`/`upload-pack` (smart-HTTP); `receive-pack`/`ssh://` quedan fuera, ver nota en `GitRateLimiter`
- [x] Search cross-repo: índice dedicado por repo (`storage::search_index`, mantenido en el push, no en cada query) en vez de `git` en vivo — sigue fanning out por repo para agregar resultados, pero sin subproceso/tree-walk por query; peer mirrors remotos y repos sin indexar todavía caen al camino en vivo de antes

## Tier 3 — valor medio, esfuerzo alto / arquitectónico

- [ ] Session store para poder revocar **una** sesión específica (hoy stateless-only, TTL fijo 7d)
- [ ] SAML / LDAP (SSO enterprise)
- [ ] Discussions tipo foro
- [ ] External mirror: modo genérico de proveedor (hoy 3 hardcoded: GitHub/GitLab/Codeberg)
- [ ] Clustering horizontal / HA para `gitquarry-server` (hoy proceso único, sin failover)
- [ ] Federación: identidad de usuario federada cross-peer
- [ ] Federación: vista unificada de issues/MRs cross-instancia (más allá de fork/browse)
- [ ] Storage: lifecycle policies / compactación automática de huerfanos (hoy solo `fsck` manual)
- [ ] Storage: backup/replicación multi-región

## Tier 4 — investigar antes de implementar (ambigüedad en doc, no gap confirmado)

- [ ] Remote fork: ¿re-sync incremental post-fork o copia de un solo momento?
- [ ] Remote fork: colisión de nombres entre repos de distintos peers
- [ ] Repo fork: qué pasa con issues/MRs abiertos del repo original
- [ ] External mirror: flujo de resolución manual cuando queda `Diverged`
- [ ] External mirror: ¿LFS se sincroniza o solo refs de git puro?
- [ ] Gossip table: comportamiento al superar cap de 500 entradas (¿eviction? ¿priorización?)
- [ ] Circuit breaker / degradación explícita si `federation` cae mientras `core` sigue

---

## Fuera de este plan (decisión deliberada, no gap)

No tocar salvo pedido explícito nuevo del usuario:

- Zero client-JS en webui (elección de privacidad/CSP)
- Fork remoto siempre público
- `unban` resetea siempre a `BrowseOnly`, nunca restaura tier previo
- Audit trail fire-and-forget (nunca bloquea la acción que describe)
- Sin identidad criptográfica de peer (mTLS) — gap ya documentado como fuera de scope en el propio código fuente
- Personal access tokens de larga duración con scopes granulares: `auth/access/mod.rs` dice explícito que ese concepto vive en `gitquarry-federation`, no en core — decidido 2026-08-09 no reintroducirlo en core solo para cubrir el caso standalone-sin-federation
- Verificación de firmas SSH en commits (`gpg.format=ssh`): la firma GPG cubre el caso real (`crate::gpg`, reutiliza `User::gpg_keys`); SSH signing quedó fuera de scope de esta pasada — no hay bloqueo técnico, solo no se llegó a implementar; retomar si se pide explícitamente

## Fuera de este plan (son las 6 features grandes de "Pendiente", otro roadmap)

wiki, CI, webhooks, notificaciones, package registry, OAuth2-as-provider — y todo lo que depende de ellas (status checks obligatorios en MR, integración release↔package-registry).
