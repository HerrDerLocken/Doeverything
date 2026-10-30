import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:path_provider/path_provider.dart';

import 'database.dart';
import 'screens/home_screen.dart';
import 'src/rust/api/tracker.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final dir = await getApplicationSupportDirectory();
  await dir.create(recursive: true);
  final dbPath = '${dir.path}${Platform.pathSeparator}doeverything.sqlite';
  try {
    final tracker = await openTracker(
      dbPath: dbPath,
      keys: const SecureKeyStore(),
    );
    runApp(DoeverythingApp(home: HomeScreen(tracker: tracker)));
  } catch (e) {
    runApp(
      DoeverythingApp(
        home: StartupErrorScreen(error: e, dbPath: dbPath),
      ),
    );
  }
}

class DoeverythingApp extends StatelessWidget {
  const DoeverythingApp({super.key, required this.home});

  final Widget home;

  @override
  Widget build(BuildContext context) {
    ThemeData theme(Brightness brightness) =>
        ThemeData(colorSchemeSeed: Colors.teal, brightness: brightness);

    return MaterialApp(
      title: 'Doeverything',
      theme: theme(Brightness.light),
      darkTheme: theme(Brightness.dark),
      localizationsDelegates: GlobalMaterialLocalizations.delegates,
      supportedLocales: const [Locale('de'), Locale('en')],
      home: home,
    );
  }
}

/// Wird statt eines Absturzes gezeigt, wenn die Datenbank nicht geöffnet werden kann.
/// Bietet bewusst keine Aktion an, die Daten löschen oder überschreiben könnte.
class StartupErrorScreen extends StatelessWidget {
  const StartupErrorScreen({
    super.key,
    required this.error,
    required this.dbPath,
  });

  final Object error;
  final String dbPath;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final hint = error is KeyMissingException
        ? 'Deine Daten sind unverändert und verschlüsselt gespeichert. '
              'Prüfe, ob der Schlüsselspeicher des Systems entsperrt ist '
              '(unter Linux z. B. KWallet oder GNOME Keyring), und starte die App neu.'
        : 'Deine Daten wurden nicht verändert.';
    return Scaffold(
      body: SafeArea(
        child: Padding(
          padding: const EdgeInsets.all(24),
          child: Column(
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Icon(
                Icons.lock_outline,
                size: 48,
                color: theme.colorScheme.error,
              ),
              const SizedBox(height: 16),
              Text(
                'Datenbank kann nicht geöffnet werden',
                style: theme.textTheme.headlineSmall,
              ),
              const SizedBox(height: 12),
              Text(hint),
              const SizedBox(height: 16),
              SelectableText(
                '$error\n\nDatei: $dbPath',
                style: theme.textTheme.bodySmall,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// Für Tests: App mit bereits geöffnetem Tracker.
Widget appWith(Tracker tracker) =>
    DoeverythingApp(home: HomeScreen(tracker: tracker));
