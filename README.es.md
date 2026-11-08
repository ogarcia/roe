# RoE: Reticulum over Ethernet

<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

RoE (Reticulum over Ethernet) permite que los nodos
[Reticulum](https://reticulum.network) de un mismo segmento Ethernet hablen
entre sí directamente en capa 2, con su propio EtherType, en lugar de ir
encapsulados en UDP o TCP sobre IP. No tiene nada que ver con IEEE 1914.3,
también conocido como RoE (Radio over Ethernet).

Reticulum está diseñado para no depender de IP: sobre LoRa, enlaces serie o
radio por paquetes, Reticulum *es* la red. En Ethernet, sin embargo, todas
sus interfaces integradas van sobre la pila IP del sistema. `roe` cubre ese
hueco: los paquetes de Reticulum van directamente en tramas Ethernet, sin
necesidad de ninguna dirección IP, ni siquiera una IPv6 link-local.

```
 Reticulum ── RoeInterface ══ socket pair ══ roe ── AF_PACKET ── Ethernet
 (rnsd, Python)                             (Rust, CAP_NET_RAW)    EtherType 0x88B5
```

> **Estado: experimental.** Funciona, está probado (consulta
> [docs/testing.md](docs/testing.md)) y en hardware real es más rápido que el
> transporte UDP del propio Reticulum, pero el formato de trama aún puede
> cambiar.

## Características

- **Sin IP entre nodos.** Las tramas usan el EtherType experimental local
  `0x88B5` de IEEE y una dirección multicast de grupo administrada
  localmente.
- **Más rápido que UDP.** Entre dos máquinas físicas, una transferencia de
  10 MB fue a 72,7 Mb/s con `roe`, frente a 53,2 Mb/s con la
  `AutoInterface` de Reticulum, gracias a paquetes de 1432 bytes (consulta
  [docs/testing.md](docs/testing.md)).
- **Separación de privilegios.** Solo el pequeño binario `roe` necesita
  `CAP_NET_RAW`; `rnsd` funciona sin privilegios.
- **Compatible con el enrutado de Reticulum.** Los nodos de transporte
  reenvían entre segmentos `roe`, o entre `roe` y cualquier otra interfaz,
  como con cualquier interfaz de Reticulum.
- **Se recupera solo.** Aguanta que la interfaz caiga, que se desenchufe el
  cable, que una tarjeta de red desaparezca y vuelva, y que el proceso
  auxiliar falle.
- **Binarios estáticos** para x86_64 y aarch64 (musl), sin dependencias en
  tiempo de ejecución.

## Requisitos

- Linux (sockets en bruto `AF_PACKET`).
- Reticulum 1.6.0 o posterior (probado con 1.6.0).
- Todos los nodos en el mismo segmento de capa 2 (mismo switch, bridge o
  cable). Los routers no reenvían estas tramas; los nodos de transporte de
  Reticulum, sí.
- `CAP_NET_RAW` para el binario `roe`, o ejecutarlo como root.

## Instalación

### Arch Linux

El [paquete de AUR](https://aur.archlinux.org/packages/reticulum-over-ethernet)
instala `/usr/bin/roe` con `CAP_NET_RAW` ya asignada, y el módulo
`RoeInterface` en `/usr/share/roe/`:

```sh
yay -S reticulum-over-ethernet      # o cualquier otro asistente de AUR
```

### Binarios precompilados

Cada [release](https://github.com/ogarcia/roe/releases) incluye binarios
estáticos para x86_64 y aarch64, que funcionan en cualquier distribución
Linux:

```sh
curl -LO https://github.com/ogarcia/roe/releases/latest/download/roe-x86_64-linux-musl
curl -LO https://github.com/ogarcia/roe/releases/latest/download/SHA256SUMS
sha256sum --ignore-missing -c SHA256SUMS
sudo install -m 755 roe-x86_64-linux-musl /usr/local/bin/roe
sudo setcap cap_net_raw+ep /usr/local/bin/roe
```

`RoeInterface.py` va incluido en los paquetes de prueba de la misma release.

### Contenedor

Hay una imagen publicada con Reticulum, `roe` y `RoeInterface`, para
x86_64 y aarch64:

```sh
podman run -d --name roe --network host --cap-add NET_RAW \
    -e ROE_DEVICE=eth0 -v roe-data:/data ghcr.io/ogarcia/roe:master
```

Para una release, usa la versión en lugar de `master`. El `Containerfile`
construye la misma imagen en local.

Consulta [docs/container.md](docs/container.md) (en inglés).

### Desde el código fuente

```sh
cargo build --release
sudo install -m 755 target/release/roe /usr/local/bin/roe
sudo setcap cap_net_raw+ep /usr/local/bin/roe
```

`RoeInterface.py` está en `dist-files/`.

## Puesta en marcha rápida con un paquete

Los paquetes son kits de prueba autocontenidos: un `roe` estático, el
módulo `RoeInterface` y el script `roe-node`, que instala Reticulum en un
venv local y arranca un nodo sin tocar `~/.reticulum`.

```sh
scripts/pack.sh                     # genera dist/roe-<versión>-{x86_64,aarch64}.tar.gz
```

En cada máquina:

```sh
tar xzf roe-*-x86_64.tar.gz && cd roe-*-x86_64/
./roe-node setup                    # instala Reticulum en ./.venv (con uv o pip)
./roe-node start eth0               # arranca el nodo; pide sudo una vez para setcap
```

Después, desde otra terminal en una de las máquinas:

```sh
./roe-node id                       # hash de probe de este nodo
./roe-node probe <hash-del-otro>    # "ping" de Reticulum
./roe-node listen                   # recibe ficheros en ./received
./roe-node send <fichero> <hash>    # envía un fichero a un nodo que escucha
```

No descomprimas el paquete en `/tmp` ni en otro sistema de ficheros montado
con `nosuid`: ahí se ignoran las capacidades de fichero y `roe` no puede
abrir la interfaz. `roe-node` lo detecta y lo avisa.

## Usar RoE con tu propia instalación de Reticulum

Con `roe` ya [instalado](#instalación), pon el módulo de interfaz en el
directorio de configuración de Reticulum. Con el paquete de AUR, enlázalo
para que le lleguen las actualizaciones del paquete (un `/etc/reticulum` a
nivel de sistema ya tiene el enlace):

```sh
mkdir -p ~/.reticulum/interfaces
ln -s /usr/share/roe/RoeInterface.py ~/.reticulum/interfaces/
```

Si no, cópialo de un paquete de prueba o de `dist-files/`:

```sh
mkdir -p ~/.reticulum/interfaces
cp dist-files/RoeInterface.py ~/.reticulum/interfaces/
```

Y añade una interfaz a `~/.reticulum/config`:

```ini
[interfaces]
  [[Ethernet]]
    type = RoeInterface
    interface_enabled = Yes
    device = eth0
    binary = /usr/local/bin/roe
```

| Opción | Por defecto | Significado |
|---|---|---|
| `device` | (obligatoria) | Interfaz de red que se usa. |
| `binary` | `roe` del `PATH` | Ruta al binario `roe` (`/usr/bin/roe` con el paquete de AUR). |
| `respawn_delay` | `5` | Segundos antes de relanzar `roe` si termina. |
| `verbose` | `No` | Hace que `roe` registre cada trama que envía o recibe. |

Las opciones estándar de las interfaces de Reticulum (`mode`,
`network_name`, `passphrase`, `ifac_size`, `bitrate`...) funcionan como
siempre.

`roe` también puede funcionar con la `PipeInterface` integrada en
Reticulum (`command = roe eth0`). Así no hace falta ningún módulo extra,
pero es unas seis veces más lento; consulta
[docs/testing.md](docs/testing.md).

## Como biblioteca

El crate también expone las piezas con las que está hecho `roe`: el formato
de trama, el socket en bruto y los protocolos hacia Reticulum. Están
pensadas para proyectos que quieran hablar RoE directamente, como una
implementación de Reticulum en Rust. Consulta la
[documentación de la API](https://docs.rs/reticulum-over-ethernet) (en
inglés).

```sh
cargo add reticulum-over-ethernet
```

## Compilación

```sh
cargo build --release                                   # nativo
cargo build --release --target x86_64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build --release --target aarch64-unknown-linux-musl
```

Los tests necesitan namespaces de usuario sin privilegios y un venv de
Python con Reticulum; consulta [docs/testing.md](docs/testing.md).

Para pasar antes de cada commit las mismas comprobaciones de formato y
lint que el CI (rustfmt, clippy y, si está instalado, ruff), activa los
hooks del repositorio una vez por clon:

```sh
git config core.hooksPath .githooks
```

## Documentación

La documentación detallada está en inglés:

- [Motivation](docs/motivation.md): por qué Reticulum en Ethernet no
  debería necesitar IP, objetivos y lo que no pretende ser.
- [Protocol](docs/protocol.md): formato de trama, direccionamiento, MTU,
  los dos protocolos entre `roe` y Reticulum, versionado y seguridad.
- [Architecture](docs/architecture.md): componentes, modelo de
  privilegios, tratamiento de fallos y plataformas.
- [Testing](docs/testing.md): batería de tests y resultados medidos.
- [Running as a service](docs/deployment.md): unidades de systemd, y cómo
  obtiene `roe` `CAP_NET_RAW` cuando el servicio está aislado.
- [Container](docs/container.md): cómo construir y ejecutar la imagen de
  contenedor.
