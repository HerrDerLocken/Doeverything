import 'dart:io';

import 'package:doeverything/main.dart';
import 'package:doeverything/src/rust/api/tracker.dart';
import 'package:doeverything/src/rust/frb_generated.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

/// Wartet zusätzlich auf Rust-Aufrufe: `pumpAndSettle` kennt nur Animationen,
/// nicht Futures, die auf einem anderen Thread fertig werden.
Future<void> settle(WidgetTester tester) async {
  for (var i = 0; i < 5; i++) {
    await tester.pumpAndSettle();
    await Future<void>.delayed(const Duration(milliseconds: 50));
  }
  await tester.pumpAndSettle();
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
    final tracker = await Tracker.open(dbPath: '${dir.path}/test.sqlite');
    await tester.pumpWidget(DoeverythingApp(tracker: tracker));
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
    final tracker = await Tracker.open(dbPath: '${dir.path}/finanzen.sqlite');
    await tester.pumpWidget(DoeverythingApp(tracker: tracker));
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
}
