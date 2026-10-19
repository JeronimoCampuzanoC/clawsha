# DFSha (Clawsha)

Sistema de archivos distribuido con alta disponibilidad. Proyecto 1 de SI3007 Sistemas Distribuidos, EAFIT 2026-2.

DFS por bloques tipo HDFS: los archivos se parten en bloques de 16 MiB con 2 réplicas, una por zona de disponibilidad. Dos masters (activo–pasivo) guardan la metadata en DynamoDB, gateways sin estado mueven los bloques y los workers los almacenan. Backend en Rust (tokio, tonic, axum), frontend en React + Vite e infraestructura en Terraform sobre AWS.

## 1. Estructura del repositorio

El backend es un workspace de Cargo con **tres paquetes**, uno por servicio: `master`, `worker` y `gateway`. Comparten un único contrato gRPC en `proto/`. La web vive en `frontend/`, la infraestructura como código en `infra/` y todo lo necesario para correr en contenedores en `deploy/`.

```text
dfsha/
├── Cargo.toml             # workspace: crates/master, crates/worker, crates/gateway
├── proto/                 # contrato gRPC (paquete dfs.v1)
│   ├── dfs.proto          #   punto de entrada: importa los demás
│   ├── common.proto       #   tipos compartidos
│   ├── master.proto       #   MasterService  (gateway → master)
│   ├── cluster.proto      #   ClusterService (worker → master: heartbeats, block reports)
│   └── worker.proto       #   WorkerService  (bloques: PutBlock, GetBlock, Replicate, Delete)
├── crates/
│   ├── master/            # NameNode: metadata en DynamoDB con fencing, namespace,
│   │                      # subida/commit atómico, planes de lectura, AvailableSpace,
│   │                      # lease de líder, re-replicación (health), GC, métrica del ASG
│   ├── worker/            # DataNode: bloques en disco con hash y cuota, streaming,
│   │                      # pipeline, scrubber con cuarentena, heartbeats y block reports
│   └── gateway/           # API REST: JWT, troceo en bloques, cifrado ChaCha20,
│                          # verificación de hash y cambio de réplica
├── frontend/              # web React + Vite: login, explorador de archivos, dashboard
├── deploy/
│   ├── compose.local.yml  # clúster completo en Docker (2 masters, 3 workers, gateway, web)
│   ├── compose.build.yml  # override para construir las imágenes desde este checkout
│   ├── haproxy.cfg        # equivalente local del NLB interno (enruta al master líder)
│   └── docker/            # Dockerfiles multi-stage y plantilla de nginx del frontend
├── infra/                 # Terraform (AWS, us-east-1, 2 AZ)
│   ├── main.tf, variables.tf, outputs.tf, versions.tf, backend.tf
│   ├── terraform.tfvars.example
│   ├── modules/           # network, security, data, control, workers, edge
│   └── scripts/           # user-data de cada rol (instala Docker y arranca el contenedor)
├── scripts/dev/           # DynamoDB Local, pruebas del master, failover local
├── scripts/bench/         # pruebas de sistema y benchmarks del informe (ver su README)
├── docs/                  # running.md (guía completa) y master.md (diseño del master)
└── .github/workflows/     # CI: build, clippy y pruebas; publica las imágenes en Docker Hub
```

## 2. Requisitos previos

**Para correr el sistema en local** solo hace falta Docker:

| Herramienta | Versión | Para qué |
| --- | --- | --- |
| Docker Engine o Docker Desktop | con `docker compose` (v2) | correr todos los contenedores |

No hace falta compilar nada. Las imágenes ya están publicadas en Docker Hub (`samuelmarti22/clawsha-{master,worker,gateway,frontend}:latest`) y basta con descargarlas. Son imágenes `amd64`: en un Mac con Apple Silicon, Docker Desktop las corre emuladas, lo que funciona pero es más lento. Si se prefiere, se pueden construir nativas desde el código (ver 3.1).

**Para desplegar en AWS**, además:

| Herramienta | Versión | Para qué |
| --- | --- | --- |
| Terraform | ≥ 1.6 | crear y destruir la infraestructura |
| AWS CLI | v2 | comprobar las credenciales |
| Cuenta de AWS | AWS Academy Learner Lab o propia | dónde se despliega |

**Solo para desarrollar** (opcional): Rust con `rustup`, `protoc` y Node.js con npm. Ver [docs/running.md](docs/running.md).

## 3. Guía rápida

Todos los comandos se corren desde la raíz del repositorio.

### 3.1 Despliegue local (red de contenedores de Docker)

Levanta el clúster completo en una red de Docker: DynamoDB Local, 2 masters, un HAProxy que hace de NLB interno, 3 workers (dos en la AZ `a` y uno en la `b`), el gateway y la web.

```bash
# 1. descargar las imágenes publicadas
docker compose -f deploy/compose.local.yml pull

# 2. levantar todo en segundo plano
docker compose -f deploy/compose.local.yml up -d

# 3. comprobar que hay un líder (uno responde 200 y el otro 503)
curl localhost:8081/health/leader
curl localhost:8082/health/leader

# 4. abrir la web
open http://localhost:8088        # en Linux: xdg-open http://localhost:8088
```

El gateway puede reiniciarse un par de veces al arrancar mientras se elige el primer líder; es normal.

| Servicio | Puerto en el host |
| --- | --- |
| Web (frontend) | 8088 |
| API REST (gateway) | 3001 |
| Masters: gRPC / health | 50051, 50052 / 8081, 8082 |
| HAProxy (master líder) | 50050 |
| Workers | 50053–50055 |
| DynamoDB Local | 8000 |

Comandos útiles:

```bash
docker compose -f deploy/compose.local.yml ps                 # estado de los contenedores
docker compose -f deploy/compose.local.yml logs -f master-a   # logs de un servicio
docker compose -f deploy/compose.local.yml down               # apagar (los bloques se conservan)
docker compose -f deploy/compose.local.yml down -v            # apagar y borrar los bloques
```

DynamoDB Local corre **en memoria**: al apagarla se pierden usuarios y archivos. Si se apaga sin `-v`, los workers quedan con bloques que nadie referencia; usa `down -v` para empezar de cero.

Para construir las imágenes desde el código en lugar de descargarlas (la primera vez tarda varios minutos):

```bash
docker compose -f deploy/compose.local.yml -f deploy/compose.build.yml up -d --build
```

Pruebas de falla a mano:

```bash
docker stop deploy-master-a-1     # failover: el otro master lidera en ~10 s
docker stop deploy-worker-b-1     # a los 30 s pasa a DEAD y sus bloques se re-replican
docker start deploy-worker-b-1
```

### 3.2 Despliegue en AWS con Terraform

**1. Credenciales.** En el Learner Lab: *Start Lab* → *AWS Details* → *AWS CLI: Show*. Copia el bloque completo en `~/.aws/credentials`:

```ini
[default]
aws_access_key_id=ASIA...
aws_secret_access_key=...
aws_session_token=...
```

Las credenciales del Learner Lab cambian en cada sesión, así que hay que repetir este paso cada vez. Comprueba que funcionan:

```bash
aws sts get-caller-identity
```

**2. Variables.** Copia el ejemplo y pon las imágenes que se van a desplegar:

```bash
cp infra/terraform.tfvars.example infra/terraform.tfvars
```

```hcl
master_image   = "samuelmarti22/clawsha-master:latest"
worker_image   = "samuelmarti22/clawsha-worker:latest"
gateway_image  = "samuelmarti22/clawsha-gateway:latest"
frontend_image = "samuelmarti22/clawsha-frontend:latest"
```

`terraform.tfvars` está ignorado por git. El estado de Terraform queda en `infra/` por defecto; para conservarlo entre sesiones, crea un bucket S3 y descomenta `infra/backend.tf`.

**3. Aplicar.**

```bash
cd infra
terraform init                    # descarga los providers (solo la primera vez)
terraform plan                    # revisar lo que se va a crear
terraform apply                   # escribir "yes"; tarda unos minutos
terraform output                  # api_url, masters_internal_dns, worker_asg, ...
```

Abre `api_url` en el navegador y acepta el certificado (es autofirmado; `terraform output -raw api_ca_pem` lo muestra). Las instancias tardan uno o dos minutos más en descargar las imágenes y arrancar los contenedores.

Lo que crea: una VPC en 2 AZ con subredes públicas y privadas por rol, NAT y endpoint de VPC para DynamoDB; las tablas de DynamoDB; 2 masters `t3.small` detrás de un NLB interno; 2 nodos `t3.small` de gateway + web detrás de un NLB público en `:443`; y un ASG de 3 a 6 workers `t3.micro` con volumen gp3 de 10 GiB. El detalle está en [docs/running.md](docs/running.md#4-en-aws-learner-lab).

**4. Destruir al terminar.** El NAT y los NLB cobran por hora aunque el laboratorio esté apagado:

```bash
cd infra
terraform destroy                 # escribir "yes"
```

## 4. Uso del sistema

Las mismas operaciones se pueden hacer desde la web o directamente contra la API REST (`/api/v1`). En local, la web está en `http://localhost:8088` y la API en `http://localhost:3001`; en AWS, ambas en `api_url`.

| Operación | En la web |
| --- | --- |
| Registrarse e iniciar sesión | En la pantalla de inicio, *Regístrate* para crear la cuenta (contraseña de al menos 8 caracteres) y luego *Entrar*. La sesión dura una hora. |
| Crear una carpeta | Panel *Crear carpeta*: escribir el nombre y pulsar *Crear carpeta*. Se crea dentro de la carpeta actual. |
| Moverse entre carpetas | Doble clic en una carpeta para entrar; la barra de ruta de arriba permite volver a cualquier carpeta anterior. |
| Subir un archivo | Panel *Subir archivo*: elegir el archivo y pulsar *Subir aquí*. Se ve el progreso; si ya existe uno con ese nombre, se guarda como `nombre (1).ext`. |
| Descargar un archivo | Doble clic en el archivo (o *Abrir*) para la vista previa y luego *Descargar*. |

Lo mismo con `curl`, en local:

```bash
API=http://localhost:3001/api/v1

# registrarse e iniciar sesión (guarda el token)
curl -s -X POST $API/auth/register -H 'Content-Type: application/json' \
  -d '{"username":"ana","password":"clave-segura-123"}'
TOKEN=$(curl -s -X POST $API/auth/login -H 'Content-Type: application/json' \
  -d '{"username":"ana","password":"clave-segura-123"}' | sed -E 's/.*"token":"([^"]+)".*/\1/')

# crear una carpeta
curl -s -X POST "$API/dirs?path=/docs" -H "Authorization: Bearer $TOKEN"

# subir un archivo a esa carpeta
curl -s -X PUT "$API/files?path=/docs/informe.pdf" -H "Authorization: Bearer $TOKEN" \
  --data-binary @informe.pdf

# moverse entre carpetas: listar la raíz y luego /docs
curl -s "$API/fs?path=/" -H "Authorization: Bearer $TOKEN"
curl -s "$API/fs?path=/docs" -H "Authorization: Bearer $TOKEN"

# descargar el archivo
curl -s "$API/files?path=/docs/informe.pdf&disposition=attachment" \
  -H "Authorization: Bearer $TOKEN" -o informe-descargado.pdf
```

## 5. Limitaciones conocidas y trabajo futuro

### Limitaciones conocidas

- **Imágenes solo `amd64`.** En Apple Silicon corren emuladas; para ir más rápido hay que construirlas en local (3.1).
- **La metadata local no persiste.** DynamoDB Local corre en memoria: reiniciar el contenedor borra usuarios y archivos.
- **El tráfico interno no va cifrado.** El gRPC entre gateway, masters y workers viaja en texto plano dentro de la VPC (mTLS pendiente). Los bloques sí están cifrados en reposo.
- **Una sola llave de cifrado** para todos los bloques, guardada en los gateways. No hay llaves por usuario ni cifrado de extremo a extremo.
- **Descarga secuencial.** El gateway trae un bloque a la vez y lo verifica completo antes de enviarlo, así que la velocidad de descarga queda limitada por un solo worker y el primer byte tarda lo que tarda un bloque entero.
- **Archivos de escritura única.** No hay escrituras parciales ni `append`; volver a subir un archivo crea una copia numerada.
- **Modo seguro fijo.** Después de un failover, el nuevo líder espera 30 s antes de re-replicar, aunque los workers ya hayan reportado.
- **Los workers solo crecen.** El ASG agrega workers según el uso, pero nunca los retira (no hay drenado de bloques).
- **Learner Lab.** Las credenciales cambian en cada sesión, el certificado HTTPS es autofirmado y el ASG no puede pasar de 6 instancias.
- **Las pruebas de sistema corren en local.** `scripts/bench/` automatiza las fallas y los benchmarks sobre Docker en una sola máquina; falta correrlos contra el despliegue en AWS, con los límites de red reales.

### Trabajo futuro: Clawsha Home

El objetivo a mediano plazo es llevar Clawsha de un laboratorio en AWS a **la casa de cualquiera**, como alternativa propia a Google Drive o Dropbox: tus archivos en tus discos, replicados y cifrados, sin pagar suscripción ni entregarle tus datos a nadie.

1. **Modo homelab.** Un instalador de un comando que corra Clawsha en lo que la gente ya tiene en casa: una Raspberry Pi, un NAS viejo, el PC que ya nadie usa. Para eso:
   - imágenes multi-arquitectura (`arm64` + `amd64`);
   - metadata en SQLite o etcd en lugar de DynamoDB, detrás de la misma capa de acceso con fencing que ya existe;
   - descubrimiento automático de workers en la red local (mDNS), sin tocar ninguna configuración.
2. **Zonas de disponibilidad entre amigos.** Hoy una AZ es un centro de datos de AWS. En Clawsha Home, una "zona" puede ser la casa de un familiar o un amigo: cada bloque guarda una réplica en tu casa y otra en la de alguien de confianza, conectadas por una VPN (WireGuard o Tailscale). Si se te inunda el apartamento, tus fotos siguen vivas en casa de tu tía, cifradas y sin que ella pueda verlas.
3. **Intercambio de capacidad.** Una red federada en la que cada homelab aporta disco y a cambio recibe espacio para sus réplicas en otros. La política de ubicación por espacio disponible (*AvailableSpace*) ya decide dónde poner cada bloque; solo falta agregarle reputación y cuotas por participante.
4. **Cifrado de extremo a extremo.** La llave se deriva de la contraseña del usuario (Argon2id, que ya usamos) y el cifrado ocurre en el navegador o en el cliente. Ni el gateway ni los workers ni el amigo que guarda tus réplicas ven nunca el contenido.
5. **Menos disco por la misma seguridad.** Pasar de 2 réplicas completas a *erasure coding* (Reed–Solomon): con 4 + 2 fragmentos se tolera perder 2 nodos usando 1,5 veces el tamaño del archivo en lugar de 2, que es lo que importa cuando el disco es el de tu casa. A eso se suma deduplicación por hash entre versiones y entre usuarios.
6. **Que se sienta como una carpeta más.** Un cliente de sincronización de escritorio (al estilo de Dropbox), montaje como unidad de red vía WebDAV o FUSE, una app móvil que respalde las fotos automáticamente y enlaces para compartir archivos con permisos y fecha de vencimiento.
7. **Operación sin sustos.** Scripts de caos que apaguen nodos al azar y comprueben que no se pierde nada, benchmarks automáticos en el CI, drenado de workers antes de retirarlos y descargas en paralelo desde varias réplicas para aprovechar todos los discos de la casa.
