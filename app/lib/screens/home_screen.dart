import 'package:flutter/material.dart';

import '../src/rust/api/tracker.dart';
import '../sync_service.dart';
import 'collection_screen.dart';
import 'ledger_screen.dart';
import 'sync_screen.dart';
import 'common.dart';

/// Übersicht aller Listen.
class HomeScreen extends StatefulWidget {
  const HomeScreen({super.key, required this.tracker, required this.sync});

  final Tracker tracker;
  final SyncService sync;

  @override
  State<HomeScreen> createState() => _HomeScreenState();
}

class _HomeScreenState extends State<HomeScreen> {
  List<CollectionInfo>? _collections;

  Tracker get _tracker => widget.tracker;

  int _seenVersion = 0;

  @override
  void initState() {
    super.initState();
    _seenVersion = widget.sync.dataVersion;
    widget.sync.addListener(_onSync);
    _load();
  }

  @override
  void dispose() {
    widget.sync.removeListener(_onSync);
    super.dispose();
  }

  /// Ein Sync hat neue Daten geholt.
  void _onSync() {
    if (widget.sync.dataVersion != _seenVersion) {
      _seenVersion = widget.sync.dataVersion;
      _load();
    }
  }

  /// Nach einer lokalen Änderung: neu laden und bald synchronisieren.
  Future<void> _changed() async {
    widget.sync.schedule();
    await _load();
  }

  Future<void> _load() async {
    final collections = await guarded(
      context,
      () => _tracker.collections(today: isoDate(DateTime.now())),
    );
    if (mounted && collections != null) {
      setState(() => _collections = collections);
    }
  }

  Future<void> _open(CollectionInfo collection) async {
    await Navigator.of(context).push(
      MaterialPageRoute<void>(
        builder: (_) => switch (collection.view) {
          CollectionView.ledger => LedgerScreen(
            tracker: _tracker,
            sync: widget.sync,
            collection: collection,
          ),
          CollectionView.checklist => CollectionScreen(
            tracker: _tracker,
            sync: widget.sync,
            collection: collection,
          ),
        },
      ),
    );
    await _changed();
  }

  Future<void> _create() async {
    final result = await showDialog<(String, String)>(
      context: context,
      builder: (_) => const _NewCollectionDialog(),
    );
    if (result == null || !mounted) return;
    final (name, template) = result;
    final id = await guarded(
      context,
      () => _tracker.createCollection(name: name, templateId: template),
    );
    if (id == null) return;
    await _changed();
    final created = _collections?.where((c) => c.id == id).firstOrNull;
    if (created != null) await _open(created);
  }

  Future<void> _rename(CollectionInfo collection) async {
    final name = await showDialog<String>(
      context: context,
      builder: (_) => _RenameDialog(initial: collection.name),
    );
    if (name == null || !mounted) return;
    await guarded(
      context,
      () => _tracker.renameCollection(collectionId: collection.id, name: name),
    );
    await _changed();
  }

  Future<void> _delete(CollectionInfo collection) async {
    await guarded(
      context,
      () => _tracker.deleteCollection(collectionId: collection.id),
    );
    await _changed();
    if (!mounted) return;
    showUndo(context, '„${collection.name}“ gelöscht', () async {
      await _tracker.restoreCollection(collectionId: collection.id);
      await _changed();
    });
  }

  String _subtitle(CollectionInfo c) {
    if (c.totalCount == 0) return 'Leer';
    return switch (c.view) {
      CollectionView.ledger =>
        'Diesen Monat: ${formatAmount(minor: c.monthBalanceMinor ?? 0, currency: c.currency ?? 'EUR')}',
      CollectionView.checklist =>
        '${c.openCount} offen · ${c.totalCount} gesamt',
    };
  }

  @override
  Widget build(BuildContext context) {
    final collections = _collections;
    return Scaffold(
      appBar: AppBar(
        title: const Text('Doeverything'),
        actions: [SyncIcon(sync: widget.sync)],
      ),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: _create,
        icon: const Icon(Icons.add),
        label: const Text('Neue Liste'),
      ),
      body: switch (collections) {
        null => const Center(child: CircularProgressIndicator()),
        [] => const _EmptyState(),
        _ => ListView.builder(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 96),
          itemCount: collections.length,
          itemBuilder: (context, index) {
            final c = collections[index];
            return Card(
              child: ListTile(
                leading: Icon(templateIcon(c.template)),
                title: Text(c.name),
                subtitle: Text(_subtitle(c)),
                onTap: () => _open(c),
                trailing: PopupMenuButton<String>(
                  onSelected: (action) => switch (action) {
                    'rename' => _rename(c),
                    _ => _delete(c),
                  },
                  itemBuilder: (_) => const [
                    PopupMenuItem(value: 'rename', child: Text('Umbenennen')),
                    PopupMenuItem(value: 'delete', child: Text('Löschen')),
                  ],
                ),
              ),
            );
          },
        ),
      },
    );
  }
}

class _EmptyState extends StatelessWidget {
  const _EmptyState();

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    return Center(
      child: Padding(
        padding: const EdgeInsets.all(32),
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(Icons.checklist, size: 64, color: theme.colorScheme.primary),
            const SizedBox(height: 16),
            Text('Noch keine Listen', style: theme.textTheme.titleLarge),
            const SizedBox(height: 8),
            const Text(
              'Lege eine To-Do-Liste, Einkaufsliste oder ein Haushaltsbuch an.',
              textAlign: TextAlign.center,
            ),
          ],
        ),
      ),
    );
  }
}

class _NewCollectionDialog extends StatefulWidget {
  const _NewCollectionDialog();

  @override
  State<_NewCollectionDialog> createState() => _NewCollectionDialogState();
}

class _NewCollectionDialogState extends State<_NewCollectionDialog> {
  final _templates = templates();
  final _name = TextEditingController();
  late String _template = _templates.first.id;

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    final template = _templates.firstWhere((t) => t.id == _template);
    final name = _name.text.trim().isEmpty ? template.name : _name.text.trim();
    Navigator.of(context).pop((name, _template));
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Neue Liste'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Wrap(
            spacing: 8,
            children: [
              for (final t in _templates)
                ChoiceChip(
                  avatar: Icon(templateIcon(t.id), size: 18),
                  label: Text(t.name),
                  selected: t.id == _template,
                  onSelected: (_) => setState(() => _template = t.id),
                ),
            ],
          ),
          const SizedBox(height: 16),
          TextField(
            controller: _name,
            autofocus: true,
            decoration: const InputDecoration(
              labelText: 'Name',
              hintText: 'z. B. Wocheneinkauf',
            ),
            onSubmitted: (_) => _submit(),
          ),
        ],
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Abbrechen'),
        ),
        FilledButton(onPressed: _submit, child: const Text('Anlegen')),
      ],
    );
  }
}

class _RenameDialog extends StatefulWidget {
  const _RenameDialog({required this.initial});

  final String initial;

  @override
  State<_RenameDialog> createState() => _RenameDialogState();
}

class _RenameDialogState extends State<_RenameDialog> {
  late final _name = TextEditingController(text: widget.initial);

  @override
  void dispose() {
    _name.dispose();
    super.dispose();
  }

  void _submit() {
    final name = _name.text.trim();
    if (name.isNotEmpty) Navigator.of(context).pop(name);
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Umbenennen'),
      content: TextField(
        controller: _name,
        autofocus: true,
        decoration: const InputDecoration(labelText: 'Name'),
        onSubmitted: (_) => _submit(),
      ),
      actions: [
        TextButton(
          onPressed: () => Navigator.of(context).pop(),
          child: const Text('Abbrechen'),
        ),
        FilledButton(onPressed: _submit, child: const Text('Speichern')),
      ],
    );
  }
}
