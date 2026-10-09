/* ROMDUMP: dump the fx-9860G flash ROM to storage memory, 1 MiB at a time.

   Segment N covers 0x80000000 + N * 1 MiB and is written to \\fls0\ROMNN.bin.
   The write routine is the one from gintctl's memory dump (src/gint/dump.c),
   run in the OS world so the OS's own BFile functions write the file.
   Reading only; nothing is ever written to flash outside the file itself. */

#include <gint/display.h>
#include <gint/keyboard.h>
#include <gint/bfile.h>
#include <gint/gint.h>
#include <stdio.h>
#include <stdint.h>

#define ROM_BASE   0x80000000
#define SEG_SIZE   (1 << 20)
#define SEG_COUNT  8
#define CHUNK      1024

static uint32_t chunk_buf[CHUNK / 4];

static void switch_dump(uint32_t start, int size, char const *filename, int *retcode)
{
	uint16_t file[30] = { 0 };
	for(int i = 0; i < 29 && filename[i]; i++) file[i] = filename[i];

	*retcode = 2; /* "started but did not finish" */

	int x = BFile_Remove(file);
	if(x < 0 && x != -1) { *retcode = x; return; }

	x = BFile_Create(file, BFile_File, &size);
	if(x < 0) { *retcode = x; return; }

	int fd = BFile_Open(file, BFile_WriteOnly);
	if(fd < 0) { *retcode = fd; return; }

	/* Storage memory lives on the same flash chip as the ROM, so never hand
	   the OS a pointer into flash: copy each chunk to RAM first. */
	for(int off = 0; off < size; off += CHUNK)
	{
		uint32_t const *src = (void *)(start + off);
		for(int i = 0; i < CHUNK / 4; i++) chunk_buf[i] = src[i];

		x = BFile_Write(fd, chunk_buf, CHUNK);
		if(x < 0) { BFile_Close(fd); *retcode = x; return; }
	}

	x = BFile_Close(fd);
	if(x < 0) { *retcode = x; return; }
	*retcode = 1;
}

/* Simple 32-bit word sum, shown on screen to cross-check the PC copy */
static uint32_t segment_sum(uint32_t start)
{
	uint32_t const *p = (void *)start;
	uint32_t sum = 0;
	for(int i = 0; i < SEG_SIZE / 4; i++) sum += p[i];
	return sum;
}

int main(void)
{
	int segment = 0, retcode = 0;
	uint32_t sum = 0;
	int have_sum = 0;
	char path[30], name[16];

	while(1)
	{
		uint32_t start = ROM_BASE + segment * SEG_SIZE;
		sprintf(name, "ROM%02d.bin", segment);

		dclear(C_WHITE);
		dtext(1, 1,  C_BLACK, "ROM dumper (1 MiB/seg)");
		dprint(1, 11, C_BLACK, "Segment: %d / %d", segment, SEG_COUNT - 1);
		dprint(1, 19, C_BLACK, "Addr: %08X", start);
		dprint(1, 27, C_BLACK, "File: %s", name);
		if(have_sum) dprint(1, 35, C_BLACK, "Sum:  %08X", sum);
		if(retcode == 1) dtext(1, 43, C_BLACK, "Done! Now use LINK.");
		if(retcode == 2) dtext(1, 43, C_BLACK, "Incomplete!");
		if(retcode == 0 && have_sum) dtext(1, 43, C_BLACK, "Not run?!");
		if(retcode < 0)  dprint(1, 43, C_BLACK, "Error %d", retcode);
		dtext(1, 56, C_BLACK, "UP/DN seg EXE dump");
		dupdate();

		int key = getkey().key;
		if(key == KEY_EXIT || key == KEY_MENU) break;
		if(key == KEY_UP)   { segment = (segment + 1) % SEG_COUNT; retcode = 0; have_sum = 0; }
		if(key == KEY_DOWN) { segment = (segment + SEG_COUNT - 1) % SEG_COUNT; retcode = 0; have_sum = 0; }
		if(key == KEY_EXE || key == KEY_F6)
		{
			dtext(1, 43, C_BLACK, "Dumping...");
			dupdate();
			sum = segment_sum(start);
			have_sum = 1;
			sprintf(path, "\\\\fls0\\%s", name);
			retcode = 0;
			gint_world_switch(GINT_CALL(switch_dump, start, SEG_SIZE, (void *)path, &retcode));
		}
	}

	return 1;
}
