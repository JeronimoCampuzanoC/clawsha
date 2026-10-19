# Campaña del 19 de octubre de 2026

Datos que respaldan las tablas y figuras de la sección 7 del informe. Se tomaron en un Apple M5 Pro (15 núcleos, Docker Desktop con 15 CPU y 8 GB), con las imágenes construidas desde la rama de integración de la entrega (equivalente a `main` en 702f793), sobre el clúster de `compose.bench.yml`.

La campaña se corrió con la primera versión de estos scripts. Las mediciones son las mismas, pero algunos formatos difieren de lo que generan los scripts actuales:

- Los CSV de rendimiento se pasaron al formato actual (encabezado y columna `rep`), sin cambiar ningún valor.
- Las latencias de metadata se guardaron en segundos y aquí están convertidas a milisegundos.
- De varias pruebas de falla solo quedó la salida de consola y los registros de los servicios; los `faults-d*.txt` la reproducen tal cual, con los fragmentos de registro relevantes.
- Del primer byte de las descargas (`ttfb.py`) solo se guardó el resumen p50/p95, no las 30 muestras.

`summary.md`, `pgfplots.tex` y los PNG se generan con `python3 summarize.py results/2026-10-19-informe`.

## Qué respalda cada parte del informe

| Informe | Archivo |
|---|---|
| Tabla "Prueba de integridad por tamaño de archivo" | `functional-integrity.txt` |
| Tabla "Casos de prueba funcionales" (F1–F13) | `functional-cases.txt` |
| Figura y tabla "Throughput según el tamaño del archivo" | `perf-size.csv` |
| Figura "Clientes concurrentes" | `perf-conc.csv` (5 repeticiones por nivel) |
| Tabla "Tamaño de bloque" | `perf-bsize.csv` |
| Tabla "Costo de replicar" | `perf-repl.csv` |
| Tabla "Latencia" | `latency-*.txt`, `ttfb-summary.txt` |
| Tabla "Reparto de 140 bloques nuevos" | `placement.txt` |
| D1–D6 | `faults-d1.txt` … `faults-d6.txt` |
| Tabla "Pruebas de seguridad" | `security.txt` |

## Corridas descartadas

- Una primera corrida de `bsize` y `repl` con 3 repeticiones coincidió con compilaciones de LaTeX en la misma máquina (repeticiones de 43–66 MiB/s frente a ~130 MiB/s); se repitió todo con 5 repeticiones sin otra carga propia.
- Un primer `placement` arrancó `worker-d` con 2 GiB de cuota en vez de 8 GiB, de modo que su porcentaje de uso subía cuatro veces más rápido; se repitió con la misma cuota que los demás.
