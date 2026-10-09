# fx-9860GII-2 – Firmware-Dump und Emulator

Dieses Projekt hat die komplette Firmware eines CASIO fx-9860GII-2
("USB POWER GRAPHIC 2", Hardware-ID `Gy363007`, OS 02.09.0201) aus dem
echten Gerät ausgelesen und einen eigenen Emulator geschrieben, der diese
Firmware unverändert ausführt. Bootloader und Betriebssystem laufen wie auf dem
Rechner, mit den Einstellungen und Daten, die beim Auslesen darauf gespeichert waren.

**Die Firmware ist nicht in diesem Repository.** Sie gehört CASIO. Zum Bauen brauchst
du einen Dump deines eigenen Rechners; wie man ihn ausliest, steht unten.

<img src="fxemu/res/skin/skin.png" alt="Emulator-Fenster" width="300">

Das Fenster ist dem echten Rechner nachgebildet: silbernes Gehäuse, dunkles Bedienfeld
mit Display, runde REPLAY-Wippe, und über jeder Taste die orange SHIFT-Funktion
und der rote ALPHA-Buchstabe.

## Schnellstart

1. Den 4-MiB-Dump deines Rechners als `dump/fx9860gii2_full_4MB.bin` ablegen
   (siehe [Wie der Dump entsteht](#wie-der-dump-entsteht)).
2. Den Emulator bauen (siehe [Bauen](#bauen)). Die Firmware wird dabei in die exe eingebettet.
3. Die exe doppelklicken.

- Der allererste Start bootet den Rechner (ca. 8 Sekunden, wie beim echten Gerät).
  Danach geht es bei jedem Start genau dort weiter, wo das Fenster geschlossen wurde.
- **Schließen (X)** = Ausschalten. Alles wird gespeichert.
- **SHIFT + AC/ON** schaltet aus, **AC/ON** wieder ein.
- Die fertige exe ist eigenständig (Firmware eingebaut, keine weiteren Dateien nötig).
  Gib sie deshalb nicht weiter. Es läuft immer nur eine Instanz; ein zweiter Start holt das offene Fenster nach vorne.

### Bedienung

| Eingabe | Wirkung |
|---|---|
| Maus | Tasten anklicken (gedrückte Taste leuchtet auf); Pfeile über den Rand der REPLAY-Wippe |
| `0`–`9` `+ - * / ( ) ^ . ,` | werden direkt eingegeben |
| Enter / Backspace / Esc | EXE / DEL / EXIT |
| Pfeiltasten, F1–F6 | wie am Rechner |
| `M` / Pos1 (Home) / Tab | MENU / AC/ON / ALPHA |
| `s` `c` `t` `l` `x` `e` | sin / cos / tan / ln / X,θ,T / EXP |
| SHIFT | nur per Maus (die PC-Umschalttaste wird für `(` und `*` gebraucht) |

Unten im Fenster:
- **Turbo**: läuft so schnell wie möglich, etwa für langsame Graphen.
- **Screenshot**: speichert das Display als PNG in deinen Bilder-Ordner.
- **Restart**: startet den Rechner neu; gespeicherte Dateien bleiben erhalten.

Beim Start passt sich das Fenster an die Bildschirmhöhe an. Es lässt sich größer oder kleiner ziehen; der Rechner skaliert mit.

### Wo die Daten liegen

`%APPDATA%\fxemu\`
- `flash.bin`: der emulierte Flash-Speicher (OS, deine Dateien und Einstellungen)
- `state.bin`: Momentaufnahme des ganzen Rechners zum Fortsetzen

Wer den Ordner löscht, bekommt den Rechner im Zustand vom Zeitpunkt des Dumps zurück.
Der Original-Dump wird dabei nicht verändert.

## Projektinhalt

| Pfad | Inhalt |
|---|---|
| `fxemu/` | Quellcode des Emulators (Rust) |
| `romdump/` | Quellcode und fertiges Dumper-Add-in `ROMDUMP.g1a` für den Rechner |
| `p7.py` | PC-Link-Werkzeug (CASIO Protocol 7.00 über USB) |

Nicht im Repository (per `.gitignore` ausgeschlossen): `dump/` mit der Firmware und die
gebaute `fx9860GII.exe`, weil sie die Firmware enthält.

### Aufteilung des Flash

| Adresse | Inhalt |
|---|---|
| `0x000000` | Bootloader (Marker `CASIOABS`) |
| `0x010000` | Betriebssystem 02.09.0201 (Header `CASIOWIN`) |
| `0x250000` | Sicherung des Hauptspeichers (`CASIOMEMDATA`) |
| `0x270000` | Speicher-Dateisystem (deine Dateien) |
| `0x300000+` | größtenteils leer |

## Wie der Dump entsteht

1. **USB-Verbindung:** Der Rechner meldet sich als `CESG502` (`07CF:6101`). Mit Zadig
   wurde der WinUSB-Treiber installiert; `p7.py` spricht damit CASIOs Protocol 7.00
   (Dokumentation: [Cahute-Projekt](https://cahuteproject.org/)).
   CASIOs FA-124 funktioniert mit WinUSB nicht, bis der Treiber im Geräte-Manager
   zurückgesetzt wird.
2. **Dumper-Add-in:** Das Link-Protokoll kann keinen Flash lesen. Deshalb wurde mit dem
   fxSDK (in WSL) ein kleines Add-in `ROMDUMP.g1a` gebaut. Es kopiert jeweils 1 MiB des
   ROMs als Datei in den Speicher. Weil Speicher und ROM auf demselben Flash-Chip liegen,
   läuft die Kopie über einen RAM-Puffer.
3. **Übertragung:** Pro Segment eine Runde: RomDump ausführen → LINK → RECV →
   `p7.py pull ROM0x.bin` lädt die Datei, löscht sie vom Rechner und optimiert den Speicher.
   Die vier Segmente ergeben aneinandergehängt `fx9860gii2_full_4MB.bin`, unter Windows z. B. mit
   `copy /b ROM00.bin+ROM01.bin+ROM02.bin+ROM03.bin dump\fx9860gii2_full_4MB.bin`.
   Segment 0 wurde per Prüfsumme gegen den Rechner verifiziert, Segment 2 per zweitem Dump.

`p7.py` braucht Python mit `pyusb` und `libusb-package`
(`pip install pyusb libusb-package`). Befehle: `info`, `ls`, `get`, `put`, `pull`, `rm`,
`optimize`, `prep`. Der Rechner muss dafür in LINK → RECV stehen.

Das Add-in baut man im fxSDK mit `fxsdk build-fx` im Ordner `romdump/`.

## Der Emulator (`fxemu/`)

Geschrieben in Rust, ohne Abhängigkeiten bis auf `minifb` für das Windows-Fenster.

| Datei | Aufgabe |
|---|---|
| `src/cpu.rs` | SH-4A-Prozessor (SH4AL-DSP): Befehlssatz, Delay-Slots, Registerbänke, Exceptions, Interrupts, MMU/TLB |
| `src/bus.rs` | Speicherkarte und On-Chip-Peripherie des SH7305 |
| `src/timers.rs` | TMU, ETMU, CMT, Echtzeituhr |
| `src/flash.rs` | NOR-Flash mit AMD/Spansion-Befehlssatz |
| `src/lcd.rs` | Display-Controller (T6K11-kompatibel, 128×64) |
| `src/state.rs` | Speichern/Laden des kompletten Zustands |
| `src/gui.rs` | natives Windows-Fenster |
| `src/web.rs` | Browser-Oberfläche (für `--web` und Nicht-Windows) |
| `src/main.rs` | Start, Zeitsteuerung, Debug-Optionen |
| `res/make_skin.py` | zeichnet die Rechner-Oberfläche im Stil des echten fx-9860GII (Ergebnis in `res/skin/`) |

### Selbst herausgefundene Hardware-Details

Diese Punkte standen in keiner Dokumentation, die wir gefunden haben; sie stammen aus dem OS-Code:

- **Boot-Pin:** Port `0xA405013A` Bit 0 muss 1 lesen, sonst startet das OS im
  "OSUpdate"-Modus.
- **Tastatur (KEYSC):** Status-Register `0xA44B0014`; das untere Byte sind Ereignis-Flags
  (Bit 3 = neue Tastendaten), die durch Zurückschreiben gelöscht werden.
  Die Tastenmatrix liegt in `0xA44B0000–0B`.
- **BCD-Rechenwerk** `0xA4CB0010` (CASIO-eigen): addiert/subtrahiert 8-stellige
  Dezimalzahlen. Operanden in `+0x14`/`+0x18`, Ergebnis in `+0x1C`. Befehl: Bit 0 = Addition
  (sonst a − b), Bit 1 = Übertrag des vorigen Schritts verwenden, Bit 2 = Übertrag 1.
  Das OS rechnet alle Zahlen über dieses Werk.
- **Batterie-ADC** `0xA4610080`, **CMT-Timer** `0xA44A0000`, **DMA** `0xFE008020`.

### Bauen

Voraussetzung: dein Dump liegt unter `dump/fx9860gii2_full_4MB.bin`. Ohne diese Datei
bricht der Build ab.

In WSL Ubuntu mit Rust und `gcc-mingw-w64-x86-64`:

```bash
cd fxemu
cargo build --release --target x86_64-pc-windows-gnu
```

Ergebnis: `target/x86_64-pc-windows-gnu/release/fxemu.exe`. Die Firmware wird beim Bauen
eingebettet. Mit `--rom DATEI` lässt sich zur Laufzeit ein anderer Dump verwenden.

### Debug-Optionen

```bash
fxemu --headless 10 --fast --press 9:EXE --screen-every 2 --profile
```

`--headless` läuft ohne Fenster und gibt am Ende einen Bericht samt Display als Text aus.
Weitere Optionen: `--trace-io`, `--trace-irq`, `--trace-exc`, `--trace-flash`,
`--break ADRESSE`, `--trace-all-after SEKUNDEN`, `--web`, `--rom DATEI`, `--data-dir ORDNER`
(siehe `fxemu/README.md`).

## Grenzen

- Kein USB-Link: Programme/Add-ins lassen sich nicht vom PC in den Emulator übertragen.
- Kein serieller Port (3-Pin-Kabel).
- Zeitverhalten nur ungefähr (nicht taktgenau); die Batterie zeigt immer einen festen Wert.
- Add-ins (`.g1a`) sind ungetestet.
- Die Löschblock-Aufteilung des Flash-Chips ist geschätzt; sehr viele Speicher-Operationen
  könnten den *emulierten* Speicher beschädigen (nie den echten Rechner).

## Rechtliches

Dieses Repository enthält nur eigenen Code. Die Firmware gehört CASIO und ist deshalb
nicht enthalten. Ein eigener Dump und die daraus gebaute exe sind nur für den eigenen
Gebrauch gedacht und dürfen nicht weitergegeben oder veröffentlicht werden.
CASIO und fx-9860GII sind Marken der CASIO Computer Co., Ltd.; dieses Projekt ist kein
Produkt von CASIO.
