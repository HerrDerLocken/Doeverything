import 'dart:io';

import 'package:flutter/material.dart';
import 'package:flutter_localizations/flutter_localizations.dart';
import 'package:path_provider/path_provider.dart';

import 'screens/home_screen.dart';
import 'src/rust/api/tracker.dart';
import 'src/rust/frb_generated.dart';

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init();
  final dir = await getApplicationSupportDirectory();
  await dir.create(recursive: true);
  final tracker = await Tracker.open(
    dbPath: '${dir.path}${Platform.pathSeparator}doeverything.sqlite',
  );
  runApp(DoeverythingApp(tracker: tracker));
}

class DoeverythingApp extends StatelessWidget {
  const DoeverythingApp({super.key, required this.tracker});

  final Tracker tracker;

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
      home: HomeScreen(tracker: tracker),
    );
  }
}
