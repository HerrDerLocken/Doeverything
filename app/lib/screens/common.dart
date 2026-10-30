import 'package:flutter/material.dart';

/// Symbol je Modul-Vorlage (`todo@1`, `shopping@1`, …).
IconData templateIcon(String template) => switch (template.split('@').first) {
  'shopping' => Icons.shopping_cart_outlined,
  'todo' => Icons.check_circle_outline,
  'finance' => Icons.account_balance_wallet_outlined,
  _ => Icons.list_alt,
};

/// Führt eine Aktion aus und zeigt Fehler als Snackbar statt abzustürzen.
Future<T?> guarded<T>(BuildContext context, Future<T> Function() action) async {
  try {
    return await action();
  } catch (e) {
    if (context.mounted) {
      ScaffoldMessenger.of(context)
          .showSnackBar(SnackBar(content: Text('Fehler: $e')));
    }
    return null;
  }
}

/// Snackbar mit "Rückgängig"-Aktion.
void showUndo(BuildContext context, String message, VoidCallback onUndo) {
  ScaffoldMessenger.of(context)
    ..hideCurrentSnackBar()
    ..showSnackBar(
      SnackBar(
        content: Text(message),
        action: SnackBarAction(label: 'Rückgängig', onPressed: onUndo),
      ),
    );
}

/// `YYYY-MM-DD`, wie es die Rust-Seite erwartet.
String isoDate(DateTime d) =>
    '${d.year.toString().padLeft(4, '0')}-${d.month.toString().padLeft(2, '0')}-${d.day.toString().padLeft(2, '0')}';

const monthNames = [
  'Januar',
  'Februar',
  'März',
  'April',
  'Mai',
  'Juni',
  'Juli',
  'August',
  'September',
  'Oktober',
  'November',
  'Dezember',
];

/// `30.10.2026` aus `2026-10-30`.
String germanDate(String iso, {bool withYear = true}) {
  final d = DateTime.parse(iso);
  final day =
      '${d.day.toString().padLeft(2, '0')}.${d.month.toString().padLeft(2, '0')}.';
  return withYear ? '$day${d.year}' : day;
}
