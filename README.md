# GitQuarry

Usa S3 (o cualquier backend S3-compatible: MinIO, R2, etc.) como remoto git.
Un solo binario (`gitquarry`) arranca en el mismo proceso un server SSH
propio (`ssh://`, con auth por clave publica y permisos por repo), git
smart-HTTP sobre el mismo puerto web (push/clone/fetch con auth basica y
permisos por repo) y una UI web de solo lectura, todos sobre el mismo
bucket. Sirve **multiples repos** (uno por path pedido). Cualquier cliente
usa `git clone http://host:puerto/repo` o `git clone ssh://user@host:puerto/repo`
normal — sin instalar nada especial, solo el `git` de siempre. `ssh://` y
http son los unicos dos transportes publicos soportados: no hay daemon
`git://` anonimo ni remote helper directo contra el bucket.

## Como funciona

### server unificado (ssh:// + http + UI web, multi-repo)

- Un server = un bucket base. Cada path que pida un cliente es un repo
  distinto: `git clone http://host/proyectoA` guarda en
  `<bucket>/repos/proyectoA`, `git clone ssh://host/equipo/proyectoB` en
  `<bucket>/repos/equipo/proyectoB`. Sin declarar nada de antemano: el
  primer push a un path nuevo crea el repo. Todo repo vive bajo el prefijo
  `repos/` (nunca en la raiz del bucket): asi el namespace de un repo
  (usuario o grupo, bastante libre) nunca puede colisionar con `.conf/` u
  otra carpeta reservada de la raiz — viven en prefijos disjuntos por
  construccion, no por una lista de nombres prohibidos.
- Por cada repo mantiene un mirror bare local (`git init --bare`, bajo
  `--mirrors-dir`) sincronizado con S3, compartido entre ssh://, git
  smart-HTTP y la UI web.
- Los mirrors son cache, no estado: un barrido periodico (cada 15min) los
  limpia con dos limites independientes — `--mirrors-max-age` (default `3d`,
  evita el que no se consulto en ese tiempo) y `--mirrors-max-size` (default
  `5G`, si el directorio sigue por encima borra los menos usados hasta bajar
  del limite). Se recrean solos desde S3 en la proxima consulta. El barrido
  respeta el lock por repo: nunca borra un mirror que otra request esta
  usando en ese momento.
- En cada conexion git (clone/fetch/push) o request web primero baja de S3
  los packs que falten y actualiza refs/HEAD del mirror para que coincidan
  con `refs.json`.
- Para push/fetch exec'a el `git upload-pack` / `git receive-pack` real del
  sistema (canal SSH de por medio en ssh://, `--stateless-rpc` sobre HTTP en
  el otro transporte) — la negociacion del protocolo (thin packs, shallow,
  etc.) la hace git, no gitquarry.
- Un push que git acepta localmente dispara el hook `pre-receive` (instalado
  automaticamente en cada mirror), que sube los packs nuevos a S3 y actualiza
  `refs.json` con CAS. Si otro host movio el mismo ref en S3 desde el ultimo
  sync, el hook rechaza el push (`hace fetch y reintenta`) en vez de pisarlo.
- El rechazo de pushes non-fast-forward (sin `--force`) lo hace el propio
  git cliente/server via la negociacion normal de refs — gitquarry no
  reimplementa esa logica.
- La UI web (server-rendered, sin JS, estilo cgit) lee del mismo mirror.
  Vistas por repo, todas via `?view=`: `summary` (default: description, owner,
  clone URLs, README renderizado si es markdown, branches/tags recientes,
  commits recientes), `refs` (todos los branches y tags), `tree` (con links a
  submodulos cuando hay `.gitmodules`), `blob` (syntax highlighting via
  `syntect`, numeros de linea con anchor `#Lnn`, imagenes conocidas inline),
  `log` (busqueda por `?q=` via `git log --grep`, paginacion por `?skip=`,
  decorate refs), `commit` (diffstat + diff coloreado + links a patch/rawdiff),
  `stats` (commits por autor y por mes), `blame`, y settings del repo
  (description/owner). Exports sin HTML: `plain` (blob crudo), `patch`
  (`git format-patch`), `rawdiff`, `snapshot` (`.tar.gz`/`.zip` via
  `git archive`) y `atom` (feed de commits). El index (`/`) tiene busqueda por
  nombre y agrupa repos por namespace (`equipo/proyecto` bajo un header
  `equipo`). Solo sirve repos que ya existen en el bucket: pedir un path
  inventado da 404 y no crea nada en disco.
- Cada repo tiene un lock en el proceso: el mirror local es estado compartido,
  asi que un clone, un push y un render de la UI **del mismo repo** se
  serializan entre si. Repos distintos siguen andando en paralelo.
- La UI web de solo lectura no tiene cifrado de transporte propio (sin TLS
  adelante) — pensada para localhost o red confiable, o detras de un proxy
  con TLS/auth. Los dos transportes de push/fetch (ssh:// y git smart-HTTP)
  si requieren identidad real (clave publica / password) y validan permiso
  por repo antes de ejecutar nada.
- **Visibility por repo** (estilo Nest de pijul, `public`/`private`, default
  `public`): un repo `private` no aparece en el index y da 404 en cualquier
  vista de la UI web. `ssh://` no cambia por esto: ya requiere una entrada
  explicita en `.conf/access.json` por clave y por repo sin importar
  visibility, asi que ya es "privado" donde eso importa (quien puede
  clonar/pushear). Git smart-HTTP tampoco: mismo requisito de permiso
  explicito, salvo una excepcion — clonar/fetch de solo lectura de un repo
  `public` no pide credenciales, igual que la UI web anonima (nunca push, y
  nunca en un repo que no sea `public`). Se edita en la pestaña "settings"
  de cada repo, que es la unica vista que ignora este chequeo (si no, un
  repo puesto en `private` no se podria volver a hacer publico). Con login
  configurado (ver mas abajo), un usuario logueado con acceso de lectura (o
  `is_admin`, que ve todo) puede ver el repo `private` igual en la web.

### server SSH (`ssh://`, con auth por pubkey y permisos por repo)

- Servidor SSH propio embebido en el mismo proceso (no depende del `sshd`
  del sistema ni de usuarios del SO): pensado para poder dockerizarse sin
  perder identidad ni config entre reinicios.
- Auth 100% por clave publica, sin password: el username de la url
  (`ssh://cualquiera@host:puerto/repo`) se ignora, la identidad es la clave
  resuelta a un **usuario**.
- Modelo de usuarios (no claves sueltas): cada usuario tiene nombre, email
  opcional, una o mas claves SSH, permisos por repo (`read`/`write`, `"*"`
  como default para cualquier repo no listado) y, opcionalmente, una
  password (para login por password en la web) y un flag `is_admin` (acceso
  total a la UI web, ver seccion de login mas abajo). Los permisos cuelgan
  del usuario, no de una clave individual — perder un dispositivo es
  sacarle esa clave al usuario (`/_users` en la UI web), no volver a
  otorgar cada repo a una clave nueva. Una clave pertenece a un solo usuario
  (rechaza agregarla dos veces). `is_admin`/password son cosas de la UI web
  nomas: `ssh://` sigue siendo 100% por clave, sin passwords, gatea por
  `repos` sin importar `is_admin`. Guardado en `.conf/access.json` dentro
  del bucket.
- La host key del server tambien vive en el bucket
  (`.conf/ssh_host_key.json`, generada sola la primera vez), no en disco —
  mismo motivo que el punto anterior: no perderla en un contenedor nuevo.
- Reusa la misma logica de spawn de `git upload-pack`/`git receive-pack` y
  el mismo hook `pre-receive` que git smart-HTTP; solo cambia el transporte
  (canal SSH) y que antes de ejecutar nada se valida el permiso de la clave
  sobre ese repo.

### git smart-HTTP (mismo puerto que la UI web, con auth por permisos por repo)

- No es un servidor aparte: es una capa de middleware sobre el mismo router
  axum que sirve la UI web (`--web-bind`), que intercepta exactamente
  `GET .../info/refs?service=...`, `POST .../git-upload-pack` y
  `POST .../git-receive-pack` antes de que cualquier ruta de la UI de
  lectura los vea, y deja pasar todo lo demas sin tocarlo — nunca comparte
  codigo con las vistas HTML de la UI (ni al reves).
- Auth por HTTP Basic (`git clone http://usuario@host/repo`, git pide la
  password en el prompt) contra el mismo `password_hash` que el login web
  usa — no hay un tipo de credencial separado para esto. Clonar/fetch de un
  repo `public` no pide nada (mismo criterio que la UI web anonima); todo lo
  demas (push, o leer un repo `limited`/`private`) exige credenciales
  validas y el permiso correspondiente (`read` para clonar, `write` para
  pushear) via el mismo `.conf/access.json` que `ssh://` usa. Sin
  credenciales en un pedido que las necesita: `401` con
  `WWW-Authenticate: Basic`. Con credenciales pero sin el permiso que hace
  falta: `403`.
- Igual que `ssh://`, exec'a `git upload-pack`/`git receive-pack` reales del
  sistema (en modo `--stateless-rpc`, la variante de un pedido/respuesta por
  llamada que el protocolo HTTP necesita) y dispara el mismo hook
  `pre-receive` en cada push.

### Login para la UI web (opcional): OIDC y/o password por usuario

Dos formas de entrar, independientes entre si, que resuelven a la misma
identidad: un `User` de `.conf/access.json` (el mismo modelo que `ssh://`
usa para pubkeys/permisos, no una tabla de identidades aparte).

- **OIDC**: desactivado por default hasta que los 4 campos
  `oidc_issuer_url`/`oidc_client_id`/`oidc_client_secret`/
  `oidc_redirect_base_url` se configuran juntos en `/_settings` (van los 4 o
  ninguno) y se reinicia el server (discovery contra el issuer corre una vez
  al arrancar). El `email` que verifica el provider (claim `email`, respeta
  `email_verified` si lo manda) se resuelve contra `email` de un `User`
  existente. Sin `User` con ese email, queda como si no hubiera logueado.
- **Password**: por usuario, sin nada global que activar — se setea desde
  `/_users` (campo `password`, hasheado con Argon2id antes de guardarse,
  nunca en claro). Cualquier usuario puede tener una, no solo un admin;
  `/_login` muestra el formulario usuario/password apenas exista al menos
  uno con password seteada (mas un link a OIDC si tambien esta configurado).
  La misma password sirve para el login web y para HTTP Basic contra git
  smart-HTTP (ver seccion de arriba) — una sola credencial, no dos.
- **`is_admin`** (por usuario, en `/_users`, independiente de tener o no
  password/OIDC): acceso total a `/_settings`, `/_users`, el tab "settings"
  de cada repo, y cualquier repo `private` sin importar `repos`/permisos.
  Sin `is_admin`, un usuario logueado (por cualquiera de los dos metodos)
  solo ve los repos `private` a los que `permission_for` le da acceso.
- El gate de admin distingue tres casos: nada configurado (ni OIDC ni ningun
  `User` con password) → `/_settings`/`/_users`/tab "settings" siguen
  exactamente tan abiertos como siempre; logueado pero sin `is_admin` → 403
  en vez de un loop de vuelta al login; sin sesion → redirect a `/_login`.
- **Sin sesion server-side ni DB**: la cookie (uno u otro metodo) va sellada
  con el mismo `Cipher` que cifra todo lo demas (XChaCha20Poly1305, AAD
  `"web-session"` — distinto del AAD de cualquier objeto del bucket),
  `HttpOnly`+`SameSite=Lax`, 7 dias. Solo prueba una identidad verificada
  "hace poco" (el IdP para OIDC, la password al momento del login para
  password); `access.json` se re-lee en cada request, asi que borrar el
  `User`, su email, o su `password_hash` corta el acceso al toque aunque la
  cookie siga viva. Nada de esto sobrevive solo en memoria — un restart no
  desloguea a nadie ni deja sesiones huerfanas para limpiar.
- El index (`/`) no cambia: sigue listando solo repos `public`. Un repo
  `private` al que el usuario logueado tiene acceso es alcanzable por URL
  directa pero no aparece listado — limitacion deliberada, ver "Estado /
  limitaciones conocidas".
- Sin `axum-extra`/crate de cookies: parsear `Cookie:`/armar `Set-Cookie:`
  es un par de `split`s, hecho a mano (`src/web/mod.rs`) — el contenido de esas
  cookies si tiene cripto real, via el `Cipher` de siempre.

### Cifrado del lado de la API

El bucket S3 es un intermediario de storage, no un participante de confianza:
todo lo que sube el proceso se cifra ANTES de salir del proceso, con
XChaCha20Poly1305 (AEAD). El proveedor S3 nunca ve texto plano ni la clave.

- **Que se cifra**: todo el contenido — packs (`objects/pack/*.pack`/`.idx`),
  `refs.json`, `.conf/global.json`, `.conf/access.json`,
  `.conf/ssh_host_key.json` (la clave PRIVADA del server SSH), `<repo>/.conf/config.json`.
  Lo unico que queda en claro son los **nombres** de clave S3 (se ve que existe
  un repo `proyectoA` y cuantos packs tiene, no su contenido): hacen falta para
  listar/sincronizar sin poder descifrar.
- **De donde sale la clave**: de `passphrase` en `gitquarry.toml` (ver
  Configuracion), derivada con Argon2id (parametros OWASP de bajo consumo:
  19 MiB / 2 iteraciones / 1 hilo, fijos, no configurables) mas un salt
  aleatorio por bucket. El salt SI se guarda sin cifrar en
  `.conf/encryption.json` (primer uso del bucket, generado solo): un salt no
  es secreto, y sin la passphrase no le sirve a nadie para nada.
- **Verificacion temprana**: junto al salt se guarda un valor testigo cifrado.
  Si la `passphrase` configurada no coincide con la del bucket, el server no
  arranca con un error claro, en vez de fallos de descifrado dispersos en
  medio de un push.
- **Atado a la clave S3**: cada objeto cifrado usa su propia clave S3 (ej.
  `repos/proyectoA/refs.json`) como AAD. Mover el ciphertext de un objeto a otra
  clave (un proveedor de storage malicioso, o un bug) hace que el descifrado
  falle: no se puede hacer pasar el contenido de un objeto por el de otro.
- **Packs cifrados en streaming, memoria acotada**: subir/bajar/mover un pack
  (push, clone, `repack`, fork, mover de owner) cifra/descifra en chunks
  fijos (`crypto::Cipher::seal_stream`/`open_stream`), nunca bufferea el pack
  entero en RAM sin importar su tamaño — cada chunk lleva su propio nonce y
  un AAD que ata la clave del objeto + el largo total + el indice del chunk,
  asi que reordenar/truncar/duplicar chunks se detecta igual que un tamper en
  el formato de un solo golpe. A cambio, cada subida/bajada usa
  transitoriamente ~2-3x el tamaño del pack en **disco** (un temporal cifrado
  ademas del plano, y en fork/mover-de-owner un tercero para el re-cifrado) —
  si `/tmp` es tmpfs en tu deploy, ese margen si vive en RAM, tenelo en
  cuenta. Objetos chicos (refs.json, usuarios, settings, audit) siguen con el
  cifrado de un solo golpe de siempre, sin este trade-off.
- **Formato sin compatibilidad hacia atras**: el cifrado en streaming de
  arriba es un formato nuevo, no aditivo — un pack ya subido con el formato
  anterior a esta feature queda indescifrable despues de actualizar (error
  claro, no corrupcion silenciosa). `repack` en un repo existente lo
  resuelve solo (sube los packs consolidados con el formato nuevo); si no,
  hace falta un re-push desde un clone fresco.
- **La `passphrase` es indispensable y no tiene reset**: si se pierde, todo lo
  ya cifrado con ella queda irrecuperable — no hay backdoor ni mecanismo de
  reset. Guardala en un lugar seguro, separado del propio repo/config.

### Validacion de entrada

Todo lo que llega de un cliente (path de repo, `?ref=`, `?path=`, `?sha=`)
termina siendo un path de filesystem, una clave de S3 y un argumento de `git`,
asi que se valida en el borde (`src/validate.rs`) antes de tocar nada:

- Los paths de repo se normalizan y se rechazan si tienen `..`, segmentos
  vacios, caracteres de control o empiezan con `-`. El path del mirror se
  vuelve a chequear contra `--mirrors-dir` como red de seguridad.
- Los revs/shas van contra un charset conservador y nunca pueden empezar con
  `-`; ademas todo rev que venga de afuera se pasa a git despues de
  `--end-of-options`, para que no se interprete como flag.
- Todo lo que se imprime en el HTML pasa por escape; los blobs de mas de 1 MiB
  no se renderizan.

### settings por UI, guardadas en el bucket

Toda la config que vive dentro del bucket cuelga de una carpeta reservada
`.conf/`, en dos niveles (similar a la config global vs. por-repo de
forgejo/pijul):

- **Global** (`/_settings` en la UI, organizado en secciones: red, cache de
  mirrors, login OIDC): binds (`web-bind`/`ssh-bind`),
  mirrors-dir/mirrors-max-age/mirrors-max-size, y los 4 campos OIDC
  (`oidc_issuer_url`/`oidc_client_id`/`oidc_client_secret`/
  `oidc_redirect_base_url`, ver seccion de arriba). Se guarda en
  `.conf/global.json`, en la raiz del bucket.
- **Usuarios** (`/_users` en la UI): el mismo `.conf/access.json` que
  `ssh://` usa para pubkeys/permisos, mas `password`/`is_admin` por usuario
  (ver seccion de login de arriba).
- **Por repo** (pestaña "settings" de cada repo en la UI): `description`,
  `owner` y `visibility` (`public`/`private`, ver seccion de arriba). Se
  guarda en `<repo>/.conf/config.json`, junto al resto de los objetos de ese
  repo.

Ademas, tambien bajo `.conf/` pero gestionados aparte (`/_users`): los
usuarios, sus claves SSH y sus permisos por repo (`.conf/access.json`) y la
host key del server SSH (`.conf/ssh_host_key.json`, ver seccion de arriba).

Todo esto usa compare-and-swap con ETag, igual que `refs.json`. Detalles:

- El bootstrap (bucket + credenciales, en `gitquarry.toml`/config global)
  siempre tiene que vivir en disco local — no hay forma de leer del bucket
  las credenciales para llegar al bucket.
- Precedencia al arrancar: flag de CLI explicito > lo guardado en
  `.conf/global.json` > default hardcodeado.
- Los binds/mirrors-dir se leen una vez al arrancar (son sockets ya
  bindeados) — cambiarlos desde la UI aplica recien al reiniciar el proceso.
- `.conf/` vive en la raiz del bucket; todo repo vive bajo `repos/`. Prefijos
  disjuntos por construccion — no hace falta evitar `.conf` como nombre de
  usuario/grupo/repo, no hay forma de que colisione. Buckets creados antes
  de este cambio (repos sueltos en la raiz) necesitan correr
  `gitquarry migrate-repos-prefix --apply` una vez para moverlos a `repos/`
  (re-encripta cada objeto bajo su key nueva, ver "atado a la clave S3"
  arriba; dry-run sin `--apply`).

## Build e instalacion

Un solo binario (`gitquarry`) sirve todos los modos: server unificado
(default), `pre-receive-hook` (invocado por git, no por un humano), `fsck`,
`repack` y `backfill-repo-index` (mantenimiento, humano).

```sh
cargo build --release
install -m755 target/release/gitquarry ~/.local/bin/gitquarry
```

## Configuracion

**Primer arranque sin config**: si ninguno de los 4 candidatos de abajo existe
como archivo, `gitquarry` no falla: sirve un wizard de setup de una sola
pagina en `--web-bind` (default `127.0.0.1:8080`). Ahi se completa una vez
el nombre de la entrada, el driver de storage (`s3`/`r2`/`azure`/`gcs`/
`local`) con sus credenciales, la `passphrase` de cifrado y un usuario admin
para la UI web — sin JavaScript, la pagina muestra los campos de los cinco
drivers juntos (solo se leen los del driver elegido); al enviar el form se
verifica todo contra el backend de verdad (`get_or_create_cipher`, que
tambien inicializa el cifrado si el bucket es nuevo) antes de escribir nada
— una credencial o passphrase mala vuelve a mostrar el form con el error, no
deja un `gitquarry.toml` roto en disco. Si pasa, escribe `gitquarry.toml`
(permisos `600`), siembra `.conf/access.json` con ese usuario (`is_admin =
true`, solo si el bucket todavia no tenia usuarios — no pisa un bucket ya
inicializado), y el mismo proceso sigue directo al arranque normal (ssh://,
http, UI web) sin reiniciar. `ssh://` no se bindea durante el wizard, solo
la UI web
temporal en `--web-bind` (que ya sirve http una vez el arranque normal
empieza).

Orden de resolucion (primer match gana):

1. `GITQUARRY_CONFIG=/ruta/a/config.toml` (override explicito).
2. `gitquarry.toml` en el directorio actual.
3. `gitquarry.toml` en la raiz del repo git (`git rev-parse --show-toplevel`),
   funciona aunque corras el comando desde un subdirectorio.
4. `~/.config/gitquarry/config.toml` (config global, `$XDG_CONFIG_HOME` si esta seteado).

Recomendado: `gitquarry.toml` en la raiz del proyecto que corre el server,
agregado a `.gitignore` (tiene credenciales). Asi la config queda controlada
junto al proyecto en vez de perdida en el home del usuario.

Formato del archivo:

```toml
# AWS S3 real
[buckets.mi-bucket]
driver = "s3"
bucket = "mi-bucket-s3"     # nombre literal del bucket en el proveedor
region = "us-east-1"
access_key_id = "AKIA..."
secret_access_key = "..."
# Obligatoria: cifra todo lo que este bucket sube. Generarla con algo
# como `openssl rand -base64 32`. Guardala aparte tambien: sin ella, todo lo
# ya cifrado con ella es irrecuperable (ver seccion "Cifrado del lado de la
# API" arriba).
passphrase = "..."

# S3-compatible (MinIO, etc.) con endpoint propio y path-style
[buckets.mi-bucket-minio]
driver = "s3"
bucket = "mi-bucket-minio"
region = "us-east-1"
endpoint_url = "http://127.0.0.1:9000"
access_key_id = "minioadmin"
secret_access_key = "minioadmin"
path_style = true
passphrase = "..."

# Cloudflare R2 -- mismo protocolo S3 por detras, pero el endpoint sale solo
# del account_id (no hace falta endpoint_url/region/path_style a mano)
[buckets.mi-bucket-r2]
driver = "r2"
bucket = "mi-bucket-r2"
account_id = "..."
access_key_id = "..."
secret_access_key = "..."
passphrase = "..."

# Filesystem local -- sin proveedor externo. CAS real via flock, pero
# valido solo en una maquina (ver detalle del driver local mas abajo).
# "bucket" aca es el directorio.
[buckets.mi-bucket-local]
driver = "local"
bucket = "/var/lib/gitquarry/mi-bucket-local"
passphrase = "..."

# Azure Blob Storage -- protocolo distinto a S3, conditional writes nativos.
# "bucket" aca es el nombre del container.
[buckets.mi-bucket-azure]
driver = "azure"
bucket = "mi-container"
account_name = "..."
tenant_id = "..."
client_id = "..."
client_secret = "..."
passphrase = "..."

# Google Cloud Storage -- API nativa (no el endpoint XML S3-interop de GCS),
# asi el CAS de refs.json corre sobre ifGenerationMatch con garantia real
[buckets.mi-bucket-gcs]
driver = "gcs"
bucket = "mi-bucket-gcs"
service_account_json_path = "/etc/gitquarry/gcs-key.json"
passphrase = "..."
```

`driver` es obligatorio (sin default): identifica que backend de storage usa
esta entrada. Por ahora `s3` (cualquier endpoint que hable la API de S3 —
AWS real, MinIO, etc.), `r2` (Cloudflare R2), `azure` (Azure Blob Storage,
protocolo propio via `azure_storage_blobs`, conditional writes nativos por
`If-Match`/`If-None-Match` igual que S3), `gcs` (Google Cloud Storage via su
API nativa — no el endpoint XML S3-interop de GCS, para tener la misma
garantia de conditional-write documentada que S3/R2/Azure en vez de confiar
en compatibilidad no garantizada) y `local` (directorio en el filesystem
local, sin proveedor externo — el compare-and-swap que `refs.json` necesita
se emula con `flock` sobre un sidecar por objeto, asi
que solo da la misma garantia que S3/R2/Azure **dentro de una sola maquina**:
no apunta dos gitquarry distintos al mismo `root` compartido por NFS
esperando la misma seguridad). Los cinco drivers comparten el mismo campo
`bucket` para su destino fisico (bucket de verdad en s3/r2/gcs, container en
azure, directorio en local) — mismo nombre de campo en todos aunque el
proveedor de turno le diga distinto, asi cada `[buckets.<nombre>]` tiene
siempre la misma forma sin importar el driver. La clave
`[buckets.<nombre>]` es el nombre local de la entrada (lo que matchea el
`--bucket <nombre>` pasado al arrancar el server) — no tiene por que
coincidir con `bucket` (el nombre literal en el proveedor), aunque en el uso
tipico son el mismo string. Si el config tiene un solo bucket definido, `--bucket`
es opcional: el server lo detecta solo.

## Uso: server unificado (ssh:// + http + UI web, multi-repo)

```sh
# un solo comando arranca los tres servicios en el mismo proceso
# (foreground; Ctrl+C para pararlo, o '&' para segundo plano)
gitquarry --bucket mi-bucket
# --bucket es opcional si el config solo tiene un bucket definido

# en cualquier otra terminal/maquina con acceso de red al puerto,
# cada path es un repo distinto (se crea solo al primer push):
git clone http://127.0.0.1:8080/proyectoA
git clone http://127.0.0.1:8080/equipo/proyectoB.git

cd proyectoA
git push origin main   # funciona igual que cualquier remoto git

# ssh://: primero autorizar tu pubkey desde la UI web (abajo), despues:
git clone ssh://cualquiera@127.0.0.1:2222/proyectoA

# UI web de solo lectura sobre los mismos repos, + /_settings para
# editar binds/mirrors-dir y /_users para autorizar claves SSH/password
# (se guarda en el bucket, /_settings aplica al reiniciar):
xdg-open http://127.0.0.1:8080/
```

Flags:

- `--bucket NOMBRE`: bucket S3 base. Opcional si la config solo tiene uno.
- `--web-bind ADDR:PUERTO` (default `127.0.0.1:8080`): UI web de lectura +
  git smart-HTTP, mismo puerto para ambos.
- `--ssh-bind ADDR:PUERTO` (default `127.0.0.1:2222`; no es el puerto 22
  estandar de ssh, asi no compite con el sshd del sistema — el cliente lo
  indica en la url, `ssh://host:2222/...`, o via `Port` en `~/.ssh/config`).
- `--mirrors-dir PATH` (default `~/.cache/gitquarry/mirrors/<bucket>`):
  raiz donde viven los mirrors bare locales, uno por repo/path, compartidos
  entre ssh://, http y la UI web.
- `--mirrors-max-age DURACION` (default `3d`; numero + `s`/`m`/`h`/`d`):
  mirror sin consultar en mas de esto se limpia del cache.
- `--mirrors-max-size TAMANO` (default `5G`; numero + `B`/`K`/`M`/`G`): si
  `--mirrors-dir` sigue por encima despues de la limpieza por edad, borra los
  mirrors menos usados hasta bajar del limite.
- `--help` / `--version`.

La config (`gitquarry.toml` / `GITQUARRY_CONFIG` / global) se resuelve
una vez al arrancar el server, con el mismo orden de siempre.

## Estado / limitaciones conocidas

- Sin repack automatico: historia larga con muchos pushes acumula muchos
  packs chicos en S3 (funciona, pero fetch se vuelve mas lento con el
  tiempo). `gitquarry repack <repo> --apply` lo hace a demanda (baja todo a
  un mirror temporal, corre `git repack -a -d`, sube el pack consolidado,
  borra los viejos una vez confirmado) — operador lo corre cuando quiere,
  no hay barrido periodico todavia, y no es seguro correrlo contra un repo
  con pushes en curso de otro proceso (ver `gitquarry repack --help`).
- Compare-and-swap de refs.json requiere que el backend S3 soporte
  `If-Match`/`If-None-Match` condicional (AWS S3 si; MinIO/R2 revisar version).
- Credenciales via archivo de config propio, no via `~/.aws/credentials` /
  variables de entorno AWS estandar.
- La UI web de lectura (navegar repos `public`, `/_settings`, `/_users`) no
  tiene auth propia mas alla del gate por `private`/admin: sin login OIDC
  y/o password configurado (ver seccion de arriba) queda tan abierta como
  siempre estuvo. Sigue sin proteccion CSRF real en los forms
  (`SameSite=Lax` ayuda, no es lo mismo que un token CSRF). Bindear el
  puerto web a `127.0.0.1` (el default) o poner un proxy con auth adelante
  si se expone mas alla de una red confiable — `ssh://` y git smart-HTTP ya
  exigen credenciales reales y permiso por repo para todo lo que no sea leer
  un repo `public`, asi que no dependen de este mismo gate.
- La cookie de sesion OIDC no fuerza `Secure`: mismo criterio que el resto
  (puertos pensados para localhost/red confiable/detras de un proxy). Si se
  expone la web directo a internet, poner TLS adelante.
- El index (`/`) no lista repos `private` aunque el usuario logueado tenga
  acceso — solo repos `public`. Son alcanzables por URL directa, no
  aparecen en la lista (evita sumarle un chequeo de sesion+permiso al fetch
  concurrente por repo que el index ya hace).
- El archivo de config guarda credenciales y `passphrase` en texto plano; el
  server avisa al arrancar si tiene permisos abiertos (`chmod 600` recomendado).
- `.conf/access.json` cambio de forma (de lista de claves sueltas a lista de
  usuarios con claves adentro) sin migracion automatica: un `access.json` de
  antes de este cambio no deserializa y `get_access`/`put_access` van a
  fallar hasta borrarlo o reescribirlo a mano en el formato nuevo.
- Los repos cambiaron de vivir en la raiz del bucket (`<owner>/<name>`) a
  vivir bajo `repos/` (`repos/<owner>/<name>`), tampoco con migracion
  automatica: un bucket de antes de este cambio necesita
  `gitquarry migrate-repos-prefix --apply` una vez (ver seccion de arriba)
  antes de que el server encuentre esos repos.
- Un push que pierde el compare-and-swap deja en S3 el pack que ya habia
  subido; no es corrupcion (git ignora packs sin refs que los alcancen) pero
  ocupa espacio hasta que exista el repack remoto.
- El cifrado de packs es en streaming (ver seccion "Cifrado del lado de la
  API"), memoria acotada sin importar el tamaño del pack — a cambio de
  ~2-3x ese tamaño en disco de forma transitoria durante la subida/bajada.
- Sin rotacion de passphrase: cambiar `passphrase` deja ilegible todo lo ya
  cifrado con la anterior (no hay comando de re-cifrado masivo todavia).
- El index de la UI web hace un fetch de config a S3 por repo (para filtrar
  los `private`), concurrente pero uno por repo igual — con muchos repos en
  el bucket eso es varias decenas/cientos de requests por carga del index.

## Desarrollo

Workspace de varios crates (el binario `gitquarry` sigue siendo uno solo,
esto es organizacion de codigo, no de deploy):

- `gitquarry-core`: motor — dominio, storage, auth, git (ssh:///smart-HTTP),
  mirrors, cifrado. Lib pura, sin binario propio.
- `gitquarry-webui`: la UI web (server-rendered, sin JS) + login/settings/
  busqueda. Separada de `gitquarry-core` para poder cambiar la
  implementacion del frontend a futuro sin tocar el codigo de git-serving
  (ver `docs/webui-crate-split-concept.md`).
- `gitquarry-server`: el binario real (`gitquarry`) — arranca ssh:// +
  UI web juntos en el mismo proceso, mas el wizard de setup del primer
  arranque. Depende de los dos anteriores.

```sh
cargo test --workspace          # unitarios (validacion, parseo de protocolo/git, escape) +
                                # un test de integracion que arma un repo git real en un
                                # tempdir y ejercita las vistas de la UI web contra el
cargo clippy --workspace --all-targets
```
