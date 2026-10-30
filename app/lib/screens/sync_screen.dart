import 'package:flutter/material.dart';
import 'package:flutter/services.dart';

import '../src/rust/api/tracker.dart';
import '../sync_service.dart';
import 'common.dart';

/// Einrichtung und Status der Synchronisation.
class SyncScreen extends StatelessWidget {
  const SyncScreen({super.key, required this.sync});

  final SyncService sync;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: sync,
      builder: (context, _) => Scaffold(
        appBar: AppBar(title: const Text('Synchronisation')),
        body: ListView(
          padding: const EdgeInsets.all(16),
          children: switch (sync.state?.mode) {
            null => const [Center(child: CircularProgressIndicator())],
            SyncMode.off => [_SetupForm(sync: sync)],
            SyncMode.pairing => [_PairingWait(sync: sync)],
            SyncMode.connected => [_Connected(sync: sync)],
          },
        ),
      ),
    );
  }
}

/// Erklärt, was der Server sieht. Steht auf allen drei Seiten.
class _PrivacyNote extends StatelessWidget {
  const _PrivacyNote();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Card(
      color: theme.colorScheme.surfaceContainerHigh,
      child: const ListTile(
        leading: Icon(Icons.lock_outline),
        title: Text('Ende-zu-Ende-verschlüsselt'),
        subtitle: Text(
          'Der Server speichert nur verschlüsselte Pakete. Listen, Einträge und '
          'Beträge kann er nicht lesen. Den Schlüssel haben nur deine Geräte.',
        ),
      ),
    );
  }
}

class _SetupForm extends StatefulWidget {
  const _SetupForm({required this.sync});

  final SyncService sync;

  @override
  State<_SetupForm> createState() => _SetupFormState();
}

class _SetupFormState extends State<_SetupForm> {
  final _server = TextEditingController();
  final _name = TextEditingController(text: SyncService.defaultDeviceName);
  bool _busy = false;
  String? _error;

  @override
  void dispose() {
    _server.dispose();
    _name.dispose();
    super.dispose();
  }

  Future<void> _run(Future<void> Function(String, String) action) async {
    setState(() {
      _busy = true;
      _error = null;
    });
    try {
      await action(_server.text.trim(), _name.text.trim());
    } catch (e) {
      if (mounted) setState(() => _error = errorText(e));
    } finally {
      if (mounted) setState(() => _busy = false);
    }
  }

  @override
  Widget build(BuildContext context) {
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text(
          'Synchronisiere deine Listen über einen eigenen oder gemeinsam '
          'genutzten Server zwischen deinen Geräten.',
          style: Theme.of(context).textTheme.bodyLarge,
        ),
        const SizedBox(height: 16),
        TextField(
          key: const Key('sync-server'),
          controller: _server,
          keyboardType: TextInputType.url,
          autocorrect: false,
          decoration: const InputDecoration(
            labelText: 'Server-Adresse',
            hintText: 'https://sync.example.org',
            helperText: 'Selbst hosten: podman compose up (siehe README)',
          ),
        ),
        const SizedBox(height: 8),
        TextField(
          controller: _name,
          decoration: const InputDecoration(
            labelText: 'Name dieses Geräts',
            helperText: 'Damit du es auf anderen Geräten wiedererkennst',
          ),
        ),
        if (_error != null) ...[
          const SizedBox(height: 12),
          Text(
            _error!,
            style: TextStyle(color: Theme.of(context).colorScheme.error),
          ),
        ],
        const SizedBox(height: 20),
        FilledButton.icon(
          onPressed: _busy ? null : () => _run(widget.sync.createAccount),
          icon: const Icon(Icons.person_add_alt),
          label: const Text('Neues Sync-Konto anlegen'),
        ),
        const SizedBox(height: 4),
        const Text('Für dein erstes Gerät.', textAlign: TextAlign.center),
        const SizedBox(height: 16),
        OutlinedButton.icon(
          onPressed: _busy ? null : () => _run(widget.sync.startPairing),
          icon: const Icon(Icons.devices),
          label: const Text('Mit bestehendem Gerät koppeln'),
        ),
        const SizedBox(height: 4),
        const Text(
          'Wenn du Sync auf einem anderen Gerät schon eingerichtet hast.',
          textAlign: TextAlign.center,
        ),
        if (_busy) ...[
          const SizedBox(height: 16),
          const Center(child: CircularProgressIndicator()),
        ],
        const SizedBox(height: 24),
        const _PrivacyNote(),
      ],
    );
  }
}

class _PairingWait extends StatelessWidget {
  const _PairingWait({required this.sync});

  final SyncService sync;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final code = sync.state?.code ?? '';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Text('Gerät koppeln', style: theme.textTheme.titleLarge),
        const SizedBox(height: 8),
        const Text(
          'Öffne auf einem Gerät, das schon synchronisiert, '
          '„Synchronisation → Gerät hinzufügen“ und füge diesen Code ein:',
        ),
        const SizedBox(height: 12),
        Card(
          child: Padding(
            padding: const EdgeInsets.all(12),
            child: SelectableText(
              code,
              key: const Key('pairing-code'),
              style: theme.textTheme.bodySmall?.copyWith(
                fontFamily: 'monospace',
              ),
            ),
          ),
        ),
        Align(
          alignment: Alignment.centerRight,
          child: TextButton.icon(
            onPressed: () {
              Clipboard.setData(ClipboardData(text: code));
              ScaffoldMessenger.of(context)
                  .showSnackBar(const SnackBar(content: Text('Code kopiert')));
            },
            icon: const Icon(Icons.copy),
            label: const Text('Kopieren'),
          ),
        ),
        const SizedBox(height: 8),
        const Row(
          children: [
            SizedBox.square(
              dimension: 20,
              child: CircularProgressIndicator(strokeWidth: 2),
            ),
            SizedBox(width: 12),
            Expanded(child: Text('Warte auf Bestätigung …')),
          ],
        ),
        if (sync.lastError != null) ...[
          const SizedBox(height: 12),
          Text(
            sync.lastError!,
            style: TextStyle(color: theme.colorScheme.error),
          ),
        ],
        const SizedBox(height: 16),
        Text(
          'Der Code enthält keine Daten und keinen Schlüssel. Er läuft nach '
          '10 Minuten ab, nachdem das andere Gerät ihn bestätigt hat.',
          style: theme.textTheme.bodySmall,
        ),
        const SizedBox(height: 16),
        OutlinedButton(
          onPressed: sync.disconnect,
          child: const Text('Abbrechen'),
        ),
      ],
    );
  }
}

class _Connected extends StatelessWidget {
  const _Connected({required this.sync});

  final SyncService sync;

  Future<void> _addDevice(BuildContext context) async {
    final code = await showDialog<String>(
      context: context,
      builder: (_) => const _CodeDialog(),
    );
    if (code == null || !context.mounted) return;
    final name = await guarded(context, () => sync.approve(code));
    if (name != null && context.mounted) {
      ScaffoldMessenger.of(context).showSnackBar(
        SnackBar(
          content: Text('„$name“ hinzugefügt. Es verbindet sich gleich.'),
        ),
      );
    }
  }

  Future<void> _disconnect(BuildContext context) async {
    final ok = await showDialog<bool>(
      context: context,
      builder: (context) => AlertDialog(
        title: const Text('Sync beenden?'),
        content: const Text(
          'Dieses Gerät synchronisiert dann nicht mehr. Deine Daten bleiben '
          'auf diesem Gerät und auf dem Server erhalten.',
        ),
        actions: [
          TextButton(
            onPressed: () => Navigator.of(context).pop(false),
            child: const Text('Abbrechen'),
          ),
          FilledButton(
            onPressed: () => Navigator.of(context).pop(true),
            child: const Text('Beenden'),
          ),
        ],
      ),
    );
    if (ok == true) await sync.disconnect();
  }

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final state = sync.state!;
    final last = sync.lastSync;
    final status = sync.syncing
        ? 'Synchronisiere …'
        : sync.lastError != null
        ? 'Fehler: ${sync.lastError}'
        : last == null
        ? 'Noch nicht synchronisiert'
        : 'Zuletzt um ${last.hour.toString().padLeft(2, '0')}:'
              '${last.minute.toString().padLeft(2, '0')}:'
              '${last.second.toString().padLeft(2, '0')}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Card(
          child: Column(
            children: [
              ListTile(
                leading: Icon(
                  sync.lastError != null
                      ? Icons.sync_problem
                      : Icons.cloud_done,
                  color: sync.lastError != null
                      ? theme.colorScheme.error
                      : null,
                ),
                title: Text(state.server ?? ''),
                subtitle: Text(status),
              ),
              ListTile(
                leading: const Icon(Icons.badge_outlined),
                title: const Text('Konto'),
                subtitle: Text(
                  state.account ?? '',
                  style: const TextStyle(fontFamily: 'monospace'),
                ),
              ),
            ],
          ),
        ),
        const SizedBox(height: 12),
        FilledButton.icon(
          onPressed: sync.syncing ? null : sync.syncNow,
          icon: const Icon(Icons.sync),
          label: const Text('Jetzt synchronisieren'),
        ),
        const SizedBox(height: 8),
        OutlinedButton.icon(
          onPressed: () => _addDevice(context),
          icon: const Icon(Icons.add_to_queue),
          label: const Text('Gerät hinzufügen'),
        ),
        const SizedBox(height: 24),
        const _PrivacyNote(),
        const SizedBox(height: 24),
        TextButton(
          onPressed: () => _disconnect(context),
          child: const Text('Sync auf diesem Gerät beenden'),
        ),
      ],
    );
  }
}

class _CodeDialog extends StatefulWidget {
  const _CodeDialog();

  @override
  State<_CodeDialog> createState() => _CodeDialogState();
}

class _CodeDialogState extends State<_CodeDialog> {
  final _code = TextEditingController();

  @override
  void dispose() {
    _code.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Gerät hinzufügen'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          const Text(
            'Wähle auf dem neuen Gerät „Mit bestehendem Gerät koppeln“ und '
            'füge den angezeigten Code hier ein.',
          ),
          const SizedBox(height: 12),
          TextField(
            controller: _code,
            autofocus: true,
            minLines: 2,
            maxLines: 4,
            decoration: const InputDecoration(
              labelText: 'Kopplungscode',
              hintText: 'DOE1-…',
            ),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Abbrechen'),
        ),
        FilledButton(
          onPressed: () => Navigator.of(context).pop(_code.text.trim()),
          child: const Text('Hinzufügen'),
        ),
      ],
    );
  }
}

/// Symbol für die App-Leiste, je nach Sync-Zustand.
class SyncIcon extends StatelessWidget {
  const SyncIcon({super.key, required this.sync});

  final SyncService sync;

  @override
  Widget build(BuildContext context) {
    return ListenableBuilder(
      listenable: sync,
      builder: (context, _) {
        final (icon, tooltip) = switch (sync.state?.mode) {
          SyncMode.connected when sync.syncing => (
            Icons.cloud_sync_outlined,
            'Synchronisiere …',
          ),
          SyncMode.connected when sync.lastError != null => (
            Icons.sync_problem,
            'Sync-Fehler',
          ),
          SyncMode.connected => (Icons.cloud_done_outlined, 'Synchronisiert'),
          SyncMode.pairing => (Icons.devices, 'Kopplung läuft'),
          _ => (Icons.cloud_off_outlined, 'Sync einrichten'),
        };
        return IconButton(
          tooltip: tooltip,
          icon: Icon(icon),
          onPressed: () => Navigator.of(context).push(
            MaterialPageRoute<void>(builder: (_) => SyncScreen(sync: sync)),
          ),
        );
      },
    );
  }
}
