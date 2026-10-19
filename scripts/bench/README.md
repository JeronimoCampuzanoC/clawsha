# Pruebas de sistema y benchmarks

Scripts de la campaña de pruebas del informe (sección 7). Levantan el clúster completo en Docker, construido desde este checkout, y lo someten a carga y a fallas reales por su API REST, como lo haría un cliente. Después resumen los resultados en tablas y gráficas.

Todo corre en local. Las cifras de rendimiento miden el costo del software (cifrado, hashes, pipeline, coordinación con el master) en una sola máquina, sin los límites de red de las instancias `t3` de AWS. Son una cota superior, no el rendimiento del despliegue real.

## Requisitos

- Docker con Compose v2 y unos 6 GB libres para las imágenes y los volúmenes.
- `curl`, `jq` y `python3`. Para las gráficas en PNG, además `matplotlib` (sin él, `summarize.py` igual genera las tablas y las coordenadas para pgfplots).
- Puertos libres: 3101 (gateway), 8181 y 8182 (`/health/leader` de cada master). No chocan con el entorno de desarrollo de `deploy/compose.local.yml`.

## Uso rápido

```bash
scripts/bench/run-all.sh          # construye, genera datos y corre todo (~45 min)
scripts/bench/run-all.sh --no-build
```

Los resultados quedan en `scripts/bench/results/<fecha-hora>/`, con `summary.md`, `pgfplots.tex` y las gráficas. Mientras corre conviene no usar la máquina: entre repeticiones hubo variaciones de hasta ±15 % por carga ajena.

## Scripts

| Script | Qué hace | Sección del informe | Salida |
|---|---|---|---|
| `stack.sh build\|up\|down` | Construye las imágenes, levanta un clúster limpio y espera a que esté listo (gateway, 3 workers vivos, líder fuera de modo seguro), o lo baja | 7.1 | — |
| `gen-data.sh` | Archivos aleatorios en los bordes de la partición: 0 B, 1 B, 1 MiB, 16 MiB, 16 MiB + 1 B, 100 MiB, 256 MiB y 1 GiB | 7.1 | `data/` |
| `functional.sh` | Integridad por tamaño y casos F1–F13: espacio de nombres, aislamiento entre usuarios, nombres repetidos y dos escritores en la misma ruta | 7.2 | `functional-*.{csv,txt}` |
| `security.sh` | Casos S1–S9: JWT ausente, alterado, vencido, con otro secreto o `alg: none`, ruta inexistente, *path traversal* e HTML forzado a descarga | 7.4 | `security.txt` |
| `faults.sh d1…d6\|all` | D1 réplica corrupta, D2 cambio de réplica en la lectura, D3 worker caído y su regreso, D4 caída de la AZ b, D5 y D6 caída del líder durante una subida y una descarga | 7.4 | `faults-d*.txt` |
| `perf.sh size\|conc\|bsize\|repl\|all` | Throughput según el tamaño del archivo, con 1, 5 y 10 clientes, con bloques de 4, 16 y 64 MiB y con R = 1 frente a R = 2; 5 repeticiones (`REPS`) | 7.3 | `perf-*.csv` |
| `latency.sh` | 100 × `ls`/`mkdir`/`mv`/`rm` y 30 × primer byte del cuerpo de una descarga (`ttfb.py`), contra los objetivos de RNF10 | 7.3 | `latency-*.txt` |
| `placement.sh` | Agrega un worker vacío a un clúster cargado y cuenta a dónde van las copias nuevas (`AvailableSpace`) | 7.3 | `placement.txt` |
| `summarize.py [DIR]` | Medianas [mín–máx], p50/p95, coordenadas para pgfplots y gráficas PNG | 7.3 | `summary.md`, `pgfplots.tex`, `*.png` |

Cada prueba de falla y cada experimento de rendimiento arranca su propio clúster limpio, así que se pueden correr sueltos:

```bash
scripts/bench/stack.sh build
scripts/bench/gen-data.sh
scripts/bench/faults.sh d4                        # solo la caída de una AZ
REPS=3 scripts/bench/perf.sh size                 # menos repeticiones
python3 scripts/bench/summarize.py scripts/bench/results/latest
```

Sin `RESULTS`, los scripts sueltos escriben en `results/latest/` (ignorada por git).

## El clúster de pruebas

`compose.bench.yml` reproduce la topología mínima de AWS bajo el proyecto `clawsha-bench`:

| AZ a | AZ b |
|---|---|
| `master-a`, `worker-a`, `worker-c` | `master-b`, `worker-b` (y `worker-d` con `--profile extra`) |

HAProxy hace de NLB interno: revisa `/health/leader` cada 2 s y manda el gRPC al líder. DynamoDB Local corre en memoria, así que cada `stack.sh up` empieza sin datos. Las fallas son reales: `dd` sobre el archivo de un bloque dentro del contenedor, `docker compose stop` o `kill` de contenedores.

Variables que lee `stack.sh up [BLOCK] [R] [QUOTA]` y el compose:

| Variable | Por defecto | Uso |
|---|---|---|
| `BLOCK_SIZE` | 16 MiB | tamaño de bloque del master |
| `REPLICATION` | 2 | factor de replicación |
| `QUOTA` | 2 GiB (8 GiB en `perf.sh` y `placement.sh`) | cuota de cada worker |
| `SCRUB_SECS` | 30 | intervalo del *scrubber* (600 en producción) |

## Resultados del informe

`results/2026-10-19-informe/` tiene los datos de la campaña del 19 de octubre de 2026, que son los que cita el informe. Ver su `NOTES.md` para saber qué archivo respalda cada tabla.
