# DFSha

Sistema de archivos distribuido con alta disponibilidad. Proyecto 1 de SI3007 Sistemas Distribuidos, EAFIT 2026-2.

DFS por bloques tipo HDFS: los archivos se parten en bloques de 16 MiB con 2 réplicas, una por zona de disponibilidad. Dos masters (activo–pasivo) guardan la metadata en DynamoDB, gateways sin estado mueven los bloques y los workers los almacenan. Backend en Rust (tokio, tonic, axum), frontend en React + Vite e infraestructura en Terraform sobre AWS.

## Estructura

```text
dfsha/
├── proto/              # contrato gRPC (dfs.proto)
├── crates/
│   ├── master/         # metadata, namespace, planes, lease de líder,
│   │                   # re-replicación (health), GC y métrica del ASG
│   ├── worker/         # almacenamiento de bloques, pipeline, scrubber, heartbeats
│   └── gateway/        # API REST con JWT, troceo, cifrado y lectura de bloques
├── frontend/           # web React + Vite: archivos y dashboard del clúster
├── deploy/
│   ├── compose.local.yml  # clúster completo en local (2 masters, 3 workers)
│   ├── compose.build.yml  # construir las imágenes desde este checkout
│   └── docker/         # Dockerfiles multi-stage y nginx del frontend
├── infra/
│   ├── modules/        # network, security, data, control, workers, edge
│   └── scripts/        # arranque de las instancias (user-data)
├── scripts/dev/        # DynamoDB Local, pruebas del master, failover local
├── docs/               # guía de arranque y notas del master
└── .github/workflows/  # CI: clippy + pruebas; imágenes a Docker Hub
```

## Correr en local

**Todo el clúster** (Docker): masters, workers, gateway y frontend.

```bash
docker compose -f deploy/compose.local.yml -f deploy/compose.build.yml up -d --build
open http://localhost:8088          # web; el dashboard está en /cluster
```

**Solo el master**, para desarrollarlo: hace falta Rust (rustup), `protoc` y una DynamoDB local.

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
| `proto/dfs.proto` | `WorkerService`, `MasterService` y `ClusterService` |
| `crates/master` | Metadata en DynamoDB con fencing, namespace, subida/commit atómico, planes de lectura, AvailableSpace, lease de líder y `/health/leader`; re-replicación de bloques con menos de R copias vivas; GC de versiones viejas, subidas abandonadas y bloques huérfanos; métrica `DFSha/ClusterUsedPercent` para el ASG. Ver `docs/master.md` |
| `crates/worker` | Bloques en disco con hash y cuota, `PutBlock`/`GetBlock` en streaming con pipeline, `ReplicateBlock`, `DeleteBlock`, heartbeats y block reports, scrubber con cuarentena |
| `crates/gateway` | API REST con JWT (registro, login, listar, carpetas, subir, bajar/previsualizar, borrar, mover, estado del clúster), cifrado de bloques con ChaCha20, verificación de hash y cambio de réplica |
| `frontend/` | Login, explorador de archivos con previsualización y dashboard del clúster (`/cluster`) |
| `infra/` | Terraform: VPC en 2 AZ, DynamoDB, 2 masters con NLB interno, 2 nodos de gateway + frontend con NLB público `:443`, ASG de workers con escalado por uso y CPU |
| Pendiente | mTLS entre servicios, scripts de caos (`scripts/chaos/`), benchmarks |
