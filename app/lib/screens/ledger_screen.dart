import 'package:flutter/material.dart';

import '../src/rust/api/tracker.dart';
import 'common.dart';

/// Haushaltsbuch: Buchungen nach Monat mit Einnahmen, Ausgaben, Saldo und Kategorien.
class LedgerScreen extends StatefulWidget {
  const LedgerScreen({
    super.key,
    required this.tracker,
    required this.collection,
  });

  final Tracker tracker;
  final CollectionInfo collection;

  @override
  State<LedgerScreen> createState() => _LedgerScreenState();
}

class _LedgerScreenState extends State<LedgerScreen> {
  LedgerData? _data;

  /// Weggewischte Buchungen, bis der nächste Reload sie ohnehin nicht mehr enthält.
  final _hidden = <String>{};

  Tracker get _tracker => widget.tracker;
  String get _id => widget.collection.id;

  @override
  void initState() {
    super.initState();
    _load();
  }

  Future<void> _load() async {
    final data = await guarded(
      context,
      () => _tracker.ledger(collectionId: _id),
    );
    if (mounted && data != null) {
      setState(() {
        _data = data;
        _hidden.clear();
      });
    }
  }

  String _money(int minor) =>
      formatAmount(minor: minor, currency: _data?.currency ?? 'EUR');

  Future<CategoryInfo?> _addCategory(String name) async {
    final id = await guarded(
      context,
      () => _tracker.addCategory(collectionId: _id, name: name),
    );
    if (id == null) return null;
    await _load();
    return CategoryInfo(id: id, name: name);
  }

  Future<void> _add() async {
    final data = _data;
    if (data == null) return;
    final result = await showDialog<_EntryResult>(
      context: context,
      builder: (_) => _EntryDialog(
        categories: data.categories,
        onAddCategory: _addCategory,
      ),
    );
    if (result == null || !mounted) return;
    await guarded(
      context,
      () => _tracker.addEntry(
        collectionId: _id,
        description: result.description,
        amountMinor: result.amountMinor,
        date: result.date,
        categoryId: result.categoryId,
      ),
    );
    await _load();
  }

  Future<void> _edit(LedgerEntry entry) async {
    final data = _data;
    if (data == null) return;
    final result = await showDialog<_EntryResult>(
      context: context,
      builder: (_) => _EntryDialog(
        categories: data.categories,
        onAddCategory: _addCategory,
        initial: entry,
      ),
    );
    if (result == null || !mounted) return;
    await guarded(
      context,
      () => _tracker.updateEntry(
        entryId: entry.id,
        description: result.description,
        amountMinor: result.amountMinor,
        date: result.date,
        categoryId: result.categoryId,
      ),
    );
    await _load();
  }

  Future<void> _delete(LedgerEntry entry) async {
    setState(() => _hidden.add(entry.id));
    await guarded(context, () => _tracker.deleteItem(itemId: entry.id));
    await _load();
    if (!mounted) return;
    final label = entry.description.isEmpty
        ? _money(entry.amountMinor)
        : '„${entry.description}“';
    showUndo(context, '$label gelöscht', () async {
      await _tracker.restoreItems(itemIds: [entry.id]);
      await _load();
    });
  }

  @override
  Widget build(BuildContext context) {
    final data = _data;
    return Scaffold(
      appBar: AppBar(title: Text(widget.collection.name)),
      floatingActionButton: FloatingActionButton.extended(
        onPressed: data == null ? null : _add,
        icon: const Icon(Icons.add),
        label: const Text('Buchung'),
      ),
      body: switch (data) {
        null => const Center(child: CircularProgressIndicator()),
        LedgerData(months: []) => Center(
          child: Text(
            'Noch keine Buchungen',
            style: Theme.of(context).textTheme.bodyLarge,
          ),
        ),
        _ => ListView(
          padding: const EdgeInsets.fromLTRB(12, 8, 12, 96),
          children: [
            for (final (index, month) in data.months.indexed) ...[
              _MonthCard(
                month: month,
                money: _money,
                initiallyExpanded: index == 0,
              ),
              for (final entry in month.entries)
                if (!_hidden.contains(entry.id)) _entryTile(entry),
              const SizedBox(height: 12),
            ],
          ],
        ),
      },
    );
  }

  Widget _entryTile(LedgerEntry entry) {
    final theme = Theme.of(context);
    final income = entry.amountMinor >= 0;
    final category = entry.categoryName;
    final title = entry.description.isNotEmpty
        ? entry.description
        : (category ?? 'Ohne Beschreibung');
    return Dismissible(
      key: ValueKey(entry.id),
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
      onDismissed: (_) => _delete(entry),
      child: ListTile(
        leading: CircleAvatar(
          backgroundColor: income
              ? incomeColor(theme).withValues(alpha: 0.15)
              : theme.colorScheme.surfaceContainerHighest,
          child: income
              ? Icon(Icons.south_west, color: incomeColor(theme), size: 20)
              : Text(
                  (category ?? '?').characters.first,
                  style: theme.textTheme.titleMedium,
                ),
        ),
        title: Text(title),
        subtitle: Text(
          [
            germanDate(entry.date, withYear: false),
            if (category != null && entry.description.isNotEmpty) category,
          ].join(' · '),
        ),
        trailing: Text(
          _money(entry.amountMinor),
          style: theme.textTheme.titleMedium?.copyWith(
            color: income ? incomeColor(theme) : null,
            fontFeatures: const [FontFeature.tabularFigures()],
          ),
        ),
        onTap: () => _edit(entry),
      ),
    );
  }
}

Color incomeColor(ThemeData theme) => theme.brightness == Brightness.dark
    ? Colors.green.shade300
    : Colors.green.shade700;

class _MonthCard extends StatelessWidget {
  const _MonthCard({
    required this.month,
    required this.money,
    required this.initiallyExpanded,
  });

  final MonthView month;
  final String Function(int) money;
  final bool initiallyExpanded;

  @override
  Widget build(BuildContext context) {
    final theme = Theme.of(context);
    final balance = month.balanceMinor;
    Widget stat(String label, int minor, {Color? color}) => Expanded(
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Text(label, style: theme.textTheme.labelMedium),
          Text(
            money(minor),
            style: theme.textTheme.titleMedium?.copyWith(color: color),
          ),
        ],
      ),
    );

    return Card(
      child: Padding(
        padding: const EdgeInsets.fromLTRB(16, 12, 16, 4),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Text(
              month.year == 1970
                  ? 'Ohne Datum'
                  : '${monthNames[month.month - 1]} ${month.year}',
              style: theme.textTheme.titleLarge,
            ),
            const SizedBox(height: 8),
            Row(
              children: [
                stat('Einnahmen', month.incomeMinor, color: incomeColor(theme)),
                stat('Ausgaben', -month.expenseMinor),
                stat(
                  'Saldo',
                  balance,
                  color: balance < 0
                      ? theme.colorScheme.error
                      : incomeColor(theme),
                ),
              ],
            ),
            if (month.categories.isNotEmpty)
              Theme(
                // ExpansionTile ohne Trennlinien innerhalb der Karte
                data: theme.copyWith(dividerColor: Colors.transparent),
                child: ExpansionTile(
                  tilePadding: EdgeInsets.zero,
                  childrenPadding: const EdgeInsets.only(bottom: 12),
                  initiallyExpanded: initiallyExpanded,
                  title: Text(
                    'Ausgaben nach Kategorie',
                    style: theme.textTheme.labelLarge,
                  ),
                  children: [
                    for (final c in month.categories)
                      _CategoryBar(
                        name: c.name,
                        amount: money(c.expenseMinor),
                        share: month.expenseMinor == 0
                            ? 0
                            : c.expenseMinor / month.expenseMinor,
                      ),
                  ],
                ),
              )
            else
              const SizedBox(height: 12),
          ],
        ),
      ),
    );
  }
}

class _CategoryBar extends StatelessWidget {
  const _CategoryBar({
    required this.name,
    required this.amount,
    required this.share,
  });

  final String name;
  final String amount;
  final double share;

  @override
  Widget build(BuildContext context) {
    return Padding(
      padding: const EdgeInsets.symmetric(vertical: 4),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Expanded(child: Text(name)),
              Text('$amount · ${(share * 100).round()} %'),
            ],
          ),
          const SizedBox(height: 4),
          ClipRRect(
            borderRadius: BorderRadius.circular(4),
            child: LinearProgressIndicator(value: share, minHeight: 6),
          ),
        ],
      ),
    );
  }
}

typedef _EntryResult = ({
  String description,
  int amountMinor,
  String date,
  String? categoryId,
});

class _EntryDialog extends StatefulWidget {
  const _EntryDialog({
    required this.categories,
    required this.onAddCategory,
    this.initial,
  });

  final List<CategoryInfo> categories;
  final Future<CategoryInfo?> Function(String name) onAddCategory;
  final LedgerEntry? initial;

  @override
  State<_EntryDialog> createState() => _EntryDialogState();
}

class _EntryDialogState extends State<_EntryDialog> {
  static const _newCategory = '__neu__';

  late final _categories = [...widget.categories];
  late bool _expense = (widget.initial?.amountMinor ?? -1) < 0;
  late final _amount = TextEditingController(
    text: widget.initial == null ? '' : _plain(widget.initial!.amountMinor),
  );
  late final _description = TextEditingController(
    text: widget.initial?.description,
  );
  late DateTime _date = widget.initial == null
      ? DateTime.now()
      : DateTime.parse(widget.initial!.date);
  late String? _categoryId = widget.initial?.categoryId;
  String? _amountError;

  /// Betrag ohne Vorzeichen und Währung, z. B. `12,34`.
  static String _plain(int minor) {
    final abs = minor.abs();
    return '${abs ~/ 100},${(abs % 100).toString().padLeft(2, '0')}';
  }

  @override
  void dispose() {
    _amount.dispose();
    _description.dispose();
    super.dispose();
  }

  Future<void> _pickDate() async {
    final picked = await showDatePicker(
      context: context,
      initialDate: _date,
      firstDate: DateTime(2000),
      lastDate: DateTime(2100),
    );
    if (picked != null) setState(() => _date = picked);
  }

  Future<void> _selectCategory(String? value) async {
    if (value != _newCategory) {
      setState(() => _categoryId = value);
      return;
    }
    final name = await showDialog<String>(
      context: context,
      builder: (_) => const _CategoryNameDialog(),
    );
    if (name == null) return;
    final created = await widget.onAddCategory(name);
    if (created == null || !mounted) return;
    setState(() {
      if (!_categories.any((c) => c.id == created.id)) {
        _categories.add(created);
      }
      _categoryId = created.id;
    });
  }

  void _submit() {
    final parsed = parseAmount(text: _amount.text);
    if (parsed == null || parsed == 0) {
      setState(() => _amountError = 'Bitte einen Betrag eingeben, z. B. 12,34');
      return;
    }
    final abs = parsed.abs();
    Navigator.of(context).pop((
      description: _description.text.trim(),
      amountMinor: _expense ? -abs : abs,
      date: isoDate(_date),
      categoryId: _categoryId,
    ));
  }

  @override
  Widget build(BuildContext context) {
    return AlertDialog(
      title: Text(
        widget.initial == null ? 'Neue Buchung' : 'Buchung bearbeiten',
      ),
      content: SingleChildScrollView(
        child: Column(
          mainAxisSize: MainAxisSize.min,
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            SegmentedButton<bool>(
              segments: const [
                ButtonSegment(
                  value: true,
                  label: Text('Ausgabe'),
                  icon: Icon(Icons.north_east),
                ),
                ButtonSegment(
                  value: false,
                  label: Text('Einnahme'),
                  icon: Icon(Icons.south_west),
                ),
              ],
              selected: {_expense},
              onSelectionChanged: (s) => setState(() => _expense = s.first),
            ),
            const SizedBox(height: 12),
            TextField(
              key: const Key('betrag'),
              controller: _amount,
              autofocus: true,
              keyboardType: const TextInputType.numberWithOptions(
                decimal: true,
              ),
              decoration: InputDecoration(
                labelText: 'Betrag',
                suffixText: '€',
                errorText: _amountError,
              ),
              onChanged: (_) {
                if (_amountError != null) setState(() => _amountError = null);
              },
              onSubmitted: (_) => _submit(),
            ),
            TextField(
              key: const Key('beschreibung'),
              controller: _description,
              decoration: const InputDecoration(labelText: 'Beschreibung'),
              onSubmitted: (_) => _submit(),
            ),
            const SizedBox(height: 8),
            DropdownButtonFormField<String?>(
              initialValue: _categoryId,
              decoration: const InputDecoration(labelText: 'Kategorie'),
              items: [
                const DropdownMenuItem(
                  value: null,
                  child: Text('Ohne Kategorie'),
                ),
                for (final c in _categories)
                  DropdownMenuItem(value: c.id, child: Text(c.name)),
                const DropdownMenuItem(
                  value: _newCategory,
                  child: Text('Neue Kategorie …'),
                ),
              ],
              onChanged: _selectCategory,
            ),
            const SizedBox(height: 8),
            ListTile(
              contentPadding: EdgeInsets.zero,
              leading: const Icon(Icons.event),
              title: Text(germanDate(isoDate(_date))),
              onTap: _pickDate,
            ),
          ],
        ),
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

class _CategoryNameDialog extends StatefulWidget {
  const _CategoryNameDialog();

  @override
  State<_CategoryNameDialog> createState() => _CategoryNameDialogState();
}

class _CategoryNameDialogState extends State<_CategoryNameDialog> {
  final _name = TextEditingController();

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
      title: const Text('Neue Kategorie'),
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
        FilledButton(onPressed: _submit, child: const Text('Anlegen')),
      ],
    );
  }
}
