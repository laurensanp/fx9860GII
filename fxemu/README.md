# fxemu

Emulator für den CASIO fx-9860GII-2 (SH7305). Ohne Argumente öffnet `fxemu` den Rechner
im eigenen Fenster und setzt dort fort, wo er zuletzt geschlossen wurde. Der emulierte
Flash und der gespeicherte Zustand liegen in `%APPDATA%\fxemu`.

Zum Bauen wird ein eigener Firmware-Dump unter `../dump/fx9860gii2_full_4MB.bin` benötigt;
siehe die [Haupt-README](../README.md).

## Optionen

| Option | Wirkung |
|---|---|
| `--rom DATEI` | anderen Dump statt des eingebauten verwenden |
| `--data-dir ORDNER` | anderer Ordner für `flash.bin` und `state.bin` |
| `--web` | Browser-Oberfläche statt des Fensters |
| `--port N` | Port für die Browser-Oberfläche und die Einzelinstanz (Standard 47860) |
| `--no-window` | kein Fenster öffnen, nur mit `--web` sinnvoll |

## Testlauf ohne Fenster

```bash
fxemu --headless SEKUNDEN [Optionen]
```

Der Emulator läuft die angegebene emulierte Zeit und gibt danach einen Bericht aus:
Register, Interrupts, Zugriffe auf nicht nachgebildete Register und das Display als Text.

| Option | Wirkung |
|---|---|
| `--flash DATEI` | Flash-Zustand laden und speichern |
| `--fast` | nicht auf Echtzeit bremsen |
| `--press ZEIT:TASTE` | Taste drücken, z. B. `3.0:EXE`, `5:MENU`, `6:F1` |
| `--screen-every SEK` | Display in diesem Abstand als Text ausgeben |
| `--profile` | häufigste Programmadressen ausgeben |
| `--break HEXADRESSE` | Register ausgeben, sobald diese Adresse erreicht wird |
| `--trace-io` | ersten Zugriff auf jedes nicht nachgebildete Register melden |
| `--trace-all-after SEK` | ab diesem Zeitpunkt alle Peripheriezugriffe melden |
| `--trace-exc` | jede Exception melden |
| `--trace-irq` | Änderungen der Interrupt-Leitung melden |
| `--trace-flash` | Flash-Befehle melden |
