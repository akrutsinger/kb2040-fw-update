INCLUDE memory-layout.x
REGION_ALIAS("FLASH", ACTIVE);

/* boot2 belongs to the bootloader. Flashing it from the app would erase the
 * bootloader's first sector. Discarded here rather than with embassy-rp's
 * boot2-none feature, which a workspace build would also apply to the bootloader. */
SECTIONS
{
  /DISCARD/ : { *(.boot2) }
}
