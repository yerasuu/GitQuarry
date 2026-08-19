# `gitquarry-ci-sdk` / `gitquarry_ci` — SDK de definición de pipelines (Rust + Python)

## Contexto

Extiende `gitquarry-ci` (ver `docs/gitquarry-ci.md`, Fase 9 "SDK ergonomics", ahí marcada como
paper-design). Este documento la promueve a diseño implementable y agrega un requisito nuevo no
cubierto en el original: soporte de primera clase para Ansible como mecanismo de despliegue.

**Premisa que no cambia respecto al documento base:** el SDK nunca ejecuta nada. Su única salida
es un `JobGraph` (JSON) volcado a stdout cuando el pipeline se invoca con `--dump-graph`.
`gitquarry-ci` sigue siendo el único ejecutor del sistema, sin importar qué SDK/lenguaje generó
el grafo.

**Decisión de arquitectura cerrada explícitamente en esta ronda de diseño:** se evaluó y se
descartó PyO3 (en cualquier dirección — embeber Python en Rust, o exponer Rust como extension
module de Python). Motivo: los macros `#[pymodule]`/`#[pyfunction]` expanden código `unsafe`
dentro de la propia unidad de compilación del crate, lo cual es incompatible con
`unsafe_code = forbid` a nivel de workspace — `forbid` no admite `#[allow(unsafe_code)]` local,
a diferencia de `deny`. No hay excepción parcial posible. Se evaluaron alternativas (CLI/subprocess
bridge, WASM, C ABI angosto en crate FFI separado) pero el requisito real del producto es
**"librería Python nativa, instalable con pip, con tipos y autocompletado"** — no "Rust reusado
desde Python" — por lo que la decisión final es: **dos implementaciones puras e independientes,
una por lenguaje, sin FFI entre ellas, sincronizadas por contrato de schema, no por código
compartido binario.**

## Decisiones de diseño (settled, no open)

| Pregunta | Decisión | Por qué |
|---|---|---|
| ¿El SDK ejecuta algo? | No, nunca. Solo construye y serializa un `JobGraph` a stdout. `gitquarry-ci` es el único ejecutor. | Invariante heredado del diseño base — un tercer "modo de ejecución" reintroduce el problema Actions/Woodpecker que todo el diseño evita. |
| ¿Rust y Python comparten binario? | No. Dos paquetes independientes (`gitquarry-ci-sdk` crate, `gitquarry_ci` paquete PyPI), cero FFI, cero PyO3. | `unsafe_code = forbid` es regla de workspace sin excepciones; el requisito de "librería Python nativa" no necesita FFI para cumplirse. |
| ¿Cómo se mantienen sincronizados? | JSON-schema versionado (`gitquarry-wire/schema/pipeline/v1/`) como única fuente de verdad, tipos generados en build-time en ambos lenguajes (no escritos a mano), más un test de contrato que corre ambos SDKs contra fixtures equivalentes y compara el JSON resultante. | Reemplaza la garantía que en otro diseño daría compartir código: la garantía viene de un test automatizado, no de un binario común. |
| Validación de grafos (ciclos, ids duplicados, `needs` inexistentes) | Reimplementada de forma independiente en cada lenguaje (DFS con coloreo blanco/gris/negro). Rust además comparte el validador con el propio servidor `gitquarry-ci` vía crate `gitquarry-graph-validate` (Rust↔Rust únicamente, nunca cruza a Python). | El algoritmo es barato de duplicar (un puñado de líneas); no vale introducir FFI para ahorrarse esto. |
| Secrets en el SDK | Único método de entrada: `.secret(env_key, secret_name)` en ambos lenguajes. No existe ningún método que acepte el *valor* de un secreto — solo la referencia. El valor nunca pasa por el proceso de resolución. | Imposible-por-construcción, no por convención: no hay API alternativa para violar la regla por accidente. |
| Ansible | No es un tercer lenguaje de pipeline ni un modo de ejecución nuevo. Es un builder ergonómico (`AnsibleJobBuilder`) que termina produciendo un `JobBuilder`/`JobSpec` normal — mismo contrato, mismo executor. | Consistente con "un solo motor de ejecución": Ansible es contenido de un job, no un concepto nuevo del sistema. |
| Jobs de despliegue | Nuevo campo `kind: JobKind` (`build \| test \| deploy`, default `build`) en `JobSpec`. Todo job `kind: deploy` requiere aprobación explícita de un maintainer antes de correr, generalizando el gate de aprobación que el diseño base ya define para secrets en fork-PRs — ahora aplica a cualquier push, opt-out explícito por repo, no opt-in. | Evita el error común de deploys automáticos sin gate; reutiliza el mecanismo de aprobación existente en vez de inventar uno nuevo. |
| Empaquetado Python | Cero dependencias externas (`pyproject.toml` con `dependencies = []`). Python estándar puro, instalable en cualquier imagen `python:3-slim` sin build step. | Coherente con la decisión original de correr pipelines Python como subprocess simple; nada que compilar por plataforma. |
| Builder mutable vs. inmutable | Mutable (`self` mutado y retornado) en ambos lenguajes. | El resultado vive segundos (un proceso que corre, dumpea y muere); inmutabilidad agrega ceremonia sin beneficio real acá. |
| Contrato de versión | `schema_version` explícito (`const: 1` en v1), `additionalProperties: false` en todo el schema. Un cambio incompatible implica un nuevo `v2/`, nunca mutar `v1` in-place. | Evita drift silencioso entre SDKs de distinta versión; el servidor declara qué rango de versiones acepta. |

## Estructura de repositorio

```
gitquarry-wire/schema/pipeline/v1/
├── job-graph.schema.json
├── job-spec.schema.json
├── job-kind.schema.json
├── secret-ref.schema.json
└── pipeline-trigger.schema.json

crates/gitquarry-graph-validate/     (Rust, compartido: SDK Rust + servidor gitquarry-ci)

crates/gitquarry-ci-sdk/             (Rust, publicado a crates.io)
├── build.rs                         (genera wire_generated.rs desde el JSON-schema vía typify)
├── src/
│   ├── lib.rs
│   ├── job.rs                       (JobBuilder)
│   ├── graph.rs                     (GraphBuilder, validate(), dump())
│   ├── context.rs                   (PipelineContext)
│   ├── kind.rs                      (impl Default para JobKind)
│   ├── ansible.rs                   (AnsibleJobExt, AnsibleJobBuilder)
│   └── wire_generated.rs            (generado, no editado a mano)

python/gitquarry-ci/                 (Python, publicado a PyPI)
├── pyproject.toml                   (dependencies = [])
├── scripts/generate_wire_types.sh   (datamodel-codegen sobre el mismo JSON-schema)
└── gitquarry_ci/
    ├── __init__.py
    ├── job.py                       (JobBuilder)
    ├── graph.py                     (GraphBuilder)
    ├── validate.py                  (GraphError, validate_graph, _detect_cycles)
    ├── context.py                   (PipelineContext, context())
    ├── ansible.py                   (AnsibleJobBuilder, ansible_playbook())
    └── wire_generated.py            (generado, no editado a mano)

gitquarry-ci-sdk-contract-tests/     (test de contrato, ver Fase 5)
├── fixtures/
│   ├── simple.expected.json
│   ├── monorepo-dynamic.expected.json
│   └── ansible-deploy.expected.json
├── rust_fixture_runner/
└── python_fixture_runner/
```

## API pública de referencia (Python — la superficie que ve el usuario final)

```python
from gitquarry_ci import job, graph, context, JobKind
from gitquarry_ci.ansible import ansible_playbook

ctx = context()

g = graph()
for path in ctx.changed_paths():
    if path.startswith("crates/"):
        crate = path.split("/")[1]
        g.add_job(
            job(f"test-{crate}")
            .image("rust:1.80")
            .run(f"cargo test -p {crate}")
        )

g.add_job(
    ansible_playbook(job("deploy").needs("test-*").kind(JobKind.DEPLOY), "site.yml")
    .inventory("inventory/prod")
    .vault_password_secret("ansible-vault-pass")
    .finish()
)

g.dump()
```

El equivalente Rust (`.image().run().needs()` encadenado, `AnsibleJobExt::ansible_playbook`) debe
producir el mismo `JobGraph` para el mismo pipeline lógico — esto es exactamente lo que el test de
contrato de la Fase 5 verifica.

## Implementación

### Fase 1 — Wire schema (`gitquarry-wire/schema/pipeline/v1/`)

- Escribir los 5 archivos JSON-schema (`job-graph`, `job-spec`, `job-kind`, `secret-ref`,
  `pipeline-trigger`), todos con `additionalProperties: false`.
- `job-spec.schema.json` incluye el campo nuevo `kind` (`enum: [build, test, deploy]`,
  `default: build`) que no existía en el diseño base — necesario para el gate de deploy.
- `job-graph.schema.json` incluye `schema_version` como `const: 1`.
- Sin código todavía — este schema es la única fuente de verdad para ambos SDKs.

### Fase 2 — `gitquarry-graph-validate` (Rust, compartido SDK↔servidor)

- Extraer/crear el validador de grafos como crate independiente: `check_unique_ids`,
  `check_needs_exist`, `check_no_cycles` (DFS blanco/gris/negro), `check_nonempty_image`,
  `check_resource_bounds`.
- Este crate lo consumen `gitquarry-ci-sdk` (Rust) y el servidor `gitquarry-ci` (Fase 3 del
  documento base). Python **no** lo consume — reimplementa el mismo algoritmo en `validate.py`.
- Tests unitarios directos sobre cada check, sin necesidad de contenedor.

### Fase 3 — `gitquarry-ci-sdk` (Rust)

- `build.rs` genera `wire_generated.rs` desde el JSON-schema vía `typify` en cada build del
  crate — nunca editar ese archivo a mano.
- `job.rs`: `JobBuilder` con `#[must_use]` en el struct (evita el bug de construir un job y
  olvidar `add_job`). Único método de entrada para datos sensibles: `secret(env_key, secret_name)`.
- `graph.rs`: `GraphBuilder` con `add_job`, `for_each_dir` (azúcar para monorepos), `validate()`
  (usa `gitquarry-graph-validate`), `dump() -> !` (serializa a stdout, valida antes de dumpear,
  `exit(1)` en error, `exit(0)` en éxito — nunca retorna).
- `context.rs`: `PipelineContext` poblado desde env vars (`GITQUARRY_CI_SHA`,
  `GITQUARRY_CI_REFNAME`, `GITQUARRY_CI_PUSHER`, `GITQUARRY_CI_REPO_UID`,
  `GITQUARRY_CI_CHANGED_PATHS_JSON`, `GITQUARRY_CI_IS_FORK_PR`) — panic con mensaje explicativo
  si falta alguna (bug de `gitquarry-ci` inyectando el entorno, no condición recuperable por el
  autor del pipeline). Sin ningún método de I/O de red.
- `ansible.rs`: `AnsibleJobExt` (extension trait sobre `JobBuilder`) + `AnsibleJobBuilder` con
  `inventory`, `extra_var`, `vault_password_secret`, `tags`, `finish() -> JobBuilder`. El vault
  password se inyecta como `.secret("GITQUARRY_VAULT_PASS", secret_name)` y se referencia en el
  comando via `--vault-password-file $VAULT_PASS_FIFO` (no como env var plana ni arg de CLI en
  texto — `ansible-vault` espera archivo). Usar una función `shell_escape` propia (shlex-style)
  para todo valor derivado de `PipelineContext` (ej. `ctx.sha()`) antes de interpolarlo en el
  comando.
- `unsafe_code = forbid` a nivel de crate, sin excepción.

### Fase 4 — `gitquarry_ci` (Python)

- `scripts/generate_wire_types.sh` corre `datamodel-codegen` sobre el mismo JSON-schema de la
  Fase 1, genera `wire_generated.py` (dataclasses + `JobKind(str, Enum)`). Se commitea el
  resultado al repo; un test de CI verifica que está actualizado respecto al schema fuente
  (falla si `datamodel-codegen` regenerado difiere del commiteado).
- `job.py`: `JobBuilder` equivalente 1:1 al de Rust. Usar `__del__` con warning a stderr como
  aproximación en tiempo de ejecución al `#[must_use]` de Rust (Python no tiene chequeo estático
  de este tipo).
- `validate.py`: reimplementación independiente del mismo algoritmo DFS (blanco/gris/negro) que
  `gitquarry-graph-validate`. `MAX_JOBS` debe coincidir numéricamente con el bound de
  `check_resource_bounds` en Rust — cubierto por el test de contrato de la Fase 5, no por
  compartir constante.
- `graph.py`: `GraphBuilder` con `add_job`, `for_each_dir`, `validate()`, `dump()` (serializa
  dataclasses anidadas + enums a dict plano vía `_to_dict`, luego `json.dumps` a stdout,
  `sys.exit(1)`/`sys.exit(0)`).
- `context.py`: función `context()` (no clase con factory estático — más idiomático en Python),
  mismas env vars que el lado Rust, mismo comportamiento de error si falta alguna.
- `ansible.py`: `AnsibleJobBuilder` + función libre `ansible_playbook(job_builder, path)` (Python
  no tiene extension traits; esta es la forma idiomática equivalente). Mismo tratamiento del
  vault password (fifo, no env var plana) y `shlex.quote` para interpolación segura.
- `pyproject.toml`: `dependencies = []`, `requires-python = ">=3.11"`. Cero dependencias es
  requisito de diseño, no accidente — nada de PyO3, nada de bindings nativos, nada de wheels
  compiladas por plataforma.
- `__init__.py` exporta exactamente: `job`, `JobBuilder`, `graph`, `GraphBuilder`, `context`,
  `PipelineContext`, `JobKind`, `GraphError`.

### Fase 5 — Test de contrato (`gitquarry-ci-sdk-contract-tests/`)

- Para cada fixture (`simple`, `monorepo-dynamic`, `ansible-deploy`): un `pipeline.rs` y un
  `pipeline.py` que expresan el mismo pipeline lógico.
- El corredor ejecuta ambos con `--dump-graph`, contra el mismo `PipelineContext` mockeado
  (mismas env vars inyectadas a los dos procesos).
- Cada output se valida contra `job-graph.schema.json` (rechazo duro si hay campos extra o
  faltantes — `additionalProperties: false` hace esto automático).
- Ambos outputs se comparan entre sí normalizados (orden de `jobs` no importa; orden de
  `commands` dentro de un job sí importa) — mismatch falla el test.
- Corre en CI en cada PR que toque `gitquarry-ci-sdk/`, `python/gitquarry-ci/`, o
  `gitquarry-wire/schema/pipeline/`.

### Fase 6 — Extensión al servidor `gitquarry-ci` (cambios sobre fases ya existentes del diseño base, no fases nuevas)

- **Wire schema (Fase 2 del doc base):** agregar `kind: JobKind` a `JobSpec` — campo opcional
  con default `build`, no rompe compatibilidad con grafos ya emitidos antes de este cambio.
- **Validación del grafo (Fase 3 del doc base):** si `kind == deploy` y el repo no tiene el gate
  de deploy desactivado explícitamente en `RepoConfig`, el job se crea en estado
  `pending_approval` en vez de encolarse directamente — mismo punto del código donde hoy se
  aplica la regla de secrets-para-forks, generalizada para cubrir también deploys.
- **Web UI (Fase 6 del doc base):** el componente de aprobación ya existente para "fork-PR
  secrets" pasa a listar también jobs `deploy` pendientes de cualquier push — un solo componente,
  dos triggers de aparición.
- **Imagen de referencia:** publicar y documentar `gitquarry-ci-ansible:<version>` (Ansible +
  collections comunes preinstaladas) como convención recomendada, no forzada.

### Explícitamente fuera de alcance

- Orquestación de rollback automático post-deploy — responsabilidad del propio playbook/rol
  Ansible, no de `gitquarry-ci` ni del SDK.
- Gestión de inventory dinámico (consultas a cloud providers) — vive dentro del contenedor del
  job, el SDK no lo modela.
- Cualquier builder equivalente para otras herramientas de deploy (Terraform, Pulumi, etc.) —
  mismo patrón que Ansible (módulo aparte, `finish() -> JobBuilder`) es aplicable a futuro pero
  no se diseña ni se implementa ahora.
- Compartir código en runtime entre el SDK Rust y el SDK Python (FFI, PyO3, WASM, subprocess
  bridge) — decisión cerrada, ver sección de Contexto.

## Archivos críticos (nuevos/tocados)

- `gitquarry-wire/schema/pipeline/v1/{job-graph,job-spec,job-kind,secret-ref,pipeline-trigger}.schema.json` (nuevo)
- `crates/gitquarry-graph-validate/src/lib.rs` (nuevo)
- `crates/gitquarry-ci-sdk/{build.rs,src/{lib,job,graph,context,kind,ansible,wire_generated}.rs}` (nuevo crate)
- `python/gitquarry-ci/{pyproject.toml,scripts/generate_wire_types.sh,gitquarry_ci/{__init__,job,graph,validate,context,ansible,wire_generated}.py}` (nuevo paquete)
- `gitquarry-ci-sdk-contract-tests/{fixtures/*.expected.json,rust_fixture_runner/,python_fixture_runner/}` (nuevo)
- `crates/gitquarry-wire/src/pipeline.rs` (tocado: agregar `kind: JobKind` a `JobSpec`)
- `crates/gitquarry-ci/src/graph.rs` (tocado: aplicar gate `pending_approval` a jobs `kind: deploy`)
- `crates/gitquarry-webui/src/webui/ci/{mod,detail}.rs` (tocado: listar deploys pendientes junto a secrets de fork-PR)

## Verificación

- `cargo test` en `gitquarry-graph-validate` y `gitquarry-ci-sdk` — validación de grafos y
  builders sin necesidad de contenedor.
- `pytest` en `python/gitquarry-ci` — mismos casos de validación reimplementados en Python.
- Test de contrato (Fase 5) corriendo en CI en cada cambio a cualquiera de los dos SDKs o al
  schema — bloqueante, no informativo.
- `cargo clippy --all` / `cargo fmt --check`, `unsafe_code = forbid` sin excepción sobre
  `gitquarry-ci-sdk` y `gitquarry-graph-validate`.
- `mypy`/`ruff` sobre `python/gitquarry-ci` — sin dependencias que compilar, el chequeo de tipos
  corre directo sobre Python puro.
- Manual: pipeline con job `kind: deploy` sin gate desactivado en `RepoConfig` → confirmar que
  queda en `pending_approval` y no se encola; con gate desactivado → confirmar que corre directo.
- Manual: pipeline Ansible con vault password vía `.secret()` → confirmar que el valor nunca
  aparece en logs (mismo mecanismo de redacción por substring-scanning del documento base) ni en
  el `JobGraph` serializado (solo debe aparecer `secret_name`, nunca el valor).
- Commit por fase, un cambio lógico por commit — mismo criterio que el documento base.
