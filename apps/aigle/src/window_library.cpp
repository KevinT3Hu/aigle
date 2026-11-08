#include "command_json.hpp"
#include "gui_json.hpp"
#include "commands.hpp"
#include "photo_identity.hpp"
#include "canvas.hpp"
#include "photo_delegate.hpp"
#include "window.hpp"
#include "localization.hpp"
#include <QApplication>
#include <QCheckBox>
#include <QClipboard>
#include <QDialog>
#include <QDialogButtonBox>
#include <QDir>
#include <QFileDialog>
#include <QFileInfo>
#include <QGridLayout>
#include <QInputDialog>
#include <QJsonDocument>
#include <QMessageBox>
#include <QMimeData>
#include <QProgressDialog>
#include <QPushButton>
#include <QSet>
#include <QStatusBar>
#include <QTreeWidget>
#include <QUuid>
#include <QVBoxLayout>

namespace aigle {
void Window::transferAdjustments(bool sync) {
    if (!open_)
        return;
    const auto source = params_;
    const auto sourcePath = path_;
    QList<PhotoIdentity> targets;
    QStringList targetDescriptions;
    if (sync) {
        for (auto* item : files_->selectedItems()) {
            const auto path = item->data(Qt::UserRole).toString();
            if (path != sourcePath || item->data(PhotoDelegate::CopyRole).toLongLong() != state_.copy.value_or(0)) {
                targets.append(photoIdentity(item));
                targetDescriptions.append(item->text() + "\n" + path);
            }
        }
        if (targets.isEmpty()) {
            statusBar()->showMessage(
                tr("Select target photos in addition to the open source photo"), 6000);
            return;
        }
    }
    QDialog dialog(this);
    dialog.setWindowTitle(sync ? tr("Sync selected adjustments") : tr("Copy selected adjustments"));
    auto* layout = new QVBoxLayout(&dialog);
    auto* explanation = new QLabel(
        sync ? tr("Source: %1%2\nApply checked groups to %3 selected photos. Unchecked "
                  "groups and metadata are preserved.")
                   .arg(QFileInfo(sourcePath).fileName())
                   .arg(!state_.copy.has_value() ? QString{} : tr(" (open virtual copy)"))
                   .arg(targets.size())
             : tr("Choose the adjustment groups to copy. Paste applies them as one undo step."));
    explanation->setWordWrap(true);
    layout->addWidget(explanation);
    auto* grid = new QGridLayout;
    layout->addLayout(grid);
    const QList<QPair<QString, const char*>> groups{{"profile", QT_TR_NOOP("Profile")},
                                                {"wb", QT_TR_NOOP("White balance")},
                                                {"light", QT_TR_NOOP("Light")},
                                                {"hdr", QT_TR_NOOP("HDR")},
                                                {"presence", QT_TR_NOOP("Presence")},
                                                {"curve", QT_TR_NOOP("Curves")},
                                                {"mixer", QT_TR_NOOP("Color mixer")},
                                                {"bw", QT_TR_NOOP("Monochrome")},
                                                {"grading", QT_TR_NOOP("Color grading")},
                                                {"detail", QT_TR_NOOP("Detail")},
                                                {"lens", QT_TR_NOOP("Lens")},
                                                {"effects", QT_TR_NOOP("Effects")},
                                                {"raw", QT_TR_NOOP("RAW")},
                                                {"geometry", QT_TR_NOOP("Crop and geometry")},
                                                {"masks", QT_TR_NOOP("Masks")},
                                                {"spots", QT_TR_NOOP("Heal / clone")},
                                                {"red_eye", QT_TR_NOOP("Red eye")}};
    QList<QCheckBox*> checks;
    for (int i = 0; i < groups.size(); ++i) {
        auto* check = new QCheckBox;
        const auto sourceLabel = groups[i].second;
        bindTranslation(check, [check, sourceLabel] { check->setText(tr(sourceLabel)); });
        check->setChecked(i < 13);
        check->setProperty("group", groups[i].first);
        grid->addWidget(check, i / 2, i % 2);
        checks.append(check);
    }
    auto* note = new QLabel(
        tr("Selected groups replace the target's entire group, including lists. Crop, masks, and "
           "retouching are unchecked by default because they depend on image position."));
    note->setWordWrap(true);
    layout->addWidget(note);
    if (sync) {
        auto* details = new QPushButton;
        details->setObjectName("syncReviewTargets");
        bindTranslation(details, [details] { details->setText(tr("Review target photos…")); });
        layout->addWidget(details);
        connect(details, &QPushButton::clicked, &dialog, [&] {
            QMessageBox::information(&dialog, tr("Sync targets"), targetDescriptions.join("\n\n"));
        });
    }
    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Ok | QDialogButtonBox::Cancel);
    layout->addWidget(buttons);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    const auto enable = [=] {
        buttons->button(QDialogButtonBox::Ok)
            ->setEnabled(std::any_of(checks.begin(), checks.end(),
                                     [](auto* check) { return check->isChecked(); }));
    };
    for (auto* check : checks)
        connect(check, &QCheckBox::toggled, &dialog, enable);
    const bool sourceIsCopy = !!state_.copy.has_value();
    bindTranslation(&dialog, [&, sourceIsCopy] {
        dialog.setWindowTitle(sync ? tr("Sync selected adjustments") : tr("Copy selected adjustments"));
        explanation->setText(sync
            ? tr("Source: %1%2\nApply checked groups to %3 selected photos. Unchecked "
                 "groups and metadata are preserved.")
                .arg(QFileInfo(sourcePath).fileName())
                .arg(sourceIsCopy ? tr(" (open virtual copy)") : QString{})
                .arg(targets.size())
            : tr("Choose the adjustment groups to copy. Paste applies them as one undo step."));
        note->setText(tr("Selected groups replace the target's entire group, including lists. Crop, masks, and "
                         "retouching are unchecked by default because they depend on image position."));
    });
    if (dialog.exec() != QDialog::Accepted)
        return;
    QStringList selected;
    for (auto* check : checks)
        if (check->isChecked())
            selected.append(check->property("group").toString());
    QJsonObject payload{{"params", source}, {"groups", QJsonArray::fromStringList(selected)}};
    if (sync) {
        emit request(commands::SyncSelected{.params = source, .groups = selected, .targets = targets});
    } else {
        auto* mime = new QMimeData;
        mime->setData("application/x-aigle-selected-edits",
                      QJsonDocument(payload).toJson(QJsonDocument::Compact));
        QApplication::clipboard()->setMimeData(mime);
        statusBar()->showMessage(tr("Copied %1 adjustment groups").arg(selected.size()), 5000);
    }
}
void Window::stackSelection(bool remove) {
    const auto selected = files_->selectedItems();
    if (selected.isEmpty() || (!remove && selected.size() < 2)) {
        statusBar()->showMessage(tr("Select at least two photos to stack, or one stack to unstack"),
                                 5000);
        return;
    }
    QSet<QString> stacks;
    for (int i = 0; i < files_->count(); ++i) {
        const auto meta = files_->item(i)->data(PhotoDelegate::MetadataRole).toJsonObject();
        if (!meta["meta_loaded"].toBool() &&
            QFileInfo::exists(files_->item(i)->data(Qt::UserRole).toString())) {
            statusBar()->showMessage(tr("Wait for photo metadata to load before editing stacks"),
                                     5000);
            return;
        }
    }
    for (auto* item : selected) {
        const auto id = item->data(PhotoDelegate::MetadataRole).toJsonObject()["stack"].toString();
        if (!id.isEmpty())
            stacks.insert(id);
    }
    QList<PhotoIdentity> paths;
    for (int i = 0; i < files_->count(); ++i) {
        auto* item = files_->item(i);
        const auto id = item->data(PhotoDelegate::MetadataRole).toJsonObject()["stack"].toString();
        if (remove ? stacks.contains(id) : item->isSelected())
            paths.append(photoIdentity(item));
    }
    if (!paths.isEmpty())
        emit request(
            commands::BatchMetadata{.targets = paths, .stack = remove ? QJsonValue(QJsonValue::Null)
                              : QJsonValue(QUuid::createUuid().toString(QUuid::WithoutBraces))});
}
void Window::findDuplicates() {
    if (duplicateProgress_)
        return;
    QStringList paths;
    // Scan every member of the current folder/album, including filtered/stacked photos.
    for (int i = 0; i < files_->count(); ++i)
        if (!files_->item(i)->data(PhotoDelegate::CopyRole).toLongLong())
            paths.append(files_->item(i)->data(Qt::UserRole).toString());
    if (paths.size() < 2) {
        statusBar()->showMessage(tr("Open a folder or album with at least two photos"), 5000);
        return;
    }
    duplicateProgress_ =
        new QProgressDialog(tr("Scanning files for exact duplicates…"), tr("Cancel"), 0, 0, this);
    auto* progress = duplicateProgress_;
    bindTranslation(progress, [progress] {
        progress->setLabelText(tr("Scanning files for exact duplicates…"));
        progress->setCancelButtonText(tr("Cancel"));
    });
    duplicateProgress_->setWindowModality(Qt::WindowModal);
    connect(duplicateProgress_, &QProgressDialog::canceled, this,
            [this] { emit request(commands::CancelDuplicates{}); });
    duplicateProgress_->show();
    emit request(commands::FindDuplicates{.paths = paths, .id = ++duplicateId_});
}
void Window::updateDuplicates(const DuplicateResult& state) {
    if (!duplicateProgress_ || state.id != duplicateId_ ||
        state.running)
        return;
    auto* progress = duplicateProgress_;
    duplicateProgress_ = nullptr;
    progress->close();
    progress->deleteLater();
    if (state.cancelled) {
        statusBar()->showMessage(tr("Duplicate scan cancelled"), 5000);
        return;
    }
    if (state.error.has_value()) {
        QMessageBox::warning(this, tr("Duplicate scan"), *state.error);
        return;
    }
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Exact duplicates"));
    dialog.resize(680, 420);
    auto* layout = new QVBoxLayout(&dialog);
    auto* summary = new QLabel(
        tr("%1 duplicate groups. Select a group to review its photos. No files are deleted.")
            .arg(state.groups.size()));
    summary->setWordWrap(true);
    layout->addWidget(summary);
    auto* tree = new QTreeWidget;
    tree->setHeaderLabel(tr("Files with identical contents"));
    int groupNumber = 0;
    for (const auto& value : state.groups) {
        const auto& paths = value;
        auto* group = new QTreeWidgetItem(
            tree, {tr("Group %1 — %2 photos").arg(++groupNumber).arg(paths.size())});
        group->setData(0, Qt::UserRole, paths);
        for (const auto& path : paths) {
            auto* child = new QTreeWidgetItem(group, {path});
            child->setData(0, Qt::UserRole, paths);
        }
    }
    tree->expandAll();
    layout->addWidget(tree);
    QStringList failures;
    for (const auto& failure : state.failures) {
        failures.append(failure.first + ": " + failure.second);
    }
    if (!failures.isEmpty()) {
        auto* errors =
            new QLabel(tr("%1 files could not be scanned. See details.").arg(failures.size()));
        bindTranslation(errors, [errors, count = failures.size()] {
            errors->setText(tr("%1 files could not be scanned. See details.").arg(count));
        });
        layout->addWidget(errors);
        auto* details = new QPushButton;
        bindTranslation(details, [details] { details->setText(tr("Failure details…")); });
        layout->addWidget(details);
        connect(details, &QPushButton::clicked, &dialog,
                [&] { QMessageBox::warning(&dialog, tr("Scan failures"), failures.join('\n')); });
    }
    auto* buttons = new QDialogButtonBox(QDialogButtonBox::Close);
    auto* select = buttons->addButton(tr("Select group"), QDialogButtonBox::AcceptRole);
    select->setEnabled(false);
    connect(tree, &QTreeWidget::itemSelectionChanged, &dialog,
            [=] { select->setEnabled(tree->currentItem() != nullptr); });
    connect(buttons, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    connect(buttons, &QDialogButtonBox::accepted, &dialog, &QDialog::accept);
    layout->addWidget(buttons);
    dialog.setObjectName("duplicateReview");
    bindTranslation(&dialog, [&, count = state.groups.size()] {
        dialog.setWindowTitle(tr("Exact duplicates"));
        summary->setText(tr("%1 duplicate groups. Select a group to review its photos. No files are deleted.").arg(count));
        tree->setHeaderLabel(tr("Files with identical contents"));
        select->setText(tr("Select group"));
        for (int i = 0; i < tree->topLevelItemCount(); ++i) {
            auto* group = tree->topLevelItem(i);
            group->setText(0, tr("Group %1 — %2 photos").arg(i + 1).arg(group->childCount()));
        }
    });
    if (dialog.exec() == QDialog::Accepted && tree->currentItem()) {
        const auto paths = tree->currentItem()->data(0, Qt::UserRole).toStringList();
        // Keep the user's filters; explain if they hide some results.
        files_->clearSelection();
        int hidden = 0;
        for (int i = 0; i < files_->count(); ++i) {
            auto* item = files_->item(i);
            if (!item->data(PhotoDelegate::CopyRole).toLongLong() && paths.contains(item->data(Qt::UserRole).toString())) {
                if (item->isHidden())
                    ++hidden;
                else
                    item->setSelected(true);
            }
        }
        if (hidden)
            statusBar()->showMessage(
                tr("%1 duplicate photos are hidden by filters or collapsed stacks").arg(hidden),
                8000);
        setMode(false);
    }
}
void Window::startFileActions(const QString& action) {
    if (fileProgress_)
        return;
    const auto selected = files_->selectedItems();
    if (selected.isEmpty())
        return;
    if (exportQueue_.hasPending() || state_.exporting.running) {
        QMessageBox::information(this, tr("File actions"),
                                 tr("Finish or cancel pending exports first."));
        return;
    }
    if ((action == "rename" || action == "relink") && selected.size() != 1) {
        statusBar()->showMessage(tr("Select one photo for this action"), 5000);
        return;
    }
    QString destination;
    const auto first = selected.first()->data(Qt::UserRole).toString();
    const bool renameCopy = action == "rename" && selected.first()->data(PhotoDelegate::CopyRole).toLongLong();
    QSet<QString> originals;
    int copies = 0;
    for (auto* item : selected)
        if (!item->data(PhotoDelegate::CopyRole).toLongLong()) originals.insert(item->data(Qt::UserRole).toString());
    for (auto* item : selected)
        if (item->data(PhotoDelegate::CopyRole).toLongLong() && !originals.contains(item->data(Qt::UserRole).toString())) ++copies;
    if (action == "move") {
        destination = QFileDialog::getExistingDirectory(this, tr("Move photos to folder"), folder_);
    } else if (renameCopy) {
        destination = QInputDialog::getText(this, tr("Rename virtual copy"), tr("Name")).trimmed();
    } else if (action == "rename") {
        bool ok = false;
        const auto stem =
            QInputDialog::getText(this, tr("Rename photo"), tr("File name without extension"),
                                  QLineEdit::Normal, QFileInfo(first).completeBaseName(), &ok)
                .trimmed();
        if (!ok)
            return;
        if (stem.isEmpty() || stem == "." || stem == ".." || stem.contains('/') ||
            stem.contains('\\')) {
            QMessageBox::warning(this, tr("Rename photo"),
                                 tr("Enter a file name without folder separators."));
            return;
        }
        destination = QFileInfo(first).dir().filePath(stem + '.' + QFileInfo(first).suffix());
    } else if (action == "relink") {
        if (QFileInfo::exists(first)) {
            QMessageBox::information(this, tr("Locate original"),
                                     tr("This original is not missing."));
            return;
        }
        destination = QFileDialog::getOpenFileName(this, tr("Locate missing original"),
                                                   QFileInfo(first).absolutePath());
    } else if (QMessageBox::question(
                   this, tr("Move to Trash"),
                   tr("Move %1 original photos and their sidecars to Trash, and delete %2 selected virtual copies? Copies of trashed originals follow their originals. Other original photos are preserved.")
                       .arg(originals.size()).arg(copies),
                   QMessageBox::Yes | QMessageBox::Cancel, QMessageBox::Cancel) != QMessageBox::Yes)
        return;
    if (action != "trash" && destination.isEmpty())
        return;
    QSet<QString> queuedPaths;
    for (auto* item : selected) {
        const auto path = item->data(Qt::UserRole).toString();
        const auto copy = item->data(PhotoDelegate::CopyRole).toLongLong();
        const bool removeCopy = action == "trash" && copy;
        if (removeCopy && originals.contains(path)) continue;
        if (!removeCopy && !renameCopy && queuedPaths.contains(path)) continue;
        queuedPaths.insert(path);
        fileActions_.enqueue(
            commands::FileAction{.action = renameCopy ? "rename_copy" : removeCopy ? "remove_copy" : action == "rename" ? "move" : action, .path = path, .copy = copy ? QJsonValue(copy) : QJsonValue(QJsonValue::Null), .destination = action == "move"
                                 ? QDir(destination).filePath(QFileInfo(path).fileName())
                                 : destination});
    }
    if (action != "trash" && !renameCopy) {
        QStringList plan;
        for (const auto& entry : fileActions_)
            plan.append(entry.path.value_or(QString{}) + " → " + entry.destination.value_or(QString{}));
        QMessageBox review(QMessageBox::Question, tr("Review file changes"),
                           tr("Update %1 photos and their associated sidecars? Catalog history, "
                              "virtual copies, and album membership follow the new paths. Existing "
                              "destination edits will not be overwritten.")
                               .arg(fileActions_.size()),
                           QMessageBox::Ok | QMessageBox::Cancel, this);
        review.setDetailedText(plan.join('\n'));
        review.setDefaultButton(QMessageBox::Cancel);
        if (review.exec() != QMessageBox::Ok) {
            fileActions_.clear();
            return;
        }
    }
    fileCompleted_ = 0;
    fileErrors_.clear();
    fileProgress_ = new QProgressDialog(tr("Updating photos and sidecars…"), tr("Cancel remaining"),
                                        0, int(fileActions_.size()), this);
    fileProgress_->setWindowModality(Qt::WindowModal);
    fileProgress_->setAutoClose(false);
    connect(fileProgress_, &QProgressDialog::canceled, this, [this] { fileActions_.clear(); });
    fileProgress_->show();
    nextFileAction();
}
void Window::nextFileAction() {
    if (!fileActions_.isEmpty()) {
        const auto request = fileActions_.dequeue();
        fileProgress_->setLabelText(tr("Updating %1\nCancel stops after the current photo.")
                                        .arg(QFileInfo(request.path.value_or(QString{})).fileName()));
        emit this->request(request);
        return;
    }
    auto* progress = fileProgress_;
    fileProgress_ = nullptr;
    progress->close();
    progress->deleteLater();
    if (activeAlbumId_)
        emit request(commands::AlbumPaths{.id = activeAlbumId_});
    else if (!folder_.isEmpty())
        openFolder(folder_);
    if (!fileErrors_.isEmpty())
        QMessageBox::warning(this, tr("File action failures"), fileErrors_.join('\n'));
    else
        statusBar()->showMessage(tr("Processed %1 photos").arg(fileCompleted_), 5000);
}
void Window::finishFileAction(const FileActionResult& result) {
    if (!fileProgress_)
        return;
    if (result.error.has_value())
        fileErrors_.append(result.photo.path + ": " + *result.error);
    if (result.reopenError.has_value())
        fileErrors_.append(*result.reopenError);
    if (result.ok && result.action == "remove_copy") {
        for (int i = files_->count() - 1; i >= 0; --i)
            if (samePhoto(files_->item(i), result.photo)) delete files_->takeItem(i);
    } else if (result.ok && result.action == "rename_copy") {
        emit request(commands::LibraryCopies{.paths = QStringList{result.photo.path}});
    }
    if (result.opened.has_value()) {
        auto opened = *result.opened;
        opened.develop = developMode_;
        receive({"open", opened});
    } else if (result.closed) {
        path_.clear();
        open_ = false;
        autosave_.stop();
        viewportTimer_.stop();
        pendingViewport_ = {};
        libraryPreview_->clear();
        libraryInfo_->clear();
        setMode(false);
        canvas_->setFrame({}, false);
        canvas_->setFrame({}, true);
    }
    fileProgress_->setValue(++fileCompleted_);
    nextFileAction();
}
} // namespace aigle
