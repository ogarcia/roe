# rtp: Reticulum sobre Ethernet sin IP

Paquete de prueba para hacer hablar dos o más nodos Reticulum directamente
sobre Ethernet (EtherType `0x88B5`), sin TCP/IP entre ellos.

## Requisitos en cada máquina

- Linux (x86_64 o aarch64, según el paquete).
- `uv` o `python3` con `venv`, e Internet la primera vez para instalar
  Reticulum.
- `sudo` (una sola vez, para dar `CAP_NET_RAW` al binario `rtp`) o ejecutar
  como root.
- Las máquinas deben estar en el mismo segmento de capa 2: mismo switch o
  cable directo. Un router no reenvía estas tramas.

## Uso

```sh
tar xzf rtp-*.tar.gz && cd rtp-*/
./rtp-node setup               # instala Reticulum en ./.venv
./rtp-node start eth0          # arranca el nodo (se queda en primer plano)
```

En otra terminal de la misma máquina:

```sh
./rtp-node id                  # hash de este nodo, para pasarlo al otro
./rtp-node probe <hash-otro>   # ping de Reticulum al otro nodo
./rtp-node status              # estado de la interfaz y tráfico
```

Transferencia de ficheros, para medir rendimiento:

```sh
./rtp-node listen              # en el receptor: muestra su hash
./rtp-node send fichero <hash> # en el emisor, con el hash que mostró listen
```

Todo queda dentro del directorio descomprimido (`.venv`, `node/` y
`received/`). No se toca `~/.reticulum` y no choca con un `rnsd` normal que
ya esté funcionando en la máquina.

## Comprobar que no hay IP de por medio

```sh
sudo tcpdump -e -n -i eth0 ether proto 0x88b5
```

Mientras haces `probe` o `send` deberían aparecer las tramas Reticulum,
todas dirigidas a la dirección de grupo `03:52:4e:53:00:01`. El nodo no
tiene configurada ninguna interfaz IP.

Esa dirección es multicast, no broadcast. La tarjeta de red de una máquina
que no ejecute `rtp` descarta estas tramas por hardware. Por eso todas las
máquinas de la prueba deben usar la misma versión del paquete. Con
`ip maddr show dev eth0` se ve si el nodo se ha suscrito al grupo.

## Problemas conocidos

- Por Wi-Fi puede no funcionar. Algunos puntos de acceso o drivers descartan
  EtherTypes que no conocen.
- `rtp` termina si no puede abrir la interfaz. `rnsd` lo reintenta cada 2 s,
  así que en el log aparecerá el error repetido.
