import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';

import 'screens/common.dart' show errorText;
import 'src/rust/api/tracker.dart';

/// Synchronisiert im Hintergrund und meldet neue Daten an die Bildschirme.
///
/// - verbunden: alle 15 s und kurz nach jeder lokalen Änderung ([schedule])
/// - Kopplung läuft: alle 3 s nachsehen, ob ein anderes Gerät bestätigt hat
class SyncService extends ChangeNotifier {
  SyncService(this.tracker);

  final Tracker tracker;

  SyncState? state;
  bool syncing = false;
  String? lastError;
  DateTime? lastSync;

  /// Steigt, wenn ein Sync neue Daten geholt hat. Bildschirme laden dann neu.
  int dataVersion = 0;

  Timer? _timer;
  Timer? _debounce;

  static const _interval = Duration(seconds: 15);
  static const _pairingPoll = Duration(seconds: 3);

  bool get connected => state?.mode == SyncMode.connected;

  /// Vorschlag für den Gerätenamen.
  static String get defaultDeviceName {
    try {
      return Platform.localHostname;
    } catch (_) {
      return 'Gerät';
    }
  }

  Future<void> start() async {
    await _refresh();
    if (connected) unawaited(syncNow());
  }

  Future<void> _refresh() async {
    state = await tracker.syncStatus();
    _timer?.cancel();
    _timer = switch (state!.mode) {
      SyncMode.connected => Timer.periodic(_interval, (_) => syncNow()),
      SyncMode.pairing => Timer.periodic(_pairingPoll, (_) => _pollPairing()),
      SyncMode.off => null,
    };
    notifyListeners();
  }

  /// Nach einer lokalen Änderung: gleich synchronisieren, mehrere Änderungen zusammenfassen.
  void schedule() {
    if (!connected) return;
    _debounce?.cancel();
    _debounce = Timer(const Duration(milliseconds: 800), syncNow);
  }

  Future<void> syncNow() async {
    if (!connected || syncing) return;
    syncing = true;
    notifyListeners();
    try {
      final result = await tracker.syncNow();
      if (!result.skipped) {
        lastError = null;
        lastSync = DateTime.now();
        if (result.pulled > 0) dataVersion++;
      }
    } catch (e) {
      lastError = errorText(e);
    } finally {
      syncing = false;
      notifyListeners();
    }
  }

  Future<void> _pollPairing() async {
    try {
      if (await tracker.syncFinishPairing()) {
        lastError = null;
        await _refresh();
        await syncNow();
      }
    } catch (e) {
      lastError = errorText(e);
      notifyListeners();
    }
  }

  Future<void> createAccount(String server, String deviceName) async {
    await tracker.syncCreateAccount(server: server, deviceName: deviceName);
    await _refresh();
    await syncNow();
  }

  Future<void> startPairing(String server, String deviceName) async {
    await tracker.syncStartPairing(server: server, deviceName: deviceName);
    await _refresh();
  }

  /// Gibt den Namen des hinzugefügten Geräts zurück.
  Future<String> approve(String code) async {
    final name = await tracker.syncApprovePairing(code: code);
    unawaited(syncNow());
    return name;
  }

  Future<void> disconnect() async {
    await tracker.syncDisconnect();
    lastError = null;
    lastSync = null;
    await _refresh();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _debounce?.cancel();
    super.dispose();
  }
}
