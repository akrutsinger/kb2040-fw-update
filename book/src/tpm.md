# Wiring

A [52Pi TPM9670](https://wiki.52pi.com/index.php/EP-0149) board: an Infineon
[SLB 9670](https://www.infineon.com/assets/row/public/documents/30/49/infineon-slb-9670vq2.0-datasheet-en.pdf)
TPM 2.0 on a 26-pin Raspberry Pi header. The wiki only labels power and the SPI data pins; CS# is
on CE1 (pin 26), per the Raspberry Pi `tpm-slb9670` overlay.

| TPM9670 pin | Signal | KB2040                            |
| ----------- | ------ | --------------------------------- |
| 1           | 3.3 V  | 3V                                |
| 6           | GND    | GND                               |
| 19          | MOSI   | MO (GP19)                         |
| 20          | GND    | GND, on a wire twisted around SCK |
| 21          | MISO   | MI (GP20)                         |
| 23          | SCLK   | SCK (GP18)                        |
| 26          | CS#    | D10 (GP10)                        |

- RST# and PIRQ# stay unconnected. RST# has an internal pull-up, and the datasheet (section 5.5)
  warns that asserting it at the wrong moment can trip the TPM's dictionary-attack defenses. So
  resetting the KB2040 (watchdog, update) doesn't reset the TPM.
- SPI runs in mode 0 at 1 MHz, which is safe over jumper wires. The chip takes up to 43 MHz.
- The second ground (pin 20) runs on a wire twisted around the SCK wire. I think the SCK picks
  up the edges of MISO (pin 21 is next to pin 23), and the TPM saw extra clock edges. It still
  answered, but with data that was wrong the same way every time. A slower SPI clock didn't help,
  since it's the edges that couple, not the clock rate. On a Raspberry Pi the board plugs straight
  onto the header and doesn't have this problem. Pin 25 isn't a ground on the TPM9670 (no continuity
  to pin 6), so it doesn't help.

`tpm` on the console reads the ID registers, requests locality 0, and starts the TPM:

```
> tpm
Infineon TPM, did 001b rid 10
locality 0 active
TPM 2.0, started
```

If the first two lines fail, it's almost always wiring. embassy-rp leaves MISO floating, so when the
TPM isn't driving it (missing, unpowered, or CS# not reaching it), MISO picks up noise from the
neighboring wires. That shows up as `ERR TPM never ready`, an unexpected vendor, or a locality
that's never granted. Check CS#, MISO, and power. If the wrong values repeat exactly from one `tpm`
to the next (mine read vendor `93a4` and ACCESS `0x82`), noise on SCK is the more likely cause:
check the ground wire twisted around SCK (see above), and keep the jumpers short and apart from
each other. If only the third line fails with `ERR TPM runs TPM 1.2 firmware`, see
[TPM 1.2 firmware](tpm-firmware.md).

## The driver

The driver (`crates/tpm/`) follows the register interface and SPI protocol of the TCG
"PC Client Platform TPM Profile (PTP) Specification". TPM2 commands and responses are encoded and
decoded by [`tpm2-protocol`](https://crates.io/crates/tpm2-protocol) (`no_std`, no allocation). It
takes an `SpiBus` plus a CS# pin rather than an `SpiDevice`, because the TPM can stretch a transfer
with wait states (see `crates/tpm/src/interface/spi.rs`).
