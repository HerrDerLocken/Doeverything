import 'package:flutter/material.dart';

import '../src/rust/api/tracker.dart';
import '../sync_service.dart';
import 'common.dart';

/// Einträge einer Liste: offene oben, erledigte darunter.
class CollectionScreen extends StatefulWidget {
  const CollectionScreen({
    super.key,
    required this.tracker,
    required this.sync,
    required this.collection,
  });

  final Tracker tracker;
  final SyncService sync;
  final CollectionInfo collection;

  @override
  State<CollectionScreen> createState() => _CollectionScreenState();
}

class _CollectionScreenState extends State<CollectionScreen> {
  List<ItemInfo>? _items;
  final _title = TextEditingController();
  final _quantity = TextEditingController();
  final _titleFocus = FocusNode();

  Tracker get _tracker => widget.tracker;
  String get _id => widget.collection.id;
  bool get _hasQuantity => widget.collection.template.startsWith('shopping');

  int _seenVersion = 0;

  @override
  void initState() {
    super.initState();
    _seenVersion = widget.sync.dataVersion;
    widget.sync.addListener(_onSync);
    _load();
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

  @override
  void dispose() {
    widget.sync.removeListener(_onSync);
    _title.dispose();
    _quantity.dispose();
    _titleFocus.dispose();
    super.dispose();
  }

  Future<void> _load() async {
    final items = await guarded(
      context,
      () => _tracker.items(collectionId: _id),
    );
    if (mounted && items != null) setState(() => _items = items);
  }

  Future<void> _add() async {
    final title = _title.text.trim();
    if (title.isEmpty) return;
    final quantity = _quantity.text.trim();
    _title.clear();
    _quantity.clear();
    _titleFocus.requestFocus();
    await guarded(
      context,
      () => _tracker.addItem(
        collectionId: _id,
        title: title,
        quantity: quantity.isEmpty ? null : quantity,
      ),
    );
    await _changed();
  }

  Future<void> _toggle(ItemInfo item) async {
    await guarded(
      context,
      () => _tracker.setDone(itemId: item.id, done: !item.done),
    );
    await _changed();
  }

  Future<void> _delete(ItemInfo item) async {
    // Sofort aus der Liste nehmen, damit Dismissible nicht auf einen Reload wartet.
    setState(() => _items = _items?.where((i) => i.id != item.id).toList());
    await guarded(context, () => _tracker.deleteItem(itemId: item.id));
    await _changed();
    if (!mounted) return;
    showUndo(context, '„${item.title}“ gelöscht', () async {
      await _tracker.restoreItems(itemIds: [item.id]);
      await _changed();
    });
  }

  Future<void> _clearDone() async {
    final removed = await guarded(
      context,
      () => _tracker.clearDone(collectionId: _id),
    );
    await _changed();
    if (!mounted || removed == null || removed.isEmpty) return;
    showUndo(context, '${removed.length} erledigte entfernt', () async {
      await _tracker.restoreItems(itemIds: removed);
      await _changed();
    });
  }

  Future<void> _edit(ItemInfo item) async {
    final result = await showDialog<(String, String?)>(
      context: context,
      builder: (_) => _EditItemDialog(item: item, withQuantity: _hasQuantity),
    );
    if (result == null || !mounted) return;
    final (title, quantity) = result;
    await guarded(
      context,
      () => _tracker.updateItem(
        itemId: item.id,
        title: title,
        quantity: quantity,
      ),
    );
    await _changed();
  }

  @override
  Widget build(BuildContext context) {
    final items = _items;
    final open = items?.where((i) => !i.done).toList() ?? [];
    final done = items?.where((i) => i.done).toList() ?? [];

    return Scaffold(
      appBar: AppBar(
        title: Text(widget.collection.name),
        actions: [
          IconButton(
            tooltip: 'Erledigte entfernen',
            icon: const Icon(Icons.delete_sweep_outlined),
            onPressed: done.isEmpty ? null : _clearDone,
          ),
        ],
      ),
      body: Column(
        children: [
          Expanded(
            child: switch (items) {
              null => const Center(child: CircularProgressIndicator()),
              [] => Center(
                child: Text(
                  _hasQuantity
                      ? 'Liste ist leer. Was fehlt noch?'
                      : 'Nichts zu tun 🎉',
                  style: Theme.of(context).textTheme.bodyLarge,
                ),
              ),
              _ => ListView(
                padding: const EdgeInsets.only(bottom: 8),
                children: [
                  for (final item in open) _tile(item),
                  if (done.isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.fromLTRB(16, 16, 16, 4),
                      child: Text(
                        'Erledigt (${done.length})',
                        style: Theme.of(context).textTheme.labelLarge,
                      ),
                    ),
                  for (final item in done) _tile(item),
                ],
              ),
            },
          ),
          _inputBar(context),
        ],
      ),
    );
  }

  Widget _tile(ItemInfo item) {
    final theme = Theme.of(context);
    return Dismissible(
      key: ValueKey(item.id),
      direction: DismissDirection.endToStart,
      background: Container(
        color: theme.colorScheme.errorContainer,
        alignment: Alignment.centerRight,
        padding: const EdgeInsets.symmetric(horizontal: 24),
        child: Icon(
          Icons.delete_outline,
          color: theme.colorScheme.onErrorContainer,
        ),
      ),
      onDismissed: (_) => _delete(item),
      child: ListTile(
        leading: Checkbox(value: item.done, onChanged: (_) => _toggle(item)),
        title: Text(
          item.title,
          style: item.done
              ? TextStyle(
                  decoration: TextDecoration.lineThrough,
                  color: theme.colorScheme.onSurfaceVariant,
                )
              : null,
        ),
        subtitle: item.quantity == null ? null : Text(item.quantity!),
        onTap: () => _edit(item),
      ),
    );
  }

  Widget _inputBar(BuildContext context) {
    return Material(
      elevation: 3,
      child: SafeArea(
        top: false,
        child: Padding(
          padding: const EdgeInsets.fromLTRB(12, 8, 8, 8),
          child: Row(
            children: [
              Expanded(
                flex: 3,
                child: TextField(
                  key: const Key('neuer-eintrag'),
                  controller: _title,
                  focusNode: _titleFocus,
                  textInputAction: TextInputAction.done,
                  decoration: InputDecoration(
                    hintText: _hasQuantity ? 'Artikel' : 'Neue Aufgabe',
                    border: InputBorder.none,
                  ),
                  onSubmitted: (_) => _add(),
                ),
              ),
              if (_hasQuantity)
                Expanded(
                  child: TextField(
                    controller: _quantity,
                    decoration: const InputDecoration(
                      hintText: 'Menge',
                      border: InputBorder.none,
                    ),
                    onSubmitted: (_) => _add(),
                  ),
                ),
              IconButton.filled(
                tooltip: 'Hinzufügen',
                icon: const Icon(Icons.add),
                onPressed: _add,
              ),
            ],
          ),
        ),
      ),
    );
  }
}

class _EditItemDialog extends StatefulWidget {
  const _EditItemDialog({required this.item, required this.withQuantity});

  final ItemInfo item;
  final bool withQuantity;

  @override
  State<_EditItemDialog> createState() => _EditItemDialogState();
}

class _EditItemDialogState extends State<_EditItemDialog> {
  late final _title = TextEditingController(text: widget.item.title);
  late final _quantity = TextEditingController(text: widget.item.quantity);

  @override
  void dispose() {
    _title.dispose();
    _quantity.dispose();
    super.dispose();
  }

  void _submit() {
    final title = _title.text.trim();
    if (title.isEmpty) return;
    final quantity = _quantity.text.trim();
    Navigator.of(context).pop((title, quantity.isEmpty ? null : quantity));
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: const Text('Bearbeiten'),
      content: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          TextField(
            controller: _title,
            autofocus: true,
            decoration: const InputDecoration(labelText: 'Titel'),
            onSubmitted: (_) => _submit(),
          ),
          if (widget.withQuantity)
            TextField(
              controller: _quantity,
              decoration: const InputDecoration(labelText: 'Menge'),
              onSubmitted: (_) => _submit(),
            ),
        ],
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
