# Distribución de runtime bundles (§16–§17)

Cómo se **construyen, publican, versionan e instalan** los runtime bundles
(`nodejs22.x`, `python3.13`), y cuál es el flujo de CI. Referencia operativa; el
diseño vive en el RFC [`lambda-zapcloud.md`](rfc/lambda-zapcloud.md) §16–§17 y el
detalle de comandos en [`../runtimes/README.md`](../runtimes/README.md).

---

## 1. Modelo mental

Un **bundle** es el runtime que ejecuta las funciones de usuario en un host Linux:
intérprete OSS (Node.js oficial / CPython-PSF) + RIC de AWS (Apache-2.0) +
`bootstrap` propio + manifest/SBOM/licencias. Nunca contiene artefactos de Amazon
Linux (§16).

Dos piezas separan **dónde están los bytes** de **cuál se usa**:

| Pieza | Rol | Análogo |
|---|---|---|
| `ghcr.io/<owner>/zapcloud/runtime-*` | Almacena los bytes (OCI artifact) | crates.io |
| `runtimes/index.json` | **Lockfile**: pinnea `tree_sha256` + `oci_digest` por `runtime × plataforma` | `Cargo.lock` |

`ghcr.io` es un **registry OCI** (blob storage por HTTP). El daemon baja con el
crate Rust `oci-client` — **no necesita Docker** para correr. Que `docker pull`
funcione es un efecto secundario útil para depurar, no una dependencia.

> **Regla de oro:** `runtimes/index.json` es generado, **no se edita a mano**.
> Lo escribe CI; vos revisás y mergeás su PR (igual que `Cargo.lock`).

---

## 2. Comandos (`xtask` + daemon)

```bash
# Ensamblar un bundle (clean-room, RIC real). Sin --target usa el host.
cargo run -p xtask -- bundle --runtime nodejs22.x --target linux-x86_64

# Verificar integridad (bootstrap + tree_sha256 contra el manifest).
cargo run -p xtask -- verify runtimes/nodejs22-linux-x86_64

# Publicar a ghcr + pinnear la entrada en runtimes/index.json (solo Linux).
cargo run -p xtask -- publish --runtime nodejs22.x --target linux-x86_64

# En un host desplegado: aplicar el pin del índice (instalar, actualizar o reparar).
zapcloud runtimes install --runtime nodejs22.x
```

- **Resolución en el invoke** (`zc-runtime::resolve`): cache-only, **nunca toca
  la red**. Si el bundle falta o su integridad no verifica, el cold start falla.
- **`ensure`** (install / preflight de `serve`): lo único que baja de la red.
  Compara la entrada completa del índice con el recibo de instalación y verifica
  el árbol, runtime, SO, arquitectura y versiones del manifest. Un cambio de
  `oci_digest` cuenta como cambio de pin aunque el árbol sea idéntico.

> **Estado actual:** `runtimes/index.json` ya está poblado para Linux/amd64. Una
> instalación desde un binario con cache vacía aún necesita recibir ese índice como
> recurso versionado; `ensure` no puede descubrir el runtime sin él.

### Actualización, reparación y rollback

Cada instalación verificada se guarda bajo
`.versions/<bundle>/gen-<id>/bundle/`, con un recibo `pin.json` al lado. La ruta
pública, por ejemplo `nodejs22-linux-x86_64`, es un symlink relativo a la generación
activa. `resolve` devuelve rutas canónicas: cambiar el enlace afecta a los nuevos
environments; los existentes siguen usando su generación.

Para actualizar o volver a una versión anterior, despliega el `index.json`
revisado correspondiente y ejecuta `zapcloud runtimes install`. `ensure` valida
primero una generación conservada del pin solicitado; si no existe o está corrupta,
descarga por digest OCI, verifica y activa una nueva generación. No modifica ni
borra los bytes de una generación anterior. Con `[runtimes].offline = true` puede
hacer rollback a una generación íntegra en cache; falla si el pin deseado no está
disponible. No acepta otro pin ni un índice ausente como sustituto.

Un lock del SO por bundle serializa instaladores, incluso de distintos procesos.
El cambio de enlace usa un único rename, sin ventana con el destino ausente. Un
fallo de descarga, integridad o activación conserva el destino anterior. Cancelar
la descarga limpia staging; tras morir el proceso, la próxima instalación limpia
staging abandonado y puede reutilizar generaciones verificadas sin activar.
Esto cubre interrupciones de proceso; no promete durabilidad ante pérdida eléctrica
(no hay un protocolo de `fsync` del árbol completo).

**Migración del layout anterior:** la primera instalación necesita red para
acreditar el digest OCI; un manifest local no prueba ese digest. En Linux se
intercambian atómicamente el directorio antiguo y el enlace con `renameat2` /
`RENAME_EXCHANGE`, conservando el directorio en `.previous-*`. Si el filesystem no
soporta el intercambio, falla sin eliminar el directorio anterior. Detén los
daemons antiguos durante esta primera migración: pueden conservar rutas del
layout anterior. Los upgrades posteriores entre generaciones no lo necesitan.

GC/cuotas siguen pendientes: las generaciones y backups se conservan y consumen
disco. No los borres mientras un environment pueda usarlos. El preflight mantiene
su política de advertir y continuar si falla; puede seguir disponible el bundle
anterior, pero eso no acredita que el pin nuevo se haya aplicado.

Pruebas de recuperación, sin registry externo (el E2E levanta HTTP en loopback):

```sh
cargo test --locked -p zc-runtime --lib distribute::tests
```

CI Linux las ejecuta en el test del workspace, incluidas migración legacy y
descarga OCI real. macOS solo prueba la lógica local; la distribución pública
continúa restringida a Linux.

---

## 3. Alcance de plataformas

Hoy CI construye **solo `linux/amd64`**. `arm64` está fuera de scope hasta que
haya un host Linux ARM objetivo (Graviton/Ampere): compilar el RIC nativo para
arm64 exige QEMU (~7 min/bundle) sin beneficio actual. El **código** conserva el
soporte arm64 (`xtask bundle --target linux-arm64`, `resolve`, `index`), así que
reactivarlo es volver a añadir `arm64` a la matriz del workflow.

macOS (`darwin-arm64`) es **dev-only**: usa `dev-runtime.mjs`/`.py` (sin RIC
nativo) y **no se publica** ni se construye en CI. Es para desarrollar en tu
máquina, no para servidores.

---

## 4. Flujo de CI

Workflow: [`.github/workflows/runtimes.yml`](../.github/workflows/runtimes.yml).

### Prerrequisito (una vez): GitHub App

El job que abre el PR del índice usa un **GitHub App token** (no el
`GITHUB_TOKEN`, porque un PR abierto con el token por defecto no dispara los
checks). Configurá:

- Secrets `RUNTIMES_BOT_APP_ID` y `RUNTIMES_BOT_PRIVATE_KEY` (Settings → Secrets
  and variables → Actions).
- El App con permisos **Contents: Read/Write** + **Pull requests: Read/Write**,
  instalado en el repo.

Sin esto, el job `update-index` falla en el paso `app-token`.

### Bootstrap (histórico; índice actualmente poblado)

```
Actions → runtimes → Run workflow (main)   [o: gh workflow run runtimes.yml --ref main]
   ↓  build + verify + gate
   ↓  publica bundles a ghcr.io
   ↓  abre PR "ci/update-runtime-index" con los pins reales
VOS: revisás los digests → merge
   → se revisan los pins del índice. FIN.
```

### Recurrente (cada cambio de runtime: `xtask/**` o `zapcloud-functions/runtime/**`)

```
1. rama desde main → cambios → PR a main
2. CI en el PR: build + verify + gate (ver §5)
3. review + merge a main
4. CI en main: build + publica a ghcr + abre/actualiza "ci/update-runtime-index"
5. VOS: revisás los digests del PR de índice → merge
6. FIN  (el merge toca solo index.json → no re-dispara: ver §6)
```

Cada job de la matriz ejecuta `xtask publish --index` sobre un índice temporal
vacío y sube **solo la entrada publicada** (`runtime × plataforma`). No sube una
copia completa del índice del checkout: esa copia contendría pins antiguos de
los otros jobs y podría sobrescribir una publicación nueva al combinarse.

`update-index` aplica los fragmentos sobre `runtimes/index.json` del checkout con
[merge-runtime-index.jq](../.github/scripts/merge-runtime-index.jq). Reemplaza cada
entrada completa, conserva las entradas no publicadas en esa ejecución y rechaza
fragmentos vacíos, con varias entradas o con publicaciones duplicadas. Escribe
primero un archivo temporal; solo reemplaza el índice si la unión termina bien.

La regresión corre en el job `test` de CI, sin registry ni credenciales:

```sh
python3 -B -m unittest discover -s tests/runtime_index -p 'test_*.py' -v
```

Comprueba publicaciones de Node/Python desde la misma base, ambos órdenes de
unión, plataformas de un mismo runtime, conservación de pins ajenos y rechazo
del formato antiguo de snapshots completos. Requiere Python 3 y `jq`.

---

## 5. El gate de reproducibilidad (integridad ≠ reproducibilidad)

Dos objetivos distintos, que el CI mantiene separados:

- **Integridad** (crítico): `tree_sha256` pinnea el artifact **publicado** —
  incluidos los binarios nativos— y el daemon lo verifica al bajar. Garantiza
  que se ejecutan exactamente los bytes revisados. No depende de reproducibilidad.
- **Reproducibilidad** (§17): el gate reconstruye el bundle **dos veces** y exige
  que **las partes que zapcloud ensambla** (bootstrap, layout, SBOM, intérprete
  verificado por checksum) sean byte-idénticas.

**Excepción deliberada:** los binarios nativos **upstream** (`*.node`, `*.so`,
compilados por el RIC de AWS) **no son bit-reproducibles** —el no-determinismo
está en el código compilado C++, no en metadata limpiable— y el gate los
**excluye**. Su integridad la cubre el `tree_sha256` publicado + la verificación
del daemon, no la reproducibilidad. El gate **falla** solo si difiere una parte
determinista (eso sí sería un bug nuestro; ver §7).

**Estado del gate:** el workflow actual compara dos builds de la misma ejecución,
pero todavía no compara automáticamente el resultado contra el `tree_sha256`
publicado en `runtimes/index.json`. Hasta v0.1.2 debe llamarse *repeatability smoke
test*, no gate completo de reproducibilidad del pin.

---

## 6. Por qué no hay bucle de PRs

Dos cortacircuitos independientes:

1. **Filtro de paths.** El PR de índice toca **solo** `runtimes/index.json`, que
   **no** está en `paths` del trigger `push: [main]`. Mergearlo no re-dispara el
   workflow → no republica → no abre otro PR.
2. **Idempotencia.** `create-pull-request` no abre PR si no hay diff, y si lo
   hay actualiza siempre la misma rama `ci/update-runtime-index` (nunca duplica).

Al abrirse el PR de índice sí corre el trigger `pull_request` (index.json está en
*sus* paths): solo build + verify + gate (no publica, no abre PRs). Es la
verificación de reproducibilidad del §5, no un bucle.

---

## 7. Troubleshooting

### `invalid reference format` al publicar
`ghcr.io/<Owner>/...` con mayúsculas. OCI exige el nombre del repo en minúsculas;
`github.repository` conserva el case del owner. Ya resuelto: `oci_ref` normaliza
el base a minúsculas (`zc-runtime::oci`). No requiere acción.

### El gate `partes deterministas` falla
Difiere una parte que zapcloud ensambla (no un binario nativo upstream) — **eso
sí es un bug de determinismo nuestro**. Causas ya resueltas históricamente:
- **SBOM no-determinista**: `npm sbom` inyecta `serialNumber`/`timestamp` nuevos
  por corrida; `normalize_node_sbom` los quita y reserializa con claves ordenadas.

Si aparece un fichero nuevo en el diff del gate, replicá el patrón: hacelo
determinista en `xtask bundle` (sin timestamps/UUID/orden variable).

### `rapid-client.node` (u otro `*.node`) difiere entre builds
**Esperado, no es bug.** El addon nativo upstream del RIC no es bit-reproducible.
El gate lo excluye a propósito (§5); su integridad va por el digest publicado.
No intentes forzarlo con `strip`/`SOURCE_DATE_EPOCH`: se probó y el
no-determinismo está en el código compilado, no en metadata.

### El job `update-index` falla en `app-token`
Faltan los secrets del GitHub App (§4, prerrequisito).

### La rama `ci/update-runtime-index` quedó con un pin viejo
Es basura generada por un pipeline anterior. **Borrala** (`git push origin
--delete ci/update-runtime-index`) y republicá desde main; se regenera limpia.

---

## 8. Rollback (objetivo de v0.1.2)

El procedimiento previsto es revertir el commit del índice (`git revert`) o
mergear un PR que restaure el `index.json` anterior. Para que esto sea efectivo,
v0.1.2 debe hacer que `ensure` compare el pin deseado y reemplace atómicamente el
bundle local, incluso si existe un destino corrupto. Hoy cambiar el índice por sí
solo no fuerza la reinstalación. No se re-taggea en el registry.

## 9. Limitaciones conocidas hasta v0.1.2

- La matriz de CI publica índices completos desde jobs paralelos; el merge puede
  sobrescribir una actualización con un pin antiguo.
- Solo se distribuye Linux/amd64. `darwin-*` es dev-only y Linux/arm64 aún no tiene
  una entrada publicada.
- `runtimes.registry` del ejemplo es informativo; actualmente la autoridad efectiva
  es `oci_ref` del índice. Debe eliminarse o hacerse funcional antes de declararlo
  una opción de configuración.
