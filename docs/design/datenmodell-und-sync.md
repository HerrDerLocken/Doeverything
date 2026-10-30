# Datenmodell & Sync-Protokoll

Status: Entwurf v0.1 · 2026-10-30

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
  Später kann man z. B. einen Space "Familie" mit anderen Accounts teilen,
  ohne den privaten Space offenzulegen.
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

Beispiele:

```yaml
shopping@1:
  fields:
    Artikel:   text
    Menge:     decimal
    Einheit:   enum [Stück, g, kg, ml, l, Packung]
    Kategorie: enum [Obst & Gemüse, Kühlregal, Drogerie, Sonstiges]
    Erledigt:  bool
  views:
    - list: group_by=Kategorie, sort=[Erledigt asc, _order asc]

finance@1:            # zwei Collections im selben Space
  Konten:
    Name: text
    Startsaldo: money
  Buchungen:
    Betrag: money      # negativ = Ausgabe
    Datum: date
    Kategorie: enum [...]
    Konto: ref → Konten
    Notiz: text
  views:
    - table: sort=[Datum desc]
    - chart: sum(Betrag) group_by=Kategorie, period=month

fitness@1:
  Übungen:  { Name: text, Muskelgruppe: multienum [...] }
  Workouts: { Start: instant, Dauer: duration, Notiz: text }
  Sätze:    { Workout: ref, Übung: ref, Wiederholungen: int, Gewicht: decimal }
```

**Berechnete Werte** (Summen, Kontostände, Statistiken, Steuerrechner) werden
lokal aus den Daten berechnet und **nie synchronisiert**.

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

```sql
-- Rohes Log aller bekannten Changesets (eigene und fremde)
CREATE TABLE changesets (
  space_id   BLOB NOT NULL,
  device_id  BLOB NOT NULL,
  seq        INTEGER NOT NULL,
  payload    BLOB NOT NULL,              -- Klartext-CBOR (DB selbst ist verschlüsselt)
  PRIMARY KEY (space_id, device_id, seq)
);

-- Materialisierter Zustand
CREATE TABLE entities (
  id         BLOB PRIMARY KEY,
  space_id   BLOB NOT NULL,
  kind       INTEGER NOT NULL,
  parent_id  BLOB,
  deleted    INTEGER NOT NULL DEFAULT 0  -- Cache von _deleted
);
CREATE INDEX entities_parent ON entities(parent_id, deleted);

CREATE TABLE field_values (
  entity_id  BLOB NOT NULL,
  field_key  BLOB NOT NULL,              -- Systemfeld-Code oder Field-EntityId
  value      BLOB NOT NULL,              -- CBOR-kodiertes Value
  hlc        BLOB NOT NULL,              -- 26 Byte, memcmp-vergleichbar
  PRIMARY KEY (entity_id, field_key)
);

-- Was dieses Gerät pro Space schon hat
CREATE TABLE version_vector (
  space_id   BLOB NOT NULL,
  device_id  BLOB NOT NULL,
  max_seq    INTEGER NOT NULL,           -- höchste lückenlos vorhandene seq
  PRIMARY KEY (space_id, device_id)
);

-- Sync-Stand je Gegenstelle (Server oder LAN-Peer)
CREATE TABLE sync_state (
  peer_id    BLOB NOT NULL,
  space_id   BLOB NOT NULL,
  cursor     INTEGER,                    -- nur Server: server_seq
  PRIMARY KEY (peer_id, space_id)
);
```

Abfragen von Views (Filter, Gruppieren, Summen) laufen direkt auf
`field_values`. Für große Collections können später pro Collection
Index-Tabellen bzw. generierte Spalten ergänzt werden.

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

Der Server speichert pro Space ein Append-only-Log von Envelopes und vergibt
zusätzlich eine fortlaufende `server_seq`. Damit kann ein Client einfach
"alles seit Cursor N" abholen, statt jedes Mal Version Vectors zu vergleichen.

**Was der Server sieht:** Account-ID, Device-IDs + Public Keys, Space-IDs,
seq, Zeitpunkt, Größenstufe. **Was er nicht sieht:** Inhalte, Namen von
Listen/Feldern, Anzahl der Einträge, Feldtypen.

HTTP-API (v1):

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
| Datenminimierung / Privacy by Design (Art. 5, 25) | Server sieht keine Inhalte; Größen-Padding; kein Passwort am Server; Discovery-Tags rotieren |
| Auskunft & Übertragbarkeit (Art. 15, 20) | Export eines Space als JSON (Entities + Werte) und CSV pro Collection, lokal ohne Server |
| Löschung (Art. 17) | `DELETE /v1/account` löscht alle Envelopes, Snapshots, Blobs, Geräte. Lokal: Datenbank + Schlüssel löschen (Crypto-Shredding). Tombstones verschwinden per GC. |
| Besondere Kategorien (Art. 9, Fitness/Gesundheit) | Ende-zu-Ende-Verschlüsselung; optional eigener Space mit eigenem Schlüssel |
| Integrität & Vertraulichkeit (Art. 32) | AEAD + Signaturen, verschlüsselte lokale DB, Noise im LAN, TLS zum Server |

---

## 8. Offene Punkte

- **Teilen mit anderen Accounts** (Familien-Einkaufsliste): Space Key mit dem
  X25519-Key des Mitglieds wrappen, Mitgliederliste als Collection. Format ist
  vorbereitet (Space Keys, Signaturen, Epochs), Ablauf noch nicht spezifiziert.
- **OR-Set für MultiEnum**, falls LWW in der Praxis stört.
- **Undo/Verlauf**: das Changeset-Log erlaubt "Änderungsverlauf eines Eintrags";
  UI dafür offen.
- **Schutz gegen Rollback durch den Server** (er hält Changesets zurück): Geräte
  könnten ihre VVs signiert untereinander austauschen. Für v1 nicht vorgesehen.
- **Kodierung**: CBOR (`ciborium`) vs. Protobuf festlegen. Vorschlag: CBOR, weil
  schemafrei und für `Value` natürlicher.
