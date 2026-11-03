# Datenmodell & Sync-Protokoll

Status: Entwurf v0.4 · 2026-11-03

Dieses Dokument legt fest, wie Daten lokal strukturiert sind, wie Änderungen
zwischen Geräten ausgetauscht werden (Server und LAN) und wie sie verschlüsselt
sind. Ziel: ein einziges Modell, das To-Do-Listen, Einkaufslisten, Finanzen,
Fitness usw. abdeckt, offline funktioniert und konfliktfrei synchronisiert.

---

## 1. Grundprinzipien

1. **Local-first** – die lokale Datenbank ist die Wahrheit. Sync ist optional.
2. **Alles ist eine Entity** – auch Schemas, Felder und Ansichten. Dadurch
   synchronisiert sich die Struktur mit demselben Mechanismus wie die Daten.
3. **Änderungen sind Operationen** – jede Änderung ist eine kleine,
   idempotente, kommutative Operation. Reihenfolge des Empfangs ist egal.
4. **Last-Writer-Wins pro Feld** – Konflikte werden auf Feldebene per
   Hybrid Logical Clock (HLC) entschieden. Zwei Geräte, die verschiedene Felder
   desselben Eintrags ändern, verlieren nichts.
5. **Server ist blind** – er sieht nur verschlüsselte, signierte Pakete und
   minimale Routing-Metadaten.
6. **Ein Protokoll für Server und LAN** – der Server ist nur ein Peer, der
   speichert, aber nicht entschlüsseln kann.

### Warum eigenes LWW-Modell statt Automerge/Yjs komplett?

| | LWW-Feld-Map (eigen) | Automerge/Yjs für alles |
|---|---|---|
| Passt zu Tracker-Daten (strukturierte Felder) | sehr gut | ok |
| Direkt in SQLite abfragbar (Filter, Summen) | ja | nur nach Materialisierung |
| Implementierungs-/Testaufwand | klein, gut verständlich | Bibliothek, aber Blackbox |
| Gleichzeitiges Tippen im selben Text | letzter gewinnt | wird gemergt |

→ **Entscheidung:** LWW-Feld-Map als Basis. Für den seltenen Fall von
Fließtext, der gleichzeitig bearbeitet wird (z. B. lange Notizen), gibt es
später den Feldtyp `richtext`, dessen Wert ein eingebettetes `yrs`-Dokument ist.

---

## 2. Datenmodell

### 2.1 Hierarchie

```
Account
 └─ Space                (Einheit für Verschlüsselung & späteres Teilen)
     └─ Collection       ("Einkaufsliste", "Haushaltsbuch 2026", "Workouts")
         ├─ Field        (Schema: "Artikel", "Menge", "Erledigt")
         ├─ View         (Liste / Tabelle / Kalender / Diagramm + Filter/Sortierung)
         └─ Record       (ein Eintrag: "Milch, 2, Liter, nicht erledigt")
```

- Ein **Space** hat einen eigenen Schlüssel. Standard: ein Space "Privat".
  Ein Space "Familie" kann mit anderen Accounts geteilt werden, ohne den
  privaten Space offenzulegen (Abschnitt 8).
- Alle Ebenen sind **Entities** mit demselben Aufbau.

### 2.2 Entity

```rust
struct EntityId([u8; 16]);          // UUIDv7 (zeitlich sortierbar, global eindeutig)

enum Kind { Space, Collection, Field, View, Record }

struct Entity {
    id: EntityId,
    kind: Kind,                     // unveränderlich
    parent: Option<EntityId>,       // Collection→Space, Field/View/Record→Collection
    fields: Map<FieldKey, Versioned<Value>>,
}

struct Versioned<T> { value: T, hlc: Hlc }

enum FieldKey {
    System(SysField),               // reservierte Felder, siehe unten
    User(EntityId),                 // ID der Field-Entity → Umbenennen ändert keine Daten
}
```

**Systemfelder** (gelten für alle Kinds, wo sinnvoll):

| Key | Typ | Bedeutung |
|---|---|---|
| `_deleted` | bool | Tombstone. Entity ausgeblendet, solange `true`. |
| `_order` | text | Fraktionaler Index für manuelle Sortierung (`"a0"`, `"a0V"`, …) |
| `_name` | text | Name von Space / Collection / Field / View |
| `_template` | text | Modul-Vorlage + Version, z. B. `shopping@1` (nur Collection) |
| `_type` | text | Feldtyp (nur Field), z. B. `money` |
| `_config` | json | Typ-Optionen (Field) bzw. Ansichts-Konfiguration (View) |

### 2.3 Feldtypen (`Value`)

```rust
enum Value {
    Null,
    Bool(bool),
    Int(i64),
    Decimal { mantissa: i128, scale: u8 },       // 1.25 → (125, 2); nie f64
    Money   { minor: i64, currency: [u8; 3] },   // 12,34 € → (1234, "EUR")
    Text(String),
    Date    { y: i16, m: u8, d: u8 },            // Kalendertag ohne Zeitzone (Fälligkeit, Geburtstag)
    Instant { unix_ms: i64 },                    // Zeitpunkt in UTC (Workout-Start)
    Duration{ ms: i64 },
    Enum(EntityId),                              // Option-ID aus _config des Feldes
    MultiEnum(BTreeSet<EntityId>),
    Ref(EntityId),                               // Verweis auf Record (z. B. Buchung → Konto)
    Blob    { hash: [u8; 32], size: u64, mime: String }, // Anhang, separat gespeichert
    Json(serde_json::Value),                     // nur für _config
}
```

Regeln:
- **Geld nie als Fließkomma.**
- **Enum-Optionen** haben stabile IDs; Umbenennen einer Option ändert nur `_config`.
- **Typänderung eines Feldes ist nicht erlaubt.** Stattdessen: neues Feld
  anlegen und Werte per Migration (= normale Set-Operationen) übertragen.
- Passt ein Wert nicht zum Feldtyp (z. B. durch alte Geräte), wird er als
  "ungültig" angezeigt, **aber nie verworfen**.

### 2.4 Module (Vorlagen)

Ein Modul ist reine Konfiguration: ein Satz von Field- und View-Entities, der
beim Anlegen einer Collection erzeugt wird. Danach gehört das Schema der
Collection und kann frei angepasst werden.

Felder tragen in `_config` optional eine **Rolle** (`{"role": "title"}`). Darüber findet eine
generische Oberfläche Titel, Erledigt-Haken, Menge usw., ohne die Vorlage zu kennen.

Beispiele:

```yaml
shopping@1:            # umgesetzt in crates/core/src/templates.rs
  fields:
    Artikel:   text      role=title
    Menge:     text      role=quantity   # Freitext ("2 kg"); strukturiert später als shopping@2
    Erledigt:  bool      role=done
  views:
    - list: sort=[Erledigt asc, _order asc]

todo@1:                # umgesetzt
  fields:
    Aufgabe:   text      role=title
    Erledigt:  bool      role=done
    Fällig:    date      role=due
    Notiz:     text      role=notes

finance@1:             # umgesetzt: eine Collection, Konten folgen als finance@2
  fields:
    Beschreibung: text   role=title
    Betrag:       money  role=amount   _config.currency = EUR   # negativ = Ausgabe
    Datum:        date   role=date
    Kategorie:    enum   role=category  _config.options = [Lebensmittel, Haushalt, …]
    Notiz:        text   role=notes
  views:
    - ledger: Monate (neueste zuerst) mit Einnahmen, Ausgaben, Saldo, Ausgaben je Kategorie

fitness@1:
  Übungen:  { Name: text, Muskelgruppe: multienum [...] }
  Workouts: { Start: instant, Dauer: duration, Notiz: text }
  Sätze:    { Workout: ref, Übung: ref, Wiederholungen: int, Gewicht: decimal }
```

**Berechnete Werte** (Summen, Kontostände, Statistiken, Steuerrechner) werden
lokal aus den Daten berechnet und **nie synchronisiert** (`crates/core/src/ledger.rs`,
`money.rs`).

**Welche Ansicht** eine Collection bekommt, ergibt sich aus den Rollen ihrer Felder, nicht aus
der Vorlage: Betragsfeld → Buchungsansicht, sonst Checkliste. Eigene oder abgewandelte
Collections bekommen so automatisch die passende Ansicht.

**Optionen von Auswahlfeldern** liegen als Ganzes in `_config`. Legen zwei Geräte gleichzeitig
offline eine neue Option an, gewinnt eine (LWW). Später: Optionen als eigene Entities.

**Rechner-Module** (Steuerrechner) speichern höchstens ihre Eingaben als
Record; die Regeln sind versionierter Code pro Steuerjahr (`tax-de@2026`).

---

## 3. Operationen & Konfliktlösung

### 3.1 Hybrid Logical Clock

```rust
struct Hlc {
    wall_ms: u64,       // 48 Bit genutzt
    counter: u16,
    device:  DeviceId,  // 16 Byte, Tie-Breaker
}
// Vergleich lexikografisch (wall_ms, counter, device) → totale Ordnung.
// Kodiert als 26 Byte Big-Endian → in SQLite per memcmp vergleichbar.
```

- Beim lokalen Schreiben: `wall = max(jetzt, letzte_hlc.wall)`, Counter erhöhen
  falls gleich.
- Beim Empfang: lokale HLC auf `max(lokal, empfangen)` ziehen – **aber**
  empfangene HLCs, die mehr als 24 h vor der lokalen Uhr liegen, werden nicht
  übernommen und das Gerät wird in der UI markiert (verhindert, dass ein Gerät
  mit falscher Uhr dauerhaft jeden Konflikt gewinnt).

### 3.2 Operationen

```rust
enum Op {
    Create { id: EntityId, kind: Kind, parent: Option<EntityId>, hlc: Hlc },
    Set    { id: EntityId, key: FieldKey, value: Value, hlc: Hlc },
}
```

Nur zwei Operationen. Löschen ist `Set _deleted = true`, Wiederherstellen
`Set _deleted = false`.

**Anwenden** (idempotent, kommutativ):
- `Create`: Entity anlegen, falls nicht vorhanden; sonst ignorieren.
- `Set`: Wert übernehmen, wenn `op.hlc > gespeicherte hlc` (oder Feld leer).
  Trifft ein `Set` vor dem zugehörigen `Create` ein, wird es trotzdem
  gespeichert (Entity "pending"), bis das `Create` kommt.

### 3.3 Konfliktbeispiele

| Situation | Ergebnis |
|---|---|
| Handy hakt "Milch" ab, Laptop ändert Menge auf 2 (beide offline) | beides bleibt erhalten (verschiedene Felder) |
| Beide ändern Menge (2 vs. 3) | spätere HLC gewinnt |
| A löscht Eintrag, B ändert ihn gleichzeitig | bleibt gelöscht (Edit setzt `_deleted` nicht zurück), Änderung ist beim Wiederherstellen sichtbar |
| A benennt Feld "Menge" → "Anzahl" um, B trägt Werte ein | Werte hängen an Feld-ID, kein Konflikt |
| Beide fügen Tag zu MultiEnum hinzu | v1: letzter gewinnt (bekannte Einschränkung; später OR-Set) |
| Beide sortieren Liste per Drag & Drop | fraktionale Indizes, kein Konflikt; gleiche Indizes → nach Entity-ID |

### 3.4 Changeset

Operationen einer lokalen Transaktion werden zu einem **Changeset**
gebündelt. Changesets sind die Einheit für Speicherung, Verschlüsselung und Sync.

```rust
struct ChangesetId { device: DeviceId, seq: u64 }  // seq: pro Gerät & Space lückenlos ab 1

struct Changeset {
    id: ChangesetId,
    space: EntityId,
    ops: Vec<Op>,
}
```

---

## 4. Lokale Speicherung (SQLite, per SQLCipher verschlüsselt)

Implementiert in `crates/store` (Schema: `SCHEMA_V1` in `crates/store/src/lib.rs`).

| Tabelle | Inhalt |
|---|---|
| `meta` | Geräte-ID u. Ä. (Schlüssel/Wert) |
| `changesets` | rohes Log aller bekannten Changesets (eigene und fremde), Ops als CBOR, plus `max_hlc` damit die Uhr nach einem Neustart monoton bleibt |
| `entities` | ID, Space, `kind` (`NULL` = nur Sets, Create fehlt noch), Elternteil, `deleted` als Cache von `_deleted` |
| `field_values` | (Entity, Feldschlüssel) → CBOR-Wert + HLC (26 Byte, per `memcmp` vergleichbar) |
| `version_vector` | pro Space und Gerät die höchste lückenlos vorhandene `seq` |
| `sync_state` | Sync-Cursor je Gegenstelle (Server) |

Regeln:
- **Last-Writer-Wins direkt in SQL**: `INSERT … ON CONFLICT DO UPDATE … WHERE excluded.hlc > field_values.hlc`.
- **Ein Changeset = eine SQL-Transaktion** (Log, Version Vector, Ops gemeinsam oder gar nicht).
- **Migrationen** über `PRAGMA user_version`; eine Datenbank einer neueren App-Version wird nicht geöffnet.
- **Verschlüsselung**: Feature `sqlcipher`, 32-Byte-Rohschlüssel aus dem Schlüsselspeicher des Betriebssystems.
  Ablauf beim App-Start (`app/lib/database.dart`):
  1. Schlüssel vorhanden → Datenbank damit öffnen.
  2. Kein Schlüssel, Datei verschlüsselt → Fehlerbildschirm, **nichts** anlegen oder überschreiben.
  3. Kein Schlüssel, Datei fehlt oder ist Klartext → neuen Schlüssel erzeugen, speichern, gegenlesen,
     erst dann öffnen. Klartext älterer Versionen wird per `sqlcipher_export` in eine Nachbardatei
     kopiert, probeweise geöffnet und atomar über das Original geschoben.
  Geht der Schlüsselspeicher verloren, sind die lokalen Daten nicht mehr lesbar. Abhilfe kommt mit
  Sync (Daten liegen auf weiteren Geräten) und dem Recovery Key aus Abschnitt 5.1.
- Jede `Store`-Implementierung muss die Konformitätstests (`konformitaets_tests!`) bestehen.

Abfragen von Views (Filter, Gruppieren, Summen) laufen auf `field_values`. Für große
Collections können später pro Collection Index-Tabellen bzw. generierte Spalten ergänzt werden.

---

## 5. Kryptografie

### 5.1 Schlüssel

```
Account Root Key (ARK)          32 Byte zufällig, verlässt nie unverschlüsselt ein Gerät
 ├─ Space Key[space, epoch]     32 Byte zufällig, mit ARK verschlüsselt ("wrapped")
 ├─ Discovery Key               = HKDF(ARK, "lan-discovery")
 └─ Recovery-Wrapping           ARK verschlüsselt mit Recovery Key (24 Wörter, zum Ausdrucken)
                                optional zusätzlich: ARK verschlüsselt mit Argon2id(Passwort)

Device Keys (pro Gerät)
 ├─ Signing Key   Ed25519       signiert Changesets, authentifiziert beim Server
 └─ Exchange Key  X25519        Empfang des ARK beim Pairing, Noise im LAN
```

Gespeichert im OS-Schlüsselspeicher (Keychain / Android Keystore /
Secret Service / DPAPI).

### 5.2 Verschlüsselung eines Changesets

```
plaintext  = zstd(cbor(Changeset.ops))
padded     = auf nächste Größenstufe auffüllen (256 B, 512 B, 1 KiB, … ) → verbirgt Größe
header     = { proto: 1, space_id, device_id, seq, key_epoch }
ciphertext = XChaCha20-Poly1305(SpaceKey[space, epoch], nonce=random 24 B, aad=header)
signature  = Ed25519(device_signing_key, header || nonce || ciphertext)

Envelope   = header || nonce || ciphertext || signature
```

- **AAD = Header** → der Server kann Pakete nicht zwischen Spaces/Geräten/seq
  vertauschen, ohne dass die Entschlüsselung fehlschlägt.
- **Signatur** → nur registrierte Geräte können gültige Changesets erzeugen;
  wichtig, sobald Spaces mit anderen Personen geteilt werden.
- Empfänger prüft: Signatur gegen bekannte Geräteliste, dann Entschlüsselung.

### 5.3 Geräteliste

Die Liste der Geräte (Public Keys, Name, Status) ist selbst eine Collection im
Space "Account" und wird ganz normal synchronisiert. Gerät entfernen:
1. `Set device._deleted = true`
2. Neue `key_epoch` für jeden Space erzeugen, nur an verbleibende Geräte verteilen.
3. Neue Changesets nutzen die neue Epoch; Changesets eines widerrufenen Geräts
   mit `seq` nach dem Widerruf werden abgelehnt.

### 5.4 Pairing (neues Gerät hinzufügen)

```
Neues Gerät N                         Bestehendes Gerät E
-------------                          -------------------
erzeugt Device Keys
zeigt QR: { N.exchange_pub,
            N.signing_pub,
            pairing_secret (16 B),
            server_url? }
                                       scannt QR
                                       trägt N in Geräteliste ein
                                       sealed = X25519-SealedBox(N.exchange_pub,
                                                 ARK || account_id)
                                       mac   = HMAC(pairing_secret, sealed)
                                       sendet (sealed, mac) per
                                         LAN direkt  ODER
                                         Server-Mailbox (TTL 10 min)
prüft mac, entschlüsselt ARK
→ synchronisiert normal
```

Ohne Kamera (Desktop ↔ Desktop): 8-stelliger Code + SPAKE2 statt QR.

---

### 5.5 Umsetzungsstand (v0.3)

Umgesetzt in `crates/proto/src/crypto.rs`, `crates/server`, `crates/sync`. Abweichungen vom Entwurf:

- **Space-Schlüssel** werden per HKDF aus dem Account Root Key abgeleitet
  (`space_key(ARK, space, epoch)`), statt zufällig erzeugt und gewrappt. Einfacher; Folge: Ein
  entferntes Gerät kennt den ARK, echte Rotation braucht daher einen neuen ARK.
- **Geräteliste** liegt auf dem Server (`/v1/devices`), nicht als E2E-synchronisierte
  Collection. Seit v0.4 entscheidet aber nicht die Liste, sondern das Gerätezertifikat (8.2),
  ob einem Gerät vertraut wird. Ein vom Server eingetragenes Gerät ohne Zertifikat wird ignoriert.
- **Kopplung** per Text-Code (`DOE1-…`) statt QR, sonst wie 5.4: Code enthält nur öffentliche
  Schlüssel und ein Einmal-Geheimnis. Das bestehende Gerät registriert das neue und legt den ARK
  verschlüsselt (X25519 + HKDF mit Einmal-Geheimnis) ins Server-Postfach. QR folgt.
- **Kein Recovery Key** bisher. Verlorene Geräte = verlorene Daten, solange nicht ein weiteres
  Gerät gekoppelt ist.
- **Push nur eigener Changesets**: Ein Gerät kann fremde nicht in deren Namen signieren.
  Für LAN-Sync müssen Envelopes daher künftig unverändert gespeichert und weitergereicht werden.

## 6. Sync-Protokoll

### 6.1 Grundmechanik (für alle Gegenstellen gleich)

Jedes Gerät kennt pro Space seinen **Version Vector**:
`{ device_id → höchste lückenlos vorhandene seq }`.

Sync zwischen zwei Teilnehmern A und B:
1. Beide tauschen ihre Version Vectors aus.
2. Jeder schickt dem anderen alle Changesets, die dieser laut VV noch nicht hat,
   pro Gerät in `seq`-Reihenfolge.
3. Empfänger prüft, entschlüsselt, wendet Ops an, aktualisiert VV.

Weil Ops idempotent sind, schadet doppeltes Senden nicht. Weil sie kommutativ
sind, ist die Reihenfolge zwischen Geräten egal.

### 6.2 Nachrichten (CBOR, Transport-unabhängig)

```rust
enum Msg {
    Hello      { proto: u16, peer_id: DeviceId, spaces: Vec<EntityId> },
    Have       { space: EntityId, vv: Map<DeviceId, u64> },
    Changesets { space: EntityId, items: Vec<Envelope> },
    Ack        { space: EntityId, vv: Map<DeviceId, u64> },
    Snapshot   { space: EntityId, covers: Map<DeviceId, u64>, blob_hash: [u8; 32] },
    Error      { code: u16, msg: String },
}
```

### 6.3 Server

**Speicherung:** SQLite als Standard (ein Container, ein Volume, Backup = Datei kopieren).
Der Server macht keine komplexen Abfragen, er hängt nur verschlüsselte Pakete an und liest sie
der Reihe nach. Postgres kann später als Option hinter derselben Schnittstelle folgen, falls eine
große gehostete Instanz es braucht.

Der Server speichert pro Space ein Append-only-Log von Envelopes und vergibt
zusätzlich eine fortlaufende `server_seq`. Damit kann ein Client einfach
"alles seit Cursor N" abholen, statt jedes Mal Version Vectors zu vergleichen.

**Was der Server sieht:** Account-ID, Device-IDs + Public Keys, Space-IDs,
seq, Zeitpunkt, Größenstufe. **Was er nicht sieht:** Inhalte, Namen von
Listen/Feldern, Anzahl der Einträge, Feldtypen.

HTTP-API (v1):

Tatsächliche Endpunkte (v1): `POST /v1/account`, `POST /v1/auth/challenge`, `POST /v1/auth/verify`,
`GET|POST /v1/devices`, `GET /v1/spaces`, `GET /v1/spaces/{id}/vv`, `POST /v1/spaces/{id}/push`,
`GET /v1/spaces/{id}/pull?after=N`, `PUT|GET|DELETE /v1/pairing/{id}`. Bodies als CBOR. Noch nicht
umgesetzt: Snapshots, Blobs, WebSocket-Benachrichtigung, Account löschen.

Ursprünglicher Entwurf:

| Methode | Pfad | Zweck |
|---|---|---|
| POST | `/v1/auth/challenge` | Nonce für Gerät holen |
| POST | `/v1/auth/verify` | Nonce mit Device-Signing-Key signiert → Session-Token |
| POST | `/v1/account` | Account anlegen (erstes Gerät, Public Keys) |
| DELETE | `/v1/account` | Account samt allen Daten löschen |
| POST | `/v1/spaces/{id}/push` | Envelopes hochladen → `{ server_seq je Envelope }` |
| GET | `/v1/spaces/{id}/pull?after=N&limit=M` | Envelopes ab Cursor |
| GET/PUT | `/v1/spaces/{id}/snapshot` | verschlüsselter Snapshot |
| PUT/GET | `/v1/blobs/{hash}` | verschlüsselte Anhänge |
| POST/GET | `/v1/pairing/{code}` | einmalige Mailbox, TTL 10 min |
| WS | `/v1/events` | Push-Hinweis "Space X hat neue Daten" (ohne Inhalt) |

Es gibt **kein Passwort auf dem Server**. Authentifizierung läuft ausschließlich
über Geräteschlüssel.

Server-Prüfungen (ohne Entschlüsselung möglich): Signatur gültig, Gerät gehört
zum Account und ist nicht widerrufen, `seq` lückenlos, Duplikate werden
ignoriert (`(space, device, seq)` eindeutig).

### 6.4 LAN

- **Discovery**: mDNS-Service `_doeverything._tcp`. TXT-Record enthält
  **keine** Account-ID, sondern `tag = HMAC(DiscoveryKey, aktuelle Stunde)[..8]`.
  Nur eigene Geräte erkennen sich; Fremde im WLAN sehen nur zufällige Werte,
  die sich stündlich ändern.
- **Transport**: TCP + **Noise IK** mit den X25519-Device-Keys aus der
  Geräteliste. Unbekannte Schlüssel → Verbindung abgelehnt.
- **Protokoll**: identisch zu 6.1 (Hello → Have → Changesets → Ack). Kein
  Cursor, nur Version Vectors.
- Envelopes werden auch im LAN **unverändert verschlüsselt und signiert**
  übertragen. Ein Gerät kann so Changesets von Dritten weiterreichen, ohne dass
  sich am Format etwas ändert.

Plattformgrenzen: Web/PWA kein LAN-Sync. iOS nur im Vordergrund und mit
Berechtigung "Lokales Netzwerk".

### 6.5 Snapshots & Aufräumen

Ohne Aufräumen wächst das Log unbegrenzt.

- Ein Gerät erzeugt periodisch (z. B. alle 1000 Changesets) einen **Snapshot**:
  vollständiger materialisierter Zustand eines Space **inklusive HLC pro Feld
  und Tombstones**, verschlüsselt wie ein Changeset, plus `covers`-VV.
- Ein Snapshot ist selbst ein Zustand eines State-based CRDT: Er kann per
  LWW-Merge mit jedem lokalen Zustand zusammengeführt werden.
- Der Server darf Changesets löschen, die von einem Snapshot abgedeckt sind und
  älter als 30 Tage sind (konfigurierbar).
- Neues Gerät: Snapshot laden → Changesets ab `covers` nachziehen.
- **Tombstone-GC**: Entities, die seit über 180 Tagen gelöscht sind, werden aus
  Snapshots entfernt. Geräte, die länger als 180 Tage offline waren, laden beim
  nächsten Sync zuerst den Snapshot und pushen danach ihre eigenen Changesets
  (Ops auf unbekannte, bereits entfernte Entities werden verworfen).

---

## 7. DSGVO-Bezug

| Anforderung | Umsetzung im Modell |
|---|---|
| Datenminimierung / Privacy by Design (Art. 5, 25) | Server sieht keine Inhalte; Größen-Padding; kein Passwort am Server; Discovery-Tags rotieren; bei geteilten Spaces nur Account-IDs und Rollen, keine Namen (8.10) |
| Auskunft & Übertragbarkeit (Art. 15, 20) | Export eines Space als JSON (Entities + Werte) und CSV pro Collection, lokal ohne Server |
| Löschung (Art. 17) | `DELETE /v1/account` löscht alle Envelopes, Snapshots, Blobs, Geräte. Lokal: Datenbank + Schlüssel löschen (Crypto-Shredding). Tombstones verschwinden per GC. |
| Besondere Kategorien (Art. 9, Fitness/Gesundheit) | Ende-zu-Ende-Verschlüsselung; optional eigener Space mit eigenem Schlüssel |
| Integrität & Vertraulichkeit (Art. 32) | AEAD + Signaturen, verschlüsselte lokale DB, Noise im LAN, TLS zum Server |

---

## 8. Geteilte Spaces

Ziel: Ein Space (z. B. "Familie" mit Einkaufsliste und Haushaltsbuch) wird von mehreren Accounts
gemeinsam genutzt. Der Server bleibt blind für Inhalte, kann keine Mitglieder einschleusen und
keine Rechte fälschen. Private Spaces bleiben unverändert.

**Granularität:** Geteilt wird immer ein ganzer Space, nie eine einzelne Collection. Wer eine Liste
teilen will, legt sie in einem geteilten Space an (bzw. verschiebt sie dorthin, siehe 8.9).

### 8.1 Begriffe

| Begriff | Bedeutung |
|---|---|
| **Account-Identität** | Schlüsselpaar, mit dem ein Account gegenüber *anderen* Accounts auftritt |
| **Gerätezertifikat** | Bestätigung "Gerät X gehört zu Account Y", signiert mit der Account-Identität |
| **Mitgliederkette** | signiertes, verkettetes Log aller Mitgliedschaftsänderungen eines Space |
| **Schlüsselpaket** | Space-Schlüssel einer Epoch, verschlüsselt für genau ein Mitglied |
| **Sicherheitsnummer** | Kurzform des Identitätsschlüssels zum Vergleichen (12 Ziffern) |

### 8.2 Account-Identität & Gerätezertifikate

Bisher kennen sich nur die Geräte *eines* Accounts, und die Geräteliste kommt vom Server (5.5).
Für fremde Accounts reicht das nicht: Ein Mitglied muss Signaturen von Geräten anderer Accounts
prüfen können, ohne dem Server zu glauben.

```
Account Root Key (ARK)
 ├─ Identity Signing Key   Ed25519 = HKDF(ARK, "account identity sign v1")
 └─ Identity Exchange Key  X25519  = HKDF(ARK, "account identity exchange v1")
```

- Beide werden aus dem ARK abgeleitet. Jedes eigene Gerät kennt sie damit automatisch, ohne
  zusätzlichen Austausch beim Pairing.
- Jedes Gerät bekommt ein **Gerätezertifikat**:

```rust
struct DeviceCert {
    account: AccountId,
    device: DeviceId,
    signing_pub: [u8; 32],
    exchange_pub: [u8; 32],
    issued_at: u64,
    signature: [u8; 64],     // Identity Signing Key über alle Felder davor
}
```

- Ausgestellt beim Anlegen des Accounts bzw. beim Pairing vom bestehenden Gerät. Der Server
  speichert die Zertifikate und liefert sie aus (`GET /v1/spaces/{id}/devices`), kann aber keine
  gültigen erzeugen.
- **Prüfregel:** Ein Changeset in einem geteilten Space ist gültig, wenn sein Gerät ein Zertifikat
  hat, das von der Identität eines Mitglieds mit Schreibrecht signiert ist (siehe 8.4).
- Nebeneffekt: Auch für den eigenen Account ersetzt das die Server-Geräteliste als
  Vertrauensquelle (schließt die Lücke aus 5.5).

**Sicherheitsnummer** = erste 8 Byte von `SHA-256("doe identity" || identity_sign_pub ||
identity_exchange_pub)` als Zahl modulo 10¹², also 12 Dezimalziffern (≈ 40 Bit) in Dreiergruppen.
Wird in der Mitgliederliste angezeigt und kann bei Bedarf persönlich verglichen werden.

**Umsetzungsstand (v0.4):** Identität, Zertifikate und Sicherheitsnummer sind umgesetzt
(`crates/proto/src/crypto.rs`). Jedes Gerät stellt sein Zertifikat beim ersten Sync selbst aus und
legt es per `PUT /v1/devices/{id}/cert` ab; der Server prüft nur, dass Gerät, Account und
Geräteschlüssel zusammenpassen. Changesets werden nur von Geräten mit gültigem Zertifikat
übernommen. Gelistete Geräte ohne (gültiges) Zertifikat, etwa mit älterer App-Version, werden
**zurückgestellt**: Der Pull-Cursor bleibt vor ihrem ersten Changeset stehen, sodass es beim
nächsten Durchgang erneut geprüft wird. Die Sicherheitsnummer steht in der App unter Sync.

### 8.3 Space-Schlüssel geteilter Spaces

Private Spaces behalten die HKDF-Ableitung aus dem ARK (5.5). Für geteilte Spaces geht das nicht,
weil sonst jede Rotation einen neuen ARK aller Beteiligten erfordern würde.

→ **Entscheidung:** Geteilte Spaces haben **zufällige** Space-Schlüssel pro Epoch (wie im
ursprünglichen Entwurf 5.1). Der Envelope-Header enthält `key_epoch` bereits; das Format der
Changesets ändert sich nicht.

```rust
struct KeyPackage {
    space: EntityId,
    epoch: u32,
    recipient: AccountId,
    sealed: Vec<u8>,          // X25519-SealedBox(recipient.identity_exchange_pub, K[epoch])
    author_device: DeviceId,
    signature: [u8; 64],      // Gerätesignatur über alle Felder davor
}

struct KeyChain {             // einmal pro Epoch, für alle Mitglieder gleich
    space: EntityId,
    epoch: u32,
    nonce: [u8; 24],
    sealed_prev: Vec<u8>,     // XChaCha20-Poly1305(K[epoch], [K[0], …, K[epoch-1]])
}
```

- Wer `K[epoch]` hat, kann über die `KeyChain` alle älteren Schlüssel öffnen. Ein neues Mitglied
  braucht also nur **ein** Schlüsselpaket und sieht die gesamte Historie.
- → **Entscheidung:** Neue Mitglieder sehen die vollständige Historie. Für eine Einkaufsliste oder
  ein Haushaltsbuch ist das erwartet. "Nur ab Beitritt" ist als spätere Option denkbar (neue Epoch
  ohne `KeyChain` für das neue Mitglied), wird aber nicht vorbereitet.
- Space-Schlüssel liegen lokal in der SQLCipher-Datenbank (Tabelle `space_keys`), nicht im
  OS-Schlüsselspeicher.

### 8.4 Mitgliederkette

Die Idee aus der früheren Fassung ("Mitgliederliste als Collection") reicht nicht: Jeder mit dem
Space-Schlüssel kann jede Collection beschreiben, und LWW kennt keine Rechte. Ein einfaches Mitglied
könnte sich so selbst zum Admin machen.

| | Collection im Space (LWW) | Mitgliederkette (eigenes Log) |
|---|---|---|
| Rechte durchsetzbar | nein, jeder Schreiber kann alles setzen | ja, jeder Eintrag wird gegen den Stand davor geprüft |
| Reihenfolge eindeutig | nein (gleichzeitige Änderungen) | ja, verkettet über Hashes |
| Server kann Rechte prüfen | nein (verschlüsselt) | ja (Rollen sind Klartext-Metadaten) |
| Aufwand | gering | mittel |

→ **Entscheidung:** Mitgliedschaft ist eine eigene, hash-verkettete Kette pro Space.
Anzeigenamen und andere persönliche Angaben gehören **nicht** hinein, sondern verschlüsselt in eine
normale Collection "Mitglieder" im Space (`_template = members@1`, Felder: Account-ID, Name).

```rust
struct MemberKeys {
    account: AccountId,
    identity_sign_pub: [u8; 32],
    identity_exchange_pub: [u8; 32],
}

enum Role { Admin, Writer, Reader }

enum LogBody {
    Genesis      { owner: MemberKeys },                         // Ersteller wird Admin
    Add          { member: MemberKeys, role: Role, invite: [u8; 16] },
    SetRole      { account: AccountId, role: Role, cutoff: Map<DeviceId, u64> },
    Remove       { account: AccountId, cutoff: Map<DeviceId, u64> },
    Leave        { cutoff: Map<DeviceId, u64> },                // nur für sich selbst
    Rotate       { epoch: u32 },                                // neue Schlüssel-Epoch
    IdentityChange { new_keys: MemberKeys, sig_new: [u8; 64] }, // nach ARK-Wechsel, siehe 8.8
    DeleteSpace,
}

struct LogEntry {
    space: EntityId,
    index: u64,               // 0 = Genesis
    prev_hash: [u8; 32],      // SHA-256 des vorherigen Eintrags (bei Genesis: Nullen)
    author_device: DeviceId,
    created_at: u64,
    body: LogBody,
    signature: [u8; 64],      // Gerätesignatur; Gerät muss gültiges DeviceCert haben
}
```

**Wer darf was:**

| Eintrag | Erlaubt für |
|---|---|
| `Add`, `SetRole`, `Remove`, `DeleteSpace` | Admin |
| `Leave` | jedes Mitglied für sich selbst; nicht der letzte Admin |
| `Rotate` | Admin und Writer |
| `IdentityChange` | das Mitglied selbst, signiert mit alter **und** neuer Identität |

**Reihenfolge:** Der Server nimmt einen Eintrag nur an, wenn `index = Länge der Kette` und
`prev_hash` zum letzten Eintrag passt (Compare-and-Swap). Bei gleichzeitigen Änderungen zweier
Admins bekommt der zweite `409 Conflict`, lädt die Kette neu und versucht es erneut.
Clients prüfen die gesamte Kette selbst: Signaturen, Hashes, Rechte jedes Eintrags gegen den
Stand davor. Der Server kann Einträge zurückhalten, aber keine fälschen oder umordnen.

**`cutoff`:** Beim Entfernen bzw. Herabstufen auf Reader wird festgehalten, bis zu welcher `seq` die
Geräte des Betroffenen noch schreiben durften (Version Vector des entfernenden Geräts). Spätere
Changesets dieser Geräte werden von allen Clients verworfen. Was der Betroffene davor offline
geschrieben, aber noch nicht hochgeladen hat, geht verloren. Das ist für einen Rauswurf akzeptabel.

### 8.5 Einladen

```
Admin A                         Server                         Eingeladene B
-------                         ------                         -------------
erzeugt invite_id (16 B),
        invite_secret (16 B)
PUT /v1/invites/{invite_id}
  { space, expires_at }  ──────►  speichert (TTL 7 Tage)
schickt Link außerhalb der App
(Messenger, QR, persönlich):
  DOEINV1-{server, invite_id,
           invite_secret, space,
           A.sicherheitsnummer}
                                                               öffnet Link
                                                               req = { B.MemberKeys,
                                                                       Name verschlüsselt }
                                ◄──────  POST /v1/invites/{id}/request
                                         { req, mac = HMAC(invite_secret, req) }
GET /v1/invites/{id}  ◄─────────
prüft mac, zeigt:
"Lisa (123 456 789 012)
 möchte beitreten – Rolle?"
A bestätigt:
  Kette += Add{B, Rolle, invite_id}
  KeyPackage für B
  Name in "Mitglieder"-Collection
  POST …  ───────────────────────►  Mitglied eingetragen,
                                    Einladung verbraucht
                                ──────►  B holt Kette + KeyPackage,
                                         prüft Kette, entschlüsselt K[epoch],
                                         synchronisiert normal
```

- **Ein Link, ein Beitritt.** Danach ist die Einladung verbraucht.
- Der MAC mit `invite_secret` verhindert, dass der Server eine eigene Beitrittsanfrage unterschiebt.
  Er kennt das Geheimnis nicht.
- Leitet jemand den Link weiter, sieht A beim Bestätigen den falschen Namen bzw. eine unbekannte
  Sicherheitsnummer. Deshalb gibt es **keine** automatische Annahme.
- B prüft beim ersten Laden, dass die Sicherheitsnummer des Genesis- bzw. einladenden Admins zur
  Nummer im Link passt. So erkennt B einen vom Server untergeschobenen Space.
- B braucht einen Account auf **demselben** Server. Föderation zwischen Servern ist nicht
  vorgesehen.

### 8.6 Entfernen, Verlassen, Rotation

1. Admin hängt `Remove { account, cutoff }` an (bzw. das Mitglied selbst `Leave`).
2. Direkt danach erzeugt dasselbe Gerät eine neue Epoch: zufälliger `K[e+1]`, `Rotate { e+1 }`,
   `KeyChain` und je ein `KeyPackage` für jedes verbleibende Mitglied.
   Ist das abgebrochen (Gerät offline), holt es jedes Admin- oder Writer-Gerät nach, das einen
   `Remove`/`Leave` ohne folgendes `Rotate` sieht.
3. Ab dem `Rotate` lehnt der Server neue Envelopes mit älterer `key_epoch` ab.
   Clients verschlüsseln eigene, noch nicht hochgeladene Changesets vor dem Push mit der
   aktuellen Epoch neu (gleiche `seq`, neue Signatur).
4. Der entfernte Account bekommt für den Space `403`.

Was der Entfernte bis dahin gesehen hat, behält er lokal. Das lässt sich technisch nicht verhindern
und wird in der UI beim Entfernen so gesagt.

### 8.7 Rollen

| Rolle | Lesen | Schreiben | Mitglieder verwalten |
|---|---|---|---|
| Admin | ja | ja | ja |
| Writer | ja | ja | nein |
| Reader | ja | nein | nein |

`Reader` wird **doppelt** durchgesetzt: Der Server nimmt von Reader-Geräten keine Envelopes an, und
Clients verwerfen Changesets, deren Account laut Kette kein Schreibrecht hat. Ein Reader kann zwar
technisch verschlüsseln, seine Änderungen erreichen aber niemanden.

Strukturänderungen (Felder, Ansichten) sind normale Schreibvorgänge. Eine feinere Trennung
("darf Einträge, aber keine Felder ändern") ist nicht vorgesehen.

### 8.8 Geräte & Identitätswechsel

- **Neues eigenes Gerät:** bekommt per Pairing den ARK und damit die Identität. Es lädt die
  Kette und die Schlüsselpakete des Accounts und hat sofort Zugriff auf alle geteilten Spaces. Die
  anderen Mitglieder merken nur ein neues Gerätezertifikat.
- **Eigenes Gerät entfernen:** Das Gerät kennt den ARK und damit die Identität (5.5). Nötig ist ein
  neuer ARK, damit eine neue Identität, und pro geteiltem Space ein `IdentityChange` (signiert mit
  alter und neuer Identität) plus `Rotate`. Danach werden Zertifikate der alten Identität ignoriert.
  - Bekannte Lücke: Das entfernte Gerät könnte schneller sein und selbst ein `IdentityChange`
    anhängen. Die anderen Mitglieder sehen dann eine geänderte Sicherheitsnummer, und der
    Betroffene kann den Fall per Admin-`Remove` + neuer Einladung bereinigen. Für v1 akzeptiert.

### 8.9 Space anlegen und Daten verschieben

- **Neuer geteilter Space:** Space-Entity anlegen, `Genesis` mit eigener Identität, `K[0]` zufällig,
  `KeyPackage` für sich selbst.
- **Privaten Space teilen** (später): `Rotate` auf eine zufällige Epoch `e+1`, die `KeyChain`
  enthält die bisherigen HKDF-Schlüssel. Das gibt nur die Schlüssel dieses einen Space preis, nicht
  den ARK (HKDF ist nicht umkehrbar). Ab dann gilt der Space als geteilt.
- **Collection in anderen Space verschieben:** Entities gehören fest zu ihrem Space, weil der Space
  die Verschlüsselungseinheit ist. Verschieben ist daher Kopieren (neue IDs im Ziel-Space) und
  Löschen im Quell-Space. Verweise (`Ref`) in andere Spaces werden dabei zu `Null` und in der UI
  gemeldet.
- **Space löschen:** `DeleteSpace` durch einen Admin. Der Server löscht Envelopes, Kette und
  Schlüsselpakete. Clients blenden den Space aus und löschen ihn lokal nach Bestätigung.
  Lokale Kopien anderer Mitglieder kann niemand erzwingen löschen.

### 8.10 Server

Neue bzw. geänderte Tabellen:

```sql
CREATE TABLE device_certs  (device_id BLOB PRIMARY KEY, account_id BLOB NOT NULL, cert BLOB NOT NULL);
CREATE TABLE space_members (space_id BLOB, account_id BLOB, role TEXT NOT NULL,
                            PRIMARY KEY (space_id, account_id));
CREATE TABLE space_log     (space_id BLOB, idx INTEGER, entry BLOB NOT NULL, hash BLOB NOT NULL,
                            PRIMARY KEY (space_id, idx));
CREATE TABLE key_packages  (space_id BLOB, epoch INTEGER, account_id BLOB, package BLOB NOT NULL,
                            PRIMARY KEY (space_id, epoch, account_id));
CREATE TABLE key_chains    (space_id BLOB, epoch INTEGER, chain BLOB NOT NULL,
                            PRIMARY KEY (space_id, epoch));
CREATE TABLE invites       (id BLOB PRIMARY KEY, space_id BLOB NOT NULL, request BLOB,
                            expires_at INTEGER NOT NULL);
```

- `spaces.account_id` bleibt als "privat, gehört Account X". Geteilte Spaces haben dort `NULL`,
  der Zugriff ergibt sich aus `space_members`. `space_members` ist ein Cache, den der Server beim
  Anhängen an die Kette pflegt. Er prüft die Rechte dabei selbst nach denselben Regeln wie die
  Clients.
- Zugriffsprüfung: privat → wie bisher; geteilt → Account ist Mitglied (Lesen) bzw. Mitglied mit
  Admin/Writer und aktuelle `key_epoch` (Schreiben).

Neue Endpunkte (Bodies CBOR):

| Methode | Pfad | Zweck |
|---|---|---|
| POST | `/v1/spaces/{id}/log` | Eintrag anhängen (CAS über `index`), optional mit KeyPackages und KeyChain in derselben Transaktion |
| GET | `/v1/spaces/{id}/log?after=N` | Kette ab Index |
| GET | `/v1/spaces/{id}/keys` | eigene Schlüsselpakete + KeyChains |
| GET | `/v1/spaces/{id}/devices` | Gerätezertifikate aller Mitglieder |
| PUT | `/v1/devices/{id}/cert` | eigenes Gerätezertifikat hinterlegen |
| PUT/GET | `/v1/invites/{id}` | Einladung anlegen / Anfrage abholen (Admin) |
| POST | `/v1/invites/{id}/request` | Beitrittsanfrage (eingeladener Account) |

`GET /v1/spaces` liefert zusätzlich die geteilten Spaces mit eigener Rolle.

**Was der Server zusätzlich sieht:** welche Accounts in welchem Space Mitglied sind, ihre Rollen und
Identitäts-Public-Keys, Zeitpunkte von Beitritt und Rotation. **Nicht:** Namen der Mitglieder oder
des Space, Inhalte.

### 8.11 Client

- **Store:** neue Tabellen `space_keys (space, epoch, key)`, `space_log (space, idx, entry)`,
  `device_certs`. Ein Space ist "geteilt", wenn eine Kette existiert.
- **Sync-Reihenfolge pro geteiltem Space:** Kette nachladen und prüfen → Schlüssel nachladen →
  Zertifikate nachladen → Envelopes wie bisher. Ein Envelope, dessen Gerät oder Epoch noch unbekannt
  ist, wird zurückgestellt statt verworfen und beim nächsten Durchlauf erneut geprüft.
- **UI:**
  - Space-Umschalter in der Seitenleiste (Privat, Familie, …)
  - "Teilen" → Einladungslink + QR, Rolle vorwählen
  - Beitrittsanfrage bestätigen (Name, Sicherheitsnummer, Rolle)
  - Mitgliederliste mit Rollen, Entfernen, Verlassen, Sicherheitsnummer
  - Hinweis bei geänderter Sicherheitsnummer eines Mitglieds

### 8.12 Umsetzungsreihenfolge

1. **Identität & Zertifikate:** Ableitung, `DeviceCert` bei Account-Anlage und Pairing, Server
   speichert und liefert Zertifikate, Clients prüfen Signaturen gegen Zertifikate statt gegen die
   Server-Geräteliste.
2. **Kette & Schlüssel:** `LogEntry`/`KeyPackage`/`KeyChain` in `crates/proto`, Prüfung der Kette als
   reine Funktion (`fn validate(chain) -> Result<MemberState>`), ausführlich getestet.
3. **Server:** Tabellen, Endpunkte, Rechteprüfung, CAS.
4. **Einladung & Sync** geteilter Spaces, Space-Umschalter in der App.
5. **Entfernen, Verlassen, Rotation**, Neuverschlüsselung ausstehender Changesets.
6. **Reader-Rolle**, QR-Code, Hinweise bei geänderter Sicherheitsnummer.

Tests, die es mindestens geben muss:
- Writer hängt `SetRole` an → Server und Clients lehnen ab.
- Zwei Admins ändern gleichzeitig → einer bekommt `409`, nach Neuladen sind beide Änderungen drin.
- Server unterschiebt Beitrittsanfrage ohne gültigen MAC → wird nicht angezeigt.
- Entferntes Mitglied pusht danach mit alter Epoch → `403`. Ein manipulierter Server, der das
  Envelope trotzdem verteilt → Clients verwerfen es (`cutoff`).
- Neues Mitglied sieht nach Beitritt die vollständige Historie über mehrere Epochs.
- Offline-Writer hat Changesets mit alter Epoch und erfährt erst beim Sync von der Rotation →
  Neuverschlüsselung, nichts geht verloren.

---

## 9. Offene Punkte

- **Geteilte Spaces:** spezifiziert in Abschnitt 8, noch nicht umgesetzt. Offen:
  "Historie erst ab Beitritt", Föderation zwischen Servern, feinere Rechte als
  Admin/Writer/Reader.
- **OR-Set für MultiEnum**, falls LWW in der Praxis stört.
- **Undo/Verlauf**: das Changeset-Log erlaubt "Änderungsverlauf eines Eintrags";
  UI dafür offen.
- **Schutz gegen Rollback durch den Server** (er hält Changesets zurück): Geräte
  könnten ihre VVs signiert untereinander austauschen. Für v1 nicht vorgesehen.
- **Kodierung**: CBOR (`ciborium`) vs. Protobuf festlegen. Vorschlag: CBOR, weil
  schemafrei und für `Value` natürlicher.
