# tests/

Pruebas a nivel de repo, no unitarias de crate.

- `golden/` — **golden compatibility tests** (§70): paridad medida contra AWS
  real. AWS RIE se reutiliza aquí para validar el protocolo Runtime API (§2).
- `isolation/` — **isolation escape tests** (§32, §82). Son **criterio de
  release**: `tenant_trust=semi-trusted` sólo se habilita desde v0.2 y sólo si
  esta suite pasa. El server no finge aislamiento (§78).
