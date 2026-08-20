# Split de la capa de presentacion: `gitquarry-webui` como crate separado, `gitquarry-server` como binario final

> **Implementado.** Workspace hoy: `crates/gitquarry-wire`,
> `crates/gitquarry-core` (lib puro, sin bin), `crates/gitquarry-webui`
> (presentacion), `crates/gitquarry-server` (paquete `gitquarry`, el binario
> que efectivamente se corre), `crates/gitquarry-federation`. Ver `git log`
> para el detalle de cada commit, no asumir que algo especifico de aca sigue
> pendiente sin verificar contra el codigo actual primero.

## Contexto

Objetivo: separar el frontend (web UI servida hoy con `maud`, sin JS/SPA)
del binario que sirve git (ssh://, git-smart-HTTP), para poder cambiar la
implementacion del frontend en el futuro (Dioxus, u otra) sin tocar el
codigo de git-serving. Decision del usuario: el nuevo crate se carga y
arranca junto con el resto — un boundary a nivel de compilacion, no un
segundo proceso desplegado (a diferencia de `gitquarry-federation`, que
habla por HTTP loopback justamente porque es opcional y se despliega
aparte).

## El ciclo de Cargo que esto forzo a resolver

`gitquarry-webui` necesita depender de `gitquarry-core` (tipos de
dominio/storage/auth). Si `gitquarry-core` a su vez dependiera de
`gitquarry-webui` (para que su propio binario pudiera arrancar el router
del webui), eso es un ciclo real: Cargo resuelve dependencias a nivel de
paquete, no de target — el `[dependencies]` de un paquete aplica a su lib
*y* su bin por igual, no hay forma de que solo el bin dependa de algo. Por
eso el binario final no puede vivir en el paquete de `gitquarry-core`.

Solucion: un tercer paquete, `gitquarry-server` (dir `crates/gitquarry-server`,
package name `gitquarry` — mismo nombre de binario que siempre tuvo), que
depende de los otros dos. Ahi vive `main.rs` (dispatch de subcomandos),
`serve.rs` (el `tokio::select!` que arranca ssh:// de core + web de webui
juntos) y `setup.rs` (el wizard de primer arranque, que tambien necesita
`esc`/`layout`/`non_empty` de webui para su form). Sigue siendo un solo
binario, un solo proceso — el boundary es de compilacion, no de deploy.

## Arquitectura final

- **`gitquarry-core`** (lib, sin bin): dominio (`domain::{repo,merge_request,
  release,remote_fork}`), storage, auth de negocio (`auth::{access,domain,
  oidc,password,totp,entities,federation_token}` — no `web`/`i18n`, eso es
  presentacion), git (ssh + smart-HTTP + `git::browse`), mirror, crypto,
  validate, i18n (`Locale`/`msg!`/catalogos — cross-cutting, lo usa
  cualquiera de los otros dos), config (sin `setup`), `commands`
  (fsck/repack/backfill), `internal_api`.
- **`gitquarry-webui`**: `webui/` completo, `auth::{web,i18n}` (paginas de
  login/seguridad/favoritos/perfil/usuarios/grupos — logica de negocio
  sigue en `gitquarry-core::auth`), `search/`, `settings/`. Depende
  solo de la API publica de `gitquarry-core`.
- **`gitquarry-server`**: `main.rs`, `serve.rs`, `setup.rs`. El unico lugar
  del workspace que depende de ambos — necesario para arrancar los dos
  juntos en un solo proceso.

## Cosas que hubo que mover/exponer para que el boundary sea real (no cosmetico)

- `storage::entities` y `auth::entities` eran `pub(crate)` pero la
  presentacion los tocaba directo (DTOs planos, sin logica) — bump a `pub`.
- `app::REQUEST_BACKEND` (task-local del cache por-request) y
  `AppContext::remote_for_uid` eran `pub(crate)`, tocados directo por
  `webui` — bump a `pub`.
- `webui::federation_client::resolve_token` (resolucion de token
  fine-grained para el Basic-Auth de git-smart-HTTP) se movio a
  `auth::federation_token` en `gitquarry-core`: `git::http` lo llama
  directo y no puede depender de un crate que a su vez depende de core.
  El resto de `federation_client` (lookups de browse/MR-proxy) se quedo en
  `webui`, es presentacion genuina.
- `i18n::msg!` (la macro que expande un catalogo a un `match Locale`) paso
  de `pub(crate) use` a `#[macro_export]` + `pub use` — cualquier catalogo
  de `auth::i18n`/`search::i18n`/`settings::i18n` ahora vive en
  `gitquarry-webui`, crate distinto de donde se define la macro.
- `webui::{esc, layout, non_empty}` pasaron de `pub(crate)` a `pub`: el
  wizard de setup (en `gitquarry-server`) los necesita para su propio form,
  y no tiene sentido duplicar el page-chrome completo (nav, CSS, i18n) para
  una pantalla de primer arranque.
