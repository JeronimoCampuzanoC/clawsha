# Cómo levantar DFSha

Guía práctica: el clúster completo en local con Docker, el master solo para
desarrollarlo, y el despliegue en AWS con Terraform.

## 0. Qué existe

| Componente | Estado | Dueño |
| --- | --- | --- |
| `proto/dfs.proto` | Los 3 servicios (`WorkerService`, `MasterService`, `ClusterService`) | A y B |
| `crates/master` | Metadata, namespace, subida/commit, planes de lectura, lease de líder, `/health/leader`, re-replicación, GC y métrica del ASG. Ver `docs/master.md` | A (+ health/GC) |
| `crates/worker` | gRPC completo (`PutBlock`/`GetBlock` con pipeline, `ReplicateBlock`, `DeleteBlock`), heartbeats, block reports y scrubber | B |
| `crates/gateway`, `frontend/` | API REST con JWT y cifrado de bloques; web con archivos y dashboard | C |
| `infra/` | Terraform de toda la plataforma (sección 4) | B |
| `deploy/compose.local.yml` | Clúster completo en local (sección 3) | todos |

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

## 2. Solo la base de datos y el master (desarrollo del master)

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

Desde la raíz del repo. Construye las imágenes desde este checkout (la
primera vez tarda varios minutos por la compilación de Rust):

```bash
docker compose -f deploy/compose.local.yml -f deploy/compose.build.yml up -d --build
```

Sin `compose.build.yml` usa las imágenes publicadas en Docker Hub
(`docker compose -f deploy/compose.local.yml pull`), que pueden no tener build
para `arm64`.

| Servicio | Qué es | Puerto en el Mac |
| --- | --- | --- |
| `dynamodb` | DynamoDB Local **en memoria**: si el contenedor se reinicia, se pierden usuarios y archivos | 8000 |
| `master-a`, `master-b` | Los dos masters; uno lidera | gRPC 50051/50052, health 8081/8082 |
| `master-lb` | HAProxy: el equivalente local del NLB interno, enruta al que responde 200 en `/health/leader` | 50050 |
| `worker-a`, `worker-c` (AZ `a`), `worker-b` (AZ `b`) | Workers; con R = 2 el tercero es el que permite re-replicar | 50053–50055 |
| `gateway` | API REST | 3001 |
| `frontend` | Web; `/api/` va al gateway | 8088 |

Abrir `http://localhost:8088`, registrarse y subir un archivo. El dashboard
está en `http://localhost:8088/cluster`: líder, workers por AZ, bloques y
sub-replicados, refrescado cada 3 s.

**Pruebas de falla a mano**

```bash
# failover del master: el otro lidera en ~10 s (ver el dashboard)
docker stop deploy-master-b-1        # o el que sea líder

# worker caído: a los 30 s pasa a DEAD y los sub-replicados vuelven a 0
docker stop deploy-worker-b-1
docker start deploy-worker-b-1       # vuelve; el GC quita las copias sobrantes

# estado crudo, sin el gateway
grpcurl -plaintext -import-path proto -proto dfs.proto \
  localhost:50050 dfs.v1.MasterService/ClusterStatus
```

**Si DynamoDB Local se reinició** (por ejemplo, al reiniciar Docker Desktop),
los workers quedan con bloques que ningún archivo referencia. El GC no los
borra: si la mayoría de los bloques son huérfanos que él no produjo, asume que
la metadata está mal y deja un error en el log del líder. Para empezar de cero:

```bash
docker compose -f deploy/compose.local.yml down
docker volume rm deploy_worker-a-data deploy_worker-b-data deploy_worker-c-data
```

## 4. En AWS (Learner Lab)

**Antes de aplicar**

1. Las imágenes en Docker Hub: el CI las publica en cada push a `main` como
   `<usuario>/clawsha-{master,worker,gateway,frontend}:latest`.
2. Iniciar el Learner Lab y copiar las credenciales de "AWS Details" a
   `~/.aws/credentials` (cambian en cada sesión).
3. `cp infra/terraform.tfvars.example infra/terraform.tfvars` y poner las
   imágenes. Opcional: el bucket S3 del estado en `backend.tf`.

**Aplicar**

```bash
cd infra
terraform init
terraform apply          # unos minutos
terraform output         # api_url, masters_internal_dns, worker_asg, ...
```

Lo que crea, en `us-east-1`:

| Pieza | Detalle |
| --- | --- |
| Red | VPC en 2 AZ, subredes públicas y privadas por rol, NAT, endpoint de VPC para DynamoDB |
| Seguridad | Un security group por rol, sin puerto 22 (acceso por SSM), IMDSv2 obligatorio |
| Datos | Las 7 tablas de DynamoDB con PITR (como en `crates/master/src/db/tables.rs`) |
| Masters | 2 ASG de exactamente 1 instancia `t3.small`, una por AZ, detrás de un NLB interno `:50051` que revisa `/health/leader` |
| Gateways | 1 nodo `t3.small` por AZ con el gateway y el frontend (nginx), detrás de un NLB público `:443` con certificado autofirmado (`api_ca_pem`) |
| Workers | ASG de 3 a 6 `t3.micro` en las 2 AZ, volumen gp3 cifrado de 10 GiB en `/data`; +1 worker si `DFSha/ClusterUsedPercent` pasa del 70 % por 2 min o la CPU por 3 min. Sin scale-in |

El secreto del JWT y la llave de cifrado de bloques los genera Terraform
(`random_password`, `random_id`) y llegan a los gateways por user-data. Los
masters publican la métrica de uso porque `master.sh.tftpl` pone
`MASTER_CLOUDWATCH_NAMESPACE=DFSha`.

**Verificar que quedó bien**

1. EC2 → Target groups: el NLB interno tiene 1 target sano (el líder) y 1 no
   sano (el standby); el público, 2 gateways sanos.
2. Abrir `api_url`, aceptar el certificado, registrarse, subir un archivo de
   más de 16 MiB y mirar `/cluster`.
3. CloudWatch → Metrics → `DFSha` → `ClusterUsedPercent` recibe un punto por
   minuto.
4. Terminar una instancia de worker desde la consola: en el dashboard pasa a
   DEAD a los 30 s, los sub-replicados vuelven a 0 y el ASG lanza otra.

**Al terminar cada jornada**

```bash
terraform destroy
```

El NAT y los NLB cobran por hora aunque el lab esté apagado; si se acaba el
presupuesto, la cuenta se desactiva y se pierde todo.

## 5. Qué falta

- mTLS entre servicios: hoy el gRPC interno va en texto plano dentro de la VPC.
- Scripts de caos en `scripts/chaos/` (las pruebas de falla de la sección 3
  son a mano) y benchmarks.
- Modo seguro "hasta que reporten todos los workers"; hoy son 30 s fijos.
