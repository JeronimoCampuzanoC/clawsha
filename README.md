# DFSha

Sistema de archivos distribuido con alta disponibilidad. Proyecto 1 de SI3007 Sistemas Distribuidos, EAFIT 2026-2.

DFS por bloques tipo HDFS: los archivos se parten en bloques de 16 MiB con 2 réplicas, una por zona de disponibilidad. Dos masters (activo–pasivo) guardan la metadata en DynamoDB, gateways sin estado mueven los bloques y los workers los almacenan. Backend en Rust (tokio, tonic, axum), frontend en React + Vite e infraestructura en Terraform sobre AWS.

## Estructura

```text
dfsha/
├── proto/              # contrato gRPC (dfs.proto)
├── crates/
│   ├── common/         # config, TLS/mTLS, hashing, errores, tracing
│   ├── master/         # namespace, planes, lease de líder, salud de bloques
│   ├── worker/         # almacenamiento de bloques, pipeline, scrubber
│   └── gateway/        # API REST, troceo y lectura de bloques
├── frontend/           # web React + Vite (la sirve el gateway)
├── deploy/
│   └── docker/         # Dockerfiles multi-stage y compose local
├── infra/
│   ├── modules/        # network, security, data, control, workers, edge
│   └── user-data/      # arranque de las instancias
├── scripts/chaos/      # pruebas de falla
├── bench/              # benchmarks
├── docs/               # informe y diagramas
└── .github/workflows/  # CI
```

## Estado

Esqueleto inicial: solo carpetas.
