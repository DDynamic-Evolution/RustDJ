# RustDJ 0.18

Native DJ-Anwendung in Rust für Windows. Das kompakte Pro-Layout orientiert sich
an der gewünschten Anordnung klassischer DJ-Software: laufende Wellenformen
oben, Decks links/rechts, ein zentraler Mixer und der Musikbrowser mit Sidelist unten.
Gain, EQ und Kanal-Fader liegen im Mixer-Tab. Der DJ-202 verbindet sich automatisch.
Der Musikbrowser zeigt BPM und Tonart und speichert Analysen dauerhaft in SQLite.
Im Tab PLAYLIST lassen sich eigene Titellisten erstellen, speichern und laden.
Musikdateien lassen sich zwischen Browser, Listen und Decks ziehen.
Alle vier Decks haben acht gespeicherte Hotcues sowie acht direkte Loop-Buttons,
die auch über die DJ-202-Pads bedient werden. Kompaktere Decks geben dem Mixer
mehr Platz für Kanal- und Ausgangspegel.
Farbige Detail-Wellenformen trennen Tiefen, Mitten und Höhen. Alle vier Decks bieten
einen LP-/HP-Filter und drei Effekte mit gemeinsamer FX-Tiefe.
Jedes Deck hat seine eigene Farbe: **1 Lila, 3 Pink, 4 Türkis, 2 Gelb**.
Rahmen, Regler, Pegelanzeigen und Lade-Buttons verwenden die jeweilige Deck-Farbe.
Über **STREAMER** im zentralen Mixer lässt sich der Master an SHOUTcast senden
und unabhängig davon als MP3 aufnehmen.
**MASTER**, **HEADPHONES** und **CUE / MASTER** sind unter den Kanalreglern
dauerhaft sichtbar, auch im Tab MIXER. Alle drei Werte werden gespeichert.

## Start

`target\release\rustdj.exe` starten. Der Build läuft ohne Rust-Installation.
Musik im Browser auswählen oder auf ein Deck ziehen und mit **Play** starten.
Die Kopfzeile zeigt den Controller ausdrücklich als **ON** oder **OFF** sowie
die ausgewählten Master-/Kopfhörer-Soundkarten und Kanalpaare.

## Musikbrowser

- Der Musikbrowser zeigt Ordner und unterstützte Musikdateien.
- **Ordner …** wählt einen Musikordner; alternativ einen Pfad eingeben und Enter
  oder **Gehe zu** drücken. **Hoch** führt in den übergeordneten Ordner.
- Ordner per Doppelklick öffnen. Eine Datei auswählen und die Lade-Buttons **1, 3, 4, 2** anklicken.
  Ein Doppelklick auf eine Datei lädt sie in das aktive Deck.
- Die Suche filtert die Einträge des aktuellen Ordners, ohne Unterordner rekursiv zu durchsuchen.
- **Favorit +** merkt sich einen Ordner in der Seitenleiste; daneben stehen Laufwerke bereit.
- Der zuletzt besuchte Ordner und Favoriten bleiben nach einem Neustart erhalten.
- Musikdateien lassen sich auch direkt auf ein Deck ziehen; ein abgelegter Ordner
  öffnet sich im Browser.
- **SESSION / SIDELIST** zeigt die in dieser Sitzung erfolgreich geladenen Tracks.

## Drag-and-drop

- Eine Musikzeile im Browser mit der linken Maustaste greifen und auf ein beliebiges Deck ziehen. Beim Loslassen wird der Titel auf diesem Deck geladen.
- Auf **PLAYLIST** oder **SESSION** ziehen, um den Titel der jeweiligen Liste
  hinzuzufügen. Auch der Tab einer gerade ausgeblendeten Liste nimmt Titel an.
  Die Datei bleibt im Musikbrowser und auf dem Datenträger erhalten.
- Eine leere Playlist nimmt Titel direkt auf der freien Fläche an. Bei einer
  gefüllten Liste markiert eine Linie die Einfügeposition. Unterhalb der Einträge
  wird der Titel angehängt.
- Titel aus Playlist und Sidelist lassen sich ebenfalls auf alle vier Decks ziehen.
  Innerhalb der Playlist ändert Ziehen die Reihenfolge, ohne den Titel zu duplizieren.
- Der Dateiname folgt beim Ziehen dem Mauszeiger; gültige Ablageflächen werden
  hervorgehoben. Außerhalb der Ziele loslassen oder Escape drücken bricht ab.
- Hinzufügen zu einer Liste startet keine Wiedergabe. Die Sidelist führt denselben
  Dateipfad einmal; die Playlist erlaubt bewusst mehrfache Einträge.
- Dateien aus dem Windows-Explorer können ebenfalls auf Decks und Listen abgelegt
  werden. Beim Ablegen mehrerer Dateien auf ein Deck werden maximal zwei auf
  alle vier Decks verteilt. Ein abgelegter Ordner öffnet sich weiter im Musikbrowser.

## BPM, Keys und Analyse-Datenbank

Die Dateiliste enthält **LÄNGE**, **BPM**, **KEY**, **ANALYSE**, Format und Größe.
Ein Klick auf Titel, Länge, BPM oder Key sortiert die Liste; ein weiterer Klick
kehrt die Reihenfolge um. Unbekannte Werte stehen beim Sortieren hinten.
Keys erscheinen als musikalische Tonart und Camelot-Code, zum Beispiel **Am / 8A**.
Auch nach einem Key oder Camelot-Code kann gesucht werden.

1. Einen Musikordner öffnen. Bereits gespeicherte Analysen erscheinen automatisch.
2. **Unanalysierte analysieren (N)** analysiert offene oder geänderte Musikdateien
   im aktuellen Ordner, ohne Unterordner einzubeziehen. Die Analyse läuft im
   Hintergrund und zeigt Fortschritt und aktuelle Datei an.
3. **Auswahl analysieren** verarbeitet nur die ausgewählte Datei.
   **Auswahl erneut analysieren** berechnet ein vorhandenes Ergebnis neu.
4. **Abbrechen** beendet die Warteschlange; abgeschlossene Ergebnisse bleiben
   gespeichert. Fehlerhafte Dateien werden markiert, die übrige Warteschlange
   läuft weiter. Details stehen im Tooltip der Statuszeile oder Fehleranzeige.

Status **Offen** bedeutet noch nicht analysiert, **Geändert** ein veraltetes
Ergebnis, **Analysiert** eine abgeschlossene Analyse. **—** bei BPM oder Key kann
auch bedeuten, dass die Analyse keinen hinreichend eindeutigen Wert gefunden hat.
**Neu laden** prüft den Ordner erneut, etwa nach externen Dateiänderungen.

Die Datenbank wird automatisch unter **`%APPDATA%\RustDJ\library.sqlite3`**
angelegt. Sie speichert BPM, Beatgrid, geschätzte Tonart, Dauer und Wellenformen.
Dateipfad, Dateigröße, Änderungszeit und Analyseversion bestimmen, ob ein Ergebnis
wiederverwendet werden kann. Beim Laden eines Decks wird derselbe Cache verwendet;
fehlende Ergebnisse werden dabei analysiert und gespeichert. Die Musikdateien
werden nicht verändert. SQLite ist eingebunden; eine separate Installation ist
nicht nötig. Bei nicht verfügbarer Datenbank bleibt das Laden von Musik möglich,
mit einer sichtbaren Warnung zur fehlenden Speicherung.

BPM und Tonart sind Schätzungen. Die BPM-Erkennung verwendet die ersten maximal
zwei Minuten. Für Keys werden Spektralpeaks zu Tonklassen zusammengefasst und
mit Dur-/Moll-Profilen verglichen; ausgewertet werden maximal etwa zwei Minuten,
bei bekannter längerer Dauer in vier Abschnitten über den Track verteilt.
Unklare Ergebnisse bleiben leer. Tempo-Wechsel, Tonart-Wechsel, starke Percussion
und ungewöhnliche Stimmungen können die Erkennung beeinflussen. Die Anzeige ist
kein Key-Lock; manuelle BPM-Korrekturen eines Decks gelten weiterhin nur für die Sitzung.

Technische Grundlagen: [RustFFT](https://docs.rs/rustfft/6.4.1/rustfft/),
[rusqlite](https://docs.rs/rusqlite/0.37.0/rusqlite/),
[Krumhansl-Schmuckler-Tonartprofile](https://extras.humdrum.org/man/keycor/) und
[Camelot-Zuordnung](https://mixedinkey.com/camelot-wheel/).

MP3 und FLAC werden nativ über den Rust-Decoder verarbeitet. Tests verwenden echte,
aus den eigenen Demo-Loops erzeugte MP3-/FLAC-Dateien und prüfen Dekodierung,
Dauer, Wellenform, BPM und Positionssprünge. WAV, OGG/Vorbis, M4A/AAC und AIFF
werden ebenfalls angeboten, abhängig vom enthaltenen Codec. Kein DRM.

## Playlists speichern und laden

Rechts unten liegen die Tabs **SESSION** und **PLAYLIST**. SESSION enthält die
in dieser Sitzung geladenen Tracks. PLAYLIST ist eine eigene, frei angeordnete
Titelliste.

- Links im Musikbrowser eine Datei auswählen und **+ Playlist** anklicken.
  Der Playlist-Tab öffnet sich automatisch.
- **+ Dateien …** fügt mehrere ausgewählte Musikdateien hinzu. Derselbe Titel
  darf mehrmals in einer Playlist stehen.
- Einen Titel in der Playlist markieren und mit **Hoch** / **Runter** verschieben.
  **Entfernen** entfernt den Verweis aus der Liste.
- **A** / **B** lädt den jeweiligen Titel auf ein Deck. Doppelklick lädt auf das
  aktive Deck. Die Playlist startet die Wiedergabe nicht automatisch.
- **Speichern** wählt beim ersten Mal eine M3U8-Datei und aktualisiert danach
  dieselbe Datei. **Speichern unter …** legt eine weitere Playlist-Datei an.
- **Laden …** öffnet eine gespeicherte M3U8- oder UTF-8-M3U-Datei.
  Reihenfolge, Titel und doppelte Einträge bleiben erhalten.
- **Neu** beginnt eine leere Playlist. Ein Stern **\*** zeigt ungespeicherte
  Änderungen an. Vor Neu, Laden oder Beenden kann gespeichert, verworfen oder
  abgebrochen werden; ein abgebrochener Speicherdialog verwirft nichts.

Die Liste zeigt vorhandene BPM, Keys und Dauer aus der Analyse-Datenbank.
**Werte aktualisieren** liest diese Werte und die Dateiverfügbarkeit erneut ein,
zum Beispiel nach einer Ordneranalyse. Beim Laden eines noch nicht analysierten
Titels auf ein Deck wird er wie bisher analysiert.

M3U8-Dateien enthalten UTF-8-Text mit Verweisen auf die Musikdateien. Die Musik
wird nicht in die Playlist kopiert. RustDJ speichert absolute Dateipfade;
importierte relative Pfade werden vom Ordner der Playlist aus aufgelöst.
Fehlende Dateien bleiben sichtbar und können nach Wiederherstellen des Pfads
erneut geprüft werden. Internet-Streams und HLS-Listen werden nicht unterstützt.
Ältere M3U-Dateien mit anderer Zeichenkodierung bitte als UTF-8/M3U8 exportieren.

Die Ausgabe verwendet Extended M3U mit EXTM3U-/EXTINF-Zeilen; siehe die
[M3U-Implementierung von VideoLAN](https://github.com/videolan/vlc/blob/master/modules/misc/playlist/m3u.c).

## Pro-Layout

- Oben stehen zwei detaillierte, zeitlich laufende Wellenformen mit gemeinsamem
  Abspielmarker und Beat-Markierungen. Durch Ziehen lässt sich die Position ändern.
- Tiefen unter 250 Hz erscheinen **lila**, Mitten **grün**, Höhen über 4 kHz **helllila**.
  Die Farben basieren auf den gemessenen Frequenzanteilen der Datei. Positive und
  negative Spitzen werden getrennt erfasst; kurze Transienten bleiben sichtbar.
- **+ / −** über der Wellenform zoomen zwischen 1 und 16 Sekunden; Standard sind
  4 Sekunden. In der Nahansicht erscheinen zusätzliche Viertelbeat-Linien.
  Auch die kleinen Track-Übersichten im Deck verwenden die Frequenzfarben.
- Darunter liegen kompaktere Decks A und B mit Track-Übersicht, Laufzeit, BPM,
  Tempo, Transport und den acht Hotcue-/Loop-Buttons.
- Im zentralen Tab **MIXER** liegen für alle vier Decks **GAIN**, **HÖHEN**, **MITTEN**,
  **TIEFEN**, Kanal-Fader und Kopfhörer-CUE. Dazwischen stehen große, skalierte
  Kanalpegel A/B mit Zahlenwerten in dBFS. Diese messen nach Gain/EQ vor dem Fader.
  Der Crossfader bleibt darunter erreichbar.
- **MASTER**, **HEADPHONES** und **CUE / MASTER** bleiben in beiden Mixer-Tabs
  als Lautstärke-/Vorhörregler sichtbar. Im Tab **MASTER / PHONES** stehen zusätzlich
  die Kopfhörer-Geräteauswahl und getrennte Stereo-Pegel L/R für beide Ausgänge.
  Gemessen wird die tatsächliche
  Summe nach den Lautstärkereglern auf dem gewählten Kanalpaar. Die Zahlenwerte
  für Master und Phones bleiben in beiden Mixer-Tabs sichtbar.
- Ohne Signal erscheint **−∞ dBFS**. Rot beziehungsweise **CLIP** kennzeichnet
  digitale Ausgangsspitzen ab 0 dBFS. Hardware-Lautstärkeregler nach dem
  Softwareausgang beeinflussen diese Anzeige nicht.
- Unten stehen gleichzeitig Ordnernavigation, Dateiliste und die **SESSION / SIDELIST**
  bereit. Die Breite der Sidelist lässt sich am Trenner ändern.

Die Sidelist enthält die in dieser Sitzung geladenen Tracks. Es werden keine
funktionslosen Effekt-, Video- oder Pad-Steuerungen als Platzhalter angezeigt.

## Vier Decks und Master-Equalizer

Die Decks und Mixerkanäle stehen von links nach rechts in der Reihenfolge
**1, 3, 4, 2**. Der zentrale Mixer sitzt zwischen Deck 3 und 4. Alle vier Decks
haben eigenes Laden, Play/Pause, Transport-Cue, Tempo/Sync, acht Hotcues,
acht Loop-Längen, Filter und drei Effekte. Dateien können auf jedes Deck
gezogen oder über die nummerierten Lade-Buttons in Browser und Listen geladen werden.
Die vier laufenden Wellenformen verwenden ebenfalls die Reihenfolge 1, 3, 4, 2.

Die Zifferntasten **1–4** wählen ein Deck aus. Sync kann mehrere Decks an
eine Referenz binden; als Referenz wird ein anderes geladenes, noch nicht
synchronisiertes Deck gewählt, bevorzugt ein laufendes mit höherem Master-Anteil.
Manuelles Tempo, Joggen, Hotcue- oder Loop-Sprünge lösen dessen Sync-Bindung.
Jedes Deck hat einen eigenen Kopfhörer-Cue-Schalter, Gain, Kanal-Fader und
Drei-Band-EQ im Mixer. **Höhen, Mitten und Tiefen stehen pro Kanal senkrecht
untereinander**, Gain und Kanal-Fader daneben. Der Crossfader mischt links
**1/3**, rechts **4/2**.
Bei deaktiviertem Crossfader stehen alle vier Kanal-Fader unabhängig zur Verfügung.

**EQ** im zentralen Mixer öffnet den zusätzlichen **10-Band-Master-Equalizer**:
31, 63, 125, 250, 500 Hz sowie 1, 2, 4, 8 und 16 kHz, jeweils −12 bis +12 dB.
Zusätzlich gibt es Vorverstärkung, Ein/Aus und Neutral/Reset. Der EQ wirkt
nach dem Zusammenmischen auf Lautsprecher-Master, SHOUTcast und MP3-Aufnahme.
Die Kanal-EQs bleiben unabhängig davon für jedes Deck verfügbar. Das direkte
Kopfhörer-Vorhören wird vom Master-EQ nicht bearbeitet.
Ein/Aus, Vorverstärkung und alle zehn Werte werden automatisch gespeichert
und beim Programmstart sowie nach einem Wechsel des Audiogeräts wiederhergestellt.

Am DJ-202 schalten **DECK 3** und **DECK 4** die linke Seite zwischen 1/3 und
die rechte Seite zwischen 2/4 um. Transport, Pads, Load, Gain, EQ, Filter, FX
und Kanal-Fader bedienen die jeweilige Ebene. LED-Rückmeldungen folgen der
Auswahl. Ein gehaltenes Transport-Cue wird beim Loslassen am ursprünglichen
Deck freigegeben, auch wenn zwischendurch die Ebene gewechselt wurde.
Seit 0.18 werden die nativen MIDI-Kanäle für Deck 1, 2, 3 und 4 direkt zugeordnet;
ein Rückwechsel auf 1/2 wird nicht mehr durch die vorherige Software-Ebene nach
3/4 umgeleitet. Die gemeinsame FX-Sektion und die beiden Load-Tasten folgen
weiter der gewählten Seite. Native Pad-Modusmeldungen gehören zum gemeldeten
Deck. Eigene feste Deck-Belegungen werden nicht durch die Ebenenauswahl verändert.
Ältere MIDI-Profile werden ergänzt; eigene Belegungen bleiben berücksichtigt.
Die Ebenen-MIDI-Nachrichten sind getestet; eine vollständige Prüfung der
physischen DJ-202-Tasten und Regler bleibt erforderlich.

## SHOUTcast-Streamer und MP3-Aufnahmen

Der Button **STREAMER** oben im zentralen Mixer öffnet die Einstellungen.
Stream und Aufnahme bekommen ausschließlich den Stereo-Master nach Kanal-Fadern,
Crossfader, Gain, EQ, Filtern, Effekten und dem Software-Master-Level. Phones-CUE
und andere Windows-Programme sind nicht enthalten. Ein stummgeschalteter
Software-Master ist auch im Stream und in der Aufnahme stumm.

### Stream einrichten

1. Hostname oder IP ohne `http://`, Port oder Pfad eingeben.
2. **Source-Port** laut Anbieter einstellen, häufig 8001. Den getrennten
   **Metadata-/Admin-Port** eintragen, häufig 8000. Neuere Server können beide
   Verbindungen auch auf demselben Port annehmen.
3. Bei SHOUTcast 1 normalerweise **Stream-ID 1**, bei DNAS 2 die zugewiesene ID.
4. Source-Passwort und Stationsdaten eingeben. Alle Streamdaten bleiben nach
   einem Neustart erhalten. Das Passwort wird für dein Windows-Benutzerkonto
   mit DPAPI verschlüsselt in `settings.json` gespeichert und beim Start geladen.
5. MP3-Bitrate wählen: 128, 192, 256 oder 320 kbit/s, Stereo mit 44,1 kHz.
6. **Stream starten**. Status, gesendete Datenmenge und Fehler werden angezeigt.
   **Stream stoppen** trennt die Source-Verbindung.

Die Verbindung verwendet das SHOUTcast-1-kompatible ICY-Source-Protokoll über
TCP, einschließlich der DNAS-2-Stream-ID-Erweiterung. TLS/HTTPS und native
Ultravox-2-Verbindungen sind nicht enthalten. Ein kompatibler Source-Eingang
muss beim Anbieter verfügbar sein. Es erfolgt keine automatische Anmeldung im
öffentlichen SHOUTcast-Verzeichnis (`icy-pub:0`). RustDJ verbindet sich erst
nach **Stream starten** und startet nach einem Programmneustart keine Sendung.

### Optionale Titel-Tags

**Titel-Tags an den Server senden** ist zunächst ausgeschaltet. Eingeschaltet
sendet RustDJ den geladenen Titelnamen des laufenden Decks mit dem stärkeren
Master-Anteil. Dieser Name basiert aktuell auf dem Musikdateinamen. Stattdessen
kann ein **manueller Titel** eingetragen werden. Bei keinem hörbaren laufenden
Deck wird im automatischen Modus der Stationsname verwendet.

Die Titel werden über `admin.cgi?sid=…&mode=updinfo` aktualisiert, bei Änderungen
höchstens alle zwei Sekunden. Umlaute und Sonderzeichen werden URL-kodiert.
Das Source-Passwort muss diese Titelupdates erlauben. Bei ausgeschalteten Tags
werden keine neuen Titelupdates gesendet. Metadata-Fehler beenden den
Audiostream nicht und werden separat angezeigt.

### MP3 aufnehmen

Den Aufnahmeordner auswählen und **MP3-Aufnahme starten** drücken. Ohne Änderung
liegt er unter `%USERPROFILE%\Music\RustDJ-Aufnahmen`. Jede Aufnahme erhält eine
neue Datei `RustDJ-Master-<Zeitstempel>-<Nummer>.mp3`; vorhandene Dateien werden
nicht überschrieben. **Aufnahme stoppen** schreibt die restlichen MP3-Frames
und schließt die Datei ab. Auch reguläres Beenden von RustDJ schließt eine
laufende Aufnahme ab.

Aufnahme und Streaming lassen sich unabhängig starten und stoppen. Die
MP3-Aufnahme funktioniert auch ohne Serverdaten oder laufenden Stream. Ein
Serverausfall unterbricht die Aufnahme nicht. Ein Schreibfehler der Aufnahme
beendet den Stream nicht. Die Encoder und Netzwerk-/Dateizugriffe laufen in
getrennten Hintergrundthreads. Begrenzte Audiopuffer verhindern, dass ein
langsamer Server oder Datenträger die Wiedergabe blockiert; ein Überlauf beendet
die betroffene Ausgabe mit einer Fehlermeldung.

Audioausgänge lassen sich wechseln, nachdem Stream und Aufnahme beendet sind.
Serverdaten, Ports, Stream-ID, Stationsdaten, Bitrate, Tag-Einstellungen,
Aufnahmeordner und das geschützte Passwort werden automatisch gespeichert.
Nach einem Wechsel des Windows-Kontos oder Rechners muss das Passwort unter
Umständen neu eingegeben werden.
Der MP3-Encoder ist eingebaut, eine FFmpeg-Installation wird nicht benötigt.

Protokollreferenzen: [libshout ICY-Source-Implementierung](https://raw.githubusercontent.com/xiph/Icecast-libshout/master/src/proto_icy.c),
[SHOUTcast DNAS 2.5.1](https://yp.shoutcast.com/v/2_5_1),
[LAME-Rust-Encoder](https://docs.rs/mp3lame-encoder/0.2.5/mp3lame_encoder/).
Die Passwortspeicherung verwendet [Windows DPAPI](https://learn.microsoft.com/en-us/windows/win32/api/dpapi/nf-dpapi-cryptprotectdata).

## Filter, Effekte und Crossfader

Unter dem Jog-Rad jedes Decks liegen **FILTER** und **FX LEVEL**.
FILTER steht in der Mitte auf **OFF**, links arbeitet ein Tiefpass (LP),
rechts ein Hochpass (HP). Doppelklick setzt die Mitte wieder her.
Oben im Deck schalten **FLG**, **REV** und **ECHO** Flanger, Reverb und Echo
einzeln ein oder aus. Mehrere Effekte können gleichzeitig laufen.
FX LEVEL regelt die Tiefe aller eingeschalteten Effekte dieses Decks.
Das Echo folgt einem halben Beat des Track-BPM-Werts; ohne Beatgrid gelten 120 BPM.
Filter- und Effektänderungen werden geglättet und gelten für Master und Vorhören.

Im Mixer schaltet **CROSSFADER ON/OFF** den Crossfader ein oder aus.
Bei OFF läuft jedes Deck mit seinem Kanal-Fader, ohne Abschwächung durch die
Crossfader-Position. Master-, Phones- und Cue-Regelung bleiben aktiv.
Beim Deaktivieren springt der Crossfader sofort in die Mitte. Bei OFF werden
MIDI-Faderbewegungen und die Pfeiltasten ignoriert; die Anzeige bleibt mittig.
Beim erneuten Aktivieren startet er aus der Mitte und reagiert wieder auf neue
Eingaben. Der Schaltzustand wird gespeichert; auch ein Start mit gespeichertem
OFF beginnt mittig.

Das DJ-202-Profil verbindet die drei FX-Tasten und den LEVEL-Regler jeder Seite
mit den jeweiligen Deck-Effekten; die Tasten erhalten LED-Rückmeldung.
CC 1A steuert den Deck-Filter. Alle Funktionen einschließlich Crossfader-Schalter
stehen auch in MIDI-Learn zur Verfügung. Profilversion 6 ergänzt ältere Profile
unter Berücksichtigung vorhandener eigener Zuordnungen.
Die LEVEL-Funktion folgt dem
[Roland-Handbuch, Seite 12](https://static.roland.com/assets/media/pdf/DJ-202_eng03_W.pdf).

## Hotcues und Loops

**Hotcues 1–8:** Ein leeres Pad setzt einen Punkt an der aktuellen Position.
Ein belegtes Pad springt dorthin und startet die Wiedergabe. Shift+Klick oder
Rechtsklick löscht den Punkt. Die acht Punkte werden pro Musikdatei automatisch
in der vorhandenen SQLite-Datenbank gespeichert und beim erneuten Laden
wiederhergestellt. Bei geänderter Dateigröße oder Änderungszeit werden alte
Punkte nicht übernommen. Die Musikdatei bleibt unverändert.

**Loop-Buttons:** Pro Deck gibt es acht Tasten für **¼, ½, 1, 2, 4, 8, 16 und
32 Beats**. Ein Klick startet den jeweiligen Loop, ein weiterer Klick auf
dieselbe Taste beendet ihn und spielt weiter. Ein Klick auf eine andere Länge
passt den laufenden Loop an. Der aktive Button ist hervorgehoben.
Der Anfang wird am vorhergehenden Beat des Grids ausgerichtet. Die BPM müssen
erkannt oder manuell gesetzt sein. Loops über das Trackende werden abgelehnt.
Stop, Transport-CUE, ein Hotcue-Sprung oder ein Positionssprung außerhalb
des Bereichs verlassen den Loop ebenfalls.

Der Bereich wird im Hintergrund vorbereitet. Währenddessen steht **…** auf dem
gewählten Button. Nach der Vorbereitung läuft die Wiederholung mit PCM-Samples direkt
im Audio-Transport, einschließlich Master und Kopfhörer, EQ, Gain und Tempo.
Die Oberfläche muss dafür keinen periodischen Positionssprung auslösen.
Die Vorbereitung ist auf 64 MB Audiospeicher begrenzt.
Beim Trackwechsel werden Loop-Grenzen zurückgesetzt; aktive Loops werden nicht
dauerhaft gespeichert. Hotcues sind davon unabhängig.

### DJ-202-Pads

| Controller-Bedienung | RustDJ |
|---|---|
| HOT CUE, Pads 1–8 | Hotcue setzen beziehungsweise anspringen und spielen |
| SHIFT + Hotcue-Pad | Den jeweiligen Hotcue löschen |
| LOOP, Pads 1–8 | ¼, ½, 1, 2, 4, 8, 16 beziehungsweise 32 Beats; dasselbe Pad erneut drücken beendet den Loop |
| PARAM − / + im Loop-Modus | Loop halbieren / verdoppeln |

RustDJ schaltet beim Verbinden und nach einer USB-Neuverbindung die acht
DJ-202-Pad-Ebenen frei. Die dafür notwendige Initialisierung läuft auch bei
abgeschalteter LED-Rückmeldung. Einmal **HOT CUE** oder **LOOP** drücken, dann
das gewünschte Pad verwenden. Doppelklick und SHIFT+Modustaste wählen am Gerät
andere Modi; für die obige Belegung die Modustaste wieder einmal normal drücken.

Unter **Controller / MIDI** stehen der erkannte Pad-Modus für Deck 1/2/3/4 und
die letzte ausgelöste Controller-Aktion. Modusnachrichten werden auch mit dem
Wert 0 (HOT CUE) erkannt und bei MIDI-Learn nicht als Pad angelernt.
Wenn der MIDI-Ausgang nicht geöffnet oder die Pad-Ebenen nicht freigeschaltet
werden können, zeigt RustDJ dies dort als Fehler an.

Gesetzte Hotcues und Loop-Zustände werden an die Pad-LEDs zurückgemeldet.
Seit Profilversion 4 verwenden die bisherigen acht Standard-Loop-Pads
diese Längen; fehlende Pad-Zuordnungen werden ergänzt.
Bestehende benutzerdefinierte Zuordnungen werden berücksichtigt; alle neuen
Aktionen stehen auch im MIDI-Learn zur Verfügung.

Die Notenadressen beruhen auf dem
[DJ-202-Mapping des ursprünglichen Autors](https://gist.github.com/upCASE/c8dace3e70d565a4b1178f7013d3d738).
Die Pad-Freischaltung und Moduskennungen richten sich nach dessen
[Initialisierung und Pad-Steuerung](https://gist.github.com/upCASE/88fb258e85546092af18bc3ff924f752).
Die Pad-Modi beschreibt das
[Roland-Handbuch, Seite 11](https://static.roland.com/assets/media/pdf/DJ-202_eng03_W.pdf).
RustDJ implementiert die oben genannten Hotcue-/Loop-Funktionen; Sampler, Sequencer,
Pitch-Play und Roll-Modi gehören weiterhin nicht zum Funktionsumfang.

## Mixer, Gain, EQ und Kopfhörer

**GAIN** regelt die Vorverstärkung pro Deck von -24 bis +12 dB, mit 0 dB als
neutraler Einstellung. Gain wirkt vor dem EQ auf Master und Kopfhörer-Cue.
Die Änderungen werden geglättet, um abrupte Pegelsprünge zu vermeiden.

Im zentralen Mixer gibt es pro Deck **TIEFEN**, **MITTEN** und **HÖHEN** von -24 bis +12 dB.
Die Drehregler lassen sich ziehen; Doppelklick setzt 0 dB. Unter dem Regler ist
der Wert direkt editierbar, **Reset** setzt Gain und alle drei EQ-Bänder auf 0 dB zurück.
Der EQ verwendet geglättete Biquad-Filter: Low-Shelf 200 Hz, Mid-Peak 1 kHz,
High-Shelf 4 kHz. Er wirkt auf Master und Vorhörsignal. Die Filterfrequenzen
bleiben bei Tempoänderungen im hörbaren Ausgangsbereich konstant.

### Audioausgänge auswählen

1. **Audio / Ausgänge** öffnen und Geräte bei Bedarf aktualisieren.
2. Unter **MASTER OUTPUT** den Lautsprecher-/Mischpultausgang auswählen.
3. Unter **PHONES · AUSGABEQUELLE ZUM VORHÖREN** die Kopfhörer-Soundkarte wählen.
   Diese Auswahl ist immer sichtbar. Ein Gerät auszuwählen aktiviert den
   Kopfhörerausgang; **Aus – kein Kopfhörerausgang** deaktiviert ihn.
4. Bei zwei getrennten Geräten normalerweise jeweils **Kanäle 1/2** verwenden.
5. Bei einem gemeinsamen Mehrkanalgerät getrennte Paare wählen, zum Beispiel
   Master 1/2 und Phones 3/4. Das setzt voraus, dass der Windows-Treiber die
   entsprechenden Kanäle anbietet. Als getrennte Windows-Geräte angebotene
   Master-/Cue-Ausgänge können jeweils auf 1/2 verwendet werden.
6. **Übernehmen** anklicken. Der Wechsel erhält Position und Play/Pause-Zustand
   vorhandener Tracks. Die Auswahl wird gespeichert.

Die Phones-Ausgabe lässt sich auch direkt im Mixer-Tab **MASTER / PHONES**
auswählen: In der **PHONES**-Zeile Gerät und Kanalpaar einstellen und **Setzen**
drücken. Dabei wird ausschließlich die Kopfhörer-Ausgabe geändert; die aktive
Master-Ausgabe bleibt erhalten. Die Dropdown-Liste bietet auch **Geräte
aktualisieren …**. Das **…** neben **Setzen** öffnet die vollständigen
Audio-Einstellungen. Auch ein Klick auf die helllila **PHONES**-Anzeige in der
Kopfzeile öffnet die Geräteauswahl.

Master und Phones dürfen nicht auf demselben Gerät/Kanalpaar liegen. Solche
Konfigurationen werden abgelehnt. Bei fehlenden Geräten wird kein anderer
Ausgang als Ersatz geöffnet. Ein fehlgeschlagener Wechsel erhält die vorherige
Konfiguration. Es wird kein ASIO verwendet, sondern der normale Windows-Audiotreiber.

### Vorhören

- **CUE im Mixer-Kanalzug** schaltet dessen Kopfhörer-Vorhören ein.
  Das ist unabhängig vom Transport-CUE und startet den Track nicht automatisch.
- **MIXING CUE / MASTER** ganz links: nur ausgewählte Cue-Decks hören.
  Ganz rechts: den Program-Mix hören. In der Mitte beide Signale mischen.
- Cue liegt nach dem EQ, aber **vor Kanal-Fader und Crossfader**. Ein geschlossenes
  Deck lässt sich dadurch vorhören, ohne es auf den Master zu geben.
- **MASTER LEVEL** regelt nur die Lautsprecher-Ausgabe.
- **PHONES LEVEL** regelt nur den Kopfhörerausgang. Das Program-Signal im
  Kopfhörer wird vor Master-Level abgegriffen und bleibt dort separat regelbar.
- Zum Vorhören einen Track starten und den Kanal-Fader schließen beziehungsweise
  den Crossfader auf das andere Deck stellen. Alternativ Transport-CUE halten.

Beispiel: Deck 1 läuft auf dem Master. Deck 3 wählen, dessen Fader auf 0 stellen,
Kanal-CUE auf B im Mixer aktivieren, MIXING im Master-Tab nach Cue stellen,
PHONES LEVEL aufdrehen und Deck 3 starten.
Deck 3 ist im Kopfhörer hörbar; der Master bleibt bei Deck 1.

Unterschiedliche Soundkarten und insbesondere Bluetooth können verschiedene
Latenzen haben. Die Kopfhörer-Abspielposition wird bei größerer Clock-Abweichung
nachgeführt; die öffentliche Master-Wiedergabe wird dabei nicht versetzt.
Ein gemeinsames Mehrkanalgerät vermeidet getrennte Geräte-Clocks. Noch kein Limiter.

### Transport-CUE

**SET** speichert die aktuelle Trackposition. Der CUE-Knopf neben Play arbeitet so:

- Während Play drücken: zum Cue-Punkt springen und pausieren.
- Im Stand gedrückt halten: ab Cue vorhören.
- Loslassen: wieder zum Cue-Punkt zurück und pausieren.
- Während gehaltenem Cue Play drücken: Wiedergabe dauerhaft übernehmen.

MIDI verarbeitet dafür auch Note-Off beziehungsweise Note-On mit Velocity 0;
beide Audiowege werden gemeinsam gesteuert. Die Taste C bleibt ein direkter
Sprung zum Cue-Punkt mit Pause.

## BPM und Sync

Die Analyse schätzt ein konstantes Tempo zwischen 70 und 180 BPM und ein Beatgrid
anhand der Audioenergie. Sie wertet maximal die ersten zwei Minuten für BPM aus.
Sie erkennt keine musikalische Takt-Eins und kein variables Tempo. Bei komplexer
Musik oder Intros können Halb-/Doppeltempo und ungenaue Beatpositionen auftreten.

- Das BPM-Feld zeigt das Originaltempo. Per Ziehen oder Doppelklick korrigieren.
- **/2** und **x2** korrigieren Halb-/Doppeltempo.
- **Tap** mindestens dreimal im Beat anklicken, um das Tempo manuell zu setzen.
- **Beat hier** setzt die aktuelle Abspielposition auf das Beatgrid.
- Die Anzeige daneben zeigt die effektiven BPM einschließlich Tempoänderung.
- **SYNC** lässt dieses Deck dem jeweils anderen Deck folgen. Das Tempo wird
  dauerhaft gekoppelt; beim Einschalten und Starten des folgenden Decks wird
  seine Beatphase an die des anderen Decks angepasst. Das Deck startet dadurch
  nicht automatisch. Die Songstelle wird um höchstens etwa einen halben Beat verschoben.
- Erneutes **SYNC** löst die Kopplung. Laden eines Tracks beendet die Kopplung.
  Eine manuelle Tempoänderung am folgenden Deck oder Jog-Suche beendet sie ebenfalls.
- Sync arbeitet mit 50–200 % Geschwindigkeit. Es ist kein kontinuierlicher
  Phasenregler und kein Key-Lock: Tempoänderungen verändern die Tonhöhe.

Die Abspielposition wird direkt aus den dekodierten Quellsamples ermittelt und
bleibt deshalb auch bei wechselndem Tempo in der Zeitbasis der Originaldatei.

## Roland DJ-202 und andere MIDI-Controller

1. Controller per USB anschließen. Unter Windows gegebenenfalls den
   [offiziellen Roland-DJ-202-Treiber](https://www.roland.com/global/products/dj-202/downloads/)
   installieren.
2. RustDJ starten. Der DJ-202 wird automatisch gefunden und verbunden.
3. Auch ein später eingestecktes Gerät wird gesucht. Die Gerätesuche läuft im
   Hintergrund ungefähr alle zwei Sekunden; fehlgeschlagene Verbindungen werden
   frühestens nach drei Sekunden erneut versucht.
4. Im Musikbrowser eine Datei auswählen und am Controller **LOAD** drücken.

Unter **Controller / MIDI** lässt sich die Automatik deaktivieren. Ein manuell
verbundener Controller wird als bevorzugter Port gespeichert und beim nächsten
Start wieder verbunden. Ist dieser Port nicht vorhanden, wartet RustDJ darauf;
es wird kein beliebiges MIDI-Keyboard als Ersatz verbunden. **DJ-202 automatisch
wählen** hebt die Präferenz auf und sucht wieder gezielt nach dem DJ-202.
**Trennen** schaltet die Automatik aus, damit der Controller getrennt bleibt.
Beim USB-Abziehen bleibt die Automatik dagegen aktiv und verbindet nach dem
Wiedereinstecken erneut, sobald die Gerätesuche die Änderung erkennt.

Das mitgelieferte Basisprofil unterstützt die Hardware-Decks 1–4:

| Bedienelement | RustDJ-Funktion |
| --- | --- |
| Play/Pause | Wiedergabe umschalten |
| Transport-Cue | Rücksprung/Pause oder Halten zum Vorhören, Loslassen zum Zurückkehren |
| Sync | BPM-/Beat-Sync zum anderen Deck aktivieren/deaktivieren |
| Shift + Play | Stop und zurück zum Anfang |
| Shift + Cue | Aktuellen Cue-Punkt setzen |
| Tempo-Fader | Tempo ±16 %; beendet ggf. Sync auf diesem Deck |
| Kanal-Fader / Crossfader | Lautstärke / Überblendung, 14-Bit-Auflösung |
| HI / MID / LOW | Drei-Band-EQ, 14-Bit-Auflösung |
| TRIM | Gain / Vorverstärkung, 14-Bit-Auflösung (CC 16 / 34 hex) |
| Kanal-CUE im Mixer | PHONES CUE vor dem Fader ein-/ausschalten |
| MIXING | Cue-/Master-Anteil im Kopfhörer |
| PHONES LEVEL (wenn als MIDI gesendet) | Software-Kopfhörerpegel |
| Browse-Drehgeber | Auswahl im Musikbrowser bewegen |
| Browse drücken | Ausgewählten Ordner öffnen bzw. Datei ins aktive Deck laden |
| Load links/rechts | Ausgewählte Datei in Deck 1/2/3/4 laden |
| Jog drehen | Abspielposition fein verschieben, kein Scratch |
| Play-/Sync-/Mixer-Cue-LEDs | Zustandsrückmeldung, wenn der DJ-202-MIDI-Ausgang geöffnet werden kann |

Die MIDI-Adressen wurden anhand der vom Mapping-Autor veröffentlichten
[DJ-202-Belegung](https://github.com/Lykos153/mixxx/tree/Mapping-DJ-202/res/controllers)
und des [Mixxx-Geräteartikels](https://github.com/mixxxdj/mixxx/wiki/Roland-Dj-202)
recherchiert; die Rust-Implementierung ist eigenständig. Es werden keine Serato-Lizenzen benötigt.

**Hardwareprüfung ausstehend:** MIDI-Nachrichten und Fader-Auflösung sind durch
simulierte Nachrichten getestet. Das Zusammenspiel einschließlich LEDs muss
noch an einem echten angeschlossenen DJ-202 geprüft werden. Nicht enthalten:
Scratch, Sampler, Sequencer, Roll und Pitch-Play.

Für andere Geräte in **MIDI-LEARN** eine Funktion auswählen und **Anlernen**
aktivieren; danach Taste/Fader bewegen. Relative Encoder können mit „64 Mitte“
oder „1 / 127“ angelernt werden. Die zuletzt empfangenen MIDI-Bytes sind sichtbar.
Eigene Belegungen werden gespeichert; **DJ-202-Profil laden** ersetzt die Belegungen.

Die Geräteanbindung beschreibt **MASTER LEVEL und PHONES LEVEL am DJ-202 als
hardwareseitige Pegelregler**. Der physische Master-Regler ist deshalb keinem
erfundenen Software-MIDI-CC zugeordnet. Er regelt den RCA-Ausgang des DJ-202
weiterhin direkt; bei externen Lautsprechergeräten stattdessen den Software-
MASTER LEVEL verwenden. Für Controller, die einen Master-CC senden, steht die
Funktion „Master Level“ in MIDI-Learn bereit. Phones-CC 0C und Mixing-CC 0D
sind im recherchierten Basisprofil enthalten; eine Software-Rückmeldung ist
nur möglich, wenn das Gerät diese Werte tatsächlich sendet. Die Hardware-
Lautstärke und der Software-Pegel können gleichzeitig wirken.
Siehe [DJ-202-Geräteanbindung](https://virtualdj.com/manuals/hardware/roland/dj202/layout/mixer.html).

Alte MIDI-Profile werden um neue Mixer-Funktionen ergänzt, ohne vorhandene
benutzerdefinierte Zuweisungen zu überschreiben. Ein vollständig geleertes
Profil bleibt leer. **DJ-202-Profil laden** lädt die aktuelle Standardbelegung.

## Tastatur

| Taste | Funktion |
| --- | --- |
| 1 / 2 / 3 / 4 | Aktives Deck wählen (A/B bleiben Aliasse für 1/2) |
| Leertaste | Aktives Deck starten/pausieren |
| S | Sync des aktiven Decks umschalten |
| C | Zum Cue-Punkt springen und pausieren |
| Escape | Alle vier Decks stoppen und zurücksetzen |
| Pfeil links / rechts | Crossfader bewegen |

Ein Klick in ein Deck wählt es ebenfalls aus. Bei Texteingabe werden diese
Kürzel unterdrückt. Transport-Cue-Punkte, manuelle BPM-Korrekturen und Session-Bibliothek
bleiben nur während der aktuellen Sitzung erhalten.

## Einstellungen

Unter Windows liegen Browser-Ordner, Favoriten, MIDI-Belegungen, Auto-Connect und bevorzugter MIDI-Port, Audioausgänge
und Master-/Phones-/Mixing-Pegel in
`%APPDATA%\RustDJ\settings.json`. Der DJ-202 beziehungsweise der bevorzugte MIDI-Port wird standardmäßig automatisch geöffnet.

## Entwickeln

Unter Windows werden Rust mit dem MSVC-Target sowie die Visual-Studio-
C++-Build-Tools und das Windows SDK benötigt. `rust-toolchain.toml` legt die
Rust-Version fest. `dev.ps1` verwendet eine vorhandene lokale Toolchain in
`.tools` oder die regulär installierte Rust-Toolchain.

```powershell
.\dev.ps1 run
.\dev.ps1 test
.\dev.ps1 clippy
.\dev.ps1 build -Release
```

Mit regulärer Rust-Installation alternativ `cargo run` und `cargo build --release`.
`Cargo.lock` fixiert die Bibliotheken: eframe/egui, rodio/Symphonia, midir und rfd.
Die zwei gezielten Regressionstests für Version 0.18 laufen mit
`cargo test --lib regression18_`. Build-Dateien, lokale Einstellungen,
Datenbanken und Aufnahmen gehören nicht ins Repository.

Der optionale stumme Test am echten Audioausgang läuft außerhalb isolierter
Shell-Umgebungen mit `cargo test --test audio_device -- --ignored --nocapture`.

Die eigenen Demo-WAVs entstehen mit `python scripts/generate_demos.py`.
Mit FFmpeg lassen sich die verwendeten komprimierten Versionen neu erzeugen:

```powershell
ffmpeg -y -i assets/demo-a.wav -c:a libmp3lame -b:a 192k assets/demo-a.mp3
ffmpeg -y -i assets/demo-b.wav -c:a flac assets/demo-b.flac
```

## Aufbau und nächste Schritte

- `app.rs`: Oberfläche, Bedienung und Sync-Kopplung
- `audio.rs`: Transport, beide Wiedergabewege, Quellzeit und Pegelmessung
- `output.rs`: Geräteauswahl, isolierte Kanalpaare und Master-Abgriff
- `capture.rs`: begrenzte Stereo-Audiopuffer ohne blockierende Audio-Callback-Zugriffe
- `streamer.rs`: SHOUTcast-Source, optionale Titel-Tags und unabhängige MP3-Aufnahme
- `dsp.rs`: Drei-Band-EQ und Master-/Phones-Mischmatrix für vier Decks
- `equalizer.rs`: zusätzlicher 10-Band-Master-EQ
- `decks.rs`: Deck-Anordnung und Auswahl der Sync-Referenz
- `track.rs` / `beat.rs` / `key.rs`: Wellenform-, Tempo- und Tonartanalyse
- `waveform.rs`: hochauflösende Min-/Max-Spitzen, Frequenzfarben und Detailstufen
- `effects.rs`: Tief-/Hochpass, Flanger, Reverb und Echo pro Deck
- `analysis.rs` / `database.rs`: Analyse-Warteschlange und persistenter SQLite-Cache
- `browser.rs`: Musikbrowser mit asynchroner Ordnersuche und Analyse-Spalten
- `playlist.rs`: M3U8-Import/-Export, Playlist-Tab und Reihenfolge
- `loops.rs`: vorbereitete PCM-Loops, Quellzeit und Transport-Sprünge
- `performance.rs`: geordnetes Speichern der Hotcues im Hintergrund
- `dragdrop.rs`: ziehbare Musiktitel, Zielauswahl, Vorschau und einmalige Ablage
- `midi.rs`: MIDI-Verbindung, DJ-202-Profil, LED-Ausgabe und MIDI-Learn
- `settings.rs`: lokale Einstellungen
- `tests/`: Audioformate und optionaler Audio-Gerätetest

Noch offen: kontinuierliche Phasenregelung, variables Beatgrid, Key-Lock,
zusätzliche Effekte und ASIO.

## Validierung 0.18

Zwei gezielte Regressionstests: native MIDI-Bänke 1↔3 und 2↔4 mit Transport,
EQ/Gain/Fader, Filter, allen acht Hotcue-/Loop-Pads, gemeinsamem FX/Load,
Cue-Loslassen über einen Bankwechsel, Pad-Modi, eigenen Belegungen und LEDs;
außerdem simulierte Mausbedienung aller drei Ausgangsregler bei schmalen
Mixerbreiten, gespeicherte Werte und tatsächliche Samples von vier Decks auf
getrennten Master-/Kopfhörerwegen einschließlich Stummschaltung und Vorhören
bei geschlossenem Master. Die physische Prüfung am Controller steht aus.
Clippy mit Warnungen als Fehler und Release-Build erfolgreich.

### Vorherige Validierung 0.17

Zwei gezielte Tests erfolgreich:

- Vier-Deck-MIDI für Transport, Load, alle Hotcue-/Loop-Pads und Fader;
  Controller-Ebenenwechsel mit weitergeleiteten FX/Load-Bedienelementen,
  Cue-Loslassen nach Ebenenwechsel, LED-Adressen, Profil-Erweiterung,
  Reihenfolge/Fensterbreiten, Vier-Deck-Crossfader-/Kopfhörermischung und Sync-Referenz.
- Tatsächliche Filterwirkung aller zehn EQ-Bänder, Stereo-Kanaltrennung,
  trockener Bypass, gespeicherte EQ-Einstellungen, reale Summe von vier Quellen
  auf einem Mehrkanal-Master, Abgriff nach dem EQ und dekodierbare MP3-Aufnahme
  mit dem erwarteten bearbeiteten Pegel.

Clippy mit Warnungen als Fehler und Release-Build erfolgreich.
Die Kanal-EQs sind senkrecht angeordnet; die vier Deck-Farben sind auch in den
Lade-Buttons der Musiklisten sichtbar.

### Vorherige Validierung 0.16


Zwei gezielte Tests erfolgreich:

- Tatsächliches Master-Summensignal auf einem Mehrkanalpaar, Ausschluss von
  Phones/anderen Kanälen, MP3-Aufnahme und Dekodierung mit 44,1 kHz Stereo,
  Kanalpegel, Abschluss beim Stoppen/Beenden, eindeutige Dateinamen sowie
  begrenzte Audiopuffer und Überlauf-Erkennung.
- Lokaler SHOUTcast-Testserver mit Source-Anmeldung, DNAS-2-Stream-ID,
  MP3-Datenstrom und Dekodierung, Titel-Tags ein/aus, UTF-8-/Sonderzeichen,
  abgelehnte Anmeldung ohne Passwortausgabe und Weiterlaufen der Aufnahme;
  zusätzlich Speichern/Neuladen/Ersetzen aller Streamdaten und geschützter
  Passwörter mit tatsächlichem Windows-DPAPI-Roundtrip.

Es wurde kein externer Radioserver verbunden. Der Test ersetzt keine Prüfung
der individuellen Ports und Source-Konfiguration beim Anbieter.
Beide Tests, Clippy mit Warnungen als Fehler und der Release-Build sind erfolgreich.

### Vorherige Validierung 0.15

Zwei gezielte Tests prüfen das Zentrieren beim Ausschalten, das Ignorieren von
DJ-202-MIDI-Faderbewegungen und Tastatureingaben bei OFF, erneutes Einschalten,
den Neustart mit gespeichertem OFF sowie den unveränderten Audio-Bypass.
Beide Tests, Clippy mit Warnungen als Fehler und der Release-Build sind erfolgreich.

### Vorherige Validierung 0.14

Die Akzentfarben und Wellenform-Frequenzfarben wurden auf Lila/Helllila geändert.
Die dunkle Oberfläche verwendet dazu passende Auswahl- und Hover-Flächen.
Zwei bestehende Tests für Mixer-Geometrie und Drag-and-drop erfolgreich;
Clippy mit Warnungen als Fehler und Release-Build erfolgreich.

### Vorherige Validierung 0.13

Zwei gezielte Audiotests erfolgreich: getrennte Master-/Phones-Kanalpaare ohne
Übersprechen und unabhängig geregelte tatsächliche Audiosamples beider Ausgänge.
Clippy mit Warnungen als Fehler und Release-Build erfolgreich.
Phones können ohne vorheriges Aktivierungshäkchen ausgewählt werden; die neue
Direktauswahl verwendet dieselbe vorhandene Audio-Routing- und Speicherfunktion.
Die Breiten der Stereo-Pegelanzeigen und der Plattenteller-/Filterspalte sind
begrenzt, damit der Master-/Phones-Tab und beide Decks im Fenster Platz finden.

### Vorherige Validierung 0.12

Zwei gezielte Tests erfolgreich: Freischaltung der acht Pad-Ebenen und
Statusabfrage, Fehlerbehandlung, getrennte Modus-Erkennung beider Decks,
MIDI-Learn ohne versehentliche Modusbelegung und Zurücksetzen beim Trennen;
außerdem alle acht Hotcue-/Lösch-/Loop-Pads auf beiden Decks durch die
MIDI-Eingangsverarbeitung, Loslassen ohne doppelte Aktion, Loop-Längen,
PARAM −/+, Pad-LED-Nachrichten und Erhalt eigener Zuordnungen.
Clippy mit Warnungen als Fehler und Release-Build erfolgreich.
Die Prüfung der physischen Pads am angeschlossenen DJ-202 steht weiterhin aus.

### Vorherige Validierung 0.11

Zwei gezielte Tests erfolgreich: Bandfarben synthetischer Stereo-Töne,
Min-/Max-Spitzen, höhere Zeitauflösung und Datenbank-Roundtrip bei erhaltenen
Hotcues/BPM-Daten; außerdem tatsächliche Filterwirkung, veränderte Effektsignale
ohne Kanalübersprechen, trockener Bypass, Crossfader-OFF und dessen Speicherung
sowie DJ-202-FX-/Filter-MIDI und Profil-Erweiterung.
Clippy mit Warnungen als Fehler und Release-Build wurden ausgeführt.

Farbige Detail-Wellenformen werden komprimiert zusätzlich in SQLite gespeichert.
Vorhandene BPM-, Key- und Hotcue-Daten bleiben erhalten. Beim ersten Laden einer
alten Analyse erstellt RustDJ die fehlenden Frequenzdaten einmalig neu.
Normale Tracks werden in ungefähr 1-ms-Fenstern erfasst; lange Aufnahmen
werden bei Bedarf unter Erhalt der Spitzen zusammengefasst, um den Speicher zu begrenzen.
Die Darstellung zeigt das analysierte Originalaudio, unabhängig von laufenden FX.

### Vorherige Validierung 0.10

Zwei gezielte Tests erfolgreich: echte Summenmessung mit getrennten L/R-Pegeln
auf einem ausgewählten Mehrkanal-Paar, Verbindungserhalt bei Stille und
nachträglich hinzugefügten Quellen; außerdem Mixer-/Deck-Breiten bei den
unterstützten Fenstergrößen, ON/OFF-Texte und dBFS-Werte.
Clippy mit Warnungen als Fehler und Release-Build wurden ebenfalls ausgeführt.

### Vorherige Validierung 0.9

Zwei gezielte Tests erfolgreich: die acht Beat-Längen einschließlich
Beat-Ausrichtung sowie die Pad-Zuordnung beider Decks mit Press/Release,
Profil-Migration, Erhalt benutzerdefinierter Zuordnungen und LED-Bank.
Clippy mit Warnungen als Fehler und Release-Build wurden ebenfalls ausgeführt.
Der bestehende Audio-Loop-Transport wurde für diese Änderung nicht umgebaut.

### Vorherige Validierung 0.8

45 automatisierte Tests einschließlich Frequenztests für alle drei EQ-Bänder,
Pre-Fader-Cue trotz geschlossenem Fader/Master, Kanaltrennung, Transport-Cue,
MIDI-Release/14-Bit-EQ, Gain, Auto-Connect-Auswahl und Profilmigration.
Zusätzlich geprüft: Tonarterkennung mit synthetisierten Dur-/Moll-Akkordfolgen,
alle 24 Tonartprofile, Wiederverwendung nach erneutem Öffnen der Datenbank,
Ungültigkeit bei geänderten Dateien/Analyseversionen, Abbruch, Browser-Metadaten
und Fortsetzung einer Warteschlange nach einer defekten MP3-Datei.
Clippy mit `-D warnings` und Release-Build sind erfolgreich.
Im nativen Windows-Fenster wurden die neuen Spalten, Fortschritt und Ergebnisse
geprüft; eine Ordneranalyse schloss 26 Musikdateien mit 0 Fehlern ab.
Playlist-Tests prüfen Speichern und erneutes Laden einschließlich Überschreiben,
Umlauten, Reihenfolge, doppelten und fehlenden Dateien, relativen Pfaden und
Fehlern beim Öffnen oder Speichern.
Egui-Mausereignistests prüfen die Ablage auf Deck A/B und beiden Listen, genau
einmalige Übernahme, Abbruch per Escape und ungültige Ziele. Ein weiterer Test
prüft Einfügen zwischen Playlist-Einträgen und Drag-Reihenfolge nach dem Speichern.
Grundlage ist die [egui-Drag-and-drop-API](https://docs.rs/egui/0.33.3/egui/struct.DragAndDrop.html).
Zusätzlich geprüft: samplegenaue Stereo-Loop-Wiederholung und Ausstieg,
zyklische Quellzeit, Loop-Vorbereitung aus MP3/FLAC, Beat-Grenzen, zwei echte
Rodio-Sinks mit Loop über das Dateiende hinaus und Gain, geordnetes Speichern
und Wiederherstellen/Löschen der Hotcues sowie Pad-MIDI, Profil-Erweiterung und
getrennte LED-Bänke beider Decks.

Ein stummer nativer Windows-Integrationstest mit zwei verschiedenen vorhandenen
Audioausgängen prüfte beide Audio-Uhren, Pause, Seek bei 125 % Geschwindigkeit,
Cue-Halten/Loslassen, Hotcue-Sprünge und Löschen, mehrere Loop-Wiederholungen auf
beiden Wegen, Verdoppeln, Loop-Ausstieg, manuelle IN-/OUT-Loops, Reloop, Stop
und Ablehnung überlappender Ausgänge. Dieser Test schreibt
seinen Bericht nach `work/audio-check.txt` und lässt beide Ausgangspegel bei 0:

```powershell
cargo build --release --example audio_check
# target/release/examples/audio_check.exe normal unter Windows starten
```

Die vollständige Prüfung sämtlicher physischer DJ-202-Bedienelemente steht aus.
Die EQ-Formeln basieren auf dem [W3C Audio EQ Cookbook](https://www.w3.org/TR/audio-eq-cookbook/).

Die neue Oberfläche wurde im nativen Windows-Fenster geprüft. Der angeschlossene
DJ-202 wurde beim Start ohne manuelles Verbinden erkannt; die aktuellen Gain-Werte
wurden bereits über MIDI übernommen. Die vollständige Prüfung aller physischen
Regler sowie eines USB-Abzieh-/Einsteckzyklus steht weiterhin aus.
