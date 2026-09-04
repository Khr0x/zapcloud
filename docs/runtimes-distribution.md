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

# En un host desplegado: bajar y verificar los bundles ausentes.
zapcloud runtimes install --runtime nodejs22.x
```

- **Resolución en el invoke** (`zc-runtime::resolve`): cache-only, **nunca toca
  la red**. Si el bundle falta o su integridad no verifica, el cold start falla.
- **`ensure`** (install / preflight de `serve`): lo único que baja de la red.
  Descarga pinneada por `oci_digest`, verifica `tree_sha256`, instala atómico
  (staging + rename).

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

### Bootstrap (primera vez / índice en `{}`)

```
Actions → runtimes → Run workflow (main)   [o: gh workflow run runtimes.yml --ref main]
   ↓  build + verify + gate (PINNED vacío → "primera publicación", pasa)
   ↓  publica bundles a ghcr.io
   ↓  abre PR "ci/update-runtime-index" con los pins reales
VOS: revisás los digests → merge
   → runtimes/index.json queda poblado. FIN.
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

---

## 5. El gate de reproducibilidad (dónde es duro)

El build **debe** reproducir el `tree_sha256` pinneado (§17). Pero un cambio de
bundle *intencional* difiere a propósito, así que el gate no es duro en todos
lados —si lo fuera, no podrías actualizar nunca un bundle—:

| Contexto | Comportamiento |
|---|---|
| Índice vacío (primera publicación) | pasa (nada que reproducir) |
| Hash == pin | pasa |
| Hash ≠ pin, en el **PR `ci/update-runtime-index`** | **falla** (el pin propuesto DEBE reproducir) |
| Hash ≠ pin, en un PR de código | **aviso** (no bloquea; si es intencional, el pin se actualiza tras el merge) |
| Hash ≠ pin, en `main` | **aviso** (publica igual; el PR de índice lleva el pin nuevo, revisado) |

La garantía real de reproducibilidad la da el **PR de índice**: reconstruye y
exige que el pin recién generado reproduzca. Si ahí falla, el build es
no-determinista y hay que arreglarlo (ver §7).

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

### `tree_sha256 no reproduce: pinneado=… build=…`
El build **no es reproducible**. Causas conocidas:
- **SBOM no-determinista** (resuelto): `npm sbom` inyecta `serialNumber`/`timestamp`
  nuevos por corrida; `normalize_node_sbom` los quita.
- **Pin stale**: si los hashes son idénticos entre corridas pero no matchean el
  pin, el pin viene de un build viejo (pre-fix). Descartá la rama
  `ci/update-runtime-index` y republicá desde main (índice `{}` → re-pinnea).
- **Hashes NUEVOS y distintos entre corridas**: queda otra fuente de
  no-determinismo. Sospechoso: el addon nativo del RIC (`rapid-client.node`,
  g++). Se ataca con `SOURCE_DATE_EPOCH` + `-ffile-prefix-map` en el build Docker.

### El job `update-index` falla en `app-token`
Faltan los secrets del GitHub App (§4, prerrequisito).

### La rama `ci/update-runtime-index` quedó con un pin viejo
Es basura generada por un pipeline anterior. **Borrala** (`git push origin
--delete ci/update-runtime-index`) y republicá desde main; se regenera limpia.

---

## 8. Rollback

Un bundle malo se revierte revirtiendo el commit del índice (`git revert`) o
mergeando un PR que restaure el `index.json` anterior. Los hosts vuelven a
instalar el `oci_digest` previo. No se re-taggea en el registry.
