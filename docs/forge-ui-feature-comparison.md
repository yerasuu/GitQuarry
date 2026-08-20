# Comparativa de interfaz y features — GitHub / GitLab / Codeberg (Forgejo)

> Snapshot 2026-08-10. Investigación vía búsqueda web + conocimiento previo, no auditoría exhaustiva de código de los tres forges. Objetivo: dar contexto externo al roadmap de gitquarry (ver `feature-compare.md`), no replicar cada feature — priorizar cuáles son relevantes por arquitectura y escala reales de gitquarry.

---

## 1. Navegación de código

| | GitHub | GitLab | Codeberg (Forgejo) |
|---|---|---|---|
| Árbol/blob/blame/history | Sí, sidebar redimensionable, breadcrumb de símbolo al hacer scroll | Sí, blame agrupado por commit | Sí, server-rendered, sin bundler JS |
| Edición in-browser | `github.dev` (VS Code sin cómputo) + Codespaces (contenedor real, sesiones colaborativas 2026) | Web IDE (VS Code-based, multi-archivo) + Remote Development/Workspaces (Premium/Ultimate, k8s) | Editor cambiado de Monaco a **CodeMirror** en v14 (ene 2026) — más liviano, mejor accesibilidad |
| Code search cross-repo | **Blackbird**: motor propio en Rust, sharded, ~640 qps, indexa ~120k docs/s + capa semántica NL (2026) | **Advanced Search** (Elasticsearch, Premium+) para todo; **Zoekt** (beta) solo para código exacto | **No resuelto a nivel instancia** — bleve/ES configurable para issues/texto, pero code search cross-repo lleva años estancado (zoekt PR #8827 sigue en draft) |

**Relevancia gitquarry**: nuestro search cross-repo (índice per-repo mantenido en push, cap 100/sección) ya está **por delante de Codeberg** en este punto específico — Codeberg reconoce el gap abiertamente. No perseguir Blackbird-tier (fuera de escala), pero sí vale la pena documentar esto como ventaja real, no solo gap cerrado.

---

## 2. Issues

| | GitHub | GitLab | Codeberg |
|---|---|---|---|
| Templates | YAML forms estructuradas | Sí (markdown/YAML) | YAML/markdown (`.forgejo/ISSUE_TEMPLATE`) |
| Sub-issues / jerarquía | Sub-issues nativas (GA 2025, reemplazó tasklist blocks) | Epics/Iterations/Weight — **Premium/Ultimate only** | Sin sub-issues nativas |
| Issues relacionados (blocks/blocked-by) | No nativo fuera de sub-issues | Sí, **free** | No nativo |
| Bulk actions | Sí (multi-select en toolbar) | Sí, **free** | No mencionado |
| Reacciones | Sí | — | Sí |
| Time tracking | No | — (Ultimate tiene reporting más rico) | Básico (start/stop, sin reporting) |
| Búsqueda semántica NL | Preview ene 2026 | — | — |

**Relevancia gitquarry**: `domain/issue.rs` ya cubre labels/milestones/assignees/comments/transitions. Gaps reales confirmados por esta comparativa: **issues relacionados (blocks/blocked-by)** es free hasta en GitLab y ninguno de los tres lo trata como feature premium — vale más que sub-issues jerárquicas (más simple de implementar, cubre el caso de uso real "esto bloquea aquello"). Bulk actions también es universal en los tres, gitquarry no lo tiene.

---

## 3. Pull/Merge requests

| | GitHub | GitLab | Codeberg |
|---|---|---|---|
| Suggested changes aplicables | Sí | Sí, **free**, commit atribuido al sugeridor | Sí ("suggest change" diff-quote) |
| CODEOWNERS | Sí + required-reviewer rule separada (GA feb 2026) | Sí, approval rules — **Premium/Ultimate** | Sí (root/`docs/`/`.forgejo/`) |
| Merge queue / trains | Merge queue, CODEOWNERS es gate duro sin bypass | Merge trains — **Premium/Ultimate**, solo merge-commit | Ausente en mainline; "stacked PRs" en progreso (#10641) |
| Auto-merge | Sí | "Merge when pipeline succeeds", **free** | **Ausente en mainline** |
| Status checks requeridos | Sí, ligado a rulesets | Sí, third-party checks — **Ultimate** | Vía Actions/Woodpecker commit status, branch-protection rule |
| Métodos de merge | merge/squash/rebase | merge/semi-linear/squash/fast-forward, todos **free** | merge/rebase/rebase-merge/squash/fast-forward-only |

**Relevancia gitquarry**: nuestro `merge_request/mod.rs` ya tiene 3 estrategias de merge, reviews append-only, auto-close por keyword — a la par de Codeberg en lo estructural. Gaps confirmados como universales-y-gratuitos en los tres (no premium-gated en ninguno): **CODEOWNERS/reviewers requeridos** y **status checks obligatorios**. Merge queue/trains es feature de escala de equipo grande — bajo prioridad real para gitquarry hoy. Auto-merge notablemente **ausente también en Codeberg** — si Forgejo (que es 10+ años de código Gitea) no lo tiene resuelto, no es urgente para gitquarry tenerlo ya.

---

## 4. CI/CD

- **GitLab**: diferenciador histórico — pipeline embebido en la misma nav que código/issues, no un tab aparte. Gráfico de pipeline con dependencias upstream/downstream multi-proyecto.
- **GitHub Actions**: tab "Checks" separado, anotaciones inline en diff desde comandos de workflow, badges de status por commit.
- **Codeberg**: dos sistemas en paralelo confusamente coexistiendo — Forgejo Actions (compatible GitHub Actions, ya "production ready") + Woodpecker CI (histórico, todavía corriendo). El propio runner hosteado de Codeberg sigue en alpha limitado por preocupaciones de seguridad/bus-factor.

**Relevancia gitquarry**: CI es el gap #1 del roadmap propio (`feature-compare.md` ya lo marca como máximo impacto). Esta comparativa confirma que **status checks obligatorios en MR dependen 100% de tener CI** — sin eso, el merge machinery ya construido (reviews, merge strategies) queda cojo en la práctica de equipo real. Lección de Codeberg: no lanzar dos sistemas de CI en paralelo — elegir uno solo desde el diseño (evitar la confusión Actions/Woodpecker que el propio Codeberg documenta como FAQ recurrente).

---

## 5. Wiki

Los tres: wiki respaldada por git propio (repo separado), markdown, historial vía commits. Ninguno tuvo rework relevante 2025-2026 — es una feature "resuelta y estable" en el ecosistema, no un área de innovación. Confirma que implementarla en gitquarry es trabajo bien acotado (git-backed repo + render markdown + sidebar de páginas), no hay sorpresas de diseño esperando.

---

## 6. Notificaciones

- **GitHub**: filtros guardados custom (hasta 15) con query syntax (`is:`, `reason:`, `author:`).
- **GitLab**: niveles jerárquicos (proyecto > grupo > global), modo custom por tipo de evento, todo **free**.
- **Codeberg**: watch levels por repo (Watching/Not Watching/Ignoring) + dropdown granular por evento (issue/PR/release/push), settings separados de email.

**Relevancia gitquarry**: es el gap #1 de UX real hoy (usuario debe revisar manualmente). Ningún patrón de los tres es complejo de replicar a escala básica: watch level por repo + filtro por tipo de evento + email opcional cubre el 80% de valor sin necesitar infra de digest ni motor de reglas.

---

## 7. Releases & package registry

- **GitHub**: release notes auto-generadas desde PRs mergeados desde el último tag, categorizadas por label, GitHub Packages (npm/Maven/Docker/NuGet/RubyGems).
- **GitLab**: Package Registry es **free desde la 13.3** — npm/Maven/PyPI/NuGet/Go/Composer/Conan/Helm/generic, con UI de actividad por paquete.
- **Codeberg/Forgejo**: package registry integrado — npm/Maven-Gradle/Docker-OCI/Helm/NuGet/PyPI, scoped a user/org.

**Relevancia gitquarry**: package registry es feature **gratuita y estándar en los tres**, no premium en ninguno — reafirma que está bien puesto en el roadmap propio como pendiente de impacto medio, pero cuando se aborde, hacerlo genérico multi-formato desde el inicio (no un formato a la vez), copiando el approach content-addressed que ya existe para LFS/release-assets en `domain/release.rs`.

---

## 8. Federación

- **GitLab/GitHub**: sin federación cross-instancia real (ninguno lo necesita, son SaaS centralizados).
- **Codeberg/Forgejo (ForgeFed)**: solo **star federation** shipeado (2025); issues/PRs/identidad federados siguen siendo roadmap, no implementados.

**Relevancia gitquarry**: esta es la comparación más favorable para el proyecto — `gitquarry-federation` (peers/browse/fork/MR-proxy/tokens de grano fino, 4 capas completas) está **arquitectónicamente más avanzado** que el estado actual de ForgeFed en Forgejo, el peer más cercano en filosofía. Vale la pena que esto quede explícito en cualquier material de posicionamiento del proyecto — no es un gap, es una ventaja real y verificada externamente.

---

## 9. Seguridad (scanning, secrets, dependencias)

- **GitHub**: tab "Security" unificado (Dependabot + code scanning/CodeQL + secret scanning con push protection).
- **GitLab**: SAST es el único scanner **free**; DAST/dependency/container/secret detection son **Ultimate**.
- **Codeberg**: nada equivalente encontrado — no es foco del proyecto (comunidad FOSS pequeña, no producto empresarial de seguridad).

**Relevancia gitquarry**: fuera de escala/foco actual — ninguno de los tres lo resuelve gratis salvo GitHub (que lo subsidia como producto empresarial GHAS) y GitLab lo cobra caro. No priorizar.

---

## 10. Arquitectura de frontend (JS del cliente)

- **GitHub**: Rails + partial-navigation (pjax/Turbo-style), React solo en superficies densas (Projects, parte de PR review). No hay reescritura completa a SPA confirmada.
- **GitLab**: Vue.js SPA sobre shell Haml, design system propio (`@gitlab/ui`/Pajamas), a11y forzada vía axe-linter en CI.
- **Codeberg/Forgejo**: mezcla vanilla-JS/Vue-lite con webpack, dirección explícita 2025-2026 hacia **más páginas JS-opcionales** (retrofit incremental de un frontend Gitea viejo, no rediseño desde cero).

**Relevancia gitquarry**: nuestra regla de "cero JS de cliente salvo un archivo" + CSP estricta es **más estricta que los tres**, incluido Codeberg (que solo va *hacia* JS-opcional de forma incremental, sin haberlo decidido como regla de diseño desde el día uno como gitquarry). Es una diferencia real de postura arquitectónica, no solo un detalle — vale la pena mantenerla como ventaja de seguridad/privacidad diferenciada, aceptando el costo ya documentado en `feature-compare.md` (renunciar a edición rica in-browser).

---

## Síntesis: qué mover en el roadmap propio a la luz de esta comparativa

No cambia el orden de impacto ya declarado en `feature-compare.md` (CI+webhooks+notificaciones > wiki+package-registry > OAuth2 provider), pero **matiza dos puntos concretos**:

1. **Issues relacionados (blocks/blocked-by) y bulk actions** suben de prioridad dentro del bloque de issues — son gratuitos y universales en los tres competidores, no requieren infra nueva (a diferencia de time-tracking o sub-issues jerárquicas, que sí son más caras de construir bien).
2. **CODEOWNERS/reviewers requeridos y status checks obligatorios en MR** son el par de features que, cuando se resuelva CI, hay que construir junto y no después — los tres forges los tratan como parte del mismo paquete de "merge gating", no como features separadas.

Dos ventajas reales confirmadas por esta investigación externa, no solo supuestas internamente:
- **Search cross-repo** de gitquarry ya adelanta a Codeberg en un punto que Codeberg mismo reconoce como no resuelto.
- **Federación de gitquarry** (4 capas) adelanta al estado actual de ForgeFed, el proyecto de federación más cercano en filosofía a gitquarry.
