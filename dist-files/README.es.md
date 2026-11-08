# RoE: Reticulum over Ethernet

<p align="center"><a href="README.md">English</a> · <strong>Español</strong></p>

Paquete de prueba para hacer hablar dos o más nodos Reticulum directamente
sobre Ethernet (EtherType `0x88B5`), sin TCP/IP entre ellos.

## Requisitos en cada máquina

- Linux (x86_64 o aarch64, según el paquete).
- `uv` o `python3` con `venv`, e Internet la primera vez para instalar
  Reticulum.
- `sudo` (una sola vez, para dar `CAP_NET_RAW` al binario `roe`) o ejecutar
  como root.
- Las máquinas deben estar en el mismo segmento de capa 2: mismo switch o
  cable directo. Un router no reenvía estas tramas.

## Uso

```sh
tar xzf roe-*.tar.gz && cd roe-*/
./roe-node setup               # instala Reticulum en ./.venv
./roe-node start eth0          # arranca el nodo (se queda en primer plano)
```

No descomprimas el paquete en `/tmp` ni en otro sistema de ficheros montado
con `nosuid`: ahí se ignoran las capacidades de fichero y `roe` no puede
abrir la interfaz. `roe-node` lo detecta y lo avisa.

En otra terminal de la misma máquina:

```sh
./roe-node id                  # hash de este nodo, para pasarlo al otro
./roe-node probe <hash-otro>   # ping de Reticulum al otro nodo
./roe-node status              # estado de la interfaz y tráfico
```

Transferencia de ficheros, para medir rendimiento:

```sh
./roe-node listen              # en el receptor: muestra su hash
./roe-node send fichero <hash> # en el emisor, con el hash que mostró listen
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
que no ejecute `roe` descarta estas tramas por hardware. Por eso todas las
máquinas de la prueba deben usar la misma versión del paquete. Con
`ip maddr show dev eth0` se ve si el nodo se ha suscrito al grupo.

## Modos de interfaz

`./roe-node start eth0` usa `RoeInterface`, una interfaz de Reticulum
incluida en el paquete que habla con `roe` por un socket `SOCK_SEQPACKET`
(un mensaje por paquete, sin entramado) y usa paquetes de hasta 1432 bytes.
Es el modo normal.

Para comparar rendimiento con la misma prueba de `send`:

- `./roe-node start --pipe eth0` usa la `PipeInterface` de Reticulum con
  `roe`. Es la primera versión: entramado HDLC y paquetes de 500 bytes.
- `./roe-node start --udp eth0` usa la `AutoInterface` de Reticulum (UDP
  sobre IPv6 link-local), sin `roe`. Necesita que el cortafuegos deje pasar
  UDP en los puertos 29716 y 42671; `roe` no lo necesita, porque trabaja por
  debajo de IP.

Con `ROE_VERBOSE=1 ./roe-node start eth0`, `roe` registra cada trama que
envía o recibe.

## Problemas conocidos

- Por Wi-Fi puede no funcionar. Algunos puntos de acceso o drivers descartan
  EtherTypes que no conocen.
- `roe` termina si no puede abrir la interfaz. `rnsd` lo reintenta cada 2 s,
  así que en el log aparecerá el error repetido.
