import 'package:flutter/material.dart';

/// Symbol je Modul-Vorlage (`todo@1`, `shopping@1`, …).
IconData templateIcon(String template) => switch (template.split('@').first) {
  'shopping' => Icons.shopping_cart_outlined,
  'todo' => Icons.check_circle_outline,
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
