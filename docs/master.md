# Master (NameNode): metadata, planes y alta disponibilidad

Notas de la parte A para el informe y la sustentación. Describe lo que hay en
`crates/master/` tal como quedó implementado; los diagramas generales están en
el documento de arquitectura.

## Qué hace y qué no hace

El master es el plano de control. Sabe qué archivos existen, de qué bloques
se componen y en qué workers está cada bloque; decide dónde van los bloques
nuevos y publica una subida de forma atómica. **Nunca toca bytes de archivos**:
el gateway corta el archivo y habla directo con los workers.

Hay dos masters iguales (uno por AZ). Solo uno actúa a la vez: el que tiene el
*lease de líder* en DynamoDB. El otro responde `UNAVAILABLE` a todo y espera.

## Módulos

| Archivo | Qué resuelve |
| --- | --- |
| `db/` | Única puerta a DynamoDB. Tablas, lecturas y escrituras; todas las escrituras van *fenced* (ver abajo). |
| `namespace.rs` | `ls`, `mkdir`, `rm`/`rmdir`, `mv`, `stat` sobre la tabla `inodes`. Todo por usuario. |
| `auth.rs` | argon2id para contraseñas; reglas de usuario y contraseña. |
| `cluster.rs` | Lo que el líder sabe de los workers, solo en memoria: direcciones, uso, último heartbeat, `block_id → workers`. |
| `placement.rs` | AvailableSpace de HDFS: dos réplicas, una por AZ, con preferencia 0,6 por el worker más vacío. |
| `upload.rs` | `StartUpload`, `ReassignBlock`, `CommitUpload`, `AbortUpload` y el lease de ruta. |
| `read_plan.rs` | `GetReadPlan` y el detalle de `stat` (bloques con réplicas vivas). |
| `leader.rs` | El ciclo del lease de líder: renovar cada 3 s, tomar si está vencido, bajar a standby si falla la renovación. |
| `grpc/` | `MasterService` (gateway → master) y `ClusterService` (worker → master). Solo traducen mensajes. |
| `http.rs` | `/health` (200 si el proceso vive) y `/health/leader` (200 solo en el líder). |

## Tablas en DynamoDB

Todas con facturación por demanda. Las crea Terraform en AWS; en local las crea
el master si arranca con `--create-tables` (ver `db/tables.rs`).

| Tabla | Clave | Atributos | Para qué consulta |
| --- | --- | --- | --- |
| `users` | `user_id` | `username`, `password_hash`, `created_at` | datos del usuario; el JWT lleva el `user_id` |
| `usernames` | `username` | `user_id` | login; se escribe en la misma transacción que `users` con condición de no existir, así no hay dos usuarios con el mismo nombre |
| `inodes` | `parent_id` / `name` | `inode_id`, `owner_id`, `kind` (DIR/FILE), `current_version`, `size`, `modified_at` | `ls` = un Query por `parent_id`; resolver una ruta = un GetItem por nivel; `mv` = mover un ítem |
| `versions` | `inode_id` / `version_no` | `status` (PENDING/COMMITTED/OLD), `size`, `sha256`, `block_count`, `updated_at` | versión actual de un archivo; GC de versiones viejas (índice `by_status`) |
| `blocks` | `version_id` (`<inode_id>#<version_no>`) / `idx` | `block_id`, `size`, `sha256` | lista ordenada de bloques de una versión y el hash esperado de cada uno |
| `cluster` | `pk` = `"leader"` | `holder`, `addr`, `fencing`, `expires_at` | lease de líder |
| `path_leases` | `inode_id` | `holder`, `expires_at` | lock de escritura por ruta |

La raíz de cada usuario es el inodo virtual `root:<user_id>`: no tiene fila,
solo aparece como `parent_id` de sus entradas de primer nivel. Por eso la ruta
de otro usuario simplemente no existe (404), sin ninguna verificación extra.

**Dónde está cada bloque no se guarda en la base.** Lo saben los workers y se
lo cuentan al líder con heartbeats (cada 3 s, con los bloques nuevos) y block
reports (lista completa). Si el líder cambia, el nuevo arranca con el mapa
vacío y lo reconstruye con los reportes, igual que el NameNode de HDFS.

## Fencing: por qué un master viejo no puede hacer daño

Cada escritura del master es una `TransactWriteItems` cuyo primer ítem es un
`ConditionCheck` sobre la fila `leader`: `holder = yo AND fencing = mi número`.
Si otro master tomó el lease, su `fencing` es mayor, la condición falla y
DynamoDB rechaza la transacción completa. En el código eso es
`MasterError::Fenced` y sale de un solo lugar: `db::fenced_write`.

Las únicas escrituras que no pasan por ahí son las dos del lease mismo
(`try_acquire_leader`, `renew_leader`): ellas *son* el fence.

## Flujo de subida

1. `StartUpload(path, size)`: toma el lease de la ruta (60 s; si otro lo
   tiene, 409), crea la versión `PENDING` con número `último + 1`, calcula
   `ceil(size / 16 MiB)` bloques, les asigna UUID y un par de workers (uno por
   AZ), y escribe las filas de `blocks` sin hash. Si el archivo es nuevo, crea
   el inodo con `current_version = 0`: existe para el que escribe pero sigue
   invisible para `ls` y `stat`.
2. El gateway escribe los bloques en los workers. Si un par falla, pide
   `ReassignBlock`: nuevo UUID, nuevo par, misma posición.
3. `CommitUpload`: valida el reporte (tantos bloques como toca, en orden,
   tamaños que suman, hash de 32 bytes y **al menos R = 2 copias por bloque**),
   reescribe `blocks` con los hashes y hace **una transacción**: versión a
   `COMMITTED`, inodo apunta a ella, versión anterior a `OLD`, lease liberado.
   Después registra en memoria las ubicaciones que confirmaron los acks.
4. `AbortUpload`: versión a `OLD` (el GC borra sus bloques) y lease liberado.
   Si el archivo nunca tuvo una versión confirmada, se borra también el inodo
   fantasma.

Reemplazar un archivo es volver a subirlo: los lectores ven la versión vieja
completa hasta el commit y la nueva completa después, nunca una mezcla.

## Ubicación de réplicas (AvailableSpace)

Para cada bloque:

1. Candidatos: workers `ALIVE` (heartbeat hace menos de 10 s) con espacio libre
   para al menos 2 bloques.
2. Primera réplica: dos candidatos al azar; si su porcentaje de uso difiere en
   más de 5 puntos, gana el más vacío con probabilidad 0,6; si no, moneda.
   Son los valores por defecto de Hadoop.
3. Segunda réplica: lo mismo pero solo entre workers de **otra AZ**. Si la otra
   AZ no tiene candidatos, otro host de la misma AZ.
4. Dentro de un mismo plan, los bytes ya asignados se le cargan al worker en
   la copia local de candidatos, para que un archivo grande no caiga entero en
   el que estaba más vacío al principio.

## Lectura

`GetReadPlan(path)` resuelve el inodo, toma la versión actual (o la que fija
un `open()`), lista sus bloques y les pega las réplicas que el líder cree vivas
(`ALIVE` o `SUSPECT`; nunca `DEAD`). Si algún bloque no tiene ninguna réplica
viva, responde `UNAVAILABLE` (503) y el gateway reintenta: típico justo después
de un failover, mientras llegan los block reports.

`ReportCorruption(block, worker)` saca esa réplica del mapa; la orden de
reparación la da el módulo de salud (B).

## Failover del líder

- El líder renueva el lease cada 3 s; el lease dura 10 s.
- El standby intenta tomarlo cada 3 s con una escritura condicional
  (`attribute_not_exists(holder) OR expires_at < now`). Al ganar, `fencing + 1`
  y borra su mapa de workers: los workers ven el fencing nuevo en la respuesta
  de su heartbeat y mandan el block report completo.
- El NLB interno revisa `/health/leader` cada 5 s; con dos 200 seguidos manda
  el tráfico al nuevo líder. Gateways y workers siempre llaman al mismo DNS.
- Si el viejo líder vuelve, su renovación falla por la condición
  `holder = yo AND fencing = mío`, pasa a standby y cualquier escritura que
  tuviera en vuelo muere en el `ConditionCheck`.
- Los primeros 30 s tras un failover son "modo seguro": nada destructivo
  (re-replicar, borrar) hasta que los workers hayan reportado.

Prueba local: `scripts/dev/failover-local.sh` levanta dos masters contra
DynamoDB Local, mata al líder y mide cuánto tarda el otro en responder 200.

## Códigos de error

| gRPC | HTTP (gateway) | Cuándo |
| --- | --- | --- |
| `NOT_FOUND` | 404 | la ruta no existe o es de otro usuario |
| `ALREADY_EXISTS` | 409 | `mkdir` sobre un nombre ocupado; username repetido |
| `FAILED_PRECONDITION` | 409 | carpeta no vacía; ruta con lease de otro escritor |
| `INVALID_ARGUMENT` | 400 | ruta relativa o con `..`; contraseña corta; reporte de commit inconsistente |
| `UNAUTHENTICATED` | 401 | usuario o contraseña incorrectos |
| `UNAVAILABLE` | 503 | no soy el líder; no hay 2 workers para escribir; un bloque sin réplica viva |

## Cómo correrlo en local

```bash
scripts/dev/dynamodb-local.sh                 # DynamoDB Local en :8000 (Java)
scripts/dev/test-master.sh                    # cargo test -p master contra esa base
cargo run -p master -- --id master-a --dynamodb-endpoint http://localhost:8000 --create-tables
curl localhost:8080/health/leader             # 200 = líder
```

Variables de entorno en `crates/master/.env.example`.

## Pendiente / trabajo futuro de A

- Modo seguro: hoy es solo un flag por tiempo (30 s); falta que el líder espere
  a que reporten todos los workers que conocía.
- Caché LRU `ruta → inodo` (hoy cada resolución es un GetItem por nivel).
- Marcar un bloque para moverse cuando las dos réplicas quedaron en la misma AZ.
- Un `lock()` que nunca se usa deja un inodo oculto con el nombre reservado.
