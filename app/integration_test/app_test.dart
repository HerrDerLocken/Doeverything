import 'dart:io';

import 'package:doeverything/main.dart';
import 'package:doeverything/src/rust/api/tracker.dart';
import 'package:doeverything/src/rust/frb_generated.dart';
import 'package:flutter/material.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:integration_test/integration_test.dart';

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
    await tester.pumpAndSettle();
    expect(find.text('Noch keine Listen'), findsOneWidget);

    // Liste anlegen
    await tester.tap(find.text('Neue Liste'));
    await tester.pumpAndSettle();
    await tester.tap(find.text('Einkaufsliste'));
    await tester.enterText(find.byType(TextField), 'Wocheneinkauf');
    await tester.tap(find.text('Anlegen'));
    await tester.pumpAndSettle();
    expect(find.text('Wocheneinkauf'), findsOneWidget);

    // Artikel hinzufügen
    for (final artikel in ['Milch', 'Brot']) {
      await tester.enterText(find.byKey(const Key('neuer-eintrag')), artikel);
      await tester.testTextInput.receiveAction(TextInputAction.done);
      await tester.pumpAndSettle();
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
    await tester.pumpAndSettle();
    expect(find.text('Erledigt (1)'), findsOneWidget);

    // Zurück zur Übersicht: Zähler stimmt
    await tester.tap(find.byType(BackButton));
    await tester.pumpAndSettle();
    expect(find.text('1 offen · 2 gesamt'), findsOneWidget);
  });
}
