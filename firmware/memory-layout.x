/* Flash layout shared by the bootloader and the app. Each crate's memory.x
 * includes this file and aliases FLASH to its own region. */
MEMORY
{
  BOOT2            : ORIGIN = 0x10000000, LENGTH = 0x100
  BOOTLOADER       : ORIGIN = 0x10000100, LENGTH = 24K - 0x100
  BOOTLOADER_STATE : ORIGIN = 0x10006000, LENGTH = 4K
  ACTIVE           : ORIGIN = 0x10007000, LENGTH = 768K
  /* DFU must be one erase page (4K) larger than ACTIVE for the swap. */
  DFU              : ORIGIN = 0x100C7000, LENGTH = 768K + 4K
  /* Reserved for data kept across boots and updates (keys, logs, ...). */
  STORAGE          : ORIGIN = 0x107F0000, LENGTH = 64K
  RAM              : ORIGIN = 0x20000000, LENGTH = 264K
}

/* Partition offsets from the start of flash, read by embassy-boot. */
__bootloader_state_start = ORIGIN(BOOTLOADER_STATE) - ORIGIN(BOOT2);
__bootloader_state_end = ORIGIN(BOOTLOADER_STATE) + LENGTH(BOOTLOADER_STATE) - ORIGIN(BOOT2);

__bootloader_active_start = ORIGIN(ACTIVE) - ORIGIN(BOOT2);
__bootloader_active_end = ORIGIN(ACTIVE) + LENGTH(ACTIVE) - ORIGIN(BOOT2);

__bootloader_dfu_start = ORIGIN(DFU) - ORIGIN(BOOT2);
__bootloader_dfu_end = ORIGIN(DFU) + LENGTH(DFU) - ORIGIN(BOOT2);
