import 'dart:io';

import 'package:doeverything/database.dart';
import 'package:doeverything/main.dart';
import 'package:doeverything/src/rust/api/tracker.dart';
import 'package:doeverything/src/rust/frb_generated.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

/// Schlüsselspeicher im Arbeitsspeicher, damit Tests nicht den echten des Systems verwenden.
class MemoryKeyStore implements KeyStore {
  String? value;
  int writes = 0;

  @override
  Future<String?> read() async => value;

  @override
  Future<void> write(String v) async {
    value = v;
    writes++;
  }
}

/// Wartet zusätzlich auf Rust-Aufrufe: `pumpAndSettle` kennt nur Animationen,
/// nicht Futures, die auf einem anderen Thread fertig werden.
Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 5; i++) {
    await tester.pumpAndSettle();
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
  await tester.pumpAndSettle();
}

/// Scrollt [finder] in den sichtbaren Bereich und tippt darauf.
Future<void> tapVisible(WidgetTester tester, Finder finder) async {
  await tester.ensureVisible(finder);
  await tester.pump();
  await tester.tap(finder);
}

/// Setzt den Inhalt eines Textfelds direkt. `enterText` kommt im Linux-Integrationstest beim
/// zweiten Mal auf demselben Feld nicht an (Test-Tastatur verbindet sich nicht neu); geprüft
/// werden soll hier die Reaktion der App auf die Eingabe, nicht das Tippen.
Future<void> setField(WidgetTester tester, Finder field, String text) async {
  tester.widget<TextField>(field).controller!.text = text;
  await tester.pump();
}

/// Pumpt, bis [finder] etwas findet (für Netzwerkaufrufe mit unbekannter Dauer).
Future<void> waitFor(WidgetTester tester, Finder finder) async {
  for (var i = 0; i < 100 && finder.evaluate().isEmpty; i++) {
    await Future<void>.delayed(const Duration(milliseconds: 100));
    await tester.pump();
  }
  if (finder.evaluate().isEmpty) {
    final texte = find
        .byType(Text)
        .evaluate()
        .map((e) => (e.widget as Text).data)
        .whereType<String>()
        .join(' | ');
    fail('Nicht gefunden: $finder\nAngezeigt: $texte');
  }
}

void main() {
  IntegrationTestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async => await RustLib.init());

  late Directory dir;
  setUp(
    () async => dir = await Directory.systemTemp.createTemp('doeverything'),
  );
  tearDown(() async => await dir.delete(recursive: true));

  testWidgets('Einkaufsliste anlegen, Artikel hinzufügen und abhaken', (
    tester,
  ) async {
    final tracker = await Tracker.open(
      dbPath: '${dir.path}/test.sqlite',
      key: generateDbKey(),
    );
    await tester.pumpWidget(await appWith(tracker));
    await settle(tester);
    expect(find.text('Noch keine Listen'), findsOneWidget);

    // Liste anlegen
    await tester.tap(find.text('Neue Liste'));
    await settle(tester);
    await tester.tap(find.text('Einkaufsliste'));
    await tester.enterText(find.byType(TextField), 'Wocheneinkauf');
    await tester.tap(find.text('Anlegen'));
    await settle(tester);
    expect(find.text('Wocheneinkauf'), findsOneWidget);

    // Artikel hinzufügen
    for (final artikel in ['Milch', 'Brot']) {
      await tester.enterText(find.byKey(const Key('neuer-eintrag')), artikel);
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await settle(tester);
    }
    expect(find.text('Milch'), findsOneWidget);
    expect(find.text('Brot'), findsOneWidget);

    // Milch abhaken → wandert in "Erledigt"
    await tester.tap(
      find.descendant(
        of: find.widgetWithText(ListTile, 'Milch'),
        matching: find.byType(Checkbox),
      ),
    );
    await settle(tester);
    expect(find.text('Erledigt (1)'), findsOneWidget);

    // Zurück zur Übersicht: Zähler stimmt
    await tester.tap(find.byType(BackButton));
    await settle(tester);
    expect(find.text('1 offen · 2 gesamt'), findsOneWidget);
  });

  testWidgets('Haushaltsbuch: Einnahme und Ausgabe buchen, Saldo prüfen', (
    tester,
  ) async {
    final tracker = await Tracker.open(
      dbPath: '${dir.path}/finanzen.sqlite',
      key: generateDbKey(),
    );
    await tester.pumpWidget(await appWith(tracker));
    await settle(tester);

    await tester.tap(find.text('Neue Liste'));
    await settle(tester);
    await tester.tap(find.text('Haushaltsbuch'));
    await tester.enterText(find.byType(TextField), 'Finanzen');
    await tester.tap(find.text('Anlegen'));
    await settle(tester);
    expect(find.text('Noch keine Buchungen'), findsOneWidget);

    Future<void> buchen(
      String betrag,
      String text, {
      bool einnahme = false,
    }) async {
      await tester.tap(find.text('Buchung'));
      await settle(tester);
      if (einnahme) await tester.tap(find.text('Einnahme'));
      await tester.enterText(find.byKey(const Key('betrag')), betrag);
      await tester.enterText(find.byKey(const Key('beschreibung')), text);
      await tester.tap(find.text('Speichern'));
      await settle(tester);
    }

    // Ungültiger Betrag wird abgelehnt, Dialog bleibt offen
    await tester.tap(find.text('Buchung'));
    await settle(tester);
    await tester.enterText(find.byKey(const Key('betrag')), 'zwölf');
    await tester.tap(find.text('Speichern'));
    await settle(tester);
    expect(find.textContaining('Bitte einen Betrag'), findsOneWidget);
    await tester.tap(find.text('Abbrechen'));
    await settle(tester);

    await buchen('1.000', 'Gehalt', einnahme: true);
    await buchen('12,34', 'Wocheneinkauf');
    await buchen('5', 'Kaffee');

    expect(find.text('Wocheneinkauf'), findsOneWidget);
    // Saldo: 1000,00 - 12,34 - 5,00
    expect(find.textContaining('982,66'), findsWidgets);
    expect(find.textContaining('-17,34'), findsOneWidget);

    // Kaffee wegwischen → Saldo steigt
    await tester.drag(find.text('Kaffee'), const Offset(-600, 0));
    await settle(tester);
    expect(find.text('Kaffee'), findsNothing);
    expect(find.textContaining('987,66'), findsWidgets);

    await tester.tap(find.byType(BackButton));
    await settle(tester);
    expect(find.textContaining('Diesen Monat: 987,66'), findsOneWidget);
  });

  testWidgets(
    'Schlüssel wird beim ersten Start angelegt und danach verwendet',
    (tester) async {
      final pfad = '${dir.path}/schluessel.sqlite';
      final keys = MemoryKeyStore();

      final erster = await openTracker(dbPath: pfad, keys: keys);
      expect(keys.writes, 1);
      expect(databaseState(dbPath: pfad), DatabaseState.encrypted);
      await erster.createCollection(name: 'Geheim', templateId: 'todo@1');
      erster.dispose();

      final zweiter = await openTracker(dbPath: pfad, keys: keys);
      expect(
        keys.writes,
        1,
        reason: 'vorhandener Schlüssel wird wiederverwendet',
      );
      final listen = await zweiter.collections(today: '2026-10-30');
      expect(listen.single.name, 'Geheim');
      zweiter.dispose();

      // Schlüssel weg (z. B. Schlüsselbund zurückgesetzt): nichts überschreiben
      final ohne = MemoryKeyStore();
      await expectLater(
        openTracker(dbPath: pfad, keys: ohne),
        throwsA(isA<KeyMissingException>()),
      );
      expect(
        ohne.writes,
        0,
        reason: 'kein neuer Schlüssel für vorhandene Daten',
      );

      // Falscher Schlüssel: Fehler statt Absturz oder Datenverlust
      final falsch = MemoryKeyStore()
        ..value = 'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=';
      await expectLater(
        openTracker(dbPath: pfad, keys: falsch),
        throwsA(anything),
      );
      final nochmal = await openTracker(dbPath: pfad, keys: keys);
      expect(
        (await nochmal.collections(today: '2026-10-30')).single.name,
        'Geheim',
      );
    },
  );

  testWidgets(
    'Sync-Bildschirm: Einrichtung und Fehler bei ungültiger Adresse',
    (tester) async {
      final tracker = await Tracker.open(
        dbPath: '${dir.path}/sync.sqlite',
        key: generateDbKey(),
      );
      await tester.pumpWidget(await appWith(tracker));
      await settle(tester);

      await tester.tap(find.byTooltip('Sync einrichten'));
      await settle(tester);
      expect(find.text('Neues Sync-Konto anlegen'), findsOneWidget);
      expect(find.text('Ende-zu-Ende-verschlüsselt'), findsOneWidget);

      await setField(tester, find.byKey(const Key('sync-server')), 'ftp://x');
      await tapVisible(tester, find.text('Neues Sync-Konto anlegen'));
      await waitFor(tester, find.textContaining('http:// oder https://'));

      // Nicht erreichbarer Server: Fehlermeldung, Zustand bleibt "aus"
      await setField(
        tester,
        find.byKey(const Key('sync-server')),
        'http://127.0.0.1:9',
      );
      await tapVisible(tester, find.text('Neues Sync-Konto anlegen'));
      await waitFor(tester, find.textContaining('nicht erreichbar'));
      expect(find.text('Neues Sync-Konto anlegen'), findsOneWidget);

      // Kopplung starten zeigt einen Code (ohne Server-Kontakt)
      await setField(
        tester,
        find.byKey(const Key('sync-server')),
        'https://sync.example.org',
      );
      await tapVisible(tester, find.text('Mit bestehendem Gerät koppeln'));
      await waitFor(tester, find.byKey(const Key('pairing-code')));
      final code = tester.widget<SelectableText>(
        find.byKey(const Key('pairing-code')),
      );
      expect(code.data, startsWith('DOE1-'));
      await tapVisible(tester, find.text('Abbrechen'));
      await settle(tester);
      expect(find.text('Neues Sync-Konto anlegen'), findsOneWidget);
    },
  );
}
