#include "commands.hpp"
#include "shortcuts.hpp"
#include "localization.hpp"
#include "dialog_shortcuts.hpp"
#include "window.hpp"
#include "canvas.hpp"
#include "tool_panel.hpp"
#include "adjustments.hpp"
#include <functional>
#include "photo_identity.hpp"
#include <QAbstractItemView>
#include <QAbstractSlider>
#include <QStatusBar>
#include <QSignalBlocker>
#include <QAbstractSpinBox>
#include <QApplication>
#include <QComboBox>
#include <QDialog>
#include <QDialogButtonBox>
#include <QHeaderView>
#include <QKeyEvent>
#include <QKeySequenceEdit>
#include <QMessageBox>
#include <QMenu>
#include <QPlainTextEdit>
#include <QPushButton>
#include <QTextEdit>
#include <QTreeWidget>
#include <QVBoxLayout>
namespace aigle {
void Window::initializeShortcuts() {
    const QList<QPair<QString, const char*>> commands{
        {"open_photo", QT_TR_NOOP("Open &Photo…")},
        {"open_folder", QT_TR_NOOP("Open &Folder…")},
        {"save", QT_TR_NOOP("&Save")},
        {"export", QT_TR_NOOP("&Export…")},
        {"quit", QT_TR_NOOP("&Quit")},
        {"undo", QT_TR_NOOP("&Undo")},
        {"redo", QT_TR_NOOP("&Redo")},
        {"copy_adjustments", QT_TR_NOOP("Copy selected adjustments…")},
        {"paste_adjustments", QT_TR_NOOP("Paste selected adjustments")},
        {"fit", QT_TR_NOOP("Fit image")},
        {"actual_pixels", QT_TR_NOOP("Actual pixels")},
        {"hdr", QT_TR_NOOP("HDR canvas")},
        {"before_after", QT_TR_NOOP("Before / after")},
        {"show_original", QT_TR_NOOP("Show original")},
        {"clipping", QT_TR_NOOP("Clipping overlay")},
        {"library", QT_TR_NOOP("Library")},
        {"develop", QT_TR_NOOP("Develop")},
        {"export_jobs", QT_TR_NOOP("Export jobs…")},
        {"cancel_exports", QT_TR_NOOP("Cancel exports")},
        {"reset_all_adjustments", QT_TR_NOOP("Reset all adjustments")},
        {"auto_white_balance", QT_TR_NOOP("Auto white balance")},
        {"auto_tone", QT_TR_NOOP("Auto tone")},
        {"load_preset", QT_TR_NOOP("Load preset…")},
        {"save_preset", QT_TR_NOOP("Save preset…")},
        {"sync_selected_adjustments_to_selection", QT_TR_NOOP("Sync selected adjustments to selection…")},
        {"preferences", QT_TR_NOOP("Preferences…")},
        {"keyboard_shortcuts", QT_TR_NOOP("Keyboard shortcuts…")},
        {"create_snapshot", QT_TR_NOOP("Create snapshot…")},
        {"restore_snapshot", QT_TR_NOOP("Restore snapshot…")},
        {"create_virtual_copy", QT_TR_NOOP("Create virtual copy…")},
        {"open_virtual_copy", QT_TR_NOOP("Open virtual copy…")},
        {"rename_current_virtual_copy", QT_TR_NOOP("Rename current virtual copy…")},
        {"delete_current_virtual_copy", QT_TR_NOOP("Delete current virtual copy…")},
        {"open_master_photo", QT_TR_NOOP("Open master photo")},
        {"import_photos", QT_TR_NOOP("Import photos…")},
        {"cancel_import", QT_TR_NOOP("Cancel import")},
        {"create_album_from_selection", QT_TR_NOOP("Create album from selection…")},
        {"open_album", QT_TR_NOOP("Open album…")},
        {"add_selection_to_album", QT_TR_NOOP("Add selection to album…")},
        {"remove_selection_from_current_album", QT_TR_NOOP("Remove selection from current album")},
        {"rename_album", QT_TR_NOOP("Rename album…")},
        {"delete_album", QT_TR_NOOP("Delete album…")},
        {"stack_selected_photos", QT_TR_NOOP("Stack selected photos")},
        {"unstack_selected_stacks", QT_TR_NOOP("Unstack selected stacks")},
        {"collapse_stacks", QT_TR_NOOP("Collapse stacks")},
        {"find_exact_duplicates", QT_TR_NOOP("Find exact duplicates…")},
        {"compare_selected_photos", QT_TR_NOOP("Compare selected photos…")},
        {"rename_selected_photo", QT_TR_NOOP("Rename selected photo…")},
        {"move_selected_photos", QT_TR_NOOP("Move selected photos…")},
        {"move_selected_photos_to_trash", QT_TR_NOOP("Move selected photos to Trash…")},
        {"locate_missing_original", QT_TR_NOOP("Locate missing original…")},
        {"manage_saved_searches", QT_TR_NOOP("Manage saved searches…")},
        {"back_up_catalog", QT_TR_NOOP("Back up catalog…")},
        {"restore_catalog", QT_TR_NOOP("Restore catalog…")},
        {"enable_soft_proof", QT_TR_NOOP("Enable soft proof")},
        {"gamut_warning", QT_TR_NOOP("Gamut warning")},
        {"history", QT_TR_NOOP("History")},
        {"metadata_sidebar", QT_TR_NOOP("Metadata sidebar")},
        {"about_aigle", QT_TR_NOOP("About Aigle")},
        {"brush_radius", QT_TR_NOOP("Brush radius…")},
        {"pause_after_current_photo", QT_TR_NOOP("Pause after current photo")},
        {"resume_exports", QT_TR_NOOP("Resume exports")},
        {"pan", QT_TR_NOOP("Pan")},
        {"crop", QT_TR_NOOP("Crop")},
        {"white_balance_picker", QT_TR_NOOP("White balance picker")},
        {"heal_spot", QT_TR_NOOP("Heal spot")},
        {"clone_spot", QT_TR_NOOP("Clone spot")},
        {"red_eye", QT_TR_NOOP("Red eye")},
        {"radial_mask", QT_TR_NOOP("Radial mask")},
        {"linear_mask", QT_TR_NOOP("Linear mask")},
        {"brush_mask", QT_TR_NOOP("Brush mask")}};
    for (const auto& [id, title] : commands) {
        for (auto* action : findChildren<QAction*>()) {
            if (action->text() != tr(title) || action->isSeparator() || action->menu())
                continue;
            bindTranslation(action, [action, title] { action->setText(Window::tr(title)); });
            action->setProperty("commandId", id);
            action->setProperty("defaultShortcut",
                                action->shortcut().toString(QKeySequence::PortableText));
            addAction(action);
            shortcutActions_.append(action);
            break;
        }
    }
    for (auto* action : findChildren<QAction*>()) {
        if (!action->property("commandId").isValid() || shortcutActions_.contains(action)) continue;
        action->setProperty("defaultShortcut", action->shortcut().toString(QKeySequence::PortableText));
        addAction(action);
        shortcutActions_.append(action);
    }
    const auto addCommand = [this](const QString& id, std::function<QString()> title, const QString& binding,
                                    std::function<void()> execute) {
        auto* action = new QAction(title(), this);
        bindTranslation(action, [action, title] { action->setText(title()); });
        action->setProperty("commandId", id);
        action->setProperty("defaultShortcut", binding);
        action->setShortcut(QKeySequence(binding));
        connect(action, &QAction::triggered, this, [execute](bool) { execute(); });
        addAction(action);
        shortcutActions_.append(action);
    };
    const auto saveChoice = [this](const QString& key, const QVariant& value) {
        settings_.setValue(key, value);
        settings_.sync();
        if (settings_.status() == QSettings::NoError) return true;
        statusBar()->showMessage(tr("Could not save preferences."), 10000);
        return false;
    };
    addCommand("language_english", [this] { return tr("Language: English"); }, {}, [this, saveChoice] {
        if (saveChoice("preferences/language", "en")) applyTranslations(*qApp, "en");
    });
    addCommand("language_chinese", [this] { return tr("Language: Simplified Chinese"); }, {}, [this, saveChoice] {
        if (saveChoice("preferences/language", "zh-CN")) applyTranslations(*qApp, "zh-CN");
    });
    addCommand("display_auto", [this] { return tr("Display profile: automatic"); }, {}, [this, saveChoice] {
        if (!saveChoice("preferences/autoDisplay", true)) return;
        preferences_.autoDisplay = true;
        applyDisplayProfile();
    });
    addCommand("display_srgb", [this] { return tr("Display profile: sRGB"); }, {}, [this, saveChoice] {
        if (!saveChoice("preferences/autoDisplay", false)) return;
        preferences_.autoDisplay = false;
        applyDisplayProfile();
    });
    addCommand("reload_shortcuts", [this] { return tr("Reload keyboard shortcuts"); }, {}, [this] { reloadShortcuts(); });
    const auto cull = [this](commands::Metadata fields) {
        if (!files_->selectedItems().isEmpty()) batchMetadata(fields);
        else if (open_) { emit request(fields); }
    };
    for (int rating = 0; rating <= 5; ++rating)
        addCommand("rate_" + QString::number(rating), [=, this] { return tr("Set rating: %1 stars").arg(rating); },
                   QString::number(rating), [cull, rating] { cull(commands::Metadata{.rating = rating}); });
    addCommand("pick_photo", [=, this] { return tr("Pick selected photos"); }, "P", [cull] { cull(commands::Metadata{.flag = "picked"}); });
    addCommand("reject_photo", [=, this] { return tr("Reject selected photos"); }, "Shift+X", [cull] { cull(commands::Metadata{.flag = "rejected"}); });
    addCommand("clear_flag", [=, this] { return tr("Clear selected flags"); }, "U", [cull] { cull(commands::Metadata{.flag = "none"}); });
    const QList<QPair<QString, const char*>> labels{{"red", QT_TR_NOOP("Red")}, {"yellow", QT_TR_NOOP("Yellow")},
        {"green", QT_TR_NOOP("Green")}, {"blue", QT_TR_NOOP("Blue")}, {"purple", QT_TR_NOOP("Purple")}};
    for (int i = 0; i < labels.size(); ++i) {
        const auto value = labels[i].first;
        addCommand("label_" + value, [=, this] { return tr("Set color label: %1").arg(tr(labels[i].second)); },
                   i < 4 ? QString::number(6 + i) : QString{}, [cull, value] { cull(commands::Metadata{.label = value}); });
    }
    addCommand("clear_label", [=, this] { return tr("Clear color labels"); }, {}, [cull] { cull(commands::Metadata{.label = QJsonValue::Null}); });
    addCommand("select_all", [=, this] { return tr("Select all visible photos"); }, "Ctrl+A", [this] {
        {
            const QSignalBlocker blocker(files_);
            for (int i = 0; i < files_->count(); ++i) files_->item(i)->setSelected(!files_->item(i)->isHidden());
        }
        updateSelection();
    });
    addCommand("open_selected", [=, this] { return tr("Open selected photo"); }, "Return", [this] {
        if (const auto* item = files_->currentItem())
            open(item->data(Qt::UserRole).toString(), true, item->data(PhotoDelegate::CopyRole).toLongLong());
    });
    const auto navigate = [this](int direction, bool vertical) {
        QList<QListWidgetItem*> visible;
        for (int i = 0; i < files_->count(); ++i)
            if (!files_->item(i)->isHidden()) visible.append(files_->item(i));
        if (visible.isEmpty()) return;
        const int columns = developMode_ ? 1 : std::max(1, files_->viewport()->width() / std::max(1, files_->gridSize().width()));
        const int current = int(visible.indexOf(files_->currentItem()));
        const int next = current < 0 ? 0 : std::clamp(current + direction * (vertical ? columns : 1), 0, int(visible.size()) - 1);
        auto* item = visible[next];
        files_->setCurrentItem(item, QItemSelectionModel::ClearAndSelect);
        files_->scrollToItem(item);
        open(item->data(Qt::UserRole).toString(), developMode_, item->data(PhotoDelegate::CopyRole).toLongLong());
    };
    addCommand("navigate_left", [=, this] { return tr("Previous photo"); }, "Left", [navigate] { navigate(-1, false); });
    addCommand("navigate_right", [=, this] { return tr("Next photo"); }, "Right", [navigate] { navigate(1, false); });
    addCommand("navigate_up", [=, this] { return tr("Previous grid row"); }, "Up", [navigate] { navigate(-1, true); });
    addCommand("navigate_down", [=, this] { return tr("Next grid row"); }, "Down", [navigate] { navigate(1, true); });
    addCommand("toggle_monochrome", [=, this] { return tr("Toggle black and white"); }, "V", [this] {
        if (!open_) return;
        emit request(commands::Edit{.label = tr("Toggle black and white"), .ops = QList<EditOperation>{EditOperation{EditOperation::Kind::Set, "bw.monochrome", !params_["bw"].toObject()["monochrome"].toBool()}}});
    });
    addCommand("toggle_zoom", [=, this] { return tr("Toggle fit / actual pixels"); }, "Z", [this] { canvas_->toggleZoom(); });
    addCommand("show_masks", [=, this] { return tr("Show masks inspector"); }, "M", [this] { showInspector("masks"); });
    addCommand("show_information", [=, this] { return tr("Show photo information"); }, "I", [this] { showInspector("edit"); adjustments_->showPage(3); });
    addCommand("toggle_mask_overlay", [=, this] { return tr("Toggle mask overlay"); }, "O", [this] { toolsPanel_->toggleOverlay(); });
    const auto contextCommand = [this](const QString& id, const char* name, const QString& context, const QString& binding) {
        auto* action = new QAction(tr(name), this);
        bindTranslation(action, [action, name] { action->setText(Window::tr(name)); });
        action->setProperty("commandId", id);
        action->setProperty("shortcutContextName", context);
        action->setProperty("defaultShortcut", binding);
        action->setShortcut(QKeySequence(binding));
        shortcutActions_.append(action);
    };
    contextCommand("tool_cancel", QT_TR_NOOP("Tool: cancel"), "canvas", "Esc");
    contextCommand("crop_accept", QT_TR_NOOP("Crop: apply"), "crop", "Return");
    contextCommand("crop_swap_aspect", QT_TR_NOOP("Crop: swap aspect ratio"), "crop", "X");
    contextCommand("retouch_delete", QT_TR_NOOP("Retouch: delete selected correction"), "retouch", "Delete");
    contextCommand("retouch_delete_alternate", QT_TR_NOOP("Retouch: alternate delete binding"), "retouch", "Backspace");
    contextCommand("dialog_accept", QT_TR_NOOP("Dialog: confirm"), "dialog", "Return");
    contextCommand("dialog_cancel", QT_TR_NOOP("Dialog: cancel"), "dialog", "Esc");
    contextCommand("comparison_fit", QT_TR_NOOP("Comparison: fit all"), "comparison", "F");
    contextCommand("comparison_pixels", QT_TR_NOOP("Comparison: actual pixels"), "comparison", "1");
    const QMap<QString, QString> defaults{{"actual_pixels", "Ctrl+1"}, {"crop", "R"},
        {"white_balance_picker", "W"}, {"heal_spot", "Q"}, {"red_eye", "E"},
        {"enable_soft_proof", "S"}, {"gamut_warning", "Shift+S"}, {"history", "H"},
        {"metadata_sidebar", "Shift+I"}, {"auto_tone", "Ctrl+U"},
        {"rename_selected_photo", "F2"}, {"move_selected_photos_to_trash", "Ctrl+Delete"}};
    for (auto* action : shortcutActions_) {
        const auto id = action->property("commandId").toString();
        if (defaults.contains(id)) {
            action->setProperty("defaultShortcut", defaults[id]);
            action->setShortcut(QKeySequence(defaults[id]));
        }
    }
    reloadShortcuts();
    qApp->installEventFilter(this);
}
void Window::reloadShortcuts() {
    settings_.sync();
    // Saved choices take precedence over newly introduced defaults. Resolve only
    // conflicting bindings; never reset the user's entire keymap on upgrade.
    QList<QAction*> ordered = shortcutActions_;
    std::stable_sort(ordered.begin(), ordered.end(), [this](auto* a, auto* b) {
        return settings_.contains("shortcuts/" + a->property("commandId").toString()) &&
               !settings_.contains("shortcuts/" + b->property("commandId").toString());
    });
    QList<QAction*> assigned;
    QStringList conflicts;
    for (auto* action : ordered) {
        const auto id = action->property("commandId").toString();
        auto sequence = QKeySequence::fromString(settings_.value("shortcuts/" + id,
            action->property("defaultShortcut")).toString(), QKeySequence::PortableText);
        for (auto* previous : assigned) {
            const auto a = previous->property("shortcutContextName").toString();
            const auto b = action->property("shortcutContextName").toString();
            const bool shared = shortcutContextsOverlap(a, b);
            if (shared && shortcutConflict(previous->shortcut(), sequence)) {
                conflicts.append(action->text());
                sequence = {};
                break;
            }
        }
        action->setShortcut(sequence);
        assigned.append(action);
    }
    if (!conflicts.isEmpty())
        QTimer::singleShot(0, this, [this, conflicts] {
            statusBar()->showMessage(tr("Conflicting shortcuts left unbound: %1. Review Keyboard shortcuts.").arg(conflicts.join(", ")), 15000);
        });
    QVariantMap active;
    for (auto* action : shortcutActions_)
        active[action->property("commandId").toString()] = action->shortcut().toString(QKeySequence::PortableText);
    qApp->setProperty("aigleEffectiveShortcuts", active);
}
bool Window::shortcutOverride(QObject* watched, QEvent* event) {
    if (event->type() == QEvent::Show)
        if (auto* dialog = qobject_cast<QDialog*>(watched)) installDialogShortcuts(dialog);
    if (event->type() == QEvent::ShortcutOverride) {
        auto* key = static_cast<QKeyEvent*>(event);
        auto* widget = qobject_cast<QWidget*>(watched);
        bool text = false;
        for (auto* parent = widget; parent; parent = parent->parentWidget())
            text |=
                qobject_cast<QLineEdit*>(parent) || qobject_cast<QTextEdit*>(parent) ||
                qobject_cast<QPlainTextEdit*>(parent) || qobject_cast<QAbstractSpinBox*>(parent) ||
                qobject_cast<QKeySequenceEdit*>(parent) ||
                (qobject_cast<QComboBox*>(parent) && static_cast<QComboBox*>(parent)->isEditable());
        bool nativeNavigation = false;
        for (auto* parent = widget; parent; parent = parent->parentWidget()) {
            if (parent == files_) break;
            nativeNavigation |= qobject_cast<QComboBox*>(parent) || qobject_cast<QAbstractItemView*>(parent) || qobject_cast<QAbstractSlider*>(parent);
        }
        if (nativeNavigation && key->modifiers() == Qt::NoModifier &&
            (key->key() == Qt::Key_Left || key->key() == Qt::Key_Right || key->key() == Qt::Key_Up || key->key() == Qt::Key_Down || key->key() == Qt::Key_Return)) {
            key->accept(); return true;
        }
        // Let text widgets consume plain keys and their own editing shortcuts.
        if (text &&
            ((key->modifiers() & (Qt::ControlModifier | Qt::AltModifier | Qt::MetaModifier)) == 0 ||
             key->matches(QKeySequence::Undo) || key->matches(QKeySequence::Redo) ||
             key->matches(QKeySequence::Copy) || key->matches(QKeySequence::Paste) ||
             key->matches(QKeySequence::Cut) || key->matches(QKeySequence::SelectAll))) {
            key->accept();
            return true;
        }
    }
    return false;
}
void Window::editShortcuts() {
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Keyboard shortcuts"));
    dialog.resize(620, 600);
    auto* layout = new QVBoxLayout(&dialog);
    auto* hint =
        new QLabel(tr("Select a binding and type a shortcut or multi-key chord. Conflicting and "
                      "prefix bindings must be cleared or reassigned before saving. Text fields "
                      "keep typing and standard editing shortcuts."));
    hint->setWordWrap(true);
    layout->addWidget(hint);
    auto* tree = new QTreeWidget;
    tree->setHeaderLabels({tr("Command"), tr("Shortcut")});
    tree->setRootIsDecorated(false);
    tree->header()->setSectionResizeMode(QHeaderView::Stretch);
    layout->addWidget(tree);
    QList<QKeySequenceEdit*> editors;
    for (auto* action : shortcutActions_) {
        auto title = action->text();
        title.remove('&');
        auto* item = new QTreeWidgetItem(tree, {title});
        auto* editor = new QKeySequenceEdit(action->shortcut());
        editor->setClearButtonEnabled(true);
        editor->setAccessibleName(tr("Shortcut for %1").arg(title));
        tree->setItemWidget(item, 1, editor);
        editors.append(editor);
    }
    auto* conflicts = new QLabel;
    conflicts->setWordWrap(true);
    layout->addWidget(conflicts);
    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Save | QDialogButtonBox::Cancel |
                                         QDialogButtonBox::RestoreDefaults);
    layout->addWidget(buttons);
    const auto validate = [&] {
        QStringList errors;
        for (int i = 0; i < editors.size(); ++i)
            for (int j = i + 1; j < editors.size(); ++j)
                if (const auto a = shortcutActions_[i]->property("shortcutContextName").toString(),
                               b = shortcutActions_[j]->property("shortcutContextName").toString();
                    (shortcutContextsOverlap(a, b)) &&
                    shortcutConflict(editors[i]->keySequence(), editors[j]->keySequence()))
                    errors.append(
                        tr("%1 conflicts with %2")
                            .arg(tree->topLevelItem(i)->text(0), tree->topLevelItem(j)->text(0)));
        conflicts->setText(errors.join('\n'));
        buttons->button(QDialogButtonBox::Save)->setEnabled(errors.isEmpty());
    };
    const auto refreshRows = [&] {
        for (int i = 0; i < editors.size(); ++i) {
            auto title = shortcutActions_[i]->text();
            title.remove('&');
            tree->topLevelItem(i)->setText(0, title);
            editors[i]->setAccessibleName(tr("Shortcut for %1").arg(title));
        }
        validate();
    };
    // Action bindings may refresh after the dialog's language event filter.
    // React to each action's final label as well, without touching staged keys.
    for (auto* action : shortcutActions_)
        connect(action, &QAction::changed, &dialog, refreshRows);
    bindTranslation(&dialog, [&] {
        dialog.setWindowTitle(tr("Keyboard shortcuts"));
        tree->setHeaderLabels({tr("Command"), tr("Shortcut")});
        hint->setText(tr("Select a binding and type a shortcut or multi-key chord. Conflicting and "
                         "prefix bindings must be cleared or reassigned before saving. Text fields "
                         "keep typing and standard editing shortcuts."));
        refreshRows();
    });
    for (auto* editor : editors)
        connect(editor, &QKeySequenceEdit::keySequenceChanged, &dialog, validate);
    connect(buttons->button(QDialogButtonBox::RestoreDefaults), &QPushButton::clicked, &dialog,
            [&] {
                for (int i = 0; i < editors.size(); ++i)
                    editors[i]->setKeySequence(QKeySequence::fromString(
                        shortcutActions_[i]->property("defaultShortcut").toString(),
                        QKeySequence::PortableText));
            });
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, [&] {
        for (int i = 0; i < editors.size(); ++i)
            settings_.setValue("shortcuts/" + shortcutActions_[i]->property("commandId").toString(),
                               editors[i]->keySequence().toString(QKeySequence::PortableText));
        settings_.sync();
        if (settings_.status() != QSettings::NoError) {
            QMessageBox::warning(&dialog, tr("Keyboard shortcuts"),
                                 tr("Could not save shortcuts."));
            return;
        }
        for (int i = 0; i < editors.size(); ++i)
            shortcutActions_[i]->setShortcut(editors[i]->keySequence());
        QVariantMap active;
        for (auto* action : shortcutActions_)
            active[action->property("commandId").toString()] = action->shortcut().toString(QKeySequence::PortableText);
        qApp->setProperty("aigleEffectiveShortcuts", active);
        dialog.accept();
    });
    validate();
    dialog.exec();
}
} // namespace aigle
