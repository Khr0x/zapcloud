# Quickstart — probar zapcloud Functions en local

Guía práctica para levantar el daemon y ejecutar funciones end-to-end, 100% local,
con el **AWS CLI real** apuntando a tu `localhost`. Sin Docker (salvo para ensamblar
bundles Linux), sin nada en la nube: solo SQLite + filesystem (§5.2).

Estado cubierto: **v0.1.1** (pasos 1–10). Runtimes disponibles: `provided.al2023`,
`nodejs22.x`, `python3.13`. Invocación **síncrona** (`RequestResponse`) en process/T1
(**sin aislamiento** — no ejecutes código no confiable).

---

## 1. Requisitos

- **Rust** (toolchain del repo) para compilar el daemon.
- **AWS CLI v2** (o cualquier SDK de AWS) como cliente.
- **Docker** solo si quieres ensamblar los bundles Linux con RIC real. Para probar en
  macOS no hace falta: los bundles `darwin-arm64` usan `dev-runtime` (menor fidelidad).

`provided.al2023` no necesita ningún bundle (el `bootstrap` lo trae tu ZIP).
`nodejs22.x` y `python3.13` necesitan su bundle ensamblado (ver §6).

---

## 2. Arrancar el daemon

Usa el `zapcloud.toml.example` como base (escucha en `127.0.0.1:9000`, región `local-1`,
auth `none`):

```bash
cp zapcloud.toml.example zapcloud.toml
mkdir -p data
cargo run -p zapcloud --release -- serve --config zapcloud.toml
```

Deberías ver `zapcloud serve listo`. Comprueba salud en otra terminal:

```bash
curl -s http://127.0.0.1:9000/health/ready
curl -s http://127.0.0.1:9000/metrics | head
```

---

## 3. Configurar el cliente AWS (profile aislado)

Auth en modo `none`: las credenciales son dummy pero el CLI las exige. En vez de
exportar `AWS_*` en la shell (contamina la sesión y puede **ensombrecer tus
credenciales reales**), usa un **profile en un config aislado** del propio repo: no
toca tu `~/.aws`, mete el `endpoint_url` dentro del profile y se destruye con un
`rm -rf` (ver §11).

```bash
# Config aislado en ./.aws-local (no toca ~/.aws)
export AWS_CONFIG_FILE="$PWD/.aws-local/config"
export AWS_SHARED_CREDENTIALS_FILE="$PWD/.aws-local/credentials"
mkdir -p .aws-local

aws configure --profile zapcloud set region local-1
aws configure --profile zapcloud set aws_access_key_id local
aws configure --profile zapcloud set aws_secret_access_key local
aws configure --profile zapcloud set endpoint_url http://127.0.0.1:9000

alias zc='aws --profile zapcloud'
```

Con esto, `zc lambda ...` ya lleva endpoint, región y credenciales del profile —
sin repetir `--endpoint-url` en cada comando.

> **`endpoint_url` en el profile requiere AWS CLI ≥ 2.13.** Con una versión anterior,
> quita esa línea y añade `--endpoint-url http://127.0.0.1:9000` a cada comando (o
> `export AWS_ENDPOINT_URL=http://127.0.0.1:9000`).
>
> **Nota `--payload`:** para pasarlo como JSON crudo añade
> `--cli-binary-format raw-in-base64-out` (incluido en los ejemplos).

---

## 4. Ejemplo A — `provided.al2023` (sin bundle, fidelidad total)

El runtime `provided.*` implementa el bucle del Runtime API en tu propio `bootstrap`.
Este ejemplo en shell hace **eco del evento**.

```bash
mkdir -p demo-provided && cd demo-provided
cat > bootstrap <<'EOF'
#!/bin/sh
set -eu
while true; do
  HEADERS="$(mktemp)"
  EVENT="$(curl -sS -LD "$HEADERS" "http://$AWS_LAMBDA_RUNTIME_API/2018-06-01/runtime/invocation/next")"
  REQ_ID="$(grep -i '^lambda-runtime-aws-request-id:' "$HEADERS" | tr -d '\r' | awk '{print $2}')"
  curl -sS "http://$AWS_LAMBDA_RUNTIME_API/2018-06-01/runtime/invocation/$REQ_ID/response" \
    -d "{\"runtime\":\"provided.al2023\",\"echo\":$EVENT}"
done
EOF
chmod +x bootstrap
zip fn.zip bootstrap

zc lambda create-function \
  --function-name eco-provided \
  --runtime provided.al2023 \
  --role arn:aws:iam::000000000000:role/lambda-role \
  --handler bootstrap \
  --zip-file fileb://fn.zip

zc lambda invoke \
  --function-name eco-provided \
  --cli-binary-format raw-in-base64-out \
  --payload '{"hello":"zap"}' \
  out.json && cat out.json
cd ..
```

Esperado: `out.json` con `{"runtime":"provided.al2023","echo":{"hello":"zap"}}`.

---

## 5. Ejemplo B — `nodejs22.x`

Handler `index.handler` → `index.js` en la raíz del ZIP.

```bash
mkdir -p demo-node && cd demo-node
cat > index.js <<'EOF'
exports.handler = async (event) => ({ echoed: event, pid: process.pid });
EOF
zip fn.zip index.js

zc lambda create-function \
  --function-name eco-node \
  --runtime nodejs22.x \
  --role arn:aws:iam::000000000000:role/lambda-role \
  --handler index.handler \
  --zip-file fileb://fn.zip

zc lambda invoke \
  --function-name eco-node \
  --cli-binary-format raw-in-base64-out \
  --payload '{"n":1}' \
  out.json && cat out.json
cd ..
```

Invócala dos veces: el `pid` se repite → **reuso warm** del proceso.

---

## 6. Ejemplo C — `python3.13`

Handler `lambda_function.handler` → `lambda_function.py` en la raíz del ZIP.

Primero, asegúrate de tener el bundle del host ensamblado:

```bash
# darwin (nativo, sin Docker): dev-runtime
cargo run -p xtask -- bundle --runtime python3.13 --target darwin-arm64
# linux con RIC real (requiere Docker):
# cargo run -p xtask -- bundle --runtime python3.13 --target linux-arm64
```

```bash
mkdir -p demo-py && cd demo-py
cat > lambda_function.py <<'EOF'
import os

def handler(event, context):
    return {"echoed": event, "pid": os.getpid()}
EOF
zip fn.zip lambda_function.py

zc lambda create-function \
  --function-name eco-py \
  --runtime python3.13 \
  --role arn:aws:iam::000000000000:role/lambda-role \
  --handler lambda_function.handler \
  --zip-file fileb://fn.zip

zc lambda invoke \
  --function-name eco-py \
  --cli-binary-format raw-in-base64-out \
  --payload '{"n":2}' \
  out.json && cat out.json
cd ..
```

> `nodejs22.x` funciona igual: su bundle se ensambla con
> `cargo run -p xtask -- bundle --runtime nodejs22.x --target darwin-arm64`.

---

## 7. Resto de la API

```bash
# Listar funciones
zc lambda list-functions

# Detalle de una función
zc lambda get-function --function-name eco-node

# Actualizar el código
zip fn.zip index.js
zc lambda update-function-code --function-name eco-node --zip-file fileb://fn.zip

# Borrar
zc lambda delete-function --function-name eco-node
```

---

## 8. Observabilidad

```bash
curl -s http://127.0.0.1:9000/health/live
curl -s http://127.0.0.1:9000/health/ready
curl -s http://127.0.0.1:9000/metrics
```

---

## 9. Qué NO funciona todavía (por roadmap)

- **Aislamiento**: process/T1, el código **no está sandboxeado** (v0.2).
- **Invoke async** (`Event`, 202): solo síncrono `RequestResponse` (v0.3).
- **Variables de entorno de usuario** (`Environment.Variables`): aún no se inyectan (paso 13).
- **`GetFunctionConfiguration` / `UpdateFunctionConfiguration`** (paso 12).
- **Versions / aliases** (v0.4), **Function URLs** (v0.6), **runtimes no-AWS/WASM** (v0.8).

---

## 10. Troubleshooting

| Síntoma | Causa / arreglo |
|---|---|
| `RuntimeUnavailable ... bundle no está instalado` | Falta el bundle del runtime → `cargo run -p xtask -- bundle --runtime <nodejs22.x\|python3.13>` |
| `InvalidParameterValue` en runtime | Runtime no soportado en v0.1.1 (solo `provided.al2023`, `nodejs22.x`, `python3.13`) |
| `invoke` devuelve base64 raro | Falta `--cli-binary-format raw-in-base64-out` en el `invoke` |
| Node/Python en macOS con salida "dev" | Esperado: darwin usa `dev-runtime` (menor fidelidad). El RIC real es solo Linux |
| No conecta al endpoint | El daemon no está corriendo, o el `endpoint_url` del profile no apunta a `http://127.0.0.1:9000` |
| `endpoint_url` ignorado / error de config | AWS CLI < 2.13 → usa `--endpoint-url` por comando o `AWS_ENDPOINT_URL` (ver §3) |
| El profile usa tus credenciales reales de AWS | Faltó exportar `AWS_CONFIG_FILE`/`AWS_SHARED_CREDENTIALS_FILE` antes de `aws configure` (ver §3) |

---

## 11. Limpieza (destruir el profile)

El profile vive en `./.aws-local`, aislado de tu `~/.aws`. Para eliminarlo sin dejar
residuos en tu config real:

```bash
rm -rf .aws-local
unset AWS_CONFIG_FILE AWS_SHARED_CREDENTIALS_FILE
unalias zc 2>/dev/null || true
```

Para borrar también los datos del daemon (funciones, artifacts, SQLite):

```bash
rm -rf data
```
