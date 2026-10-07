# demoscene control protocol

The app starts two local control endpoints by default:

| Endpoint | Default | Purpose |
|---|---:|---|
| HTTP | `127.0.0.1:34200` | Browser front-end, schema, state, binary control POSTs |
| UDP | `127.0.0.1:34201` | Realtime binary parameter deltas from external controllers |

Override them with `DEMOSCENE_HTTP_ADDR` and `DEMOSCENE_UDP_ADDR`.

## HTTP

- `GET /` or `/index.html`: dynamic control panel.
- `GET /schema.json`: parameter schema and protocol constants.
- `GET /state.json`: current values and live telemetry.
- `POST /control`: same binary packet accepted by UDP.

The browser front-end batches changed controls into binary delta packets every
5 ms and posts them to `/control`. External tools should use UDP when they need
lower overhead.

## Binary packet

All integers and floats are little-endian. Packet kind `1` is parameter deltas.

| Offset | Size | Type | Value |
|---:|---:|---|---|
| 0 | 4 | bytes | ASCII `DSC1` |
| 4 | 1 | u8 | version, currently `1` |
| 5 | 1 | u8 | kind, currently `1` |
| 6 | 2 | u16 | delta count |
| 8 | 4 | u32 | sequence number |
| 12 | 4 | u32 | FNV-1a checksum of payload bytes |
| 16 | N | deltas | repeated `(u16 param_id, f32 value)` |

The receiver rejects packets with bad magic/version/kind, wrong length, bad
checksum, unknown parameter IDs, non-finite values, or values outside the
schema range. No partial packet application occurs.

The parameter schema is intentionally data-driven. Use `/schema.json` rather
than hard-coding IDs in controllers.

## Minimal Python UDP sender

```python
import socket, struct

def fnv1a32(data):
    h = 0x811C9DC5
    for b in data:
        h ^= b
        h = (h * 0x01000193) & 0xFFFFFFFF
    return h

seq = 1
payload = struct.pack("<Hf", 8, 0.45)  # cy.dimension_pulse
header = b"DSC1" + struct.pack("<BBHI", 1, 1, 1, seq)
packet = header + struct.pack("<I", fnv1a32(payload)) + payload
sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
sock.sendto(packet, ("127.0.0.1", 34201))
```
