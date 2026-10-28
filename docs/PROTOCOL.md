# PIE / Reflecta DigitDia USB protocol

Reverse-engineered from CyberView X5 5.16.75 (`plugins/DD6000.dll`, the driver plugin for
USB ID `05e3:0142`), and verified against USB captures of CyberView driving a
**Reflecta DigitDia 4000** (INQUIRY model `0x2f`, firmware 1.14) and against the hardware.
The SANE `pieusb` backend documents the same command family for related PIE scanners.

## Transport

The scanner is a parallel-port SCSI design behind a Genesys USB bridge
(one vendor interface: bulk EP `0x81` in, bulk EP `0x02` out, interrupt EP `0x83`).
CyberView talks to it through `usbscan.sys` `IOCTL_WRITE_REGISTERS` (`0x80002010`) /
`IOCTL_READ_REGISTERS` (`0x8000200c`), which become vendor control transfers:

| Transfer | bmRequestType | bRequest | wValue | Data |
|---|---|---|---|---|
| write register | `0x40` | `0x0C` | register | 1 byte |
| read register | `0xC0` | `0x0C` | register | 1 byte |
| bulk header | `0x40` | `0x04` | `0x82` | `[dir,0,0,0,len LE32]` (dir 0 = in, 1 = out) |

Registers: `0x88` IEEE-1284 data, `0x87` IEEE-1284 control, `0x85` SCSI byte FIFO,
`0x84` phase/status, `0x82` bulk size.

**IEEE-1284 select** (`FUN_1002e3c0`): write `ff aa 55 00 ff 87 78 <cmd>` to `0x88`,
3 ms pause, `05`, `04` to `0x87`, `ff` to `0x88`. `cmd`: `0xE0` = SCSI command,
`0x30` = reset, `0x00` = address.

**SCSI command** (`FUN_10030d80`):
1. select `0xE0`, write the 6 CDB bytes to `0x85`, read the phase byte from `0x84`;
2. phase `1` (data in): bulk header with the length, read bulk EP `0x81` (chunks ≤ `0xFFF0`);
   phase `0` (data out): write each byte to `0x85` (< 256 bytes) or bulk header with
   `dir=1` and write EP `0x02`; then read `0x84` again;
3. phase `3` = done; the next read of `0x84` is the SCSI status:
   `0` good, `2` check condition (issue REQUEST SENSE), `8` busy (retry).
4. any other phase: recovery = write `04` to `0x87`, select `0x30`.

**Connect sequence** (capture): `0x87←04`, select `0x30` ×2, select `0x00`, INQUIRY,
select `0x30`/`0x00`, bridge check (`0x8c←84` with wIndex `0x10`, read `0x8e`, twice).

**Timing.** While motors run, the bridge NAKs control transfers for several seconds.
CyberView waits patiently (one TEST UNIT READY in the capture blocked 4.9 s). Hosts that
abort a control transfer pending for ~5 s (e.g. some VM USB passthrough) wedge the bridge
until power-cycle, so the driver stays off the bus for 15 s after SCAN.

## Commands (CDB `[op, 0, 0, len_hi, len_lo, 0]`)

| Op | Name | Notes |
|---|---|---|
| `00` | TEST UNIT READY | `02/04/01` while the lamp warms up |
| `03` | REQUEST SENSE | 14 bytes |
| `08` | READ | count = lines (image/shading) or bytes (parameter blocks) |
| `0A` | WRITE | parameter blocks, see below |
| `0F` | PARAM | 18 bytes: width, lines, bytes/line (LE16) — after SCAN only |
| `12` | INQUIRY | `0xA4` bytes; model at 116 (`0x2f` = DigitDia 4000) |
| `15` | MODE SELECT | 16 bytes, see below |
| `18` | COPY (CCD mask) | `width` bytes; `0` marks CCD elements used at this resolution |
| `1A` | MODE SENSE | 16 bytes |
| `1B` | SCAN | len 1 = start, 0 = stop |
| `D1` | SLIDE | 4 bytes `[action, count/focus LE16, 0]` |
| `D7` | READ GAIN/OFFSET | 103 bytes |
| `DC` | WRITE GAIN/OFFSET | 29 bytes: exposure R,G,B (LE16), offset R,G,B @6, gain R,G,B @12, lamp level @15, IR exposure @18, IR offset @20, IR gain @22 |
| `DD` | READ STATE | 12 bytes: `[2]` magazine position (0-based), `[3]` focus, `[5]` busy, `[6]&7==5` magazine ready, `[6]&0x40` scanning, `[8]` lamp in power-save |
| `F2` | LAMP ON | no data; wakes the lamp from power-save. The answer only comes after the lamp has ignited (> 5 s) |

WRITE blocks (`[code LE16, len LE16, …]`): `0x12` scan frame `[index=0x80, x0, y0, x1, y1]`
in optical (3600 dpi) units; `0x13` exposure `[filter, percent]`; `0x14` highlight/shadow
`[filter, percent]` (filters `2`/`4`/`8` = R/G/B); `0x95` prepare reading the shading
parameters (then READ `0x80`: lines @11, width @12); `0x17` CMD_17 (rejected by the
DigitDia 4000 with `05/26/80`; CyberView ignores that).

MODE SELECT: `[0, 0x0f, res LE16, passes, depth, format=4, 0, order=1, quality, 0, 0, final, 0x80, 0x10, 0]`;
passes `0x80` RGB, `0x90` RGB + infrared; depth `0x04` 8 bit, `0x20` 16 bit;
quality `0x08` = skip shading collection; `final` = 1 for the final (non-preview) scan.

SLIDE actions on the DigitDia 4000: `0x10` load the slide at the current magazine position
into the scanner (byte 1 = focus; fixed `0x73` on this model) — SCAN must follow within
about a second or the slide is returned; `0x04`/`0x05` move the magazine forward/back
by `count`; `0x40` reload the magazine.

## Scan sequence (per slide, from the capture)

1. exposure 100 % and highlight/shadow 100 % for R, G, B; scan frame; CMD_17;
2. READ GAIN/OFFSET, WRITE GAIN/OFFSET; MODE SELECT;
3. SLIDE `[10 73 00 00]`, 0.5 s, TEST UNIT READY, SCAN;
4. on the first scan of a session: D7/DC, READ 4 shading lines, READ the rest minus 4,
   D7/DC, READ 4 (40 lines × R, G, B, I at `2·width+2` bytes, 16 bit, tagged `RR`/`GG`/`BB`/`II`);
5. CCD mask, PARAM, then READ image lines in blocks of ≤ 255; each line starts with a
   two-byte colour tag (`RR`, `GG`, `BB`, `II`); colours arrive interleaved per line;
6. READ STATE until not busy (slide returned to the magazine).

Between slides CyberView sends SLIDE `[04 01 …]` and polls READ STATE until the position
increments. Per slide it previews at 300 dpi with fixed exposures (R/G/B `0x7ac2/0x7582/0x4daf`,
gain `0x19`), derives exposures from the preview, and scans the final image with gains
`0x36/0x3c/0x3f` and correspondingly shorter exposures (`exposure × gain` is preserved;
gain curve from pieusb).

## Lamp power-save

After about 15 minutes without a scan the scanner switches the lamp off and sets READ STATE
byte 8 (polling does not prevent this). Loading a slide in that state fails: SCAN answers
`02/04/01`, the lamp needs about a minute, but the transport gives up on the waiting slide
after ~25 s and latches `04/81/82`. CyberView (`FUN_10019320`) therefore checks byte 8 before
each scan and, if set, waits 2 s, sends LAMP ON (`F2`) and polls READ STATE every ~3 s
until byte 5 (busy) clears, before loading the slide.

## Sense codes seen

| Sense | Meaning |
|---|---|
| `02/04/01` | lamp warming up |
| `05/20/00` | command not valid now (e.g. READ STATE during a scan) |
| `05/26/80` | not supported by this model (CMD_17) |
| `09/30/01` | no slide in the scan position |
| `04/81/82` | slide transport fault (magazine or slide lift disturbed) — power-cycle |
