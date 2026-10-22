# TPM 1.2 firmware

The SLB 9670 supports either TPM 1.2 or TPM 2.0 firmware. It's the same chip with the same
device ID (`001b`), so wiring and `tpm`'s first two lines can't tell them apart. Everything here
needs TPM 2.0: TPM 1.2 has none of its commands, no elliptic curves (only RSA and SHA-1), and
Infineon's TPM 1.2 firmware before 6.43 generates weak RSA keys
([ROCA, CVE-2017-15361](https://nvd.nist.gov/vuln/detail/CVE-2017-15361)). The TPM9670 I bought off
Amazon said it was a TPM 2.0 module, but came with TPM 1.2 firmware 6.40.190.0.

How it shows up:

- `tpm`: `ERR TPM runs TPM 1.2 firmware, needs 2.0 (see README)` on the third line. `pubkey` and
  `sign` fail with the same message. A TPM 1.2 answers TPM 2.0 commands in its own format (tag
  `0x00C4`), which the driver recognizes.
- A hint in the registers: `INTF_CAPS` bits 30..28 read `010` (TIS 1.3, the TPM 1.2 interface)
  instead of `011`. The revision ID also differs between the firmwares (`10` on mine).

## Switching it to TPM 2.0

Infineon's `TPMFactoryUpd` can change the firmware between TPM 1.2 and TPM 2.0. It runs on Linux
against `/dev/tpm0`, so the easiest place is a Raspberry Pi, which the TPM9670 plugs straight onto.
Infineon doesn't publish the 1.2-to-2.0 image itself, but Supermicro does, for its own SLB 9670
module. Its package goes from TPM 1.2 6.40.190.0 (and 6.41 to 6.43) to TPM 2.0 7.62.3126.0, which
has the ROCA fix and everything this project
needs.

Before you start:

- **Don't interrupt the update** (power, Ctrl-C, reboot). An interrupted update must be resumed with
  the exact same image, and only a few resume attempts are allowed. Use a solid power supply.
- **The number of firmware updates is limited.** `-info` shows how many are left.
- Anything stored in the TPM is lost. Nothing here depends on the TPM 1.2 contents.

Steps:

1. **Raspberry Pi:** install Raspberry Pi OS. Add these lines to `/boot/firmware/config.txt`,
   power off, plug the TPM9670 onto header pins 1 to 26 (its pin 1 on the Pi's pin 1), and boot:

   ```
   dtparam=spi=on
   dtoverlay=tpm-slb9670
   ```

   `ls /dev/tpm0` should show the device, and `dmesg | grep -i tpm` a 1.2 TPM.

2. **Download** from <https://www.supermicro.com/wdl/driver/TPM/> and check the MD5s against
   Supermicro's `CheckSum.txt` there:

   | File                                                     | MD5                                |
   | -------------------------------------------------------- | ---------------------------------- |
   | `AOM-TPM-9670VH_7.62.3126.0.zip`                         | `2c9fc9111ea46200c11dbbb5579dad36` |
   | `TPM_FU_v1.01.2529.00_ToolsOnly_Linux_SourceCode.tar.gz` | `a0c2dae43b28f42134047ab8e6b0d3ac` |

3. **Build the tool.** The source is from 2017, so two fixes are needed for a current Raspberry Pi
   OS (gcc 14, OpenSSL 3): don't turn warnings into errors, and port `Crypt.c` to the OpenSSL 1.1
   API with `tools/tpmfactoryupd-openssl11.patch` from this repo (copy it to the Pi):

   ```sh
   cd ~
   sudo apt install build-essential libssl-dev
   mkdir tpmfu && tar xzf TPM_FU_v1.01.2529.00_ToolsOnly_Linux_SourceCode.tar.gz -C tpmfu
   patch tpmfu/Source/Common/Crypt/Linux/Crypt.c < tpmfactoryupd-openssl11.patch
   cd tpmfu/Source/TPMFactoryUpd
   sed -i 's/-Werror/-Wno-error/' makefile
   make
   ```

   The OpenSSL deprecation warnings that remain are harmless: OpenSSL 3 still has those functions.

4. **Check the TPM:** `sudo ./TPMFactoryUpd -info`. Mine said:

   ```
   TPM family                        :    1.2
   TPM firmware version              :    6.40.190.0
   TPM enabled                       :    No
   TPM activated                     :    No
   TPM owner set                     :    No
   TPM deferred physical presence    :    No (Settable)
   Remaining updates                 :    64
   ```

   Also make sure nothing else uses the TPM during the update: `sudo lsof /dev/tpm0` should print
   nothing, and `/sys/class/misc/hw_random/rng_current` shouldn't be `tpm-rng-0`.

5. **Update.** Unzip the Supermicro package. Which update type works depends on `-info`
   (the tool's manual, `Doc/TPMFactoryUpd_UserManual.pdf` in the tarball, section 5.4):

   - **Owner not set, deferred physical presence `Yes` or `No (Settable)`** (the state above):
     `tpm12-PP`, naming the image for the installed version:

     ```sh
     cd ~/AOM-TPM-9670VH_7.62.3126.0/Firmware
     sudo ~/tpmfu/Source/TPMFactoryUpd/TPMFactoryUpd -update tpm12-PP \
         -firmware TPM12_6.40.190.0_to_TPM20_7.62.3126.0.BIN -log update.log
     ```

   - **Enabled, activated, owner not set:** the package's config file also works. It picks the image
     itself and uses `tpm12-takeownership`, which needs exactly that state:
     `sudo ~/tpmfu/Source/TPMFactoryUpd -update config-file -config TPM20_latest.cfg`, run from the
     `Firmware` folder.

6. **Verify:** reboot the Pi, then `sudo ./TPMFactoryUpd -info` should say TPM family 2.0, firmware
   7.62.3126.0. Move the TPM9670 back to the KB2040, and `tpm` should end with `TPM 2.0, started`.

If `/dev/tpm0` doesn't appear after an interrupted update, the kernel driver likely gave up because
the TPM's self-test failed in its half-updated state. The tarball's `Kernel/` folder has kernel
patches that ignore that failure, so the update can be resumed.
