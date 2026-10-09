fxemu – Emulator für den CASIO fx-9860GII-2 (SH7305)

Ohne Argumente: öffnet den Rechner im eigenen Fenster und setzt dort fort,
wo er zuletzt geschlossen wurde. Daten liegen in %APPDATA%\fxemu.

Optionen für den normalen Betrieb:
  --rom DATEI          andere Firmware statt der eingebauten verwenden
  --data-dir ORDNER    anderer Ordner für flash.bin und state.bin
  --web                Browser-Oberfläche statt des Fensters
  --port N             Port für die Browser-Oberfläche / Einzelinstanz (Standard 47860)
  --no-window          kein Fenster öffnen (nur mit --web sinnvoll)

Testlauf ohne Fenster:
  fxemu --headless SEKUNDEN [Optionen]
      Läuft die angegebene emulierte Zeit und gibt dann einen Bericht aus
      (Register, Interrupts, nicht nachgebildete Register, Display als Text).
  --flash DATEI          Flash-Zustand laden/speichern
  --fast                 nicht auf Echtzeit bremsen
  --press ZEIT:TASTE     Taste drücken, z. B. 3.0:EXE, 5:MENU, 6:F1
  --screen-every SEK     Display regelmäßig als Text ausgeben
  --profile              häufigste Programmadressen ausgeben
  --break HEXADRESSE     Register ausgeben, wenn diese Adresse erreicht wird
  --trace-io             ersten Zugriff auf jedes nicht nachgebildete Register melden
  --trace-all-after SEK  ab dieser Zeit alle Peripheriezugriffe melden
  --trace-exc            jede Exception melden
  --trace-irq            Änderungen der Interrupt-Leitung melden
  --trace-flash          Flash-Befehle melden
