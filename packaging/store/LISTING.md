# Store-Eintrag für Quarz

## Schritte

1. Im [Microsoft Partner Center](https://partner.microsoft.com/dashboard) als Entwickler anmelden (Einzelperson).
2. Neue App anlegen und den Namen **Quarz** reservieren (ist er vergeben, einen anderen wählen).
3. Unter *Produktidentität* stehen **Package/Identity/Name**, **Package/Identity/Publisher** und **Package/Properties/PublisherDisplayName**.
4. Paket bauen:
   ```text
   powershell -ExecutionPolicy Bypass -File packaging\store\build.ps1 -IdentityName "<Name>" -Publisher "<Publisher>" -PublisherDisplayName "<Anzeigename>"
   ```
   Ergebnis: `target\store\Quarz_0.2.0.0_x64.msix`. Microsoft signiert es beim Hochladen.
5. Paket hochladen, Texte unten einfügen, Screenshots aus `docs/images` hochladen, Preis wählen, zur Prüfung einreichen.

Die Datenschutzerklärung muss unter einer öffentlichen Adresse erreichbar sein, z. B. `PRIVACY.md` in einem öffentlichen Repository oder als GitHub Gist.

Texte zum Einfügen in Partner Center (Store-Einträge). Deutsch und Englisch.

## Deutsch

**Name:** Quarz

**Kurzbeschreibung:** Der Archivierer, der mitdenkt – kleiner als ZIP und 7-Zip, so einfach wie Windows.

**Beschreibung:**

Quarz packt Dateien und Ordner kleiner als ZIP und 7-Zip und öffnet fast jedes Archiv – ohne Fachchinesisch.

Ziehen, loslassen, fertig: Dateien auf das Fenster ziehen oder im Explorer mit der rechten Maustaste „Quarz → packen“ wählen. Ein Doppelklick auf ein Archiv zeigt den Inhalt, ein Klick entpackt alles.

Warum kleiner? Viele Dateien sind schon einmal schlecht gepackt – Programme, Office- und PDF-Dateien, ZIP-Sammlungen, Spielstände. Quarz macht diese alte Kompression bitgenau rückgängig, speichert Doppeltes nur einmal und packt dann neu. Auf echten Ordnern ist das Ergebnis im Schnitt ein Viertel kleiner als mit 7-Zip.

• Öffnet .qz, ZIP, 7z, RAR, TAR, .tar.gz, .tar.xz, .tar.bz2 und .zst
• Passwortschutz mit AES-256 – auch die Dateinamen sind verschlüsselt
• Große Archive in Teile aufteilen und wieder zusammenfügen
• Schutz gegen Beschädigung mit eingebauter Reparatur
• Selbstentpackende .exe für Leute ohne Quarz
• Direkt im Rechtsklick-Menü von Windows 11
• Deutsch, English, Español, Français – hell und dunkel
• Jede Datei kommt Byte für Byte so heraus, wie sie hineinging

Kostenlos für private und andere nicht-kommerzielle Nutzung.

**Schlüsselwörter:** zip, 7z, rar, entpacken, packen, archiv, komprimieren

## English

**Name:** Quarz

**Short description:** The archiver that thinks ahead – smaller than ZIP and 7-Zip, as simple as Windows.

**Description:**

Quarz packs files and folders smaller than ZIP and 7-Zip and opens almost any archive – no jargon.

Drag, drop, done: drop files on the window, or right-click in Explorer and choose “Quarz → Pack”. Double-click an archive to see what's inside, one click extracts everything.

Why smaller? Many files have already been compressed once, badly – programs, Office and PDF files, ZIP collections, game saves. Quarz undoes that old compression bit for bit, stores repeated content once and then compresses again. On real folders the result is on average a quarter smaller than with 7-Zip.

• Opens .qz, ZIP, 7z, RAR, TAR, .tar.gz, .tar.xz, .tar.bz2 and .zst
• AES-256 password protection – file names are encrypted too
• Split large archives into parts and join them again
• Protection against damage with built-in repair
• Self-extracting .exe for people without Quarz
• Right in the Windows 11 right-click menu
• English, Deutsch, Español, Français – light and dark
• Every file comes out byte for byte as it went in

Free for personal and other noncommercial use.

**Keywords:** zip, 7z, rar, extract, compress, archive, unzip

## Weitere Angaben

- **Kategorie:** Dienstprogramme & Tools → Dateimanager
- **Datenschutzerklärung:** Quarz sendet keine Daten ins Internet. Eine Datenschutzerklärung (URL) wird trotzdem verlangt, weil das Paket `runFullTrust` nutzt.
- **Screenshots:** `docs/images/de/*.png` und `docs/images/en/*.png` (mindestens einer pro Sprache)
- **Begründung für `runFullTrust`:** „Quarz ist ein klassisches Desktop-Programm (Win32), das Dateien und Ordner packt und entpackt, die der Nutzer auswählt, und einen Eintrag im Explorer-Kontextmenü bereitstellt.“
