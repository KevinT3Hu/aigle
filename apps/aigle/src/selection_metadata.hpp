#pragma once
#include "commands.hpp"
#include "gui_json.hpp"
#include "localization.hpp"
#include <QCoreApplication>
#include <QJsonArray>
#include <QJsonObject>
#include <QLabel>
#include <QLineEdit>
#include <QRegularExpression>
#include <QSet>
#include <QVBoxLayout>
#include <functional>

namespace aigle {
class SelectionMetadata final : public QWidget {
    Q_DECLARE_TR_FUNCTIONS(aigle::SelectionMetadata)
    struct Row { QString field, shown; QLineEdit* input; bool pending = false; };
    QList<Row> rows_;
    QList<PhotoIdentity> targets_;
    static QStringList keywords(const QString& text) {
        QStringList result;
        QSet<QString> seen;
        for (const auto& token : text.split(QRegularExpression("[,;]"), Qt::SkipEmptyParts)) {
            QStringList levels;
            for (const auto& level : token.split('>', Qt::SkipEmptyParts))
                if (!level.trimmed().isEmpty()) levels.append(level.trimmed());
            const auto value = levels.join(" > ");
            if (!value.isEmpty() && !seen.contains(value.toCaseFolded())) {
                seen.insert(value.toCaseFolded());
                result.append(value);
            }
        }
        return result;
    }
    void commit(Row& row) {
        if (targets_.isEmpty() || !row.input->isModified()) return;
        const auto typed = row.input->text().trimmed();
        row.input->setModified(false);
        if (typed == row.shown && !row.input->property("mixed").toBool()) return;
        auto request = commands::BatchMetadata{.targets = targets_};
        if (row.field.isEmpty()) {
            const auto before = keywords(row.shown), after = keywords(typed);
            QStringList added, removed;
            for (const auto& value : after)
                if (!before.contains(value, Qt::CaseInsensitive)) added.append(value);
            for (const auto& value : before)
                if (!after.contains(value, Qt::CaseInsensitive)) removed.append(value);
            if (added.isEmpty() && removed.isEmpty()) return;
            request.tags = added;
            request.remove_tags = removed;
            request.tag_mode = "patch";
            row.shown = after.join(", ");
        } else {
            request.iptc = QMap<QString, QString>{{row.field, typed}};
            row.shown = typed;
        }
        row.pending = true;
        row.input->setText(row.shown);
        if (command) command(request);
    }
  public:
    std::function<void(const EngineCommand&)> command;
    explicit SelectionMetadata(QWidget* parent = nullptr) : QWidget(parent) {
        auto* layout = new QVBoxLayout(this);
        layout->setContentsMargins(0, 0, 0, 0);
        const QList<QPair<QString, const char*>> fields{
            {"", QT_TR_NOOP("Keywords")}, {"title", QT_TR_NOOP("Title")}, {"caption", QT_TR_NOOP("Caption")},
            {"headline", QT_TR_NOOP("Headline")}, {"creator", QT_TR_NOOP("Creator")},
            {"creator_job_title", QT_TR_NOOP("Creator job title")}, {"copyright", QT_TR_NOOP("Copyright")},
            {"copyright_url", QT_TR_NOOP("Copyright URL")}, {"credit", QT_TR_NOOP("Credit")},
            {"source", QT_TR_NOOP("Source")}, {"instructions", QT_TR_NOOP("Instructions")},
            {"job_id", QT_TR_NOOP("Job ID")}, {"sublocation", QT_TR_NOOP("Sublocation")},
            {"city", QT_TR_NOOP("City")}, {"state", QT_TR_NOOP("State / province")},
            {"country", QT_TR_NOOP("Country")}, {"country_code", QT_TR_NOOP("Country code")}};
        for (const auto& [field, title] : fields) {
            auto* label = new QLabel(tr(title));
            label->setObjectName("parameterLabel");
            auto* input = new QLineEdit;
            input->setObjectName("metadata_" + (field.isEmpty() ? QString("keywords") : field));
            bindTranslation(input, [input, label, title, field] {
                label->setText(tr(title));
                input->setAccessibleName(tr(title));
                input->setPlaceholderText(input->property("metadataLoading").toBool() ? tr("Loading metadata…")
                    : input->property("mixed").toBool() ? tr("Mixed values")
                    : field.isEmpty() ? tr("Comma-separated keywords") : QString{});
                input->setToolTip(field.isEmpty()
                    ? tr("Shared keywords only. Adding or removing a keyword preserves each photo's other keywords.")
                    : tr("Changes apply to all selected photos."));
            });
            label->setBuddy(input);
            layout->addWidget(label);
            layout->addWidget(input);
            const int index = int(rows_.size());
            rows_.append({field, {}, input});
            connect(input, &QLineEdit::editingFinished, this, [this, index] { commit(rows_[index]); });
        }
        setEnabled(false);
    }
    void setSelection(const QList<PhotoIdentity>& targets, const QList<SelectionPhotoMetadata>& metadata) {
        const bool changed = targets != targets_;
        if (changed) {
            for (auto& row : rows_) commit(row); // Commit to the previous selection.
            targets_ = targets;
        }
        bool ready = !targets.isEmpty() && targets.size() == metadata.size();
        for (const auto& meta : metadata) ready &= meta.loaded;
        setEnabled(ready);
        for (auto& row : rows_) {
            if (changed) row.pending = false;
            QString value;
            bool mixed = false;
            if (ready) {
                const auto first = metadata.first();
                if (row.field.isEmpty()) {
                    auto common = first.tags;
                    for (const auto& entry : metadata) {
                        const auto other = entry.tags;
                        common.removeIf([&](const QString& tag) { return !other.contains(tag, Qt::CaseInsensitive); });
                    }
                    for (const auto& entry : metadata) mixed |= entry.tags.size() != common.size();
                    value = common.join(", ");
                } else {
                    value = first.iptc.value(row.field);
                    for (const auto& entry : metadata)
                        mixed |= entry.iptc.value(row.field) != value;
                    if (mixed) value.clear();
                }
            }
            auto equivalent = value == row.shown;
            if (row.field.isEmpty()) {
                auto actual = keywords(value.toCaseFolded()), expected = keywords(row.shown.toCaseFolded());
                actual.sort(); expected.sort();
                equivalent = actual == expected;
            }
            if (!changed && (row.input->isModified() || (row.pending && !equivalent))) continue;
            row.pending = false;
            row.shown = value;
            row.input->setProperty("mixed", mixed);
            row.input->setProperty("metadataLoading", !ready && !targets.isEmpty());
            row.input->setText(value);
            row.input->setPlaceholderText(!ready && !targets.isEmpty() ? tr("Loading metadata…")
                : mixed ? tr("Mixed values") : row.field.isEmpty() ? tr("Comma-separated keywords") : QString{});
            row.input->setToolTip(row.field.isEmpty()
                ? tr("Shared keywords only. Adding or removing a keyword preserves each photo's other keywords.")
                : tr("Changes apply to all selected photos."));
        }
    }
    void failed() { for (auto& row : rows_) row.pending = false; }
};
}
