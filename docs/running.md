# Cómo levantar DFSha

Guía práctica: qué correr en el Mac para probar lo que hay hoy, cómo se verá
el clúster completo en local cuando B y C entreguen sus partes, y cómo queda
desplegado en AWS según el diseño. Los comandos de la sección local están
probados; los de la nube describen lo que falta construir en `infra/`.

## 0. Qué existe hoy

| Componente | Estado | Dueño |
| --- | --- | --- |
| `proto/dfs.proto` | Los 3 servicios definidos (`WorkerService`, `MasterService`, `ClusterService`) | A y B |
| `crates/master` | Completo para P0: metadata, namespace, subida/commit, planes de lectura, lease de líder, `/health/leader`. Ver `docs/master.md` | A |
| `crates/worker` | Solo el almacenamiento en disco (escritura segura, hash, cuota). El binario abre `/data` y termina: todavía no sirve gRPC ni manda heartbeats | B |
| `crates/gateway`, `frontend/` | Vacíos | C |
| `infra/` (Terraform), `infra/scripts` (user-data) | Escrito, sin aplicar en AWS | B |
| `deploy/compose.local.yml` | DynamoDB Local + 2 masters; faltan workers y gateway | A (base) |

Conclusión: hoy se puede levantar la base de datos y los dos masters, hablarles
por gRPC y ver el failover. No hay todavía por dónde subir un archivo de
verdad, porque eso pasa por el gateway y los workers.

## 1. Requisitos en el Mac

Ya están instalados en este equipo (quedaron en el PATH vía `~/.zshrc`):

| Herramienta | Para qué | Instalación |
| --- | --- | --- |
| Rust (`cargo`, `rustc`) | compilar todo | `curl https://sh.rustup.rs -sSf \| sh` |
| `protoc` | `build.rs` compila `dfs.proto` | `brew install protobuf` |
| Java 17+ | DynamoDB Local sin Docker | `brew install openjdk` |
| `grpcurl` | hablarle al master a mano | `brew install grpcurl` |
| Docker Desktop | compose local, construir imágenes | ya estaba (las descargas de imágenes son lentas en este equipo) |

Faltan para las otras partes: `terraform` (B, `brew install terraform`),
Node con `nvm` (C, ya hay nvm) y la AWS CLI (ya está).

Comprobación rápida:

```bash
cargo --version && protoc --version && java -version && grpcurl --version
```

## 2. Local, hoy: base de datos + master

Todo desde la raíz del repo (`code/dfsha`).

**Terminal 1: DynamoDB Local** (en memoria; al cerrarla se pierde todo)

```bash
scripts/dev/dynamodb-local.sh
```

La primera vez descarga el tarball oficial a `.dynamodb-local/` (ignorado por
git). Alternativa con Docker: `docker compose -f deploy/compose.local.yml up dynamodb`.

**Terminal 2: un master**

```bash
cp crates/master/.env.example crates/master/.env   # apunta a localhost:8000 y crea las tablas
cargo run -p master
```

El binario busca el `.env` en el directorio actual y, si no está, en
`crates/master/`, así que da igual desde dónde lo corras. Sin `.env` hay que
pasar al menos `--id`: `cargo run -p master -- --id master-a --dynamodb-endpoint http://localhost:8000 --create-tables`.

Deberías ver `master up, waiting for the leader lease` y enseguida
`became leader fencing=1`. Puertos: gRPC 50051, health 8080.

**Terminal 3: probarlo**

```bash
curl localhost:8080/health          # ok
curl localhost:8080/health/leader   # leader fencing=1  (503 "standby" en el otro master)

# qué servicios expone
grpcurl -plaintext -import-path proto -proto dfs.proto localhost:50051 list

# registrar y entrar (el master devuelve el user_id; el JWT lo hará el gateway)
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d '{"username":"jero","password":"clave-segura-123"}' \
  localhost:50051 dfs.v1.MasterService/Register
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d '{"username":"jero","password":"clave-segura-123"}' \
  localhost:50051 dfs.v1.MasterService/Login
```

Con el `userId` que devuelve (ponlo en `UID`):

```bash
UID=2f66d7f9-...
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d "{\"user_id\":\"$UID\",\"path\":\"/docs\"}" localhost:50051 dfs.v1.MasterService/Mkdir
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d "{\"user_id\":\"$UID\",\"path\":\"/\"}" localhost:50051 dfs.v1.MasterService/List

# un "worker" de mentira saludando (lo que hará el worker real cada 3 s)
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d '{"worker_id":"w1","addr":"127.0.0.1:50052","az":"a","quota_bytes":2147483648,"used_bytes":0}' \
  localhost:50051 dfs.v1.ClusterService/Heartbeat

# lo que verá el dashboard
grpcurl -plaintext -import-path proto -proto dfs.proto localhost:50051 dfs.v1.MasterService/ClusterStatus

# una subida: con un solo worker responde UNAVAILABLE (hacen falta 2, uno por AZ)
grpcurl -plaintext -import-path proto -proto dfs.proto \
  -d "{\"user_id\":\"$UID\",\"path\":\"/docs/a.bin\",\"size\":1000}" \
  localhost:50051 dfs.v1.MasterService/StartUpload
```

Si mandas un segundo heartbeat con `worker_id` `w2` y `az` `b`, el
`StartUpload` ya responde con el plan: un bloque, `block_id` UUID y los dos
workers en orden de pipeline. El flujo completo (commit, plan de lectura) está
recorrido en `crates/master/src/tests/grpc_test.rs`, que es la mejor
referencia de cómo debe llamar el gateway.

**Pruebas**

```bash
scripts/dev/test-master.sh          # cargo test -p master contra localhost:8000
cargo test --workspace              # también el worker (necesita la base arriba)
DFSHA_SKIP_DB_TESTS=1 cargo test    # solo las pruebas puras, sin base
```

**Dos masters y failover**

```bash
scripts/dev/failover-local.sh
```

Levanta `master-a` (gRPC 50061, health 8081) y `master-b` (50062, 8082)
contra la misma base, mira cuál responde 200 en `/health/leader`, lo mata con
`kill -9` y mide cuánto tarda el otro en responder 200. Sale algo como:

```text
leader at health port: 8081
killing the leader (pid 4734) at 18:21:23
new leader at health port 8082 after 9 s
```

Los logs quedan en `/tmp/master-a.log` y `/tmp/master-b.log`. Para hacerlo a
mano, los dos comandos son:

```bash
cargo run -p master -- --id master-a --az a --port 50061 --health-port 8081 \
  --advertise-addr 127.0.0.1:50061 --dynamodb-endpoint http://localhost:8000 --create-tables
cargo run -p master -- --id master-b --az b --port 50062 --health-port 8082 \
  --advertise-addr 127.0.0.1:50062 --dynamodb-endpoint http://localhost:8000 --create-tables
```

## 3. Local, clúster completo

La idea del plan es un solo `compose`:

```bash
docker compose -f deploy/compose.local.yml pull
docker compose -f deploy/compose.local.yml up -d
```

El archivo levanta `dynamodb`, dos masters, dos workers y un gateway:

| Servicio | Quién | Qué necesita |
| --- | --- | --- |
| `worker-a`, `worker-b` | B | `WORKER_ID`, `WORKER_AZ`, `WORKER_DATA_DIR` con un volumen, `WORKER_QUOTA_BYTES` y la dirección del master |
| `gateway` | C | Puerto 3001 en local, secreto del JWT y dirección del master |

Flujo esperado cuando todo esté: el navegador entra al gateway, el gateway
pide `StartUpload` al master líder, manda los bloques a los workers con
`PutBlock`, confirma con `CommitUpload`; los workers mandan heartbeats cada
3 s y el dashboard lee `ClusterStatus`.

Construir solo la imagen del master (para verificar el Dockerfile):

```bash
docker build -f deploy/docker/master.Dockerfile -t dfsha-master .
```

## 4. En AWS (Learner Lab), como está diseñado

Nada de esto existe todavía en `infra/`; es el guion que debe cumplir el
Terraform de B y lo que cada nodo necesita para arrancar.

**Antes de aplicar**

1. Las imágenes Docker de master, worker y gateway publicadas en GHCR por
   GitHub Actions (el workflow actual solo compila y prueba; falta el paso de
   `docker build` + `push`).
2. Iniciar el Learner Lab y copiar las credenciales de "AWS Details" a
   `~/.aws/credentials` (access key, secret y session token). Cambian en cada
   sesión; solo la persona dueña de la cuenta corre Terraform.
3. Un bucket S3 para el estado de Terraform (`backend.tf`).

**Aplicar**

```bash
cd infra
terraform init
terraform apply          # unos minutos
terraform output         # DNS del NLB público y del interno
```

Lo que crea, en `us-east-1`:

| Pieza | Detalle |
| --- | --- |
| Red | VPC `10.0.0.0/16`, 2 AZ, subredes públicas y privadas, 1 NAT, endpoint de VPC para DynamoDB |
| Seguridad | Un security group por rol; ningún puerto 22; acceso por SSM Session Manager; IMDSv2 obligatorio |
| Datos | Las 7 tablas de DynamoDB (abajo), secretos en Secrets Manager (CA, certificados por rol, secreto del JWT) |
| Control | 2 ASG de exactamente 1 instancia `t3.small` (una por AZ): corren master + gateway |
| Workers | 1 ASG de 4 a 7 `t3.micro` en las 2 AZ, volumen de datos de 10 GiB gp3 cifrado; alarma de CloudWatch `DFSha/ClusterUsedPercent > 70 %` durante 2 min → +1 |
| Borde | NLB público `:443` → gateways; NLB interno `:50051` → el master que responda 200 en `:8080/health/leader` |

**Tablas que Terraform debe crear** (exactamente como en
`crates/master/src/db/tables.rs`, facturación por demanda):

| Tabla | Clave de partición | Clave de orden | Índice |
| --- | --- | --- | --- |
| `dfsha_users` | `user_id` (S) | — | |
| `dfsha_usernames` | `username` (S) | — | |
| `dfsha_inodes` | `parent_id` (S) | `name` (S) | |
| `dfsha_versions` | `inode_id` (S) | `version_no` (N) | GSI `by_status`: `status` (S) / `updated_at` (N), proyección ALL |
| `dfsha_blocks` | `version_id` (S) | `idx` (N) | |
| `dfsha_cluster` | `pk` (S) | — | |
| `dfsha_path_leases` | `inode_id` (S) | — | |

Point-in-time recovery activado. El prefijo `dfsha_` es `MASTER_TABLE_PREFIX`.

**Cómo arranca cada nodo** (`infra/scripts/*.sh.tftpl`)

Nodo de control (`control.sh`): instala Docker, lee los secretos con el
`LabInstanceProfile`, averigua su AZ e IP privada por IMDSv2 y corre dos
contenedores:

```bash
docker run -d --name master --restart always -p 50051:50051 -p 8080:8080 \
  -e MASTER_ID=control-$AZ -e MASTER_AZ=$AZ \
  -e MASTER_ADVERTISE_ADDR=$PRIVATE_IP:50051 \
  -e MASTER_TABLE_PREFIX=dfsha_ -e MASTER_CREATE_TABLES=false \
  -e MASTER_REPLICATION=2 -e RUST_LOG=info \
  ghcr.io/<org>/dfsha-master:latest
# + el contenedor del gateway (C): puerto 443, secreto del JWT, DNS del NLB interno
```

Sin `MASTER_DYNAMODB_ENDPOINT` el SDK usa DynamoDB real con las credenciales
del instance profile. La región está fija en `us-east-1` en el código.

Worker (`worker.sh`): formatea y monta el volumen de datos en `/data`, lee su
AZ por IMDSv2 y corre el contenedor del worker con `WORKER_ID` (el instance
id), `WORKER_AZ`, `WORKER_DATA_DIR=/data`, `WORKER_QUOTA_BYTES` y la dirección
del master = DNS del NLB interno `:50051`.

**Verificar que quedó bien**

1. Consola de EC2 → Target groups: el NLB interno tiene 1 target sano (el
   líder) y 1 no sano (el standby); el público, 2 gateways sanos.
2. `aws dynamodb list-tables` muestra las 7 tablas; `aws dynamodb get-item
   --table-name dfsha_cluster --key '{"pk":{"S":"leader"}}'` muestra quién
   lidera y el `fencing`.
3. Entrar a un nodo por SSM (`aws ssm start-session --target <instance-id>`)
   y `curl localhost:8080/health/leader`.
4. Abrir `https://<DNS del NLB público>` (o el subdominio de DuckDNS), entrar,
   subir un archivo de más de 16 MiB y ver sus bloques por AZ en el detalle.
5. Correr las pruebas de caos de `scripts/chaos/` (B) para el video.

**Al terminar cada jornada**

```bash
terraform destroy
```

El NAT y los NLB cobran por hora aunque el lab esté apagado; si se acaba el
presupuesto, la cuenta se desactiva y se pierde todo.

## 5. Qué falta para que "levantar todo" sea real

- **B:** servidor gRPC del worker (`PutBlock`/`GetBlock` en streaming,
  `ReplicateBlock`, `DeleteBlock`), heartbeat cada 3 s a `ClusterService` con
  los bloques nuevos y borrados, `BlockReport` completo al arrancar, cada 5 min
  y cuando cambia el `fencing`; módulo `health` del master (re-replicación,
  reparación, GC con `versions_by_status`/`delete_blocks`, métrica para el
  ASG); Terraform, user-data y scripts de caos.
- **C:** gateway REST (JWT, troceo en frames de 256 KiB, `PutBlock` al primer
  worker con `next_worker`, `CommitUpload` con los acks de las 2 réplicas,
  `GetReadPlan` + `GetBlock` con verificación de hash y fallback), web React,
  CA propia y mTLS, secretos.
- **A:** modo seguro real tras un failover, caché ruta → inodo, marcar bloques
  para moverse cuando las dos réplicas quedaron en la misma AZ, y las secciones
  del informe (`docs/master.md` es el borrador).
