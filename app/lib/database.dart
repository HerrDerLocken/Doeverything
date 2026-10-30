import 'dart:convert';

import 'package:flutter_secure_storage/flutter_secure_storage.dart';

import 'src/rust/api/tracker.dart';

/// Ablage für den Datenbankschlüssel (Base64).
abstract interface class KeyStore {
  Future<String?> read();
  Future<void> write(String value);
}

/// Schlüsselspeicher des Betriebssystems: Secret Service/KWallet (Linux), Keychain (macOS/iOS),
/// Keystore (Android), Credential Manager (Windows).
class SecureKeyStore implements KeyStore {
  const SecureKeyStore();

  static const _name = 'doeverything-db-key-v1';
  static const _storage = FlutterSecureStorage();

  @override
  Future<String?> read() => _storage.read(key: _name);

  @override
  Future<void> write(String value) => _storage.write(key: _name, value: value);
}

/// Die Datenbank ist verschlüsselt, aber im Schlüsselspeicher liegt kein Schlüssel.
class KeyMissingException implements Exception {
  const KeyMissingException(this.dbPath);

  final String dbPath;

  @override
  String toString() =>
      'Der Schlüssel für die Datenbank wurde im Schlüsselspeicher nicht gefunden.';
}

/// Öffnet die verschlüsselte Datenbank und legt beim ersten Start einen Schlüssel an.
///
/// Fehlt der Schlüssel, obwohl schon eine verschlüsselte Datenbank existiert, wird **nichts**
/// angelegt oder überschrieben, sondern [KeyMissingException] geworfen. Ein neuer Schlüssel
/// wird gespeichert und gegengelesen, bevor er die Datenbank berührt; so kann ein Absturz
/// nie eine Datenbank hinterlassen, deren Schlüssel verloren ist.
Future<Tracker> openTracker({
  required String dbPath,
  required KeyStore keys,
}) async {
  final stored = await keys.read();
  if (stored != null) {
    return Tracker.open(dbPath: dbPath, key: base64Decode(stored));
  }
  if (databaseState(dbPath: dbPath) == DatabaseState.encrypted) {
    throw KeyMissingException(dbPath);
  }
  final key = generateDbKey();
  final encoded = base64Encode(key);
  await keys.write(encoded);
  if (await keys.read() != encoded) {
    throw StateError(
      'Schlüssel konnte nicht im Schlüsselspeicher abgelegt werden.',
    );
  }
  return Tracker.open(dbPath: dbPath, key: key);
}
