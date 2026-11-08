# rtp: Reticulum Transmission Protocol

<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

`rtp` (Reticulum Transmission Protocol) permite que los nodos
[Reticulum](https://reticulum.network) de un mismo segmento Ethernet hablen
entre sí directamente en capa 2, con su propio EtherType, en lugar de ir
encapsulados en UDP o TCP sobre IP. No tiene nada que ver con RTP, el
Real-time Transport Protocol que se usa para audio y vídeo.

Reticulum está diseñado para no depender de IP: sobre LoRa, enlaces serie o
radio por paquetes, Reticulum *es* la red. En Ethernet, sin embargo, todas
sus interfaces integradas van sobre la pila IP del sistema. `rtp` cubre ese
hueco: los paquetes de Reticulum van directamente en tramas Ethernet, sin
necesidad de ninguna dirección IP, ni siquiera una IPv6 link-local.

```
 Reticulum ── RtpInterface ══ socket pair ══ rtp ── AF_PACKET ── Ethernet
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
  10 MB fue a 72,7 Mb/s con `rtp`, frente a 53,2 Mb/s con la
  `AutoInterface` de Reticulum, gracias a paquetes de 1432 bytes (consulta
  [docs/testing.md](docs/testing.md)).
- **Separación de privilegios.** Solo el pequeño binario `rtp` necesita
  `CAP_NET_RAW`; `rnsd` funciona sin privilegios.
- **Compatible con el enrutado de Reticulum.** Los nodos de transporte
  reenvían entre segmentos `rtp`, o entre `rtp` y cualquier otra interfaz,
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
- `CAP_NET_RAW` para el binario `rtp`, o ejecutarlo como root.

## Puesta en marcha rápida con un paquete

Los paquetes son kits de prueba autocontenidos: un `rtp` estático, el
módulo `RtpInterface` y el script `rtp-node`, que instala Reticulum en un
venv local y arranca un nodo sin tocar `~/.reticulum`.

```sh
scripts/pack.sh                     # genera dist/rtp-<versión>-{x86_64,aarch64}.tar.gz
```

En cada máquina:

```sh
tar xzf rtp-0.1.0-x86_64.tar.gz && cd rtp-0.1.0-x86_64/
./rtp-node setup                    # instala Reticulum en ./.venv (con uv o pip)
./rtp-node start eth0               # arranca el nodo; pide sudo una vez para setcap
```

Después, desde otra terminal en una de las máquinas:

```sh
./rtp-node id                       # hash de probe de este nodo
./rtp-node probe <hash-del-otro>    # "ping" de Reticulum
./rtp-node listen                   # recibe ficheros en ./received
./rtp-node send <fichero> <hash>    # envía un fichero a un nodo que escucha
```

No descomprimas el paquete en `/tmp` ni en otro sistema de ficheros montado
con `nosuid`: ahí se ignoran las capacidades de fichero y `rtp` no puede
abrir la interfaz. `rtp-node` lo detecta y lo avisa.

## Usar rtp con tu propia instalación de Reticulum

Compila el binario y dale la capacidad que necesita:

```sh
cargo build --release
sudo install -m 755 target/release/rtp /usr/local/bin/rtp
sudo setcap cap_net_raw+ep /usr/local/bin/rtp
```

Instala el módulo de interfaz en el directorio de configuración de
Reticulum:

```sh
mkdir -p ~/.reticulum/interfaces
cp dist-files/RtpInterface.py ~/.reticulum/interfaces/
```

Y añade una interfaz a `~/.reticulum/config`:

```ini
[interfaces]
  [[Ethernet]]
    type = RtpInterface
    interface_enabled = Yes
    device = eth0
    binary = /usr/local/bin/rtp
```

| Opción | Por defecto | Significado |
|---|---|---|
| `device` | (obligatoria) | Interfaz de red que se usa. |
| `binary` | `rtp` del `PATH` | Ruta al binario `rtp`. |
| `respawn_delay` | `5` | Segundos antes de relanzar `rtp` si termina. |
| `verbose` | `No` | Hace que `rtp` registre cada trama que envía o recibe. |

Las opciones estándar de las interfaces de Reticulum (`mode`,
`ifac_netname`, `ifac_netkey`, `bitrate`...) funcionan como siempre.

`rtp` también puede funcionar con la `PipeInterface` integrada en
Reticulum (`command = rtp eth0`). Así no hace falta ningún módulo extra,
pero es unas seis veces más lento; consulta
[docs/testing.md](docs/testing.md).

## Compilación

```sh
cargo build --release                                   # nativo
cargo build --release --target x86_64-unknown-linux-musl
CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER=rust-lld \
    cargo build --release --target aarch64-unknown-linux-musl
```

Los tests necesitan namespaces de usuario sin privilegios y un venv de
Python con Reticulum; consulta [docs/testing.md](docs/testing.md).

## Documentación

La documentación detallada está en inglés:

- [Motivation](docs/motivation.md): por qué Reticulum en Ethernet no
  debería necesitar IP, objetivos y lo que no pretende ser.
- [Protocol](docs/protocol.md): formato de trama, direccionamiento, MTU,
  los dos protocolos entre `rtp` y Reticulum, versionado y seguridad.
- [Architecture](docs/architecture.md): componentes, modelo de
  privilegios, tratamiento de fallos y plataformas.
- [Testing](docs/testing.md): batería de tests y resultados medidos.
