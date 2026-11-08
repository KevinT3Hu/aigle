#include "command_json.hpp"
#include "commands.hpp"
#include "window.hpp"
#include <QDialog>
#include <QDialogButtonBox>
#include <QFileDialog>
#include <QFileInfo>
#include <QHeaderView>
#include <QMessageBox>
#include <QPushButton>
#include <QStatusBar>
#include <QToolButton>
#include <QTreeWidget>
#include <QVBoxLayout>

namespace aigle {
bool Window::checkExportQueue() {
    if (exportQueue_.error.isEmpty())
        return true;
    if (!queueErrorReported_) {
        queueErrorReported_ = true;
        showError(exportQueue_.error);
    }
    return false;
}
void Window::advanceExportQueue() {
    if (!engineReady_ || closing_ || !checkExportQueue() || fileProgress_)
        return;
    const auto job = exportQueue_.next();
    if (!checkExportQueue())
        return;
    if (job) emit request(*job);
}
void Window::cancelExports() {
    exportQueue_.cancelPending();
    checkExportQueue();
    if (!exportQueue_.runningId().isEmpty())
        emit request(commands::CancelExport{});
}
void Window::observeExport(const ExportProgress& state) {
    if (exportQueue_.finish(state)) {
        if (state.error.has_value())
            statusBar()->showMessage(tr("Export failed: %1").arg(*state.error), 10000);
        advanceExportQueue();
    }
    int pending = 0;
    for (const auto& job : exportQueue_.jobs)
        pending += job.status == ExportStatus::Pending;
    queueButton_->setText(tr("Jobs · %1%2")
                              .arg(pending + !exportQueue_.runningId().isEmpty())
                              .arg(exportQueue_.paused && pending ? tr(" paused") : QString{}));
    // A persistence failure pauses dispatch and is surfaced once, not on every poll.
    if (!exportQueue_.error.isEmpty() && lastError_ != exportQueue_.error)
        checkExportQueue();
}
void Window::showExportQueue() {
    QDialog dialog(this);
    dialog.setWindowTitle(tr("Export jobs"));
    dialog.resize(880, 440);
    auto* layout = new QVBoxLayout(&dialog);
    auto* summary = new QLabel;
    summary->setWordWrap(true);
    layout->addWidget(summary);
    auto* tree = new QTreeWidget;
    tree->setHeaderLabels({tr("Source"), tr("Destination"), tr("Status"), tr("Details")});
    tree->setRootIsDecorated(false);
    tree->setStyleSheet("QTreeView::item { min-height: 26px; }");
    tree->header()->setSectionResizeMode(QHeaderView::ResizeToContents);
    tree->header()->setStretchLastSection(true);
    layout->addWidget(tree);
    auto* actions = new QHBoxLayout;
    layout->addLayout(actions);
    auto* pause = new QPushButton(tr("Pause after current"));
    auto* resume = new QPushButton(tr("Resume"));
    auto* retry = new QPushButton(tr("Retry selected"));
    auto* retryAs = new QPushButton(tr("Retry to new file…"));
    auto* cancel = new QPushButton(tr("Cancel all"));
    auto* clear = new QPushButton(tr("Clear finished"));
    for (auto* button : {pause, resume, retry, retryAs, cancel, clear})
        actions->addWidget(button);
    auto* close = new QDialogButtonBox(QDialogButtonBox::Close);
    layout->addWidget(close);
    connect(close, &QDialogButtonBox::rejected, &dialog, &QDialog::reject);
    QList<ExportJob> previous;
    const auto refresh = [&] {
        summary->setText(!exportQueue_.error.isEmpty() ? exportQueue_.error
                         : exportQueue_.paused
                             ? tr("Paused. Recovered jobs wait for Resume; interrupted jobs "
                                  "require Retry. Existing files are never overwritten.")
                             : tr("Processing one photo at a time. Pause holds remaining jobs; the "
                                  "current photo finishes."));
        if (previous != exportQueue_.jobs) {
            const auto selected = tree->currentItem()
                                      ? tree->currentItem()->data(0, Qt::UserRole).toString()
                                      : QString{};
            tree->clear();
            for (const auto& value : exportQueue_.jobs) {
                const auto& job = value;
                const auto& request = job.request;
                const auto source =
                    request.source_ref.value_or(QJsonValue{}).toObject()["File"].toString(tr("Generated image"));
                const auto code = exportStatusName(job.status);
                const auto status = code == "pending"       ? tr("Pending")
                                    : code == "running"     ? tr("Exporting")
                                    : code == "done"        ? tr("Complete")
                                    : code == "failed"      ? tr("Failed")
                                    : code == "interrupted" ? tr("Interrupted")
                                                            : tr("Cancelled");
                const auto destination = request.options.value_or(ExportOptions{}).path;
                auto* item = new QTreeWidgetItem(tree, {request.source_name.value_or(QFileInfo(source).fileName()),
                                                        QFileInfo(destination).fileName(), status,
                                                        job.error.value_or(QString{})});
                item->setToolTip(0, source);
                item->setToolTip(1, destination);
                item->setData(1, Qt::UserRole, destination);
                item->setData(0, Qt::UserRole, job.id);
                item->setData(0, Qt::UserRole + 1, code);
                item->setToolTip(3, job.error.value_or(QString{}));
                if (job.id == selected)
                    tree->setCurrentItem(item);
            }
            previous = exportQueue_.jobs;
        }
        const bool canRetry =
            tree->currentItem() && QStringList{"failed", "cancelled", "interrupted"}.contains(
                                       tree->currentItem()->data(0, Qt::UserRole + 1).toString());
        retry->setEnabled(canRetry && exportQueue_.error.isEmpty());
        retryAs->setEnabled(canRetry && exportQueue_.error.isEmpty());
        resume->setEnabled(engineReady_ && exportQueue_.paused && exportQueue_.hasPending() &&
                           exportQueue_.error.isEmpty());
        pause->setEnabled(!exportQueue_.paused);
    };
    connect(tree, &QTreeWidget::itemSelectionChanged, &dialog, [&] {
        const bool eligible =
            tree->currentItem() && QStringList{"failed", "cancelled", "interrupted"}.contains(
                                       tree->currentItem()->data(0, Qt::UserRole + 1).toString());
        retry->setEnabled(eligible && exportQueue_.error.isEmpty());
        retryAs->setEnabled(eligible && exportQueue_.error.isEmpty());
    });
    connect(pause, &QPushButton::clicked, &dialog, [&] {
        exportQueue_.paused = true;
        refresh();
    });
    connect(resume, &QPushButton::clicked, &dialog, [&] {
        exportQueue_.paused = false;
        advanceExportQueue();
        refresh();
    });
    const auto retrySelected = [&](bool newFile) {
        if (!tree->currentItem())
            return;
        const auto id = tree->currentItem()->data(0, Qt::UserRole).toString();
        QString destination;
        if (newFile) {
            destination =
                QFileDialog::getSaveFileName(&dialog, tr("Retry export to new file"),
                                             tree->currentItem()->data(1, Qt::UserRole).toString());
            if (destination.isEmpty())
                return;
            if (QFileInfo::exists(destination)) {
                QMessageBox::warning(
                    &dialog, tr("Export destination"),
                    tr("Choose a new filename. Existing files will not be overwritten."));
                return;
            }
        }
        exportQueue_.retry(id, destination);
        checkExportQueue();
        advanceExportQueue();
        refresh();
    };
    connect(retry, &QPushButton::clicked, &dialog, [&] { retrySelected(false); });
    connect(retryAs, &QPushButton::clicked, &dialog, [&] { retrySelected(true); });
    connect(cancel, &QPushButton::clicked, &dialog, [&] {
        cancelExports();
        refresh();
    });
    connect(clear, &QPushButton::clicked, &dialog, [&] {
        exportQueue_.clearFinished();
        checkExportQueue();
        refresh();
    });
    QTimer timer;
    connect(&timer, &QTimer::timeout, &dialog, refresh);
    timer.start(250);
    refresh();
    dialog.exec();
}
} // namespace aigle
