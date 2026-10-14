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
│   └── scripts/        # arranque de las instancias (user-data)
├── scripts/chaos/      # pruebas de falla
├── bench/              # benchmarks
├── docs/               # informe y diagramas
└── .github/workflows/  # CI
```

## Correr en local

Hace falta Rust (rustup), `protoc` y una DynamoDB local.

```bash
# 1. base de datos (Java, sin Docker)          # o: docker compose -f deploy/compose.local.yml up -d dynamodb
scripts/dev/dynamodb-local.sh

# 2. pruebas del master contra esa base
scripts/dev/test-master.sh

# 3. un master de verdad
cp crates/master/.env.example crates/master/.env   # el binario lo encuentra desde cualquier directorio
cargo run -p master
curl localhost:8080/health/leader              # 200 = este master es el líder

# 4. dos masters y matar al líder (failover)
scripts/dev/failover-local.sh
```

Guía completa (local hoy, clúster completo y despliegue en AWS):
[docs/running.md](docs/running.md).

Las pruebas del master que usan la base necesitan `DFSHA_TEST_DYNAMO_ENDPOINT`;
sin la variable fallan con instrucciones. Para correr solo las pruebas puras
(sin Java ni Docker): `DFSHA_SKIP_DB_TESTS=1 cargo test`.

## Estado

| Componente | Estado |
| --- | --- |
| `proto/dfs.proto` | `WorkerService`, `MasterService` y `ClusterService` definidos |
| `crates/master` | Metadata en DynamoDB con fencing, namespace, subida/commit atómico, planes de lectura, AvailableSpace, lease de líder y `/health/leader`. Ver `docs/master.md` |
| `crates/worker` | Almacenamiento de bloques en disco con verificación de hash y cuota |
| `crates/gateway`, `frontend/`, `infra/` | Pendientes |
