# Memory layout

Defined once in `firmware/memory-layout.x`. Each crate's `memory.x` includes it and points
`FLASH` at its own region (`REGION_ALIAS`).

| Region           | Start        | Size        | Notes                                              |
| ---------------- | ------------ | ----------- | -------------------------------------------------- |
| BOOT2            | `0x10000000` | 256 B       | second-stage boot, linked into the bootloader only |
| BOOTLOADER       | `0x10000100` | 24K - 256 B |                                                    |
| BOOTLOADER_STATE | `0x10006000` | 4K          | embassy-boot swap/state flags                      |
| ACTIVE           | `0x10007000` | 768K        | the running app                                    |
| DFU              | `0x100C7000` | 772K        | incoming update; ACTIVE + one 4K page for the swap |
| STORAGE          | `0x107F0000` | 64K         | reserved for data kept across boots (end of flash) |

Flash from `0x10188000` to `0x107F0000` (about 6.4 MB) is unused.

The whole file. The `__bootloader_*` symbols at the end are how embassy-boot, in both the
bootloader and the app, finds the partitions:

```c
{{#include ../../firmware/memory-layout.x}}
```

App size: use `size target/thumbv6m-none-eabi/release/app` (`text + data` is what gets flashed).
The ELF file itself is much bigger because of debug info.
